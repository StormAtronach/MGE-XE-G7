//! part: caustics

// The view group (`view.rs`, group 0) comes before this code: the game view in `u`, with the
// camera, the direction of the sun and the linear fog of the near cells (`u.fog.zw`). The
// values of the caustics are in `c`, group 1.
struct Caustics {
    view: mat4x4f,
    // The two numbers of the projection for the width and the height, and the two that
    // give the depth.
    projection: vec4f,
    // The size of the picture, and the length of the blend into the distant land.
    size: vec4f,
    // x: the level of the water; y: the time of the waves.
    water: vec4f,
    // x: the strength; y: the view range of the game.
    values: vec4f,
}
@group(1) @binding(0) var<uniform> c: Caustics;
@group(1) @binding(1) var depth: texture_depth_2d;
@group(1) @binding(2) var waves: texture_3d<f32>;
@group(1) @binding(3) var wave_sampler: sampler;

struct Corner {
    @builtin(position) position: vec4f,
    // The draw of the chain (the instance): see `draws`.
    @location(0) @interpolate(flat) step: u32,
}

@vertex fn vs(@builtin(vertex_index) index: u32, @builtin(instance_index) step: u32) -> Corner {
    // One triangle that covers the picture.
    return Corner(vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0), step);
}

@fragment fn fs(in: Corner) -> @location(0) vec4f {
    let position = in.position;
    let at = vec2i(position.xy);
    let d = textureLoad(depth, at, 0);
    // The depth of the pixel along the view of the camera. Nothing drawn, or past the view
    // range of the game: no caustics.
    if depth_empty(d) {
        discard;
    }
    let z = view_distance(d, c.projection.zw);
    if z > c.values.y {
        discard;
    }
    // The point of the pixel in the world: the ray of the pixel for a unit of view depth.
    let ndc = position.xy / c.size.xy * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    let ray = vec3f(c.view[0].z, c.view[1].z, c.view[2].z)
        + (ndc.x / c.projection.x) * vec3f(c.view[0].x, c.view[1].x, c.view[2].x)
        + (ndc.y / c.projection.y) * vec3f(c.view[0].y, c.view[1].y, c.view[2].y);
    var under = u.eye.xyz + ray * z;
    under.z -= c.water.x;
    if under.z > 0.0 {
        discard;
    }
    let fog = clamp((u.fog.w - z) / (u.fog.w - u.fog.z), 0.0, 1.0);
    // The blend takes the picture of the game into the distant land over the last stretch
    // before the view range (`blend.rs`, drawn before this pass): the caustics go out with it.
    let bound = c.values.y - c.size.z;
    let kept = 1.0 - clamp((blend_distance(depth, at, c.projection.zw, c.projection.xy, c.size.xy, bound) - bound) / c.size.z, 0.0, 1.0);
    let factor = 1.0 + caustic_light(under, u.sun_direction.xyz, c.water.y, z, c.values.x) * fog * kept;
    // The draw `step` multiplies by the part of the factor between 2^step and 2^(step+1); the
    // first one by all of it to 2, a darker pixel too.
    let low = exp2(f32(in.step));
    if in.step > 0u && factor <= low {
        discard;
    }
    let part = select(min(factor, 2.0), clamp(factor / low, 1.0, 2.0), in.step > 0u);
    return vec4f(vec3f(0.5 * part), 1.0);
}
