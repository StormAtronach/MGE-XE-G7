//! interface: mge-xe-wgsl 1
//! passes: paint, main_blurH, main_blurV
//! category: final
//! priority_adjust: 68
//! variables: aspectRatio f32 = 1.3, viewportSize f32 = 0.8, radius f32 = 1, blur_strength f32 = 1

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
