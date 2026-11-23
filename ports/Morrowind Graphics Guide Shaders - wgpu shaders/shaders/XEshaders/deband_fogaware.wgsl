//! interface: mge-xe-wgsl 1
//! passes: PS_Deband
//! category: none

// Deband shader by haasn
// https://github.com/mpv-player/mpv/blob/master/video/out/opengl/video_shaders.c
// Modified and optimized for ReShade by JPulowski
// http://reshade.me/forum/shader-presentation/768-deband
// 1847.64 - Ported to MGE XE 0.10.0
// intensity tied to interiors and fog
// full intensity for interiors, reduced intensity for close view exteriors if fog is closer then 500 units

const fogun: f32 = 470.0; // file
const Threshold: f32 = 0.006; // file
const Range: f32 = 5.3; // file
const Grain: f32 = 0.006; // file
const drandom: i32 = 3914; // file

fn toView(tex: vec2f) -> vec3f {
    let t = 2.0 * tan(0.5 * radians(u.eyepos.w));
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let x = (tex.x - 0.5) * depth * t;
    let y = (tex.y - 0.5) * depth * t / rcpres().y * rcpres().x;
    return vec3f(x, y, depth);
}

fn rand(x: f32) -> f32 {
    return fract(x * 0.024390243);
}

fn permute(x: f32) -> f32 {
    return ((34.0 * x + 1.0) * x) % 289.0;
}

fn average(pos: vec2f, range: f32, h: ptr<function, f32>) -> vec3f {
    // Helper: Compute a stochastic approximation of the avg color around a pixel

    // Compute a random rangle and distance
    let dist = rand(*h) * range;
    *h = permute(*h);
    let dir = rand(*h) * 6.2831853;
    *h = permute(*h);

    let pt = dist * rcpres();
    let o = vec2f(cos(dir), sin(dir));

    // Sample at quarter-turn intervals around the source pixel
    var r = textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(o.x, o.y), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(-o.y, o.x), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(-o.x, -o.y), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(o.y, -o.x), 0.0).rgb;

    // Return the mean
    return 0.25 * r;
}

@fragment fn PS_Deband(in: Quad) -> @location(0) vec4f {
    let texcoord = in.tex;
    let isInterior = u.sunamb.w;
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;

    // Initialize the PRNG by hashing the position + a random uniform
    let m = vec3f(texcoord, f32(drandom) * 0.0002) + 1.0;
    var h = permute(permute(permute(m.x) + m.y) + m.z);

    // Sample the source pixel
    var col = textureSampleLevel(lastshader, point_wrap, texcoord, 0.0).rgb;

    // Sample the average pixel and use it instead of the original if the difference is below the given threshold
    let avg = average(texcoord, Range, &h);
    let diff = abs(col - avg);
    let dist = length(toView(texcoord));
    var fog = saturate((fogrange - dist) / (fogrange - fogstart));
    fog = saturate(1.0 - fog);
    fog = mix(fog, isInterior, isInterior);
    let fogac = saturate(-fogstart * 0.1 + fogun * 0.1);
    fog = mix(1.0, fog, fogac);

    col = select(avg, col, diff > vec3f(fog * Threshold));

    if Grain > 0.0 {
        // Add some random noise to smooth out residual differences
        var noise: vec3f;
        noise.x = rand(h);
        h = permute(h);
        noise.y = rand(h);
        h = permute(h);
        noise.z = rand(h);
        h = permute(h);

        col += Grain * (noise - 0.5);
    }

    return vec4f(col, 1.0);
}
