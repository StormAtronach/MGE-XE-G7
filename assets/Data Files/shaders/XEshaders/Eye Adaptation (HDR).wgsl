//! interface: mge-xe-wgsl 1
//! passes: tone
//! category: tone
//! requires_hdr: true

const neutral_point: f32 = 0.335; // file
const sensitivity: f32 = 0.11; // file
const max_exposure: f32 = 3.2; // file

@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let scotopic = 0.5 * saturate(exposure / max_exposure - 0.66666);
    let c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;
    // More dynamic range in low light.
    var g = exposure * c + (1.0 - exposure) * (c * c * c * c);
    // The toe and the knee of a film response.
    g = (((0.727408 * g + vec3f(-2.05477)) * g + vec3f(1.63747)) * g + vec3f(0.712032)) * g;
    // Less colour when the exposure is high.
    g = mix(g, vec3f(dot(g, vec3f(0.33333))), scotopic);
    return vec4f(saturate3(g), 1.0);
}
