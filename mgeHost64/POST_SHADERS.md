# Post shaders for the wgpu renderer

The wgpu renderer of MGE XE runs the shader chain of `mgeXE.toml` (`[shaders] chain`) with
shaders written in WGSL. An effect file of Direct3D 9 (`.fx`, HLSL) does not run there. For
each name of the chain the renderer takes the file `Data Files/shaders/XEshaders/<name>.wgsl`.
A name with no such file is skipped, also when a file `<name>.fx` is there: the log has the
line `post: no file <name>.wgsl; skipped`, and `mgeXE-shaders.txt` has "The wgpu renderer has
no file <name>.wgsl." The renderer has no copy of a shader of its own.

MGE XE's package has these files in `Data Files/shaders/XEshaders`, beside the `.fx` files of
the same names:
- the nine stock shaders: `Bloom Fine.wgsl`, `Bloom Soft.wgsl`, `Depth of Field.wgsl`,
  `Eye Adaptation (HDR).wgsl`, `SSAO Fast.wgsl`, `SSAO HQ.wgsl`, `Sunshafts.wgsl`,
  `Underwater Effects.wgsl` and `Underwater Interior Effects.wgsl`. Each has the switches and
  the numbers of its `.fx` as it ships (for example the quality 3 of Bloom Soft);
- three shaders of the renderer, with methods that the pass model of Direct3D 9 did not
  allow: `Bloom S_A.wgsl` (a chain of half-size levels, after Jimenez 2014), `SSAO S_A.wgsl`
  (ground-truth ambient occlusion at half size, after Jimenez 2016) and
  `Depth of Field S_A.wgsl` (a thin lens with the near and the far field apart, at half size).
  A chain names them like any other; MGE XE on Direct3D 9 has no file of these names and
  skips them.

The shaders of mods come in shader packs. A pack is a small mod `<Mod> - wgpu shaders` with the
`.wgsl` files of the mod's shaders (the folders of `ports/` in the repository), installed after
the mod. A mod without its pack runs no shader on wgpu: the log names each one that is skipped.
A mod's file with the name of a stock shader (the Sunshafts of the STEP Shaders Compilation and
of Watch the Skies, the Eye Adaptation (HDR) and SSAO HQ of STEP, Krokantor's Underwater
Effects) has a `.wgsl` file of that name in the mod's pack: the pack must be after MGE XE in the
mod order, as the mod's `.fx` is, so that its file wins. Without the pack, the stock shader of
our package runs.

The packs: Morrowind Graphics Guide Shaders (MGG SSAO, MGG darker interiors, film,
deband_fogaware, Bloom Soft Sky), STEP Shaders Compilation (ColorMood, gammavtdt,
alternatebloom, and its files with the names of stock shaders), Heat Haze (heathaze),
Outlander Configuration (Special Process, Lens Effects, Cinematic Bokeh DoF), Magicka Expanded
(oj_me_fog_box), Vapourmist (tew_fogbox), Skoomaesthesia (skoomaesthesia), Kogoruhn Expanded
(Disorient), MM - Enhanced Invisibility (Invisibility), Inspect It (InspectIt/Bokeh), Modern
Lockpicking, Ashfall (ashfall), Joy of Painting (the 23 shaders `jop_*`), Ultra Bloom (Ultra
Bloom and XERO-lut; XERO-lut reads the colour table `Textures/XERO-lut.tga` of the mod), CRT
Shader (CRT, CRT Zoomed Out), Darker Nights, Watch the Skies (the current and the older
version), Krokantor's Enhanced Water Shader, and Eternal Shine. `ports/README.md` lists them
with their mods. On wgpu, Ultra Bloom takes its exposure from the night and the cell: 1 in the
day, 2 at night, 1.5 in an interior. The mod's menu values Exposure, Interior exposure bonus and
Night exposure bonus do not act on wgpu; its other values do.

Eternal Shine runs on wgpu only with its pack: its `Eternal Shine.wgsl` gives a glow around
each light from the lights of the frame, without the mod's Lua. The mod's menu does not act on
wgpu; the values are the settings of the file (see "Settings"), with the Lua's defaults:
`radmult` 0.3, `int` 0.25, `ext` 0.4, `rain` 0.5, and `near_eye` 100. A line of Lua sets one,
for example `mge.shaders.find({ name = "Eternal Shine" }).radmult = 0.4`. A light gives a glow
only when a draw that it lights is in view, and a light within `near_eye` units of the eye gives
none (the torch of the player in first person).

The log lines start with `post:`. A shader without a category stays where the chain has it, as
on Direct3D 9: it takes the place of the shader with a category before it.

So a mod can ship a `.wgsl` file beside its `.fx` file, with the same name: MGE XE on
Direct3D 9 uses the `.fx`, the wgpu renderer uses the `.wgsl`.

## The file

The file starts with lines `//! key: value`. They have what the technique of an effect file
has in its annotations. The rest of the file is WGSL.

```wgsl
//! interface: mge-xe-wgsl 1
//! passes: tint
//! category: final

@fragment fn tint(in: Quad) -> @location(0) vec4f {
    let c = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(c.b, c.g, c.r, 1.0);
}
```

| key | needed | meaning |
|---|---|---|
| `interface` | yes | `mge-xe-wgsl 1` |
| `passes` | yes | the fragment functions, one for each pass, with commas, in their order. A pass draws into a texture of the size of the picture with 8-bit colours; `name /N` draws into one of the picture size over N (2, 4, 8, ...), `name f16` into 16-bit floats, and `name /4 f16` both |
| `category` | no | `scene`, `atmosphere`, `lens`, `sensor`, `tone`, `none` or `final`: the place in the chain, as in an effect file. `none` without the key |
| `priority_adjust` | no | a number added to the place of the category |
| `mgeflags` | no | the shader does not run in a frame that has one of these bits: 1 interior, 2 exterior, 4 interior that behaves as an exterior, 8 under the water, 16 above the water, 32 the sun is visible, 64 the sun is not visible |
| `requires_hdr` | no | `true` turns the HDR measure on; the shader reads it as `u.hdr` |
| `disable_sunglare` | no | `true` takes the sun glare of the game out of the frame, when the shader is in the chain at the start of the game (as MGE XE on Direct3D 9: a shader that a mod loads later does not change the sun glare) |
| `texture` | no | up to ten files under `Data Files/textures`, with commas, bound as `tex1` to `tex10` in their order: DDS files, or TGA files (true colour, 24 or 32 bits). A file that is not found reads black |
| `variables` | no | the variables that a mod sets from Lua: see "A shader that a mod drives from Lua" |
| `settings` | no | the variables that a player adjusts and that MGE XE keeps between game sessions, with commas: see "Settings" |

MGE XE on the game side (d3d8.dll) reads these lines with the same parser as the renderer
(mge-config's `post_header`): a file that one of them refuses, the other refuses too.

## What a shader has

The renderer puts a common part before the file. A shader does not declare these:

- `struct Quad` with `position` and `tex` (the texture coordinate of the pixel, 0 to 1, the
  top left corner is 0, 0), and the vertex function `vs`. A pass is a fragment function
  that takes `in: Quad` and returns `@location(0) vec4f`.
- The textures: `lastshader` (the picture before this shader), `lastpass` (the output of
  the pass before, the picture for the first pass), `pass0` to `pass7` (the outputs of the
  first eight passes of this shader, white before they are drawn), `depthframe` (the
  distance of the pixel from the camera along the view axis in its red part, in units of
  the game; `1e38` where the game drew nothing), `watertexture` (the wave texture of the
  water, 3D), `tex1` to `tex10` (the textures of the shader, white without one).
- The samplers: `point_clamp`, `linear_clamp`, `linear_wrap`, `linear_mirror`,
  `point_wrap`, `point_mirror`. Read a texture with `textureSampleLevel(texture, sampler, coordinate, 0.0)`.
  `depthframe` takes `point_clamp` or `point_wrap` only (32-bit floats take no filter);
  `textureLoad(depthframe, vec2i, 0)` reads one texel.
- `target_size()` and `target_rcpres()`: the size of the target of this pass and 1 over it
  (`rcpres()` is always 1 over the size of the picture).
- The variables of MGE XE, in the uniform `u`:

| HLSL of MGE XE | WGSL here |
|---|---|
| `mview[row][col]`, `mproj[row][col]` | `mview(row, col)`, `mproj(row, col)` |
| `eyevec`, `sunvis` | `u.eyevec.xyz`, `u.eyevec.w` |
| `eyepos`, `fov` | `u.eyepos.xyz`, `u.eyepos.w` |
| `sunvec`, `time` | `u.sunvec.xyz`, `u.sunvec.w` |
| `suncol`, `waterlevel` | `u.suncol.xyz`, `u.suncol.w` |
| `sunamb`, `isInterior` | `u.sunamb.xyz`, `u.sunamb.w` |
| `sunpos`, `isUnderwater` | `u.sunpos.xyz`, `u.sunpos.w` |
| `HDR` | `u.hdr` |
| `fogcol`, `fogstart` | `u.fogcol.xyz`, `u.fogcol.w` |
| `fognearcol`, `fogrange` | `u.fognearcol.xyz`, `u.fognearcol.w` |
| `fognearstart`, `fognearrange` | `u.fog_near.x`, `u.fog_near.y` |
| `#ifdef USE_EXPFOG` | `if use_expfog() { ... }` |
| `rcpres` | `rcpres()` |

- The helpers `saturate(x)` for a number and `saturate3(x)` for a `vec3f`.
- The weather of the game, which no variable of MGE XE has (W622):
  - `weather_known()`: false without the weather record (an older d3d8.dll or capture); the
    functions below then give weather 0;
  - `weather_now()` and `weather_next()`: the index of the weather now and of the weather it
    changes to (the same when it does not change), 0 to 9;
  - `weather_ratio()`: the ratio of the change, 0 to 1;
  - `weather_mix(table)`: a table of 10 values (`array<f32, 10>`), one for each weather, gives
    the value of the weather now, mixed with the value of the weather it changes to by the
    ratio.

  The indices are the game's: 0 Clear, 1 Cloudy, 2 Foggy, 3 Overcast, 4 Rain, 5 Thunder,
  6 Ash, 7 Blight, 8 Snow, 9 Blizzard.

Each pass draws the whole of its target. A pass that uses `discard` keeps what the texture
held at that pixel. The output of the last pass is what the next shader gets, whatever its
size and format.

## A shader that a mod drives from Lua

A mod can load a shader and set its variables with the MWSE functions of MGE XE:

```lua
local shader = mge.shaders.load({ name = "my_shader" })
shader.intensity = 0.5
shader.enabled = true
```

This works with the wgpu renderer too. MGE XE reads the variables of the file that the
renderer runs, keeps what the mod sets, and sends it to the renderer. With
`my_shader.wgsl`, the Lua object has the variables of its line `variables` (`f32` is a
number, `f32[N]` a list of N numbers, `vec2` to `vec4` a vector, and `vecK[N]` a list of N
vectors), with the values after `=` before a mod sets them, and `mgeflags` when the header
has it; the category, `priority_adjust`, `requires_hdr` and `disable_sunglare` come from
the header too. Without a `.wgsl` file, the Lua object has the variables of the effect file
`my_shader.fx` (its names and types, as on Direct3D 9). When both files are there, the
`.wgsl` file gives the Lua object, as the renderer runs it; a mod's set of a variable that
only the effect file has does nothing. The renderer loads the shader of that name
when a mod loads it: the file `my_shader.wgsl`. A shader that a mod loaded is
on until the mod turns it off, and has its place in the chain by its category, as on
Direct3D 9.

The shaders of the chain of the settings are there too: `mge.shaders.find` and
`mge.shaders.list` give them, and the old calls (`mge.enableShader`, `mge.disableShader`,
`mge.setShaderFloat`, `mge.setShaderLong`) work on them, as the mod Heat Haze uses them. A
shader of the chain runs as the settings have it until a mod changes it.

`shader:reload()` works too: the renderer reads the files of the shader again (its WGSL
file and its textures), and the variables go back
to the values of the file, as a reload on Direct3D 9 makes the effect again. A mod that
writes a texture while the game runs (a canvas, a colour table, a mask) calls it to show
the new texture; an author of a shader calls it after a change of the `.wgsl` file, and a
shader that did not compile gets a new try. `mge.saveScreenshot` writes its file in this
mode too (`.png`, `.bmp`, `.tga`, `.dds`, `.jpg`), with the post shaders in it, with or
without the interface of the game.

The WGSL side names the variables that it reads, with the header line `variables`:

```wgsl
//! interface: mge-xe-wgsl 1
//! passes: tint
//! variables: intensity f32 = 0, colour vec3 = 1 0.5 0, weights f32[8]

@fragment fn tint(in: Quad) -> @location(0) vec4f {
    let c = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(mix(c.rgb, c.rgb * colour(), intensity()), 1.0);
}
```

A variable with the name of a function or a word of WGSL (`distance`, `length`, `step`, ...)
is read with `_` after its name: `distance_()`.

- A variable is `name type`, with `= numbers` for its values before a mod sets it (0
  without them). The types: `f32`, `vec2`, `vec3`, `vec4`, and an array `type[N]`.
- The WGSL reads a variable with a function of its name: `intensity()`, `colour()`,
  `weights(i)` for an array.
- The names are those of the effect file. `float name[N][3]` of an effect file (N groups of
  3 numbers, set from Lua as one list) is `name vec3[N]`.
- A shader has room for 256 values or vectors in all. A variable that the WGSL side does
  not name is not read; the log says so one time.

## Settings

The line `settings` names the variables that a player adjusts, for example from a line of
Lua or a menu of a mod:

```wgsl
//! variables: radius f32 = 0.3, strength f32 = 0.5, flicker f32 = 0
//! settings: radius, strength
```

MGE XE keeps their values between game sessions in the file `<name>.values.toml` beside the
shader file (`Data Files/shaders/XEshaders`). Under Mod Organizer 2, a new values file goes
into the overwrite folder, and a values file that is already in a mod's folder is changed
there. The file has one line `name = number` for each setting. It reads the file when the shader loads, and
writes it about 1 s after the last change from Lua, so a change in the last second before the
game ends can be lost. A missing file gives the values of the line `variables`; a line of the
file that is not valid is skipped, with a line in mgeXE.log.

- Each setting is a variable of the line `variables` of type `f32` (one number), named once.
  Another name drops the file, with its reason in the log.
- A variable that is not a setting is not kept: a mod that sets a variable in each frame
  makes no write of the file.

## From HLSL to WGSL

- `lerp` is `mix`, `frac` is `fract`, `tex2Dlod(s, float4(t, 0, 0))` is
  `textureSampleLevel`. WGSL has no implicit conversions: write `1.0`, not `1`, and
  `vec3f(0.5)`, not `0.5`, where a vector is needed.
- WGSL has no preprocessor and no `static` globals computed from variables: compute such a
  value at the start of the fragment function.
- `pow(x, n)` with a negative `x` has no defined result in WGSL. Multiply instead
  (`x * x * x * x`).
- `clip(v)` is `if v < 0.0 { discard; }`.

## When a file does not work

The renderer writes one line for each shader of the chain to its log. A file with a wrong
header line, a file whose WGSL does not compile, and a pass that is not a function of the
file are reported with the reason and skipped; the other shaders run. The line numbers of a
compile error in the log count the common part first: the message says which line of it is
the first line of the file.

The renderer also writes the result of each shader into `mgeXE-shaders.txt` in the folder
of the game, with the line numbers of your file. The MGE XE options read it: see the next
part.

## Edit a shader while the game runs

The MGE XE options (the mod configuration menu, MGE XE, then the button that opens the
settings) have the list of the loaded post shaders on the page Rendering.

- **Reload when the file changes.** With the setting "Reload a shader when its file
  changes" on (it is off at the start: switch it on while you work on a shader), the
  options look at the `.fx` and the `.wgsl` file of each loaded shader two times in a
  second. When you save a file, the shader is loaded
  again. A shader that compiles shows "shader reloaded"; a shader that does not compile
  shows the error of the compiler with the line of your file, and the full text goes into
  `MWSE.log`. With Direct3D 9, the error is in `mgeXE.log`.
- **Parameters.** The button "Parameters" of a row opens a window with an editor for each
  variable of the shader that a mod can set (`extern` variables of the effect file; not
  the variables that MGE XE sets itself, such as `time` or `eyepos`). A change is in the
  picture at once. The values are not saved: they are for a look by eye. The button
  "Reload" of the window loads the shader again by hand.

Without more, each number has the buttons - and +. A file `<name>.toml` beside the shader
(`XEshaders/<name>.toml`) describes the parameters, and a parameter with a range gets a
slider:

```toml
[[parameter]]
name = "strength"       # the name of the variable in the effect file
label = "Strength"      # the text of its row; the name without it
about = "How much of the tint is in the picture."
start = 0.5             # the value of the button "Reset"; the first value without it
least = 0.0             # least, most and step together give a slider
most = 1.0
step = 0.05
```

The rows of the parameters that the file names come first, in its order. An entry of a
vector (`float3`) is for each of its parts. An array, a matrix and a text are listed, but
they are not editable in the window.

The water of the wgpu renderer has a row of its own in the list ("water (renderer)"): its
parameters are in `tunables.toml` of the MGE XE options, in the same form.
