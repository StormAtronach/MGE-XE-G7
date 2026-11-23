//! interface: mge-xe-wgsl 1
//! passes: brightpass, warpfx
//! category: none
//! texture: warpfxnorm.dds
//! variables: invisint f32 = 1, warpint f32 = 1

// invis fx 0.1

const wspeed: f32 = -0.3; // file
const thres: f32 = 0.3; // file
const thresm: f32 = 1.3; // file

const PI: f32 = 3.1415926535897932384626433832795;
const TAU: f32 = 6.28318530718;
const MAX_ITER: i32 = 6;

const hscale: f32 = 0.19; // file

fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn cylindrical(tex: vec2f) -> vec2f {
    let worldpos = toWorld(tex);
    let cu = -atan2(worldpos.y, worldpos.x) / PI;
    let cv = -worldpos.z / length(worldpos.xy);
    return vec2f(0.5 * cu + 0.5, hscale * cv);
}

fn waterHighlight(p: vec2f, time: f32) -> f32 {
    var i = p;
    var c = 0.0;
    let foaminess_factor = 1.81;
    let inten = 0.005 * foaminess_factor;

    for (var n = 0; n < MAX_ITER; n++) {
        let t = time * (1.0 - (3.5 / f32(n + 1)));
        i = p + vec2f(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
        c += 1.0 / length(vec2f(p.x / (sin(i.x + t)), p.y / (cos(i.y + t))));
    }
    c = 0.2 + c / (inten * f32(MAX_ITER));
    c = 1.17 - pow(c, 1.4);
    c = pow(abs(c), 8.0);
    return c / sqrt(foaminess_factor);
}

@fragment fn brightpass(in: Quad) -> @location(0) vec4f {
    var col = sample0(lastshader, linear_mirror, in.tex) * 1.4;
    col = max(col - thres, vec4f(0.0));
    col /= 1.0 + col;
    col *= thresm;
    // The file takes the dot of four parts with three: HLSL uses the first three.
    col = mix(col, vec4f(dot(col.rgb, vec3f(0.299, 0.587, 0.114))), 0.7);
    //col.rgb = lerp(col.rgb, float3(0.8,0.6,0.7), saturate(col.rgb * 10));
    return col;
}

@fragment fn warpfx(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let time = u.sunvec.w;
    let isInterior = u.sunamb.w;
    // The file reads the depth with a linear filter at the centre of a pixel: the value of
    // that pixel.
    var dmask = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let inoff = mix(1.0, 0.5, isInterior);
    let fmask = smoothstep(500.0 * inoff, 3000.0 * inoff, dmask);
    let maskint = 1.0 - smoothstep(0.0, 5500.0, dmask);
    dmask = smoothstep(55.0, 60.0, dmask);
    let timetick = time * wspeed;
    let uv = cylindrical(tex);

    let ftime = timetick * 0.4 + 23.0;
    // fmod of HLSL has the sign of its first value, as % of WGSL.
    let p = (uv * TAU) % vec2f(TAU) - 250.0;

    //float middle = abs(saturate(abs(uv.g)) - 1);
    var middle = 1.0 - min(1.0, abs(uv.g));
    middle = pow(middle, 4.0);

    var warpn = sample0(tex1, linear_wrap, uv * vec2f(2.0, -2.0) + vec2f(0.0, timetick)).xy + 0.5;

    warpn = mix(vec2f(1.0), mix(vec2f(1.0, 1.0), warpn * dmask, warpint()), dmask * middle);
    warpn = mix(warpn, vec2f(1.0, 1.0), mix(vec2f(1.0), vec2f(0.95, 0.7), maskint));
    let warpnless = mix(warpn, vec2f(1.0, 1.0), mix(vec2f(1.0), vec2f(0.99, 0.8), maskint));
    let orgorg = sample0(lastshader, linear_mirror, tex);
    let org = sample0(lastshader, linear_mirror, tex * warpnless);
    let px = sample0(lastpass, linear_mirror, tex * warpn);

    let c = waterHighlight(p, ftime);

    let water_color = vec3f(0.0, 0.35, 0.5);

    var colorfog = saturate3(c + water_color);
    colorfog = mix(water_color, colorfog, middle);

    var comb = org.rgb + px.rgb;
    // dot(1.0, warpn) of the file: the sum of the two parts.
    comb = mix(comb, inoff * colorfog, saturate3(fmask + colorfog * 0.0005 + (warpn.x + warpn.y) * 0.0005));

    comb = mix(comb, vec3f(dot(vec3f(0.299, 0.587, 0.114), comb)), 0.6);

    let contrasted = comb * comb * comb * (comb * (comb * 6.0 - 15.0) + 10.0);
    comb = mix(comb, contrasted, 0.5);

    comb = mix(comb, comb * vec3f(0.83, 0.95, 0.87), 3.0);

    comb = mix(orgorg.rgb, comb * 1.2, invisint());
    return vec4f(comb, 1.0);
}
