# The wgpu renderer of MGE XE G7 (test build)

This build of MGE XE G7 carries a second renderer. With it on, the game draws nothing with
Direct3D: the `nirender` MWSE plugin gives the game a Direct3D 8 of its own, turns each
draw of the game into a record, and the program `numidium.exe` draws the records with
wgpu (Vulkan or Direct3D 12) into the window of the game, together with the distant land,
the water, the shadows and the post shaders of MGE XE.

The names (2026-10-17): Numidium is the helper (`numidium.exe`, `mgeHost64.exe` before) and
Akulakhan the plugin (`akulakhan.dll`, `nirender.dll` before). The Lua module of the plugin is
`akulakhan`; `nirender` stays an alias of it once the plugin's mod has loaded.

It is a test build. Read the known issues before you report one.

## The switch

The new renderer is on by default. With a new settings file, you do not have to set
anything.

MGE XE does not change a settings file that you have. If you used an earlier test build,
your `mgeXE.toml` has the line `renderer_wgpu = false`, because those builds wrote it. Then
the new renderer is off, and a message in the game tells you. To turn it on, change the
line below `[runtime]` in `mgeXE.toml`:

```toml
[runtime]
renderer_wgpu = true
```

The line must be below `[runtime]`. Below `[render]` it has no effect, and `mgeXE.log` has
a warning about an unknown key.

To go back to MGE XE on Direct3D 9, set the line to `false`. The plugin is then idle.

This archive does not have the `d3d9.dll` (DXVK) of MGE XE G7, and the new renderer does
not use it. MGE XE on Direct3D 9 (`renderer_wgpu = false`) runs with the `d3d9.dll` of
Windows; for MGE XE G7's own DXVK build, take `d3d9.dll` from the MGE XE G7 archive. Do
not use an official DXVK release: it runs out of memory.

How to know that the new renderer is in use:

- `mgeXE.log` has the line `>> Using the wgpu renderer: no Direct3D device`.
- `MWSE.log` has a line `[nirender] ... mode live`.
- The file `nirender-host.log` is in the Morrowind folder.

The renderer draws with Vulkan or with Direct3D 12. It takes the first choice for the
machine; to name one, add a line under `[runtime]`:

```toml
renderer_backend = "dx12"
```

The values are `auto`, `vulkan` and `dx12`. The first lines of `nirender-host.log` name the
adapter and the one in use.

The renderer keeps the textures of a frame in one list on the graphics card. Its room is
`renderer_texture_slots` under `[runtime]`: `0` (the default) lets the renderer choose, what
the card allows and at most 16384. If some textures of a place draw white and
`nirender-host.log` has the line `texture slots: the room of ... is full`, raise it:

```toml
renderer_texture_slots = 32768
```

The F10 panel shows the slots in use in the line `textures`.

## Install, upgrade and uninstall

Written for a player who installs mods with Mod Organizer 2 and has not seen this project
before. The archive is a whole MGE XE G7 with the renderer; `MGE XE Readme.md` has the
requirements of MGE XE G7 itself.

### What is in the archive

The files go into two places:

- The Morrowind folder, the one with `Morrowind.exe`: `d3d8.dll`, `dinput8.dll`,
  `MGEXEgui.exe`, `numidium.exe`, `MWSE.dll`, the folder `mge3`, and the readme and
  license files.
- `Data Files`: the shaders of MGE XE (`shaders\core`, `shaders\XEshaders`,
  `shaders\core-mods`), its textures (`textures\MGE`) and meshes (the `_dist.nif` files
  in `meshes\x`, `meshes\f` and `meshes\i`), `XE Sky Variations.esp`, the plugin
  (`MWSE\lib\akulakhan.dll` and `MWSE\mods\nirender`), the options mod of MGE XE
  (`MWSE\mods\MGE XE Options`), and True Water (`MWSE\lib\watervolumes.dll`,
  `MWSE\mods\waterVolumes`, `meshes\wv`, `meshes\wvs`, `meshes\wvm`, `meshes\wvb` and
  `Water Volumes Kit.esm`; its own readme is in the archive).

Install the whole archive: the plugin and `numidium.exe` must come from the same one.

### Before you install

- You need MWSE 2.1 and the Morrowind Code Patch (see "Requirements" below).
- Make a copy of your `MWSE.dll`. The archive has its own `MWSE.dll`, which replaces yours:
  the renderer needs a part of MWSE that the public MWSE does not have yet. Do not run
  `MWSE-Update.exe` while you use this build: it puts the public `MWSE.dll` back, and the
  renderer turns off.
- MSOC (the occlusion culling mod for MWSE): switch it off.

### Install into the Morrowind folder (no mod manager)

1. Extract the whole archive into the Morrowind folder, overwriting when asked.
2. Run `MGEXEgui.exe` from the Morrowind folder: set the resolution and the graphics
   options on the Graphics tab, and generate the distant land on the Distant Land tab
   (once; see "A new settings file" for the automatic generation after that).
3. Start the game. "The switch" above says how to see that the renderer is in use.

### Install with Mod Organizer 2

Mod Organizer 2 gives the game only `Data Files` through its virtual folder. The files of
the Morrowind folder must be in the Morrowind folder itself, in one of two ways:

- With the Root Builder plugin of Mod Organizer 2 (its copy mode, which is the default):
  make one mod from the archive. In the mod, put the files of the Morrowind folder into a
  folder named `Root` (`Root\d3d8.dll`, `Root\dinput8.dll`, `Root\MGEXEgui.exe`,
  `Root\numidium.exe`, `Root\MWSE.dll`, `Root\mge3`), and the content of `Data Files` at
  the top of the mod (`shaders`, `textures`, `meshes`, `MWSE`, the esm and the esp). Root
  Builder copies `Root` into the Morrowind folder each time you start the game, and takes
  it out again after. Files that the game or the GUI write into the Morrowind folder
  (`mgeXE.toml`, the logs, a capture, a crash dump) are then in `overwrite\Root` of the
  instance after the game has ended.
- Without Root Builder: extract the files of the Morrowind folder into the Morrowind
  folder by hand (your copy of `MWSE.dll` first), and make a mod of `Data Files` only.

In both ways, add `MGEXEgui.exe` as an executable of Mod Organizer 2 and run it from
there, so that the distant land generator sees your mods. Check that the mod has the
`.dll` files (`MWSE\lib\akulakhan.dll`, `MWSE\lib\watervolumes.dll`): a mod manager that
leaves them out gives you MGE XE on Direct3D 9 without the distant land, and `MWSE.log`
has the line `[nirender] akulakhan.dll not loaded. If you use a mod manager, check that it
did not leave .dll files out.`

### The settings you may need

`mgeXE.toml` in the Morrowind folder. A new file has the right values; a file you have keeps
yours. Each key goes below its section line.

| Key | Section | Set it to | When |
| --- | --- | --- | --- |
| `renderer_wgpu` | `[runtime]` | `true` | a file from an earlier test build has `false`; `false` goes back to Direct3D 9 |
| `renderer_probe` | `[runtime]` | `true`; `false` skips the probe | the probe said no but you want to try the wgpu renderer; a card that cannot run it then stops the game with the reason |
| `renderer_backend` | `[runtime]` | `"auto"`, `"vulkan"` or `"dx12"` | to name the backend; a window capture of the game (OBS) needs `"dx12"` |
| `renderer_texture_slots` | `[runtime]` | `0` (automatic); a larger number only if `nirender-host.log` says the room is full | some textures draw white; see "The switch" above |
| `automatic_rebuild` | `[distant_land]` | `true` | the distant land is made again at the start when your mods or the data version change; a new file has it |

### Upgrade from an earlier test build

- From v0.2.11 or later: extract the whole archive over the old files, overwriting. Every
  file name of an archive since v0.2.11 is in the later archives too (True Water came in
  with v0.2.15), so there is nothing to delete. With Mod
  Organizer 2, replace the content of the mod. Read the release notes of the new build:
  they say when the distant land must be generated again, when a setting changed, and
  whether `MWSE.dll` changed.
- From a build before v0.2.11: the plugin and the renderer had other names. Delete
  `Data Files\MWSE\lib\nirender.dll` and `mgeHost64.exe` from the Morrowind folder (or
  from the mod), then install as above.
- Your `mgeXE.toml` keeps its values. "The switch" above says what an older file needs.

### Uninstall

To use MGE XE on Direct3D 9 and keep the files: set `renderer_wgpu = false`.

To remove the archive:

1. From the Morrowind folder: `d3d8.dll`, `dinput8.dll`, `MGEXEgui.exe`, `numidium.exe`,
   `mgeXE.toml`, `mge3\MGE XE Default Statics Classifiers.toml`, the readme and license
   files of the archive, and the files the renderer wrote: `mgeXE.log` and
   `nirender-host.log` (each with its `.1`, the log of the run before), `nirender.log` (from
   an older build),
   `mgeXE-shaders.txt` and the folder `nirender-captures` (the captures of the F11 key).
2. `MWSE.dll`: put your copy back, or run `MWSE-Update.exe` to get the public MWSE back.
3. From `Data Files`: `MWSE\lib\akulakhan.dll`, `MWSE\mods\nirender`,
   `MWSE\config\nirender.json` (if it exists), `MWSE\mods\MGE XE Options`,
   `shaders\core`, `shaders\XEshaders` and `shaders\core-mods` (the files of the archive;
   keep shader mods of your own), `textures\MGE`, the `_dist.nif` meshes of `meshes\x`,
   `meshes\f` and `meshes\i`, `XE Sky Variations.esp`, the generated `distantland` folder,
   and True Water: `MWSE\lib\watervolumes.dll`, `MWSE\mods\waterVolumes`, `meshes\wv`,
   `meshes\wvs`, `meshes\wvm`, `meshes\wvb` and `Water Volumes Kit.esm`.
4. With Mod Organizer 2: remove the mod; the `Root` files go with it. `overwrite\Root` may
   still hold `mgeXE.toml`, the logs and captures.

## A new settings file

A new `mgeXE.toml` (MGE XE writes one when there is none) starts with the settings that we
test with: the distant land and the distant statics on, the draw distance 7 cells,
per-pixel lighting on, and the post shaders SSAO Fast, Sunshafts and Bloom Fine. A file that
you have keeps your values.

Generate the distant land once in the MGE XE GUI (`MGEXEgui.exe`, the Distant Land page).
After that, `automatic_rebuild = true` (on in a new file) generates it again at the start
of the game when the load order, the generation settings or the version of the data
change. That start can take some minutes. With an older file, set it below
`[distant_land]`, or generate again in the GUI after you change your mods.

## Requirements

- Everything that MGE XE G7 needs (see `MGE XE Readme.md`): a GPU with Vulkan 1.3, 64-bit
  Windows 10 or 11.
- MWSE 2.1 with the native ABI for DLL modules. Public MWSE does not have it yet, so this
  test build ships an `MWSE.dll` in its Morrowind folder: install it over yours. With an
  MWSE.dll without the ABI the renderer is refused for the session, the game runs on
  Direct3D 9, and `mgeXE.log` says "the MWSE.dll of the game has no native ABI". Do not
  run `MWSE-Update.exe` while you test this build: it puts the public MWSE.dll back. The
  plugin loads as an MWSE plugin (`Data Files/MWSE/lib/akulakhan.dll`,
  `Data Files/MWSE/mods/nirender/main.lua`); with a mod manager, check that it does not leave
  `.dll` files out.
- The Morrowind Code Patch, 2.5 beta 19 or later as shipped with recent MWSE. The plugin
  replaces 301 functions of the game's renderer with its own code and follows the changes
  that the Code Patch makes in them. At the start it compares the code of each function
  with its database; a change that it does not know turns the renderer off for the session,
  and the game runs on Direct3D 9 through MGE XE's proxy, without the distant land for that
  session. The message box after the load says so; the reasons are in `mgeXE.log`. Set
  `renderer_wgpu = false` to get MGE XE with its distant land back until the cause is
  known.
- Another mod that changes the renderer code of the game (not its Lua) has the same effect.
- A graphics card without the binding arrays of textures draws with texture tier C: each
  draw binds its own textures. The log names the tier (`texture tier: a` or `c`).
- A graphics card or a driver that lacks what the renderer needs (the BC texture
  compression, 32 sampled textures in a shader stage, or an adapter at all on Vulkan and
  Direct3D 12): before the game starts, MGE XE
  runs `numidium.exe --probe`, which asks the backends and answers in about a third of a
  second. When the answer is none, a box says what is lacking once, the game runs on
  Direct3D 9 without the distant land for that start, and `mgeXE.log` has the probe's
  lines. A probe that crashes counts as none, with its exit code in the log. A probe that
  gives no answer in 5 s is ended, and that start tries the wgpu renderer (a card that cannot
  run it then stops the game with the reason); the next start asks again, and a second "no
  answer" in a row counts as none, with a box that says so. MGE XE keeps the answer in
  `mgeXE-renderer-probe.txt` beside `mgeXE.log`, so the next starts do not ask again: after
  a none they run all of MGE XE on Direct3D 9, with no box, until the graphics card, its
  driver or `numidium.exe` change. To ask again, delete that file. To use the wgpu renderer
  without the probe, set `renderer_probe = false` under `[runtime]`; a card that cannot run
  it then stops the game at its start with the reason.
- When `numidium.exe` or `MWSE.dll` is not in the Morrowind folder, MGE XE runs on
  Direct3D 9 as without the switch, and `mgeXE.log` names the file. A virus scanner can
  take a program away, and an archive can be extracted in part.
- When the plugin `Data Files/MWSE/lib/akulakhan.dll` is missing, the game runs on
  Direct3D 9 through the proxy for that session, without the distant land, and `mgeXE.log`
  says so.
- The same fallback applies when the Lua side of the plugin does not run (the Lua of MWSE
  failed to start, or `Data Files/MWSE/mods/nirender/main.lua` is missing): the game runs
  on Direct3D 9 through the proxy for that session, and `mgeXE.log` says why.

## What you get

- The picture of the game as the game draws it, with MGE XE's distant land, statics,
  grass, water, shadows and sky, and the post shader chain of `mgeXE.toml`.
- Every stock post shader has a port; a mod can ship a WGSL shader (see
  `MGE XE Post Shaders Readme.md`). Three shaders of this build are new: `Bloom S_A`,
  `SSAO S_A` and `Depth of Field S_A`; name them in the chain like the others.
- The frame rate of the game thread goes up where the Direct3D 9 driver was the limit.
- Multisampling as the Direct3D 9 path has it: the "Anti-aliasing" level of the MGE XE
  settings (`graphics.anti_aliasing`) gives the samples of each pixel of the world, and
  "Transparency anti-aliasing" (`graphics.transparency_antialiasing`) gives the grass and the
  distant statics alpha to coverage. The post shaders read the resolved picture and the
  nearest depth of each pixel. With "none", every pixel has one sample. On a card without the
  adapter's own format features, 8 samples run as 4 and 2 as 1 (the log line
  `samples: asked 8, using 4`).
- A setting that a mod or the MGE XE options change while the game runs (the draw distance,
  the ends of the static tiers, the water options, the caustics, the lighting mode, the speed
  of the HDR) reaches the renderer at once.
- The dynamic waves of the water (`dynamic_ripples`): the rings of rain and of the player,
  and the height of the waves. The blur of the reflection of Direct3D 9 (`blur_reflections`);
  with it, a new blur that stays sharp where an object meets the water
  (`blur_contact_hardening`, on by default).
- Parameters of a shader: the shader list of the MGE XE options has a Parameters button on
  each row. It opens an editor for each number of that shader, a slider where a mod gives
  the range in `Data Files/shaders/XEshaders/<name>.toml` (see
  `MGE XE Post Shaders Readme.md`). The row
  "water (renderer)" has the numbers of the blur of the reflection. A change lasts until the
  game ends.
- Reload of a shader when its file changes (an option of the MGE XE options, off at the
  start): for an author who edits a `.fx` or a `.wgsl` beside the game. The result, with the
  error of a shader that does not compile, is in the Parameters window and in
  `mgeXE-shaders.txt` in the Morrowind folder.
- The post shaders run also when the distant land is off. Outside the distant land they get
  the fog of the game, as on Direct3D 9.
- Water volumes: the water surfaces that mods add (lakes, ponds, wells, the water in a
  boat's hold) are drawn as water, near the camera and among the distant statics. The dry
  spaces that a mod reports (the inside of a boat's hold) have no water. A mod can give a
  surface a water shader of its own (`Data Files/shaders/water/<name>.wgsl`).
- More light on water volumes: three options under `[distant_land.water]`, each on by
  default. `volume_shadows`: the shadows of distant land fall on the surface.
  `volume_point_lights`: the lamps and fires of the game glint in it. `volume_caustics`: the
  light pattern of ripples on what is under it.
- The vis groups of the distant statics: a group of distant statics that the game hides is
  hidden, for example the Ghostfence after the end of the main quest.
- Distant lights at night: each placed light of the exterior has a glow far away, in the
  colour of its light, and the land and the distant statics get the light of the lamps near
  them. They come from the file `distantland\lights.data`, which the generator of this build
  writes; distant land made by an earlier build is refused (see below), so generate it again.
  Without the file, `nirender-host.log` has the line `distant lights: none`. The MGE XE GUI
  has no option for them yet; to turn them off, set `distant_lights = false` (or `bake_lights = false` for the
  glows only) under `[generation.settings]` in `mgeXE.toml`, and generate again.
- The light of each pixel uses the lights of the game: each light keeps its colour and its
  whole falloff. To get the lights as MGE XE on Direct3D 9 decodes them, set
  `per_pixel_original_lights = true` under `[distant_land]`. With it on, an install whose
  light constant is 0.000 and whose lights use a linear falloff turns every object light
  grey (with a quadratic falloff only, the colour is kept): that is MGE XE's own rule, on
  Direct3D 9 too.
- Each per-pixel point light fades to zero before 1.75 times its radius
  (`per_pixel_light_fade`, on by default since this build; `per_pixel_light_fade_radius`).
  A `mgeXE.toml` written by an older build keeps its own value.
- `clustered_lights = true` under `[distant_land]` (off by default): each pixel takes its
  lights from the clusters of the view. The picture is the same; it can pay with many lights
  for each object, and costs CPU time otherwise.
- The GPU chooses the distant statics in view. The CPU of the renderer does less work for
  them. A GPU without the features for this (`nirender-host.log`: `the places are chosen
  on the CPU`) gets the same picture from the CPU.
- The merged distant statics stay under a limit of GPU memory. The limit comes from the
  memory budget of the GPU on Vulkan. Without a budget (Direct3D 12, or a driver without
  the extension) the limit is `statics_fallback_cap_mb` under `[distant_land]`: 1024 MB at
  the start, `0` for no limit. When the limit is full, a far static can show late.
- The merged distant statics stay on the GPU after the camera leaves them, up to a high mark
  (`statics_residency = "auto"` and `statics_resident_mb` under `[distant_land]`; `0` is half
  of the limit), so a return to a place seen before does not load them again. `"all"` loads
  all of them at the start when they fit; `"near"` keeps only those near the camera.
- With `automatic_rebuild = true` under `[distant_land]`, the distant land is made again at
  the start when the load order or the generation settings changed, as with Direct3D 9.
- Distant land data that this build cannot read (made by an older or newer MGE XE) is not
  drawn. The distant land is off for the session, and the F10 panel and `nirender-host.log`
  tell you to generate the distant land again in the MGE XE GUI.
- While the game sends no picture for more than half a second (at its start, when it loads),
  a small line turns in the lower right corner of the window. The game is not stopped.
- Alt+F4 closes the game, with either renderer. The game does not ask and does not save.
- The screenshot key (PrintScreen, or the macro) writes the picture as on Direct3D 9: the
  folder, the name and the format of the screenshot settings. The status text of MGE XE and
  its frame rate counter show in the upper left corner, as on Direct3D 9.
- The distant statics and the grass fade in over 1 s when they arrive (after a load, a
  teleport, or as you travel), where Direct3D 9 shows them at once. The time is the
  parameter "Distant statics: fade-in time" of the renderer's parameters window (row
  "water (renderer)"); 0 turns the fade off until the game ends.
- To have the distant land complete when a loaded game starts or after a teleport (a door,
  a travel, Recall), as MGE XE on Direct3D 9 does at the first load: "Wait on the loading
  screen for the distant land" in the Distant Land page of the MGE XE GUI
  (`wait_on_load = true` under `[distant_land]`). The loading screen then stays until the
  distant statics around you are there, at most 15 s. A teleport waits only with an MWSE
  that has the event of the player's change of cell (its native event 5).
- Three looks of the grass shader of Remiros' Groundcover are options in the Distant Land
  page of the MGE XE GUI (`[distant_land.grass]`, off at the start): `soft_end` (the grass
  sinks into the ground near the end of its range, so it ends without a line),
  `vertex_color_sway` (the red of the vertex colour of a grass mesh weights its sway) and
  `wrap_lighting` (the sun light wraps past the side of a blade, and some comes through
  from the back). The wgpu renderer only.
- "Pause world rendering in menus" (`menu_caching` under `[runtime]`, off at the start)
  works as on Direct3D 9: in a menu the picture of the world stays still, and the renderer
  does less work, until a click.
- For mod authors: the event `distantLandReady` tells when the distant land is loaded and
  when it is there around the camera after a load or a teleport (see
  `docs/architecture/mwse-api.md`). It works on Direct3D 9 too.
- For mod authors: a view that a mod draws itself (a camera into a `niRenderedTexture`, as a
  mirror, or a second camera into the screen) can get parts of MGE XE with
  `mge.camera.setViewOptions(key, options, flags)` (MGE XE API version 4, in the `MWSE.dll` of
  this build): `distantLand`, `distantStatics`, `grass`, `distantWater`, `shaders` and `glows`.
  With `{ intoTextures = true }` as the flags, the options of a camera are its default for
  each texture it draws into. `full_tier_max` under `[views]` in `mgeXE.toml` (default 1,
  0 to 8) limits how many such views of a frame get the parts; 0 refuses every request. With
  the Direct3D 9 renderer the call stores the options and draws nothing more.
- MGE XE's menu background video (`Data Files\video\menu_background.bik`) plays on the wgpu
  renderer too, drawn as the game draws its films.
- The window can be resized, and the game's own menu changes the resolution during play: the
  picture scales into the window. A fullscreen game is a borderless window on its own monitor.
- If the renderer program stops (`numidium.exe` crashed or was ended), the game saves as
  "MGE XE autosave <date>", and a message box says so and closes the game. Start again, load
  that save, and send `nirender-host.log`, `mgeXE.log`, and `numidium.dmp` if it is there.

## Licenses

Each part of the archive, with its license. The texts are in the archive:
`COPYING-GPL-2.0.txt` (the GNU General Public License, version 2), `LICENSE-nirender.txt`
(MIT), `THIRD-PARTY-NOTICES.txt` (the crates inside the programs), and True Water's own two
files. `SOURCES.txt` names the source repository and the commit of each part: the complete
source of every program here is public at those commits.

| Part | License |
| --- | --- |
| MGE XE G7: `d3d8.dll`, `dinput8.dll`, `MGEXEgui.exe`, the MGE XE part of `numidium.exe`, the shaders, the textures, the meshes, `XE Sky Variations.esp`, the MGE XE Options mod | GNU General Public License, version 2 |
| `MWSE.dll` (a build of our fork of MWSE, with the part the renderer needs) | GNU General Public License, version 2 |
| nirender: `Data Files\MWSE\lib\akulakhan.dll`, `Data Files\MWSE\mods\nirender`, and the host crate inside `numidium.exe` | MIT. `numidium.exe` as a whole is distributed under the GNU General Public License, version 2, since it contains the code of MGE XE G7 |
| True Water: `Data Files\MWSE\lib\watervolumes.dll`, `Data Files\MWSE\mods\waterVolumes`, the `meshes\wv*` folders, `Water Volumes Kit.esm` | MIT for the mod's files; `watervolumes.dll` under the GNU General Public License, version 2, since it is built with the code of MWSE (its readme and license files say so) |
| The Rust crates inside `numidium.exe`, `MGEXEgui.exe`, `d3d8.dll` and `dinput8.dll` | each under its own license, named in `THIRD-PARTY-NOTICES.txt` with its text |

## Known issues

The open issues of each build are in its release notes on GitHub:
<https://github.com/StormAtronach/nirender-plugin/releases>. Read the notes of your build
before you report one. This file does not repeat them, so that the two cannot disagree.

Differences from MGE XE on Direct3D 9 that are by design, not issues:

- The shadow on an object that has a scale (a tree at two times its size) is lighter than
  on Direct3D 9. The Direct3D 9 path makes that shadow deeper with the scale of the
  object, and the wgpu renderer does not.
- An interior that behaves as an exterior (the Mournhold plazas and some mod cells) is
  drawn with its distant statics, shadows and sky. A plain interior cell that has distant
  statics in the generated data gets its statics and grass, with the fog of the game.
- The grass receives the shadows of the distant land and the distant statics, as on
  Direct3D 9. Where the shadow map ends (about 4000 units ahead) and in the fog, its shadow
  fades as the shadow on the ground under it does; Direct3D 9 ends it at once. The
  parameter "Grass: shadow" of the renderer's parameters window chooses: 2 (as the ground),
  1 (as Direct3D 9), 0 (no shadow on the grass).
- A skinned shape takes every authored influence. On Direct3D 9 a vertex has at most 4, so a
  picture differs on a triangle with over 4 bones across its vertices.
- The character in the inventory is lighter than on Direct3D 9. The light is the one that
  `Morrowind.ini` names under `[Inventory]`; Direct3D 9 draws it wrong.

## The debug overlay

Press F10 in the game (the key is `overlayKey` in `Data Files/MWSE/config/nirender.json`;
`"none"` turns it off). A panel shows the frame rate, the time of each of the last 600
frames as a graph with the long frames in red, the memory of the renderer and of the GPU,
and the memory of the game with its address space and the largest free block of it. The
game is a 32-bit program: when the address space is near 4096 MB, or the largest free block
is small, the game is near a crash for lack of memory. The panel is not in screenshots.

The line `picture` tells how old the picture is when the renderer gives it to the window,
in ms, and where the time goes: the frame of the game, the wait between the game and the
renderer, and the renderer. The numbers are those of a usual frame (the median of the last
half second). The time that the GPU and Windows use after that is not in the number. If the
mouse pointer of the menus follows your hand late, send us this line.

## Debug views

A debug view shows one value of the renderer in the picture, for a developer's test. At most
one shader view is on at a time. The line sources are a set beside it; this build has no line
pass yet, so a line source has no effect.

| Shader view | What it shows |
| --- | --- |
| `lights` | a draw lit for each pixel shows only the light it takes, without its material, its textures and the fog |
| `shadow-layers` | each pixel coloured by the layer of the shadow map that serves it: red the first, green the second, blue outside both, grey a draw without the shadow test; darker where the map shadows it. Indoors it shows nothing: an interior has no shadow map |
| `shadow-map` | the atlas of the shadow map in the lower left of the picture |
| `water <term>` | one term of the built-in water shader, 1 to 10: the table and the rule for a core mod's `water_color` are in `docs/core-mods.md`, "The water debug views" |
| `post <term>` | 1 tints the result of the post chain, 2 shows its linear depth over 10000 units as grey |

The line sources: `lights`, `shadow-boxes`, `bounds`, `lua`.

- In a game, from Lua: `nirender.debug.view(name, term)` (`nirender.debug.view("off")` for
  none) and `nirender.debug.lines(name, on)`. Each sends a tune.
- The tunes: `debug view <name> [term]`, `debug view off`, `debug lines <name> on|off`.
- `mge.render.lightDebug = true` turns the `lights` view on, and `false` turns it off. A
  Settings record that does not change the value leaves another view as it is.
- In a replay: `--debug-view <name> [term]` and `--debug-lines <name,name>` of `numidium.exe
  --replay` (and of the plugin's `tools/replay.py`). A replay takes its view only from these
  arguments: a tune or a Settings record in the capture does not change it, so one capture
  serves every view.

## When something is wrong

Report it at <https://github.com/StormAtronach/nirender-plugin/issues>. The form there
asks for the files below. There is no other address.

1. Press F11 in the game (the key is `captureKey` in `Data Files/MWSE/config/nirender.json`;
   `"none"` turns it off). The plugin writes the next 3 frames into
   `nirender-captures\nirender-<date>.nircap` in the Morrowind folder. We can draw that file without your
   game and your mods, and see what you saw. The file holds the pictures and the textures
   of that place, so it can be large (a few MB to some hundred MB). Zip it; if it is still
   too large to attach, put it on a file host and paste the link. Some pictures of the
   menus are not in the file (the game cannot give them a second time), so a menu can
   have blank parts in it.
2. Attach that file with `mgeXE.log`, `MWSE.log` and `nirender-host.log`
   from the Morrowind folder, your `mgeXE.toml`, and a screenshot. Name the archive you
   installed. The logs and the dump attach as they are; zip `mgeXE.toml` and a save
   (`.ess`) before you attach them, since GitHub does not take those file types. The line of `mgeXE.log` that starts with `nirender: nirender v` and the first line
   of `nirender-host.log` name the versions,
   and the line `adapter:` of `nirender-host.log` names the graphics card, the backend and
   the driver. With Mod Organizer 2 and Root Builder, these files are in `overwrite\Root`
   of the instance after the game has ended.
3. After a crash, attach `MWSE_MiniDump.dmp` from the Morrowind folder too, and `numidium.dmp`
   if it is there. MWSE writes it
   when the game crashes, and the next start of the game deletes it: copy it first.
4. If the game does not start or shows nothing: set `renderer_wgpu = false`, start again,
   and send the same logs.
5. If an object is missing, comes late, or stays in an old pose: press F11 there, tell
   us the place, and send the capture with the logs. The console function `engineWalk`
   of the build v0.2.11 is gone: the game finds the objects itself in every build now.
6. If a message box says that the graphics device was lost (the driver was reset, or it
   stopped), the renderer cannot draw any more. Quit the game and start it again, and send
   `nirender-host.log`. A fault of the GPU that is not a loss does not stop the game: a
   part of the picture can be missing, and `nirender-host.log` has lines `gpu error`.

## Switches for a developer's test

Never for a game. Each is an environment variable of the game process or of a replay, read
by MGE XE's part of `numidium.exe`. The helper's other switches are in the README of the
`nirender` plugin.

- `MGE_XE_SKIP=water,terrain,statics,grass,blend`: the parts named are not drawn, for a
  replay that tests one suspect at a time.
- `MGE_XE_FULL_TIER_MAX=<n>` (0 to 32, the view slots of the plugin; the setting of the file takes 0 to 8): how many views of a frame get the parts of MGE XE that a mod asks for with `mge.camera.setViewOptions`, in place of `views.full_tier_max` of `mgeXE.toml` (default 1; 0 refuses every request).
- `MGE_XE_CORE_MODS=0`: no core mod (`Data Files/shaders/core-mods/XE Mod <name>.wgsl`, the
  WGSL hook files of `mgeHost64/src/renderer/core_mods.rs`) joins the shaders: the built-in
  hooks only. The baseline gate of the plugin sets it for its replays. The log names the files
  in use, or each file that is dropped and why, in its `core mods:` and `core mod:` lines.
- `MGE_XE_BOX_SECONDS=<n>`: a message box of d3d8.dll, of the helper or of the plugin
  closes itself after n seconds, and its text is in that part's log as `box:` lines
  (always); unset or 0, the box waits for the click. The test runner sets 5.
- `MGE_XE_SLOW_STEP=<seconds>`: one step of the helper's frame thread sleeps that long inside a
  long-step guard, then 600 ms more outside it in the same step, 300 frames after the loads ended,
  for a test of the long limits: 12 goes on, and 40 ends at the watchdog's 30 s with the exit code
  30. The log has its `step time:` line.
- `MGE_XE_STALL_LOG=1` (d3d8.dll): a line in mgeXE.log for each scan of the vis groups with
  the time the cell worker stalled for it; without it, a line only when a scan or its waits
  take 2 ms or more.
- `MGE_XE_DRAW_STATE_LOG=<n>` (d3d8.dll, Direct3D 9 only; 1 to 600): a window of n frames
  starts at a press of the Pause key while the game has the focus, or when the file
  `mge-draw-state.trigger` is in the game folder (d3d8.dll deletes it; it looks at each 30th
  frame; if the delete fails, it logs this once, and the file starts no more windows). Each draw of the game in the window writes a `draw-state frame <f> scene <s> draw <d>:`
  line to mgeXE.log: the primitives, the vertex format, whether a shader is bound, the depth,
  cull, stencil, blend, alpha test, colour mask, fog and lighting states, the lights that are
  on (bits), the normalize, specular and ambient states, the material, and the texture, colour,
  alpha, coordinate index and transform states of stages 0 to 3. Lines at the start and the end
  of the window count its frames and draws. Unset or 0, nothing is logged.
- `NIRENDER_HOST_EXE`: not a switch of a player. d3d8.dll sets it to `numidium.exe` of the
  game folder before it loads the plugin, which starts the program it names as the helper
  (the plugin's README has the other `NIRENDER_*` variables).
- `MGE_XE_STATICS_CAP_MB=<n>`: a fixed cap of the merged subsets of the distant statics, in
  place of `distant_land.statics_fallback_cap_mb`.
- `MGE_XE_LIST_WORLDS=1`: the report lists every interior world space of the distant
  statics, not the first 12.
- `MGE_XE_GRASS_HASH=1`: each grass call prints a line with the hash of its record list, in two
  forms: `ordered` (the records in the order of the draw) and `unordered` (the sum of the hashes
  of the records, which does not see the order). Two builds that draw the same grass give the
  same lines for the same capture. A picture does not show the order of the draws; this line does.
  The line is `grass hash frame <n> main|view: <r> records, cut <c>, ordered <h>, unordered <h>`.
  Without the switch the call does no hashing.
- `MGE_XE_DISTANT_LAND_DIR=<folder>`: the distant land of another folder, for a replay or a
  test.
- `MGE_XE_GAME_DIR=<folder>`: the folder of the game (its Data Files and archives), for a
  replay where the distant land is elsewhere.
