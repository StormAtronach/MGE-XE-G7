//! interface: mge-xe-wgsl 1
//! passes: energy_levels, blur_vert, blur_horz_combine
//! category: sensor

const bloomThreshold: f32 = 0.48; // file
const bloomLevel: f32 = 0.23; // file
const bloomColourSens: f32 = 0.39; // file
const bloomGamma: f32 = 2.45; // file

fn shader_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, point_clamp, t, 0.0);
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn energy_levels(in: Quad) -> @location(0) vec4f {
    var radiance = shader_at(in.tex);
    let depth = textureSampleLevel(depthframe, point_clamp, in.tex, 0.0).r;
    let lum = dot(vec3f(0.27, 0.54, 0.19), radiance.rgb);
    // Mix with luminance to avoid oversaturated single channels.
    radiance = mix(vec4f(lum), radiance, bloomColourSens);
    // Cut sky/distant illumination down.
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    radiance *= 1.0 - 0.45 * saturate(2.0 * (depth - fogstart) / (fogrange - fogstart));
    return clamp(radiance - bloomThreshold, vec4f(0.0), vec4f(1.0));
}

@fragment fn blur_vert(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(0.0, rcpres().y * taps[i]));
    }
    return radiance;
}

@fragment fn blur_horz_combine(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var base = shader_at(in.tex);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(rcpres().x * taps[i], 0.0));
    }
    // Mix in linear space.
    base = pow(max(base, vec4f(0.0)), vec4f(bloomGamma));
    return pow(max(base + bloomLevel * radiance, vec4f(0.0)), vec4f(1.0 / bloomGamma));
}
