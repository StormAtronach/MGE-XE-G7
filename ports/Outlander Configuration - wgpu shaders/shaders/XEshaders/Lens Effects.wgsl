//! interface: mge-xe-wgsl 1
//! passes: barrelvig
//! category: none

// Special Contrast
// vtastek
// v1

//////////////
// Settings //
//////////////

const dscale: f32 = 1.08; // file
const chromaticDispersionRC: f32 = 0.045; // file
const chromaticDispersionBY: f32 = 0.045; // file
const distortion: f32 = 0.01; // file
const cubicDistortion: f32 = 0.031; // file

@fragment fn barrelvig(in: Quad) -> @location(0) vec4f {
    let ratio = rcpres().x / rcpres().y;
    let scaleFactor = 1.0 / dscale;

    //index of refraction of each color channel, causing chromatic dispersion
    let eta = vec3f(1.0 + chromaticDispersionRC * 0.09, 1.0 + chromaticDispersionRC * 0.06, 1.0 + chromaticDispersionRC * 0.03);
    let eta2 = vec3f(1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.03);

    let tex = 2.0 * (in.tex - 0.5);

    // lens distortion coefficient
    let k = distortion;
    // cubic distortion value
    let kcube = cubicDistortion;

    //APPLY DISTORTION
    let r2 = ratio * ratio * tex.x * tex.x + tex.y * tex.y;
    //compute the cubic distortion
    let f = 1.0 + r2 * (k + kcube * sqrt(r2));

    //Apply CA
    let rCoords = (f * eta.r * eta2.r) * scaleFactor * (tex * 0.5) + 0.5;
    let gCoords = (f * eta.g * eta2.g) * scaleFactor * (tex * 0.5) + 0.5;
    let bCoords = (f * eta.b * eta2.b) * scaleFactor * (tex * 0.5) + 0.5;

    let scene = vec3f(
        textureSampleLevel(lastshader, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastshader, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastshader, linear_clamp, bCoords, 0.0).b);
    return vec4f(scene, 1.0);
}
