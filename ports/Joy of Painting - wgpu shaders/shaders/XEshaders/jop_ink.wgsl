//! interface: mge-xe-wgsl 1
//! passes: main
//! category: final
//! priority_adjust: 70
//! variables: contrast f32 = 1, brightness f32 = 0

@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f((color.rgb + brightness()) * contrast(), color.a);
}
