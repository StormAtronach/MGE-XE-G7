//! interface: mge-xe-wgsl 1
//! passes: ThreshDownScale, MyBloomH, MyBloomV, MyBloomH, MyBloomV, gamma, barrelvig
//! category: none
//! requires_hdr: true
//! variables: exteriorbloom f32 = 2, interiorbloom f32 = 3

const scale: f32 = 4.0; // file
const BloomBoost: f32 = 0.07; // file
const thre: f32 = 0.1; // file
const intenb: f32 = 1.03; // file
const bsize: f32 = 1.0; // file
const bsize2: f32 = 1.0; // file
const dscale: f32 = 1.05; // file
const chromaticDispersionRC: f32 = 0.02; // file
const chromaticDispersionBY: f32 = 0.01; // file
const distortion: f32 = 0.0001; // file
const cubicDistortion: f32 = 0.0001; // file

fn midlevel() -> vec3f {
    return pow(sqrt(1.0 + vec3f(0.05, 0.05, 0.05)) - vec3f(0.1, 0.1, 0.1), vec3f(2.2));
}

fn Uncharted2Tonemap(x: vec3f) -> vec3f {
    let A = 0.25;
    let B = 0.50;
    let C = 0.10;
    let D = 0.20;
    let E = 0.02;
    let F = 0.30;
    return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F;
}

fn screen(org: vec3f, bloom: vec3f) -> vec3f {
    return 1.0 - (1.0 - org) * (1.0 - bloom);
}

fn bright(t: vec2f) -> vec3f {
    return pow(textureSampleLevel(lastshader, point_clamp, t, 0.0).rgb * intenb, vec3f(2.2)) - midlevel() * thre;
}

// The bright part of the picture, at a quarter of its size in the corner of the target.
@fragment fn ThreshDownScale(in: Quad) -> @location(0) vec4f {
    let Tex = in.tex;
    var ColorNew = vec3f(0.0);
    if !(Tex.x * scale > 1.05 || Tex.y * scale > 1.05) {
        var blurPixs2 = array<vec2f, 4>(vec2f(1.5, 0.5), vec2f(0.5, -1.5), vec2f(-0.5, 1.5), vec2f(-1.5, -0.5));
        ColorNew = bright(Tex * scale);
        for (var i = 0; i < 4; i++) {
            ColorNew += bright(Tex * scale + blurPixs2[i] * bsize * rcpres());
        }
    }
    return vec4f(ColorNew / 5.0, 1.0);
}

fn bloom(Tex: vec2f, axis: vec2f) -> vec4f {
    var ColorNew = vec3f(0.0);
    if !(Tex.x * scale > 1.05 || Tex.y * scale > 1.05) {
        var blurPixs = array<f32, 7>(-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0);
        var blurMags = array<f32, 7>(0.05, 0.14, 0.23, 0.32, 0.23, 0.14, 0.05);
        for (var i = 0; i < 7; i++) {
            ColorNew += textureSampleLevel(lastpass, linear_clamp, Tex + axis * blurPixs[i] * rcpres() * bsize2, 0.0).rgb * blurMags[i];
        }
    }
    return vec4f(ColorNew, 1.0);
}

@fragment fn MyBloomH(in: Quad) -> @location(0) vec4f {
    return bloom(in.tex, vec2f(1.0, 0.0));
}

@fragment fn MyBloomV(in: Quad) -> @location(0) vec4f {
    return bloom(in.tex, vec2f(0.0, 1.0));
}

@fragment fn gamma(in: Quad) -> @location(0) vec4f {
    let W = 11.2;
    var texColor = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    let bloomc = textureSampleLevel(lastpass, linear_clamp, in.tex / scale, 0.0).rgb;
    texColor = screen(pow(texColor, vec3f(2.2)), bloomc * BloomBoost);
    var multip = 0.0;
    if u.sunamb.w == 0.0 {
        multip = 1.0 - 0.5 * saturate(1.0 - u.sunpos.z * 1.4);
    }
    multip = saturate(multip + u.hdr.x - 0.2);
    texColor *= mix(interiorbloom(), exteriorbloom(), multip); // Hardcoded Exposure Adjustment
    let ExposureBias = 2.0;
    let curr = Uncharted2Tonemap(ExposureBias * texColor);
    let whiteScale = 1.0 / Uncharted2Tonemap(vec3f(W));
    return vec4f(pow(max(curr * whiteScale, vec3f(0.0)), vec3f(1.0 / 2.2)), 1.0);
}

@fragment fn barrelvig(in: Quad) -> @location(0) vec4f {
    let TexD = in.tex;
    let ratio = rcpres().x / rcpres().y;
    let scaleFactor = 1.0 / dscale;
    // index of refraction of each color channel, causing chromatic dispersion
    let eta = vec3f(1.0 + chromaticDispersionRC * 0.09, 1.0 + chromaticDispersionRC * 0.06, 1.0 + chromaticDispersionRC * 0.03);
    let eta2 = vec3f(1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.03);
    let tex = 2.0 * (TexD - 0.5);
    var uv = TexD;
    uv *= 1.0 - uv.yx;
    let r2 = ratio * ratio * tex.x * tex.x + tex.y * tex.y;
    // compute the cubic distortion
    let f = 1.0 + r2 * (distortion + cubicDistortion * sqrt(r2));
    // Apply CA
    let rCoords = (f * eta.r * eta2.r) * scaleFactor * (tex * 0.5) + 0.5;
    let gCoords = (f * eta.g * eta2.g) * scaleFactor * (tex * 0.5) + 0.5;
    let bCoords = (f * eta.b * eta2.b) * scaleFactor * (tex * 0.5) + 0.5;
    var scene = vec3f(
        textureSampleLevel(lastpass, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastpass, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastpass, linear_clamp, bCoords, 0.0).b);
    scene *= 0.2 + 0.8 * pow(16.0 * uv.x * uv.y * (1.0 - uv.x) * (1.0 - uv.y), 0.1);
    return vec4f(scene, 1.0);
}
