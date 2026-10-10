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

At each place where MGE XE infers a fact, it counts if the packet says the same. The
client can tell the host to take a fact from the packets (`useFacts` with a mask). The
default is the inference. Without a client, each function of `RenderLink` returns at once
and `resolve` returns the inference.

Facts that the host can take from a packet: UI scene, scene kind, water plane, sun light,
sky draw, land splat, moon shape.

When the water plane comes from the packets, the host takes its mark (shininess 99999) out
of the material of the water, and puts it back when the inference is in use again.

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
- `sceneBegin(MgeLinkSceneV1)` comes before the view transform and the `BeginScene` of a
  scene. The client sends it at `SetCameraData`, because the proxy decides on the scene when
  the view transform arrives. It has the scene
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
| Moon shape | material mark 88888 | class is moon or moon shadow | same |
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

## Use of the facts

`RenderLink::resolve(fact, inferred, fromPacket)` counts the comparison and returns the
answer to use. `RenderLink::resolveMainView` does the same for the view transform of a
scene. Only the first view of a scene comes from the packet. A later view in a scene (a
screen polygon, the sun glare) still goes through `detectMenu`.

The water mark is not necessary with the water plane fact: `SetMaterial` and the draw take
the answer of the packet, and the tint of the water plane comes from the material of that
draw. With the fact in use, the material of the water has its own shininess again (4 in the
test), the inference finds no water, and the picture is equal.

The moon mark is in the material of the two shapes of a moon: the face, and the shape that
hides the dark part. So the fact is "a shape of a moon". The proxy tells the two apart by
the blend mode at the replay, as before. With the fact in use the host takes the mark out
of the two moon materials, as it does for the water. In the night test the inference then
found no moon, the packet found the two shapes in each frame, and the picture was the same
but for 3 pixels that differ by one level of one colour (the night scene has that much
noise between two frames without the facts).

The scene count of the proxy comes from the scene kind: the world scene is 0, and each
other world view scene is 1 or more.

The radius of a light is compared and not used. The game keeps a radius in the light object
(the argument of its attach test), and `lightAttachRadius` tells the host the radius before
the light fade raised it. For the lights of light records this radius is equal to the one
that the proxy solves from the attenuation. A light of a mod can be different: the campfire
of Ashfall has an attenuation for a radius of 150 and an attach radius of 512. The fade
needs the radius that agrees with the attenuation, so the inference is the correct source.

Test of 2026-10-10 (`tools/run_link.py --facts all`): with the game in menu mode, three
pictures of the frame: facts off, facts on, facts off.

| Place | Result |
| --- | --- |
| Balmora, Seyda Neen, Ald-ruhn | the picture with the facts is equal, byte for byte, to a picture without. The water mark was out of the material for the picture with the facts |
| Balmora, Guild of Mages | equal |
| Vivec | equal in one run, no result in the others: the scene moves in menu mode |

The place of the test save, "Vivec, Temple", is under water: the player stands in a water
volume of a mod. So each Vivec row of the tests is the underwater case. The caustics move in
menu mode there, which is why its picture test often has no result, and the sun and its
glare are not in view from there.

A mod can add things that move in menu mode. The frost breath of Ashfall did, and it is off
in the test install.

The game draws a moon only when it is in the view, above the horizon, and not hidden by the
weather. `tools/run_link.py --moon masser` goes through the hours of the night until the
game shows the moon, sets clear weather, and keeps the camera on the moon. In that run the
moon fact was equal on each draw (two moon shapes in a frame).

With the facts in use the game ran in each place, and the counters stayed equal.

## What comes next

- The later views of a scene. `detectMenu` also runs for each view transform after the
  first one of a scene (a screen polygon, the sun glare). The host counts these as the fact
  "later view" and does not use the packet for them. The count stayed 0 in all runs. It
  also stayed 0 in a run with the sun glare of the game on the screen: Sunshafts out of the
  shader chain (that shader makes MGE XE turn the glare off), clear weather at noon, the
  player high above Seyda Neen, the camera near the sun, the glare shape drawn in each
  frame. The UI scene fact and the scene kind fact were equal in each scene of that run. So
  no case of a later view is known. The glare shape is an ordinary shape draw of the arm
  scene.
- The removal of an inference from the source, one at a time, when a client is a
  condition of the build.
