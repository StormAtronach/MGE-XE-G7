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
//!   `underwater_color` show the terms of `MGE_XE_WATER_DEBUG`.
//!
//! A hook may call the other hooks of its file, also those that the file leaves to the
//! built-ins. A hook that never ends stops the GPU: the renderer does not limit a hook.

use nirender_wgpu::wgpu::naga;

use super::shadow;

/// The interface that the first line of a file names.
pub(crate) const INTERFACE: &str = "mge-xe-core 1";

/// The folder of the files, under `Data Files`.
const FOLDER: &str = "shaders/core-mods";

/// The core files of the Direct3D 9 path that a mod can replace with an `.fx`: such a file
/// without its `.wgsl` is named in the log.
const DIRECT3D9_FILES: [&str; 10] = ["Shadow", "Shadow Data", "Water", "Water Standard", "Water Volume", "Caustics", "Grass", "Landscape", "Sky", "Statics"];

/// A function of the renderer that a core mod may replace: its name, the types of its
/// arguments and of its result as naga writes them, and its built-in WGSL.
struct Hook {
    name: &'static str,
    arguments: &'static [&'static str],
    result: &'static str,
    built_in: &'static str,
}

const SHADOW_HOOKS: [Hook; 2] = [
    Hook { name: "shadow_delta", arguments: &["vec3<f32>", "vec3<f32>"], result: "f32", built_in: SHADOW_DELTA },
    Hook { name: "shadow_esm", arguments: &["f32"], result: "f32", built_in: SHADOW_ESM },
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
    Hook { name: "water_normal", arguments: &["vec4<f32>", "f32", "vec2<f32>"], result: "vec3<f32>", built_in: super::water::WATER_NORMAL },
    Hook { name: "water_color", arguments: &["WaterPixel"], result: "vec4<f32>", built_in: super::water::WATER_COLOR },
    Hook { name: "underwater_color", arguments: &["WaterPixel"], result: "vec4<f32>", built_in: super::water::UNDERWATER_COLOR },
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
///   (`sunCol`: with the light multiplier of the weather, without the sunlight factor), the
///   wind (`windVec`), the sky colour (`skyCol`), the far fog colour (`fogColFar`), the place
///   of the eye (`eyePos`), the ambient light (`sunAmb`: the sun's ambient and the scene's
///   together, with the multiplier), 1 over the size of the picture (`rcpRes`), and the height
///   of the dynamic waves (`waveHeight`, 0 without them). A value that the built-in derives
///   from these (the sunlight factor, the wind factor) is not in the library.
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
        ShadowValues { shade: shadow::SHADE, shade_color: shadow::SHADE_COLOR, esm_c: shadow::ESM_C, esm_bias: shadow::ESM_BIAS }
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
            let Some((key, numbers)) = item.split_once('=') else { return Err(format!("the value `{item}` has no `name = number`")) };
            let numbers: Vec<f32> = numbers
                .split_whitespace()
                .map(|word| word.parse::<f32>().ok().filter(|number| number.is_finite()).ok_or_else(|| format!("`{word}` of the value {} is not a number", key.trim())))
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
                other => return Err(format!("the value `{other}` is not known (shade, shade_color, esm_c, esm_bias)")),
            }
        }
        // `shadowSunEstimate` divides by the shade plus the light, which is 0 at night: 0/0 at a
        // shade of 0.
        if values.shade < 0.01 {
            return Err(format!("the value shade = {} is out of its range: 0.01 or more", values.shade));
        }
        if values.shade_color.iter().any(|part| !(0.0..=1.0).contains(part)) {
            let [r, g, b] = values.shade_color;
            return Err(format!("the value shade_color = {r} {g} {b} is out of its range: 0 to 1 in each number"));
        }
        if !(values.esm_c > 0.0 && values.esm_c <= 88.0) {
            return Err(format!("the value esm_c = {} is out of its range: above 0, at most 88", values.esm_c));
        }
        Ok(values)
    }
}

/// A shader that calls the hooks of a file: its name for the log, and the device's check of
/// its whole source with a hook text in it.
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
    /// What the log says about the files, one line each.
    report: Vec<String>,
}

impl CoreMods {
    /// The built-in hooks only.
    pub fn built_in() -> CoreMods {
        CoreMods { shadow_hooks: hook_text(&SHADOW_HOOKS, &[], ""), shadow_values: ShadowValues::default(), water_hooks: hook_text(&WATER_HOOKS, &[], ""), report: Vec::new() }
    }

    /// The files under `Data Files` (`read` gives the bytes of a path there), checked with the
    /// homes of the shadow hooks and of the water hooks; nothing with `MGE_XE_CORE_MODS=0`. The
    /// report is printed.
    pub fn load(read: &dyn Fn(&str) -> Option<Vec<u8>>, shadow_homes: &[Home], water_homes: &[Home]) -> CoreMods {
        let mods = if nirender_wgpu::switches::on("MGE_XE_CORE_MODS") == Some(false) {
            CoreMods { report: vec!["core mods: off (MGE_XE_CORE_MODS=0)".to_string()], ..CoreMods::built_in() }
        } else {
            CoreMods::read(read, shadow_homes, water_homes)
        };
        for line in &mods.report {
            println!("{line}");
        }
        mods
    }

    /// [`load`](Self::load) without the switch and the print.
    fn read(read: &dyn Fn(&str) -> Option<Vec<u8>>, shadow_homes: &[Home], water_homes: &[Home]) -> CoreMods {
        let mut mods = CoreMods::built_in();
        // A byte order mark, which an editor on Windows can put first, is not part of the text.
        let file = |name: &str, extension: &str| read(&format!("{FOLDER}/XE Mod {name}.{extension}")).map(|bytes| String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string());
        for name in DIRECT3D9_FILES {
            if file(name, "wgsl").is_none() && file(name, "fx").is_some() {
                mods.report.push(format!("core mod: XE Mod {name}.fx is for Direct3D 9; the wgpu renderer reads XE Mod {name}.wgsl, and there is none"));
            }
        }
        let mut used = Vec::new();
        let (shadow_file, data_file) = (file("Shadow", "wgsl"), file("Shadow Data", "wgsl"));
        if shadow_file.is_some() || data_file.is_some() {
            match shadow_mod(shadow_file.as_deref(), data_file.as_deref(), shadow_homes) {
                Ok(shadow) => {
                    mods.shadow_hooks = shadow.hooks;
                    mods.shadow_values = shadow.values;
                    if shadow_file.is_some() {
                        used.push(format!("XE Mod Shadow.wgsl ({})", if shadow.defined.is_empty() { "no hook".to_string() } else { shadow.defined.join(", ") }));
                    }
                    if let Some(values) = shadow.values_line {
                        used.push(format!("XE Mod Shadow Data.wgsl ({values})"));
                    }
                }
                Err((name, problem)) => {
                    let other = if name == "Shadow" { "Shadow Data" } else { "Shadow" };
                    let with = if shadow_file.is_some() && data_file.is_some() { format!("; XE Mod {other}.wgsl is dropped with it") } else { String::new() };
                    mods.report.push(format!("core mod: XE Mod {name}.wgsl: {problem}; the built-in shadow is used{with}"));
                }
            }
        }
        if let Some(text) = file("Water", "wgsl") {
            match hook_file(&text, &WATER_HOOKS, &format!("{WATER_INTERFACE}{WATER_LIBRARY}"), &WATER_LIBRARY_NAMES, water_homes) {
                Ok((hooks, defined)) => {
                    mods.water_hooks = hooks;
                    used.push(format!("XE Mod Water.wgsl ({})", if defined.is_empty() { "no hook".to_string() } else { defined.join(", ") }));
                    mods.report.push("core mod: XE Mod Water.wgsl: the plane of the water takes its hooks; the water volumes and the dry spaces keep the built-in water".to_string());
                    if defined.iter().any(|hook| *hook == "water_color" || *hook == "underwater_color") {
                        mods.report.push("core mod: XE Mod Water.wgsl: MGE_XE_WATER_DEBUG shows no terms of its water colour (the terms are those of the built-in)".to_string());
                    }
                }
                Err(problem) => mods.report.push(format!("core mod: XE Mod Water.wgsl: {problem}; the built-in water is used")),
            }
        }
        mods.report.push(if used.is_empty() { format!("core mods: none used (Data Files/{FOLDER}, interface {INTERFACE})") } else { format!("core mods: {} (interface {INTERFACE})", used.join(", ")) });
        mods
    }

    /// The shadow hooks for a home: the text joins the home's source after its library.
    pub fn shadow_hooks(&self) -> &str {
        &self.shadow_hooks
    }

    /// The values of the shadow for the shaders.
    pub fn shadow_values(&self) -> ShadowValues {
        self.shadow_values
    }

    /// The water hooks for the plane of the water: the text joins the plane's source after its
    /// library and [`WATER_INTERFACE`].
    pub fn water_hooks(&self) -> &str {
        &self.water_hooks
    }

    /// The core mods of `files` (names under the folder, and texts), for a test of a home.
    #[cfg(test)]
    pub fn of_files(files: &[(&str, &str)], shadow_homes: &[Home], water_homes: &[Home]) -> CoreMods {
        let files: std::collections::HashMap<String, String> = files.iter().map(|(name, text)| (format!("{FOLDER}/XE Mod {name}"), text.to_string())).collect();
        CoreMods::read(&|path| files.get(path).map(|text| text.as_bytes().to_vec()), shadow_homes, water_homes)
    }

    /// The lines of the log about the files.
    #[cfg(test)]
    fn report(&self) -> &[String] {
        &self.report
    }
}

/// The shadow files that pass.
struct ShadowMod {
    hooks: String,
    values: ShadowValues,
    defined: Vec<&'static str>,
    values_line: Option<String>,
}

/// Checks the shadow files and joins them; the error names the file that failed.
fn shadow_mod(shadow_file: Option<&str>, data_file: Option<&str>, homes: &[Home]) -> Result<ShadowMod, (&'static str, String)> {
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
    let Some(text) = shadow_file else { return Ok(ShadowMod { hooks: hook_text(&SHADOW_HOOKS, &[], ""), values, defined: Vec::new(), values_line }) };
    let (hooks, defined) = hook_file(text, &SHADOW_HOOKS, SHADOW_LIBRARY, &SHADOW_LIBRARY_NAMES, homes).map_err(|problem| ("Shadow", problem))?;
    Ok(ShadowMod { hooks, values, defined, values_line })
}

/// Checks a file of hooks with its header, its rules and each of its homes. Gives the hook text
/// for the homes (the file's hooks and the built-ins of the others), and the hooks it defines.
fn hook_file(text: &str, hooks: &'static [Hook], library: &str, library_names: &[&str], homes: &[Home]) -> Result<(String, Vec<&'static str>), String> {
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
    let mut rest = text.lines().map(str::trim).skip_while(|line| line.is_empty()).filter(|line| !line.is_empty()).peekable();
    let mut header_lines = Vec::new();
    while let Some(line) = rest.next_if(|line| line.starts_with("//!")) {
        header_lines.push(&line[3..]);
    }
    if let Some(late) = rest.find(|line| line.starts_with("//!")) {
        return Err(format!("the line `{late}` comes after the code: the `//!` lines come first"));
    }
    let mut lines = header_lines.into_iter();
    let interface = lines.next().and_then(|line| line.split_once(':')).filter(|(key, _)| key.trim() == "interface").map(|(_, value)| value.trim().to_string());
    match interface {
        None => return Err(format!("no line `//! interface: {INTERFACE}` at its start")),
        Some(value) if value != INTERFACE => return Err(format!("the interface `{value}` is not {INTERFACE}")),
        Some(_) => {}
    }
    let mut keys = Vec::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else { return Err(format!("the line `//!{line}` has no `key: value`")) };
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
    let defined: Vec<&'static str> = hooks.iter().filter(|hook| defines(text, hook.name)).map(|hook| hook.name).collect();
    // The file comes first, so that the lines of an error are the lines of the file. The hooks
    // that it lacks are declarations: a file may call them, and the built-ins may use what the
    // renderer has outside the interface.
    let lacking: String = hooks.iter().filter(|hook| !defined.contains(&hook.name)).map(declaration).collect();
    let source = format!("{text}\n{library}\n{lacking}");
    let module = naga::front::wgsl::parse_str(&source).map_err(|error| error.emit_to_string(&source))?;
    if let Some(entry) = module.entry_points.first() {
        return Err(format!("the entry point `{}`: a core mod has none", entry.name));
    }
    if let Some((_, variable)) = module.global_variables.iter().next() {
        return Err(format!("the variable `{}` outside a function: a core mod has none (no binding)", variable.name.as_deref().unwrap_or("?")));
    }
    if let Some((_, value)) = module.overrides.iter().next() {
        return Err(format!("the override `{}`: a core mod has none", value.name.as_deref().unwrap_or("?")));
    }
    let own = |name: &str| name.starts_with("mod_");
    for (_, function) in module.functions.iter() {
        let name = function.name.as_deref().unwrap_or("");
        if let Some(hook) = hooks.iter().find(|hook| hook.name == name) {
            let arguments: Vec<String> = function.arguments.iter().map(|argument| type_name(&module, argument.ty)).collect();
            let result = function.result.as_ref().map_or("nothing".to_string(), |result| type_name(&module, result.ty));
            if arguments != hook.arguments || result != hook.result {
                return Err(format!("the hook `{name}` takes ({}) and gives {result}; interface 1 has ({}) -> {}", arguments.join(", "), hook.arguments.join(", "), hook.result));
            }
        } else if !own(name) && !library_names.contains(&name) {
            return Err(format!("the function `{name}`: a function of a core mod is a hook of its file, or its name starts with `mod_`"));
        }
    }
    for (_, kind) in module.types.iter() {
        // naga names the result structs of some built-in functions `__...`.
        if let (naga::TypeInner::Struct { .. }, Some(name)) = (&kind.inner, kind.name.as_deref())
            && !own(name)
            && !name.starts_with("__")
            && !library_names.contains(&name)
        {
            return Err(format!("the struct `{name}`: the name of a struct of a core mod starts with `mod_`"));
        }
    }
    if let Some(name) = module.constants.iter().filter_map(|(_, constant)| constant.name.as_deref()).find(|name| !own(name)) {
        return Err(format!("the constant `{name}`: the name of a constant of a core mod starts with `mod_`"));
    }
    Ok(defined)
}

/// A hook as a function with its signature and a zero result, for the check of a file that
/// lacks it.
fn declaration(hook: &Hook) -> String {
    let arguments: Vec<String> = hook.arguments.iter().enumerate().map(|(index, kind)| format!("a{index}: {kind}")).collect();
    format!("fn {}({}) -> {} {{ return {}(); }}\n", hook.name, arguments.join(", "), hook.result, hook.result)
}

/// True when `text` declares the function `name` (`fn name`), outside a line comment.
fn defines(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        let words: Vec<&str> = code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|word| !word.is_empty()).collect();
        words.windows(2).any(|pair| pair[0] == "fn" && pair[1] == name)
    })
}

/// The built-ins of the hooks that `defined` lacks, then `text`.
fn hook_text(hooks: &[Hook], defined: &[&str], text: &str) -> String {
    let mut joined: String = hooks.iter().filter(|hook| !defined.contains(&hook.name)).map(|hook| hook.built_in).collect();
    joined.push_str(text);
    joined
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
    let home = naga::front::wgsl::parse_str(home_source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(home_source)));
    let signature = |module: &naga::Module, function: &naga::Function| {
        let arguments: Vec<String> = function.arguments.iter().map(|argument| type_name(module, argument.ty)).collect();
        format!("({}) -> {}", arguments.join(", "), function.result.as_ref().map_or("nothing".to_string(), |result| type_name(module, result.ty)))
    };
    let mut differences = Vec::new();
    for (_, function) in declared.functions.iter() {
        let name = function.name.as_deref().unwrap_or("");
        let wanted = signature(&declared, function);
        match home.functions.iter().find(|(_, other)| other.name.as_deref() == Some(name)) {
            Some((_, other)) if signature(&home, other) == wanted => {}
            Some((_, other)) => differences.push(format!("{name}: the home has {}, the interface {wanted}", signature(&home, other))),
            None => differences.push(format!("{name}: not in the home")),
        }
    }
    differences
}

#[cfg(test)]
pub(crate) use tests::marker_water_file;

#[cfg(test)]
mod tests {
    use super::*;
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

    const HOMES: [Home; 1] = [Home { name: "test home", check: test_home }];

    fn mods_of(files: &[(&str, &str)], homes: &[Home]) -> CoreMods {
        let files: HashMap<String, String> = files.iter().map(|(name, text)| (format!("{FOLDER}/XE Mod {name}"), text.to_string())).collect();
        CoreMods::read(&|path| files.get(path).map(|text| text.as_bytes().to_vec()), homes, &[])
    }

    fn header_line() -> String {
        format!("//! interface: {INTERFACE}\n")
    }

    #[test]
    fn the_built_in_hooks_are_valid_in_a_home() {
        test_home(CoreMods::built_in().shadow_hooks()).unwrap();
        let mods = mods_of(&[], &HOMES);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert_eq!(mods.report(), [format!("core mods: none used (Data Files/{FOLDER}, interface {INTERFACE})")]);
    }

    #[test]
    fn a_copy_of_the_built_ins_is_used_whole() {
        let file = format!("{}{SHADOW_DELTA}{SHADOW_ESM}", header_line());
        let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
        // Both hooks come from the file: no built-in is joined.
        assert_eq!(mods.shadow_hooks(), file);
        assert_eq!(mods.report(), [format!("core mods: XE Mod Shadow.wgsl (shadow_delta, shadow_esm) (interface {INTERFACE})")]);
    }

    #[test]
    fn a_file_with_one_hook_keeps_the_built_in_of_the_other() {
        let file = format!("{}fn mod_half(x: f32) -> f32 {{ return 0.5 * x; }}\nfn shadow_esm(dz: f32) -> f32 {{ return mod_half(1.0); }}\n", header_line());
        let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
        assert_eq!(mods.shadow_hooks(), format!("{SHADOW_DELTA}{file}"));
        assert!(mods.report()[0].contains("XE Mod Shadow.wgsl (shadow_esm)"), "{:?}", mods.report());
    }

    /// Each rule drops the file with its reason, and the built-ins stay.
    #[test]
    fn a_file_that_breaks_a_rule_is_dropped_with_its_reason() {
        let cases = [
            ("fn shadow_esm(dz: f32) -> f32 { return 0.0; }\n".to_string(), "no line `//! interface: mge-xe-core 1` at its start"),
            ("//! interface: mge-xe-core 2\n".to_string(), "the interface `mge-xe-core 2` is not mge-xe-core 1"),
            (format!("{}//! values: shade = 1\n", header_line()), "the key `values` is not known in this file"),
            (format!("{}@group(3) @binding(0) var<uniform> mod_u: vec4f;\n", header_line()), "the variable `mod_u` outside a function"),
            (format!("{}var<private> mod_p: f32;\n", header_line()), "the variable `mod_p` outside a function"),
            (format!("{}@fragment fn mod_fs() -> @location(0) vec4f {{ return vec4f(1.0); }}\n", header_line()), "the entry point `mod_fs`"),
            (format!("{}fn helper() -> f32 {{ return 1.0; }}\n", header_line()), "the function `helper`"),
            (format!("{}struct Light {{ x: f32 }}\n", header_line()), "the struct `Light`"),
            (format!("{}const SCALE: f32 = 2.0;\n", header_line()), "the constant `SCALE`"),
            (format!("{}fn shadow_esm(dz: f32, x: f32) -> f32 {{ return 0.0; }}\n", header_line()), "the hook `shadow_esm` takes (f32, f32) and gives f32; interface 1 has (f32) -> f32"),
            (format!("{}fn shadow_delta(s0: vec3f, s1: vec3f) -> vec2f {{ return vec2f(0.0); }}\n", header_line()), "the hook `shadow_delta` takes (vec3<f32>, vec3<f32>) and gives vec2<f32>"),
            (format!("{}fn shadow_esm(dz: f32) -> f32 {{ return no_such(dz); }}\n", header_line()), "no_such"),
        ];
        for (file, reason) in cases {
            let mods = mods_of(&[("Shadow.wgsl", &file)], &HOMES);
            assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks(), "{file}");
            assert!(mods.report()[0].starts_with("core mod: XE Mod Shadow.wgsl: ") && mods.report()[0].contains(reason), "{file}: {:?}", mods.report());
            assert!(mods.report()[0].ends_with("; the built-in shadow is used"), "{:?}", mods.report());
            assert!(mods.report()[1].starts_with("core mods: none used"), "{:?}", mods.report());
        }
    }

    #[test]
    fn a_file_that_a_home_refuses_is_dropped() {
        let file = format!("{}{SHADOW_ESM}", header_line());
        let homes = [Home { name: "test home", check: test_home }, Home { name: "grass", check: failing_home }];
        let mods = mods_of(&[("Shadow.wgsl", &file)], &homes);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(mods.report()[0].contains("the grass fails with it: a planted failure"), "{:?}", mods.report());
    }

    #[test]
    fn the_values_of_shadow_data_replace_the_built_in_ones() {
        let data = format!("{}//! values: shade = 0.8, shade_color = 1 0.5 0.25\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &data)], &HOMES);
        let values = mods.shadow_values();
        assert_eq!((values.shade, values.shade_color), (0.8, [1.0, 0.5, 0.25]));
        assert_eq!((values.esm_c, values.esm_bias), (shadow::ESM_C, shadow::ESM_BIAS));
        assert_eq!(values.shade(), [1.0, 0.5, 0.25, 0.8]);
        assert_eq!(values.esm(0.5), [shadow::ESM_C, shadow::ESM_BIAS, shadow::ESM_SCALE, 0.5]);
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(mods.report()[0].contains("XE Mod Shadow Data.wgsl (shade = 0.8, shade_color = 1 0.5 0.25)"), "{:?}", mods.report());
    }

    /// A bad value drops Shadow Data and the Shadow file with it; a bad Shadow file drops the
    /// values of Shadow Data too.
    #[test]
    fn shadow_and_shadow_data_fall_back_together() {
        let good_data = format!("{}//! values: shade = 0.8\n", header_line());
        let good_shadow = format!("{}{SHADOW_ESM}", header_line());
        for (shadow_file, data, reason) in [
            (good_shadow.clone(), format!("{}//! values: shade = dark\n", header_line()), "XE Mod Shadow Data.wgsl: `dark` of the value shade is not a number"),
            (good_shadow.clone(), format!("{}//! values: depth = 1\n", header_line()), "the value `depth` is not known"),
            (good_shadow.clone(), format!("{}//! values: shade_color = 1 1\n", header_line()), "the value shade_color has 2 numbers, not 3"),
            (good_shadow.clone(), format!("{}//! values: shade = 0\n", header_line()), "the value shade = 0 is out of its range: 0.01 or more"),
            (good_shadow.clone(), format!("{}//! values: shade_color = 1 2 0\n", header_line()), "the value shade_color = 1 2 0 is out of its range: 0 to 1 in each number"),
            (good_shadow.clone(), format!("{}//! values: esm_c = 88.5\n", header_line()), "the value esm_c = 88.5 is out of its range: above 0, at most 88"),
            (good_shadow.clone(), format!("{}//! values: shade = 0.8\nfn mod_x() {{}}\nfn shadow_esm(dz: f32) -> f32 {{ return dz; }}\n", header_line()), "XE Mod Shadow Data.wgsl: the function `shadow_esm`"),
            (format!("{}fn helper() {{}}\n", header_line()), good_data.clone(), "XE Mod Shadow.wgsl: the function `helper`"),
        ] {
            let mods = mods_of(&[("Shadow.wgsl", &shadow_file), ("Shadow Data.wgsl", &data)], &HOMES);
            assert_eq!(mods.shadow_values(), ShadowValues::default(), "{reason}");
            assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks(), "{reason}");
            assert!(mods.report()[0].contains(reason) && mods.report()[0].ends_with("is dropped with it"), "{reason}: {:?}", mods.report());
        }
    }

    #[test]
    fn an_fx_file_without_its_wgsl_is_named() {
        let mods = mods_of(&[("Water.fx", "technique T {}"), ("Sky.fx", "technique T {}"), ("Sky.wgsl", "not read yet")], &HOMES);
        assert_eq!(mods.report()[0], "core mod: XE Mod Water.fx is for Direct3D 9; the wgpu renderer reads XE Mod Water.wgsl, and there is none");
        assert_eq!(mods.report().len(), 2, "{:?}", mods.report());
    }

    /// Blank lines between the `//!` lines keep the values; a `//!` line after the code drops
    /// the file with a word; a byte order mark before the first line is not read.
    #[test]
    fn the_header_takes_blank_lines_and_a_byte_order_mark() {
        let data = format!("\n{}\n//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &data)], &HOMES);
        assert_eq!(mods.shadow_values().shade, 0.8, "{:?}", mods.report());
        let late = format!("{}fn mod_x() {{}}\n//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &late)], &HOMES);
        assert_eq!(mods.shadow_values(), ShadowValues::default());
        assert!(mods.report()[0].contains("the line `//! values: shade = 0.8` comes after the code"), "{:?}", mods.report());
        let marked = format!("\u{feff}{}//! values: shade = 0.8\n", header_line());
        let mods = mods_of(&[("Shadow Data.wgsl", &marked)], &HOMES);
        assert_eq!(mods.shadow_values().shade, 0.8, "{:?}", mods.report());
        let marked = format!("\u{feff}{}{SHADOW_ESM}", header_line());
        let mods = mods_of(&[("Shadow.wgsl", &marked)], &HOMES);
        assert!(!mods.shadow_hooks().contains('\u{feff}') && mods.report()[0].contains("XE Mod Shadow.wgsl (shadow_esm)"), "{:?}", mods.report());
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
        let names: Vec<&str> = module.functions.iter().filter_map(|(_, function)| function.name.as_deref()).collect();
        assert_eq!(names, SHADOW_LIBRARY_NAMES);
        let module = naga::front::wgsl::parse_str(&format!("{WATER_INTERFACE}{WATER_LIBRARY}")).unwrap();
        let mut names: Vec<&str> = module.functions.iter().filter_map(|(_, function)| function.name.as_deref()).collect();
        names.push("WaterPixel");
        assert_eq!(names, WATER_LIBRARY_NAMES);
    }

    /// A water file that colours the plane one colour and calls the built-in normal, which it
    /// lacks: the marker of the tests of the plane and of the other water modules.
    pub(crate) fn marker_water_file() -> String {
        format!("{}fn water_color(p: WaterPixel) -> vec4f {{\n    return vec4f(mod_marker(water_normal(p.texcoords, p.distance, p.world.xy)), 1.0);\n}}\nfn mod_marker(normal: vec3f) -> vec3f {{\n    return vec3f(0.123, 0.0, 0.0) + 0.0 * normal;\n}}\n", header_line())
    }

    #[test]
    fn a_water_file_joins_the_plane_with_the_built_ins_it_lacks() {
        let file = marker_water_file();
        let mods = CoreMods::of_files(&[("Water.wgsl", &file)], &HOMES, &[]);
        assert_eq!(mods.water_hooks(), format!("{}{}{file}", super::super::water::WATER_NORMAL, super::super::water::UNDERWATER_COLOR));
        assert_eq!(mods.shadow_hooks(), CoreMods::built_in().shadow_hooks());
        assert!(mods.report().iter().any(|line| line.contains("XE Mod Water.wgsl (water_color)")), "{:?}", mods.report());
        assert!(mods.report().iter().any(|line| line.contains("the water volumes and the dry spaces keep the built-in water")), "{:?}", mods.report());
        assert!(mods.report().iter().any(|line| line.contains("MGE_XE_WATER_DEBUG shows no terms")), "{:?}", mods.report());
    }

    #[test]
    fn a_water_file_that_breaks_a_rule_is_dropped() {
        for (file, reason) in [
            (format!("{}fn water_color(p: WaterPixel) -> vec3f {{ return vec3f(0.0); }}\n", header_line()), "the hook `water_color` takes (WaterPixel) and gives vec3<f32>"),
            (format!("{}struct WaterPixel {{ x: f32 }}\n", header_line()), "WaterPixel"),
            (format!("{}fn water_color(p: WaterPixel) -> vec4f {{ return vec4f(water.sky_col.rgb, 1.0); }}\n", header_line()), "water"),
        ] {
            let mods = CoreMods::of_files(&[("Water.wgsl", &file)], &HOMES, &[]);
            assert_eq!(mods.water_hooks(), CoreMods::built_in().water_hooks(), "{file}");
            assert!(mods.report()[0].starts_with("core mod: XE Mod Water.wgsl: ") && mods.report()[0].contains(reason) && mods.report()[0].ends_with("; the built-in water is used"), "{file}: {:?}", mods.report());
        }
    }

    #[test]
    fn a_hook_is_declared_by_its_signature() {
        assert_eq!(declaration(&WATER_HOOKS[1]), "fn water_color(a0: WaterPixel) -> vec4<f32> { return vec4<f32>(); }\n");
        assert_eq!(declaration(&SHADOW_HOOKS[0]), "fn shadow_delta(a0: vec3<f32>, a1: vec3<f32>) -> f32 { return f32(); }\n");
    }
}
