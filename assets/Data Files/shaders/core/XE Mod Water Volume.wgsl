//! part: volume

struct Surface {
    world: mat4x4f,
    tint: vec4f,
    anchor: vec4f,
    sky: vec4f,
    flow: vec4f,
    look: vec3f,
    flags: u32,
    // The view and the projection of the surface, relative to the eye (`clip_with`): the
    // projection of the game for the near surfaces, that of the land for the far water.
    clip: mat4x4f,
    far: vec4f,
    params: array<vec4f, 4>,
}
@group(3) @binding(0) var<uniform> s: Surface;

// The textures of the mesh, for a water shader: its base texture (white without one), and its
// second texture (a dark, detail or decal map; transparent black without one, and far away),
// with mip levels, wrapped.
@group(2) @binding(0) var mesh0: texture_2d<f32>;
@group(2) @binding(1) var mesh1: texture_2d<f32>;
@group(2) @binding(2) var mesh_sampler: sampler;

// The light of the frame on the surfaces (`VolumeLight`): the shadow map of the frame, the
// lights of the frame with their clusters (`nirender_wgpu::FrameLights`), the caustics.
struct Light {
    diffuse: vec4f,
    ambient: vec4f,
    position: vec4f,
    direction: vec4f,
    attenuation: vec4f,
    cone: vec4f,
}
struct Clusters {
    grid: vec4u,
    screen: vec4f,
    depth: vec4f,
}
struct Lighting {
    // From a position less the eye to the two layers of the shadow map (`clip_with`).
    shadow0: mat4x4f,
    shadow1: mat4x4f,
    // The constants of the shadow: c, the bias, the scale, and 1 over the resolution.
    esm: vec4f,
    // The direction that the light of the sun goes in; w the strength of the caustics, 0 for
    // none.
    sun: vec4f,
    // The point that the positions of the lights are relative to; w bits: 1 the shadow, 2 the
    // lights.
    origin: vec4f,
}
@group(1) @binding(11) var<uniform> lighting: Lighting;
@group(1) @binding(12) var shadow_atlas: texture_2d<f32>;
@group(1) @binding(13) var<storage, read> lights: array<Light>;
@group(1) @binding(14) var<uniform> clusters: Clusters;
@group(1) @binding(15) var<storage, read> cluster_table: array<vec2u>;
@group(1) @binding(16) var<storage, read> cluster_indexes: array<u32>;

// How much of the sun reaches a place, 0 to 1: the shadow map of the frame, read as the draws
// of the game read it (`extension_shade` of the renderer), so the water has the shadow of the
// shore next to it. All of it outside the map.
fn sunlight_at(place: vec3f) -> f32 {
    if (u32(lighting.origin.w) & 1u) == 0u {
        return 1.0;
    }
    let s0 = clip_with(lighting.shadow0, place);
    let s1 = clip_with(lighting.shadow1, place);
    let p0 = vec3f(s0.xy, s0.z / s0.w);
    let p1 = vec3f(s1.xy, s1.z / s1.w);
    let dz = shadow_delta(p0, p1);
    if dz >= 0.0 {
        return 1.0;
    }
    // shadowESM, and the fade at the edge of the outer layer.
    let fade = clamp(25.0 * (1.0 - abs(p1.xy)), vec2f(0.0), vec2f(1.0));
    return 1.0 - shadow_esm(dz) * fade.x * fade.y;
}

// The library of the shadow hooks (`core_mods.rs`): the shadow map of the frame and its
// values. The hooks join this shader after its text.
fn shadow_map_depth(layer: u32, uv: vec2f) -> f32 {
    if layer == 0u {
        return textureSampleLevel(shadow_atlas, clamp_sampler, vec2f(uv.x * 0.5, uv.y), 0.0).r;
    }
    return textureSampleLevel(shadow_atlas, clamp_sampler, vec2f(0.5 + uv.x * 0.5, uv.y), 0.0).r;
}
fn shadow_texel() -> f32 {
    return lighting.esm.w;
}
fn shadow_esm_scale() -> f32 {
    return lighting.esm.z;
}
fn shadow_esm_values() -> vec2f {
    return lighting.esm.xy;
}

// The light of a point or spot light at a distance, for a place in `direction` (from the place
// to the light): the sum of its falloff (the numbers of the game, or those of the light for
// each pixel, whose middle number is 0), going to zero over the last quarter of its reach
// (`nirender_wgpu::light_reach`), where its clusters end, and the cone of a spot light.
fn light_strength(light: Light, distance: f32, direction: vec3f) -> f32 {
    var reach = light.direction.w;
    if light.cone.w > 0.0 {
        reach = 1.0 / light.cone.w;
    } else if light.cone.z > 0.0 {
        reach = light.cone.z;
    }
    reach = max(min(reach, LIGHT_REACH_MAX), 0.0);
    if distance >= reach {
        return 0.0;
    }
    let a = light.attenuation;
    var fade = clamp(4.0 * distance / reach - 3.0, 0.0, 1.0);
    fade = 1.0 - fade * fade;
    return spot_cone(light, direction) * fade * fade / max(a.x + a.y * distance + a.z * distance * distance, 1e-4);
}

// The lamps and fires of the frame on the surface at a pixel: in `glint` the mirror image of
// each light in the ripples, in `glow` the light that the body of the water takes from them.
// The lights are those of the cluster of the pixel: each light that reaches it.
struct Lit {
    glint: vec3f,
    glow: vec3f,
}

fn point_lights(place: vec3f, eye_vec: vec3f, normal: vec3f, face: vec3f, pixel: vec2f, view_z: f32) -> Lit {
    var lit: Lit;
    if (u32(lighting.origin.w) & 2u) == 0u || clusters.grid.w == 0u {
        return lit;
    }
    let tile = vec2u(clamp(vec2i(pixel * clusters.screen.xy), vec2i(0), vec2i(clusters.grid.xy) - 1));
    let slice = u32(clamp(log(max(view_z, clusters.depth.z)) * clusters.depth.x + clusters.depth.y, 0.0, f32(clusters.grid.z) - 1.0));
    let cell = cluster_table[(slice * clusters.grid.y + tile.y) * clusters.grid.x + tile.x];
    for (var i = 0u; i < cell.y; i++) {
        let light = lights[cluster_indexes[cell.x + i]];
        // The sun has its own glint.
        if light.position.w == 3.0 {
            continue;
        }
        let to_light = light.position.xyz + lighting.origin.xyz - place;
        let distance = length(to_light);
        let direction = to_light / max(distance, 1e-3);
        let color = light.diffuse.rgb * light_strength(light, distance, direction);
        let mirrored = clamp(dot(-eye_vec, reflect(-direction, normal)), 0.0, 1.0);
        lit.glint += color * (pow(mirrored, 90.0) + 0.06 * pow(mirrored, 6.0));
        lit.glow += color * clamp(dot(direction, face), 0.0, 1.0);
    }
    return lit;
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) facing: vec3f,
    @location(2) clip_w: f32,
    @location(3) color: vec4f,
    // The colour of the water.
    @location(4) tint: vec3f,
    // The texture coordinates of the mesh: the first set, and the second (the first again
    // when the mesh has one).
    @location(5) uv: vec4f,
}

// The normal, the colour and the coordinates of a vertex that has none come from a buffer of
// zeros.
@vertex fn vs(@location(0) local: vec3f, @location(1) normal: vec3f, @location(2) color: vec4f, @location(3) uv0: vec2f, @location(4) uv1: vec2f) -> Out {
    var out: Out;
    let bits = s.flags;
    out.world = (s.world * vec4f(local, 1.0)).xyz;
    // A mesh without normals faces up.
    out.facing = select(vec3f(0.0, 0.0, 1.0), (s.world * vec4f(normal, 0.0)).xyz, (bits & 1u) != 0u);
    // A D3DCOLOR has blue in its first byte.
    out.color = select(vec4f(1.0), color.zyxw, (bits & 2u) != 0u);
    out.tint = s.tint.rgb;
    out.uv = vec4f(uv0, uv1);
    out.position = clip_with(s.clip, out.world);
    out.clip_w = out.position.w;
    return out;
}

// Water among the distant statics (`WaterVolumeDistantVS`): the vertex of a distant static.
// The generator writes the colour of the water into the vertex colour, and the opacity of the
// mesh's vertex into its alpha when the look asks for it.
@vertex fn vs_far(@location(0) position: vec4f, @location(1) normal: vec4f, @location(2) color: vec4f, @location(3) uv: vec2f) -> Out {
    var out: Out;
    out.world = (s.world * vec4f(position.xyz, 1.0)).xyz;
    out.facing = (s.world * vec4f(normal.xyz * 2.0 - 1.0, 0.0)).xyz;
    out.color = vec4f(1.0, 1.0, 1.0, color.w);
    out.tint = color.zyx;
    out.uv = vec4f(uv, uv);
    out.position = clip_with(s.clip, out.world);
    out.clip_w = out.position.w;
    return out;
}

// The depth of a surface only, for the count of the dry spaces behind it.
@fragment fn fs_depth() -> @location(0) vec4f {
    return vec4f(0.0);
}

// The surface at a pixel, for a water shader (`docs/water-shaders.md`).
struct WaterSurface {
    // The place in the world.
    world: vec3f,
    // The way the mesh faces, length 1 (up for a mesh without normals).
    facing: vec3f,
    // The colour of the water: the emissive colour of the material; black for the usual.
    tint: vec3f,
    // The vertex colour of the mesh (white for a mesh without one; from the distant land,
    // white with the alpha of the mesh when the look has `opacity=vertex`).
    color: vec4f,
    // The texture coordinates of the mesh: the first set in xy, the second in zw.
    uv: vec4f,
    // The place in the plane of the ripples: that of the sea for a level surface.
    flat: vec3f,
    // The place in the picture (0 to 1), the pixel, and the view depth.
    screen: vec2f,
    pixel: vec2f,
    view_depth: f32,
}

// The look of a surface (`MGE_WaterLookSet`, the look line of a mesh).
struct WaterLook {
    // The drift of the ripples in the world, in units for each second; their speed and their
    // size (1 is the standard).
    drift: vec2f,
    speed: f32,
    scale: f32,
    glow: f32,
    opacity: f32,
    // Over how many units what is under the surface fades into the colour of deep water.
    clarity: f32,
    // The tint of the vertex colour, when the look takes it.
    tint: vec3f,
    reflects_scene: bool,
    distant: bool,
    no_depth_test: bool,
    // The free values of the look.
    p0: vec4f,
    p1: vec4f,
    p2: vec4f,
    p3: vec4f,
}

// The result of the standard shading, before the glow and the opacity.
struct WaterShade {
    colour: vec3f,
    eye_vec: vec3f,
    dist: f32,
    // rgb what the fog adds, a what is left of the colour of the thing.
    fog: vec4f,
    // The normal of the ripples as on a level surface, and the way the surface faces (to the
    // side of the eye).
    ripple: vec3f,
    face: vec3f,
    // How far the view ray goes through the water to what is behind the surface, and how
    // deep that is under the surface; 4000 or more where the water is deep.
    ray_depth: f32,
    water_depth: f32,
    // How much of the sun reaches the point, 0 to 1.
    sun: f32,
}

fn surface_look(surface: WaterSurface) -> WaterLook {
    var look: WaterLook;
    let bits = s.flags;
    look.drift = s.flow.xy;
    look.speed = s.flow.z;
    look.scale = s.flow.w;
    look.glow = s.look.x;
    look.opacity = s.look.y * select(1.0, surface.color.a, (bits & 8u) != 0u);
    look.clarity = max(s.look.z, 1.0);
    look.tint = select(vec3f(1.0), surface.color.rgb, (bits & 4u) != 0u);
    look.reflects_scene = s.tint.w > 0.5;
    look.distant = s.far.x > 0.5;
    look.no_depth_test = (bits & 16u) != 0u;
    look.p0 = s.params[0];
    look.p1 = s.params[1];
    look.p2 = s.params[2];
    look.p3 = s.params[3];
    return look;
}

// The time of the water, in seconds.
fn water_time() -> f32 {
    return water.time.x;
}

// A colour with the fog of the shade on it.
fn fog_apply(colour: vec3f, fog: vec4f) -> vec3f {
    return fog.a * colour + fog.rgb;
}

// The colour of the water: a water blue for the usual colour.
fn glow_colour(tint: vec3f) -> vec3f {
    return select(vec3f(0.25, 0.5, 0.6), tint, dot(tint, vec3f(1.0)) > 0.0);
}

// The ripples of the sea with the drift, the speed and the size of the look
// (`surfaceRipples`): the normal as on a level surface, z up. The drift moves the
// coordinates; it is wrapped, so it keeps its precision over hours.
fn standard_ripple(surface: WaterSurface, look: WaterLook) -> vec3f {
    let texcoords = vec4f(surface.flat.xy / 3900.0, surface.flat.xy / 527.0);
    let distance = length(surface.world - u.eye.xyz);
    let t = 0.4 * water.time.x * look.speed;
    let moved = look.drift * water.time.x;
    let scale = max(look.scale, 1e-3);
    let far = textureSample(waves, wave_sampler, vec3f(texcoords.xy / scale + fract(moved / 3900.0), t)).rg;
    var close = textureSample(waves, wave_sampler, vec3f(texcoords.zw / scale + fract(moved / 527.0), t)).rg;
    if water.wave.w > 0.5 {
        close += textureSampleLevel(rain, wave_sampler, texcoords.zw, 0.0).ba;
        let at = (surface.flat.xy - water.wave.xy) / 1280.0;
        if all(at >= vec2f(0.0)) && all(at <= vec2f(1.0)) {
            close += textureSampleLevel(player_waves, clamp_sampler, at, 0.0).ba;
        }
    }
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

// The standard shading, with the standard ripples.
fn shade_water(surface: WaterSurface, look: WaterLook) -> WaterShade {
    return shade_water_rippled(surface, look, standard_ripple(surface, look));
}

// The standard shading up to the look, with the ripples given: `ripple` is the normal of the
// ripples as on a level surface, z up, of length 1.
fn shade_water_rippled(surface: WaterSurface, look: WaterLook, ripple: vec3f) -> WaterShade {
    var eye_vec = surface.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    let uv = surface.screen;
    let w = surface.view_depth;
    // The tests of the branches are of uniforms only: the caustics take a change from pixel
    // to pixel.
    let far = s.far.x > 0.5;
    let no_depth = (s.flags & 16u) != 0u;
    // The way the surface faces, turned to the side of the eye; the ripples tilted to it.
    let face = select(surface.facing, -surface.facing, dot(surface.facing, eye_vec) > 0.0);
    let fog = fog_water(eye_vec, dist);
    let normal = tilt_to(face, ripple);
    let tint = surface.tint;
    // In the shadow of a thing the water has no glint of the sun, and its depth has no share
    // of the sun.
    let sun = sunlight_at(surface.world);
    let deep = fog.a * depth_base_sun(tint, sun) * look.tint + fog.rgb;
    let reffactor = (water.sun_col.w * dist + 0.1) * ripple.xy;
    // The caustics on what is under a surface that faces up, where the sun reaches it: the
    // point under the surface is that of the depth along the ray of the eye.
    // A surface drawn without the depth test (the water of a well, under the ground) has no
    // depth of what is under it: it shows deep water.
    var bed_light = 1.0;
    if lighting.sun.w > 0.0 && !no_depth {
        let first = max(0.0, ground_at(uv + reffactor.yx / w, far) - w);
        let ray = first / max(dot(eye_vec, vec3f(water.view[0].z, water.view[1].z, water.view[2].z)), 1e-3);
        let bed = surface.world + eye_vec * ray;
        let under = vec3f(bed.xy, bed.z - surface.world.z);
        bed_light = 1.0 + caustic_light(under, lighting.sun.xyz, water.time.x, dist + ray, lighting.sun.w) * sun * clamp(4.0 * face.z - 3.0, 0.0, 1.0);
    }
    var below = Refraction(deep, 0.0, 4000.0);
    if !no_depth {
        below = refraction(uv, w, reffactor, eye_vec, ripple, deep, tint, look.clarity, look.tint, far, bed_light);
    }
    // Far away and at a flat angle the surface is calm: a ripple is smaller than a pixel.
    let calm = (1.0 + dot(eye_vec, face)) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0));
    var adjust = mix(0.1 * face, normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, face, calm);
    // The ripples tilt the reflected direction only a little; a direction into the surface
    // is turned back out of it.
    var reflect_dir = reflect(eye_vec, normalize(mix(face, normal, 0.35 * (1.0 - calm))));
    reflect_dir -= 2.0 * min(0.0, dot(reflect_dir, face)) * face;
    reflect_dir = normalize(reflect_dir);
    var reflected = mix(sky_color(reflect_dir), s.sky.rgb, s.sky.a);
    // Far water reflects what is in the picture out to its range, and fades out over the
    // last quarter of a cell.
    let scene_weight = select(1.0, clamp((s.far.z - dist) / 2048.0, 0.0, 1.0), far);
    if s.tint.w > 0.5 && scene_weight > 0.0 {
        let seen = reflect_scene(surface.world, reflect_dir, far);
        reflected = mix(reflected, seen.rgb, seen.a * scene_weight);
    }
    reflected = mix(fog.rgb, reflected, fog.a);
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(below.color, reflected, fresnel);
    result += sun_glint(eye_vec, normal, fog.a) * sun;
    // The lamps and fires near the surface: their glint, and a little of their light in the
    // body of the water. Not for far water: the lights of the frame are near the camera.
    if !far {
        let lit = point_lights(surface.world, eye_vec, normal, face, surface.pixel, w);
        result += (lit.glint + 0.08 * lit.glow * glow_colour(tint)) * fog.a;
    }
    result = mix(result, below.color, below.shore * fog.a);
    var shade: WaterShade;
    shade.colour = result;
    shade.eye_vec = eye_vec;
    shade.dist = dist;
    shade.fog = fog;
    shade.ripple = ripple;
    shade.face = face;
    // From the difference of the view depths to the way along the ray.
    shade.ray_depth = below.depth_first / max(dot(eye_vec, vec3f(water.view[0].z, water.view[1].z, water.view[2].z)), 1e-3);
    shade.water_depth = shade.ray_depth * abs(dot(eye_vec, face));
    shade.sun = sun;
    return shade;
}

// The last steps of the shading, which belong to the look: the light of the surface's own,
// in the colour of the water, and what is behind a surface that is not all water.
fn finish_water(surface: WaterSurface, shade: WaterShade, look: WaterLook) -> vec4f {
    var result = shade.colour + look.glow * glow_colour(surface.tint) * shade.fog.a;
    result = mix(textureSampleLevel(picture, clamp_sampler, surface.screen, 0.0).rgb, result, look.opacity);
    return vec4f(result, 1.0);
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    var eye_vec = in.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    let uv = in.position.xy / water.size.xy;
    let w = in.clip_w;
    // Water among the distant statics has no depth test of the pass: its depth is that of the
    // distant land, and the game's is another. It is behind what the game drew and behind the
    // distant land, and not nearer than the view range of the game for a mesh that the game
    // draws itself.
    let far = s.far.x > 0.5;
    if far && (w < s.far.y || w > distance_at(uv) || w > land_distance_at(uv) * 1.0005 + 1.0) {
        discard;
    }
    var facing = vec3f(0.0, 0.0, 1.0);
    if dot(in.facing, in.facing) > 0.25 {
        facing = normalize(in.facing);
    }
    // The ripples lie in the plane of the surface: the place, turned about the anchor of the
    // mesh by the turn that takes the surface up. A level surface has the coordinates of the
    // sea, so the two agree at a seam.
    let upward = select(facing, -facing, facing.z < 0.0);
    let flat = tilt_to(vec3f(-upward.x, -upward.y, upward.z), in.world - s.anchor.xyz) + s.anchor.xyz;
    // The debug view `fog`, before the look's own shader.
    if fog_view_on() {
        return water_fog_view(eye_vec, dist);
    }
    // From under the water each surface has the look from below, as the plane has it.
    if water.modes.y > 0.5 {
        return underwater(uv, w, vec4f(flat.xy / 3900.0, flat.xy / 527.0), flat.xy, eye_vec, dist);
    }
    var surface: WaterSurface;
    surface.world = in.world;
    surface.facing = facing;
    surface.tint = in.tint;
    surface.color = in.color;
    surface.uv = in.uv;
    surface.flat = flat;
    surface.screen = uv;
    surface.pixel = in.position.xy;
    surface.view_depth = w;
    return water_shader(surface);
}
//! part: dry

@vertex fn vs_zero(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.5, 1.0);
}

@fragment fn fs_none() -> @location(0) vec4f {
    return vec4f(0.0);
}

struct Mask {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
}

// A corner of a dry space, with the clip depth of the plane of the sea (`water.rs`).
@vertex fn vs_mask_plane(@location(0) world: vec3f) -> Mask {
    var out: Mask;
    out.world = world;
    out.position = clip_of(world);
    out.position.z = out.position.w * water.land.z + water.land.w;
    return out;
}

// The same with the depth of the game, for the surfaces of volumes.
@vertex fn vs_mask(@location(0) world: vec3f) -> Mask {
    var out: Mask;
    out.world = world;
    out.position = clip_of(world);
    return out;
}

// Only the faces on the far side of the plane of the sea (the level less 1) from the eye.
@fragment fn fs_mask_plane(in: Mask) -> @location(0) vec4f {
    let level = water.world.z - 1.0;
    if (in.world.z - level) * (u.eye.z - level) > 0.0 {
        discard;
    }
    return vec4f(0.0);
}
