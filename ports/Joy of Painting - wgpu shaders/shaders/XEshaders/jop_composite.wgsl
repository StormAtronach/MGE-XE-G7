//! interface: mge-xe-wgsl 1
//! passes: composite
//! category: final
//! priority_adjust: 500
//! texture: jop/composite_tex.dds, jop/vignette/vig_sketch.tga, jop/vignette/vig_char.tga, jop/vignette/vig_water.tga, jop/vignette/vig_oil.tga, jop/perlinNoise.tga, jop/pencil_tile.tga
//! variables: aspectRatio f32 = 1.3, viewportSize f32 = 0.8, distortionStrength f32 = 0.05, doBlackenImage f32 = 0, compositeStrength f32 = 0, isRotated f32 = 0, fogDistance f32 = 250, maskEnabled f32 = 0, maskIndex f32 = 0, hatchStrength f32 = 0, hatchSize f32 = 1, diffThreshold f32 = 0.05, blendStrength f32 = 1, waterColorDistortion f32 = 0.1, maxDistance f32 = 249

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
