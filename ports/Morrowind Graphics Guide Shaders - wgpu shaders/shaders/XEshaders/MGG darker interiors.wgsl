//! interface: mge-xe-wgsl 1
//! passes: gammatoo
//! category: none
//! mgeflags: 6

//tweakables
const gamma: f32 = 1.1; // file
const saturation: f32 = 1.1; // file
const luminance: f32 = 0.9; // file
//tweakables

fn grayscale(col: vec3f) -> vec3f {
    return vec3f(dot(col.rgb, vec3f(0.3, 0.59, 0.11)));
}

@fragment fn gammatoo(in: Quad) -> @location(0) vec4f {
    var c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;
    c = mix(grayscale(c), c, saturation); // Apply saturation
    // The saturation can make a part of `c` negative. `pow` of a negative base has no
    // defined result in WGSL: the base is 0 there.
    c = pow(max(c, vec3f(0.0)), vec3f(gamma)); // Apply gamma

    return vec4f(saturate3(c * luminance), 1.0);
}
