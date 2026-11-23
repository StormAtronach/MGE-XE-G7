//! interface: mge-xe-wgsl 1
//! passes: dof, dof2
//! category: none
//! requires_hdr: true

const PI: f32 = 3.14159265;
const unit2m: f32 = 0.0142; // file
const samples: i32 = 3; // file
const rings: i32 = 3; // file
const maxblur: f32 = 2.5; // file
const threshold: f32 = 0.7; // file
const gain: f32 = 2.0; // file
const sbright: f32 = 0.5; // file
const bias: f32 = 0.5; // file
const fringe: f32 = 0.6; // file
const namount: f32 = 0.0001; // file
const dbsize: f32 = 1.25; // file
const focus: vec2f = vec2f(0.5, 0.5);

// smoothstep of HLSL: its edges can be in any order.
fn sstep(a: f32, b: f32, x: f32) -> f32 {
    let t = saturate((x - a) / (b - a));
    return t * t * (3.0 - 2.0 * t);
}

fn sungain() -> f32 {
    let top = max(u.suncol.r, max(u.suncol.g, u.suncol.b));
    var part = 0.0;
    if top > 0.0 {
        part = saturate(u.suncol.r / top);
    }
    return (1.03 - part) * 3.0;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

// blurring depth
fn tapblur(coords: vec2f, dp: f32) -> vec4f {
    var d = vec4f(0.0);
    let wh = rcpres() * dbsize;
    let df = 0.3 * (1.0 + dp * 6.0);
    let au = vec4f(1.0, 1.0, 1.0, 3.0);
    for (var i = 0; i < 9; i++) {
        let offset = vec2f(f32(i % 3 - 1), f32(i / 3 - 1)) * wh;
        let weight = f32((2 - abs(i % 3 - 1)) * (2 - abs(i / 3 - 1))) / 16.0;
        let tmp = pow(clamp(textureSampleLevel(lastpass, point_clamp, coords + offset * df, 0.0), vec4f(0.0), vec4f(1.0)) * au, vec4f(2.2));
        d = max(d, d + tmp * weight);
    }
    return pow(d, vec4f(1.0 / 2.2));
}

// generating noise/pattern texture for dithering
fn rand(coord: vec2f) -> vec2f {
    let noiseX = fract(sin(dot(coord, vec2f(12.9898, 78.233))) * 43758.5453) * 2.0 - 1.0;
    let noiseY = fract(sin(dot(coord, vec2f(12.9898, 78.233) * 2.0)) * 43758.5453) * 2.0 - 1.0;
    return vec2f(noiseX, noiseY);
}

// processing the sample
fn color(coords: vec2f, blur: f32, desky: f32) -> vec3f {
    let texel = rcpres();
    let col = vec3f(
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(0.0, 1.0) * texel * fringe * blur, 0.0).r,
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(-0.866, -0.5) * texel * fringe * blur, 0.0).g,
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(0.866, -0.5) * texel * fringe * blur, 0.0).b);
    let lum = dot(col, vec3f(0.299, 0.587, 0.114));
    let thresh = max((lum - threshold) * gain * sungain() * desky, 0.0);
    return col + mix(vec3f(0.0), col, thresh * blur);
}

@fragment fn dof(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let interior = f32(u.sunamb.w != 0.0);
    // scene depth calculation, with the focus at the middle of the picture
    let fDepth = depth_at(focus) * unit2m;
    let depth = depth_at(tex) * unit2m;
    let skdepth = saturate(sstep(9999.0, 1000.0, depth) + sbright);

    // dof blur factor calculation
    let depthme = depth * 0.01;
    let fclose = 1.0 - exp(-depthme);
    let ffar = exp(-depthme * 100.0);
    var blur = mix(fclose * 1.5, ffar * 9.7, saturate(fDepth * 0.05));
    blur = mix(fclose, blur * (1.0 - interior * 0.9), saturate(fDepth * 0.05));
    var blur2 = mix(0.0, ffar * 9.7, saturate(fDepth * 0.05));
    blur2 = mix(0.0, blur2 * (1.0 - interior * 0.9), saturate(fDepth * 0.05));

    blur = pow(blur, 1.0 + interior) * (1.0 + interior * 32.0) - 0.1;
    blur = min(blur * 1.75, 0.55);
    blur += sstep(200.0, 250.0, depth) * 0.4;
    blur = saturate(blur);

    blur2 = pow(blur2, 1.0 + interior) * (1.0 + interior * 32.0) - 0.1;
    blur2 = min(blur2 * 1.75, 0.55);
    blur2 = saturate(blur2);

    // calculation of pattern for ditering
    let noise = rand(tex) * namount * blur;
    // getting blur x and y step factor
    let w = rcpres().x * blur * maxblur + noise.x;
    let h = rcpres().y * blur * maxblur + noise.y;

    // calculation of final color
    var col = pow(textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb, vec3f(2.2));
    var s = 1.0;
    for (var i = 1; i <= rings; i++) {
        let ringsamples = i * samples;
        let edge = mix(1.0, f32(i) / f32(rings), bias);
        for (var j = 0; j < ringsamples; j++) {
            let turn = PI * 2.0 / f32(ringsamples);
            let pw = cos(f32(j) * turn) * f32(i);
            let ph = sin(f32(j) * turn) * f32(i);
            col += pow(color(tex + vec2f(pw * w, ph * h), blur, skdepth) * edge, vec3f(2.2));
            s = max(s, s + edge);
        }
    }
    col /= s * 0.90; // divide by sample count
    col = pow(col, vec3f(1.0 / 2.2));
    return vec4f(col, blur * blur2);
}

@fragment fn dof2(in: Quad) -> @location(0) vec4f {
    let c = textureSampleLevel(lastpass, point_clamp, in.tex, 0.0);
    let dp = 1.0 - sstep(0.0, 100.0, depth_at(in.tex)) * c.a;
    let blur = tapblur(in.tex, dp);
    return vec4f(mix(c.rgb, blur.rgb, blur.a), c.a);
}
