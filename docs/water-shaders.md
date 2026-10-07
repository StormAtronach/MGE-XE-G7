# Water shaders of mods

A mod can give the surface of a water volume a pixel shader of its own: foam, lava, ice,
anything. MGE XE does not ship such looks. It gives the place to plug them in.

## What a mod ships

One file for each shader, `Data Files\shaders\water\<name>.fx`. The file has a pixel shader
named `WaterShaderPS`:

```hlsl
float4 WaterShaderPS(in WaterVolumeVertOut IN) : COLOR0
{
    WaterSurfaceLook look = nearSurfaceLook(IN.color);
    WaterShade shade = shadeWaterVolume(IN.water, IN.facing, look.reflectsScene, false, waterVolumeTint, look);

    // Change shade.colour here.

    return finishWaterVolume(IN.water, shade, waterVolumeTint, look);
}
```

A surface uses the shader when its look names it (`WaterLook::shader`, see
`MGE_WaterLookSet`). The mod that owns the water volumes sets the look; with True Water it is
the key `shader=<name>` in the look line of the mesh.

The standard look is such a shader too: `shaders\core\XE Mod Water Standard.fx` is the
example above and nothing more. It uses only what is listed here. To change the look of every
water volume that names no shader, put a file of that name into `shaders\core-mods`.

## What the shader gets

| Name | What it is |
| --- | --- |
| `IN.water` | The vertex of the water plane shader: `pos` (world), `texcoords`, `screenpos` |
| `IN.facing` | The normal of the mesh, in the world |
| `IN.color` | The vertex colour of the mesh |
| `IN.uv` | The first texture coordinates of the mesh |
| `sampMesh0` | The base texture of the mesh, with mip levels, wrapped |
| `nearSurfaceLook(IN.color)` | The look of the surface: `drift`, `speed`, `scale`, `glow`, `opacity`, `tint`, `reflectsScene`, and the free values `p0` to `p3` |
| `waterVolumeTint` | The colour of the water: the emissive colour of the material |
| `shadeWaterVolume(...)` | The standard shading up to the look. The third argument says whether the surface reflects what is on screen or the sky only; the fourth is `false` |
| `WaterShade` | Its result: `colour`, `eyeVec`, `dist`, `fog`, `ripple`, `face`, `rayDepth` (how far the view ray goes through the water) and `waterDepth` (how deep the water is under the point) |
| `finishWaterVolume(...)` | The last steps: glow and opacity |

Everything else of `XE Common.fx`, `XE Mod Water.fx` and `XE Mod Water Volume.fx` can be
used too: `time`, `eyePos`, `sunColAdjusted`, `skyCol`, `fogApply`, `sampDepth`,
`sampRefract`, `reflectScene`.

## Rules

- All water shaders are part of one effect. Give every function and constant of your file a
  name of its own, for example with the name of the shader in front. `WaterShaderPS` is the
  one name that every file has; MGE XE renames it for each file.
- A shader has no pass, no vertex shader, no sampler and no render target of its own.
- The file name, without `.fx`, is the name of the shader. It is not case sensitive and has
  at most 31 characters.

## When a shader does not compile

MGE XE compiles the main effect again without the water shaders of mods, names each file
that fails with the compiler's message in `mgeXE.log`, and shows a line on the status
overlay. The surfaces then have the standard look. The same happens to a surface whose look
names a shader that is not there.

## Limits

- A surface far away, in distant land, has the standard look.
- From under the surface every surface has the underwater pass.
- The cost of a water shader is the cost of its author. The standard pass has about 360
  instruction slots.
