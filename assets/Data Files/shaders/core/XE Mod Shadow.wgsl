//! part: shadow_delta

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
//! part: shadow_esm

// shadowESM: the shadow amount of a pixel behind the map by `dz` (a negative number).
fn shadow_esm(dz: f32) -> f32 {
    let esm = shadow_esm_values();
    return 1.0 - clamp(exp(esm.x * dz + esm.y), 0.0, 1.0);
}
