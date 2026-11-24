//! interface: mge-xe-wgsl 1
//! passes: underwaterdust
//! mgeflags: 16
//! variables: sphereScale f32 = 0.1, density f32 = 15, spherePos vec4 = -24900 -13000 0 10

// Underwater dust raymarching by vtastek v0.01a: the wgpu renderer's port of underwaterdust.fx.
// A ray from the eye marches through a grid of small moving spheres; a hit under the water level
// and in front of the scene shows a speck of dust in the light of the sun. Not under the water
// (mgeflags 16), the shader does not run.
// The .fx reads depthframe with a linear filter; here it is read with point_clamp, the one filter
// that depthframe takes.

const MAX_STEPS: i32 = 200;
const MAX_DIST: f32 = 1600.0;
const SURFACE_DIST: f32 = 0.01;

fn sample0(tex: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, point_clamp, tex, 0.0);
}

fn toView(tex: vec2f, invproj: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let xy = depth * (tex - 0.5) * invproj;
    return vec3f(xy, depth);
}

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn hash3D(cell: vec3f) -> f32 {
    var p = fract(cell * 0.3183099 + 0.1);
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

// The distance from p (relative to the eye) to the nearest sphere of the grid in w, and in z the
// part of the light that the speck gets.
fn GetDist(p: vec3f) -> vec4f {
    let eyepos = u.eyepos.xyz;
    let cellIndex = floor((p + eyepos) / (density() * 2.0));

    let randomSeed = hash3D(cellIndex);
    let randomSeed2 = hash3D(cellIndex + 1.0);
    let randomSeed3 = hash3D(cellIndex + 2.0);

    let timeFactor = u.sunvec.w + randomSeed * 6.2831;

    let sphere = spherePos();
    let sphereOffset = 2.0 * vec3f(
        sin(sphere.x * 0.5 + timeFactor) * 0.5,
        cos(sphere.y * 0.4 + timeFactor * 1.3) * 0.5,
        sin(sphere.z * 0.6 + timeFactor * 0.9) * 0.5
    );

    var q = p - (sphere.xyz + 2.0 + sphereOffset) + eyepos;
    let sr = 1.0;

    // HLSL's fmod and WGSL's % both truncate; abs(q) is not negative.
    q = abs(q) % vec3f(density() * 2.0) - density();

    let sphereDist = length(q) - (sphereScale() * mix(1.0, 5.0, randomSeed3));

    let sp = step(0.9, randomSeed2 % 2.0) * smoothstep(0.0, 1.0, dot(sphereOffset, vec3f(1.0)));
    let dp = sphereDist;

    let uv = (q.xz / sr) * 0.5 + 0.5;

    return vec4f(uv, sp, dp);
}

fn RayMarch(ro: vec3f, rd: vec3f) -> f32 {
    var dO = 0.0;
    for (var i = 0; i < MAX_STEPS; i++) {
        let p = ro + dO * rd;
        let dS = GetDist(p).w;
        dO += dS;
        if dS < SURFACE_DIST || dO > MAX_DIST {
            break;
        }
    }
    return dO;
}

@fragment fn underwaterdust(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let res = rcpres();
    let invproj = 2.0 * tan(0.5 * radians(u.eyepos.w)) * vec2f(1.0, res.x / res.y);

    var col = vec3f(0.0);
    let v = toWorld(tex);

    let dm = length(toView(tex, invproj));
    let ro = vec3f(0.0);
    let rd = normalize(v);

    let d = RayMarch(ro, rd);

    let p = ro + rd * d;

    let scenecol = sample0(tex);
    let w = u.suncol.w - u.eyepos.z;

    let plain = d > MAX_DIST || dm < d || p.z > w;
    if plain {
        col = scenecol.rgb;
    } else {
        let g = GetDist(p);
        let l = max(0.0, dot(u.eyevec.xyz, -normalize(u.sunpos.xyz)));

        col = mix(pow(scenecol.rgb, vec3f(2.2)), u.sunamb.xyz + l * u.suncol.xyz, smoothstep(20.0, 30.0, d) * saturate(g.z - 0.3));
    }

    return vec4f(pow(col, vec3f(select(1.0 / 2.2, 1.0, plain))), 1.0);
}
