//! interface: mge-xe-wgsl 1
//! passes: dof, smartblur
//! category: lens
//! texture: MGE/noise64.dds

const fr: f32 = 60.0; // file
const fp: f32 = 60.0; // file
const fpa: f32 = 10.0; // file
const pupil: f32 = 0.006; // file
const blur_radius: f32 = 0.273; // file
const blur_falloff: f32 = 2.0; // file
const R: f32 = 6.0; // file
const k: f32 = 0.00001; // file
const unit2m: f32 = 0.0142;
const M: i32 = 12;

// The blur radius for the width of the picture, the same at each resolution.
fn rfixed() -> f32 {
    return R / (1280.0 * rcpres().x);
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

@fragment fn dof(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let t = 2.0 * tan(radians(0.5 * u.eyepos.w));
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    let fogoffset = saturate(-fogstart / (fogrange - fogstart));
    let s = depth_at(vec2f(0.5, 0.5)) * unit2m;
    let z_corr = length(vec3f((tex.x - 0.5) * t, (tex.y - 0.5) * t / rcpres().y * rcpres().x, 1.0));
    let z = z_corr * unit2m * depth_at(tex);
    let savemyhands = smoothstep(0.568, 0.781, z);
    let fpf = clamp(1.0 / s + fr, fp, fp + fpa);
    var c = pupil * (fr - fpf + 1.0 / z) / fr / k * blur_radius;
    let fog = fogoffset * saturate(z / (4.0 * fogrange * unit2m));
    c = min(abs(c / rfixed()) + fog, 1.0) * savemyhands;
    return vec4f(textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb, c);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let tex = in.tex;
    let rcp = rcpres();
    var color = textureSampleLevel(lastpass, linear_mirror, tex, 0.0);
    let c = color.a * rfixed();
    let rnd = normalize(textureSampleLevel(tex1, point_wrap, tex / rcp / 64.0, 0.0).xy * 2.0 + 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let dir = reflect(taps[i], rnd);
        let s_tex = tex + rcp * dir * c;
        let s_color = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0);
        let s_c = s_color.a * rfixed();
        let weight = exp2(-abs(c - s_c) / blur_falloff);
        color += s_color * weight;
        amount += weight;
    }
    return vec4f(color.rgb / amount, 1.0);
}
