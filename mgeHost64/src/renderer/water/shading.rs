//! The water shading that each surface of water shares: the values of the frame, the fog
//! over the water, the normal of the waves and the ripples, the refraction with the shore,
//! the light of the sun on the water, and the look from under the water. `XE Mod Water.fx`
//! and `XE Common.fx`. The plane in place of the water plane of the game (`water.rs`) and the
//! surfaces of water volumes (`volume.rs`) have their own entry points after this source.

/// The values of a frame for the water shader (`water` in group 1; the game view of the view
/// group has the camera, the fog and the scattering). The names are those of the uniforms of
/// `XE Mod Water.fx`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::renderer) struct Uniforms {
    pub view: [f32; 16],
    /// The translation of the plane: the camera's x and y, and the water level.
    pub world: [f32; 4],
    /// x: the time of the simulation.
    pub time: [f32; 4],
    /// The direction to the sun, and its visibility.
    pub sun_pos: [f32; 4],
    /// The colour of the sun with its visibility in it, and the wind factor.
    pub sun_col: [f32; 4],
    /// The sky colour, and the good weather value.
    pub sky_col: [f32; 4],
    /// x: the debug term (`MGE_XE_WATER_DEBUG`); y: 1 under water.
    pub modes: [f32; 4],
    /// The two numbers of the depth of the projection of the game, for the distance of a
    /// pixel from its depth; then the most height for its width of the blur that is sharp
    /// at the waterline.
    pub projection: [f32; 4],
    /// The size of the picture, the view depth where the plane ends (0 for no end), and
    /// the blur of the reflection: 0 for none, 1 for the six reads of MGE XE, 2 for the
    /// blur that is sharp at the waterline.
    pub size: [f32; 4],
    /// The dynamic waves: the world position of the corner of the texture of the ripples
    /// of the player, the wave height, and 1 when the dynamic waves are on.
    pub wave: [f32; 4],
    /// The mirror: the two numbers of the depth of its projection, the first number of the
    /// projection (the picture width of a view angle), and the widest angle of the blur
    /// that is sharp at the waterline, in radians.
    pub mirror: [f32; 4],
    /// The two numbers of the depth of the projection of the distant land (`before_world`),
    /// for the distance of a pixel of its depth; then those of the clip depth of the plane:
    /// the near plane of the game and the far plane of the distant land.
    pub land: [f32; 4],
}

/// The caustics: the light that the waves of a surface focus on what is under it. One
/// function for the sea (`caustics.rs`) and for the surfaces of water volumes; the shader
/// that has it binds `waves` and `wave_sampler`.
pub(in crate::renderer) const CAUSTICS: &str = "
// What the caustics add to 1 for a point under a surface, before the colour of the point is
// multiplied with it (`XE Mod Caustics.fx`): brighter where the waves focus the light of the
// sun, and darker in the mean by as much as the light is there, less with the depth of the
// point under the surface. `under` is the point less the level of the surface over it, `sun`
// the direction that the light of the sun goes in, `time` that of the waves, `distance` how
// far the point is from the eye, and `strength` the strength of the caustics.
fn caustic_light(under: vec3f, sun: vec3f, time: f32, distance: f32, strength: f32) -> f32 {
    // Where the ray of the sun through the point meets the surface: the waves there. A sun at
    // the horizon or under it is taken as a little above it.
    let sunray = under - sun * (under.z / min(sun.z, -0.05));
    var caust = strength * textureSample(waves, wave_sampler, vec3f(sunray.xy / 1104.0, 0.4 * time)).b;
    caust *= clamp(125.0 / distance * min(fwidth(sunray.x), fwidth(sunray.y)), 0.0, 1.0);
    return (caust - 0.3 * clamp(strength, 0.0, 1.0)) * clamp(exp(under.z / 400.0), 0.0, 1.0) * clamp(under.z / -30.0, 0.0, 1.0);
}
";

/// The strength of the caustics for the light of the frame: from the setting (0 to 100),
/// the visibility of the sun and the far fog colour, which has the light of the sky.
pub(in crate::renderer) fn caustic_strength(intensity: f32, sun_vis: f32, fog_color_far: [f32; 3]) -> f32 {
    let sunlight = 1.0 - (1.0 - sun_vis).powi(2);
    let sky = (fog_color_far[0] * fog_color_far[0] + fog_color_far[1] * fog_color_far[1] + fog_color_far[2] * fog_color_far[2]).sqrt();
    0.05 * intensity * (0.75 * sunlight + 0.35 * sky).clamp(0.0, 1.0)
}

/// The WGSL of the shared shading: the values and the textures of the frame for the water in
/// group 1, and the functions. The shader starts with the view group (`view::wgsl`).
pub(super) const COMMON: &str = "
// The view group (`view.rs`, group 0) comes before this code: the game view in `u`, with the
// camera, the exponential fog (`u.fog.xy`), the far fog colour (`u.fog_color`) and the
// scattering (`u.sky.w` 1 when it is on). The values of the water are in `water`, group 1.
struct Water {
    view: mat4x4f,
    world: vec4f,
    time: vec4f,
    sun_pos: vec4f,
    sun_col: vec4f,
    sky_col: vec4f,
    modes: vec4f,
    projection: vec4f,
    size: vec4f,
    wave: vec4f,
    mirror: vec4f,
    land: vec4f,
}
@group(1) @binding(0) var<uniform> water: Water;
@group(1) @binding(1) var reflection: texture_2d<f32>;
@group(1) @binding(2) var clamp_sampler: sampler;
@group(1) @binding(3) var waves: texture_3d<f32>;
@group(1) @binding(4) var wave_sampler: sampler;
@group(1) @binding(5) var picture: texture_2d<f32>;
@group(1) @binding(6) var depth: texture_depth_2d;
// The ripples of the rain and of the player (ripples.rs); flat without the dynamic waves.
@group(1) @binding(7) var rain: texture_2d<f32>;
@group(1) @binding(8) var player_waves: texture_2d<f32>;
// The depth of the mirror pass: how far the thing in the reflection is.
@group(1) @binding(9) var mirror_depth: texture_depth_2d;
// The depth of the distant land (`SceneView::before_depth`); 1 everywhere without it.
@group(1) @binding(10) var land_depth: texture_depth_2d;
// Bindings 11 to 16: the light of the frame for the surfaces of water volumes (`volume.rs`).

// fogColourWater: the exponential fog with the scattering of the atmosphere. rgb is the
// colour that the fog adds, a is what is left of the colour of the thing.
fn fog_water(direction: vec3f, distance: f32) -> vec4f {
    var fog_distance = (distance - u.fog.x) / max(u.fog.y, 1.0);
    let fog = clamp(exp(-fog_distance), 0.0, 1.0);
    fog_distance = clamp(0.224 * fog_distance, 0.0, 1.0);
    var added = u.fog_color.rgb * (1.0 - fog);
    if u.sky.w > 0.5 && water.sky_col.w > 0.001 && u.eye.z > -1.0 {
        let sun_cos = dot(direction, water.sun_pos.xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * u.sun_altitude.z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(u.inscatter.rgb, u.outscatter.rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (u.sun_altitude.x + 0.7 * mie);
        lost = (1.0 - exp(-fog_distance * lost)) / lost;
        let scattered = (0.125 * mie + u.sky.rgb * rayleigh) * lost * (1.17 * air + 0.89) * u.sun_altitude.y;
        added = mix(added, scattered, water.sky_col.w);
    }
    return vec4f(added, fog);
}

// getFinalWaterNormal: two reads of the wave texture, the far one past 8000 units.
fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.4 * water.time.x;
    let far = textureSample(waves, wave_sampler, vec3f(texcoords.xy, t)).rg;
    var close = textureSample(waves, wave_sampler, vec3f(texcoords.zw, t)).rg;
    if water.wave.w > 0.5 {
        // The ripples of the rain repeat over the water. Those of the player are in the
        // world around the player, and nothing outside of their texture (a border of 0).
        close += textureSampleLevel(rain, wave_sampler, texcoords.zw, 0.0).ba;
        let at = (world_xy - water.wave.xy) / 1280.0;
        if all(at >= vec2f(0.0)) && all(at <= vec2f(1.0)) {
            close += textureSampleLevel(player_waves, clamp_sampler, at, 0.0).ba;
        }
    }
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

// The distance along the view of the camera at a point of the picture, from the depth of
// the game. Very far where the game drew nothing.
fn distance_at(uv: vec2f) -> f32 {
    let size = vec2i(textureDimensions(depth));
    let at = clamp(vec2i(uv * water.size.xy), vec2i(0), size - 1);
    let d = textureLoad(depth, at, 0);
    if d >= 1.0 {
        return 1e30;
    }
    return water.projection.y / (d - water.projection.x);
}

// The same for the distant land, which has a projection of its own.
fn land_distance_at(uv: vec2f) -> f32 {
    let size = vec2i(textureDimensions(land_depth));
    let at = clamp(vec2i(uv * vec2f(size)), vec2i(0), size - 1);
    let d = textureLoad(land_depth, at, 0);
    if d >= 1.0 {
        return 1e30;
    }
    return water.land.y / (d - water.land.x);
}

// The view depth of the nearest thing at a point of the picture: what the game drew, and
// with `with_land` the distant land too.
fn ground_at(uv: vec2f, with_land: bool) -> f32 {
    let game = distance_at(uv);
    if with_land {
        return min(game, land_distance_at(uv));
    }
    return game;
}

// getProjectedReflection: the reflection at a place of the picture. With the blur
// (FILTER_WATER_REFLECTION), the mean of six reads in a radius that grows with the view
// depth `w` of the place.
// The way from the water at the view depth `w` to the thing that the mirror shows at a
// place of the picture, along the view: 0 for a thing at the water.
fn mirror_way(uv: vec2f, w: f32) -> f32 {
    let size = vec2i(textureDimensions(mirror_depth));
    let at = clamp(vec2i(uv * vec2f(size)), vec2i(0), size - 1);
    let d = textureLoad(mirror_depth, at, 0);
    return max(0.0, water.mirror.y / (d - water.mirror.x) - w);
}

// The blur that is sharp at the waterline. The water is not flat, so each point of it
// mirrors a small cone of directions. The cone is as wide as a point where the thing meets
// the water, and grows with the way from the water to the thing: a post is sharp at its
// foot and soft at its top. `stretch` makes the blur taller than wide for a flat look at
// the water, where the ripples smear a reflection along the view.
fn reflection_soft(uv: vec2f, w: f32, stretch: f32) -> vec3f {
    // The radius for a thing that is very far, in the picture: the angle of the cone
    // times the width of an angle.
    let widest = water.mirror.w * 0.5 * water.mirror.z * vec2f(1.0, stretch * water.size.x / water.size.y);
    // The nearest thing around the place sets the blur: the sky behind the edge of a tree
    // must not blur that edge.
    var way = mirror_way(uv, w);
    way = min(way, mirror_way(uv + widest * vec2f(1.0, 0.0), w));
    way = min(way, mirror_way(uv + widest * vec2f(-1.0, 0.0), w));
    way = min(way, mirror_way(uv + widest * vec2f(0.0, 1.0), w));
    way = min(way, mirror_way(uv + widest * vec2f(0.0, -1.0), w));
    let radius = widest * (way / (w + way));
    // The level of the reflection whose pixels are as wide as the radius, and four reads
    // around the place at half of the radius: a smooth disc, not a square.
    let level = clamp(log2(max(radius.x * f32(textureDimensions(reflection).x), 1.0)), 0.0, 5.0);
    var color = textureSampleLevel(reflection, clamp_sampler, uv, level).rgb * 2.0;
    color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(0.5, 0.25), level).rgb;
    color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(-0.5, -0.25), level).rgb;
    color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(-0.25, 0.5), level).rgb;
    color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(0.25, -0.5), level).rgb;
    return color / 6.0;
}

fn reflection_at(uv: vec2f, w: f32, stretch: f32) -> vec3f {
    if water.size.w > 1.5 {
        return reflection_soft(uv, w, stretch);
    }
    var color = textureSampleLevel(reflection, clamp_sampler, uv, 0.0).rgb;
    if water.size.w > 0.5 {
        let radius = 0.006 * clamp(0.11 + w / 6000.0, 0.0, 1.0) * vec2f(1.0, water.size.x / water.size.y);
        color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(0.60, 0.10), 0.0).rgb;
        color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(0.30, -0.21), 0.0).rgb;
        color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(0.96, -0.03), 0.0).rgb;
        color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(-0.40, 0.06), 0.0).rgb;
        color += textureSampleLevel(reflection, clamp_sampler, uv + radius * vec2f(-0.70, 0.18), 0.0).rgb;
        color /= 6.0;
    }
    return color;
}

// The colour of deep water with the fog over it: what the light of the sun and of the sky
// gives the water, then the fog. `tint` is the colour of the water of a surface (the
// emissive colour of its material, `waterDepthBase`); black for the usual colour.
fn depth_base(tint: vec3f) -> vec3f {
    return depth_base_sun(tint, 1.0);
}

// The same, with the share of the sun in it: 0 in the shadow of a thing, 1 in the sun.
fn depth_base_sun(tint: vec3f, sun: f32) -> vec3f {
    if max(tint.r, max(tint.g, tint.b)) >= 0.5 / 255.0 {
        return (0.05 * sun * water.sun_col.rgb + 0.085 * (2.0 * water.sky_col.rgb + u.fog_color.rgb)) * tint;
    }
    return sun * water.sun_col.rgb * vec3f(0.03, 0.04, 0.05) + (2.0 * water.sky_col.rgb + u.fog_color.rgb) * vec3f(0.075, 0.08, 0.085);
}

fn deep_color(fog: vec4f, tint: vec3f) -> vec3f {
    return fog.a * depth_base(tint) + fog.rgb;
}

// What is left of the light that comes through this much water of the colour `tint`
// (`waterTransmission`): the colour takes the other colours out, more with depth. All of
// it for the usual colour (black).
fn transmission(tint: vec3f, depth: f32) -> vec3f {
    let most = max(tint.r, max(tint.g, tint.b));
    if most < 0.5 / 255.0 {
        return vec3f(1.0);
    }
    return pow(max(tint / max(most, 1e-4), vec3f(0.05)), vec3f(depth / 200.0));
}

// The colour of the sky in a direction (`fogColourSky`): the far fog colour at the horizon
// goes into the sky colour above it, with the scattering of the atmosphere.
fn sky_color(direction: vec3f) -> vec3f {
    let up = 1.0 - pow(clamp(1.0 - 2.22 * clamp(direction.z - 0.075, 0.0, 1.0), 0.0, 1.0), 1.15);
    var color = mix(u.fog_color.rgb, water.sky_col.rgb, up);
    if u.sky.w > 0.5 && water.sky_col.w > 0.001 && u.eye.z > -1.0 {
        let sun_cos = dot(direction, water.sun_pos.xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * u.sun_altitude.z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(u.inscatter.rgb, u.outscatter.rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (u.sun_altitude.x + 0.7 * mie);
        lost = (1.0 - exp(-lost)) / lost;
        let scattered = (0.125 * mie + u.sky.rgb * rayleigh) * lost * (1.17 * air + 0.89) * u.sun_altitude.y;
        color = mix(color, scattered, water.sky_col.w);
    }
    return color;
}

// Turns `v` by the turn that takes straight up to `up` (`tiltTo`).
fn tilt_to(up: vec3f, v: vec3f) -> vec3f {
    let axis = vec3f(-up.y, up.x, 0.0);
    return v * up.z + cross(axis, v) + axis * (dot(axis, v) / max(1.0 + up.z, 1e-3));
}

// The reflection of what is in the picture (`reflectScene`): the march of the reflected ray
// from `origin` along `direction` over the depth of the game. Each step is 1.25 times as
// long as the one before, from 12 units. A place where the ray goes a little behind what the
// depth has is found by five halvings of the step; a place far behind is a thing nearer to
// the eye that hides the ray, and the march goes on to where the ray is in the open again.
// The colour of what the ray meets, and in alpha how much to trust it (less at the edge of
// the picture); alpha 0 when it meets nothing.
fn reflection_sample(origin: vec4f, direction: vec4f, travelled: f32, with_land: bool) -> vec3f {
    let clip = origin + direction * travelled;
    let uv = vec2f(0.5, 0.5) + vec2f(0.5, -0.5) * clip.xy / clip.w;
    return vec3f(uv, clip.w - ground_at(uv, with_land));
}

fn reflect_scene(origin: vec3f, direction: vec3f, with_land: bool) -> vec4f {
    let clip_origin = clip_of(origin);
    let clip_direction = u.relative * vec4f(direction, 0.0);
    // How far along the ray it leaves the picture: on it while each of these is positive.
    let room = vec4f(clip_origin.w - clip_origin.x, clip_origin.w + clip_origin.x, clip_origin.w - clip_origin.y, clip_origin.w + clip_origin.y);
    let closing = vec4f(clip_direction.x - clip_direction.w, -clip_direction.x - clip_direction.w, clip_direction.y - clip_direction.w, -clip_direction.y - clip_direction.w);
    let reaches = select(vec4f(1e9), room / closing, closing > vec4f(0.0));
    let reach = 0.999 * min(min(reaches.x, reaches.y), min(reaches.z, reaches.w));
    var step = 12.0;
    var marched = 0.0;
    var hidden = false;
    var open = 0.0;
    var behind = 0.0;
    var at_behind = vec3f(0.0);
    var halvings = 0;
    var limit = 0.0;
    for (var i = 0; i < 39; i++) {
        if halvings > 0 {
            let middle = 0.5 * (open + behind);
            let at_middle = reflection_sample(clip_origin, clip_direction, middle, with_land);
            if at_middle.z > 0.0 {
                behind = middle;
                at_behind = at_middle;
            } else {
                open = middle;
            }
            halvings -= 1;
            if halvings == 0 {
                if at_behind.z < limit {
                    let edge = min(at_behind.xy, vec2f(1.0) - at_behind.xy);
                    let color = textureSampleLevel(picture, clamp_sampler, at_behind.xy, 0.0).rgb;
                    return vec4f(color, clamp(12.0 * min(edge.x, edge.y), 0.0, 1.0));
                }
                hidden = true;
            }
        } else {
            if marched >= reach {
                break;
            }
            let next = min(marched + step, reach);
            let at_next = reflection_sample(clip_origin, clip_direction, next, with_land);
            if at_next.z > 0.0 {
                if !hidden && at_next.z < 2.0 * step + 16.0 {
                    open = marched;
                    behind = next;
                    at_behind = at_next;
                    halvings = 5;
                    limit = step / 16.0 + 16.0;
                } else {
                    hidden = true;
                }
            } else {
                hidden = false;
            }
            marched = next;
            step *= 1.25;
        }
    }
    return vec4f(0.0);
}

// What the water shows of the things under it at a place of the picture (`uv`, at the
// view depth `w`), with the shift of the waves; deep water where the bottom is far. `tint`
// is the colour of the water, `clarity` over how many units what is under the water fades
// into the colour of deep water (800 for the sea), `look_tint` the tint of a look; with
// `with_land` the bottom can be the distant land. `light` multiplies the colour of the
// bottom before the water takes it (the caustics); 1 for none.
struct Refraction {
    color: vec3f,
    // 1 at the shore, where the water is so shallow that it shows the bottom as it is.
    shore: f32,
    // The depth of the water under the place, for a test.
    depth_first: f32,
}

fn refraction(uv: vec2f, w: f32, reffactor: vec2f, eye_vec: vec3f, normal: vec3f, deep: vec3f, tint: vec3f, clarity: f32, look_tint: vec3f, with_land: bool, light: f32) -> Refraction {
    var out: Refraction;
    var new_uv = uv + reffactor.yx / w;
    var depth_of = max(0.0, ground_at(new_uv, with_land) - w);
    out.depth_first = depth_of;
    out.color = deep;
    out.shore = 0.0;
    if depth_of < 4000.0 {
        new_uv = uv + clamp(depth_of / 100.0, 0.0, 1.0) * reffactor.yx / w;
        var refracted = textureSample(picture, clamp_sampler, new_uv).rgb * light;
        depth_of = max(0.0, ground_at(new_uv, with_land) - w);
        // From a difference of the depth to a distance along the ray.
        depth_of /= dot(eye_vec, vec3f(water.view[0].z, water.view[1].z, water.view[2].z));
        refracted *= transmission(tint, depth_of * 800.0 / clarity) * look_tint;
        depth_of += 300.0 * (0.95 - normal.z);
        let depthscale = clamp(exp(-depth_of / clarity), 0.0, 1.0);
        out.shore = pow(depthscale, 90.0);
        out.color = mix(deep, refracted, 0.8 * depthscale + 0.2 * out.shore);
    }
    return out;
}

// The light of the sun that the water mirrors to the eye.
fn sun_glint(eye_vec: vec3f, normal: vec3f, fog_a: f32) -> vec3f {
    let vdotr = clamp(1.0025 * dot(-eye_vec, reflect(-water.sun_pos.xyz, normal)), 0.0, 1.0);
    return water.sun_col.rgb * (pow(vdotr, 170.0) + 0.07 * pow(vdotr, 4.0)) * fog_a;
}

// UnderwaterPS: the water seen from below, at a place of the picture (`uv`, at the view
// depth `w`).
fn underwater(uv: vec2f, w: f32, texcoords: vec4f, world_xy: vec2f, eye_vec: vec3f, dist: f32) -> vec4f {
    let fog = clamp(exp(-dist / 4096.0), 0.0, 1.0);
    let normal = -water_normal(texcoords, dist, world_xy);
    let reffactor = 2.0 * (water.sun_col.w * dist + 0.1) * normal.xy;
    var refracted = textureSample(picture, clamp_sampler, uv + 2.0 * -reffactor.xy / w).rgb;
    refracted = mix(u.fog_color.rgb, refracted, exp(-dist / 500.0));
    let reflected = reflection_at(uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / w, w, 1.0);
    // With the total reflection of the inside of the water.
    let fresnel = pow(clamp(1.12 - 0.65 * dot(-eye_vec, normal), 0.0, 1.0), 8.0);
    let result = mix(refracted, reflected, fresnel);
    let refract_sun = dot(-eye_vec, normalize(-water.sun_pos.xyz + normal));
    let spec = water.sun_col.rgb * pow(max(refract_sun, 0.0), 6.0) * fog;
    return vec4f(result + spec, 1.0);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_water_struct_has_the_size_of_its_wgsl() {
        // A matrix and eleven vectors of four numbers.
        assert_eq!(size_of::<Uniforms>(), 4 * (16 + 11 * 4));
        let water = &COMMON[COMMON.find("struct Water {").unwrap()..];
        let water = &water[..water.find('}').unwrap()];
        assert_eq!(water.matches(": vec4f,").count(), 11);
        assert_eq!(water.matches(": mat4x4f,").count(), 1);
    }
}
