//! The core mods of the wgpu renderer (the owner's D226, "wgsl hook files"): a file
//! `Data Files/shaders/core-mods/XE Mod <name>.wgsl` replaces named functions (the hooks) of
//! the renderer's shaders, as `XE Mod <name>.fx` replaces the core file of that name on
//! Direct3D 9. The design: `moreFPS/docs/Numidium Project/designs/core-mod-hooks-2026-11-10.md`.
//!
//! The files are read once, when the helper starts. A file starts with the line
//! `//! interface: mge-xe-core 1`. It holds only hook functions with their exact signatures,
//! and helpers, constants and structs of its own whose names start with `mod_`: no binding, no
//! entry point, no variable outside a function. A hook may call the library of its file: the
//! functions that every home of the hook (each shader that calls it) gives in the same form.
//! So one hook text joins every home, and the renderer can change all that is outside the hooks.
//!
//! A file that breaks a rule, or that makes one of its homes fail the device's check, is
//! dropped alone: its hooks take the built-ins, and the log names the file and the reason.
//! `XE Mod Shadow` and `XE Mod Shadow Data` are dropped together, so that the code of one mod
//! never runs with the values of another. With `MGE_XE_CORE_MODS=0` no file is read: the
//! replays of the baseline gate set it, so the accepted pictures do not depend on what is
//! installed.
//!
//! The hooks of interface 1 in this module:
//!
//! - `XE Mod Shadow`: `fn shadow_delta(s0: vec3f, s1: vec3f) -> f32` (`shadowDeltaZ`: the
//!   depth of the shadow map less that of the pixel, from the pixel in the spaces of the two
//!   layers; a positive number where the map has nothing nearer to the sun) and
//!   `fn shadow_esm(dz: f32) -> f32` (`shadowESM`: the shadow amount, 0 to 1, of a negative
//!   `dz`). Their homes: the draws of the game (the world shader), the grass, the surfaces of
//!   the water volumes. Their library: `shadow_map_depth(layer: u32, uv: vec2f) -> f32` (the
//!   map at level 0, layer 0 or 1, `uv` 0 to 1 over the layer), `shadow_texel() -> f32` (1
//!   over the resolution of a layer), `shadow_esm_scale() -> f32` (the scale of the map's
//!   depth) and `shadow_esm_values() -> vec2f` (`esm_c` and `esm_bias` of `XE Mod Shadow
//!   Data`).
//! - `XE Mod Shadow Data`: no code, the line `//! values: shade = 0.4, shade_color = 1.0 0.97
//!   0.81, esm_c = 60, esm_bias = 0.12` (each key may be left out). The scale of the map's depth
//!   stays out: it matches the map's format. A value out of its range drops the file and the
//!   Shadow file with it: `shade` 0.01 or more, each number of `shade_color` 0 to 1, `esm_c`
//!   above 0 and at most 88, `esm_bias` any number.
//! - `XE Mod Water`: `fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) ->
//!   vec3f` (`getFinalWaterNormal`), `fn water_color(p: WaterPixel) -> vec4f` (`WaterPS`) and
//!   `fn underwater_color(p: WaterPixel) -> vec4f` (`UnderwaterPS`), with the struct
//!   [`WATER_INTERFACE`]. Their home: the plane of the water only. The surfaces of the water
//!   volumes and the dry spaces keep the built-ins, as Direct3D 9 has a core file of its own
//!   for the volumes. The library is [`WATER_LIBRARY`]. Only the built-in `water_color` and
//!   `underwater_color` show the terms of the debug view `water`.
//! - `XE Mod Sky`: `fn clouds_color(p: CloudPixel) -> vec4f` (`CloudsPS`): the colour and the
//!   alpha of a pixel of the clouds, final (no light, no tone curve, no fog after it; the draw's
//!   alpha test still acts on it). Its home: the world shader, for the cloud draws where the sky
//!   of MGE XE is drawn (distant land with the scattering, in a place like an exterior), as
//!   Direct3D 9 runs `CloudsPS` there. Only a file that defines it changes the clouds: there is
//!   no built-in in that path. The struct and the texture read are [`SKY_INTERFACE`], the
//!   library [`SKY_LIBRARY`]. `sky_color` (`SkyPS`) is not a hook yet: no installed mod changes
//!   it.
//! - `XE Mod Particles`: `fn particle_color(p: ParticlePixel) -> vec4f`: the colour and the
//!   alpha of a pixel of a particle after the draw's texture stages. The fog, the light, the
//!   tone curve, the alpha test and the draw's blend, depth and stencil state stay the
//!   renderer's, after the hook. Its home: the world shader, for each draw of a particle system
//!   of the game, in every scene and into every target. Direct3D 9 has no core file for it: the
//!   hook is new. The struct and the texture read are [`PARTICLE_INTERFACE`], the library
//!   [`PARTICLE_LIBRARY`]. The built-in returns `p.color`.
//!
//! A hook may call the other hooks of its file, also those that the file leaves to the
//! built-ins. A hook that never ends stops the GPU: the renderer does not limit a hook.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::naga;

use super::shadow;

/// The interface that the first line of a file names.
pub(crate) const INTERFACE: &str = "mge-xe-core 1";

/// The folder of the files, under `Data Files`.
const FOLDER: &str = "shaders/core-mods";

/// The core files of the Direct3D 9 path that a mod can replace with an `.fx`: such a file
/// without its `.wgsl` is named in the log.
const DIRECT3D9_FILES: [&str; 10] = [
    "Shadow",
    "Shadow Data",
    "Water",
    "Water Standard",
    "Water Volume",
    "Caustics",
    "Grass",
    "Landscape",
    "Sky",
    "Statics",
];

/// A function of the renderer that a core mod may replace: its name, the types of its
/// arguments and of its result as naga writes them, and its built-in WGSL.
struct Hook {
    name: &'static str,
    arguments: &'static [&'static str],
    result: &'static str,
    built_in: &'static str,
}

const SHADOW_HOOKS: [Hook; 2] = [
    Hook {
        name: "shadow_delta",
        arguments: &["vec3<f32>", "vec3<f32>"],
        result: "f32",
        built_in: SHADOW_DELTA,
    },
    Hook {
        name: "shadow_esm",
        arguments: &["f32"],
        result: "f32",
        built_in: SHADOW_ESM,
    },
];

const SHADOW_DELTA: &str = "
// shadowDeltaZ: the depth of the shadow map less that of the pixel, in the layer that has
// the pixel; a positive number when the map has nothing nearer to the sun.
fn shadow_delta(s0: vec3f, s1: vec3f) -> f32 {
    let rcp = shadow_texel();
    let margin = vec3f(1.0 - 8.0 * rcp, 1.0 - 8.0 * rcp, 1.0);
    if all(abs(s0) < margin) {
        let uv = vec2f(0.5 + 0.5 * s0.x, 0.5 - 0.5 * s0.y);
        return shadow_map_depth(0u, uv) / shadow_esm_scale() - s0.z;
    }
    if all(abs(s1) < margin) {
        let uv = vec2f(0.5 + 0.5 * s1.x, 0.5 - 0.5 * s1.y);
        return shadow_map_depth(1u, uv) / shadow_esm_scale() - s1.z;
    }
    return 1e-6;
}
";

const SHADOW_ESM: &str = "
// shadowESM: the shadow amount of a pixel behind the map by `dz` (a negative number).
fn shadow_esm(dz: f32) -> f32 {
    let esm = shadow_esm_values();
    return 1.0 - clamp(exp(esm.x * dz + esm.y), 0.0, 1.0);
}
";

/// The library of the shadow hooks as declarations, for the check of a file alone. Each home
/// has its own bodies.
pub(crate) const SHADOW_LIBRARY: &str = "
fn shadow_map_depth(layer: u32, uv: vec2f) -> f32 { return 0.0; }
fn shadow_texel() -> f32 { return 0.0; }
fn shadow_esm_scale() -> f32 { return 1.0; }
fn shadow_esm_values() -> vec2f { return vec2f(0.0); }
";

/// The names of the functions of [`SHADOW_LIBRARY`].
const SHADOW_LIBRARY_NAMES: [&str; 4] = ["shadow_map_depth", "shadow_texel", "shadow_esm_scale", "shadow_esm_values"];

const WATER_HOOKS: [Hook; 3] = [
    Hook {
        name: "water_normal",
        arguments: &["vec4<f32>", "f32", "vec2<f32>"],
        result: "vec3<f32>",
        built_in: super::water::WATER_NORMAL,
    },
    Hook {
        name: "water_color",
        arguments: &["WaterPixel"],
        result: "vec4<f32>",
        built_in: super::water::WATER_COLOR,
    },
    Hook {
        name: "underwater_color",
        arguments: &["WaterPixel"],
        result: "vec4<f32>",
        built_in: super::water::UNDERWATER_COLOR,
    },
];

/// The struct of the water hooks, part of the interface: it joins the plane's shader.
pub(crate) const WATER_INTERFACE: &str = "
// The pixel of the plane of the water for the hooks of `XE Mod Water`: its place in the
// picture (0 to 1) and its view depth (the clip w); the two wave coordinates (far in xy, close
// in zw); its place in the world; the direction from the eye to it (length 1) and its
// distance; where the reflection is read (with the dynamic waves, the point that is not above
// the plane).
struct WaterPixel {
    uv: vec2f,
    clip_w: f32,
    texcoords: vec4f,
    world: vec3f,
    eye_vec: vec3f,
    distance: f32,
    reflect_uv: vec2f,
    reflect_w: f32,
}
";

/// The library of the water hooks as declarations, for the check of a file alone. The plane's
/// shader has the bodies (`water.rs`, `shading.rs`):
///
/// - `reflection_at(uv, w, stretch)`: the reflection at a place of the picture, at the view
///   depth `w`, with the blur that is sharp at the waterline (`stretch` 1 or more makes it
///   taller);
/// - `fog_water(direction, distance)`: the fog over the water, rgb the colour it adds and a
///   what is left of the colour of the thing;
/// - `sun_glint(eye, normal, fog_a)`: the light of the sun that the water mirrors to the eye;
/// - `deep_color(fog, tint)`: the colour of deep water with the fog over it (`tint` black for
///   the usual colour);
/// - `transmission(tint, depth)`: what is left of light through this much water of a colour;
/// - `sky_color(direction)`: the colour of the sky in a direction;
/// - `picture_at(uv)`: the picture behind the water (the refraction texture of Direct3D 9);
/// - `ground_distance_at(uv)`: the view depth of the nearest thing at a place of the picture,
///   the distant land too;
/// - `wave_at(coords)`: the volume of the waves (x and y the place, z the time);
/// - `ripples_on(normal, close, world_xy)`: `normal` with the ripples of the rain (at the close
///   wave coordinates) and of the player (at the place) added; unchanged without the dynamic
///   waves;
/// - the values of the frame, each the variable of Direct3D 9 that it stands for: the time
///   (`time`), the direction to the sun (`sunPos`), its visibility (`sunVis`), its colour
///   (`sunCol`: without the sunlight factor), the wind (`windVec`), the sky colour (`skyCol`),
///   the far fog colour (`fogColFar`), the place of the eye (`eyePos`), the ambient light
///   (`sunAmb`: the sun's ambient and the scene's together), 1 over the size of the picture
///   (`rcpRes`), and the height of the dynamic waves (`waveHeight`, 0 without them). The sun
///   colour and the ambient light have the light multipliers of the weather in them. Direct3D 9
///   applies these multipliers only with per-pixel lighting on (the default); the wgpu
///   renderer applies them always. A value that the built-in derives from these (the sunlight
///   factor, the wind factor) is not in the library.
/// - `water_is_interior()`: true in a plain interior, false in an exterior and in an interior
///   that behaves like one. A mod tells an interior by this fact, not by `sunCol > 0.003`;
///   the raw `water_sun_color()` stays for a faithful port.
pub(crate) const WATER_LIBRARY: &str = "
fn reflection_at(uv: vec2f, w: f32, stretch: f32) -> vec3f { return vec3f(0.0); }
fn fog_water(direction: vec3f, distance: f32) -> vec4f { return vec4f(0.0); }
fn sun_glint(eye_vec: vec3f, normal: vec3f, fog_a: f32) -> vec3f { return vec3f(0.0); }
fn deep_color(fog: vec4f, tint: vec3f) -> vec3f { return vec3f(0.0); }
fn transmission(tint: vec3f, depth: f32) -> vec3f { return vec3f(0.0); }
fn sky_color(direction: vec3f) -> vec3f { return vec3f(0.0); }
fn picture_at(uv: vec2f) -> vec3f { return vec3f(0.0); }
fn ground_distance_at(uv: vec2f) -> f32 { return 0.0; }
fn wave_at(coords: vec3f) -> vec4f { return vec4f(0.0); }
fn ripples_on(normal: vec2f, close: vec2f, world_xy: vec2f) -> vec2f { return vec2f(0.0); }
fn water_time() -> f32 { return 0.0; }
fn water_sun_direction() -> vec3f { return vec3f(0.0); }
fn water_sun_visibility() -> f32 { return 0.0; }
fn water_sun_color() -> vec3f { return vec3f(0.0); }
fn water_wind() -> vec2f { return vec2f(0.0); }
fn water_sky_color() -> vec3f { return vec3f(0.0); }
fn water_fog_color() -> vec3f { return vec3f(0.0); }
fn water_eye() -> vec3f { return vec3f(0.0); }
fn water_ambient_color() -> vec3f { return vec3f(0.0); }
fn water_texel() -> vec2f { return vec2f(0.0); }
fn water_wave_height() -> f32 { return 0.0; }
fn water_is_interior() -> bool { return false; }
";

/// The names of [`WATER_LIBRARY`] and of the struct of [`WATER_INTERFACE`].
const WATER_LIBRARY_NAMES: [&str; 23] = [
    "reflection_at",
    "fog_water",
    "sun_glint",
    "deep_color",
    "transmission",
    "sky_color",
    "picture_at",
    "ground_distance_at",
    "wave_at",
    "ripples_on",
    "water_time",
    "water_sun_direction",
    "water_sun_visibility",
    "water_sun_color",
    "water_wind",
    "water_sky_color",
    "water_fog_color",
    "water_eye",
    "water_ambient_color",
    "water_texel",
    "water_wave_height",
    "water_is_interior",
    "WaterPixel",
];

const SKY_HOOKS: [Hook; 1] = [Hook {
    name: "clouds_color",
    arguments: &["CloudPixel"],
    result: "vec4<f32>",
    built_in: CLOUDS_COLOR,
}];

/// The built-in `clouds_color` (`CloudsPS` of `XE Mod Sky.fx`): the colour of the vertex times
/// the clouds' texture. `cloud_texture` reads the largest level, and `CloudsPS` a smaller level
/// where the texture is made smaller on the screen: the two are equal only where it is not.
const CLOUDS_COLOR: &str = "
// CloudsPS: the colour of a pixel of the clouds.
fn clouds_color(p: CloudPixel) -> vec4f {
    return p.color * cloud_texture(p.texcoords);
}
";

/// The struct of the clouds hook and the texture read, part of the interface, as declarations
/// for the check of a file alone. The world shader of `nirender_wgpu` defines both (a test
/// compares them): `CloudPixel` has the coordinate of the clouds' texture (`IN.texcoords`), the
/// colour of the vertex (`IN.color`), the direction from the eye, length 1, in the space of the
/// world (`normalize(IN.skypos)`), and the screen pixel; `cloud_texture(uv)` reads the clouds'
/// texture with its sampler at level 0 (`tex2Dlod(sampBaseTex, ...)`), also in a branch.
pub(crate) const SKY_INTERFACE: &str = "
struct CloudPixel {
    texcoords: vec2f,
    color: vec4f,
    direction: vec3f,
    pixel: vec2f,
}
fn cloud_texture(uv: vec2f) -> vec4f { return vec4f(0.0); }
";

/// The library of the clouds hook as declarations, for the check of a file alone. The world
/// shader has the bodies (`SKY_LIBRARY_BODIES` of the renderer):
///
/// - `sky_fog_color(direction)`: the colour of the fog of the sky in a direction from the eye
///   (`fogColourSky` of `XE Common.fx`), as the sky dome has it;
/// - the values of the frame, each the variable of Direct3D 9 that it stands for: the time
///   (`time`), the sun colour (`sunCol`), the ambient light (`sunAmb`: the sun's ambient and
///   the scene's together), the direction to the sun (`sunPos`), its visibility (`sunVis`), the
///   sky colour (`skyCol`) and the far fog colour (`fogColFar`). The sun colour and the ambient
///   light have the light multipliers of the weather in them, as for the water.
pub(crate) const SKY_LIBRARY: &str = "
fn sky_fog_color(direction: vec3f) -> vec4f { return vec4f(0.0); }
fn sky_time() -> f32 { return 0.0; }
fn sky_sun_color() -> vec3f { return vec3f(0.0); }
fn sky_ambient_color() -> vec3f { return vec3f(0.0); }
fn sky_sun_position() -> vec3f { return vec3f(0.0); }
fn sky_sun_visibility() -> f32 { return 0.0; }
fn sky_color_value() -> vec3f { return vec3f(0.0); }
fn sky_fog_far_color() -> vec3f { return vec3f(0.0); }
";

/// The names of [`SKY_LIBRARY`] and of [`SKY_INTERFACE`].
const SKY_LIBRARY_NAMES: [&str; 10] = [
    "sky_fog_color",
    "sky_time",
    "sky_sun_color",
    "sky_ambient_color",
    "sky_sun_position",
    "sky_sun_visibility",
    "sky_color_value",
    "sky_fog_far_color",
    "cloud_texture",
    "CloudPixel",
];

const PARTICLE_HOOKS: [Hook; 1] = [Hook {
    name: "particle_color",
    arguments: &["ParticlePixel"],
    result: "vec4<f32>",
    built_in: PARTICLE_COLOR,
}];

/// The built-in `particle_color`: the colour of the texture stages, unchanged.
const PARTICLE_COLOR: &str = "
// The colour of a pixel of a particle after the texture stages.
fn particle_color(p: ParticlePixel) -> vec4f {
    return p.color;
}
";

/// The struct of the particle hook and the functions of the draw, part of the interface, as
/// declarations for the check of a file alone. The world shader of `nirender_wgpu` defines them
/// (a test compares them): `ParticlePixel` has the colour after the texture stages, the
/// particle's own colour and alpha from the game (its fade over its life is in it), the
/// coordinate of the first texture stage (0 to 1 across a particle's quad without a texture
/// transform), the screen pixel, and the depth along the view in units of the world;
/// `particle_texture(uv)` reads the draw's first texture with its sampler at the level of the
/// stage's own read, also in a branch; `particle_adds()` is true for a draw that adds to the
/// picture.
pub(crate) const PARTICLE_INTERFACE: &str = "
struct ParticlePixel {
    color: vec4f,
    vertex_color: vec4f,
    texcoords: vec2f,
    pixel: vec2f,
    distance: f32,
}
fn particle_texture(uv: vec2f) -> vec4f { return vec4f(0.0); }
fn particle_adds() -> bool { return false; }
";

/// The library of the particle hook as declarations, for the check of a file alone. The world
/// shader has the bodies (`PARTICLE_LIBRARY_BODIES` of the renderer): `particle_time()`, the
/// time of the simulation of the game in seconds, the same value as `sky_time()` (`time` of
/// Direct3D 9).
pub(crate) const PARTICLE_LIBRARY: &str = "
fn particle_time() -> f32 { return 0.0; }
";

/// The names of [`PARTICLE_LIBRARY`] and of [`PARTICLE_INTERFACE`].
const PARTICLE_LIBRARY_NAMES: [&str; 4] = ["particle_time", "particle_texture", "particle_adds", "ParticlePixel"];

/// The values of `XE Mod Shadow Data.fx` that a core mod may set: the shade of a shadow and
/// the two numbers of the exponential shadow map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowValues {
    pub shade: f32,
    pub shade_color: [f32; 3],
    pub esm_c: f32,
    pub esm_bias: f32,
}

impl Default for ShadowValues {
    fn default() -> ShadowValues {
        ShadowValues {
            shade: shadow::SHADE,
            shade_color: shadow::SHADE_COLOR,
            esm_c: shadow::ESM_C,
            esm_bias: shadow::ESM_BIAS,
        }
    }
}

impl ShadowValues {
    /// The ESM vector of the shaders: c, the bias, the scale of the map's depth, and `rcp` (1
    /// over the resolution of a layer).
    pub fn esm(&self, rcp: f32) -> [f32; 4] {
        [self.esm_c, self.esm_bias, shadow::ESM_SCALE, rcp]
    }

    /// The shade vector of the shaders: `shadecolor`, and `shade` of `shadowSunEstimate`.
    pub fn shade(&self) -> [f32; 4] {
        let [r, g, b] = self.shade_color;
        [r, g, b, self.shade]
    }

    /// The values of a `values:` line, over the built-in ones.
    fn parse(line: &str) -> Result<ShadowValues, String> {
        let mut values = ShadowValues::default();
        for item in line.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            let Some((key, numbers)) = item.split_once('=') else {
                return Err(format!("the value `{item}` has no `name = number`"));
            };
            let numbers: Vec<f32> = numbers
                .split_whitespace()
                .map(|word| {
                    word.parse::<f32>()
                        .ok()
                        .filter(|number| number.is_finite())
                        .ok_or_else(|| format!("`{word}` of the value {} is not a number", key.trim()))
                })
                .collect::<Result<_, _>>()?;
            let key = key.trim();
            let count = if key == "shade_color" { 3 } else { 1 };
            if numbers.len() != count {
                return Err(format!("the value {key} has {} numbers, not {count}", numbers.len()));
            }
            match key {
                "shade" => values.shade = numbers[0],
                "shade_color" => values.shade_color = [numbers[0], numbers[1], numbers[2]],
                "esm_c" => values.esm_c = numbers[0],
                "esm_bias" => values.esm_bias = numbers[0],
                other => {
                    return Err(format!(
                        "the value `{other}` is not known (shade, shade_color, esm_c, esm_bias)"
                    ));
                }
            }
        }
        // `shadowSunEstimate` divides by the shade plus the light, which is 0 at night: 0/0 at a
        // shade of 0.
        if values.shade < 0.01 {
            return Err(format!(
                "the value shade = {} is out of its range: 0.01 or more",
                values.shade
            ));
        }
        if values.shade_color.iter().any(|part| !(0.0..=1.0).contains(part)) {
            let [r, g, b] = values.shade_color;
            return Err(format!(
                "the value shade_color = {r} {g} {b} is out of its range: 0 to 1 in each number"
            ));
        }
        if !(values.esm_c > 0.0 && values.esm_c <= 88.0) {
            return Err(format!(
                "the value esm_c = {} is out of its range: above 0, at most 88",
                values.esm_c
            ));
        }
        Ok(values)
    }
}

/// A shader that calls the hooks of a file: its name for the log, and the device's check of
/// its whole source with a hook text in it.
#[derive(Clone, Copy)]
pub(crate) struct Home {
    pub name: &'static str,
    pub check: fn(&str) -> Result<(), String>,
}

/// The core mods of the session: the hook text for the homes of each file, and the values.
pub(crate) struct CoreMods {
    /// The text of the shadow hooks: the hooks of the file, and the built-ins of the others.
    shadow_hooks: String,
    shadow_values: ShadowValues,
    /// The text of the water hooks for the plane, the same way.
    water_hooks: String,
    /// The water hooks of the plane for the debug view `water`: the built-in colour hooks with
    /// the file's other hooks ([`water_debug_hooks`]).
    water_debug_hooks: String,
    /// The text of the clouds hook of a file that defines it, or [`None`]: the clouds then
    /// stay as the game draws them.
    sky_hooks: Option<String>,
    /// The text of the particle hook of a file that defines it, or [`None`]: the particles then
    /// keep the colour of their texture stages, with no call.
    particle_hooks: Option<String>,
    /// What the log says about the files, one line each.
    report: Vec<String>,
    /// The problems for the debug overlay: the files that a build marker drops.
    problems: Vec<String>,
    /// The texts of the files and the homes, for the hooks of the session without a file.
    texts: Texts,
    homes: Homes,
}

impl CoreMods {
    /// The built-in hooks only.
    pub fn built_in() -> CoreMods {
        CoreMods {
            shadow_hooks: hook_text(&SHADOW_HOOKS, &[], ""),
            shadow_values: ShadowValues::default(),
            water_hooks: hook_text(&WATER_HOOKS, &[], ""),
            water_debug_hooks: hook_text(&WATER_HOOKS, &[], ""),
            sky_hooks: None,
            particle_hooks: None,
            report: Vec::new(),
            problems: Vec::new(),
            texts: Texts::default(),
            homes: Homes::default(),
        }
    }

    /// The files under `Data Files` (`read` gives the bytes of a path there), checked with the
    /// homes of the shadow hooks, of the water hooks, of the clouds hook and of the particle
    /// hook; nothing with `MGE_XE_CORE_MODS=0`. A file that a build marker of `markers` names is
    /// not used. The report is printed.
    pub fn load(
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
        shadow_homes: &[Home],
        water_homes: &[Home],
        sky_homes: &[Home],
        particle_homes: &[Home],
        markers: &BuildMarkers,
    ) -> CoreMods {
        let mods = if off() {
            CoreMods {
                report: vec!["core mods: off (MGE_XE_CORE_MODS=0)".to_string()],
                ..CoreMods::built_in()
            }
        } else {
            CoreMods::read(read, shadow_homes, water_homes, sky_homes, particle_homes, markers)
        };
        for line in &mods.report {
            println!("{line}");
        }
        mods
    }

    /// [`load`](Self::load) without the switch and the print.
    fn read(
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
        shadow_homes: &[Home],
        water_homes: &[Home],
        sky_homes: &[Home],
        particle_homes: &[Home],
        markers: &BuildMarkers,
    ) -> CoreMods {
        // A byte order mark, which an editor on Windows can put first, is not part of the text.
        let file = |name: &str, extension: &str| {
            read(&format!("{FOLDER}/XE Mod {name}.{extension}"))
                .map(|bytes| String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
        };
        let mut report = Vec::new();
        for name in DIRECT3D9_FILES {
            if file(name, "wgsl").is_none() && file(name, "fx").is_some() {
                report.push(format!("core mod: XE Mod {name}.fx is for Direct3D 9; the wgpu renderer reads XE Mod {name}.wgsl, and there is none"));
            }
        }
        let mut texts = Texts {
            shadow: file("Shadow", "wgsl"),
            shadow_data: file("Shadow Data", "wgsl"),
            water: file("Water", "wgsl"),
            sky: file("Sky", "wgsl"),
            particles: file("Particles", "wgsl"),
        };
        let mut problems = Vec::new();
        for (dropped, home) in markers.left(&texts) {
            texts = texts.without(dropped);
            problems.push(format!(
                "core mod: {}: a start before ended in the build of the {home} with it; it is not used until it changes",
                dropped.label()
            ));
        }
        report.extend(problems.iter().cloned());
        let homes = Homes {
            shadow: shadow_homes.to_vec(),
            water: water_homes.to_vec(),
            sky: sky_homes.to_vec(),
            particles: particle_homes.to_vec(),
        };
        let mut mods = CoreMods::of_texts(texts, homes);
        report.append(&mut mods.report);
        mods.report = report;
        mods.problems = problems;
        mods
    }

    /// The hooks of the files of `texts`, checked with `homes`.
    fn of_texts(texts: Texts, homes: Homes) -> CoreMods {
        let mut mods = CoreMods::built_in();
        let (shadow_homes, water_homes, sky_homes, particle_homes) =
            (&homes.shadow[..], &homes.water[..], &homes.sky[..], &homes.particles[..]);
        let mut used = Vec::new();
        let (shadow_file, data_file) = (texts.shadow.clone(), texts.shadow_data.clone());
        if shadow_file.is_some() || data_file.is_some() {
            match shadow_mod(shadow_file.as_deref(), data_file.as_deref(), shadow_homes) {
                Ok(shadow) => {
                    mods.shadow_hooks = shadow.hooks;
                    mods.shadow_values = shadow.values;
                    if shadow_file.is_some() {
                        used.push(format!(
                            "XE Mod Shadow.wgsl ({})",
                            if shadow.defined.is_empty() {
                                "no hook".to_string()
                            } else {
                                shadow.defined.join(", ")
                            }
                        ));
                    }
                    if let Some(values) = shadow.values_line {
                        used.push(format!("XE Mod Shadow Data.wgsl ({values})"));
                    }
                }
                Err((name, problem)) => {
                    let other = if name == "Shadow" { "Shadow Data" } else { "Shadow" };
                    let with = if shadow_file.is_some() && data_file.is_some() {
                        format!("; XE Mod {other}.wgsl is dropped with it")
                    } else {
                        String::new()
                    };
                    mods.report.push(format!(
                        "core mod: XE Mod {name}.wgsl: {problem}; the built-in shadow is used{with}"
                    ));
                }
            }
        }
        if let Some(text) = &texts.water {
            match hook_file(
                text,
                &WATER_HOOKS,
                &format!("{WATER_INTERFACE}{WATER_LIBRARY}"),
                &WATER_LIBRARY_NAMES,
                water_homes,
            ) {
                Ok((hooks, defined)) => {
                    mods.water_hooks = hooks;
                    // The debug text must pass the homes as the file's text does; else the
                    // view shows the built-in water.
                    let debug = water_debug_hooks(text, &defined);
                    if water_homes.iter().all(|home| (home.check)(&debug).is_ok()) {
                        mods.water_debug_hooks = debug;
                    }
                    used.push(format!(
                        "XE Mod Water.wgsl ({})",
                        if defined.is_empty() {
                            "no hook".to_string()
                        } else {
                            defined.join(", ")
                        }
                    ));
                    mods.report.push("core mod: XE Mod Water.wgsl: the plane of the water takes its hooks; the water volumes and the dry spaces keep the built-in water".to_string());
                    if defined.iter().any(|hook| WATER_COLOUR_HOOKS.contains(hook)) {
                        mods.report.push("core mod: XE Mod Water.wgsl: the debug view water shows the terms of the built-in colour on the surface of the file".to_string());
                    }
                }
                Err(problem) => mods
                    .report
                    .push(format!("core mod: XE Mod Water.wgsl: {problem}; the built-in water is used")),
            }
        }
        if let Some(text) = &texts.sky {
            match hook_file(
                text,
                &SKY_HOOKS,
                &format!("{SKY_INTERFACE}{SKY_LIBRARY}"),
                &SKY_LIBRARY_NAMES,
                sky_homes,
            ) {
                Ok((_, defined)) if defined.is_empty() => used.push("XE Mod Sky.wgsl (no hook)".to_string()),
                Ok((hooks, defined)) => {
                    mods.sky_hooks = Some(hooks);
                    used.push(format!("XE Mod Sky.wgsl ({})", defined.join(", ")));
                    mods.report.push("core mod: XE Mod Sky.wgsl: the clouds take its clouds_color where the sky of MGE XE is drawn (distant land with the scattering, in a place like an exterior)".to_string());
                }
                Err(problem) => mods.report.push(format!(
                    "core mod: XE Mod Sky.wgsl: {problem}; the clouds stay as the game draws them"
                )),
            }
        }
        if let Some(text) = &texts.particles {
            match hook_file(
                text,
                &PARTICLE_HOOKS,
                &format!("{PARTICLE_INTERFACE}{PARTICLE_LIBRARY}"),
                &PARTICLE_LIBRARY_NAMES,
                particle_homes,
            ) {
                Ok((_, defined)) if defined.is_empty() => used.push("XE Mod Particles.wgsl (no hook)".to_string()),
                Ok((hooks, defined)) => {
                    mods.particle_hooks = Some(hooks);
                    used.push(format!("XE Mod Particles.wgsl ({})", defined.join(", ")));
                }
                Err(problem) => mods.report.push(format!(
                    "core mod: XE Mod Particles.wgsl: {problem}; the particles stay as the game draws them"
                )),
            }
        }
        mods.report.push(if used.is_empty() {
            format!("core mods: none used (Data Files/{FOLDER}, interface {INTERFACE})")
        } else {
            format!("core mods: {} (interface {INTERFACE})", used.join(", "))
        });
        mods.texts = texts;
        mods.homes = homes;
        mods
    }

    /// The hooks of the session without the file `dropped`: its hooks take the built-ins.
    fn without(&self, dropped: ModFile) -> CoreMods {
        let mut mods = CoreMods::of_texts(self.texts.without(dropped), self.homes.clone());
        mods.problems = self.problems.clone();
        mods
    }

    /// The hooks of `file` join a home: the session uses the file.
    pub fn uses(&self, file: ModFile) -> bool {
        match file {
            ModFile::Shadow => self.shadow_hooks != hook_text(&SHADOW_HOOKS, &[], ""),
            ModFile::Water => self.water_hooks != hook_text(&WATER_HOOKS, &[], ""),
            ModFile::Sky => self.sky_hooks.is_some(),
            ModFile::Particles => self.particle_hooks.is_some(),
        }
    }

    /// The hook text of `file` for its homes: the shadow hooks, the water hooks of the plane, the
    /// clouds hook or the particle hook (empty without one).
    fn hooks(&self, file: ModFile) -> String {
        match file {
            ModFile::Shadow => self.shadow_hooks.clone(),
            ModFile::Water => self.water_hooks.clone(),
            ModFile::Sky => self.sky_hooks.clone().unwrap_or_default(),
            ModFile::Particles => self.particle_hooks.clone().unwrap_or_default(),
        }
    }

    /// The shadow hooks for a home: the text joins the home's source after its library.
    pub fn shadow_hooks(&self) -> &str {
        &self.shadow_hooks
    }

    /// The water hooks for the plane of the water: the text joins the plane's source after its
    /// library and [`WATER_INTERFACE`].
    #[cfg(test)]
    pub fn water_hooks(&self) -> &str {
        &self.water_hooks
    }

    /// The water hooks of the plane for the debug view `water`: those of
    /// [`water_hooks`](Self::water_hooks) with the built-in colour hooks.
    #[cfg(test)]
    pub fn water_debug_hooks(&self) -> &str {
        &self.water_debug_hooks
    }

    /// The clouds hook of a file that defines it, for the world shader after
    /// [`SKY_INTERFACE`] and the library; [`None`] without one.
    #[cfg(test)]
    pub fn sky_hooks(&self) -> Option<&str> {
        self.sky_hooks.as_deref()
    }

    /// The particle hook of a file that defines it, for the world shader after
    /// [`PARTICLE_INTERFACE`] and the library; [`None`] without one.
    #[cfg(test)]
    pub fn particle_hooks(&self) -> Option<&str> {
        self.particle_hooks.as_deref()
    }

    /// The core mods of `files` (names under the folder, and texts), for a test of a home. The
    /// particle hook has no home here: [`of_files_with_particles`](Self::of_files_with_particles).
    #[cfg(test)]
    pub fn of_files(files: &[(&str, &str)], shadow_homes: &[Home], water_homes: &[Home], sky_homes: &[Home]) -> CoreMods {
        Self::of_files_with_particles(files, shadow_homes, water_homes, sky_homes, &[])
    }

    /// [`of_files`](Self::of_files) with the homes of the particle hook.
    #[cfg(test)]
    pub fn of_files_with_particles(
        files: &[(&str, &str)],
        shadow_homes: &[Home],
        water_homes: &[Home],
        sky_homes: &[Home],
        particle_homes: &[Home],
    ) -> CoreMods {
        let files: std::collections::HashMap<String, String> = files
            .iter()
            .map(|(name, text)| (format!("{FOLDER}/XE Mod {name}"), text.to_string()))
            .collect();
        CoreMods::read(
            &|path| files.get(path).map(|text| text.as_bytes().to_vec()),
            shadow_homes,
            water_homes,
            sky_homes,
            particle_homes,
            &BuildMarkers::default(),
        )
    }

    /// The lines of the log about the files.
    #[cfg(test)]
    fn report(&self) -> &[String] {
        &self.report
    }
}

/// A unit of the core mods that a fault on the device drops as one (W317): the shadow
/// (`XE Mod Shadow` with `XE Mod Shadow Data`), the water, the sky, or the particles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModFile {
    Shadow,
    Water,
    Sky,
    Particles,
}

impl ModFile {
    const ALL: [ModFile; 4] = [ModFile::Shadow, ModFile::Water, ModFile::Sky, ModFile::Particles];

    /// The name of the unit in a build marker and in `MGE_XE_CORE_MOD_FAULT`.
    fn key(self) -> &'static str {
        match self {
            ModFile::Shadow => "Shadow",
            ModFile::Water => "Water",
            ModFile::Sky => "Sky",
            ModFile::Particles => "Particles",
        }
    }

    fn of_key(key: &str) -> Option<ModFile> {
        ModFile::ALL
            .into_iter()
            .find(|file| file.key().eq_ignore_ascii_case(key.trim()))
    }

    /// The files of the unit, for the log.
    pub fn label(self) -> &'static str {
        match self {
            ModFile::Shadow => "XE Mod Shadow.wgsl (with XE Mod Shadow Data.wgsl)",
            ModFile::Water => "XE Mod Water.wgsl",
            ModFile::Sky => "XE Mod Sky.wgsl",
            ModFile::Particles => "XE Mod Particles.wgsl",
        }
    }

    /// The world shader takes the hooks of the unit.
    pub fn in_world(self) -> bool {
        self != ModFile::Water
    }
}

/// The texts of the files as the start read them.
#[derive(Clone, Default)]
struct Texts {
    shadow: Option<String>,
    shadow_data: Option<String>,
    water: Option<String>,
    sky: Option<String>,
    particles: Option<String>,
}

impl Texts {
    fn without(&self, file: ModFile) -> Texts {
        let mut texts = self.clone();
        match file {
            ModFile::Shadow => {
                texts.shadow = None;
                texts.shadow_data = None;
            }
            ModFile::Water => texts.water = None,
            ModFile::Sky => texts.sky = None,
            ModFile::Particles => texts.particles = None,
        }
        texts
    }

    /// The text of the unit's file with the hooks: a build marker names it by this text.
    fn hook_text(&self, file: ModFile) -> Option<&str> {
        match file {
            ModFile::Shadow => self.shadow.as_deref(),
            ModFile::Water => self.water.as_deref(),
            ModFile::Sky => self.sky.as_deref(),
            ModFile::Particles => self.particles.as_deref(),
        }
    }
}

/// The homes of each file.
#[derive(Clone, Default)]
struct Homes {
    shadow: Vec<Home>,
    water: Vec<Home>,
    sky: Vec<Home>,
    particles: Vec<Home>,
}

/// The length of a text and its FNV-1a hash: a build marker names a file by them, so that a
/// changed file is used again.
fn identity(text: &str) -> (usize, u64) {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (text.len(), hash)
}

/// The start of the name of a build marker; the home follows, with `-` for each space.
const MARKER_PREFIX: &str = "core-mod-build-";

/// The build markers of the core mods (W317): a file in the user's folder
/// (`%LOCALAPPDATA%\MGE XE`, or the folder of `MGE_XE_CORE_MOD_MARKERS`) around the first build
/// of each home with the hooks of a mod. A build that ends the helper (a lost device, a fault
/// of the driver) leaves its marker. The next start does not use the files that the marker
/// names, until a file changes.
#[derive(Clone, Default)]
pub(crate) struct BuildMarkers {
    folder: Option<PathBuf>,
    /// The lines of the files that a marker held off at this start ([`BuildMarkers::left`]), each
    /// with the home of its marker. A new marker of that home keeps them, and its removal writes
    /// them back, so that a file stays held off until it changes.
    held_off: std::cell::RefCell<Vec<(String, String)>>,
}

impl BuildMarkers {
    /// The markers in the user's folder, or in the folder of `MGE_XE_CORE_MOD_MARKERS`.
    pub fn of_user() -> BuildMarkers {
        let folder = std::env::var_os("MGE_XE_CORE_MOD_MARKERS")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(|folder| PathBuf::from(folder).join("MGE XE")));
        BuildMarkers {
            folder,
            ..BuildMarkers::default()
        }
    }

    /// No marker is written into `mods_folder`, the folder of `MGE_XE_CORE_MODS_DIR`: its files
    /// are fixtures of a repository. When the marker folder is that folder, no marker is kept.
    pub fn apart_from(self, mods_folder: Option<&std::path::Path>) -> BuildMarkers {
        let same = |a: &std::path::Path, b: &std::path::Path| {
            std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf())
                == std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf())
        };
        match (&self.folder, mods_folder) {
            (Some(markers), Some(mods)) if same(markers, mods) => {
                println!(
                    "core mods: the build markers are not written: their folder is that of MGE_XE_CORE_MODS_DIR ({})",
                    mods.display()
                );
                BuildMarkers::default()
            }
            _ => self,
        }
    }

    #[cfg(test)]
    pub fn in_folder(folder: &std::path::Path) -> BuildMarkers {
        BuildMarkers {
            folder: Some(folder.to_path_buf()),
            ..BuildMarkers::default()
        }
    }

    fn path(&self, home: &str) -> Option<PathBuf> {
        let name = format!("{MARKER_PREFIX}{}.txt", home.replace(' ', "-"));
        self.folder.as_ref().map(|folder| folder.join(name))
    }

    /// Writes the marker of `home` with the files of its build: a line for each, with the unit,
    /// the length and the hash of its text. The lines that the home's marker held off at this
    /// start stay in it.
    fn write(&self, home: &str, files: &[(ModFile, String)]) {
        let built: String = files
            .iter()
            .map(|(file, text)| {
                let (length, hash) = identity(text);
                format!("{}\t{length}\t{hash:016x}\n", file.key())
            })
            .collect();
        self.write_lines(home, self.held_off_lines(home) + &built);
    }

    /// Takes down the marker of `home` after its build. The lines that it held off at this start
    /// stay: their files are held off until they change.
    fn remove(&self, home: &str) {
        let held_off = self.held_off_lines(home);
        if !held_off.is_empty() {
            self.write_lines(home, held_off);
        } else if let Some(path) = self.path(home) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn held_off_lines(&self, home: &str) -> String {
        self.held_off
            .borrow()
            .iter()
            .filter(|(of, _)| of == home)
            .map(|(_, line)| format!("{line}\n"))
            .collect()
    }

    fn write_lines(&self, home: &str, lines: String) {
        let Some(path) = self.path(home) else { return };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, lines));
        if let Err(error) = written {
            println!("core mods: no build marker {} ({error})", path.display());
        }
    }

    /// The files of `texts` that a marker names with the same text, each with the home of its
    /// marker. A marker whose files all changed is removed. The lines of the files held off are
    /// kept for the markers of this start ([`BuildMarkers::write`], [`BuildMarkers::remove`]).
    fn left(&self, texts: &Texts) -> Vec<(ModFile, String)> {
        self.held_off.borrow_mut().clear();
        let Some(entries) = self.folder.as_ref().and_then(|folder| std::fs::read_dir(folder).ok()) else {
            return Vec::new();
        };
        let mut left: Vec<(ModFile, String)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(home) = name.strip_prefix(MARKER_PREFIX).and_then(|rest| rest.strip_suffix(".txt")) else {
                continue;
            };
            let home = home.replace('-', " ");
            let marker = std::fs::read_to_string(entry.path()).unwrap_or_default();
            let unchanged: Vec<(ModFile, &str)> = marker
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split('\t');
                    let file = ModFile::of_key(parts.next()?)?;
                    let length: usize = parts.next()?.parse().ok()?;
                    let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
                    (texts.hook_text(file).map(identity) == Some((length, hash))).then_some((file, line))
                })
                .collect();
            if unchanged.is_empty() {
                let _ = std::fs::remove_file(entry.path());
            }
            for (file, line) in unchanged {
                self.held_off.borrow_mut().push((home.clone(), line.to_string()));
                if !left.iter().any(|(known, _)| *known == file) {
                    left.push((file, home.clone()));
                }
            }
        }
        left
    }
}

/// The fragment entry point of a pipeline that a plant of `MGE_XE_CORE_MOD_FAULT` breaks: no
/// module has it.
const PLANTED_ENTRY: &str = "fs_core_mod_fault_plant";

/// The fragment entry point `entry` of a home's pipeline, or [`PLANTED_ENTRY`] when the source
/// has the plant's line ([`nirender_wgpu::CORE_MOD_FAULT_PLANT`]).
pub(crate) fn fragment_entry(source: &str, entry: &'static str) -> &'static str {
    if source.contains(nirender_wgpu::CORE_MOD_FAULT_PLANT) {
        PLANTED_ENTRY
    } else {
        entry
    }
}

/// What a planted build does after its fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlantEnd {
    /// The fallback, as for a real fault.
    Fallback,
    /// The failed build stays: the null test of the fallback.
    Keep,
    /// The helper ends in the build, after the marker.
    Exit,
}

/// The plant of a test of the fallback (W317): `MGE_XE_CORE_MOD_FAULT=<file>[@<home>][:keep|:exit]`,
/// with `<file>` Shadow, Water, Sky or Particles and `<home>` a home's name of the log (`world shader`,
/// `grass`, `water plane`, `water volume`). The homes of the file (or the one home) build its
/// hooks with a fragment entry point that the module lacks: the device refuses the pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plant {
    file: ModFile,
    home: Option<String>,
    end: PlantEnd,
}

impl Plant {
    /// The plant of `MGE_XE_CORE_MOD_FAULT`, or [`None`].
    pub fn of_env() -> Option<Plant> {
        let text = std::env::var("MGE_XE_CORE_MOD_FAULT").ok()?;
        if text.trim() == "world key" {
            println!(
                "core mods: MGE_XE_CORE_MOD_FAULT plants a fault in the first world key, whatever the text (no core mod's)"
            );
            return None;
        }
        let plant = Plant::parse(&text);
        match &plant {
            Some(plant) => println!("core mods: MGE_XE_CORE_MOD_FAULT plants a fault: {plant:?}"),
            None => println!("core mods: MGE_XE_CORE_MOD_FAULT=`{text}` is not <file>[@<home>][:keep|:exit]"),
        }
        plant
    }

    fn parse(text: &str) -> Option<Plant> {
        let (rest, end) = match text.trim().rsplit_once(':') {
            Some((rest, "keep")) => (rest, PlantEnd::Keep),
            Some((rest, "exit")) => (rest, PlantEnd::Exit),
            Some(_) => return None,
            None => (text.trim(), PlantEnd::Fallback),
        };
        let (file, home) = match rest.split_once('@') {
            Some((file, home)) => (file, Some(home.trim().to_string())),
            None => (rest, None),
        };
        Some(Plant {
            file: ModFile::of_key(file)?,
            home,
            end,
        })
    }

    fn plants(&self, file: ModFile, home: &str) -> bool {
        self.file == file && self.home.as_deref().is_none_or(|planted| planted == home)
    }
}

/// The hooks of the world shader: the shadow hooks (the built-ins without a file), and the
/// clouds hook and the particle hook of files that define them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorldHooks {
    pub shadow: String,
    pub sky: Option<String>,
    pub particles: Option<String>,
}

/// The core mods of a running session, shared by the homes (W317). Each home takes its hooks at
/// its first build, so a file that failed on the device in one home is dropped for the homes
/// built after it.
#[derive(Clone)]
pub(crate) struct Shared(Arc<Mutex<Session>>);

struct Session {
    mods: CoreMods,
    markers: BuildMarkers,
    plant: Option<Plant>,
    /// The problems for the debug overlay.
    problems: Vec<String>,
    /// A home dropped a file that the world shader has: the world takes a new text.
    world_changed: bool,
}

impl Session {
    /// The hooks of `file` for `home`, with the plant's line when it plants there.
    fn hooks_for(&self, file: ModFile, home: &str) -> String {
        self.hooks_in(&self.mods, file, home)
    }

    /// The hooks of `file` in `mods` for `home`. The plant's line goes only on the hooks of a
    /// file that `mods` uses: the built-in hooks of a dropped file never take it.
    fn hooks_in(&self, mods: &CoreMods, file: ModFile, home: &str) -> String {
        let hooks = mods.hooks(file);
        let planted = mods.uses(file) && self.plant.as_ref().is_some_and(|plant| plant.plants(file, home));
        if planted {
            format!("{}\n{hooks}", nirender_wgpu::CORE_MOD_FAULT_PLANT)
        } else {
            hooks
        }
    }

    /// The hooks of the world shader in `mods`, with the plant's line where it plants.
    fn world_hooks_in(&self, mods: &CoreMods) -> WorldHooks {
        let optional = |file: ModFile| mods.uses(file).then(|| self.hooks_in(mods, file, WORLD_HOME));
        WorldHooks {
            shadow: self.hooks_in(mods, ModFile::Shadow, WORLD_HOME),
            sky: optional(ModFile::Sky),
            particles: optional(ModFile::Particles),
        }
    }

    /// Drops `file` from the session: its hooks take the built-ins in the homes built after now.
    fn drop_file(&mut self, file: ModFile, line: String) {
        if !self.mods.uses(file) {
            return;
        }
        println!("{line}");
        self.problems.push(line);
        self.mods = self.mods.without(file);
        self.world_changed |= file.in_world();
    }
}

impl Shared {
    pub fn new(mods: CoreMods, markers: BuildMarkers, plant: Option<Plant>) -> Shared {
        let problems = mods.problems.clone();
        Shared(Arc::new(Mutex::new(Session {
            mods,
            markers,
            plant,
            problems,
            world_changed: false,
        })))
    }

    /// The built-in hooks, with no marker and no plant.
    pub fn built_in() -> Shared {
        Shared::new(CoreMods::built_in(), BuildMarkers::default(), None)
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// [`CoreMods::shadow_hooks`] of the session now.
    pub fn shadow_hooks(&self) -> String {
        self.session().mods.shadow_hooks.clone()
    }

    pub fn shadow_values(&self) -> ShadowValues {
        self.session().mods.shadow_values
    }

    /// The water hooks of the plane for the debug view `water`, of the session now: the file's
    /// hooks with the built-in colour hooks ([`water_debug_hooks`]).
    pub fn water_debug_hooks(&self) -> String {
        self.session().mods.water_debug_hooks.clone()
    }

    /// The session has a clouds hook.
    pub fn has_sky_hooks(&self) -> bool {
        self.session().mods.sky_hooks.is_some()
    }

    /// The session has a particle hook.
    pub fn has_particle_hooks(&self) -> bool {
        self.session().mods.particle_hooks.is_some()
    }

    /// The problems for the debug overlay.
    pub fn problems(&self) -> Vec<String> {
        self.session().problems.clone()
    }

    /// The hooks of the world shader now, with the plant's line where it plants.
    pub fn world_hooks(&self) -> WorldHooks {
        let session = self.session();
        session.world_hooks_in(&session.mods)
    }

    /// The units in the world shader now.
    pub fn world_files(&self) -> Vec<ModFile> {
        let session = self.session();
        ModFile::ALL
            .into_iter()
            .filter(|file| file.in_world() && session.mods.uses(*file))
            .collect()
    }

    /// The hooks of the world shader without the units `dropped`, with the plant's line where
    /// it plants.
    pub fn world_hooks_without(&self, dropped: &[ModFile]) -> WorldHooks {
        let session = self.session();
        let mut mods = None::<CoreMods>;
        for file in dropped {
            mods = Some(mods.as_ref().unwrap_or(&session.mods).without(*file));
        }
        session.world_hooks_in(mods.as_ref().unwrap_or(&session.mods))
    }

    /// The plant keeps a failed build of `home`: the null test of the fallback.
    pub fn plant_keeps(&self, home: &str) -> bool {
        let session = self.session();
        session.plant.as_ref().is_some_and(|plant| {
            plant.end == PlantEnd::Keep && ModFile::ALL.into_iter().any(|file| plant.plants(file, home))
        })
    }

    /// Writes the marker of the world shader for the units `files` (none: no marker). The plant's
    /// `:exit` for one of them in the world shader ends the helper here, after the marker.
    pub fn mark_world(&self, files: &[ModFile]) {
        let session = self.session();
        let marked: Vec<(ModFile, String)> = files
            .iter()
            .filter_map(|file| Some((*file, session.mods.texts.hook_text(*file)?.to_string())))
            .collect();
        if marked.is_empty() {
            session.markers.remove(WORLD_HOME);
            return;
        }
        session.markers.write(WORLD_HOME, &marked);
        let exits = session
            .plant
            .as_ref()
            .is_some_and(|plant| plant.end == PlantEnd::Exit && files.iter().any(|file| plant.plants(*file, WORLD_HOME)));
        if exits {
            println!("core mods: MGE_XE_CORE_MOD_FAULT ends the helper in the build of the {WORLD_HOME}");
            std::process::exit(3);
        }
    }

    /// The world shader holds without the units `dropped`: they are dropped for the homes not
    /// yet built, and the marker of the world goes.
    pub fn world_held(&self, dropped: &[ModFile], error: &str) {
        let mut session = self.session();
        for file in dropped {
            let line = format!(
                "core mod: {}: the world shader failed with it on the device ({error}); the built-in hooks are used",
                file.label()
            );
            session.drop_file(*file, line);
        }
        // The world has the text of this drop already.
        session.world_changed = false;
        session.markers.remove(WORLD_HOME);
    }

    /// A home dropped a unit of the world shader since the last call.
    pub fn take_world_changed(&self) -> bool {
        std::mem::take(&mut self.session().world_changed)
    }

    /// Makes the first GPU objects of `home` with the hooks of `file` (`make` takes the hook
    /// text), under the error scopes of the device and with the build marker around them. On a
    /// fault the file is dropped from the session and `make` runs again with the built-in hooks.
    /// A home whose hooks are the built-ins builds with no marker and no scope.
    pub fn first_build<T>(
        &self,
        device: &wgpu::Device,
        home: &'static str,
        file: ModFile,
        mut make: impl FnMut(&str) -> T,
    ) -> T {
        let (hooks, end) = {
            let session = self.session();
            if !session.mods.uses(file) {
                let hooks = session.mods.hooks(file);
                drop(session);
                return make(&hooks);
            }
            let text = session.mods.texts.hook_text(file).unwrap_or_default().to_string();
            session.markers.write(home, &[(file, text)]);
            let end = session
                .plant
                .as_ref()
                .filter(|plant| plant.plants(file, home))
                .map(|plant| plant.end);
            (session.hooks_for(file, home), end)
        };
        if end == Some(PlantEnd::Exit) {
            println!("core mods: MGE_XE_CORE_MOD_FAULT ends the helper in the build of the {home}");
            std::process::exit(3);
        }
        let (made, error) = nirender_wgpu::gpu::guarded(device, || make(&hooks));
        let Some(error) = error else {
            self.session().markers.remove(home);
            return made;
        };
        if end == Some(PlantEnd::Keep) {
            println!(
                "core mods: the {home} failed with {} ({error}); MGE_XE_CORE_MOD_FAULT keeps it",
                file.label()
            );
            self.session().markers.remove(home);
            return made;
        }
        let built_in = {
            let mut session = self.session();
            let line = format!(
                "core mod: {}: the {home} failed with it on the device ({error}); the built-in hooks are used",
                file.label()
            );
            session.drop_file(file, line);
            session.mods.hooks(file)
        };
        let made = make(&built_in);
        self.session().markers.remove(home);
        made
    }
}

/// The core mods are off (`MGE_XE_CORE_MODS=0`): no file is read, with or without
/// `MGE_XE_CORE_MODS_DIR`.
fn off() -> bool {
    nirender_wgpu::switches::on("MGE_XE_CORE_MODS") == Some(false)
}

/// The folder of `MGE_XE_CORE_MODS_DIR` (W528): the core mods are read only from it, never from
/// `Data Files`. The log names it, or says that it is not a folder, and then no core mod is read.
/// The build markers stay in their own folder ([`BuildMarkers::apart_from`]).
pub(crate) fn folder_of_switch() -> Option<PathBuf> {
    let folder = nirender_wgpu::switches::path("MGE_XE_CORE_MODS_DIR")?;
    if !off() {
        if folder.is_dir() {
            println!("core mods: only the folder {} (MGE_XE_CORE_MODS_DIR)", folder.display());
        } else {
            println!(
                "core mods: MGE_XE_CORE_MODS_DIR={} is not a folder: no core mod is read",
                folder.display()
            );
        }
    }
    Some(folder)
}

/// The home names in a marker, in the log and in `MGE_XE_CORE_MOD_FAULT`.
pub(crate) const WORLD_HOME: &str = "world shader";
pub(crate) const GRASS_HOME: &str = "grass";
pub(crate) const WATER_PLANE_HOME: &str = "water plane";
pub(crate) const WATER_VOLUME_HOME: &str = "water volume";

/// The shadow files that pass.
struct ShadowMod {
    hooks: String,
    values: ShadowValues,
    defined: Vec<&'static str>,
    values_line: Option<String>,
}

/// Checks the shadow files and joins them; the error names the file that failed.
fn shadow_mod(
    shadow_file: Option<&str>,
    data_file: Option<&str>,
    homes: &[Home],
) -> Result<ShadowMod, (&'static str, String)> {
    let mut values = ShadowValues::default();
    let mut values_line = None;
    if let Some(text) = data_file {
        let fail = |problem: String| ("Shadow Data", problem);
        let header = header(text, &["values"]).map_err(fail)?;
        if let Some((_, line)) = header.iter().find(|(key, _)| *key == "values") {
            values = ShadowValues::parse(line).map_err(fail)?;
            values_line = Some(line.clone());
        }
        rules(text, &[], "", &[]).map_err(fail)?;
    }
    let Some(text) = shadow_file else {
        return Ok(ShadowMod {
            hooks: hook_text(&SHADOW_HOOKS, &[], ""),
            values,
            defined: Vec::new(),
            values_line,
        });
    };
    let (hooks, defined) = hook_file(text, &SHADOW_HOOKS, SHADOW_LIBRARY, &SHADOW_LIBRARY_NAMES, homes)
        .map_err(|problem| ("Shadow", problem))?;
    Ok(ShadowMod {
        hooks,
        values,
        defined,
        values_line,
    })
}

/// Checks a file of hooks with its header, its rules and each of its homes. Gives the hook text
/// for the homes (the file's hooks and the built-ins of the others), and the hooks it defines.
fn hook_file(
    text: &str,
    hooks: &'static [Hook],
    library: &str,
    library_names: &[&str],
    homes: &[Home],
) -> Result<(String, Vec<&'static str>), String> {
    header(text, &[])?;
    let defined = rules(text, hooks, library, library_names)?;
    let joined = hook_text(hooks, &defined, text);
    for home in homes {
        (home.check)(&joined).map_err(|problem| format!("the {} fails with it: {problem}", home.name))?;
    }
    Ok((joined, defined))
}

/// The `//! key: value` lines at the start of a file, with blank lines between them: the first
/// is the interface, the others are keys of `allowed`. A `//!` line after the first line of
/// code is an error, so that no value is lost without a word.
fn header(text: &str, allowed: &[&str]) -> Result<Vec<(String, String)>, String> {
    let mut rest = text
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .filter(|line| !line.is_empty())
        .peekable();
    let mut header_lines = Vec::new();
    while let Some(line) = rest.next_if(|line| line.starts_with("//!")) {
        header_lines.push(&line[3..]);
    }
    if let Some(late) = rest.find(|line| line.starts_with("//!")) {
        return Err(format!("the line `{late}` comes after the code: the `//!` lines come first"));
    }
    let mut lines = header_lines.into_iter();
    let interface = lines
        .next()
        .and_then(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim() == "interface")
        .map(|(_, value)| value.trim().to_string());
    match interface {
        None => return Err(format!("no line `//! interface: {INTERFACE}` at its start")),
        Some(value) if value != INTERFACE => return Err(format!("the interface `{value}` is not {INTERFACE}")),
        Some(_) => {}
    }
    let mut keys = Vec::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("the line `//!{line}` has no `key: value`"));
        };
        let key = key.trim();
        if !allowed.contains(&key) {
            return Err(format!("the key `{key}` is not known in this file"));
        }
        keys.push((key.to_string(), value.trim().to_string()));
    }
    Ok(keys)
}

/// The rule of a file, checked with naga: only the hooks of `hooks` with their signatures, the
/// names of `library` (functions and structs) used, and names of its own that start with
/// `mod_`. Gives the hooks that the file defines.
fn rules(text: &str, hooks: &'static [Hook], library: &str, library_names: &[&str]) -> Result<Vec<&'static str>, String> {
    let defined: Vec<&'static str> = hooks
        .iter()
        .filter(|hook| defines(text, hook.name))
        .map(|hook| hook.name)
        .collect();
    // The file comes first, so that the lines of an error are the lines of the file. The hooks
    // that it lacks are declarations: a file may call them, and the built-ins may use what the
    // renderer has outside the interface.
    let lacking: String = hooks
        .iter()
        .filter(|hook| !defined.contains(&hook.name))
        .map(declaration)
        .collect();
    let source = format!("{text}\n{library}\n{lacking}");
    let module = naga::front::wgsl::parse_str(&source).map_err(|error| error.emit_to_string(&source))?;
    if let Some(entry) = module.entry_points.first() {
        return Err(format!("the entry point `{}`: a core mod has none", entry.name));
    }
    if let Some((_, variable)) = module.global_variables.iter().next() {
        return Err(format!(
            "the variable `{}` outside a function: a core mod has none (no binding)",
            variable.name.as_deref().unwrap_or("?")
        ));
    }
    if let Some((_, value)) = module.overrides.iter().next() {
        return Err(format!(
            "the override `{}`: a core mod has none",
            value.name.as_deref().unwrap_or("?")
        ));
    }
    let own = |name: &str| name.starts_with("mod_");
    for (_, function) in module.functions.iter() {
        let name = function.name.as_deref().unwrap_or("");
        if let Some(hook) = hooks.iter().find(|hook| hook.name == name) {
            let arguments: Vec<String> = function
                .arguments
                .iter()
                .map(|argument| type_name(&module, argument.ty))
                .collect();
            let result = function
                .result
                .as_ref()
                .map_or("nothing".to_string(), |result| type_name(&module, result.ty));
            if arguments != hook.arguments || result != hook.result {
                return Err(format!(
                    "the hook `{name}` takes ({}) and gives {result}; interface 1 has ({}) -> {}",
                    arguments.join(", "),
                    hook.arguments.join(", "),
                    hook.result
                ));
            }
        } else if !own(name) && !library_names.contains(&name) {
            return Err(format!(
                "the function `{name}`: a function of a core mod is a hook of its file, or its name starts with `mod_`"
            ));
        }
    }
    for (_, kind) in module.types.iter() {
        // naga names the result structs of some built-in functions `__...`.
        if let (naga::TypeInner::Struct { .. }, Some(name)) = (&kind.inner, kind.name.as_deref())
            && !own(name)
            && !name.starts_with("__")
            && !library_names.contains(&name)
        {
            return Err(format!(
                "the struct `{name}`: the name of a struct of a core mod starts with `mod_`"
            ));
        }
    }
    if let Some(name) = module
        .constants
        .iter()
        .filter_map(|(_, constant)| constant.name.as_deref())
        .find(|name| !own(name))
    {
        return Err(format!(
            "the constant `{name}`: the name of a constant of a core mod starts with `mod_`"
        ));
    }
    Ok(defined)
}

/// A hook as a function with its signature and a zero result, for the check of a file that
/// lacks it.
fn declaration(hook: &Hook) -> String {
    let arguments: Vec<String> = hook
        .arguments
        .iter()
        .enumerate()
        .map(|(index, kind)| format!("a{index}: {kind}"))
        .collect();
    format!(
        "fn {}({}) -> {} {{ return {}(); }}\n",
        hook.name,
        arguments.join(", "),
        hook.result,
        hook.result
    )
}

/// True when `text` declares the function `name` (`fn name`), outside a line comment.
fn defines(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        let words: Vec<&str> = code
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|word| !word.is_empty())
            .collect();
        words.windows(2).any(|pair| pair[0] == "fn" && pair[1] == name)
    })
}

/// The built-ins of the hooks that `defined` lacks, then `text`.
fn hook_text(hooks: &[Hook], defined: &[&str], text: &str) -> String {
    let mut joined: String = hooks
        .iter()
        .filter(|hook| !defined.contains(&hook.name))
        .map(|hook| hook.built_in)
        .collect();
    joined.push_str(text);
    joined
}

/// The hooks of the colour of the plane: the debug view `water` shows the terms of their
/// built-ins.
const WATER_COLOUR_HOOKS: [&str; 2] = ["water_color", "underwater_color"];

/// The water hooks of a mod's file for the debug view `water`: the built-in colour hooks in
/// place of the file's, and the file's other hooks (its `water_normal`). The file's colour
/// functions stay under other names, so that its other functions still find what they call.
fn water_debug_hooks(text: &str, defined: &[&'static str]) -> String {
    let kept: Vec<&str> = defined
        .iter()
        .copied()
        .filter(|hook| !WATER_COLOUR_HOOKS.contains(hook))
        .collect();
    let renamed = WATER_COLOUR_HOOKS
        .iter()
        .filter(|hook| defined.contains(hook))
        .fold(text.to_string(), |text, hook| rename_definition(&text, hook));
    hook_text(&WATER_HOOKS, &kept, &renamed)
}

/// The text with the definition of the function `name` named `mod_replaced_<name>`: a name of
/// the mod's own (the rule of a file), so a built-in of `name` can join the text. A call of
/// `name` then reaches the built-in.
fn rename_definition(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(at) = rest.find("fn") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let name_start = after.trim_start();
        let word_before = matches!(out.chars().last(), Some(c) if c.is_ascii_alphanumeric() || c == '_');
        let spaced = name_start.len() < after.len();
        let definition = !word_before
            && spaced
            && name_start
                .strip_prefix(name)
                .is_some_and(|tail| tail.trim_start().starts_with('('));
        if definition {
            out.push_str("fn mod_replaced_");
            out.push_str(name);
            rest = &name_start[name.len()..];
        } else {
            out.push_str("fn");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A type as the hooks' signatures write it: `f32`, `vec3<f32>`, or the name of a struct.
fn type_name(module: &naga::Module, handle: naga::Handle<naga::Type>) -> String {
    let scalar = |scalar: naga::Scalar| match (scalar.kind, scalar.width) {
        (naga::ScalarKind::Float, 4) => "f32".to_string(),
        (naga::ScalarKind::Sint, 4) => "i32".to_string(),
        (naga::ScalarKind::Uint, 4) => "u32".to_string(),
        (naga::ScalarKind::Bool, _) => "bool".to_string(),
        (kind, width) => format!("{kind:?}{}", width * 8),
    };
    let kind = &module.types[handle];
    match &kind.inner {
        naga::TypeInner::Scalar(s) => scalar(*s),
        naga::TypeInner::Vector { size, scalar: s } => format!("vec{}<{}>", *size as u8, scalar(*s)),
        naga::TypeInner::Struct { .. } => kind.name.clone().unwrap_or_else(|| "a struct".to_string()),
        other => format!("{other:?}"),
    }
}

/// The functions of a library whose signature differs in the source of a home, or that the
/// home lacks: an empty list when the home gives the library of the interface. For a test of
/// each home (`library` is [`SHADOW_LIBRARY`] or [`WATER_LIBRARY`]).
#[cfg(test)]
pub(crate) fn library_differences(library: &str, home_source: &str) -> Vec<String> {
    let declared = naga::front::wgsl::parse_str(library).unwrap();
    let home =
        naga::front::wgsl::parse_str(home_source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(home_source)));
    let signature = |module: &naga::Module, function: &naga::Function| {
        let arguments: Vec<String> = function
            .arguments
            .iter()
            .map(|argument| type_name(module, argument.ty))
            .collect();
        format!(
            "({}) -> {}",
            arguments.join(", "),
            function
                .result
                .as_ref()
                .map_or("nothing".to_string(), |result| type_name(module, result.ty))
        )
    };
    let mut differences = Vec::new();
    for (_, function) in declared.functions.iter() {
        let name = function.name.as_deref().unwrap_or("");
        let wanted = signature(&declared, function);
        match home.functions.iter().find(|(_, other)| other.name.as_deref() == Some(name)) {
            Some((_, other)) if signature(&home, other) == wanted => {}
            Some((_, other)) => differences.push(format!(
                "{name}: the home has {}, the interface {wanted}",
                signature(&home, other)
            )),
            None => differences.push(format!("{name}: not in the home")),
        }
    }
    differences
}

/// The name of every hook that a core mod may replace: the shadow's, the water's and the sky's.
/// For the test that a shader without hooks names none of them.
#[cfg(test)]
pub(crate) fn hook_names() -> Vec<&'static str> {
    SHADOW_HOOKS
        .iter()
        .chain(WATER_HOOKS.iter())
        .chain(SKY_HOOKS.iter())
        .map(|hook| hook.name)
        .collect()
}

#[cfg(test)]
pub(crate) use tests::marker_water_file;

#[cfg(test)]
mod tests {
    use super::*;
    use nirender_wgpu::objects::Create;
    use std::collections::HashMap;

    /// A home with a library that reads a texture, and an entry point that calls both hooks.
    const TEST_HOME: &str = "
@group(0) @binding(0) var map: texture_2d<f32>;
@group(0) @binding(1) var map_sampler: sampler;
@group(0) @binding(2) var<uniform> esm: vec4f;
fn shadow_map_depth(layer: u32, uv: vec2f) -> f32 {
    return textureSampleLevel(map, map_sampler, vec2f((f32(layer) + uv.x) * 0.5, uv.y), 0.0).r;
}
fn shadow_texel() -> f32 { return esm.w; }
fn shadow_esm_scale() -> f32 { return esm.z; }
fn shadow_esm_values() -> vec2f { return esm.xy; }
@fragment fn fs(@location(0) s0: vec3f, @location(1) s1: vec3f) -> @location(0) vec4f {
    return vec4f(shadow_esm(shadow_delta(s0, s1)));
}
";

    fn test_home(hooks: &str) -> Result<(), String> {
        crate::renderer::shader_check::check("test home", &format!("{TEST_HOME}{hooks}")).map(|_| ())
    }

    fn failing_home(_hooks: &str) -> Result<(), String> {
        Err("a planted failure".to_string())
    }

    const HOMES: [Home; 1] = [Home {
        name: "test home",
        check: test_home,
    }];

    fn mods_of(files: &[(&str, &str)], homes: &[Home]) -> CoreMods {
        let files: HashMap<String, String> = files
            .iter()
            .map(|(name, text)| (format!("{FOLDER}/XE Mod {name}"), text.to_string()))
            .collect();
        mods_with_markers(files, homes, &BuildMarkers::default())
    }

    fn mods_with_markers(files: HashMap<String, String>, homes: &[Home], markers: &BuildMarkers) -> CoreMods {
        CoreMods::read(
            &|path| files.get(path).map(|text| text.as_bytes().to_vec()),
            homes,
            &[],
            &[],
            &[],
            markers,
        )
    }

    fn header_line() -> String {
        format!("//! interface: {INTERFACE}\n")
    }

    #[test]
    fn the_built_in_hooks_are_valid_in_a_home() {
        test_home(CoreMods::built_in().shadow_hooks()).unwrap();
        let mods = mods_of(&[], &HOMES);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert_eq!(
            mods.report(),
            [format!("core mods: none used (Data Files/{FOLDER}, interface {INTERFACE})")]
        );
    }

    #[test]
    fn a_copy_of_the_built_ins_is_used_whole() {
        let file = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
        // Both hooks come from the file: no built-in is joined.
        assert_eq!(mods.shadow_hooks(), file);
        assert_eq!(
            mods.report(),
            [format!(
                "core mods: XE Mod Shadow.wgsl (shadow_delta, shadow_esm) (interface {INTERFACE})"
            )]
        );
    }

    #[test]
    fn a_file_with_one_hook_keeps_the_built_in_of_the_other() {
        let file = format!(
            "{}fn mod_half(x: f32) -> f32 {{ return 0.5 * x; }}\nfn shadow_esm(dz: f32) -> f32 {{ return mod_half(1.0); }}\n",
            header_line()
        );
        let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
        assert_eq!(mods.shadow_hooks(), format!("{SHADOW_DELTA}{file}"));
        assert!(
            mods.report()[0].contains("XE Mod Shadow.wgsl (shadow_esm)"),
            "{:?}",
            mods.report()
        );
    }

    /// Each rule drops the file with its reason, and the built-ins stay.
    #[test]
    fn a_file_that_breaks_a_rule_is_dropped_with_its_reason() {
        let cases = [
            (
                "fn shadow_esm(dz: f32) -> f32 { return 0.0; }\n".to_string(),
                "no line `//! interface: mge-xe-core 1` at its start",
            ),
            (
                "//! interface: mge-xe-core 2\n".to_string(),
                "the interface `mge-xe-core 2` is not mge-xe-core 1",
            ),
            (
                format!("{}//! values: shade = 1\n", header_line()),
                "the key `values` is not known in this file",
            ),
            (
                format!("{}@group(3) @binding(0) var<uniform> mod_u: vec4f;\n", header_line()),
                "the variable `mod_u` outside a function",
            ),
            (
                format!("{}var<private> mod_p: f32;\n", header_line()),
                "the variable `mod_p` outside a function",
            ),
            (
                format!(
                    "{}@fragment fn mod_fs() -> @location(0) vec4f {{ return vec4f(1.0); }}\n",
                    header_line()
                ),
                "the entry point `mod_fs`",
            ),
            (
                format!("{}fn helper() -> f32 {{ return 1.0; }}\n", header_line()),
                "the function `helper`",
            ),
            (format!("{}struct Light {{ x: f32 }}\n", header_line()), "the struct `Light`"),
            (format!("{}const SCALE: f32 = 2.0;\n", header_line()), "the constant `SCALE`"),
            (
                format!("{}fn shadow_esm(dz: f32, x: f32) -> f32 {{ return 0.0; }}\n", header_line()),
                "the hook `shadow_esm` takes (f32, f32) and gives f32; interface 1 has (f32) -> f32",
            ),
            (
                format!(
                    "{}fn shadow_delta(s0: vec3f, s1: vec3f) -> vec2f {{ return vec2f(0.0); }}\n",
                    header_line()
                ),
                "the hook `shadow_delta` takes (vec3<f32>, vec3<f32>) and gives vec2<f32>",
            ),
            (
                format!("{}fn shadow_esm(dz: f32) -> f32 {{ return no_such(dz); }}\n", header_line()),
                "no_such",
            ),
        ];
        for (file, reason) in cases {
            let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
            assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks(), "{file}");
            assert!(
                mods.report()[0].starts_with("core mod: XE Mod Shadow.wgsl: ") && mods.report()[0].contains(reason),
                "{file}: {:?}",
                mods.report()
            );
            assert!(
                mods.report()[0].ends_with("; the built-in shadow is used"),
                "{:?}",
                mods.report()
            );
            assert!(mods.report()[1].starts_with("core mods: none used"), "{:?}", mods.report());
        }
    }

    #[test]
    fn a_file_that_a_home_refuses_is_dropped() {
        let file = format!("{}{SHADOW_ESM}", header_line());
        let homes = [
            Home {
                name: "test home",
                check: test_home,
            },
            Home {
                name: "grass",
                check: failing_home,
            },
        ];
        let mods = mods_of(&[("Shadow.wgsl", &file)], &homes);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(
            mods.report()[0].contains("the grass fails with it: a planted failure"),
            "{:?}",
            mods.report()
        );
    }

    #[test]
    fn the_values_of_shadow_data_replace_the_built_in_ones() {
        let data = format!("{}//! values: shade = 0.8, shade_color = 1 0.5 0.25\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &data)], &HOMES);
        let values = mods.shadow_values;
        assert_eq!((values.shade, values.shade_color), (0.8, [1.0, 0.5, 0.25]));
        assert_eq!((values.esm_c, values.esm_bias), (shadow::ESM_C, shadow::ESM_BIAS));
        assert_eq!(values.shade(), [1.0, 0.5, 0.25, 0.8]);
        assert_eq!(values.esm(0.5), [shadow::ESM_C, shadow::ESM_BIAS, shadow::ESM_SCALE, 0.5]);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(
            mods.report()[0].contains("XE Mod Shadow Data.wgsl (shade = 0.8, shade_color = 1 0.5 0.25)"),
            "{:?}",
            mods.report()
        );
    }

    /// A bad value drops Shadow Data and the Shadow file with it; a bad Shadow file drops the
    /// values of Shadow Data too.
    #[test]
    fn shadow_and_shadow_data_fall_back_together() {
        let good_data = format!("{}//! values: shade = 0.8\n", header_line());
        let good_shadow = format!("{}{SHADOW_ESM}", header_line());
        for (shadow_file, data, reason) in [
            (
                good_shadow.clone(),
                format!("{}//! values: shade = dark\n", header_line()),
                "XE Mod Shadow Data.wgsl: `dark` of the value shade is not a number",
            ),
            (
                good_shadow.clone(),
                format!("{}//! values: depth = 1\n", header_line()),
                "the value `depth` is not known",
            ),
            (
                good_shadow.clone(),
                format!("{}//! values: shade_color = 1 1\n", header_line()),
                "the value shade_color has 2 numbers, not 3",
            ),
            (
                good_shadow.clone(),
                format!("{}//! values: shade = 0\n", header_line()),
                "the value shade = 0 is out of its range: 0.01 or more",
            ),
            (
                good_shadow.clone(),
                format!("{}//! values: shade_color = 1 2 0\n", header_line()),
                "the value shade_color = 1 2 0 is out of its range: 0 to 1 in each number",
            ),
            (
                good_shadow.clone(),
                format!("{}//! values: esm_c = 88.5\n", header_line()),
                "the value esm_c = 88.5 is out of its range: above 0, at most 88",
            ),
            (
                good_shadow.clone(),
                format!(
                    "{}//! values: shade = 0.8\nfn mod_x() {{}}\nfn shadow_esm(dz: f32) -> f32 {{ return dz; }}\n",
                    header_line()
                ),
                "XE Mod Shadow Data.wgsl: the function `shadow_esm`",
            ),
            (
                format!("{}fn helper() {{}}\n", header_line()),
                good_data.clone(),
                "XE Mod Shadow.wgsl: the function `helper`",
            ),
        ] {
            let mods = mods_of(&[("Shadow.wgsl", &shadow_file), ("Shadow Data.wgsl", &data)], &HOMES);
            assert_eq!(mods.shadow_values, ShadowValues::default(), "{reason}");
            assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks(), "{reason}");
            assert!(
                mods.report()[0].contains(reason) && mods.report()[0].ends_with("is dropped with it"),
                "{reason}: {:?}",
                mods.report()
            );
        }
    }

    #[test]
    fn an_fx_file_without_its_wgsl_is_named() {
        let sky = header_line();
        let mods = mods_of(
            &[
                ("Water.fx", "technique T {}"),
                ("Sky.fx", "technique T {}"),
                ("Sky.wgsl", &sky),
            ],
            &HOMES,
        );
        assert_eq!(
            mods.report()[0],
            "core mod: XE Mod Water.fx is for Direct3D 9; the wgpu renderer reads XE Mod Water.wgsl, and there is none"
        );
        assert_eq!(mods.report().len(), 2, "{:?}", mods.report());
        assert!(mods.report()[1].contains("XE Mod Sky.wgsl (no hook)"), "{:?}", mods.report());
        assert!(mods.sky_hooks().is_none());
    }

    /// Blank lines between the `//!` lines keep the values; a `//!` line after the code drops
    /// the file with a word; a byte order mark before the first line is not read.
    #[test]
    fn the_header_takes_blank_lines_and_a_byte_order_mark() {
        let data = format!("\n{}\n//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &data)], &HOMES);
        assert_eq!(mods.shadow_values.shade, 0.8, "{:?}", mods.report());
        let late = format!("{}fn mod_x() {{}}\n//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &late)], &HOMES);
        assert_eq!(mods.shadow_values, ShadowValues::default());
        assert!(
            mods.report()[0].contains("the line `//! values: shade = 0.8` comes after the code"),
            "{:?}",
            mods.report()
        );
        let marked = format!("\u{feff}{}//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &marked)], &HOMES);
        assert_eq!(mods.shadow_values.shade, 0.8, "{:?}", mods.report());
        let marked = format!("\u{feff}{}{SHADOW_ESM}", header_line());
        let mods = mods_of(&[("Shadow.wgsl", &marked)], &HOMES);
        assert!(
            !mods.shadow_hooks().contains('\u{feff}') && mods.report()[0].contains("XE Mod Shadow.wgsl (shadow_esm)"),
            "{:?}",
            mods.report()
        );
    }

    #[test]
    fn a_declaration_is_found_outside_comments_and_by_its_whole_name() {
        assert!(defines("fn shadow_esm(dz: f32) -> f32", "shadow_esm"));
        assert!(defines("  fn   shadow_esm (dz: f32)", "shadow_esm"));
        assert!(!defines("// fn shadow_esm(dz: f32)", "shadow_esm"));
        assert!(!defines("fn shadow_esm_values() -> vec2f", "shadow_esm"));
        assert!(!defines("fn mod_shadow_esm(dz: f32)", "shadow_esm"));
        assert!(!defines("let x = shadow_esm(dz);", "shadow_esm"));
    }

    /// The names of each library are those that its declarations declare.
    #[test]
    fn the_library_names_match_its_declarations() {
        let module = naga::front::wgsl::parse_str(SHADOW_LIBRARY).unwrap();
        let names: Vec<&str> = module
            .functions
            .iter()
            .filter_map(|(_, function)| function.name.as_deref())
            .collect();
        assert_eq!(names, SHADOW_LIBRARY_NAMES);
        let module = naga::front::wgsl::parse_str(&format!("{WATER_INTERFACE}{WATER_LIBRARY}")).unwrap();
        let mut names: Vec<&str> = module
            .functions
            .iter()
            .filter_map(|(_, function)| function.name.as_deref())
            .collect();
        names.push("WaterPixel");
        assert_eq!(names, WATER_LIBRARY_NAMES);
    }

    /// A water file that colours the plane one colour and calls the built-in normal, which it
    /// lacks: the marker of the tests of the plane and of the other water modules.
    pub(crate) fn marker_water_file() -> String {
        format!(
            "{}fn water_color(p: WaterPixel) -> vec4f {{\n    return vec4f(mod_marker(water_normal(p.texcoords, p.distance, p.world.xy)), 1.0);\n}}\nfn mod_marker(normal: vec3f) -> vec3f {{\n    return vec3f(0.123, 0.0, 0.0) + 0.0 * normal;\n}}\n",
            header_line()
        )
    }

    #[test]
    fn a_water_file_joins_the_plane_with_the_built_ins_it_lacks() {
        let file = marker_water_file();
        let mods = CoreMods::of_files(&[("Water.wgsl", &file)], &HOMES, &[], &[]);
        assert_eq!(
            mods.water_hooks(),
            format!(
                "{}{}{file}",
                super::super::water::WATER_NORMAL,
                super::super::water::UNDERWATER_COLOR
            )
        );
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(
            mods.report()
                .iter()
                .any(|line| line.contains("XE Mod Water.wgsl (water_color)")),
            "{:?}",
            mods.report()
        );
        assert!(
            mods.report()
                .iter()
                .any(|line| line.contains("the water volumes and the dry spaces keep the built-in water")),
            "{:?}",
            mods.report()
        );
        assert!(
            mods.report()
                .iter()
                .any(|line| line.contains("the debug view water shows the terms of the built-in colour")),
            "{:?}",
            mods.report()
        );
        // The debug view: the built-in colour hooks, and the file's colour under another name.
        let debug = mods.water_debug_hooks();
        assert_eq!(debug.matches("fn water_color(").count(), 1);
        assert!(debug.contains(super::super::water::WATER_COLOR) && debug.contains("fn mod_replaced_water_color("));
        assert!(debug.contains(super::super::water::UNDERWATER_COLOR) && debug.contains("fn mod_marker("));
        assert_eq!(CoreMods::built_in().water_debug_hooks(), CoreMods::built_in().water_hooks());
    }

    /// A file's colour hooks give way to the built-ins in the debug text; its normal stays.
    #[test]
    fn the_debug_text_of_a_water_file_keeps_its_normal() {
        let text = "fn water_normal(a: vec4f, b: f32, c: vec2f) -> vec3f { return vec3f(0.0, 0.0, 1.0); }\nfn  underwater_color (p: WaterPixel) -> vec4f { return water_color(p); }\n";
        let debug = water_debug_hooks(text, &["water_normal", "underwater_color"]);
        assert!(!debug.contains(super::super::water::WATER_NORMAL));
        assert!(debug.contains("fn water_normal(a: vec4f"));
        assert!(debug.contains("fn mod_replaced_underwater_color (p: WaterPixel)"));
        assert!(debug.contains(super::super::water::WATER_COLOR) && debug.contains(super::super::water::UNDERWATER_COLOR));
        // Only a definition is renamed: a call, a longer name and a comment word stay.
        assert_eq!(
            rename_definition(
                "fn water_color2() {} let x = water_color(p); // fn water_color(",
                "water_color"
            ),
            "fn water_color2() {} let x = water_color(p); // fn mod_replaced_water_color("
        );
        assert_eq!(rename_definition("fnwater_color(", "water_color"), "fnwater_color(");
    }

    #[test]
    fn a_water_file_that_breaks_a_rule_is_dropped() {
        for (file, reason) in [
            (
                format!(
                    "{}fn water_color(p: WaterPixel) -> vec3f {{ return vec3f(0.0); }}\n",
                    header_line()
                ),
                "the hook `water_color` takes (WaterPixel) and gives vec3<f32>",
            ),
            (format!("{}struct WaterPixel {{ x: f32 }}\n", header_line()), "WaterPixel"),
            (
                format!(
                    "{}fn water_color(p: WaterPixel) -> vec4f {{ return vec4f(water.sky_col.rgb, 1.0); }}\n",
                    header_line()
                ),
                "water",
            ),
        ] {
            let mods = CoreMods::of_files(&[("Water.wgsl", &file)], &HOMES, &[], &[]);
            assert_eq!(mods.water_hooks(), CoreMods::built_in().water_hooks(), "{file}");
            assert!(
                mods.report()[0].starts_with("core mod: XE Mod Water.wgsl: ")
                    && mods.report()[0].contains(reason)
                    && mods.report()[0].ends_with("; the built-in water is used"),
                "{file}: {:?}",
                mods.report()
            );
        }
    }

    #[test]
    fn a_hook_is_declared_by_its_signature() {
        assert_eq!(
            declaration(&WATER_HOOKS[1]),
            "fn water_color(a0: WaterPixel) -> vec4<f32> { return vec4<f32>(); }\n"
        );
        assert_eq!(
            declaration(&SHADOW_HOOKS[0]),
            "fn shadow_delta(a0: vec3<f32>, a1: vec3<f32>) -> f32 { return f32(); }\n"
        );
    }

    #[test]
    fn a_plant_names_its_file_its_home_and_its_end() {
        let plant = |file, home: Option<&str>, end| Plant {
            file,
            home: home.map(str::to_string),
            end,
        };
        assert_eq!(Plant::parse("Sky"), Some(plant(ModFile::Sky, None, PlantEnd::Fallback)));
        assert_eq!(
            Plant::parse(" water@water plane:exit "),
            Some(plant(ModFile::Water, Some(WATER_PLANE_HOME), PlantEnd::Exit))
        );
        assert_eq!(
            Plant::parse("Shadow@grass:keep"),
            Some(plant(ModFile::Shadow, Some(GRASS_HOME), PlantEnd::Keep))
        );
        for wrong in ["", "Fog", "Sky:stop", "@grass"] {
            assert_eq!(Plant::parse(wrong), None, "{wrong}");
        }
        let grass = Plant::parse("Shadow@grass").unwrap();
        assert!(grass.plants(ModFile::Shadow, GRASS_HOME));
        assert!(!grass.plants(ModFile::Shadow, WORLD_HOME) && !grass.plants(ModFile::Sky, GRASS_HOME));
        assert!(Plant::parse("Shadow").unwrap().plants(ModFile::Shadow, WORLD_HOME));
    }

    /// No build marker goes into the folder of `MGE_XE_CORE_MODS_DIR`. The null: a marker folder
    /// that is another folder stays.
    #[test]
    fn the_markers_stay_out_of_the_folder_of_the_core_mods() {
        let folder = std::env::temp_dir().join(format!("core-mods-apart-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let other = folder.join("markers");

        assert_eq!(BuildMarkers::in_folder(&folder).apart_from(Some(&folder)).folder, None);
        assert_eq!(
            BuildMarkers::in_folder(&other).apart_from(Some(&folder)).folder,
            Some(other.clone())
        );
        assert_eq!(BuildMarkers::in_folder(&other).apart_from(None).folder, Some(other));
        std::fs::remove_dir(&folder).unwrap();
    }

    /// The plant marks the hooks of the world only while the file is in use: the text without the
    /// planted file, and the world after the file is dropped, have the built-in hooks with no plant
    /// (live run R2 of 2026-11-16 failed again on the planted built-in hooks). The null: the text
    /// with the file has the plant.
    #[test]
    fn the_world_without_the_planted_file_has_no_plant() {
        let shadow = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let files: HashMap<String, String> = [(format!("{FOLDER}/XE Mod Shadow.wgsl"), shadow)].into();
        let shared = Shared::new(
            mods_with_markers(files, &HOMES, &BuildMarkers::default()),
            BuildMarkers::default(),
            Plant::parse("Shadow@world shader"),
        );
        let built_in = CoreMods::built_in().shadow_hooks().to_string();

        assert!(shared.world_hooks().shadow.starts_with(nirender_wgpu::CORE_MOD_FAULT_PLANT));
        assert_eq!(shared.world_hooks_without(&[ModFile::Shadow]).shadow, built_in);

        shared.world_held(&[ModFile::Shadow], "planted");
        assert_eq!(shared.world_hooks().shadow, built_in);
    }

    /// Without `MGE_XE_CORE_MOD_FAULT` there is no plant, and no text of the world has the plant's
    /// line, with the file in use or without it. The null is
    /// [`the_world_without_the_planted_file_has_no_plant`]: the same file with the plant has it.
    #[test]
    fn without_the_switch_no_text_has_the_plant() {
        assert!(
            std::env::var_os("MGE_XE_CORE_MOD_FAULT").is_none(),
            "the test runs without the switch"
        );
        assert_eq!(Plant::of_env(), None);

        let shadow = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let files: HashMap<String, String> = [(format!("{FOLDER}/XE Mod Shadow.wgsl"), shadow.clone())].into();
        let shared = Shared::new(
            mods_with_markers(files, &HOMES, &BuildMarkers::default()),
            BuildMarkers::default(),
            Plant::of_env(),
        );
        let texts = [
            shared.world_hooks().shadow,
            shared.world_hooks_without(&[ModFile::Shadow]).shadow,
        ];
        assert_eq!(texts[0], shadow);
        for text in texts {
            assert!(!text.contains(nirender_wgpu::CORE_MOD_FAULT_PLANT));
        }
    }

    /// A marker drops the files it names with the same text; a changed file is used again, and
    /// a marker whose files all changed goes. The null: the start without markers uses the file.
    #[test]
    fn a_build_marker_drops_its_files_until_they_change() {
        let folder = std::env::temp_dir().join(format!("core-mod-markers-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let markers = BuildMarkers::in_folder(&folder);
        let shadow = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        markers.write(WATER_PLANE_HOME, &[(ModFile::Water, "water".to_string())]);
        markers.write(
            WORLD_HOME,
            &[(ModFile::Shadow, shadow.clone()), (ModFile::Sky, "sky".to_string())],
        );
        let texts = Texts {
            shadow: Some(shadow.clone()),
            water: Some("water".to_string()),
            ..Texts::default()
        };
        let mut left = markers.left(&texts);
        left.sort_by_key(|(file, _)| file.key());
        assert_eq!(
            left,
            [
                (ModFile::Shadow, WORLD_HOME.to_string()),
                (ModFile::Water, WATER_PLANE_HOME.to_string())
            ]
        );

        // The start drops the shadow file with the line of its marker; the null uses it.
        let files: HashMap<String, String> = [(format!("{FOLDER}/XE Mod Shadow.wgsl"), shadow.clone())].into();
        let mods = mods_with_markers(files.clone(), &HOMES, &markers);
        assert!(!mods.uses(ModFile::Shadow));
        assert_eq!(
            mods.problems,
            [
                "core mod: XE Mod Shadow.wgsl (with XE Mod Shadow Data.wgsl): a start before ended in the build of the world shader with it; it is not used until it changes"
            ]
        );
        assert!(mods_with_markers(files, &HOMES, &BuildMarkers::default()).uses(ModFile::Shadow));

        // A changed water file is used again, and its marker goes.
        let changed = Texts {
            water: Some("water, changed".to_string()),
            ..texts
        };
        assert_eq!(markers.left(&changed), [(ModFile::Shadow, WORLD_HOME.to_string())]);
        assert!(!folder.join("core-mod-build-water-plane.txt").exists());

        // The removal of the world marker after a build keeps the line of the file it held off.
        markers.remove(WORLD_HOME);
        assert_eq!(markers.left(&changed), [(ModFile::Shadow, WORLD_HOME.to_string())]);

        // A changed shadow file is used again, and the last marker goes.
        let all_changed = Texts {
            shadow: Some(format!("{shadow}// changed\n")),
            ..changed
        };
        assert!(markers.left(&all_changed).is_empty());
        assert!(!folder.join("core-mod-build-world-shader.txt").exists());
        std::fs::remove_dir(&folder).unwrap();
    }

    /// A file that ended the helper in the world shader's build stays held off over the start that
    /// obeys the marker: that start writes the world's marker for its own units and takes it down
    /// after the build ([`Shared::mark_world`], [`Shared::world_held`]), and the next start still
    /// does not use the file. The null: a start with the marker's lines not kept uses it again.
    #[test]
    fn the_world_marker_holds_its_file_over_the_start_that_obeys_it() {
        let folder = std::env::temp_dir().join(format!("core-mod-world-marker-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let shadow = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let files: HashMap<String, String> = [(format!("{FOLDER}/XE Mod Shadow.wgsl"), shadow.clone())].into();
        BuildMarkers::in_folder(&folder).write(WORLD_HOME, &[(ModFile::Shadow, shadow.clone())]);

        // Start 1 obeys the marker, and its world shader builds and holds.
        let markers = BuildMarkers::in_folder(&folder);
        let mods = mods_with_markers(files.clone(), &HOMES, &markers);
        assert!(!mods.uses(ModFile::Shadow));
        let shared = Shared::new(mods, markers, None);
        shared.mark_world(&shared.world_files());
        shared.world_held(&[], "");

        // Start 2 still holds the file off.
        let markers = BuildMarkers::in_folder(&folder);
        assert!(!mods_with_markers(files.clone(), &HOMES, &markers).uses(ModFile::Shadow));

        // The null: the same start 1 without the kept lines takes the marker down.
        let markers = BuildMarkers::in_folder(&folder);
        let mods = mods_with_markers(files.clone(), &HOMES, &markers);
        markers.held_off.borrow_mut().clear();
        let shared = Shared::new(mods, markers, None);
        shared.mark_world(&shared.world_files());
        shared.world_held(&[], "");
        assert!(mods_with_markers(files, &HOMES, &BuildMarkers::in_folder(&folder)).uses(ModFile::Shadow));
        assert!(!folder.join("core-mod-build-world-shader.txt").exists());
        std::fs::remove_dir(&folder).unwrap();
    }

    /// The vertex stage of the test home of [`first_build_a_planted_fault_gives_way_to_the_built_in_hooks`].
    const TEST_VERTEX: &str = "
struct TestOut { @builtin(position) position: vec4f, @location(0) s0: vec3f, @location(1) s1: vec3f }
@vertex fn vs() -> TestOut { return TestOut(vec4f(0.0), vec3f(0.0), vec3f(0.0)); }
";

    /// A planted fault in a home's first build gives way to the built-in hooks, drops the file
    /// for the homes after it and takes the marker down. The null: under `:keep` the failed
    /// build stays and the file stays in use. Needs a GPU:
    /// `cargo test -p mgeHost64 -- --ignored first_build --test-threads=1`.
    #[test]
    #[ignore = "needs a GPU"]
    fn first_build_a_planted_fault_gives_way_to_the_built_in_hooks() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let folder = std::env::temp_dir().join(format!("core-mod-first-build-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let shadow = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let files: HashMap<String, String> = [(format!("{FOLDER}/XE Mod Shadow.wgsl"), shadow.clone())].into();
        let build = |plant: &str| {
            let markers = BuildMarkers::in_folder(&folder);
            let shared = Shared::new(
                mods_with_markers(files.clone(), &HOMES, &markers),
                markers,
                Plant::parse(plant),
            );
            let mut texts = Vec::new();
            shared.first_build(&device, GRASS_HOME, ModFile::Shadow, |hooks| {
                texts.push(hooks.to_string());
                let source = format!("{TEST_HOME}{hooks}{TEST_VERTEX}");
                let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("test home"),
                    source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
                });
                // The pipeline is made for its error alone.
                let _pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("test home"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fragment_entry(&source, "fs")),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            });
            assert!(!folder.join("core-mod-build-grass.txt").exists(), "{plant}: the marker stays");
            (shared, texts)
        };

        let (shared, texts) = build("Shadow@grass");
        assert_eq!(texts.len(), 2);
        assert!(texts[0].starts_with(nirender_wgpu::CORE_MOD_FAULT_PLANT));
        assert_eq!(texts[1], CoreMods::built_in().shadow_hooks());
        assert_eq!(shared.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(
            shared
                .problems()
                .iter()
                .any(|line| line.contains("the grass failed with it on the device"))
        );
        assert!(shared.take_world_changed() && !shared.take_world_changed());

        let (shared, texts) = build("Shadow@grass:keep");
        assert_eq!(texts.len(), 1);
        assert_eq!(shared.shadow_hooks(), shadow);
        assert!(shared.problems().is_empty());

        let (shared, texts) = build("Sky");
        assert_eq!(texts, [shadow.as_str()]);
        assert_eq!(shared.shadow_hooks(), shadow);
        std::fs::remove_dir(&folder).unwrap();
    }
}
