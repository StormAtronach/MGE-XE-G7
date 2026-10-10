# Render link

The render link is a C interface between `d3d8.dll` (the host) and a plugin that knows what
the game draws (the client). The client is the nirender plugin, a Rust replica of the
`NiDX8Renderer` subsystem of Morrowind.exe that MWSE loads.

MGE XE infers what the game draws from the D3D8 calls: it counts scenes, tests the shape of
the view matrix, reads marks in materials. The client has these facts directly, because it
runs at the renderer functions of the game: it gets the camera at `BeginPaint` and the
geometry at `RenderShape`. Through the link it tells the host the scene and the object of
the D3D8 calls that follow.

The plan is in `moreFPS/docs/plans/renderer-mge-harmonization-plan.md`. The inventory of the
inferred facts and their direct sources is in
`moreFPS/docs/engineering-notes/renderer/harmonization-phase-a-inventory.md`.

## State

The host compares only. At each place where MGE XE infers a fact, it counts if the packet
says the same. Nothing that MGE XE draws depends on a packet. Without a client, each
function of `RenderLink` returns at once.

## Files

| File | Content |
| --- | --- |
| `d3d8/cpp/mge/mge_render_link.h` | The contract. A copy, byte for byte, of `contract/mge_render_link.h` of the nirender repo |
| `d3d8/cpp/mge/renderlink.h`, `renderlink.cpp` | The host: the table of functions, the current scene and draw packet, the counters |
| `d3d8/cpp/exports.def` | The export `MGE_RenderLinkConnect` |

## Contract

- The client calls `MGE_RenderLinkConnect(version)` one time and gets a table
  (`MgeRenderLinkHostV1`): its size, the version of the host, capability bits and five
  functions.
- `sceneBegin(MgeLinkSceneV1)` comes before the `BeginScene` of a scene. It has the scene
  kind (world, sorted alpha, water, shadow volume, shadow casters, first person, UI, ...),
  the camera, the scene root and the render target.
- `sceneEnd()` comes after the `EndScene`.
- `draw(MgeLinkDrawV1)` comes before the D3D8 calls of one object. It has the draw kind, the
  class of the object (sky dome, clouds, moon shadow, water plane, ripple, landscape, object,
  ...), the geometry, the property state and effect state, the shininess of the material,
  and the lights of the effect state with their device index and type.
- `frameEvent` reports `ClearBuffer`, `SetRenderTarget` and `SwapBuffers`.
- `readCounters` copies the comparison counters.

Each struct starts with its size. A new version adds members at the end and a capability
bit. The host copies only the bytes that both sides know.

Every function runs on the render thread. A pointer in a packet is valid until the function
returns. An address of a game object is valid until the scene ends.

## The comparison

| Fact | Inference of MGE XE | Packet | Place |
| --- | --- | --- | --- |
| UI scene | `!isMainView` at `BeginScene` (`detectMenu`) | scene kind is UI, splash or race menu | `MGEProxyDevice::BeginScene` |
| Scene kind | the scene count is 0 | scene kind is world | `MGEProxyDevice::BeginScene` |
| Water plane | material power 99999 | class is water plane | `MGEProxyDevice::DrawIndexedPrimitive` |
| Sun light | light index 6 | the directional light of the draw | same |
| Light radius | solved from the attenuation | radius in the light object | same |
| Sky draw | blended, scene 0, nothing recorded yet | class is a sky class | `DistantLand::inspectIndexedPrimitive` |
| Moon shadow | material mark 88888 | class is moon shadow | same |
| Land splat | same vertex buffer again, blended | class landscape, and not the first D3D8 draw of the packet | same |

Each fact has four counters: both yes, both no, only MGE XE, only the packet. The first 12
differences of a fact go to `mgeXE.log` with the name of the object.

## Results

Run on 2026-10-10 with `tools/run_link.py` of the nirender repo: Vivec, Balmora, Seyda Neen,
an interior, Old Ebonheart, Ald-ruhn. About 6 seconds in each place with the game code and
6 seconds with the Rust code. The two modes gave the same result.

- UI scene, scene kind, water plane, sun light, sky draw, land splat: no difference in
  about 1.5 million comparisons for each draw fact.
- Moon shadow: no difference, but no moon shadow was in view (always "both no"). The test
  must run again at night.
- Water plane: the first run showed draws that only the packet called water. They were the
  ripple shapes of the game ("Water Ripple N"), which are below the water plane node. The
  contract has a class for them now.
- Light radius: different for some lights in Ald-ruhn. Example: light 147 with attenuation
  0, 0.02, 0. MGE XE solves 150 (3.0 / 0.02). The packet says 512. Cause: the game keeps
  the radius that `game_dynamicLightTest` gets in the specular colour of the light, and
  `patchLightAttachRadius` of MGE XE raises that argument to the fade cutoff plus a margin.
  So with light fade on, the value in the light object is the attach radius, not the
  radius of the record. The shim `lightAttachRadius` gets the light and the radius of the
  record. It is the place to keep the true radius, with the light pointer of the packet as
  the key.

## What comes next

Phase D of the plan: one fact at a time, behind a switch, MGE XE takes the fact from the
packet. The comparison stays as the test.
