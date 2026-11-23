//! interface: mge-xe-wgsl 1
//! passes: bright, blur_across, blur_down_combine, tone
//! category: none
//! variables: radmult f32 = 1, contrast f32 = 0.5, saturation f32 = 1.5, sun f32 = 0.3, sky f32 = 0, dist1 f32 = 1.5, dist2 f32 = 0.5

fn night() -> f32 {
    return u.switches.y;
}
fn is_interior() -> bool {
    return u.sunamb.w > 0.5;
}
fn fog_range() -> f32 {
    return u.fognearcol.w;
}

// The sky in the depth frame: no surface there (1e38).
const SKY_DEPTH: f32 = 1.0e7;

// The share of the effect that a pixel keeps by its depth, in an exterior: sky() for the sky;
// all of it up to dist1() fog ranges, then less, to none dist2() fog ranges farther.
fn far_share(tex: vec2f) -> f32 {
    if is_interior() {
        return 1.0;
    }
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    if depth > SKY_DEPTH {
        return min(sky(), 1.0);
    }
    let start = fog_range() * dist1();
    return saturate(1.0 - max(depth - start, 0.0) / (fog_range() * dist2()));
}

// The light that blooms: each channel softly from the threshold sun() up to 1. The alpha is
// the hot mask: the brightest pixels only.
@fragment fn bright(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    let glow = smoothstep(vec3f(sun()), vec3f(1.0), color) * far_share(in.tex);
    let hot = smoothstep(0.9, 1.0, pow(length(color), 1.0 / 4.5));
    return vec4f(glow, hot);
}

// The blur: 17 taps 2 texels apart (the middle one, and 8 to each side at 1.5, 3.5 ... 15.5
// texels), weighted by a bell of variance 103 texels squared; the middle tap has half a
// weight. The step is a texel over lerp(1.66, 6, the red of the sun), times radmult(): a
// bright sun makes the glow tighter.
const VARIANCE: f32 = 103.0;
fn weight(x: f32) -> f32 {
    return exp(-x * x / (2.0 * VARIANCE));
}
fn weight_sum() -> f32 {
    var sum = 0.5 * weight(0.0);
    for (var i = 0; i < 8; i++) {
        sum += 2.0 * weight(1.5 + 2.0 * f32(i));
    }
    return sum;
}
fn tap_step(axis: vec2f) -> vec2f {
    let radius = mix(1.66, 6.0, u.suncol.r) / radmult();
    return axis * rcpres() / radius;
}

// Across: each tap is squared after its weight, so that the brightest parts lead. The 8-bit
// target cuts the sum at 1, as the target of Direct3D 9 did.
@fragment fn blur_across(in: Quad) -> @location(0) vec4f {
    let step = tap_step(vec2f(1.0, 0.0));
    let middle = 0.5 * weight(0.0) * textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    var sum = middle * middle;
    for (var i = 0; i < 8; i++) {
        let x = 1.5 + 2.0 * f32(i);
        let w = weight(x);
        let a = w * textureSampleLevel(lastpass, linear_clamp, in.tex + x * step, 0.0);
        let b = w * textureSampleLevel(lastpass, linear_clamp, in.tex - x * step, 0.0);
        sum += a * a + b * b;
    }
    let total = weight_sum();
    return 16.0 * sum / (total * total);
}

// Down, with the squared weights; then onto the picture in gamma 2: the glow added, the red
// halo of the hot mask, a soft curve 2x / (1 + 2x), and back.
@fragment fn blur_down_combine(in: Quad) -> @location(0) vec4f {
    let step = tap_step(vec2f(0.0, 1.0));
    let w0 = 0.5 * weight(0.0);
    var sum = w0 * w0 * textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    for (var i = 0; i < 8; i++) {
        let x = 1.5 + 2.0 * f32(i);
        let w = weight(x);
        let pair = textureSampleLevel(lastpass, linear_clamp, in.tex + x * step, 0.0)
            + textureSampleLevel(lastpass, linear_clamp, in.tex - x * step, 0.0);
        sum += w * w * pair;
    }
    let total = weight_sum();
    let glow = 16.0 * sum / (total * total);
    let picture = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    var color = picture * picture + glow.rgb;
    let halo = 3.0 * glow.rgb * glow.a * vec3f(0.6, 0.0091, 0.0);
    color += saturate3(halo - color);
    color = 2.0 * color / (1.0 + 2.0 * color);
    return vec4f(sqrt(color), 1.0);
}

// The tone: an exposure curve in gamma 2.2, the contrast toward a quintic S curve, and the
// saturation, stronger in bright pixels. The sky and the far land go back to the picture
// before the shader, as in the bright pass.
fn exposure() -> f32 {
    if is_interior() {
        return 1.5;
    }
    return 1.0 + night();
}
@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let lit = pow(textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0).rgb, vec3f(2.2)) * exposure();
    var x = pow(lit / (1.0 + lit), vec3f(1.0 / 2.2));
    let s = x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
    x = mix(x, s, contrast());
    let grey = dot(vec3f(0.2125, 0.7154, 0.0721), x);
    let saturated = mix(vec3f(grey), x, saturation());
    let color = mix(x, saturated, 0.33 * (x.r + x.g + x.b));
    let before = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    return vec4f(mix(before, color, far_share(in.tex)), 1.0);
}
