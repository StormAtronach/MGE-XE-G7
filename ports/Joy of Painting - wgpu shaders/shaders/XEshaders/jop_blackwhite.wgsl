//! interface: mge-xe-wgsl 1
//! passes: blackAndWhite
//! category: final
//! priority_adjust: 450
//! variables: threshold f32 = 0.5, max_threshold f32 = 3

@fragment fn blackAndWhite(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let darkGrayThreshold = (threshold() + 0.0) / max_threshold();
    let average = dot(color.rgb, vec3f(0.299, 0.587, 0.114));
    if average < darkGrayThreshold {
        return vec4f(0.01, 0.01, 0.01, color.a);
    }
    return vec4f(0.99, 0.99, 0.99, color.a);
}
