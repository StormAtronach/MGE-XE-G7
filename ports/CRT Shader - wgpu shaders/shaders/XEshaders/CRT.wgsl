//! interface: mge-xe-wgsl 1
//! passes: crt
//! category: final

// CRT.wgsl: the wgpu renderer's port of CRT.fx of the mod CRT Shader (md5 6a8b5ea4). The switches
// of the .fx are constants here, with the values of the file as it ships. The code of a switch
// that is off is kept, so that a change of a switch here gives what the same change gives in the
// .fx. The overscan is not ported: it is off in both files of the mod.

//------------------------------------------------------------
// Shader options
const ENABLE_CURVE = false;
const ENABLE_BLOOM = true;
const ENABLE_BLUR = true;
const ENABLE_BLACKLEVEL = false;
const ENABLE_REFRESHLINE = false;
const ENABLE_SCANLINES = true;
const ENABLE_GRAIN = true;

// Settings - Curve
const CURVE_INTENSITY = 1.0;
const CURVE_ZOOM = 0.226;

// Settings - Bloom
const BLOOM_OFFSET = 0.0015;
const BLOOM_STRENGTH = 0.8;

// Settings - Blur
const BLUR_MULTIPLIER = 1.05;
const BLUR_STRENGTH = 0.2;
const BLUR_OFFSET = 0.003;

// Settings - Blacklevel: with ENABLE_BLACKLEVEL the .fx sets the floor to 0.05 (1 / 40 without it,
// where nothing reads it).
const BLACKLEVEL_FLOOR = vec3f(0.05, 0.05, 0.05);

// Settings - Grain
const GRAIN_INTENSITY = 0.02;

// Scanlines (retro.hlsl)
const SCANLINE_FACTOR = 0.3;

//------------------------------------------------------------

fn crt_read(uv: vec2f) -> vec3f {
    return textureSampleLevel(lastshader, linear_clamp, uv, 0.0).rgb;
}

fn transform_curve(uv_in: vec2f) -> vec2f {
    var uv = uv_in - 0.5; // off the centre of the screen
    let r = (uv.x * uv.x + uv.y * uv.y) * CURVE_INTENSITY; // the ratio
    uv *= 4.2 + r; // apply the ratio
    uv *= CURVE_ZOOM; // zoom
    return uv + 0.5; // back to the centre
}

fn bloom(color: vec3f, uv: vec2f) -> vec3f {
    let b = color - crt_read(uv + vec2f(-BLOOM_OFFSET, 0.0));
    let mask = b * BLOOM_STRENGTH;
    return saturate3(color + mask);
}

// blurWeights of the .fx; the weight of the centre is 0.102.
fn blur_weight(i: i32) -> f32 {
    switch i {
        case 1: { return 0.092; }
        case 2: { return 0.081; }
        case 3: { return 0.071; }
        case 4: { return 0.061; }
        case 5: { return 0.051; }
        case 6: { return 0.041; }
        case 7: { return 0.031; }
        case 8: { return 0.021; }
        default: { return 0.0; }
    }
}

fn blur_h(uv: vec2f) -> vec3f {
    var screen = crt_read(uv) * 0.102;
    for (var i = 1; i < 9; i++) {
        screen += crt_read(uv + vec2f(f32(i) * BLUR_OFFSET, 0.0)) * blur_weight(i);
    }
    for (var i = 1; i < 9; i++) {
        screen += crt_read(uv + vec2f(-f32(i) * BLUR_OFFSET, 0.0)) * blur_weight(i);
    }
    return screen * BLUR_MULTIPLIER;
}

fn blur_v(uv: vec2f) -> vec3f {
    var screen = crt_read(uv) * 0.102;
    for (var i = 1; i < 9; i++) {
        screen += crt_read(uv + vec2f(0.0, f32(i) * BLUR_OFFSET)) * blur_weight(i);
    }
    for (var i = 1; i < 9; i++) {
        screen += crt_read(uv + vec2f(0.0, -f32(i) * BLUR_OFFSET)) * blur_weight(i);
    }
    return screen * BLUR_MULTIPLIER;
}

fn blur(color: vec3f, uv: vec2f) -> vec3f {
    let b = (blur_h(uv) + blur_v(uv)) / 2.0 - color;
    let mask = b * BLUR_STRENGTH;
    return saturate3(color + mask);
}

fn blacklevel(color_in: vec3f) -> vec3f {
    var color = color_in - BLACKLEVEL_FLOOR;
    color = saturate3(color);
    color += BLACKLEVEL_FLOOR;
    return saturate3(color);
}

// fmod of HLSL: the remainder with the sign of x (WGSL's % on floats is the same).
fn refresh_lines(color_in: vec3f, uv: vec2f, time: f32) -> vec3f {
    var color = color_in;
    let time_over = (time / 5.0) % 1.5 - 0.5;
    let tint = time_over - uv.y;
    if uv.y > time_over && uv.y - 0.03 < time_over {
        color += vec3f(tint * 2.0);
    }
    return saturate3(color);
}

fn square_wave(y: f32) -> f32 {
    return 1.0 - (y % 2.0) * SCANLINE_FACTOR;
}

// The .fx has a branch for dark pixels behind "&& false": it never runs, and is not ported.
fn scanlines(color: vec3f, pos: vec2f) -> vec3f {
    let wave = square_wave(pos.y);
    return saturate3(color * wave);
}

// The grain lookup table.
const A0 = 0.151015505647689;
const A1 = -0.5303572634357367;
const A2 = 1.365020122861334;
const B0 = 0.132089632343748;
const B1 = -0.7607324991323768;

fn permute(x_in: f32) -> f32 {
    let x = x_in * (34.0 * x_in + 1.0);
    return 289.0 * fract(x * 1.0 / 289.0);
}

fn grain(color: vec3f, uv: vec2f, time: f32) -> vec3f {
    let m = vec3f(uv, (time % 5.0) / 5.0) + 1.0;
    var state = permute(permute(m.x) + m.y) + m.z;
    // rand(state) of the .fx: permute the state, then its fraction over 41.
    state = permute(state);
    let p = 0.95 * fract(state / 41.0) + 0.025;
    let q = p - 0.5;
    let r2 = q * q;
    let g = q * (A2 + (A1 * r2 + A0) / (r2 * r2 + B1 * r2 + B0));
    return saturate3(color + vec3f(GRAIN_INTENSITY * g));
}

@fragment fn crt(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    // VPOS of Direct3D 9: the integer coordinate of the pixel.
    let pos = floor(in.position.xy);
    var uv = in.tex;

    if ENABLE_CURVE {
        uv = transform_curve(uv);
        // The outer box
        if uv.x < -0.025 || uv.y < -0.025 { return vec4f(0.0, 0.0, 0.0, 1.0); }
        if uv.x > 1.025 || uv.y > 1.025 { return vec4f(0.0, 0.0, 0.0, 1.0); }
        // The bezel
        if uv.x < -0.015 || uv.y < -0.015 { return vec4f(0.03, 0.03, 0.03, 1.0); }
        if uv.x > 1.015 || uv.y > 1.015 { return vec4f(0.03, 0.03, 0.03, 1.0); }
        // The border of the screen
        if uv.x < -0.001 || uv.y < -0.001 { return vec4f(0.0, 0.0, 0.0, 1.0); }
        if uv.x > 1.001 || uv.y > 1.001 { return vec4f(0.0, 0.0, 0.0, 1.0); }
    }

    // The screen uv is the glass of the CRT; without the overscan it is the uv of the reads.
    let screen_uv = uv;
    var color = textureSampleLevel(lastshader, linear_clamp, uv, 0.0);

    if ENABLE_BLOOM {
        color = vec4f(bloom(color.rgb, uv), color.a);
    }
    if ENABLE_BLUR {
        color = vec4f(blur(color.rgb, uv), color.a);
    }
    if ENABLE_BLACKLEVEL {
        color = vec4f(blacklevel(color.rgb), color.a);
    }
    if ENABLE_REFRESHLINE {
        color = vec4f(refresh_lines(color.rgb, screen_uv, time), color.a);
    }
    if ENABLE_SCANLINES {
        color = vec4f(scanlines(color.rgb, pos), color.a);
    }
    if ENABLE_GRAIN {
        color = vec4f(grain(color.rgb, screen_uv, time), color.a);
    }
    return color;
}
