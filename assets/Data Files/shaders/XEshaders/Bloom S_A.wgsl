//! interface: mge-xe-wgsl 1
//! passes: prefilter /2 f16, down /4 f16, down /8 f16, down /16 f16, down /32 f16, down /64 f16, up4 /32 f16, up3 /16 f16, up2 /8 f16, up1 /4 f16, up0 /2 f16, combine
//! category: sensor

// The brightness where the bloom starts, the width of its soft start, and its strength.
// The picture has 8-bit colours: a flame or a lamp is near 1, a lit wall near 0.5.
const threshold: f32 = 0.5;
const knee: f32 = 0.25;
const intensity: f32 = 0.4;

fn at(t: vec2f) -> vec3f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0).rgb;
}

// The 13 taps of a downsample by two: the centre, the four corners of the texel, and the
// four corners of the four texels around (the source is twice the size of the target).
fn down13(tex: vec2f, rcp: vec2f) -> vec3f {
    let a = at(tex + rcp * vec2f(-2.0, -2.0));
    let b = at(tex + rcp * vec2f(0.0, -2.0));
    let c = at(tex + rcp * vec2f(2.0, -2.0));
    let d = at(tex + rcp * vec2f(-2.0, 0.0));
    let e = at(tex);
    let f = at(tex + rcp * vec2f(2.0, 0.0));
    let g = at(tex + rcp * vec2f(-2.0, 2.0));
    let h = at(tex + rcp * vec2f(0.0, 2.0));
    let i = at(tex + rcp * vec2f(2.0, 2.0));
    let j = at(tex + rcp * vec2f(-1.0, -1.0));
    let k = at(tex + rcp * vec2f(1.0, -1.0));
    let l = at(tex + rcp * vec2f(-1.0, 1.0));
    let m = at(tex + rcp * vec2f(1.0, 1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

// A 3 x 3 tent over the source, which is half the size of the target.
fn tent9(tex: vec2f, rcp: vec2f) -> vec3f {
    var sum = at(tex) * 4.0;
    sum += (at(tex + rcp * vec2f(-1.0, 0.0)) + at(tex + rcp * vec2f(1.0, 0.0)) + at(tex + rcp * vec2f(0.0, -1.0)) + at(tex + rcp * vec2f(0.0, 1.0))) * 2.0;
    sum += at(tex + rcp * vec2f(-1.0, -1.0)) + at(tex + rcp * vec2f(1.0, -1.0)) + at(tex + rcp * vec2f(-1.0, 1.0)) + at(tex + rcp * vec2f(1.0, 1.0));
    return sum / 16.0;
}

// The part of a colour above the threshold, with a soft knee below it.
fn bright(color: vec3f) -> vec3f {
    let brightness = max(color.r, max(color.g, color.b));
    var soft = brightness - threshold + knee;
    soft = clamp(soft, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    let contribution = max(soft, brightness - threshold) / max(brightness, 1e-4);
    return color * contribution;
}

// The first level: the picture through the threshold, at half size. The threshold is on
// each tap, so that one bright pixel does not spread as a square.
@fragment fn prefilter(in: Quad) -> @location(0) vec4f {
    let rcp = rcpres();
    let tex = in.tex;
    let a = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, -2.0), 0.0).rgb);
    let b = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(0.0, -2.0), 0.0).rgb);
    let c = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, -2.0), 0.0).rgb);
    let d = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, 0.0), 0.0).rgb);
    let e = bright(textureSampleLevel(lastshader, linear_clamp, tex, 0.0).rgb);
    let f = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, 0.0), 0.0).rgb);
    let g = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, 2.0), 0.0).rgb);
    let h = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(0.0, 2.0), 0.0).rgb);
    let i = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, 2.0), 0.0).rgb);
    let j = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-1.0, -1.0), 0.0).rgb);
    let k = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(1.0, -1.0), 0.0).rgb);
    let l = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-1.0, 1.0), 0.0).rgb);
    let m = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(1.0, 1.0), 0.0).rgb);
    return vec4f(e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125, 1.0);
}

// The next level: half the size of the pass before.
@fragment fn down(in: Quad) -> @location(0) vec4f {
    return vec4f(down13(in.tex, target_rcpres() * 0.5), 1.0);
}

// Up the chain: the level below, spread by the tent, plus the level of this size.
@fragment fn up4(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass4, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up3(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass3, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up2(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass2, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up1(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass1, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up0(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass0, linear_clamp, in.tex, 0.0).rgb, 1.0);
}

// The picture plus the bloom. The six levels add up to a sum of about 6 at a flat bright
// area; the intensity is for that sum.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let bloom = tent9(in.tex, rcpres() * 2.0) * (intensity / 6.0);
    return vec4f(scene.rgb + bloom, scene.a);
}
