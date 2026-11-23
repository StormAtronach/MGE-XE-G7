//! interface: mge-xe-wgsl 1
//! passes: ao /2 f16, blur_x /2 f16, blur_y /2 f16, combine
//! category: scene
//! priority_adjust: -10000
//! mgeflags: 8

// The radius in units of the game, the strength, the slices and the steps of each side.
const R: f32 = 48.0;
const strength: f32 = 1.4;
const SLICES: i32 = 4;
const STEPS: i32 = 8;
const PI: f32 = 3.14159265;
const depth_scale: f32 = 10000.0;
const sky: f32 = 1e6;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

// The position in the view, with y down as the picture has it.
fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    return vec3f(depth * (tex - 0.5) * invproj(), depth);
}

// A noise without a texture: interleaved gradient noise over the pixels of the target.
fn noise(pixel: vec2f, seed: f32) -> f32 {
    let p = pixel + seed * vec2f(47.0, 17.0);
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

@fragment fn ao(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let pos = to_view(tex);
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - u.suncol.w) < 0.0 {
        return vec4f(1.0, pos.z / depth_scale, 0.0, 1.0);
    }
    // The normal from the depth: the smaller difference on each axis, so that an edge does
    // not give a wrong one.
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    let dx = select(right, left, length(left) < length(right));
    let dy = select(down, up, length(up) < length(down));
    let view_dir = normalize(-pos);
    var normal = normalize(cross(dy, dx));
    if dot(normal, view_dir) < 0.0 {
        normal = -normal;
    }
    // The radius in the picture: in the units of the texture coordinates, by axis.
    let radius = R / (pos.z * invproj());
    let pixel = in.position.xy;
    let turn = noise(pixel, 0.0);
    let jitter = noise(pixel, 1.0);
    var visibility = 0.0;
    for (var s = 0; s < SLICES; s++) {
        let phi = (f32(s) + turn) * PI / f32(SLICES);
        let slice = vec2f(cos(phi), sin(phi));
        // The plane of the slice holds the view direction; the normal projected into it.
        let direction = vec3f(slice, 0.0);
        let ortho = direction - dot(direction, view_dir) * view_dir;
        let axis = normalize(cross(ortho, view_dir));
        let projected = normal - axis * dot(normal, axis);
        let projected_length = length(projected);
        let cos_n = clamp(dot(projected, view_dir) / max(projected_length, 1e-5), -1.0, 1.0);
        let n = sign(dot(ortho, projected)) * acos(cos_n);
        // The horizons: the highest sample on each side, as the cosine of its angle to the
        // view direction; a sample past the radius counts as the floor of the slice. The
        // steps are denser near the pixel, and the first is one texel of the target at
        // least: a sample on the texel of the pixel itself has no direction.
        var horizon0 = cos(n + 0.5 * PI);
        var horizon1 = cos(n - 0.5 * PI);
        let texel = min(target_rcpres().x / radius.x, target_rcpres().y / radius.y);
        for (var i = 0; i < STEPS; i++) {
            let t = (f32(i) + jitter) / f32(STEPS);
            let offset = slice * radius * max(t * t, texel);
            let p0 = to_view(tex + offset) - pos;
            let p1 = to_view(tex - offset) - pos;
            let d0 = length(p0);
            let d1 = length(p1);
            let w0 = select(saturate(2.0 - 2.0 * d0 / R), 0.0, d0 < 1e-3);
            let w1 = select(saturate(2.0 - 2.0 * d1 / R), 0.0, d1 < 1e-3);
            horizon0 = max(horizon0, mix(cos(n + 0.5 * PI), dot(p0, view_dir) / max(d0, 1e-5), w0));
            horizon1 = max(horizon1, mix(cos(n - 0.5 * PI), dot(p1, view_dir) / max(d1, 1e-5), w1));
        }
        var h0 = -acos(clamp(horizon1, -1.0, 1.0));
        var h1 = acos(clamp(horizon0, -1.0, 1.0));
        h0 = n + max(h0 - n, -0.5 * PI);
        h1 = n + min(h1 - n, 0.5 * PI);
        let arc0 = (cos_n + 2.0 * h0 * sin(n) - cos(2.0 * h0 - n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin(n) - cos(2.0 * h1 - n)) * 0.25;
        visibility += projected_length * (arc0 + arc1);
    }
    visibility = saturate(visibility / f32(SLICES));
    return vec4f(pow(visibility, strength), pos.z / depth_scale, 0.0, 1.0);
}

// A blur along one axis that keeps the edges: a tap counts less the more its depth differs.
fn blur(tex: vec2f, step: vec2f) -> vec4f {
    let centre = textureSampleLevel(lastpass, point_clamp, tex, 0.0);
    let depth = centre.g;
    var weights = array<f32, 5>(0.2270, 0.1945, 0.1216, 0.0540, 0.0162);
    var total = centre.r * weights[0];
    var amount = weights[0];
    for (var i = 1; i < 5; i++) {
        for (var side = -1.0; side <= 1.0; side += 2.0) {
            let s = textureSampleLevel(lastpass, point_clamp, tex + step * f32(i) * side, 0.0);
            let w = weights[i] * exp2(-abs(s.g - depth) / max(depth, 1e-4) / 0.03);
            total += s.r * w;
            amount += w;
        }
    }
    return vec4f(total / amount, depth, 0.0, 1.0);
}

@fragment fn blur_x(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(target_rcpres().x, 0.0));
}

@fragment fn blur_y(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(0.0, target_rcpres().y));
}

// The picture darkened by the occlusion; the fog of the near cells takes it away with the
// distance, as the fog takes the light away.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let occlusion = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0).r;
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(scene.rgb * mix(1.0, occlusion, fog), scene.a);
}
