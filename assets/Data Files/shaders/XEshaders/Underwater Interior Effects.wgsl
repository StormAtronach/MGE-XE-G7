//! interface: mge-xe-wgsl 1
//! passes: wobble
//! category: scene
//! mgeflags: 22

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

@fragment fn wobble(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let waterlevel = u.suncol.w;
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    let d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let v = u.eyepos.xyz + d * to_world(tex);
    let k = 1.0 - water_at(vec3f(v.xy / 1783.0, 0.4 * time)).b;
    return c * (1.0 - 0.2 * step(v.z, waterlevel - 1.0) * saturate(exp((v.z - waterlevel) / 100.0)) * k);
}
