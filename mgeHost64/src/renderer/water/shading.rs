//! The water shading that each surface of water shares: the values of the frame, the fog
//! over the water, the normal of the waves and the ripples, the refraction with the shore,
//! the light of the sun on the water, and the look from under the water. `XE Mod Water.fx`
//! and `XE Common.fx`. The plane in place of the water plane of the game (`water.rs`) and the
//! surfaces of water volumes (`volume.rs`) have their own entry points after this source.

use crate::renderer::core_files::WATER;

/// The values of a frame for the water shader (`water` in group 1; the game view of the view
/// group has the camera, the fog and the scattering). The names are those of the uniforms of
/// `XE Mod Water.fx`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::renderer) struct Uniforms {
    pub view: [f32; 16],
    /// The translation of the plane: the camera's x and y, and the water level.
    pub world: [f32; 4],
    /// x: the time of the simulation.
    pub time: [f32; 4],
    /// The direction to the sun, and its visibility.
    pub sun_pos: [f32; 4],
    /// The colour of the sun with its visibility in it, and the wind factor.
    pub sun_col: [f32; 4],
    /// The sky colour, and the good weather value.
    pub sky_col: [f32; 4],
    /// x: the debug term (the debug view `water`); y: 1 under water; z: 1 in a plain interior
    /// (`water_is_interior` of the water hooks).
    pub modes: [f32; 4],
    /// The two numbers of the depth of the projection of the game, for the distance of a
    /// pixel from its depth; then the most height for its width of the blur that is sharp
    /// at the waterline.
    pub projection: [f32; 4],
    /// The size of the picture, the view depth where the plane ends (0 for no end), and
    /// the blur of the reflection: 0 for none, 1 for the six reads of MGE XE, 2 for the
    /// blur that is sharp at the waterline.
    pub size: [f32; 4],
    /// The dynamic waves: the world position of the corner of the texture of the ripples
    /// of the player, the wave height, and 1 when the dynamic waves are on.
    pub wave: [f32; 4],
    /// The mirror: the two numbers of the depth of its projection, the first number of the
    /// projection (the picture width of a view angle), and the widest angle of the blur
    /// that is sharp at the waterline, in radians.
    pub mirror: [f32; 4],
    /// The two numbers of the depth of the projection of the distant land (`before_world`),
    /// for the distance of a pixel of its depth; then those of the clip depth of the plane:
    /// the near plane of the game and the far plane of the distant land.
    pub land: [f32; 4],
    /// The colour of the sun with the light multiplier of the weather and no sunlight factor:
    /// the variable `sunCol` of Direct3D 9, for the water hooks (`water_sun_color`). Direct3D 9
    /// applies the multiplier only with per-pixel lighting on; this colour has it always.
    pub sun_color: [f32; 4],
    /// The wind of the weather in x and y: the variable `windVec` of Direct3D 9, for the water
    /// hooks (`water_wind`).
    pub wind: [f32; 4],
}

/// The caustics: the light that the waves of a surface focus on what is under it. One
/// function for the sea (`caustics.rs`) and for the surfaces of water volumes; the shader
/// that has it binds `waves` and `wave_sampler`.
pub(in crate::renderer) fn caustics() -> &'static str {
    WATER.part("caustics")
}

/// The strength of the caustics for the light of the frame: from the setting (0 to 100),
/// the visibility of the sun and the far fog colour, which has the light of the sky.
pub(in crate::renderer) fn caustic_strength(intensity: f32, sun_vis: f32, fog_color_far: [f32; 3]) -> f32 {
    let sunlight = 1.0 - (1.0 - sun_vis).powi(2);
    let sky =
        (fog_color_far[0] * fog_color_far[0] + fog_color_far[1] * fog_color_far[1] + fog_color_far[2] * fog_color_far[2])
            .sqrt();
    0.05 * intensity * (0.75 * sunlight + 0.35 * sky).clamp(0.0, 1.0)
}

/// The WGSL of the shared shading: the values and the textures of the frame for the water in
/// group 1, and the functions. The shader starts with the view group (`view::wgsl`).
pub(super) fn common() -> &'static str {
    WATER.part("common")
}

/// The built-in `water_normal` (`getFinalWaterNormal`): a hook of `XE Mod Water`
/// (`core_mods.rs`). The plane takes it or the hook of a core mod; the surfaces of the water
/// volumes and the dry spaces always take it.
pub(in crate::renderer) fn water_normal() -> &'static str {
    WATER.part("water_normal")
}

/// The names of the library above that reach what the reflection pass draws: the bindings of
/// its colour and its depth, and every function that reads them, itself or through a call. A
/// shader that names none of them does not read the reflection, so a frame whose readers all
/// name none can skip the pass. `the_reflection_readers_are_listed` keeps the list whole.
pub(in crate::renderer) const REFLECTION_READERS: [&str; 6] = [
    "reflection",
    "mirror_depth",
    "mirror_way",
    "reflection_soft",
    "reflection_at",
    "underwater",
];

/// True when the WGSL text names one of [`REFLECTION_READERS`] as a word outside a comment.
pub(in crate::renderer) fn reads_reflection(wgsl: &str) -> bool {
    words(&without_comments(wgsl)).any(|word| REFLECTION_READERS.contains(&word))
}

/// The text without its `//` and `/* */` comments.
fn without_comments(wgsl: &str) -> String {
    let mut out = String::with_capacity(wgsl.len());
    let mut rest = wgsl;
    while !rest.is_empty() {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(at), b) if b.is_none_or(|b| at < b) => {
                out.push_str(&rest[..at]);
                rest = rest[at..].find('\n').map_or("", |end| &rest[at + end..]);
            }
            (_, Some(at)) => {
                out.push_str(&rest[..at]);
                rest = rest[at + 2..].find("*/").map_or("", |end| &rest[at + 2 + end + 2..]);
            }
            _ => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out
}

/// The line of the shared shading that names the debug view `water` ([`common`]).
const WATER_DEBUG_LINE: &str = "const WATER_DEBUG: bool = false;";

/// The text of a water shader for the debug view `water`: the text with true on its one
/// `WATER_DEBUG` line. None when the text does not have that line exactly once: a missed line
/// would give a view equal to normal play, with no error.
pub(super) fn debug_source(source: &str) -> Option<String> {
    if source.matches(WATER_DEBUG_LINE).count() != 1 {
        return None;
    }
    Some(source.replacen(WATER_DEBUG_LINE, "const WATER_DEBUG: bool = true;", 1))
}

/// The words of a WGSL text: its names and numbers.
fn words(wgsl: &str) -> impl Iterator<Item = &str> {
    wgsl.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each function of the library that a mod's shader can call names no listed name unless
    /// it is listed itself: a new function that reads the reflection, or calls one that does,
    /// fails here until it joins the list.
    #[test]
    fn the_reflection_readers_are_listed() {
        for library in [common(), water_normal(), caustics()] {
            let text = without_comments(library);
            for function in text.split("\nfn ").skip(1) {
                let name = function.split('(').next().unwrap().trim();
                let body = &function[function.find('(').unwrap()..];
                if !REFLECTION_READERS.contains(&name) {
                    assert!(
                        !reads_reflection(body),
                        "the function {name} reaches the reflection: add it to REFLECTION_READERS"
                    );
                }
            }
        }
        // Each listed name is in the library: a stale name hides nothing, but says so.
        for name in REFLECTION_READERS {
            assert!(
                words(&without_comments(common())).any(|word| word == name),
                "{name} is not in the part common"
            );
        }
    }

    #[test]
    fn a_mod_shader_reads_the_reflection_by_a_listed_name() {
        assert!(reads_reflection(
            "fn water_shader(s: WaterSurface) -> vec4f { return underwater(s.screen, 1.0, vec4f(0.0), s.flat.xy, vec3f(0.0), 1.0); }"
        ));
        assert!(reads_reflection("let c = textureLoad(mirror_depth, vec2i(0), 0);"));
        assert!(!reads_reflection(
            "// the reflection of the sky\nfn water_shader(s: WaterSurface) -> vec4f { return vec4f(sky_color(s.facing), 1.0); }"
        ));
        assert!(!reads_reflection("/* reflection_at */ let my_reflection_at = 1.0;"));
    }

    #[test]
    fn the_water_struct_has_the_size_of_its_wgsl() {
        // A matrix and thirteen vectors of four numbers.
        assert_eq!(size_of::<Uniforms>(), 4 * (16 + 13 * 4));
        let text = common();
        let water = &text[text.find("struct Water {").unwrap()..];
        let water = &water[..water.find('}').unwrap()];
        assert_eq!(water.matches(": vec4f,").count(), 13);
        assert_eq!(water.matches(": mat4x4f,").count(), 1);
    }

    /// The text of the debug view changes the one `WATER_DEBUG` line; a text without it, or with
    /// it twice, has no debug text.
    #[test]
    fn the_debug_text_changes_the_one_water_debug_line() {
        let common = common();
        let source = debug_source(common).expect("the line is there once");
        assert_eq!(source.matches("const WATER_DEBUG: bool = true;").count(), 1);
        assert_eq!(
            source.replacen("const WATER_DEBUG: bool = true;", WATER_DEBUG_LINE, 1),
            common
        );
        assert_eq!(debug_source(&common.replace(WATER_DEBUG_LINE, "")), None);
        assert_eq!(debug_source(&format!("{common}{common}")), None);
    }
}
