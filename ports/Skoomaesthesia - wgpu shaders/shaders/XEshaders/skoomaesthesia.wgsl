//! interface: mge-xe-wgsl 1
//! passes: lut, blurx, blury
//! category: none
//! texture: x0_psychedelic.tga
//! variables: intensity f32 = 0, cycle f32 = 0, radius f32 = 0

// Skooma Effects

const blurinner: f32 = 0.5; // file

fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

// The file reads the table with tex2Dlod and a level from the coordinate: the table has one
// level, and its sampler has no filter between levels.
fn ClutFunc(colorIN_in: vec3f) -> vec3f {
    var colorIN = colorIN_in;
    let color_offset = vec3f(0.2, 0.4, 0.3);
    colorIN += color_offset * cycle();
    let CLut_pSize = vec2f(0.00390625, 0.0625); // 1 / float2(256, 16);
    var CLut_UV: vec4f;
    colorIN = saturate3(colorIN) * 15.0;
    CLut_UV.w = floor(colorIN.b);
    CLut_UV = vec4f((colorIN.rg + 0.5) * CLut_pSize, CLut_UV.z, CLut_UV.w);
    CLut_UV.x += CLut_UV.w * CLut_pSize.y;
    CLut_UV.z = CLut_UV.x + CLut_pSize.y;
    return mix(sample0(tex1, linear_wrap, CLut_UV.xy).rgb, sample0(tex1, linear_wrap, CLut_UV.zy).rgb, colorIN.b - CLut_UV.w);
}

@fragment fn lut(in: Quad) -> @location(0) vec4f {
    let scene = sample0(lastshader, point_clamp, in.tex);
    let orig = scene.rgb;
    return vec4f(mix(orig, ClutFunc(scene.rgb), intensity()), scene.a);
}

// The 17 taps of the two blurs, along `axis`. The file has them one by one.
fn taps(TexD: vec2f, axis: vec2f, rcprcp: vec2f) -> vec4f {
    var blr = vec4f(0.0);
    blr += 0.026109 * sample0(lastpass, linear_clamp, TexD + axis * -15.5 / rcprcp);
    blr += 0.034202 * sample0(lastpass, linear_clamp, TexD + axis * -13.5 / rcprcp);
    blr += 0.043219 * sample0(lastpass, linear_clamp, TexD + axis * -11.5 / rcprcp);
    blr += 0.052683 * sample0(lastpass, linear_clamp, TexD + axis * -9.5 / rcprcp);
    blr += 0.061948 * sample0(lastpass, linear_clamp, TexD + axis * -7.5 / rcprcp);
    blr += 0.070266 * sample0(lastpass, linear_clamp, TexD + axis * -5.5 / rcprcp);
    blr += 0.076883 * sample0(lastpass, linear_clamp, TexD + axis * -3.5 / rcprcp);
    blr += 0.081149 * sample0(lastpass, linear_clamp, TexD + axis * -1.5 / rcprcp);
    blr += 0.041312 * sample0(lastpass, linear_clamp, TexD);
    blr += 0.081149 * sample0(lastpass, linear_clamp, TexD + axis * 1.5 / rcprcp);
    blr += 0.076883 * sample0(lastpass, linear_clamp, TexD + axis * 3.5 / rcprcp);
    blr += 0.070266 * sample0(lastpass, linear_clamp, TexD + axis * 5.5 / rcprcp);
    blr += 0.061948 * sample0(lastpass, linear_clamp, TexD + axis * 7.5 / rcprcp);
    blr += 0.052683 * sample0(lastpass, linear_clamp, TexD + axis * 9.5 / rcprcp);
    blr += 0.043219 * sample0(lastpass, linear_clamp, TexD + axis * 11.5 / rcprcp);
    blr += 0.034202 * sample0(lastpass, linear_clamp, TexD + axis * 13.5 / rcprcp);
    blr += 0.026109 * sample0(lastpass, linear_clamp, TexD + axis * 15.5 / rcprcp);
    return blr / 0.93423; // renormalize to compensate for the 4 taps I skipped
}

// One blur: `inner` is where the blur of the edges starts.
fn blur(TexD: vec2f, axis: vec2f, inner: f32) -> vec4f {
    let p = TexD * (1.0 - TexD);
    var vig = saturate(p.x * p.y * 15.0);
    vig = smoothstep(inner, 1.0, 1.0 - vig);
    vig = radius() * vig;
    var blr = vec4f(0.0);
    // OPTI of the file: no taps where there is no blur.
    if vig > 0.0 {
        blr = taps(TexD, axis, 1.0 / rcpres() / vig);
    }
    let pcol = sample0(lastpass, linear_clamp, TexD);
    let col = mix(blr, pcol, 1.0 - vig);
    return vec4f(col.rgb, 1.0);
}

@fragment fn blurx(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(1.0, 0.0), 0.70);
}

@fragment fn blury(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(0.0, 1.0), blurinner);
}
