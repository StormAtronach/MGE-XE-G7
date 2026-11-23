//! interface: mge-xe-wgsl 1
//! passes: fix_sky, blur_hor_i, blur_vert_i, bright_pass, blur_hor_2, blur_vert_2, recombine_a, blur_hor_3, blur_vert_3, bloom_b
//! category: sensor

const bloom_mult_global: f32 = 0.22; // file
const bloom_mult_uwater: f32 = 1.2; // file
const bloom_mult_inside: f32 = 1.1; // file
const bloom_mult_outside: f32 = 0.8; // file
const bloom_cutoff: f32 = 0.72; // file
const bloom_power: f32 = 1.1; // file

// The switches of the file: 1 blooms the sky, 1 darkens the scene (the divisors of the
// blurs), 1 blooms the hands of the player. The quality of the file is 3.
const BLOOM_FX_SKY: i32 = 0; // file
const BLOOM_FX_DARK: i32 = 1; // file
const BLOOM_FX_HANDS: i32 = 0; // file

// The divisors of the blurs with 6 taps on each side, and with 4.
fn div6() -> f32 {
    if BLOOM_FX_DARK == 1 {
        return 5.65;
    }
    return 5.2;
}
fn div4() -> f32 {
    if BLOOM_FX_DARK == 1 {
        return 5.2;
    }
    return 4.7;
}

fn shader_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, linear_mirror, t, 0.0);
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_mirror, t, 0.0);
}
fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

// Without the sky: the picture goes dark past the fog. Without the hands: it goes dark
// under 50 units.
fn depth_fixed(tex: vec2f) -> vec4f {
    let d = depth_at(tex);
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    var color = shader_at(tex);
    var keep = 1.0;
    if BLOOM_FX_SKY != 1 {
        keep *= saturate((fogrange - d) / (fogrange - fogstart));
    }
    if BLOOM_FX_HANDS != 1 {
        keep *= smoothstep(35.0, 50.0, d);
    }
    color = vec4f(color.rgb * keep, color.a);
    return color;
}

// The six taps on each side of a blur along `step`, with the divisor of the quality 3.
fn blur6(tex: vec2f, step: vec2f) -> vec4f {
    var color = pass_at(tex);
    color += pass_at(tex - step * 1.0) * 0.8;
    color += pass_at(tex + step * 1.0) * 0.8;
    color += pass_at(tex - step * 2.0) * 0.65;
    color += pass_at(tex + step * 2.0) * 0.65;
    color += pass_at(tex - step * 3.0) * 0.4;
    color += pass_at(tex + step * 3.0) * 0.4;
    color += pass_at(tex - step * 4.0) * 0.25;
    color += pass_at(tex + step * 4.0) * 0.25;
    return color;
}

// The spread of the second blur: wider where the picture is bright.
fn spread_of(color: vec4f) -> f32 {
    let spread = 1.0 - (color.r * 0.3) + (color.g * 0.6) + (color.b * 0.1);
    return (smoothstep(0.0, 0.7, spread) * 4.0) + 1.0;
}

@fragment fn fix_sky(in: Quad) -> @location(0) vec4f {
    return depth_fixed(in.tex);
}

@fragment fn blur_hor_i(in: Quad) -> @location(0) vec4f {
    let step = vec2f(rcpres().x, 0.0);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    color /= div6();
    return color * color;
}

@fragment fn blur_vert_i(in: Quad) -> @location(0) vec4f {
    let step = vec2f(0.0, rcpres().y);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    return color / div6();
}

@fragment fn bright_pass(in: Quad) -> @location(0) vec4f {
    let c_b = pass_at(in.tex);
    var c = smoothstep(vec4f(0.0), vec4f(bloom_cutoff), c_b);
    c = pow(c, vec4f(bloom_power));
    c += smoothstep(vec4f(bloom_cutoff), vec4f(0.5 + bloom_cutoff), c_b);
    return vec4f(c.rgb, 1.0);
}

@fragment fn blur_hor_2(in: Quad) -> @location(0) vec4f {
    let spread = spread_of(pass_at(in.tex));
    return blur6(in.tex, vec2f(rcpres().x * spread, 0.0)) / div4();
}

@fragment fn blur_vert_2(in: Quad) -> @location(0) vec4f {
    let spread = spread_of(pass_at(in.tex));
    return blur6(in.tex, vec2f(0.0, rcpres().y * spread)) / div4();
}

@fragment fn recombine_a(in: Quad) -> @location(0) vec4f {
    let c_b = depth_fixed(in.tex);
    var c = smoothstep(vec4f(0.0), vec4f(bloom_cutoff), pass_at(in.tex));
    c += smoothstep(vec4f(bloom_cutoff - 0.5), vec4f(bloom_cutoff + 0.5), c_b);
    return clamp(c, vec4f(0.0), vec4f(1.0));
}

@fragment fn blur_hor_3(in: Quad) -> @location(0) vec4f {
    let rcp = rcpres();
    var color = blur6(in.tex, vec2f(rcp.x, 0.0));
    let outer = vec2f(rcp.y, 0.0);
    color += pass_at(in.tex - outer * 5.0) * 0.15;
    color += pass_at(in.tex + outer * 5.0) * 0.15;
    color += pass_at(in.tex - outer * 6.0) * 0.10;
    color += pass_at(in.tex + outer * 6.0) * 0.10;
    return color / div6();
}

@fragment fn blur_vert_3(in: Quad) -> @location(0) vec4f {
    let step = vec2f(0.0, rcpres().y);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    return color / div6();
}

@fragment fn bloom_b(in: Quad) -> @location(0) vec4f {
    let scene = shader_at(in.tex);
    var blur = pass_at(in.tex) * bloom_mult_global;
    if u.sunamb.w > 0.5 {
        blur *= bloom_mult_inside;
    } else {
        blur *= bloom_mult_outside;
    }
    if u.sunpos.w > 0.5 {
        blur *= bloom_mult_uwater;
    }
    return scene + blur;
}
