# Core mods on the wgpu renderer (WGSL hook files)

On Direct3D 9, a core mod is a file `Data Files/shaders/core-mods/XE Mod <name>.fx` that
replaces the core shader file of that name, whole. The wgpu renderer does not read `.fx`
files. It reads `XE Mod <name>.wgsl` files from the same folder. Such a file replaces named
functions of the renderer's shaders (the hooks). The renderer keeps everything else: the
passes, the bindings, the entry points and the code around the hooks.

This page describes interface 1 (`mge-xe-core 1`). The source is
`mgeHost64/src/renderer/core_mods.rs`. Today interface 1 has the shadow hooks. The water hooks
come in a later part of the work.

## The rules of a file

- **The folder and the name.** `Data Files/shaders/core-mods/XE Mod <name>.wgsl`, with the
  same name as the `.fx` file of Direct3D 9. A mod can ship both files side by side: Direct3D 9
  reads the `.fx`, wgpu reads the `.wgsl`.
- **The first line.** The file starts with the line `//! interface: mge-xe-core 1`. A file
  without this line, or with another number, is not used.
- **Hooks only.** A file holds only:
  - hook functions, with the exact signature of interface 1;
  - functions, constants and structs of its own, whose names start with `mod_`.

  A file has no binding, no entry point, no override and no variable outside a function.
- **The library.** A hook may call the library functions of its file (see below). Every
  shader that calls the hook (each "home" of the hook) gives these functions in the same form.
  So one hook text works in every home.
- **A file you leave out.** A hook that your file does not define keeps the built-in version.
  A file can define one hook and leave the others.
- **One file fails alone.** When a file breaks a rule, or makes one of its homes fail the
  device's shader check, the renderer drops that file only. Its hooks take the built-in
  versions, and the log names the file and the reason. Exception: `XE Mod Shadow` and
  `XE Mod Shadow Data` are dropped together, so the code of one mod never runs with the values
  of another.
- **The files are read once,** when the renderer's helper starts. To try a change, start the
  game again.

## The log

At its start the renderer writes one line for each event:

- `core mods: XE Mod Shadow.wgsl (shadow_delta, shadow_esm) (interface mge-xe-core 1)`: the
  files in use, with the hooks that each file defines;
- `core mods: none used (Data Files/shaders/core-mods, interface mge-xe-core 1)`: no file is
  in use;
- `core mod: XE Mod <name>.wgsl: <reason>; the built-in shadow is used`: a file that is
  dropped, and why (the shader check's error text, or the rule it breaks);
- `core mod: XE Mod <name>.fx is for Direct3D 9; the wgpu renderer reads XE Mod <name>.wgsl,
  and there is none`: a Direct3D 9 core mod with no `.wgsl` file beside it;
- `core mods: off (MGE_XE_CORE_MODS=0)`: the switch is set.

## Turn the core mods off

Set the environment variable `MGE_XE_CORE_MODS=0` to start with the built-in hooks only. No
file is read. The renderer's own test gate sets it, so its accepted pictures do not depend on
the installed mods.

## The contract

- An addition to the library keeps interface 1.
- A change to a hook's signature, a change to an interface struct, or the removal of a library
  function raises the interface number.
- A file for interface 1 stops working when the number goes up. Its log line says so, and its
  hooks take the built-ins.

## The shadow hooks: `XE Mod Shadow.wgsl`

The homes of the shadow hooks: the draws of the game (the world shader), the grass, and the
surfaces of the water volumes.

| Hook | Signature | What it gives | Direct3D 9 function |
| --- | --- | --- | --- |
| `shadow_delta` | `fn shadow_delta(s0: vec3f, s1: vec3f) -> f32` | The depth of the shadow map less the depth of the pixel. `s0` and `s1` are the pixel in the spaces of the two shadow layers. A positive number where the map has nothing nearer to the sun. | `shadowDeltaZ` |
| `shadow_esm` | `fn shadow_esm(dz: f32) -> f32` | The shadow amount, 0 to 1, for a negative `dz`. The renderer multiplies it by the sun's light. | `shadowESM` |

The library of the shadow hooks:

| Function | What it gives |
| --- | --- |
| `shadow_map_depth(layer: u32, uv: vec2f) -> f32` | The shadow map at level 0. `layer` is 0 or 1; `uv` is 0 to 1 over the layer. |
| `shadow_texel() -> f32` | 1 over the resolution of a layer. |
| `shadow_esm_scale() -> f32` | The scale of the map's stored depth. |
| `shadow_esm_values() -> vec2f` | `esm_c` and `esm_bias` of `XE Mod Shadow Data` (x and y). |

The built-in `shadow_delta` uses the first layer when the pixel is inside it with a margin of
8 texels, else the second layer, else it gives `1e-6`. The built-in `shadow_esm` gives
`1 - clamp(exp(esm_c * dz + esm_bias), 0, 1)`.

An example file that gives the shadows a sharper edge (a larger exponent):

```wgsl
//! interface: mge-xe-core 1

fn shadow_esm(dz: f32) -> f32 {
    let esm = shadow_esm_values();
    return 1.0 - clamp(exp(1.5 * esm.x * dz + esm.y), 0.0, 1.0);
}
```

## The shadow values: `XE Mod Shadow Data.wgsl`

This file has no code. It has the interface line and one `values` line:

```wgsl
//! interface: mge-xe-core 1
//! values: shade = 0.4, shade_color = 1.0 0.97 0.81, esm_c = 60, esm_bias = 0.12
```

You can leave out any key; a key you leave out keeps its built-in value.

| Key | Numbers | Built-in | What it does |
| --- | --- | --- | --- |
| `shade` | 1 | 0.4 | How much a shadow reduces the light. The Direct3D 9 file recommends 0.25 to 0.5. |
| `shade_color` | 3 | 1.0 0.97 0.81 | How much the shadow affects each colour channel (red, green, blue). |
| `esm_c` | 1 | 60 | The ESM exponent. Higher gives less fade near the caster, but less soft shadows. Float accuracy limits it to about 88. |
| `esm_bias` | 1 | 0.12 | The ESM bias. It stops blurred depth from pushing a surface through an object. |

The renderer checks only that each key is known, has its count of numbers, and that each
number is finite. It does not clamp a value. The scale of the map's depth is not a key: it
matches the map's format.

## Cautions

- **A hook can hang the GPU,** as an `.fx` core mod can on Direct3D 9: for example a loop that
  never ends. The shader check cannot find this.
- **The shadow layers view** (`MGE_XE_SHADOW_LAYERS=1`) colours each pixel by the layer that
  the built-in margin picks. With a mod's `shadow_delta`, the view still shows the built-in
  choice, not the mod's.
