//! The shadow hooks of the shaders of the wgpu renderer: `shadow_delta` (`shadowDeltaZ`) and
//! `shadow_esm` (`shadowESM`), with one built-in text for their three homes (the draws of the
//! game in the world shader, the grass, the surfaces of the water volumes), and the values of
//! the shadow, with one source for their three writers. Each home gives the library that the
//! hooks call, with its own bindings: `shadow_map_depth(layer: u32, uv: vec2f) -> f32` (the map
//! at level 0, layer 0 or 1, `uv` 0 to 1 over the layer), `shadow_texel() -> f32` (1 over the
//! resolution of a layer), `shadow_esm_scale() -> f32` (the scale of the map's depth) and
//! `shadow_esm_values() -> vec2f` (`esm_c` and `esm_bias`). The core mods of D226 replace these
//! hooks (the design: `moreFPS/docs/Numidium Project/designs/core-mod-hooks-2026-11-10.md`).

use super::shadow;

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
}

/// The shadow hooks and the values of the shadow of the session.
pub(crate) struct CoreMods {
    /// The text of the shadow hooks, which joins each home after its library.
    shadow_hooks: String,
    shadow_values: ShadowValues,
}

impl CoreMods {
    /// The built-in hooks and values.
    pub fn built_in() -> CoreMods {
        CoreMods { shadow_hooks: format!("{SHADOW_DELTA}{SHADOW_ESM}"), shadow_values: ShadowValues::default() }
    }

    /// The shadow hooks for a home: the text joins the home's source after its library.
    pub fn shadow_hooks(&self) -> &str {
        &self.shadow_hooks
    }

    /// The values of the shadow for the shaders.
    pub fn shadow_values(&self) -> ShadowValues {
        self.shadow_values
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_values_give_the_vectors_of_the_shaders() {
        let values = ShadowValues::default();
        assert_eq!(values.esm(0.5), [shadow::ESM_C, shadow::ESM_BIAS, shadow::ESM_SCALE, 0.5]);
        assert_eq!(values.shade(), [shadow::SHADE_COLOR[0], shadow::SHADE_COLOR[1], shadow::SHADE_COLOR[2], shadow::SHADE]);
    }
}
