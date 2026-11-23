//! interface: mge-xe-wgsl 1
//! passes: stretch, blur_r_half, rays, blur_t, combine
//! category: none
//! mgeflags: 9
//! disable_sunglare: true

// Volumetric sun shafts - banding removed
// Compatible with MGE XE, screen-space radial method
const N: i32 = 50; // file
const raysunradius: f32 = 0.36; // file
const raystrength: f32 = 1.85; // file
const rayfalloff: f32 = 1.7; // file
const rayfalloffconst: f32 = 0.21; // file
const raysunfalloff: f32 = 2.2; // file
const centervis: f32 = 0.25; // file
const sunrayocclude: f32 = 0.8; // file
const brightnessadd: f32 = 1.15; // file
const offscreenrange: f32 = 0.8; // file
const sundiscradius: f32 = 0.027; // file
const sundiscbrightness: f32 = 1.3; // file
const sundiscdesaturate: f32 = 0.7; // file
const sundiscocclude: f32 = 0.73; // file
const rscale: f32 = 0.5; // file
const scale: f32 = 2.0; // file
const threshold: f32 = 10000000.0; // file

// The values that the effect computes from the variables (its static consts).
struct Statics {
    raspect: f32,
    sundir: vec3f,
    forward: f32,
    sunview: vec2f,
    sunviewhalf: vec2f,
    light: f32,
    sharpness: f32,
    strength: f32,
    suncoldisc: vec3f,
    azi_horizon: f32,
}

fn statics() -> Statics {
    var s: Statics;
    let rcp = rcpres();
    s.raspect = rcp.x / rcp.y;
    s.sundir = -normalize(u.sunpos.xyz);
    s.forward = dot(s.sundir, u.eyevec.xyz);
    let fov = u.eyepos.w;
    let texproj = 0.5 * vec2f(1.0, -rcp.y / rcp.x) / tan(radians(fov * 0.5));
    // mul(float3, mview): a row vector by the rotation of the view.
    let v = u.sunpos.xyz / dot(u.eyevec.xyz, u.sunpos.xyz);
    let sunview_v = v.x * u.mview[0].xyz + v.y * u.mview[1].xyz + v.z * u.mview[2].xyz;
    s.sunview = vec2f(0.5) + sunview_v.xy * texproj;
    s.sunviewhalf = 0.5 * s.sunview;
    let sunvis = u.eyevec.w;
    s.light = 1.0 - pow(1.0 - sunvis, 2.0);
    let fogstart = u.fogcol.w;
    s.sharpness = mix(60.0, 660.0 + 360.0 * s.sundir.z, saturate(fogstart / 480.0));
    s.strength = raystrength * s.light * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.x - 0.5)) * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.y - 0.5));
    let suncol = u.suncol.xyz;
    s.suncoldisc = vec3f(1.0, 0.76 + 0.24 * u.sunpos.z, 0.54 + 0.46 * u.sunpos.z) * saturate3(suncol / max(suncol.r, max(suncol.g, suncol.b)) * (1.0 - sundiscdesaturate) + vec3f(sundiscdesaturate));
    let fogrange = u.fognearcol.w;
    let waterlevel = u.suncol.w;
    s.azi_horizon = normalize(vec2f(4.0 * fogrange, waterlevel - u.eyepos.z)).y;
    return s;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn stretch(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    var depth = 0.0;
    if s.forward < 0.0 {
        let src = scale * in.tex;
        let rcp = rcpres();
        depth = step(threshold, depth_at(src));
        depth += step(threshold, depth_at(src + vec2f(rcp.x, 0.0)));
        depth += step(threshold, depth_at(src + vec2f(0.0, rcp.y)));
        depth += step(threshold, depth_at(src + rcp));
        depth *= 0.25;
    }
    return vec4f(0.0, 0.0, 0.0, depth);
}

@fragment fn blur_r_half(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let radial = normalize(in.tex - s.sunviewhalf) * rcpres().yx;
    var alpha = 0.3333 * pass_at(in.tex).a;
    alpha += 0.2222 * pass_at(in.tex + radial).a;
    alpha += 0.2222 * pass_at(in.tex - radial).a;
    alpha += 0.1111 * pass_at(in.tex + 2.0 * radial).a;
    alpha += 0.1111 * pass_at(in.tex - 2.0 * radial).a;
    return vec4f(0.0, 0.0, 0.0, alpha);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    var dir = in.tex - s.sunview;
    let dist = length(dir * vec2f(1.0, s.raspect));
    dir /= max(dist, 1e-5);

    var l = 0.0;
    for (var i = 1; i <= N; i++) {
        let t = f32(i) / f32(N);
        let offset = t * min(raysunradius, dist);
        let sampleA = pass_at(clamp(in.tex - offset * dir, vec2f(0.0), vec2f(1.0)) * rscale).a;
        let fade = exp(-((dist - offset) / (rayfalloffconst + offset)) * rayfalloff);
        let taper = pow(1.0 - offset / raysunradius, 3.0);
        let bias = 0.25 * (1.0 - t);
        l += sampleA * fade * (taper + bias) / f32(N);
    }

    l *= s.strength * (dist / raysunradius * (1.0 - centervis) + centervis);
    l = saturate(l);

    var col = vec4f(u.suncol.r, 0.8 * u.suncol.g, 0.8 * u.suncol.b, l);
    col = vec4f(col.rgb * (1.0 + brightnessadd * pow(col.a, 3.0)), col.a);

    // subtle dithering to remove banding
    let dither = (fract(sin(dot(in.tex, vec2f(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.001;
    col = vec4f(saturate3(col.rgb + dither), col.a);
    return col;
}

@fragment fn blur_t(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let tangent = normalize(in.tex - s.sunview).yx * vec2f(rcpres().y, -rcpres().x);
    var col = 0.3333 * pass_at(in.tex);
    col += 0.2222 * pass_at(in.tex + tangent);
    col += 0.2222 * pass_at(in.tex - tangent);
    col += 0.1111 * pass_at(in.tex + 2.0 * tangent);
    col += 0.1111 * pass_at(in.tex - 2.0 * tangent);
    return col;
}

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let s = statics();
    let ray = pass_at(in.tex);
    var col = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    col *= saturate(1.0 - sunrayocclude * ray.a);
    col = saturate3(col + ray.rgb * ray.a);
    if s.forward < 0.0 {
        var screendir = in.tex - s.sunview;
        screendir.y *= s.raspect;
        var occl = s.light * step(threshold, depth_at(in.tex));
        occl *= smoothstep(0.0, 1.0, exp2(s.sharpness * (sundiscradius - length(screendir))));
        if occl > 0.004 {
            var scol = s.suncoldisc * sundiscbrightness;
            let azi = normalize(to_world(in.tex)).z;
            occl *= smoothstep(-0.005, 0.010, azi - s.azi_horizon);
            let gb = smoothstep(-0.04, 0.09, azi - s.azi_horizon);
            scol = vec3f(scol.r, scol.g * gb, scol.b * gb);
            col = mix(col, scol, sundiscocclude * occl);
        }
    }
    return vec4f(col, 1.0);
}
