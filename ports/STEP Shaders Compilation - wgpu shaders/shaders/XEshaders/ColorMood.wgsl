//! interface: mge-xe-wgsl 1
//! passes: colorMood
//! category: none
//! mgeflags: 1

// color mood settings
const fRatio: f32 = 0.4; // file
const moodR: f32 = 0.35; // file
const moodG: f32 = 0.3; // file
const moodB: f32 = 0.25; // file

@fragment fn colorMood(in: Quad) -> @location(0) vec4f {
    let colInput = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var colMood = vec4f(moodR, moodG, moodB, 1.0);

    // The sum of the four parts of the pixel, the alpha too, over 3, as the file has it.
    let fLum = dot(colInput, vec4f(1.0)) / 3.0;

    colMood = mix(vec4f(0.0), colMood, saturate(fLum * 2.0));
    colMood = mix(colMood, vec4f(1.0), saturate(fLum - 0.5) * 2.0);

    let colOutput = mix(colInput, colMood, saturate(fLum * fRatio));

    return vec4f(colOutput.rgb, 1.0);
}
