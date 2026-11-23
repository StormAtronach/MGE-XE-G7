//! interface: mge-xe-wgsl 1
//! passes: shakervib
//! category: none
//! variables: srange f32 = 0, sspeed f32 = 0, zoom f32 = 1, offset f32 = 0, intensity f32 = 0, vibright f32 = 0.5, seed f32 = 5

// Made by Vtastek
// Want to learn shaders ---> https://thebookofshaders.com/

fn rand(co: vec2f) -> f32 {
    return fract(sin(dot(co, vec2f(12.9898, 78.233))) * 43758.5453);
}

@fragment fn shakervib(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let i = floor(time * sspeed() * srange()); // integer
    let f = fract(time * sspeed() * srange()); // fraction
    let iy = floor(time * sspeed() * srange() + seed()); // integer
    let fy = fract(time * sspeed() * srange()); // fraction
    // The file gives `rand` one number: HLSL makes a vector of two from it.
    let randy = mix(rand(vec2f(i)), rand(vec2f(i + 1.0)), smoothstep(0.0, 1.0, f));
    let randx = mix(rand(vec2f(iy)), rand(vec2f(iy + 1.0)), smoothstep(0.0, 1.0, fy));

    let scaleCenter = vec2f(0.5, 0.5);
    let tex = (in.tex - scaleCenter) * zoom() + scaleCenter;
    let stex = tex + (vec2f(randx, randy) - 0.5) * srange() * 0.0015;
    var final_color = textureSampleLevel(lastshader, point_mirror, stex, 0.0);
    let finalleft = textureSampleLevel(lastshader, point_mirror, stex - vec2f(offset(), 0.0), 0.0);
    let finalright = textureSampleLevel(lastshader, point_mirror, stex + vec2f(offset(), 0.0), 0.0);
    final_color = (1.0 + offset() * vibright()) * mix(final_color, mix(finalleft, finalright, 0.5), min(offset() * intensity(), 0.5));
    return final_color;
}
