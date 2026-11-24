//! interface: mge-xe-wgsl 1
//! passes: ssao, smartblur, smartblur, combine
//! category: scene
//! priority_adjust: -10000
//! mgeflags: 8
//! texture: MGE/poisson_nrm.dds

const N: i32 = 16;
const R: f32 = 15.0; // file
const multiplier: f32 = 1.8; // file
const occlusion_falloff: f32 = 15.0; // file
const blur_falloff: f32 = 0.06; // file
const blur_radius: f32 = 4.0; // file
const depth_scale: f32 = 10000.0;
const sky: f32 = 1e6;
const M: i32 = 12;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

// The depth with the linear filter of the sampler of the effect: the depth texture takes
// no filter here (32-bit float), so the four texels are mixed by hand.
fn depth_linear(t: vec2f) -> f32 {
    let size = vec2f(textureDimensions(depthframe));
    let p = t * size - 0.5;
    let i = vec2i(floor(p));
    let f = fract(p);
    let limit = vec2i(size) - 1;
    let a = textureLoad(depthframe, clamp(i, vec2i(0), limit), 0).r;
    let b = textureLoad(depthframe, clamp(i + vec2i(1, 0), vec2i(0), limit), 0).r;
    let c = textureLoad(depthframe, clamp(i + vec2i(0, 1), vec2i(0), limit), 0).r;
    let d = textureLoad(depthframe, clamp(i + vec2i(1, 1), vec2i(0), limit), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = depth_linear(tex);
    let xy = depth * (tex - 0.5) * invproj();
    return vec3f(xy, depth);
}

fn from_view(view: vec3f) -> vec2f {
    return (view.xy / view.z) / invproj() + vec2f(0.5, 0.5);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 16>(
        vec3f(-0.00941, -0.00326, -0.05597), vec3f(0.11686, 0.00831, 0.04915),
        vec3f(-0.08125, -0.24638, 0.30141), vec3f(0.35193, 0.29639, 0.47544),
        vec3f(0.32063, -0.70203, -0.40622), vec3f(-0.37344, -0.18112, 0.37140),
        vec3f(-0.73605, -0.39320, 0.04992), vec3f(0.02274, 0.21583, 0.19429),
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    let rnd = textureSampleLevel(tex1, point_wrap, tex / rcp / 8.0, 0.0).xyz * 2.0 - 1.0;
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = reflect(dirs[j] * R, rnd);
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray));
        let diff = pos.z + ray.z - occ.z;
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = blur_radius * (2.0 * data.a - 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) / depth / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let final_ao = fog * multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let result = mix(textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb, u.fognearcol.xyz * (1.0 - fog), final_ao);
    return vec4f(result, 1.0);
}
