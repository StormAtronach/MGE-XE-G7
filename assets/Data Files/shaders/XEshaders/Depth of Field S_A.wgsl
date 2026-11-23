//! interface: mge-xe-wgsl 1
//! passes: coc /2 f16, gather /2 f16, postfilter /2 f16, combine
//! category: lens

// The lens, in units of the game (1 unit is 1.42 cm): the focal length of a 50 mm lens, the
// diameter of its aperture at f/2, and the width of a full-frame sensor. The largest circle
// in pixels of the half-size target; the taps of the disk; and the depth below which
// nothing blurs (the hands of the player).
const focal: f32 = 3.5;
const aperture: f32 = 1.76;
const sensor: f32 = 2.53;
const max_radius: f32 = 8.0;
const TAPS: i32 = 48;
const hands_near: f32 = 40.0;
const hands_far: f32 = 55.0;

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

fn noise(pixel: vec2f) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * pixel.x + 0.00583715 * pixel.y));
}

// The circle of confusion of a depth as the thin lens gives it, over the largest circle:
// negative in front of the focus, positive behind it. A far focus gives small circles, a
// near one (a table, a face) large ones, as a camera does.
fn circle(z: f32) -> f32 {
    let focus = depth_at(vec2f(0.5, 0.5));
    let pixels_per_unit = target_size().x / sensor;
    let scale = pixels_per_unit * aperture * focal / max(focus - focal, 1.0);
    let c = clamp(scale * (z - focus) / max(z, 1e-3) / max_radius, -1.0, 1.0);
    return c * smoothstep(hands_near, hands_far, z);
}

// The picture at half size with its circle in alpha.
@fragment fn coc(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0).rgb;
    return vec4f(color, circle(depth_at(in.tex)));
}

// The gather over the disk. A tap of the near field counts where its own circle reaches
// this pixel; a tap of the far field counts where its circle and the circle of this pixel
// both reach it, so that a sharp near object stays sharp against a blurred far one.
@fragment fn gather(in: Quad) -> @location(0) vec4f {
    let rcp = target_rcpres();
    let centre = textureSampleLevel(lastpass, point_clamp, in.tex, 0.0);
    let centre_circle = centre.a * max_radius;
    let turn = noise(in.position.xy) * 6.2831853;
    var far_sum = vec3f(0.0);
    var far_weight = 0.0;
    var near_sum = vec3f(0.0);
    var near_weight = 0.0;
    for (var i = 0; i < TAPS; i++) {
        // A Vogel disk: the taps spread evenly, turned by the noise of the pixel.
        let r = sqrt((f32(i) + 0.5) / f32(TAPS)) * max_radius;
        let theta = f32(i) * 2.39996323 + turn;
        let offset = vec2f(cos(theta), sin(theta)) * r;
        let s = textureSampleLevel(lastpass, linear_clamp, in.tex + offset * rcp, 0.0);
        let s_circle = s.a * max_radius;
        let far_reach = max(min(s_circle, centre_circle), 0.0);
        let far_w = saturate((far_reach - r + 2.0) / 2.0);
        let near_w = saturate((-s_circle - r + 2.0) / 2.0);
        far_sum += s.rgb * far_w;
        far_weight += far_w;
        near_sum += s.rgb * near_w;
        near_weight += near_w;
    }
    let far = far_sum / max(far_weight, 1e-4);
    let near = near_sum / max(near_weight, 1e-4);
    // The near field covers the pixel in the measure of the taps that reach it.
    let cover = saturate(near_weight / f32(TAPS) * 2.0);
    return vec4f(mix(far, near, cover), cover);
}

// A 3 x 3 tent against the noise of the disk.
@fragment fn postfilter(in: Quad) -> @location(0) vec4f {
    let rcp = target_rcpres();
    var sum = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0) * 4.0;
    sum += (textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, 0.0), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, 0.0), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(0.0, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(0.0, rcp.y), 0.0)) * 2.0;
    sum += textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, rcp.y), 0.0);
    return sum / 16.0;
}

// The picture and the blurred picture, by the far circle of the pixel and the cover of the
// near field.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let blurred = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    let far_circle = max(circle(depth_at(in.tex)), 0.0) * max_radius;
    let far_w = smoothstep(0.0, 2.0, far_circle);
    let near_w = blurred.a;
    let w = far_w + near_w - far_w * near_w;
    return vec4f(mix(scene.rgb, blurred.rgb, w), scene.a);
}
