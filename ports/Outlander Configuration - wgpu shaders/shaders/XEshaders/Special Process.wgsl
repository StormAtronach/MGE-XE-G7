//! interface: mge-xe-wgsl 1
//! passes: brightpass, blurx, blury, specialcontrast
//! category: none
//! requires_hdr: true

// Special Contrast
// vtastek
// v1

// Compatibility: MGE XE 0, fully working

//////////////
// Settings //
//////////////

const neutral_point: f32 = 0.25; // file
const sensitivity: f32 = 0.33; // file
const max_exposure: f32 = 1.6; // file

// The file computes `exposure` and `radius` as static globals from `HDR`. WGSL has no such
// globals: `blurx` and `blury` compute them at their start.
//float radius = 1.66;

const contrast: f32 = 0.3; // file
const saturation: f32 = 1.0; // file

// `s0` of the file is `lastshader` with `point_clamp`; `s1` is `lastpass` with `linear_clamp`.
fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

@fragment fn brightpass(in: Quad) -> @location(0) vec4f {
    let col = sample0(lastshader, point_clamp, in.tex).rgb;

    var lum = dot(col, vec3f(0.33, 0.33, 0.33));

    //return float4(col, 1);
    let sun = mix(0.75, 0.5, u.suncol.x);
    lum = smoothstep(sun - 0.05, sun, lum);

    //float3 fringe = abs(fwidth(final));
    //fringe = saturate(fringe - 0.1);

    //fringe = final +  final * float3(0.2,0.04, 0.0);
    let halcol = smoothstep(0.9, 1.0, pow(length(col), 1.0 / 4.5));
    //final += fringe.rgb;
    return vec4f(pow(col * lum, vec3f(1.5)), halcol);
}

@fragment fn blurx(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let radius = mix(0.1, 1.6, saturate(exposure / max_exposure) - 0.5);

    var rcprcp = 1.0 / rcpres();
    rcprcp *= radius;

    var blr = vec4f(0.0);
    blr += pow(0.026109 * sample0(lastpass, linear_clamp, in.tex + vec2f(-15.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.034202 * sample0(lastpass, linear_clamp, in.tex + vec2f(-13.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.043219 * sample0(lastpass, linear_clamp, in.tex + vec2f(-11.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.052683 * sample0(lastpass, linear_clamp, in.tex + vec2f(-9.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.061948 * sample0(lastpass, linear_clamp, in.tex + vec2f(-7.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.070266 * sample0(lastpass, linear_clamp, in.tex + vec2f(-5.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.076883 * sample0(lastpass, linear_clamp, in.tex + vec2f(-3.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.081149 * sample0(lastpass, linear_clamp, in.tex + vec2f(-1.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.041312 * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.081149 * sample0(lastpass, linear_clamp, in.tex + vec2f(1.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.076883 * sample0(lastpass, linear_clamp, in.tex + vec2f(3.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.070266 * sample0(lastpass, linear_clamp, in.tex + vec2f(5.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.061948 * sample0(lastpass, linear_clamp, in.tex + vec2f(7.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.052683 * sample0(lastpass, linear_clamp, in.tex + vec2f(9.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.043219 * sample0(lastpass, linear_clamp, in.tex + vec2f(11.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.034202 * sample0(lastpass, linear_clamp, in.tex + vec2f(13.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.026109 * sample0(lastpass, linear_clamp, in.tex + vec2f(15.5, 0.0) / rcprcp), vec4f(2.0));
    //blr += 0.019227*samplePremul( s0, (TexD+float2( 17.5,0.0)/rcprcp.xy) ).xyz;
    //blr += 0.013658*samplePremul( s0, (TexD+float2( 19.5,0.0)/rcprcp.xy) ).xyz;

    blr /= pow(0.93423, 2.0); // renormalize to compensate for the 4 taps I skipped

    return blr * 16.0;
}

@fragment fn blury(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let radius = mix(0.1, 1.6, saturate(exposure / max_exposure) - 0.5);

    var rcprcp = 1.0 / rcpres();

    rcprcp *= radius;
    var blr = vec4f(0.0);
    //blr += 0.013658*sample0( s1, (TexD+float2(0.0,-19.5)/rcprcp.xy) ).xyz;
    //blr += 0.019227*sample0( s1, (TexD+float2(0.0,-17.5)/rcprcp.xy) ).xyz;
    blr += pow(0.026109, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -15.5) / rcprcp);
    blr += pow(0.034202, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -13.5) / rcprcp);
    blr += pow(0.043219, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -11.5) / rcprcp);
    blr += pow(0.052683, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -9.5) / rcprcp);
    blr += pow(0.061948, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -7.5) / rcprcp);
    blr += pow(0.070266, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -5.5) / rcprcp);
    blr += pow(0.076883, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -3.5) / rcprcp);
    blr += pow(0.081149, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -1.5) / rcprcp);
    blr += pow(0.041312, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 0.0) / rcprcp);
    blr += pow(0.081149, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 1.5) / rcprcp);
    blr += pow(0.076883, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 3.5) / rcprcp);
    blr += pow(0.070266, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 5.5) / rcprcp);
    blr += pow(0.061948, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 7.5) / rcprcp);
    blr += pow(0.052683, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 9.5) / rcprcp);
    blr += pow(0.043219, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 11.5) / rcprcp);
    blr += pow(0.034202, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 13.5) / rcprcp);
    blr += pow(0.026109, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 15.5) / rcprcp);
    //blr += 0.019227*sample0( s1, (TexD+float2(0.0, 17.5)/rcprcp.xy) ).xyz;
    //blr += 0.013658*sample0( s1, (TexD+float2(0.0, 19.5)/rcprcp.xy) ).xyz;

    blr /= pow(0.93423, 2.0); // renormalize to compensate for the 4 taps I skipped
    blr *= 16.0;
    let org = sample0(lastshader, point_clamp, in.tex).rgb;

    var col = pow(org, vec3f(2.0));
    blr *= mix(0.0, 1.0, saturate(exposure / max_exposure) - 0.5);
    blr = clamp(blr, vec4f(0.0), vec4f(1.0));
    // The file adds a float4 to a float3: HLSL drops the alpha part.
    col = col + blr.rgb - col * blr.rgb;

    let halcol = blr.rgb * blr.a * 0.1 * vec3f(0.6, 0.0091, 0.0);
    col += saturate3(halcol * 30.0 - col);
    //col += halcol;

    //col = col / (1.0 + col);
    col = pow(col, vec3f(1.0 / 2.0));

    return vec4f(col, 1.0);
}

@fragment fn specialcontrast(in: Quad) -> @location(0) vec4f {
    /*float3 mag = sample0(s1, Tex * 0.1);
    return float4(mag, 1);
    */

    var x = sample0(lastpass, linear_clamp, in.tex);
    let contrasted = (x * x * x * (x * (x * 6.0 - 15.0) + 10.0)).rgb;
    // WGSL does not assign to `x.rgb`: the whole vector is built again.
    x = vec4f(mix(x.rgb, contrasted, contrast), x.a);
    let sat = mix(vec3f(dot(vec3f(0.2125, 0.7154, 0.0721), x.rgb)), x.rgb, saturation);

    var col = x.rgb;

    col = mix(x.rgb, sat, 0.33 * (x.r + x.g + x.b));

    return vec4f(col, 1.0);
}
