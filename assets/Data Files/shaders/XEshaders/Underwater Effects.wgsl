//! interface: mge-xe-wgsl 1
//! passes: rays f16
//! category: scene
//! mgeflags: 17

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}
fn water_at(t: vec3f) -> vec4f {
    return textureSampleLevel(watertexture, linear_wrap, t, 0.0);
}
fn wobbled(tex: vec2f) -> vec2f {
    let time = u.sunvec.w;
    var wobble = 0.01 * (2.0 * water_at(vec3f(0.25 * tex, 0.2 * time)).rg - vec2f(1.0));
    let x = 2.0 * tex - vec2f(1.0);
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    wobble *= vec2f(1.0) - x16 * x16;
    return tex + wobble;
}

const lightcolour: vec4f = vec4f(0.6, 0.91, 1.0, 0.0);

fn fetch(tex: vec3f, uw: f32) -> f32 {
    let time = u.sunvec.w;
    let c = water_at(vec3f(tex.xy / 3308.0, 0.22 * time)).b;
    return pow(c, 0.25) * saturate(exp(tex.z / 90.0)) * saturate(1.0 + tex.z / uw);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let sunvis = u.eyevec.w;
    let suncol = u.suncol.xyz;
    let sunpos = u.sunpos.xyz;
    let eyepos = u.eyepos.xyz;
    let lf = (0.2 + 0.8 * sunvis) * (0.32 * suncol.r + 0.47 * suncol.g + 0.21 * suncol.b);
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    var d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    var v = to_world(tex);
    let e = vec3f(0.1, 0.1, 1.0) * eyepos;

    d = min(d, -eyepos.z / max(1e-5, v.z));
    v = v / length(v.xy);
    let r = -sunpos.xy - v.xy * dot(-sunpos.xy, v.xy);

    var shellpos = e + 800.0 * v;
    shellpos = vec3f(shellpos.xy + 0.8 * r * shellpos.z, shellpos.z);
    var rayz = fetch(shellpos, min(eyepos.z, 400.0)) * saturate(d / 800.0) * 0.294;

    var shellpos2 = e + 1400.0 * v;
    shellpos2 = vec3f(shellpos2.xy + 1.0 * r * shellpos2.z, shellpos2.z);
    rayz += fetch(shellpos2, 2.0 * eyepos.z) * saturate(d / 800.0 - 1.0) * 0.132;

    var shellpos3 = e + 2000.0 * v;
    shellpos3 = vec3f(shellpos3.xy + 1.2 * r * shellpos3.z, shellpos3.z);
    rayz += fetch(shellpos3, 3.0 * eyepos.z) * saturate(d / 800.0 - 2.0) * 0.065;

    // The pass draws into 16-bit floats, so that the soft edge of the rays has no steps. The
    // 8-bit target of the effect file kept 0 to 1: the clamp keeps that range.
    return clamp(c + rayz * lf * lightcolour, vec4f(0.0), vec4f(1.0));
}
