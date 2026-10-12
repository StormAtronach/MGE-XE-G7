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

## Known issues

- The shadow on an object that has a scale (a tree at two times its size) is lighter than
  on Direct3D 9. This is intended: the Direct3D 9 path makes that shadow deeper with the
  scale of the object, and the wgpu renderer does not.
- An interior that behaves as an exterior (the Mournhold plazas and some mod cells) is
  drawn with its distant statics, shadows and sky. A plain interior cell that has distant
  statics in the generated data does not get them yet.
- The water of an interior cell is not drawn by the renderer's water (the game's own water
  plane shows).
- No caustics under the water; no water volumes (the surfaces that mods add); the
  reflection in the water is lighter than on Direct3D 9 (7 to 12 %).
- The grass does not receive shadows.
- The character in the inventory is lighter than on Direct3D 9. The light is the one that
  `Morrowind.ini` names under `[Inventory]`; Direct3D 9 draws it wrong.
- The three new shaders have first settings and no keys in `mgeXE.toml` yet.
- Only tested at 1920 x 1080 on one machine (NVIDIA, Vulkan). Other GPU vendors, other
  resolutions, windowed modes and long sessions are not tested.

## The debug overlay

Press F10 in the game (the key is `overlayKey` in `Data Files/MWSE/config/nirender.json`;
`"none"` turns it off). A panel shows the frame rate, the time of each of the last 600
frames as a graph with the long frames in red, the memory of the renderer and of the GPU,
and the memory of the game with its address space and the largest free block of it. The
game is a 32-bit program: when the address space is near 4096 MB, or the largest free block
is small, the game is near a crash for lack of memory. The panel is not in screenshots.

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
