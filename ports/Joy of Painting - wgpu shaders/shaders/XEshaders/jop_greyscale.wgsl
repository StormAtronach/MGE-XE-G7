//! interface: mge-xe-wgsl 1
//! passes: greyscale
//! category: final
//! priority_adjust: 450

@fragment fn greyscale(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let average = dot(color.rgb, vec3f(0.299, 0.587, 0.114));
    return vec4f(average, average, average, color.a);
}
