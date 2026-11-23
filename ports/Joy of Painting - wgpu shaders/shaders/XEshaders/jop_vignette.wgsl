//! interface: mge-xe-wgsl 1
//! passes: vignette
//! category: final

const Age: f32 = -3.0;

@fragment fn vignette(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0);
    let dist = 0.5 - in.tex;
    return vec4f(color.rgb * ((0.4 + -Age / 100.0 - dot(dist, dist)) * 2.8), color.a);
}
