# The wgpu renderer of MGE XE G7 (test build)

This build of MGE XE G7 carries a second renderer. With it on, the game draws nothing with
Direct3D: the `nirender` MWSE plugin gives the game a Direct3D 8 of its own, turns each
draw of the game into a record, and the program `mgeHost64.exe` draws the records with
wgpu (Vulkan or Direct3D 12) into the window of the game, together with the distant land,
the water, the shadows and the post shaders of MGE XE.

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

This archive does not have the `d3d9.dll` (DXVK) of MGE XE G7. The new renderer does not
use it. MGE XE on Direct3D 9 runs with the `d3d9.dll` that you have, or with the one of
Windows.

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

## Requirements

- Everything that MGE XE G7 needs (see `MGE XE Readme.md`): a GPU with Vulkan 1.3, 64-bit
  Windows 10 or 11.
- MWSE 2.1, a build of 2026 or later. The plugin loads as an MWSE plugin
  (`Data Files/MWSE/lib/nirender.dll`, `Data Files/MWSE/mods/nirender/main.lua`); with a mod
  manager, check that it does not leave `.dll` files out.
- The Morrowind Code Patch, 2.5 beta 19 or later as shipped with recent MWSE. The plugin
  replaces 301 functions of the game's renderer with its own code and follows the changes
  that the Code Patch makes in them. At the start it compares the code of each function
  with its database; a change that it does not know turns the renderer off for the session,
  and the game runs on Direct3D 9 through MGE XE's proxy, without the distant land for that
  session. The message box after the load says so; the reasons are in `nirender.log`. Set
  `renderer_wgpu = false` to get MGE XE with its distant land back until the cause is
  known.
- Another mod that changes the renderer code of the game (not its Lua) has the same effect.
- When `mgeHost64.exe` or `MWSE.dll` is not in the Morrowind folder, MGE XE runs on
  Direct3D 9 as without the switch, and `mgeXE.log` names the file. A virus scanner can
  take a program away, and an archive can be extracted in part.
- When the plugin `Data Files/MWSE/lib/nirender.dll` is missing, the game runs on
  Direct3D 9 through the proxy for that session, without the distant land, and `mgeXE.log`
  says so.
- The same fallback applies when the Lua side of the plugin does not run (the Lua of MWSE
  failed to start, or `Data Files/MWSE/mods/nirender/main.lua` is missing): the game runs
  on Direct3D 9 through the proxy for that session, and `nirender.log` says why.

## What you get

- The picture of the game as the game draws it, with MGE XE's distant land, statics,
  grass, water, shadows and sky, and the post shader chain of `mgeXE.toml`.
- Every stock post shader has a port; a mod can ship a WGSL shader (see
  `MGE XE Post Shaders Readme.md`). Three shaders of this build are new: `Bloom S_A`,
  `SSAO S_A` and `Depth of Field S_A`; name them in the chain like the others.
- The frame rate of the game thread goes up where the Direct3D 9 driver was the limit.
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
- More light on water volumes: three options under `[distant_land.water]`, each off at the
  start. `volume_shadows`: the shadows of distant land fall on the surface.
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
  `per_pixel_original_lights = true` under `[distant_land]`.
- The GPU chooses the distant statics in view. The CPU of the renderer does less work for
  them. A GPU without the features for this (`nirender-host.log`: `the places are chosen
  on the CPU`) gets the same picture from the CPU.
- The merged distant statics stay under a limit of GPU memory. The limit comes from the
  memory budget of the GPU on Vulkan. Without a budget (Direct3D 12, or a driver without
  the extension) the limit is `statics_fallback_cap_mb` under `[distant_land]`: 1024 MB at
  the start, `0` for no limit. When the limit is full, a far static can show late.
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
- To have the distant land complete when a loaded game starts, as MGE XE on Direct3D 9 does
  at the first load: "Wait on the loading screen for the distant land" in the Distant Land
  page of the MGE XE GUI (`wait_on_load = true` under `[distant_land]`). The loading screen
  then stays until the distant statics around you are there, at most 15 s. Teleports do not
  wait; their statics fade in.
- "Pause world rendering in menus" (`menu_caching` under `[runtime]`, off at the start)
  works as on Direct3D 9: in a menu the picture of the world stays still, and the renderer
  does less work, until a click.
- For mod authors: the event `distantLandReady` tells when the distant land is loaded and
  when it is there around the camera after a load or a teleport (see
  `docs/architecture/mwse-api.md`). It works on Direct3D 9 too.
- If the renderer program stops (`mgeHost64.exe` crashed or was ended), a message box says
  so. The game cannot show a new picture after that: quit and start again, and send
  `nirender-host.log` and `nirender.log`.

## Known issues

- The shadow on an object that has a scale (a tree at two times its size) is lighter than
  on Direct3D 9. This is intended: the Direct3D 9 path makes that shadow deeper with the
  scale of the object, and the wgpu renderer does not.
- An interior that behaves as an exterior (the Mournhold plazas and some mod cells) is
  drawn with its distant statics, shadows and sky. A plain interior cell that has distant
  statics in the generated data gets its statics and grass, with the fog of the game.
- The dry spaces among the distant statics are not cut: far away, a boat's hold can show
  water. Near the camera they are cut.
- The grass receives the shadows of the distant land and the distant statics, as on
  Direct3D 9. Where the shadow map ends (about 4000 units ahead) and in the fog, its shadow
  fades as the shadow on the ground under it does; Direct3D 9 ends it at once. The
  parameter "Grass: shadow" of the renderer's parameters window chooses: 2 (as the ground),
  1 (as Direct3D 9), 0 (no shadow on the grass).
- The character in the inventory is lighter than on Direct3D 9. The light is the one that
  `Morrowind.ini` names under `[Inventory]`; Direct3D 9 draws it wrong.
- The three new shaders have first settings and no keys in `mgeXE.toml` yet.
- After a capture with F11, the picture of the character in the inventory is empty until
  the inventory changes (put an item on or off).
- With `renderer_backend = "dx12"` the mouse pointer of the menus can follow the hand late.
- Only tested at 1920 x 1080 on one machine (NVIDIA, Vulkan). Other GPU vendors, other
  resolutions, windowed modes and long sessions are not tested.

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

## When something is wrong

1. Press F11 in the game (the key is `captureKey` in `Data Files/MWSE/config/nirender.json`;
   `"none"` turns it off). The plugin writes the next 3 frames into
   `nirender-<date>.nircap` in the Morrowind folder. We can draw that file without your
   game and your mods, and see what you saw. Some pictures of the menus are not in the
   file (the game cannot give them a second time), so a menu can have blank parts in it.
2. Send that file with `mgeXE.log`, `MWSE.log`, `nirender.log` and `nirender-host.log`
   from the Morrowind folder, and a screenshot. The first lines of `nirender.log` and
   `nirender-host.log` name the versions.
3. If the game does not start or shows nothing: set `renderer_wgpu = false`, start again,
   and send the same logs.
4. If a message box says that the graphics device was lost (the driver was reset, or it
   stopped), the renderer cannot draw any more. Quit the game and start it again, and send
   `nirender-host.log`. A fault of the GPU that is not a loss does not stop the game: a
   part of the picture can be missing, and `nirender-host.log` has lines `gpu error`.
