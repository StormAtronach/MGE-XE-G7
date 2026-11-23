//! part: blur

struct Uniforms {
    // The offset of a tap in uv, and the axis.
    step: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    var out: Out;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}

// shadowSoften: five taps along one axis, a mean with weights 0.2, 0.8, 1, 0.8, 0.2.
@fragment fn fs(in: Out) -> @location(0) vec4f {
    let off = u.step.xy;
    var d = textureSample(source, source_sampler, in.uv).r;
    d += 0.2 * textureSample(source, source_sampler, in.uv - 1.42 * off).r;
    d += 0.8 * textureSample(source, source_sampler, in.uv - 0.71 * off).r;
    d += 0.8 * textureSample(source, source_sampler, in.uv + 0.71 * off).r;
    d += 0.2 * textureSample(source, source_sampler, in.uv + 1.42 * off).r;
    return vec4f(d / 3.0, 0.0, 0.0, 1.0);
}
//! part: land

struct Out {
    @builtin(position) position: vec4f,
    @location(0) depth: f32,
}

@vertex fn vs(@location(0) position: vec3f, @location(1) normal: vec4f, @location(2) color: vec4f) -> Out {
    var out: Out;
    var clip = clip_of(position);
    clip.z = max(0.0, clip.z);
    out.position = clip;
    out.depth = clip.z / clip.w;
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    return vec4f(32768.0 * in.depth, 0.0, 0.0, 1.0);
}
//! part: statics


struct In {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
    @location(7) world3: vec4f,
    @location(8) subset: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) coordinate: vec2f,
    @location(1) depth: f32,
    @location(2) @interpolate(flat) bounds: vec4f,
    @location(3) @interpolate(flat) alpha_test: f32,
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    var clip = clip_of(world);
    // What is nearer to the sun than the near plane stays, at the near plane.
    clip.z = max(0.0, clip.z);
    out.position = clip;
    out.depth = clip.z / clip.w;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * set_values.handoff.x);
    }
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    if in.alpha_test > 0.5 {
        let scale = in.bounds.yw - in.bounds.zx;
        let atlas = in.bounds.zx + fract(in.coordinate) * scale;
        let texel = textureSampleGrad(base_texture, base_sampler, atlas, dpdx(in.coordinate) * scale, dpdy(in.coordinate) * scale);
        if texel.a < 180.0 / 255.0 {
            discard;
        }
    }
    return vec4f(32768.0 * in.depth, 0.0, 0.0, 1.0);
}
