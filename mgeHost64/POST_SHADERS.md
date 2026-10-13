# Post shaders for the wgpu renderer

The wgpu renderer of MGE XE runs the shader chain of `mgeXE.toml` (`[shaders] chain`) with
shaders written in WGSL. An effect file of Direct3D 9 (`.fx`, HLSL) does not run there. For
each name of the chain the renderer takes, in this order:

1. the file `Data Files/shaders/XEshaders/<name>.wgsl`, when there is one;
2. its own port of the stock shader with that name;
3. nothing: the log says that the shader was skipped.

The stock shaders with a port: SSAO Fast, SSAO HQ, Sunshafts, Depth of Field, Bloom Fine,
Bloom Soft, Eye Adaptation (HDR), Underwater Effects, Underwater Interior Effects. A port has
the switches of the file as it ships (for example the quality 3 of Bloom Soft).

Shaders of mods with a port: MGG SSAO and MGG darker interiors (Morrowind Graphics Guide
Shaders), ColorMood (STEP Shaders Compilation), heathaze (Heat Haze), Special Process.

## A port and the effect file of a mod

A mod does not have to change for its Direct3D 9 shader to run. For a name of the chain the
renderer reads the file `Data Files/shaders/XEshaders/<name>.fx` that MGE XE on Direct3D 9
would load (the file of MGE XE, or the file of a mod that replaces it), and compares its
code with the code of the file that the port was made from. The comments, the space, the
values of the numbers and the annotations of the technique do not count as code.

- The same code, the same numbers: the port runs.
- The same code, other numbers (a line `#define NAME value`, or `static const float NAME =
  value;`): the port takes the numbers of the file, and the log names them. A number that
  the port cannot take (a sample count that a table of the port goes with) is named too.
- The same code, another category in the technique (an older copy of a stock file has
  none): the port takes the place of the file in the chain.
- Other code: the port of that name runs as it is, and the log says that the file needs a
  port of its own. A file with the name of a stock shader and other code (an older version,
  a rewrite) is this case.
- No port of the name: the shader is skipped, and the log says so.

The log lines start with `post:`. A shader without a category stays where the chain has
it, as on Direct3D 9: it takes the place of the shader with a category before it.

The renderer also has three shaders of its own, with methods that the pass model of
Direct3D 9 did not allow: `Bloom S_A` (a chain of half-size levels, after Jimenez 2014),
`SSAO S_A` (ground-truth ambient occlusion at half size, after Jimenez 2016) and
`Depth of Field S_A` (a thin lens with the near and the far field apart, at half size). A
chain names them like any other; MGE XE on Direct3D 9 has no file of these names and skips
them.

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
| `disable_sunglare` | no | `true` takes the sun glare of the game out of the frame |
| `texture` | no | a file under `Data Files/textures`, bound as `tex1` |

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
  water, 3D), `tex1` (the texture of the shader, white without one).
- The samplers: `point_clamp`, `linear_clamp`, `linear_wrap`, `linear_mirror`,
  `point_wrap`. Read a texture with `textureSampleLevel(texture, sampler, coordinate, 0.0)`.
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

Each pass draws the whole of its target. A pass that uses `discard` keeps what the texture
held at that pixel. The output of the last pass is what the next shader gets, whatever its
size and format.

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
compile error count the common part first: the message says which line of it is the first
line of the file.
