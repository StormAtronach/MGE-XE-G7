//! The ports of the post shaders of the mod Joy of Painting: 23 effect files, which the mod
//! loads and sets from Lua (its `ShaderService`). Most of them include `jop_common.fx`, and
//! four include `jop_gaussian.fx`; the two macros below are the ports of those two files.
//!
//! A sampler of a file that names no filter and no address reads the picture at the centre
//! of a pixel in each of these files: the port reads with `point_clamp` there.

use super::post::{Pass, Shader, Variable, full};

/// A variable with one place: a value, or a vector of the length of `default`.
const fn v(name: &'static str, default: &'static [f32]) -> Variable {
    Variable { name, slots: 1, per: default.len() as u32, default }
}

/// The two variables of `jop_common.fx`: each shader that includes the file has them.
const ASPECT: Variable = v("aspectRatio", &[1.3]);
const VIEWPORT: Variable = v("viewportSize", &[0.8]);

/// The place of the category `final`, plus the `priorityAdjust` of the file.
const FINAL: i64 = 9_000_000;

const fn shader(name: &'static str, source: &'static str, passes: &'static [Pass], adjust: i64, textures: &'static [&'static str], variables: &'static [Variable]) -> Shader {
    Shader { name, source, passes, disable_flags: 0, no_sun_glare: false, priority: FINAL + adjust, textures, requires_hdr: false, variables }
}

/// `jop_common.fx`. The names of the functions are those of the file.
macro_rules! jop_common {
    () => {
        "
const PI: f32 = 3.14159265358979;
const SKY: f32 = 1e6;

fn Time() -> f32 {
    return u.sunvec.w;
}
fn invproj() -> vec2f {
    return 2.0 * tan(0.5 * radians(u.eyepos.w)) * vec2f(1.0, rcpres().x / rcpres().y);
}
fn xylength() -> f32 {
    return sqrt(1.0 - u.eyevec.z * u.eyevec.z);
}
fn saturate4(x: vec4f) -> vec4f {
    return clamp(x, vec4f(0.0), vec4f(1.0));
}
fn sample0(t: texture_2d<f32>, s: sampler, tex: vec2f) -> vec4f {
    return textureSampleLevel(t, s, tex, 0.0);
}

// Moves the coordinates by the red of a noise texture that drifts with the time.
fn distort(tex: vec2f, amount: f32, map: texture_2d<f32>, offset: f32) -> vec2f {
    let thisTime = Time() + offset;
    let uv = vec2f(tex.x + sin(thisTime * 0.5) * 0.1, tex.y + cos(thisTime * 0.5) * 0.1) * 0.5;
    let distortion = sample0(map, linear_wrap, uv) * 2.0 - 1.0;
    return tex + distortion.x * amount * 0.2;
}

fn readDepth(tex: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
}

fn RGBToHSL(color: vec3f) -> vec3f {
    var hsl = vec3f(0.0);
    let fmin = min(min(color.r, color.g), color.b);
    let fmax = max(max(color.r, color.g), color.b);
    let delta = fmax - fmin;
    hsl.z = (fmax + fmin) / 2.0;
    if delta != 0.0 {
        if hsl.z < 0.5 {
            hsl.y = delta / (fmax + fmin);
        } else {
            hsl.y = delta / (2.0 - fmax - fmin);
        }
        let deltaR = (((fmax - color.r) / 6.0) + (delta / 2.0)) / delta;
        let deltaG = (((fmax - color.g) / 6.0) + (delta / 2.0)) / delta;
        let deltaB = (((fmax - color.b) / 6.0) + (delta / 2.0)) / delta;
        if color.r == fmax {
            hsl.x = deltaB - deltaG;
        } else if color.g == fmax {
            hsl.x = (1.0 / 3.0) + deltaR - deltaB;
        } else if color.b == fmax {
            hsl.x = (2.0 / 3.0) + deltaG - deltaR;
        }
        if hsl.x < 0.0 {
            hsl.x += 1.0;
        } else if hsl.x > 1.0 {
            hsl.x -= 1.0;
        }
    }
    return hsl;
}

fn HueToRGB(f1: f32, f2: f32, h_in: f32) -> f32 {
    var h = h_in;
    if h < 0.0 {
        h += 1.0;
    } else if h > 1.0 {
        h -= 1.0;
    }
    if (6.0 * h) < 1.0 {
        return f1 + (f2 - f1) * 6.0 * h;
    } else if (2.0 * h) < 1.0 {
        return f2;
    } else if (3.0 * h) < 2.0 {
        return f1 + (f2 - f1) * ((2.0 / 3.0) - h) * 6.0;
    }
    return f1;
}

fn HSLToRGB(hsl: vec3f) -> vec3f {
    if hsl.y == 0.0 {
        return vec3f(hsl.z);
    }
    var f2 = (hsl.z + hsl.y) - (hsl.y * hsl.z);
    if hsl.z < 0.5 {
        f2 = hsl.z * (1.0 + hsl.y);
    }
    let f1 = 2.0 * hsl.z - f2;
    return vec3f(HueToRGB(f1, f2, hsl.x + (1.0 / 3.0)), HueToRGB(f1, f2, hsl.x), HueToRGB(f1, f2, hsl.x - (1.0 / 3.0)));
}

fn toWorld(tex: vec2f) -> vec3f {
    var w = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    w += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    w += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return w;
}

fn toView(tex: vec2f) -> vec3f {
    let depth = readDepth(tex);
    return vec3f(depth * (tex - 0.5) * invproj(), depth);
}

fn getPosition(tex: vec2f, depth: f32) -> vec3f {
    return u.eyepos.xyz + toWorld(tex) * depth;
}

fn toWorldWithDepth(uv: vec2f, depth: f32) -> vec3f {
    let clip = vec2f(2.0 * uv.x - 1.0, 1.0 - 2.0 * uv.y);
    var wpos = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    let invProjX = 1.0 / mproj(0, 0);
    let invProjY = 1.0 / mproj(1, 1);
    wpos += (clip.x * invProjX) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    wpos += (clip.y * -invProjY) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return wpos * depth;
}

fn getWorldSpaceNormal(uv: vec2f) -> vec3f {
    let pos = toView(uv);
    let water = pos.z * u.eyevec.z - pos.y * xylength() + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > SKY || (water - u.suncol.w) < 0.0 {
        return vec3f(0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let uvR = uv + vec2f(rcp.x, 0.0);
    let uvL = uv - vec2f(rcp.x, 0.0);
    let posR = toWorldWithDepth(uvR, readDepth(uvR));
    let posL = toWorldWithDepth(uvL, readDepth(uvL));
    let uvD = uv + vec2f(0.0, rcp.y);
    let uvU = uv - vec2f(0.0, rcp.y);
    let posD = toWorldWithDepth(uvD, readDepth(uvD));
    let posU = toWorldWithDepth(uvU, readDepth(uvU));
    return normalize(cross(posR - posL, posD - posU));
}

// The matrix of the file changes the y and the z of the normal: the angle is that of its y.
fn rotateUvByNormal(uv: vec2f, normal: vec3f, rotationAngle: f32) -> vec2f {
    let angle = acos(clamp(normal.y, -1.0, 1.0));
    let c = cos(angle);
    let s = sin(angle);
    let turned = vec2f(c * uv.x - s * uv.y, s * uv.x + c * uv.y);
    return vec2f(cos(rotationAngle) * turned.x - sin(rotationAngle) * turned.y, sin(rotationAngle) * turned.x + cos(rotationAngle) * turned.y);
}

fn getLuminosity(color: vec3f) -> f32 {
    return dot(color, vec3f(0.299, 0.587, 0.114));
}

fn sharpen3x3(uv: vec2f, amount: f32, t: texture_2d<f32>, s: sampler) -> vec4f {
    let rcp = rcpres();
    let scene = sample0(t, s, uv);
    var center = RGBToHSL(scene.rgb);
    let up = RGBToHSL(sample0(t, s, uv + vec2f(0.0, -rcp.y)).rgb);
    let down = RGBToHSL(sample0(t, s, uv + vec2f(0.0, rcp.y)).rgb);
    let left = RGBToHSL(sample0(t, s, uv + vec2f(-rcp.x, 0.0)).rgb);
    let right = RGBToHSL(sample0(t, s, uv + vec2f(rcp.x, 0.0)).rgb);
    let neighbors = (up.z + down.z + left.z + right.z) * 0.25;
    center.z = center.z + (center.z - neighbors) * amount;
    return saturate4(vec4f(HSLToRGB(center), scene.a));
}

fn applyVibrance(color: vec3f, amount: f32) -> vec3f {
    let average = (color.r + color.g + color.b) / 3.0;
    let maxChannel = max(max(color.r, color.g), color.b);
    let vibranceAmount = (1.0 - abs(maxChannel - average)) * amount;
    return mix(vec3f(average), color, 1.0 + vibranceAmount);
}

fn overlayChannel(base: f32, blend: f32) -> f32 {
    if blend < 0.5 {
        return saturate(2.0 * max(base, 0.2) * blend);
    }
    return saturate(1.0 - 2.0 * (1.0 - base) * (1.0 - blend));
}

fn overlay(baseColor: vec3f, canvas: vec3f, blendStrength: f32) -> vec3f {
    let coverage = saturate(blendStrength);
    let result = vec3f(overlayChannel(baseColor.r, canvas.r), overlayChannel(baseColor.g, canvas.g), overlayChannel(baseColor.b, canvas.b));
    return mix(baseColor, result, coverage);
}

// Reads a texture in the window of the canvas at the middle of the picture; nothing (0)
// outside of the window. `rcpres` is 1 over the size, as the file has it.
fn sampleCanvasAligned(tex: vec2f, image: texture_2d<f32>, s: sampler, ratio: f32, doRotate: bool) -> vec4f {
    let screenRatio = rcpres().x / rcpres().y;
    var new_width = viewportSize() * ratio * screenRatio;
    var new_height = viewportSize();
    if ratio < screenRatio {
        new_width = viewportSize();
        new_height = viewportSize() / (ratio * screenRatio);
    }
    if doRotate {
        let temp = new_width;
        new_width = new_height;
        new_height = temp;
    }
    let newMin = vec2f(0.5) - vec2f(new_width, new_height) / 2.0;
    let newMax = vec2f(0.5) + vec2f(new_width, new_height) / 2.0;
    var texCentered = tex - vec2f(0.5);
    if doRotate {
        texCentered = vec2f(texCentered.y, -texCentered.x);
    }
    let rotatedTex = texCentered + vec2f(0.5);
    if rotatedTex.x < newMin.x || rotatedTex.x > newMax.x || rotatedTex.y < newMin.y || rotatedTex.y > newMax.y {
        return vec4f(0.0);
    }
    return sample0(image, s, (rotatedTex - newMin) / (newMax - newMin));
}
"
    };
}

/// `jop_gaussian.fx`: the weights of its three kernels as the file has them (the seventh
/// column of the first one too), and its two blurs.
macro_rules! jop_gaussian {
    () => {
        "
const kernelRad4 = array<f32, 81>(
    0.0000, 0.0000, 0.0000, 0.0001, 0.0001, 0.0001, 0.00, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0004, 0.0014, 0.0023, 0.0014, 0.04, 0.0000, 0.0000,
    0.0000, 0.0004, 0.0037, 0.0146, 0.0232, 0.0146, 0.37, 0.0004, 0.0000,
    0.0001, 0.0014, 0.0146, 0.0584, 0.0926, 0.0584, 0.46, 0.0014, 0.0001,
    0.0001, 0.0023, 0.0232, 0.0926, 0.1466, 0.0926, 0.32, 0.0023, 0.0001,
    0.0001, 0.0014, 0.0146, 0.0584, 0.0926, 0.0584, 0.46, 0.0014, 0.0001,
    0.0000, 0.0004, 0.0037, 0.0146, 0.0232, 0.0146, 0.37, 0.0004, 0.0000,
    0.0000, 0.0000, 0.0004, 0.0014, 0.0023, 0.0014, 0.04, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0000, 0.0001, 0.0001, 0.0001, 0.00, 0.0000, 0.0000);
const kernelRad7 = array<f32, 225>(
    0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0001, 0.0001, 0.0001, 0.0001, 0.0001, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0000, 0.0001, 0.0002, 0.0003, 0.0004, 0.0005, 0.0004, 0.0003, 0.0002, 0.0001, 0.0000, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0001, 0.0003, 0.0006, 0.0011, 0.0016, 0.0018, 0.0016, 0.0011, 0.0006, 0.0003, 0.0001, 0.0000, 0.0000,
    0.0000, 0.0001, 0.0003, 0.0008, 0.0018, 0.0034, 0.0049, 0.0055, 0.0049, 0.0034, 0.0018, 0.0008, 0.0003, 0.0001, 0.0000,
    0.0000, 0.0002, 0.0006, 0.0018, 0.0043, 0.0079, 0.0115, 0.0130, 0.0115, 0.0079, 0.0043, 0.0018, 0.0006, 0.0002, 0.0000,
    0.0001, 0.0003, 0.0011, 0.0034, 0.0079, 0.0146, 0.0211, 0.0239, 0.0211, 0.0146, 0.0079, 0.0034, 0.0011, 0.0003, 0.0001,
    0.0001, 0.0004, 0.0016, 0.0049, 0.0115, 0.0211, 0.0305, 0.0345, 0.0305, 0.0211, 0.0115, 0.0049, 0.0016, 0.0004, 0.0001,
    0.0001, 0.0005, 0.0018, 0.0055, 0.0130, 0.0239, 0.0345, 0.0390, 0.0345, 0.0239, 0.0130, 0.0055, 0.0018, 0.0005, 0.0001,
    0.0001, 0.0004, 0.0016, 0.0049, 0.0115, 0.0211, 0.0305, 0.0345, 0.0305, 0.0211, 0.0115, 0.0049, 0.0016, 0.0004, 0.0001,
    0.0001, 0.0003, 0.0011, 0.0034, 0.0079, 0.0146, 0.0211, 0.0239, 0.0211, 0.0146, 0.0079, 0.0034, 0.0011, 0.0003, 0.0001,
    0.0000, 0.0002, 0.0006, 0.0018, 0.0043, 0.0079, 0.0115, 0.0130, 0.0115, 0.0079, 0.0043, 0.0018, 0.0006, 0.0002, 0.0000,
    0.0000, 0.0001, 0.0003, 0.0008, 0.0018, 0.0034, 0.0049, 0.0055, 0.0049, 0.0034, 0.0018, 0.0008, 0.0003, 0.0001, 0.0000,
    0.0000, 0.0000, 0.0001, 0.0003, 0.0006, 0.0011, 0.0016, 0.0018, 0.0016, 0.0011, 0.0006, 0.0003, 0.0001, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0000, 0.0001, 0.0002, 0.0003, 0.0004, 0.0005, 0.0004, 0.0003, 0.0002, 0.0001, 0.0000, 0.0000, 0.0000,
    0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 0.0001, 0.0001, 0.0001, 0.0001, 0.0001, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000);
const kernelRad10 = array<f32, 11>(0.1418, 0.1331, 0.1101, 0.0803, 0.0516, 0.0293, 0.0146, 0.0064, 0.0025, 0.0009, 0.0003);

fn GaussianBlurH(uv: vec2f, t: texture_2d<f32>, s: sampler, spread: f32, texelSize: vec2f) -> vec3f {
    var sum = textureSampleLevel(t, s, uv, 0.0).rgb * kernelRad10[0];
    for (var i = 1; i <= 10; i++) {
        let offset = vec2f(f32(i) * texelSize.x * spread, 0.0);
        sum += textureSampleLevel(t, s, uv + offset, 0.0).rgb * kernelRad10[i];
        sum += textureSampleLevel(t, s, uv - offset, 0.0).rgb * kernelRad10[i];
    }
    return sum;
}

fn GaussianBlurV(uv: vec2f, t: texture_2d<f32>, s: sampler, spread: f32, texelSize: vec2f) -> vec3f {
    var sum = textureSampleLevel(t, s, uv, 0.0).rgb * kernelRad10[0];
    for (var i = 1; i <= 10; i++) {
        let offset = vec2f(0.0, f32(i) * texelSize.y * spread);
        sum += textureSampleLevel(t, s, uv + offset, 0.0).rgb * kernelRad10[i];
        sum += textureSampleLevel(t, s, uv - offset, 0.0).rgb * kernelRad10[i];
    }
    return sum;
}
"
    };
}

/// The colour table of `jop_lut.fx` and `jop_pastel.fx`: 16 squares of 16 x 16 in a row.
macro_rules! jop_clut {
    () => {
        "
fn ClutFunc(colorIN: vec3f, t: texture_2d<f32>) -> vec3f {
    let pSize = vec2f(0.00390625, 0.0625);
    let c = saturate3(colorIN) * 15.0;
    let w = floor(c.b);
    var xy = (c.rg + 0.5) * pSize;
    xy.x += w * pSize.y;
    let z = xy.x + pSize.y;
    return mix(textureSampleLevel(t, linear_wrap, xy, 0.0).rgb, textureSampleLevel(t, linear_wrap, vec2f(z, xy.y), 0.0).rgb, c.b - w);
}
"
    };
}

const ADJUSTER: &str = concat!(
    jop_common!(),
    "
@fragment fn adjust(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var rgb = color.rgb + (brightness() + brightnessOffset());
    rgb *= contrast() + contrastOffset();
    rgb = applyVibrance(rgb, saturation() + saturationOffset());
    var hsl = RGBToHSL(rgb);
    hsl.x = fract(hsl.x * (1.0 + hue()));
    return vec4f(HSLToRGB(hsl), color.a);
}
"
);

const BLACKWHITE: &str = "
@fragment fn blackAndWhite(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let darkGrayThreshold = (threshold() + 0.0) / max_threshold();
    let average = dot(color.rgb, vec3f(0.299, 0.587, 0.114));
    if average < darkGrayThreshold {
        return vec4f(0.01, 0.01, 0.01, color.a);
    }
    return vec4f(0.99, 0.99, 0.99, color.a);
}
";

const CHARCOAL: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0);
    let hatch = sampleCanvasAligned(in.tex, tex1, linear_wrap, aspectRatio(), false);
    let result = saturate4((color + 0.1) / (1.0 - hatch));
    return mix(color, result, hatchStrength());
}
"
);

const COMPOSITE: &str = concat!(
    jop_common!(),
    "
@fragment fn composite(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    var image = textureSampleLevel(lastshader, point_clamp, tex, 0.0);
    let canvas = sampleCanvasAligned(tex, tex1, linear_wrap, aspectRatio(), isRotated() != 0.0);
    let alphaMask_1 = sampleCanvasAligned(tex, tex2, linear_clamp, aspectRatio(), false);
    let alphaMask_2 = sampleCanvasAligned(tex, tex3, linear_clamp, aspectRatio(), false);
    let alphaMask_3 = sampleCanvasAligned(tex, tex4, linear_clamp, aspectRatio(), false);
    let alphaMask_4 = sampleCanvasAligned(tex, tex5, linear_clamp, aspectRatio(), false);
    let alphaMask_5 = sampleCanvasAligned(tex, tex7, linear_wrap, aspectRatio(), false);

    let lum = getLuminosity(image.rgb);
    image = mix(image, vec4f(0.01, 0.01, 0.01, image.a), doBlackenImage());

    var canvasStrength = saturate(lum * compositeStrength());
    if hatchStrength() > 0.0 {
        canvasStrength = mix(canvasStrength, 1.0, alphaMask_5.r * smoothstep(-0.5, 1.0, lum));
    }
    image = mix(image, canvas, canvasStrength);
    if doBlackenImage() == 0.0 {
        image = vec4f(overlay(image.rgb, canvas.rgb, compositeStrength() * 0.1), image.a);
    }

    let index = i32(round(maskIndex()));
    image = mix(image, canvas, (1.0 - alphaMask_1.a) * f32(index == 1) * maskEnabled());
    image = mix(image, canvas, (1.0 - alphaMask_2.a) * f32(index == 2) * maskEnabled());
    image = mix(image, canvas, (1.0 - alphaMask_3.a) * f32(index == 3) * maskEnabled());
    image = mix(image, canvas, (1.0 - alphaMask_4.a) * f32(index == 4) * maskEnabled());
    image = mix(image, canvas, (1.0 - alphaMask_5.a) * f32(index == 5) * maskEnabled());

    // The canvas in place of the picture beyond the distance of the fog.
    let distTex = distort(tex, distortionStrength(), tex6, 0.0);
    let depth = readDepth(distTex);
    let distance_exp = fogDistance() * fogDistance();
    let maxDistance_exp = maxDistance() * maxDistance();
    let transitionD = 100.0 + fogDistance() * 10.0;
    image = mix(image, canvas, smoothstep(distance_exp, distance_exp + transitionD, depth) * step(distance_exp, maxDistance_exp));

    return vec4f(mix(vec3f(0.001), image.rgb, canvas.a), image.a);
}
"
);

const DISTORT: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let distTex = distort(in.tex, distortionStrength(), tex1, 0.0);
    return textureSampleLevel(lastshader, linear_clamp, distTex, 0.0);
}
"
);

const DOF: &str = concat!(
    jop_common!(),
    jop_gaussian!(),
    "
fn spread_at(tex: vec2f) -> f32 {
    let adjusted_focus_range = focus_range() * 1.5;
    let depth = readDepth(tex);
    let blurAmount = saturate(abs(depth - target_depth()) / adjusted_focus_range);
    return blurAmount * blur_strength();
}

@fragment fn dof_blur_h(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurH(in.tex, lastshader, linear_clamp, spread_at(in.tex), rcpres()), 1.0);
}

@fragment fn dof_blur_v(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurV(in.tex, lastpass, linear_clamp, spread_at(in.tex), rcpres()), 1.0);
}
"
);

/// The variable `distance` of the file has the name of a function of WGSL: the function that
/// reads it is `distance_`.
const FOG_BW: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let depth = readDepth(in.tex);
    let distance_exp = distance_() * distance_();
    let maxDistance_exp = maxDistance() * maxDistance();
    let transitionD = 100.0 + distance_() * 10.0;
    return mix(color, vec4f(bgColor()), smoothstep(distance_exp, distance_exp + transitionD, depth) * step(distance_exp, maxDistance_exp));
}
"
);

const FOG_COLOR: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let distTex = distort(in.tex, distortionStrength(), tex1, 0.0);
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    let depth = readDepth(distTex);
    let distance_exp = distance_() * distance_();
    let maxDistance_exp = maxDistance() * maxDistance();
    let transitionD = 100.0 + distance_() * 10.0;
    return vec4f(mix(color, fogColor(), smoothstep(distance_exp, distance_exp + transitionD, depth) * step(distance_exp, maxDistance_exp)), 1.0);
}
"
);

const GREYSCALE: &str = "
@fragment fn greyscale(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let average = dot(color.rgb, vec3f(0.299, 0.587, 0.114));
    return vec4f(average, average, average, color.a);
}
";

const HATCH: &str = concat!(
    jop_common!(),
    "
// The hatch texture is 6 levels of hatching in the red, green and blue of two pictures.
fn Hatching(uv_in: vec2f, intensity: f32) -> vec3f {
    let strength = saturate(intensity * hatchStrength());
    let uv = uv_in * 1.2;
    var hatch1 = textureSampleLevel(tex1, linear_wrap, uv / hatchSize(), 0.0).rgb;
    var hatch2 = textureSampleLevel(tex2, linear_wrap, uv / hatchSize(), 0.0).rgb;
    let overbright = max(0.0, strength - 1.0);
    var weightsA = saturate3(vec3f(strength * 6.0) + vec3f(-0.0, -1.0, -2.0));
    var weightsB = saturate3(vec3f(strength * 6.0) + vec3f(-3.0, -4.0, -5.0));
    weightsA = vec3f(weightsA.xy - weightsA.yz, weightsA.z);
    weightsA.z -= weightsB.x;
    weightsB = vec3f(weightsB.xy - weightsB.yz, weightsB.z);
    hatch2 = hatch2 * weightsA;
    hatch1 = hatch1 * weightsB;
    return vec3f(overbright + hatch2.r + hatch2.g + hatch2.b + hatch1.r + hatch1.g + hatch1.b);
}

@fragment fn hatch(in: Quad) -> @location(0) vec4f {
    var distortTex = distort(in.tex, distortionStrength(), tex3, 0.0);
    let color = textureSampleLevel(lastshader, linear_mirror, distortTex, 0.0).rgb;
    distortTex = rotateUvByNormal(distortTex, vec3f(1.0, 1.0, 1.0), PI / 4.0);
    let luminosity = dot(color, vec3f(0.299, 0.587, 0.114));
    return vec4f(Hatching(distortTex, luminosity), 1.0);
}
"
);

const INK: &str = "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f((color.rgb + brightness()) * contrast(), color.a);
}
";

/// The file reads the picture with a point sampler that names no address: that of
/// Direct3D 9 without one is `wrap`.
const KUWAHARA: &str = concat!(
    jop_common!(),
    jop_gaussian!(),
    "
const KERNEL_RADIUS: i32 = 7;

struct Region {
    mean: vec3f,
    variance: f32,
}

fn calcRegion(uv: vec2f, xStart: i32, xEnd: i32, yStart: i32, yEnd: i32) -> Region {
    var sum = vec3f(0.0);
    var squareSum = vec3f(0.0);
    var totalWeight = 0.0;
    let minRadius = max(1.0, f32(KERNEL_RADIUS) * 0.5);
    let depth = saturate(readDepth(uv) / 100000.0);
    let effectiveRadius = mix(f32(KERNEL_RADIUS), minRadius, depth);
    let rcp = rcpres();
    for (var x = xStart; x <= xEnd; x++) {
        for (var y = yStart; y <= yEnd; y++) {
            if length(vec2f(f32(x), f32(y))) <= effectiveRadius {
                let offset = vec2f(rcp.x * f32(x) * radius(), rcp.y * f32(y) * radius());
                let c = textureSampleLevel(lastshader, point_wrap, uv + offset, 0.0).rgb;
                let weight = kernelRad7[(x + KERNEL_RADIUS) * 15 + (y + KERNEL_RADIUS)];
                sum += c * weight;
                squareSum += c * c * weight;
                totalWeight += weight;
            }
        }
    }
    var r: Region;
    r.mean = sum / totalWeight;
    r.variance = length(abs((squareSum / totalWeight) - (r.mean * r.mean)));
    return r;
}

@fragment fn paint(in: Quad) -> @location(0) vec4f {
    let regionA = calcRegion(in.tex, -KERNEL_RADIUS, 0, -KERNEL_RADIUS, 0);
    let regionB = calcRegion(in.tex, 0, KERNEL_RADIUS, -KERNEL_RADIUS, 0);
    let regionC = calcRegion(in.tex, -KERNEL_RADIUS, 0, 0, KERNEL_RADIUS);
    let regionD = calcRegion(in.tex, 0, KERNEL_RADIUS, 0, KERNEL_RADIUS);

    var col = regionA.mean;
    var minVar = regionA.variance;

    var testVal = step(regionB.variance, minVar);
    col = mix(col, regionB.mean, testVal);
    minVar = mix(minVar, regionB.variance, testVal);

    testVal = step(regionC.variance, minVar);
    col = mix(col, regionC.mean, testVal);
    minVar = mix(minVar, regionC.variance, testVal);

    testVal = step(regionD.variance, minVar);
    col = mix(col, regionD.mean, testVal);
    return vec4f(col, 1.0);
}

@fragment fn main_blurH(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurH(in.tex, lastpass, linear_clamp, blur_strength(), rcpres()), 1.0);
}

@fragment fn main_blurV(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurV(in.tex, lastpass, linear_clamp, blur_strength(), rcpres()), 1.0);
}
"
);

/// The file mixes each of its ten tables with a weight of 0 or 1: the port reads the one
/// with the weight 1.
const LUT: &str = concat!(
    jop_clut!(),
    "
@fragment fn lut(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var rgb = scene.rgb;
    switch i32(round(selectedLut())) {
        case 1 { rgb = ClutFunc(rgb, tex1); }
        case 2 { rgb = ClutFunc(rgb, tex2); }
        case 3 { rgb = ClutFunc(rgb, tex3); }
        case 4 { rgb = ClutFunc(rgb, tex4); }
        case 5 { rgb = ClutFunc(rgb, tex5); }
        case 6 { rgb = ClutFunc(rgb, tex6); }
        case 7 { rgb = ClutFunc(rgb, tex7); }
        case 8 { rgb = ClutFunc(rgb, tex8); }
        case 9 { rgb = ClutFunc(rgb, tex9); }
        case 10 { rgb = ClutFunc(rgb, tex10); }
        default {}
    }
    return vec4f(rgb, scene.a);
}
"
);

const MOTTLE: &str = concat!(
    jop_common!(),
    "
fn getBrightness(color: vec3f) -> f32 {
    return max(max(color.r, color.g), color.b);
}

@fragment fn main(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let color = textureSampleLevel(lastshader, linear_mirror, tex, 0.0).rgb;

    let uv1 = vec2f(tex.x + sin(Time() * speed() + 2.0) * 0.04, tex.y + cos(Time() * speed() + 2.0) * 0.04);
    let strength1 = canvas_strength() * (1.0 - getBrightness(color));
    let distTex1 = distort(uv1, 0.05, tex3, 0.0);
    let mottleColor1 = textureSampleLevel(tex1, linear_wrap, distTex1 / scale(), 0.0).rgb;
    var result = overlay(color, mottleColor1, strength1);

    let uv2 = vec2f(tex.x + sin(Time() * speed() + 2.0) * 0.09, tex.y + cos(Time() * speed() + 2.0) * 0.09);
    let strength2 = canvas_strength() * getBrightness(color);
    let distTex2 = distort(uv2, 0.1, tex3, 20.0);
    let mottleColor2 = textureSampleLevel(tex2, linear_wrap, distTex2 / scale(), 0.0).rgb;
    result = overlay(result, mottleColor2, strength2);
    return vec4f(result, 1.0);
}
"
);

/// The first pass gives the edges of the picture in its alpha; the second pass reads no
/// other part of it.
const OIL: &str = concat!(
    jop_common!(),
    "
fn applyColorTemperature(color: vec3f, amount: f32) -> vec3f {
    let lum = dot(color, vec3f(0.299, 0.587, 0.114));
    let temperatureCurve = smoothstep(0.0, 1.0, lum);
    let temperatureTint = mix(coolColor(), warmColor(), temperatureCurve);
    return mix(color, color * temperatureTint, amount);
}

fn position_at(tex: vec2f) -> vec3f {
    return getPosition(tex, readDepth(tex));
}

@fragment fn edgedetecting(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let rcp = rcpres();
    let pos2 = position_at(tex);
    let left2 = pos2 - position_at(tex + rcp * vec2f(-1.0, 0.0));
    let right2 = position_at(tex + rcp * vec2f(1.0, 0.0)) - pos2;
    let up2 = pos2 - position_at(tex + rcp * vec2f(0.0, -1.0));
    let down2 = position_at(tex + rcp * vec2f(0.0, 1.0)) - pos2;
    var dx2 = right2;
    if length(left2) < length(right2) {
        dx2 = left2;
    }
    var dy2 = down2;
    if length(up2) < length(down2) {
        dy2 = up2;
    }
    var norm = normalize(cross(dx2, dy2));
    norm.z *= -1.0;

    var c = array<vec2f, 9>(
        vec2f(-0.0078125, 0.0078125), vec2f(0.00, 0.0078125), vec2f(0.0078125, 0.0078125),
        vec2f(-0.0078125, 0.00), vec2f(0.0, 0.0), vec2f(0.0078125, 0.007),
        vec2f(-0.0078125, -0.0078125), vec2f(0.00, -0.0078125), vec2f(0.0078125, -0.0078125));
    var lum = array<f32, 9>();
    for (var i = 0; i < 9; i++) {
        lum[i] = dot(textureSampleLevel(lastshader, linear_clamp, tex + 0.2 * c[i], 0.0).rgb, vec3f(0.30, 0.59, 0.11));
    }
    let x = lum[2] + lum[8] + 2.0 * lum[5] - lum[0] - 2.0 * lum[3] - lum[6];
    let y = lum[6] + 2.0 * lum[7] + lum[8] - lum[0] - 2.0 * lum[1] - lum[2];
    var edge = 0.0;
    if x * x + y * y < threshold() {
        edge = 1.0;
    }
    return vec4f(norm, edge);
}

fn edge_at(tex: vec2f) -> f32 {
    return textureSampleLevel(lastpass, linear_clamp, tex, 0.0).a;
}

@fragment fn splashblend(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let image = textureSampleLevel(lastshader, linear_clamp, tex, 0.0);
    let lum = sqrt(dot(image.rgb * image.rgb, vec3f(0.29, 0.58, 0.114)));
    let r = rcpres().y;
    var edges = edge_at(tex) / 2.0;
    edges += edge_at(tex + vec2f(0.5, 0.5) * r) / 8.0;
    edges += edge_at(tex + vec2f(-0.5, 0.5) * r) / 8.0;
    edges += edge_at(tex + vec2f(0.5, -0.5) * r) / 8.0;
    edges += edge_at(tex + vec2f(-0.5, -0.5) * r) / 8.0;
    edges *= smoothstep(0.0, 1.0, lum);

    var color = image.rgb * edges * edges;
    color = 1.0 - (1.0 - color) * (1.0 - image.rgb);
    color = applyVibrance(color, vibrance());
    color = applyColorTemperature(color, temperatureShiftStrength());
    return vec4f(color, 1.0);
}
"
);

const OUTLINE: &str = concat!(
    jop_common!(),
    jop_gaussian!(),
    "
// The sum of the differences of the depth to the four sides: `LinearEyeDepth` of the file
// gives the depth as it is.
fn SobelSampleDepth(uv: vec2f, offset: vec2f) -> f32 {
    let center = readDepth(uv);
    let left = readDepth(uv - vec2f(offset.x, 0.0));
    let right = readDepth(uv + vec2f(offset.x, 0.0));
    let up = readDepth(uv + vec2f(0.0, offset.y));
    let down = readDepth(uv - vec2f(0.0, offset.y));
    return (left - center) + (right - center) + (up - center) + (down - center);
}

fn calculateSobelOutline(sobel_in: f32, depth: f32) -> f32 {
    let adjustedLineTest = lineTest() + (saturate(depth / 25000.0) * 500.0);
    var sobelDepth = 0.0;
    if sobel_in > adjustedLineTest {
        sobelDepth = saturate(sobel_in);
    }
    sobelDepth = pow(saturate(sobelDepth) * OutlineDepthMultiplier(), OutlineDepthBias());
    return step(0.01, sobelDepth);
}

fn getFadeStrength(tex: vec2f) -> f32 {
    let uv = vec2f(tex.x + sin(Time() * 0.1) * 0.1, tex.y + cos(Time() * 0.1) * 0.1) / 4.0;
    let fadeStrength = textureSampleLevel(tex1, linear_wrap, uv * fadePerlinScale(), 0.0).r;
    return saturate(fadeStrength * 1.5 - 0.3);
}

fn getSobelOutline(tex: vec2f, pos: vec3f, thickness: f32, depth: f32) -> f32 {
    let sobelDepth = SobelSampleDepth(tex, rcpres() * thickness);
    let sobelOutline = calculateSobelOutline(sobelDepth, depth);
    let water = pos.z * u.eyevec.z - pos.y * xylength() + u.eyepos.z;
    return sobelOutline * f32(water > u.suncol.w);
}

// An edge where the normals around the pixel change more than the threshold.
fn getNormalsOutline(tex: vec2f, thickness: f32) -> f32 {
    let normalCenter = getWorldSpaceNormal(tex);
    var normalDiff = vec3f(0.0);
    for (var x = -4; x <= 4; x++) {
        for (var y = -4; y <= 4; y++) {
            if x * x + y * y <= 16 {
                let weight = kernelRad4[(x + 4) * 9 + (y + 4)];
                let offset = vec2f(f32(x), f32(y)) * rcpres() * thickness;
                normalDiff += abs(getWorldSpaceNormal(tex + offset) - normalCenter) * weight;
            }
        }
    }
    return step(normalOutlineThreshold(), dot(normalDiff, vec3f(1.0, 1.0, 1.0)));
}

// The dark parts of the picture are a line too: 0.99 under the level `shadow`, then a
// short fall to 0.
fn getShadow(color: vec3f) -> f32 {
    let darkGrayThreshold = shadow();
    let fadeThreshold = darkGrayThreshold + 0.01;
    let average = dot(color, vec3f(0.299, 0.587, 0.114));
    if average < darkGrayThreshold {
        return 0.99;
    } else if average < fadeThreshold {
        let smoothDecay = saturate((average - darkGrayThreshold) / (fadeThreshold - darkGrayThreshold));
        return mix(0.99, 0.0, smoothDecay);
    }
    return 0.0;
}

@fragment fn outline(in: Quad) -> @location(0) vec4f {
    let rawTex = in.tex;
    let tex = distort(rawTex, distortionStrength(), tex1, 0.0);
    let pos = toView(tex);
    let fog = saturate((u.fog_near.y - length(pos)) / (u.fog_near.y - u.fog_near.x));

    let clamped_dist = saturate(length(pos) / maxDistance());
    let thickness = (outlineThickness() + outlineThickness() * (1.0 - clamped_dist)) * getFadeStrength(tex);

    let depth = readDepth(tex);
    let sobelOutline = getSobelOutline(tex, pos, thickness, depth);
    let normalOutline = getNormalsOutline(tex, thickness);
    let shadowEffect = getShadow(textureSampleLevel(lastshader, linear_clamp, tex, 0.0).rgb);
    var line = max(normalOutline, max(sobelOutline, shadowEffect));
    line = mix(0.0, line, saturate(fog + 0.3));

    let distStrength = distortionStrength() * 1.1;
    let lineOffset = timeOffsetMulti() * min(distStrength, 0.03);
    let sceneColor = textureSampleLevel(lastshader, linear_clamp, distort(rawTex, distStrength, tex1, lineOffset), 0.0);
    let lineColor = min(sceneColor.rgb * lineDarkMulti(), vec3f(lineDarkMax()));
    return vec4f(mix(sceneColor.rgb, lineColor, line), sceneColor.a);
}
"
);

const PASTEL: &str = concat!(
    jop_common!(),
    jop_gaussian!(),
    jop_clut!(),
    "
fn getBrightness(color: vec3f) -> f32 {
    return max(max(color.r, color.g), color.b);
}

fn doOverlay(baseColor: vec3f, uv: vec2f, size: f32, amount: f32, t: texture_2d<f32>) -> vec3f {
    let coverage = textureSampleLevel(t, linear_wrap, uv * size, 0.0).r;
    return overlay(baseColor, vec3f(coverage), amount);
}

@fragment fn main_blurH(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurH(in.tex, lastshader, point_clamp, blur_strength(), rcpres()), 1.0);
}

@fragment fn main_blurV(in: Quad) -> @location(0) vec4f {
    return vec4f(GaussianBlurV(in.tex, lastpass, linear_clamp, blur_strength(), rcpres()), 1.0);
}

@fragment fn main_pastel(in: Quad) -> @location(0) vec4f {
    let blurred = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    let sharpened = sharpen3x3(in.tex, sharpen_strength(), lastshader, point_clamp);
    var rgb = mix(sharpened.rgb, blurred.rgb, 0.5);
    rgb = mix(rgb, ClutFunc(rgb, tex1), lut_strength());
    rgb = pow(max(rgb, vec3f(0.0)), vec3f(1.0 / gamma()));
    rgb = applyVibrance(rgb, vibrance());
    return vec4f(rgb, sharpened.a);
}

@fragment fn main_overlay(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let image = textureSampleLevel(lastpass, linear_clamp, tex, 0.0);
    var rgb = image.rgb;

    let uv1 = vec2f(tex.x + sin(Time() * 0.2 + 2.0) * 0.04, tex.y + cos(Time() * 0.2 + 2.0) * 0.04);
    var amount = canvas_strength() * saturate(0.3 + getBrightness(rgb));
    var distTex = distort(uv1, 0.1, tex5, 0.0);
    rgb = doOverlay(rgb, distTex, canvas_scale(), amount, tex2);

    let uv2 = vec2f(tex.x + sin(Time() * 0.2 + 2.0) * 0.09, tex.y + cos(Time() * 0.2 + 2.0) * 0.09);
    amount = canvas_strength2() * (1.0 - getBrightness(rgb));
    distTex = distort(uv2, 0.1, tex5, 20.0);
    rgb = doOverlay(rgb, distTex, grain_scale(), amount, tex3);

    rgb = overlay(rgb, textureSampleLevel(tex4, linear_wrap, tex, 0.0).rgb, grain_strength() * 0.1);
    return vec4f(rgb, image.a);
}
"
);

const QUANTIZE: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let depth = saturate(readDepth(in.tex) / 100000.0);
    let effectiveLumLevels = luminosityLevels();
    let maxHueLevels = max(50.0, hueLevels());
    let effectiveHueLevels = mix(hueLevels(), maxHueLevels, depth);

    let texColor = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var hsl = RGBToHSL(texColor.rgb);

    let luminosity = hsl.z;
    let offset = 1.0 / (2.0 * effectiveLumLevels);
    if luminosityLevels() > 0.0 {
        hsl.z = ceil(luminosity * effectiveLumLevels) / (effectiveLumLevels - 1.0) - offset;
    }
    if hueLevels() > 0.0 {
        hsl.x = ceil(hsl.x * effectiveHueLevels) / effectiveHueLevels;
    }
    return vec4f(HSLToRGB(hsl), texColor.a);
}
"
);

const SHARPEN: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(sharpen3x3(in.tex, sharpen_strength(), lastshader, point_clamp).rgb, color.a);
}
"
);

const SPLASH: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let color = textureSampleLevel(lastshader, linear_mirror, tex, 0.0).rgb;

    let uv1 = vec2f(tex.x + sin(Time() * speed() + 2.0) * 0.04, tex.y + cos(Time() * speed() + 2.0) * 0.04);
    let strength1 = strength_1() * (1.0 - getLuminosity(color));
    let distTex1 = distort(uv1, 0.01, tex3, 0.0);
    let mottleColor1 = textureSampleLevel(tex1, linear_wrap, distTex1 * scale(), 0.0).rgb;
    var result = overlay(color, mottleColor1, strength1);

    let uv2 = vec2f(tex.x + sin(Time() * speed() + 2.0) * 0.09, tex.y + cos(Time() * speed() + 2.0) * 0.09);
    let strength2 = strength_2() * getLuminosity(color);
    let distTex2 = distort(uv2, 0.01, tex3, 20.0);
    let mottleColor2 = textureSampleLevel(tex2, linear_wrap, distTex2 * scale(), 0.0).rgb;
    result = overlay(result, mottleColor2, strength2);

    let scrollColor = textureSampleLevel(tex4, linear_wrap, uv2 * scale(), 0.0).rgb;
    result = overlay(result, scrollColor, strength_3());
    return vec4f(result, 1.0);
}
"
);

const VIGNETTE: &str = "
const Age: f32 = -3.0;

@fragment fn vignette(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0);
    let dist = 0.5 - in.tex;
    return vec4f(color.rgb * ((0.4 + -Age / 100.0 - dot(dist, dist)) * 2.8), color.a);
}
";

const WATERCOLOR: &str = concat!(
    jop_common!(),
    "
@fragment fn main(in: Quad) -> @location(0) vec4f {
    let c1 = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let distTex = distort(in.tex, 0.04, tex1, 0.0);
    let distortedColor = textureSampleLevel(lastshader, point_clamp, distTex, 0.0);
    let color = mix(c1, distortedColor, 0.2);

    var hsl = RGBToHSL(applyVibrance(color.rgb, Saturation()));
    let originalSaturation = hsl.y;
    let smoothFactor = smoothstep(0.0, SaturationThreshold(), originalSaturation);
    hsl.y = mix(originalSaturation, max(originalSaturation, MinSaturation()), smoothFactor);
    return vec4f(HSLToRGB(hsl), color.a);
}
"
);

const WINDOW: &str = concat!(
    jop_common!(),
    "
@fragment fn clamp_levels(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(color.rgb * (0.9 - 0.001) + 0.001, color.a);
}

// Black outside of the window of the canvas.
@fragment fn add_window(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let screenRatio = rcpres().x / rcpres().y;
    var new_width = aspectRatio() * screenRatio;
    var new_height = 1.0;
    if aspectRatio() < screenRatio {
        new_width = 1.0;
        new_height = 1.0 / (aspectRatio() * screenRatio);
    }
    new_width = new_width * view_port_size();
    new_height = new_height * view_port_size();
    let lo = vec2f(0.5) - vec2f(new_width, new_height) / 2.0;
    let hi = vec2f(0.5) + vec2f(new_width, new_height) / 2.0;
    if tex.x < lo.x || tex.x > hi.x || tex.y < lo.y || tex.y > hi.y {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    return textureSampleLevel(lastpass, point_clamp, tex, 0.0);
}
"
);

const PERLIN: &str = "jop/perlinNoise.tga";

/// The ports, with the names of the effect files.
pub const SHADERS: &[Shader] = &[
    shader(
        "jop_adjuster",
        ADJUSTER,
        &[full("adjust")],
        36,
        &[],
        &[ASPECT, VIEWPORT, v("brightness", &[0.0]), v("contrast", &[1.0]), v("saturation", &[0.0]), v("brightnessOffset", &[0.0]), v("contrastOffset", &[0.0]), v("saturationOffset", &[0.0]), v("hue", &[0.0])],
    ),
    shader("jop_blackwhite", BLACKWHITE, &[full("blackAndWhite")], 450, &[], &[v("threshold", &[0.5]), v("max_threshold", &[3.0])]),
    shader("jop_charcoal", CHARCOAL, &[full("main")], 80, &["jop/charcoal.tga"], &[ASPECT, VIEWPORT, v("hatchStrength", &[0.2]), v("hatchSize", &[1.0])]),
    shader(
        "jop_composite",
        COMPOSITE,
        &[full("composite")],
        500,
        &["jop/composite_tex.dds", "jop/vignette/vig_sketch.tga", "jop/vignette/vig_char.tga", "jop/vignette/vig_water.tga", "jop/vignette/vig_oil.tga", PERLIN, "jop/pencil_tile.tga"],
        &[
            ASPECT,
            VIEWPORT,
            v("distortionStrength", &[0.05]),
            v("doBlackenImage", &[0.0]),
            v("compositeStrength", &[0.0]),
            v("isRotated", &[0.0]),
            v("fogDistance", &[250.0]),
            v("maskEnabled", &[0.0]),
            v("maskIndex", &[0.0]),
            v("hatchStrength", &[0.0]),
            v("hatchSize", &[1.0]),
            v("diffThreshold", &[0.05]),
            v("blendStrength", &[1.0]),
            v("waterColorDistortion", &[0.1]),
            v("maxDistance", &[249.0]),
        ],
    ),
    shader("jop_distort", DISTORT, &[full("main")], 65, &[PERLIN], &[ASPECT, VIEWPORT, v("distortionStrength", &[0.05])]),
    shader("jop_dof", DOF, &[full("dof_blur_h"), full("dof_blur_v")], 2, &[], &[ASPECT, VIEWPORT, v("target_depth", &[5000000.0]), v("blur_strength", &[0.0]), v("focus_range", &[1000.0])]),
    shader("jop_fog_bw", FOG_BW, &[full("main")], 60, &[], &[ASPECT, VIEWPORT, v("distance", &[250.0]), v("maxDistance", &[249.0]), v("bgColor", &[1.0])]),
    shader("jop_fog_color", FOG_COLOR, &[full("main")], 81, &[PERLIN], &[ASPECT, VIEWPORT, v("distance", &[250.0]), v("maxDistance", &[249.0]), v("fogColor", &[0.5, 0.5, 0.5]), v("distortionStrength", &[0.05])]),
    shader("jop_greyscale", GREYSCALE, &[full("greyscale")], 450, &[], &[]),
    shader(
        "jop_hatch",
        HATCH,
        &[full("hatch")],
        82,
        &["jop/Hatch1.tga", "jop/Hatch2.tga", PERLIN],
        &[ASPECT, VIEWPORT, v("timeOffsetMulti", &[0.0]), v("distortionStrength", &[0.05]), v("fogDistance", &[100000.0]), v("hatchStrength", &[4.0]), v("hatchSize", &[0.1])],
    ),
    shader("jop_ink", INK, &[full("main")], 70, &[], &[v("contrast", &[1.0]), v("brightness", &[0.0])]),
    shader("jop_kuwahara", KUWAHARA, &[full("paint"), full("main_blurH"), full("main_blurV")], 68, &[], &[ASPECT, VIEWPORT, v("radius", &[1.0]), v("blur_strength", &[1.0])]),
    shader(
        "jop_lut",
        LUT,
        &[full("lut")],
        160,
        &[
            "jop/luts/neutral.tga",
            "jop/luts/saturated.tga",
            "jop/luts/desaturated.tga",
            "jop/luts/warm.tga",
            "jop/luts/cold.tga",
            "jop/luts/hueShifted_1.tga",
            "jop/luts/hueShifted_2.tga",
            "jop/luts/hueShifted_3.tga",
            "jop/luts/sepia.tga",
            "jop/luts/radioactive.tga",
        ],
        &[ASPECT, VIEWPORT, v("selectedLut", &[1.0])],
    ),
    shader("jop_mottle", MOTTLE, &[full("main")], 159, &["jop/splash_watercolor.tga", "jop/splash_watercolor_2.tga", PERLIN], &[ASPECT, VIEWPORT, v("canvas_strength", &[0.3]), v("scale", &[1.0]), v("speed", &[0.3])]),
    shader(
        "jop_oil",
        OIL,
        &[full("edgedetecting"), full("splashblend")],
        70,
        &[],
        &[ASPECT, VIEWPORT, v("vibrance", &[1.0]), v("threshold", &[0.04]), v("temperatureShiftStrength", &[0.5]), v("warmColor", &[1.0, 0.9, 0.7]), v("coolColor", &[0.7, 0.8, 1.0])],
    ),
    shader(
        "jop_outline",
        OUTLINE,
        &[full("outline")],
        79,
        &[PERLIN],
        &[
            ASPECT,
            VIEWPORT,
            v("maxDistance", &[62000.0]),
            v("outlineThickness", &[2.0]),
            v("lineTest", &[5.0]),
            v("lineDarkMulti", &[0.05]),
            v("lineDarkMax", &[0.1]),
            v("fadePerlinScale", &[5.0]),
            v("normalOutlineThreshold", &[5.0]),
            v("shadow", &[0.1]),
            v("timeOffsetMulti", &[0.0]),
            v("distortionStrength", &[0.05]),
            v("OutlineDepthMultiplier", &[1.0]),
            v("OutlineDepthBias", &[1.0]),
        ],
    ),
    shader(
        "jop_pastel",
        PASTEL,
        &[full("main_blurH"), full("main_blurV"), full("main_pastel"), full("main_overlay")],
        100,
        &["jop/luts/pastel.tga", "jop/pastelOverlay.tga", "jop/pastelOverlay2.tga", "jop/grain.tga", PERLIN],
        &[
            ASPECT,
            VIEWPORT,
            v("canvas_strength", &[0.2]),
            v("canvas_strength2", &[0.2]),
            v("canvas_scale", &[1.0]),
            v("grain_strength", &[1.0]),
            v("grain_scale", &[1.0]),
            v("lut_strength", &[0.6]),
            v("blur_strength", &[1.2]),
            v("sharpen_strength", &[2.0]),
            v("gamma", &[1.5]),
            v("vibrance", &[0.0]),
            v("overlay_range", &[1.0, 1.0, 1.0]),
        ],
    ),
    shader("jop_quantize", QUANTIZE, &[full("main")], 38, &[], &[ASPECT, VIEWPORT, v("luminosityLevels", &[12.0]), v("hueLevels", &[20.0])]),
    shader("jop_sharpen", SHARPEN, &[full("main")], 50, &[], &[ASPECT, VIEWPORT, v("sharpen_strength", &[0.0])]),
    shader(
        "jop_splash",
        SPLASH,
        &[full("main")],
        120,
        &["jop/oilOverlay.tga", "jop/oilOverlay2.tga", PERLIN, "jop/oilOverlay3.tga"],
        &[ASPECT, VIEWPORT, v("strength_1", &[0.4]), v("strength_2", &[0.3]), v("strength_3", &[0.1]), v("canvas_strength", &[0.0]), v("scroll_strength", &[0.1]), v("scale", &[1.0]), v("speed", &[0.2])],
    ),
    shader("jop_vignette", VIGNETTE, &[full("vignette")], 0, &[], &[]),
    shader(
        "jop_watercolor",
        WATERCOLOR,
        &[full("main")],
        370,
        &[PERLIN],
        &[ASPECT, VIEWPORT, v("Saturation", &[0.2]), v("SaturationThreshold", &[0.1]), v("MinSaturation", &[0.2]), v("selectedLut", &[1.0]), v("LineThickness", &[0.0003])],
    ),
    shader("jop_window", WINDOW, &[full("clamp_levels"), full("add_window")], 10000, &[], &[ASPECT, VIEWPORT, v("view_port_size", &[0.8])]),
];
