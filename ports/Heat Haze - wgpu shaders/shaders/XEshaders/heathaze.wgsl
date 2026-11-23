//! interface: mge-xe-wgsl 1
//! passes: warpfx
//! category: none
//! texture: heathaze_nm.dds
//! variables: warpint f32 = 0.003, dmaskint1 f32 = 5005, dmaskint2 f32 = 9000

// warp fx 0.1

// warpint, dmaskint1 and dmaskint2 are variables: the mod Heat Haze sets them from Lua.
const wspeed: f32 = -0.06; // file

const PI: f32 = 3.1415926535897932384626433832795;

const hscale: f32 = 0.19; // file

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn cylindrical(tex: vec2f) -> vec2f {
    let worldpos = toWorld(tex);
    let u = -atan2(worldpos.y, worldpos.x) / PI;
    let v = -worldpos.z / length(worldpos.xy);
    return vec2f(0.5 * u + 0.5, hscale * v);
}

@fragment fn warpfx(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let time = u.sunvec.w;
    var depth = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let dmask = smoothstep(dmaskint1(), dmaskint2(), depth);
    depth -= dmaskint1();
    //return float4(abs(depth.xxx), 1);
    let timetick = time * wspeed;
    let uv = cylindrical(tex);

    //float middle = abs(saturate(abs(uv.g)) - 1);
    var middle = 1.0 - min(1.0, abs(uv.g));
    middle = pow(middle, 14.0);

    var warpn = textureSampleLevel(tex1, linear_wrap, uv * vec2f(4.0, -4.0) + vec2f(0.0, timetick), 0.0).xy;

    //return float4(warpn, 0, 1);
    //warpn = lerp(0.0, lerp(float2(0.0, 0.0), warpn * dmask, warpint), dmask * middle);
    warpn = warpn * 2.0 - 1.0;
    warpn = warpn * warpint();

    //return dmask * middle;

    warpn *= saturate(depth);

    var color = vec4f(0.0);

    var wtex = tex + warpn * saturate(dmask * middle);
    let wtexd = tex + 10.0 * warpn * saturate(dmask * middle);
    depth = textureSampleLevel(depthframe, point_wrap, wtexd, 0.0).r;
    depth -= dmaskint1();

    let mask = min(dpdx(depth), dpdy(depth)) * saturate(depth);
    depth = max(depth, mask);
    //return depth.xxxx;
    if (depth < 0.001) {
        wtex = tex;
    }
    //return float4(10000 * (wtex-tex),0,1);

        //return depthm2.rrrr;
        color = textureSampleLevel(lastshader, linear_mirror, wtex, 0.0);

    //color = lerp(tex2D(s0, tex), color, dmask * middle);

    //return float4(warpn.rg * 100, 0, 1.0);
    //return float4(wtex.rg,0, 1.0);
    return vec4f(color.rgb, 1.0);
}
