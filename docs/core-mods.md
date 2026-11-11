# Core mods on the wgpu renderer (WGSL hook files)

On Direct3D 9, a core mod is a file `Data Files/shaders/core-mods/XE Mod <name>.fx` that
replaces the core shader file of that name, whole. The wgpu renderer does not read `.fx`
files. It reads `XE Mod <name>.wgsl` files from the same folder. Such a file replaces named
functions of the renderer's shaders (the hooks). The renderer keeps everything else: the
passes, the bindings, the entry points and the code around the hooks.

This page describes interface 1 (`mge-xe-core 1`). The source is
`mgeHost64/src/renderer/core_mods.rs`. Interface 1 has the shadow hooks and the water hooks.

## The rules of a file

- **The folder and the name.** `Data Files/shaders/core-mods/XE Mod <name>.wgsl`, with the
  same name as the `.fx` file of Direct3D 9. A mod can ship both files side by side: Direct3D 9
  reads the `.fx`, wgpu reads the `.wgsl`.
- **The header.** The file starts with the line `//! interface: mge-xe-core 1`. A file
  without this line, or with another number, is not used. All the `//!` lines of a file come
  first. Blank lines before and between them are allowed. A `//!` line after the first line
  that is not a `//!` line or blank drops the file, and the log names that line. A `//`
  comment counts as such a line: `//! interface`, then `// a note`, then `//! values` drops
  the file. A UTF-8 byte order
  mark at the start of a file is removed before the file is read.
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
  files in use, with the hooks that each file defines; a values file shows its values line, as
  in `XE Mod Shadow Data.wgsl (shade = 0.5)`;
- `core mods: none used (Data Files/shaders/core-mods, interface mge-xe-core 1)`: no file is
  in use;
- `core mod: XE Mod <name>.wgsl: <reason>; the built-in shadow is used`: a shadow file that
  is dropped, and why (the shader check's error text, or the rule it breaks). When both shadow
  files are installed, the line ends with `; XE Mod <other>.wgsl is dropped with it`;
- `core mod: XE Mod Water.wgsl: <reason>; the built-in water is used`: the water file is
  dropped, and why;
- `core mod: XE Mod Water.wgsl: the plane of the water takes its hooks; the water volumes and
  the dry spaces keep the built-in water`: the water file is in use;
- `core mod: XE Mod Water.wgsl: the debug view water shows the terms of the built-in colour on
  the surface of the file`: the water file defines `water_color` or `underwater_color` (see
  "The water debug views");
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
| `shadow_esm` | `fn shadow_esm(dz: f32) -> f32` | The shadow amount, 0 to 1, for a negative `dz`. The renderer scales it by the sun's light and by the fade at the edge of the outer layer. | `shadowESM` |

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

The renderer checks that each key is known, has its count of numbers, that each number is
finite, and that each value is in its range:

| Key | Range |
| --- | --- |
| `shade` | 0.01 or more (the night's shadow estimate divides by it) |
| `shade_color` | 0 to 1 in each number |
| `esm_c` | above 0, at most 88 |
| `esm_bias` | any number |

A value out of its range drops the file, and `XE Mod Shadow.wgsl` with it. The log line names
the key, the value and the range, for example `the value esm_c = 88.5 is out of its range:
above 0, at most 88`. The renderer does not clamp a value. The scale of the map's depth is not
a key: it matches the map's format.

## The water hooks: `XE Mod Water.wgsl`

The home of the water hooks is the plane of the water: the water of an exterior and the water
of an interior. The surfaces of the water volumes and the dry spaces keep the built-in hooks, also
when this file is installed: Direct3D 9 has a core file of its own for the volumes
(`XE Mod Water Volume.fx`).

| Hook | Signature | What it gives | Direct3D 9 function |
| --- | --- | --- | --- |
| `water_normal` | `fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f` | The normal of the water at a pixel, length 1, z up. `texcoords` are the two wave coordinates (far in xy, close in zw), `distance` the distance from the eye, `world_xy` the place in the world. | `getFinalWaterNormal` |
| `water_color` | `fn water_color(p: WaterPixel) -> vec4f` | The colour of the water seen from above. | `WaterPS` |
| `underwater_color` | `fn underwater_color(p: WaterPixel) -> vec4f` | The colour of the plane seen from under the water. | `UnderwaterPS` |

A hook may call the other hooks of its file, also a hook that the file leaves to the built-in.
So a `water_color` of a mod can call `water_normal`, the mod's or the built-in. The built-in
`underwater_color` calls `water_normal` too, so a mod's `water_normal` also changes the colour
of the plane seen from under the water.

The struct `WaterPixel` is part of the interface:

| Field | Type | What it holds |
| --- | --- | --- |
| `uv` | `vec2f` | The place of the pixel in the picture, 0 to 1. |
| `clip_w` | `f32` | The view depth of the pixel (the clip w). |
| `texcoords` | `vec4f` | The two wave coordinates: far in xy, close in zw. |
| `world` | `vec3f` | The place of the pixel in the world. |
| `eye_vec` | `vec3f` | The direction from the eye to the pixel, length 1. |
| `distance` | `f32` | The distance from the eye to the pixel. |
| `reflect_uv` | `vec2f` | Where the reflection is read. With the dynamic waves, the point that is not above the plane. |
| `reflect_w` | `f32` | The view depth of that point. |

The library of the water hooks. The functions:

| Function | What it gives |
| --- | --- |
| `reflection_at(uv: vec2f, w: f32, stretch: f32) -> vec3f` | The reflection at a place of the picture, at the view depth `w`, with the blur that is sharp at the waterline. A `stretch` of 1 or more makes the blur taller. |
| `fog_water(direction: vec3f, distance: f32) -> vec4f` | The fog over the water: rgb the colour that it adds, a what is left of the colour of the thing. |
| `sun_glint(eye_vec: vec3f, normal: vec3f, fog_a: f32) -> vec3f` | The light of the sun that the water mirrors to the eye. |
| `deep_color(fog: vec4f, tint: vec3f) -> vec3f` | The colour of deep water with the fog over it. A black `tint` gives the usual colour. |
| `transmission(tint: vec3f, depth: f32) -> vec3f` | What is left of light through this much water of a colour. |
| `sky_color(direction: vec3f) -> vec3f` | The colour of the sky in a direction. |
| `picture_at(uv: vec2f) -> vec3f` | The picture behind the water (the refraction texture of Direct3D 9). |
| `ground_distance_at(uv: vec2f) -> f32` | The view depth of the nearest thing at a place of the picture, the distant land too. |
| `wave_at(coords: vec3f) -> vec4f` | The volume of the waves: x and y the place, z the time. |
| `ripples_on(normal: vec2f, close: vec2f, world_xy: vec2f) -> vec2f` | `normal` with the ripples of the rain (at the close wave coordinates) and of the player (at the place) added. Without the dynamic waves, `normal` is unchanged. |

The values of the frame. Each one is the Direct3D 9 shader variable that it stands for:

| Function | Direct3D 9 variable | What it gives |
| --- | --- | --- |
| `water_time() -> f32` | `time` | The time. |
| `water_sun_direction() -> vec3f` | `sunPos` | The direction to the sun. |
| `water_sun_visibility() -> f32` | `sunVis` | The visibility of the sun. |
| `water_sun_color() -> vec3f` | `sunCol` | The colour of the sun, without the sunlight factor of the built-in. |
| `water_wind() -> vec2f` | `windVec` | The wind. |
| `water_sky_color() -> vec3f` | `skyCol` | The colour of the sky. |
| `water_fog_color() -> vec3f` | `fogColFar` | The colour of the far fog. |
| `water_eye() -> vec3f` | `eyePos` | The place of the eye. |
| `water_ambient_color() -> vec3f` | `sunAmb` | The ambient light: the sun's ambient and the scene's together. |
| `water_texel() -> vec2f` | `rcpRes` | 1 over the size of the picture. |
| `water_wave_height() -> f32` | `waveHeight` | The height of the dynamic waves; 0 without them. |
| `water_is_interior() -> bool` | none | True in a plain interior. False in an exterior and in an interior that behaves as an exterior. |

The light multipliers of the weather. `water_sun_color()` and `water_ambient_color()` always
have the weather's light multipliers in them. Direct3D 9 applies these multipliers to `sunCol`
and `sunAmb` only when per-pixel lighting is active in this cell. Per-pixel lighting is on by
default, so the values are usually equal. They differ when per-pixel lighting is off, and in
an exterior when per-pixel lighting is set to interiors only.

A value that the built-in derives from these values (the sunlight factor, the wind factor) is
not in the library. Tell an interior by `water_is_interior()`, not by a test of the sun colour
such as `sunCol > 0.003`. The raw `water_sun_color()` stays for a faithful port of a
Direct3D 9 mod.

The built-in `water_normal` reads the wave texture twice with `wave_at` (the far read past 8000
units), adds the ripples with `ripples_on`, and gives the normal. An example file with calmer
waves (half the slope of the built-in):

```wgsl
//! interface: mge-xe-core 1

fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.4 * water_time();
    let far = wave_at(vec3f(texcoords.xy, t)).rg;
    var close = wave_at(vec3f(texcoords.zw, t)).rg;
    close = ripples_on(close, texcoords.zw, world_xy);
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(0.5 * n, 1.0));
}
```

## The water debug views

The debug view `water <number>` shows one term of the water shader in place of the
water's colour. The terms are those of the built-in `water_color` and `underwater_color`:

| Number | Seen from above | Seen from under the water |
| --- | --- | --- |
| 1 | the reflection | the reflection |
| 2 | the refraction | the refraction |
| 3 | the depth of the water under the pixel, over 1000 | the colour |
| 4 | the Fresnel factor | the colour |
| 5 | the fog factor | the colour |
| 6 | the distance from the eye, over 10000 | the colour |
| 7 | the normal (each number times 0.5, plus 0.5) | the colour |
| 8 | the view depth, over 10000 | the colour |
| 9 | the waves: red the height that the vertices read, green the same read for the pixel | the colour |
| 10 | the colour | the picture of the game before the fog of the water is mixed into it |

With a `water_color` or `underwater_color` of a mod, the view draws the plane with the
built-in `water_color` and `underwater_color` in place of the mod's, and with the mod's other
hooks, such as its `water_normal`. So the terms are those of the built-in colour on the
surface of the mod. The log says so. Views 1, 2 and 10 also change the surfaces of the water
volumes seen from below, as these share the built-in under-water shading.

## Cautions

- **A hook can hang the GPU,** as an `.fx` core mod can on Direct3D 9: for example a loop that
  never ends. The shader check cannot find this.
- **The shadow layers view** (the debug view `shadow-layers`) colours each pixel by the layer that
  the built-in margin picks. With a mod's `shadow_delta`, the view still shows the built-in
  choice, not the mod's.
- **The water debug views** show the terms of the built-in colour only, on the surface of the
  mod (see "The water debug views").
