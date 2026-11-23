//! part: set

struct Set {
    // x: the view range of the game in this set (0 for a set without the handoff).
    range: vec4f,
    // x: the time. y: the distance where a static is complete. z: the width of the band
    // before it. w: the distance before the band where a vertex goes away.
    handoff: vec4f,
}
@group(1) @binding(0) var<uniform> set_values: Set;
@group(1) @binding(1) var<storage, read> palettes: array<vec4f>;
@group(1) @binding(2) var base_sampler: sampler;
@group(2) @binding(0) var base_texture: texture_2d<f32>;

// The view range of the game, for the fog (`fog_of`).
fn fog_range() -> f32 {
    return set_values.range.x;
}
//! part: statics

// The order of the pixels of a block of 4 x 4 for the band of the handoff (Bayer).
const HANDOFF_ORDER = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);

struct In {
    // The position of a vertex, and the number of its palette entry.
    @location(0) position: vec4f,
    // The normal, and the light that the material gives off.
    @location(1) normal: vec4f,
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    // The world matrix of the place, row after row.
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
    @location(7) world3: vec4f,
    // x: the first palette entry of the subset. y: 1 for an alpha test. z: 1 for a texture
    // that moves. w: the number of the subset.
    @location(8) subset: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
    @location(1) coordinate: vec2f,
    @location(2) world: vec3f,
    @location(3) @interpolate(flat) bounds: vec4f,
    @location(4) @interpolate(flat) alpha_test: f32,
    // The colour of the vertex and the normal in the world, for the light of the distant
    // lamps in the pixel shader.
    @location(6) vertex_color: vec3f,
    @location(7) normal: vec3f,
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    // A position is a row vector in Direct3D: position * world.
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    out.position = clip_of(world);
    // Near the camera the game draws the real static. A vertex before the band goes away.
    if length(world - u.eye.xyz) < set_values.handoff.y - set_values.handoff.z - set_values.handoff.w {
        out.position = vec4f(0.0, 0.0, -10000.0, 0.0);
    }
    // lightStaticVert: the normal goes through the world matrix and is not made of length 1
    // again, as in the shader of MGE XE.
    let n = normalize(in.normal.xyz * 2.0 - 1.0);
    let normal = n.x * in.world0.xyz + n.y * in.world1.xyz + n.z * in.world2.xyz;
    let light = u.sun_color.rgb * clamp(dot(normal, -u.sun_direction.xyz), 0.0, 1.0) + u.ambient.rgb + in.normal.w;
    // The colour of a vertex has the bytes blue, green, red.
    out.color = in.color.bgr * light;
    out.vertex_color = in.color.bgr;
    out.normal = normal;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * set_values.handoff.x);
    }
    out.world = world;
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    return out;
}

// True for the pipelines of the frame with multisampling and alpha to coverage (a pipeline
// constant); false for the mirror of the water and with one sample.
override ALPHA_TO_COVERAGE: bool = false;

@fragment fn fs(in: Out) -> @location(0) vec4f {
    // The palette entry is min v, max u, min u, max v of a rectangle of the atlas page.
    let scale = in.bounds.yw - in.bounds.zx;
    let dx = dpdx(in.coordinate) * scale;
    let dy = dpdy(in.coordinate) * scale;
    let atlas = in.bounds.zx + fract(in.coordinate) * scale;
    let texel = textureSampleGrad(base_texture, base_sampler, atlas, dx, dy);
    // In a reflection, only one side of the water plane: flags.y is the side, flags.z the plane.
    if u.flags.y != 0.0 && (in.world.z - u.flags.z) * u.flags.y < 0.0 {
        discard;
    }
    let range = length(in.world - u.eye.xyz);
    // The handoff to the game.
    let dissolve = clamp((range - (set_values.handoff.y - set_values.handoff.z)) / set_values.handoff.z, 0.0, 1.0);
    // Across the band a static comes in by its share of covered pixels: Direct3D 9 gives the
    // dissolve to alpha to coverage, which needs the samples of multisampling; here an
    // ordered threshold of 4 x 4 pixels, the same with one sample and with more (the alpha
    // to coverage of the frame's pipeline takes the alpha of the texel only).
    let cell = vec2u(in.position.xy) % 4u;
    let threshold = (HANDOFF_ORDER[cell.y * 4u + cell.x] + 0.5) / 16.0;
    if dissolve < max(threshold, 1.0 / 255.0) {
        discard;
    }
    // The alpha test of StaticPS (AlphaRef 133). With alpha to coverage the alpha test is
    // off, as DXVK turns it off under the vendor switch of alpha to coverage, and the alpha
    // is calc_coverage(a, 133 / 255, 2) of `XE Common.fx`: the samples of the pixel that
    // the static covers.
    var alpha = 1.0;
    if in.alpha_test > 0.5 {
        if ALPHA_TO_COVERAGE {
            alpha = clamp(2.0 * (texel.a - 133.0 / 255.0) + 133.0 / 255.0, 0.0, 1.0);
        } else if texel.a < 133.0 / 255.0 {
            discard;
        }
    }
    // In the mirror under water (flags.y is -1) the statics have no scattering: the interior
    // pass of `renderWaterReflection` fogs them with the linear fog of the game.
    let lamps = lamp_sums(in.world, normalize(in.normal));
    let lit = in.color + in.vertex_color * lamps.surface;
    if lamp_values.values.w > 1.5 {
        return vec4f(texel.rgb * in.vertex_color * lamp_light(in.world, normalize(in.normal)), alpha);
    }
    if lamp_values.values.w > 0.5 {
        return vec4f(lamp_light(in.world, normalize(in.normal)), alpha);
    }
    return vec4f(fog_with(texel.rgb * lit, in.world, u.flags.y >= 0.0, lamps.air), alpha);
}
