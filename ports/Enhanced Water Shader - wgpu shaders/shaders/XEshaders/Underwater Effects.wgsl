//! interface: mge-xe-wgsl 1
//! passes: rays, chroma
//! category: none
//! mgeflags: 17

const lightcolour: vec4f = vec4f(0.06, 0.14, 0.08, 0.0);
const chromaRC: f32 = 0.0575; // file
const chromaBY: f32 = 0.0375; // file

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn water_at(t: vec3f) -> vec4f {
    return textureSampleLevel(watertexture, linear_wrap, t, 0.0);
}

fn fetch(tex: vec3f, uw: f32) -> f32 {
    let time = u.sunvec.w;
    let c = water_at(vec3f(tex.xy / 3308.0, 0.22 * time)).b;
    return pow(c, 0.25) * saturate(exp(tex.z / 90.0)) * saturate(1.0 + tex.z / uw);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let sunvis = u.eyevec.w;
    let sunpos = u.sunpos.xyz;
    let eyepos = u.eyepos.xyz;
    // `lf` of the file is one number from a sum of three: HLSL takes the first part. The
    // sky colour of the file is black (see above).
    let lf = (0.2 + 0.8 * sunvis) * (u.sunamb.x * 1.4);

    var wobble = 0.02 * (2.0 * water_at(vec3f(0.25 * in.tex, 0.2 * time)).rg - 1.0);
    // The power 32 of the file, by squares: the base can be negative.
    let x = 2.0 * in.tex - 1.0;
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    wobble *= 1.0 - x16 * x16;
    let tex = in.tex + wobble;

    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    var d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    var v = toWorld(tex);
    let e = vec3f(0.1, 0.1, 1.0) * eyepos;
    let green = vec4f(0.24, 0.52, 0.44, 1.0);

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

    var result = c + rayz * lf * lightcolour;
    if sunvis <= 0.5 {
        result += 0.2 * green;
    } else {
        result += rayz * 0.3;
        result += 0.3 * green;
    }
    return result;
}

@fragment fn chroma(in: Quad) -> @location(0) vec4f {
    let dispa = vec3f(1.0 + chromaRC * 0.07, 1.0 + chromaRC * 0.05, 1.0 + chromaRC * 0.03);
    let dispb = vec3f(1.0 + chromaBY * 0.08, 1.0 + chromaBY * 0.04, 1.0 + chromaBY * 0.02);

    let rCoords = (dispa.r * dispb.r) * in.tex;
    let gCoords = (dispa.g * dispb.g) * in.tex;
    let bCoords = (dispa.b * dispb.b) * in.tex;

    let scene = vec3f(
        textureSampleLevel(lastpass, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastpass, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastpass, linear_clamp, bCoords, 0.0).b);
    return vec4f(scene, 1.0);
}
