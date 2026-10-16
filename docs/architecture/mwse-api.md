# MWSE API contract

The versioned C++ vtable and script opcodes MGE XE exposes to MWSE. This is a cross-repo,
cross-binary interface: MWSE ships on its own schedule against a compiled copy of it, so none of
the hazards below are caught at MGE build time. Both sides are at API version 3; treat `api.h` as
frozen-by-slot (see [Append-only](#append-only)). MWSE-side paths refer to the `MWSE/MWSE`
repository. This page cites functions and types, not line numbers, so that it does not drift.

## Direction

MGE XE is the provider of the API *object* but the consumer of MWSE's *exports*. MWSE
exports `MGEInterface`, `MWSEGetVM`, `MWSEAddInstruction` (the `extern "C"` block of
`MWSE/main.cpp`, cdecl). MGE XE `GetProcAddress`es all three off MWSE.dll, allocates
`new api::MGEAPI_ExportVersion()` and pushes it in through `MGEInterface`, if MWSE exports it
(`MWSE_MGEPlugin::init` in `mwse/mgebridge.cpp`).

MGE XE loads MWSE.dll itself in `DllMain` (`main.cpp`), before distant land exists. It calls
`MWSE_MGEPlugin::init` only when MGE XE is not disabled and either the proxy-only mode is off
or the wgpu renderer is on (`!OnlyProxyD3D8To9 || RendererWgpu`). So plain proxy-only mode
gives MWSE no API object; the wgpu renderer does (see [On the wgpu renderer](#on-the-wgpu-renderer)).

## File map

- `mge/api.h`: the live interface, namespace `api`, `supported_api_version = 3`
- `mge/api.cpp`: implementations, for example `MGEAPIv1::getAPIVersion` and `getMGEVersion`
- `mwse/mgebridge.cpp`: export resolution, opcode registration, 0.9.4a fallback
- `mwse/mwseinstruction.h`: instruction base; hardcoded TES3MACHINE vtable offsets
- MWSE `MWSE/MGEApi.h`: MWSE's copy of the interface, namespace `mge`
- MWSE `MWSE/main.cpp`: `MGEInterface`, the version gate
- MWSE `MWSE/MGEApiLua.cpp`: `CoreInterface::getVersion`, the only vtable consumer of
  `getMGEVersion`

## Version negotiation

Two version numbers exist and they do different jobs.

`getAPIVersion()` returns `api::supported_api_version` (3). This is the gate of the vtable.
MWSE accepts anything `>= 1` and stores it as `mge::apiVersion`, then feature-gates on it
(`bindMGEUtil` in `MGEUtilLua.cpp`: the v2 and v3 parts).

`getMGEVersion()` returns `MGE_MWSE_VERSION`. MWSE shows it in `mge.getVersion()` for Lua,
and MWSE's dependency manager compares it: `checkDependency` in
`misc/package/Data Files/MWSE/core/lib/dependencyManagement/types/mge-xe.lua` formats
`mge.getVersion()` as `major.minor.patch` and compares it with the
`[dependencies.mge-xe] version` of a mod. A mod that needs a newer MGE XE is refused with
"out of date".

MWSE computes the version table as:

```cpp
int ver = api->getMGEVersion() - 0x40000;   // MGEApiLua.cpp, CoreInterface::getVersion
```

The `0x40000` hardcodes `MGE_MAJOR_VERSION == 4` in MWSE's source. The legacy path
(`lua_mge["getVersion"]` in MWSE `ScriptUtilLua.cpp`) reads the same number through opcode
0x3703 and subtracts the same constant. Bumping `MGE_MAJOR_VERSION` in `mgeversion.h` does not
break the vtable, but it shifts the version that MWSE reports by one major, and so it changes
the result of every mod's MGE XE version check. That is the reason to leave
`MGE_MAJOR_VERSION` alone. The current version is 4.21.0, which Lua sees as 0.21.0.

There is no hard failure keyed to the vtable API. MWSE's only hard gate is a file check
on `MGEXEgui.exe`'s version resource (`DllMain` in MWSE `main.cpp`, with
`loadMGEGUIVersion` in `MGEApi.cpp`): missing or below 0.10.0.0 -> message box and
`exit(0)`.

## Append-only

`MGEAPI` (2 methods) <- `MGEAPIv1` <- `MGEAPIv2` <- `MGEAPIv3`, with
`typedef MGEAPIv3 MGEAPI_ExportVersion` (`api.h`). Each version subclasses the last
and adds only tail slots, so layout is append-only by construction:

- v2 adds `saveScreenshot`, `weatherScatteringSkylightGet/Set`
- v3 adds `nearRenderDistanceGet/Set` and six `shaderGet/Set{Int,Bool,Vector}Array`

Inserting, reordering, or changing the signature of any existing virtual shifts vtable
slots against MWSE's already-compiled copy. Nothing catches it at MGE build time; it
fails as a wrong-function call in a user's game.

## One live declaration

`mge/api.h`, namespace `api`, is the only interface declaration in this tree. A declaration in
namespace `mge` is MWSE's own copy or a stale duplicate of an older ABI, never the live interface.

## Opcodes

102 registrations in `MWSE_MGEPlugin::init` (`mwse/mgebridge.cpp`). The range is not
contiguous and is wider than the `0x3Axx` block: `0x3700-0x3777`, `0x3A00-0x3A8D`,
`0x3AB0-0x3AB4`, `0x3AC0-0x3AC2`, `0x3AE0-0x3AEB`, plus a single outlier `0x3f11`
(`mwseSetOwner`).

By handler unit: gmst `0x3A00-01`; entity `0x3A02-09` + `0x3f11`; mwui `0x3A10-11`;
weather `0x3A80-8D`; general `0x3700-05,07`; hud `0x370A-1E` + `0x3AC0-C2`; input
`0x3733-3B`; camera `0x3756-5A`, `0x3768-77`; shader `0x3AB0-B4`; physics `0x3AE0-EB`.

Opcode numbers are a hard compatibility surface. They are the integers baked into
compiled Morrowind script bytecode and, through the save system, into savegames.
Renumbering breaks installed mods and existing saves. Nothing checks this.

## MWSE 0.9.4a fallback is live code

If `MWSEGetVM` and `MWSEAddInstruction` are absent, MGE XE assumes MWSE 0.9.4a and
steals the VM global at `dll+0x595cc` and `AddInstruction` at `dll+0x38950`
(`MWSE_MGEPlugin::init`), substitutes a thiscall shim, then runs `fixMWSE94Problems`,
which reinterprets a breakpoint array at `dll+0x56900` and re-protects nine patch pages
(`mwse/mgebridge.cpp`).

This is reachable in current builds. It triggers on which MWSE is installed, not on a
build flag. `mwseinstruction.h` additionally carries hardcoded TES3MACHINE vtable offsets
(0x34, 0x38, 0x3C, 0x6C) and reads the target-ref global `TES3::Address::scriptTargetRef`
(`0x7CEBEC`, defined in `tes3/tes3addresses.h`).

## What breaks silently

Every hazard here is an MGE-XE edit read by MWSE's precompiled header, so none of it can
be caught at MGE build time:

- Insert/reorder/re-sign any virtual in v1/v2/v3 -> vtable slot drift.
- Change `DistantLandRenderConfig` layout -> MWSE reads wrong offsets (the
  `mgeDistantLandRenderConfig` usertype in `bindMGEUtil`, `MGEUtilLua.cpp`, binds by
  member). MGE's own `static_assert` in `api.cpp` checks parity with `Configuration.DL`
  only, not with MWSE.
- Insert into the middle of `RenderFeature` -> the enum indexes `featureToFlagMap`
  (`api.cpp`) positionally; every later feature toggle changes meaning.
- Reorder/remove `MacroFunctions` members -> MWSE binds them by offset.
- MWSE's `ShaderHandleLua::setVariable` (`MGEPostShaders.cpp`) static_casts the API to
  `MGEAPIv3` with no `apiVersion` check. Safe today only because MGE always constructs a
  full v3. A bump of `supported_api_version` alone does not change this; the hazard of a
  bump is a later MWSE that gates on `apiVersion >= 4` and calls slots that this MGE XE
  does not have.

The reverse direction, MGE newer than MWSE, is safe because v3 is a superset and older
MWSE never touches the new slots. There is no cross-binary layout test anywhere.

## Fields read only at load

`getDistantLandRenderConfig()` hands out a raw pointer to the whole `Configuration.DL`
struct. MWSE binds most fields as writable properties; `wind`, `fogDistance` and
`fogOffsetPercent` are read-only, and `ShadowResolution` is not bound. Some writable fields
are read only when distant land loads and ignore later writes: `farStaticMinSize` and
`veryFarStaticMinSize` decide which statics the loaded data has.

A write through the raw C++ pointer can do worse. `ShadowResolution` sizes the shadow
textures once (`DistantLand::initShadow`) and the shader's reciprocal resolution once, but
the shadow passes (`renderShadowLayer`, `renderShadowLayerGeneric`) read it every frame for
the viewport. A late write makes the viewport disagree with the texture. See
[shadows.md](shadows.md).

Narrowing the exposed struct would fix the whole class, but it is an ABI change MWSE
reads by member offset, so it is not a local edit.

## On the wgpu renderer

With `[runtime] renderer_wgpu = true`, MGE XE makes no Direct3D device. The `nirender` MWSE
plugin gives the game its Direct3D 8, and `numidium.exe` draws the picture (see
[RENDERER.md](../../RENDERER.md)). The API object is still pushed to MWSE, and MWSE binds the
whole `mge` table. The calls work on the state in `d3d8.dll`; the namespace `Deviceless`
(`mge/mged3d8device.cpp`) sends that state to the helper as records each frame:

- record 1, the environment (fog, sun, weather, scattering, flags of the camera);
- record 2, the shader state (the chain, the enabled flags, the variables), on a change;
- record 3, a picture request (`saveScreenshot`, the screenshot macro);
- record 4, the settings (`MGEFlags`, the distances, water, the lighting mode), on a change;
- record 9, the status text, after a new text.

`DistantLand::init` returns false without a device, so `canRenderDistantLand()` is false and
the distant land of `d3d8.dll` stays unloaded. The helper loads and draws the distant land.

| Call (Lua `mge.*`) | On the wgpu renderer |
| --- | --- |
| `getVersion`, the API version | as on Direct3D 9 |
| `loadConfig`, `saveConfig` | work; the next settings record carries the values |
| `reloadDistantLand` | returns true and does nothing; the helper takes the settings on the next frame |
| `render.*` features: shaders, shadows, grass, distantStatics, updateHDR, blurReflections, dynamicRipples, reflectiveWater, reflectNearStatics, reflectSky | followed (settings record) |
| `render.*` features: exponentialFog, atmosphericScattering | followed (environment flags) |
| `render.distantLand` | refused before the first frame; after it, followed only if the helper loaded the land at the start |
| `render.*` features: fpsCounter, displayMessages | followed: the helper draws the status text and the frame rate on the window (not in screenshots, as on Direct3D 9) |
| `render.pauseRenderingInMenus` | followed: the helper keeps the picture of the first menu frame and does no distant land or post work until a click (`renderer/menu_keep.rs`) |
| `render.*` features: reflectInterior, distantWater, transparencyAA, noMWMGEBlending, noMWSunglare | the flag is stored, the helper does not use it (sun glare: each post shader's own `disable_sunglare`) |
| crosshairAutoHide, UI scale, lighting mode | work |
| `distantLandRenderConfig`: draw distance, static ends, caustics, wave height, interior fog end | followed (settings record) |
| `distantLandRenderConfig`: fog and wind; `weather` fog, per-pixel lighting, wind, scattering, skylight scattering | followed (environment record) |
| `distantLandRenderConfig`: farStaticMinSize, veryFarStaticMinSize | read when the land loads only, as on Direct3D 9 |
| `camera`: fov, zoom (all calls), shake, thirdPersonOffset, nearRenderDistance | work (`Deviceless::transform`, `updateFrameLogic`) |
| `shaders.hdrReactionSpeed` | followed |
| `shaders.find`, `list`, `load` | placeholders parsed from the `.fx` of the chain; the helper runs the WGSL port; nil if the `.fx` is missing |
| shader `enabled`, `orderPriority`, number variables (bool, int, float, vector, matrix, arrays), `reload()` | work (shader record; `reload()` makes the helper reload its WGSL) |
| shader string variables | work (the store keeps the text; the helper does not get it) |
| old calls `enableShader`, `disableShader`, `setShaderFloat`, `setShaderLong` (opcodes 0x3AB0-0x3AB4) | write to the placeholder store |
| `saveScreenshot` | the helper writes the picture; a request before the first frame waits for it |
| `macros.takeScreenshot` (and PrintScreen) | the helper writes the picture, with the name, folder and format of the settings |
| `macros.toggleDistantLand` | does nothing |
| `macros.toggleStatusText`, `toggleFpsCounter`, `showLastMessage` | work (record 9 and the settings record) |
| the camera macros of the third person view | work (tested 2026-10-15) |
| HUD calls | do nothing (no device; MWSE does not bind them) |

The FFI exports outside the vtable (`MGE_HorizonGet/SetParam`, `MGE_WaterVolumesSet`,
`MGE_WaterLookSet`, `MGE_WaterMasksSet`, `MGE_WaterDrySet`, `MGE_DistantLandReadyNext`, in
`mge/apiffi.cpp` and `exports.def`) work on both renderers; the water ones reach the helper
as records 6 and 7.

## The event distantLandReady

MGE XE triggers the MWSE event `distantLandReady` on both renderers (since 2026-10-15):

```lua
event.register("distantLandReady", function(e)
    -- e.reason == "loaded": the distant land of the session is loaded (once).
    -- e.reason == "arrived": after a game load or a jump of the camera of more than 2 cells,
    -- the distant statics around the camera are there, or the wait ended after 10 s.
end)
```

The renderer queues the reasons in `d3d8.dll` (`mge/distantlandready.h`): Direct3D 9 when
its upload pump is done and when the residency is quiet after a load or a jump; the wgpu
renderer through message 1 of its helper ([wgpu-renderer.md](wgpu-renderer.md), "Messages").
`MGE XE Options/events.lua` polls `MGE_DistantLandReadyNext` at `enterFrame` and triggers the
event, so it needs that mod. A "loaded" before a mod registers its handler is not given
again. The event is not an MWSE event: it has no entry in the MWSE documentation.
