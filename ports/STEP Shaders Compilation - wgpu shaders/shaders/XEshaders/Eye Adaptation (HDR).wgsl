//! interface: mge-xe-wgsl 1
//! passes: tone
//! category: none
//! mgeflags: 5
//! requires_hdr: true

// HDR Eye Adaptation, tuned for Morrowind lighting
// Simulates exposure adjustment
// Hrnchamd
// v3

const neutral_point: f32 = 0.31; // file
const sensitivity: f32 = 1.4; // file

@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let exposure = (1.0 + sensitivity * (neutral_point - u.hdr.x)) * sqrt(u.hdr.y / u.hdr.x);
    let c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;

    // Increase dynamic range in low light
    var g = exposure * c + (1.0 - exposure) * (c * c * c * c);

    // Film response toe/knee contrast
    // 0.727408x^4 - 2.05477x^3 + 1.63747x^2 + 0.712032x
    g = (((0.727408 * g + -2.05477) * g + 1.63747) * g + 0.712032) * g;

    return vec4f(saturate3(g), 1.0);
}
