//! interface: mge-xe-wgsl 1
//! passes: FilmPass
//! category: none
//! mgeflags: 65

// ---------------------------------------------------------------------------------------
//
//     Film shader (Softlightx2 Version) for video to be used in Avery Lee's Virtualdub
//    GPU Shader filter. v1.6 - 20.00.08 Jukka Korhonen from the original code by Avery Lee.
//
// ---------------------------------------------------------------------------------------

// ------------ TWEAKABLE VARIABLES ------------

const Strenght: f32 = 0.5; // file
const BaseGamma: f32 = 2.2; // file
const Fade: f32 = 0.05; // file
const Contrast: f32 = 1.0; // file
const Saturation: f32 = -0.2; // file
const Bleach: f32 = 0.09; // file
const RedCurve: f32 = 1.0; // file
const GreenCurve: f32 = 1.0; // file
const BlueCurve: f32 = 1.0; // file
const BaseCurve: f32 = 1.0; // file
const EffectGammaR: f32 = 1.0; // file
const EffectGammaG: f32 = 1.0; // file
const EffectGammaB: f32 = 0.75; // file
const EffectGamma: f32 = 1.0; // file
const Linearization: f32 = 2.2; // file

// ------------ END OF TWEAKABLE VARIABLES ------------

// The second row of the matrix XYZ_RGB of the file.
fn GetLuminance(rgb: vec3f) -> f32 {
    return dot(vec3f(0.212671, 0.715160, 0.072169), rgb);
}

// The soft light of the file for one part: `d` selects the curve, `b` is the base.
fn soft(d: f32, b: f32) -> f32 {
    if d < 0.5 {
        return (2.0 * d - 1.0) * (b - b * b) + b;
    }
    return (2.0 * d - 1.0) * (sqrt(b) - b) + b;
}

@fragment fn FilmPass(in: Quad) -> @location(0) vec4f {
    var B = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let H = vec4f(0.01);

    B = pow(abs(B), vec4f(Linearization));
    B = mix(H, B, Contrast);

    let A = GetLuminance(B.rgb);
    var D = vec4f(A);

    B = pow(abs(B), vec4f(1.0 / BaseGamma));

    let a = RedCurve;
    let b = GreenCurve;
    let c = BlueCurve;
    let d = BaseCurve;

    let y = 1.0 / (1.0 + exp(a / 2.0));
    let z = 1.0 / (1.0 + exp(b / 2.0));
    let w = 1.0 / (1.0 + exp(c / 2.0));
    let v = 1.0 / (1.0 + exp(d / 2.0));

    var C = B;

    D.r = (1.0 / (1.0 + exp(-a * (D.r - 0.5))) - y) / (1.0 - 2.0 * y);
    D.g = (1.0 / (1.0 + exp(-b * (D.g - 0.5))) - z) / (1.0 - 2.0 * z);
    D.b = (1.0 / (1.0 + exp(-c * (D.b - 0.5))) - w) / (1.0 - 2.0 * w);

    D = pow(abs(D), vec4f(1.0 / EffectGamma));

    let Di = 1.0 - D;

    D = mix(D, Di, Bleach);

    D.r = pow(abs(D.r), 1.0 / EffectGammaR);
    D.g = pow(abs(D.g), 1.0 / EffectGammaG);
    D.b = pow(abs(D.b), 1.0 / EffectGammaB);

    C.r = soft(D.r, B.r);
    C.g = soft(D.g, B.g);
    C.b = soft(D.b, B.b);

    var F = mix(B, C, Strenght);

    F = (1.0 / (1.0 + exp(-d * (F - 0.5))) - v) / (1.0 - 2.0 * v);

    let r2R = 1.0 - Saturation;
    let g2R = 0.0 + Saturation;
    let b2R = 0.0 + Saturation;

    let r2G = 0.0 + Saturation;
    let g2G = (1.0 - Fade) - Saturation;
    let b2G = (0.0 + Fade) + Saturation;

    let r2B = 0.0 + Saturation;
    let g2B = (0.0 + Fade) + Saturation;
    let b2B = (1.0 - Fade) - Saturation;

    let iF = F;

    F.r = (iF.r * r2R + iF.g * g2R + iF.b * b2R);
    F.g = (iF.r * r2G + iF.g * g2G + iF.b * b2G);
    F.b = (iF.r * r2B + iF.g * g2B + iF.b * b2B);

    let N = GetLuminance(F.rgb);
    var Cn = F;

    if N < 0.5 {
        Cn = (2.0 * N - 1.0) * (F - F * F) + F;
    } else {
        Cn = (2.0 * N - 1.0) * (sqrt(F) - F) + F;
    }

    Cn = pow(abs(Cn), vec4f(1.0 / Linearization));

    let Fn = mix(B, Cn, Strenght);
    return Fn;
}
