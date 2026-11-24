//! part: shadow_esm

// shadowESM: the shadow amount of a pixel behind the map by `dz` (a negative number).
fn shadow_esm(dz: f32) -> f32 {
    let esm = shadow_esm_values();
    return 1.0 - clamp(exp(esm.x * dz + esm.y), 0.0, 1.0);
}
//! part: cascades

// The shadow of the cascades of the map (not a hook; each home of the shadow hooks gets it). A
// pixel takes the cascade of its distance along the view axis; over the last 10 % of a slice
// it blends with the next cascade, and the last cascade fades out to the draw distance. The
// depth difference goes into the unit of 16384 world units with the cascade's depth range,
// and one f16 step of the cascade is added to the bias. `shadow_esm` gives the amount.

// The place in the atlas of `uv` (0 to 1 over a cascade): the cascade's square, half a texel
// inside its edges, so that the filter takes no texel of another cascade.
fn shadow_atlas_uv(cascade: u32, uv: vec2f) -> vec2f {
    let half = 0.5 * shadow_texel();
    let square = vec2f(f32(cascade % 2u), f32(cascade / 2u));
    return 0.5 * (square + clamp(uv, vec2f(half), vec2f(1.0 - half)));
}

// The cascade of a pixel at the distance `z` along the view axis (4 past the last slice), and
// the weight of the next cascade in it, 0 before the band at the end of the slice and 1 at
// its end (`cascade_of` of shadow.rs).
fn shadow_cascade_of(z: f32) -> vec2f {
    var start = 0.0;
    for (var cascade = 0u; cascade < 4u; cascade++) {
        let end = shadow_slice_end(cascade);
        if z < end {
            let band = end - 0.1 * (end - start);
            return vec2f(f32(cascade), clamp((z - band) / (end - band), 0.0, 1.0));
        }
        start = end;
    }
    return vec2f(4.0, 1.0);
}

// The shadow amount, 0 to 1, of the receiver's point `p` in a cascade; 0 past the last.
fn shadow_cascade_amount(cascade: u32, p: vec4f) -> f32 {
    if cascade >= 4u {
        return 0.0;
    }
    let s = shadow_place(cascade, p);
    let uv = vec2f(0.5 + 0.5 * s.x, 0.5 - 0.5 * s.y);
    let range = shadow_depth_range(cascade);
    let dz = (shadow_map_depth(cascade, uv) / shadow_esm_scale() - s.z) * range + range / 2048.0;
    if dz < 0.0 {
        return shadow_esm(dz);
    }
    return 0.0;
}

// The shadow amount, 0 to 1, of a pixel at the distance `z` along the view axis, at the
// receiver's point `p`.
fn shadow_of_cascades(z: f32, p: vec4f) -> f32 {
    let choice = shadow_cascade_of(z);
    let cascade = u32(choice.x);
    var v = shadow_cascade_amount(cascade, p);
    if choice.y > 0.0 {
        v = mix(v, shadow_cascade_amount(cascade + 1u, p), choice.y);
    }
    return v;
}
