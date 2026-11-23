//! part: common

// The view group (`view.rs`, group 0) comes before this code: the game view in `u`, with the
// camera, the exponential fog (`u.fog.xy`), the far fog colour (`u.fog_color`) and the
// scattering (`u.sky.w` 1 when it is on). The values of the water are in `water`, group 1.

// The debug view `water`: false in normal play. The pipelines of the view are made from this
// text with true on this line (`debug_source`); the term of the view is `water.modes.x`.
const WATER_DEBUG: bool = false;

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
    sun_color: vec4f,
    wind: vec4f,
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
    if u.sky.w > 0.5 && water.sky_col.w > 0.001 && u.eye.z > u.flags.w {
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

// The volume of the waves at `coords` (x and y the place, z the time).
fn wave_at(coords: vec3f) -> vec4f {
    return textureSample(waves, wave_sampler, coords);
}

// `normal` with the ripples of the rain (at the close wave coordinates `close`) and of the
// player (at the place `world_xy`) added; unchanged without the dynamic waves. The ripples of
// the rain repeat over the water. Those of the player are in the world around the player, and
// nothing outside of their texture (a border of 0).
fn ripples_on(normal: vec2f, close: vec2f, world_xy: vec2f) -> vec2f {
    var out = normal;
    if water.wave.w > 0.5 {
        out += textureSampleLevel(rain, wave_sampler, close, 0.0).ba;
        let at = (world_xy - water.wave.xy) / 1280.0;
        if all(at >= vec2f(0.0)) && all(at <= vec2f(1.0)) {
            out += textureSampleLevel(player_waves, clamp_sampler, at, 0.0).ba;
        }
    }
    return out;
}

// The distance along the view of the camera at a point of the picture, from the depth of
// the game. Very far where the game drew nothing.
fn distance_at(uv: vec2f) -> f32 {
    let size = vec2i(textureDimensions(depth));
    let at = clamp(vec2i(uv * water.size.xy), vec2i(0), size - 1);
    let d = textureLoad(depth, at, 0);
    if depth_empty(d) {
        return 1e30;
    }
    return view_distance(d, water.projection.xy);
}

// The same for the distant land, which has a projection of its own.
fn land_distance_at(uv: vec2f) -> f32 {
    let size = vec2i(textureDimensions(land_depth));
    let at = clamp(vec2i(uv * vec2f(size)), vec2i(0), size - 1);
    let d = textureLoad(land_depth, at, 0);
    if depth_empty(d) {
        return 1e30;
    }
    return view_distance(d, water.land.xy);
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
    return max(0.0, view_distance(d, water.mirror.xy) - w);
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
    if u.sky.w > 0.5 && water.sky_col.w > 0.001 && u.eye.z > u.flags.w {
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
    let seen = textureSample(picture, clamp_sampler, uv + 2.0 * -reffactor.xy / w).rgb;
    let refracted = mix(u.fog_color.rgb, seen, underwater_clear(dist));
    let reflected = reflection_at(uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / w, w, 1.0);
    // A look at one term, for a test: the debug view `water`, as above the water (1 and 2), and
    // 10 for the picture of the game before the fog of the water is mixed into it.
    if WATER_DEBUG {
        if water.modes.x == 1.0 {
            return vec4f(reflected, 1.0);
        } else if water.modes.x == 2.0 {
            return vec4f(refracted, 1.0);
        } else if water.modes.x == 10.0 {
            return vec4f(seen, 1.0);
        }
    }
    // With the total reflection of the inside of the water.
    let fresnel = pow(clamp(1.12 - 0.65 * dot(-eye_vec, normal), 0.0, 1.0), 8.0);
    let result = mix(refracted, reflected, fresnel);
    let refract_sun = dot(-eye_vec, normalize(-water.sun_pos.xyz + normal));
    let spec = water.sun_col.rgb * pow(max(refract_sun, 0.0), 6.0) * fog;
    return vec4f(result + spec, 1.0);
}

// What is left of the picture behind the water seen from below, at the distance `dist`: the
// rest is the fog colour (`underwater`).
fn underwater_clear(dist: f32) -> f32 {
    return exp(-dist / 500.0);
}

// The debug view `fog` on a surface of the water: the part of the fog at a pixel, as grey.
// Above the water it is the fog of `fog_water`, from below that of `underwater`.
fn water_fog_view(eye_vec: vec3f, dist: f32) -> vec4f {
    if water.modes.y > 0.5 {
        return vec4f(vec3f(1.0 - underwater_clear(dist)), 1.0);
    }
    return vec4f(vec3f(1.0 - fog_water(eye_vec, dist).a), 1.0);
}
//! part: caustics

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
//! part: water_normal

// getFinalWaterNormal: two reads of the wave texture, the far one past 8000 units.
fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.4 * water.time.x;
    let far = wave_at(vec3f(texcoords.xy, t)).rg;
    var close = wave_at(vec3f(texcoords.zw, t)).rg;
    close = ripples_on(close, texcoords.zw, world_xy);
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}
//! part: water_vertex

// WaterVS: the place of a vertex of the plane.
fn water_vertex(start: vec3f) -> WaterVertex {
    var out: WaterVertex;
    var world = start;
    if water_dynamic_waves() {
        // With DYNAMIC_RIPPLES: the waves move the plane, most between 200 and 6400 units
        // from the camera.
        let t = 0.4 * water_time();
        let height = wave_level_at(vec3f(world.xy / 1104.0, t)).a;
        let height2 = wave_level_at(vec3f(world.xy / 3900.0, t)).a;
        let dist = length(water_eye() - world);
        let add = water_wave_height() * (mix(height, height2, clamp(dist / 8000.0, 0.0, 1.0)) - 0.5) * clamp(1.0 - dist / 6400.0, 0.0, 1.0) * clamp(dist / 200.0, 0.0, 1.0);
        world.z += add;
        out.lowered = abs(add);
    } else {
        // The plane lies at the water level less 0.1, a little under the ripples of the game.
        world.z -= 0.1;
    }
    out.world = world;
    out.texcoords = vec4f(world.xy / 3900.0, world.xy / 527.0);
    return out;
}
//! part: water_color

// WaterPS: the colour of the water at a pixel seen from above.
fn water_color(p: WaterPixel) -> vec4f {
    let eye_vec = p.eye_vec;
    let dist = p.distance;
    let uv = p.uv;
    let w = p.clip_w;
    let fog = fog_water(eye_vec, dist);
    let normal = water_normal(p.texcoords, dist, p.world.xy);
    let reffactor = (water.sun_col.w * dist + 0.1) * normal.xy;
    let below = refraction(uv, w, reffactor, eye_vec, normal, deep_color(fog, vec3f(0.0)), vec3f(0.0), 800.0, vec3f(1.0), true, 1.0);
    let refracted = below.color;
    let reflect_uv = p.reflect_uv;
    let reflect_w = p.reflect_w;
    // A flat look at the water makes the blur taller: 1 from straight above, and at most
    // the number of the parameter.
    let stretch = clamp(1.0 / max(abs(eye_vec.z), 0.05), 1.0, max(water.projection.z, 1.0));
    var reflected = reflection_at(reflect_uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / reflect_w, reflect_w, stretch);
    reflected = mix(fog.rgb, reflected, fog.a);
    var adjust = mix(vec3f(0.0, 0.0, 0.1), normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, vec3f(0.0, 0.0, 1.0), (1.0 + eye_vec.z) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0)));
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(refracted, reflected, fresnel);
    result += sun_glint(eye_vec, normal, fog.a);
    result = mix(result, refracted, below.shore * fog.a);
    // A look at one term, for a test: the debug view `water`.
    if WATER_DEBUG {
        if water.modes.x == 1.0 {
            return vec4f(reflected, 1.0);
        } else if water.modes.x == 2.0 {
            return vec4f(refracted, 1.0);
        } else if water.modes.x == 3.0 {
            return vec4f(vec3f(below.depth_first / 1000.0), 1.0);
        } else if water.modes.x == 4.0 {
            return vec4f(vec3f(fresnel), 1.0);
        } else if water.modes.x == 5.0 {
            return vec4f(vec3f(fog.a), 1.0);
        } else if water.modes.x == 6.0 {
            return vec4f(vec3f(dist / 10000.0), 1.0);
        } else if water.modes.x == 7.0 {
            return vec4f(normal * 0.5 + 0.5, 1.0);
        } else if water.modes.x == 8.0 {
            return vec4f(vec3f(w / 10000.0), 1.0);
        }
    }
    return vec4f(result, 1.0);
}
//! part: underwater_color

// UnderwaterPS: the plane seen from under the water.
fn underwater_color(p: WaterPixel) -> vec4f {
    return underwater(p.uv, p.clip_w, p.texcoords, p.world.xy, p.eye_vec, p.distance);
}
//! part: plane

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) texcoords: vec4f,
    @location(2) clip_w: f32,
    // With the dynamic waves: the clip x, y and w of the point at or under the plane, for
    // the place of the reflection (screenposclamp).
    @location(3) clamped: vec3f,
    // For a test (the debug view `water 9`): the height that the vertex read in the wave texture.
    @location(4) probe: f32,
}

@vertex fn vs(@location(0) local: vec3f) -> Out {
    var out: Out;
    // The vertex at the water level; the hook `water_vertex` moves it.
    let start = local + water.world.xyz;
    let moved = water_vertex(start);
    let world = moved.world;
    if WATER_DEBUG && water.wave.w > 0.5 {
        out.probe = textureSampleLevel(waves, wave_sampler, vec3f(start.xy / 1104.0, 0.4 * water.time.x), 0.0).a;
    }
    out.world = world;
    out.texcoords = moved.texcoords;
    out.position = clip_of(world);
    // The far plane of the game is its view distance; the sea goes on to the draw distance of
    // the distant land, as on Direct3D 9, where MGE XE moves the far plane of the game there.
    // The near plane stays that of the game. The depth that the pixel writes is that of the
    // game (see `fs`).
    out.position.z = out.position.w * water.land.z + water.land.w;
    out.clip_w = out.position.w;
    // The point of the reflection is not above the plane.
    out.clamped = clip_of(vec3f(world.xy, world.z - moved.lowered)).xyw;
    return out;
}

struct Pixel {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
}

@fragment fn fs(in: Out) -> Pixel {
    // The distant land in front of the water hides it. The renderer cleared its depth for the
    // game, so the test is here; the test against the draws of the game is that of the pass.
    if in.clip_w > land_distance_at(in.position.xy / water.size.xy) * 1.0005 + 1.0 {
        discard;
    }
    var out: Pixel;
    out.color = plane_color(in);
    // The depth in the range of the game: what blends after the water tests it. Past the far
    // plane of the game it is the far end.
    out.depth = clamp(water.projection.x + water.projection.y / in.clip_w, 0.0, 1.0);
    return out;
}

fn plane_color(in: Out) -> vec4f {
    // The end of the plane in an interior and under the water.
    if water.size.z > 0.0 && in.clip_w > water.size.z {
        discard;
    }
    var p: WaterPixel;
    let eye_vec = in.world - u.eye.xyz;
    p.distance = length(eye_vec);
    p.eye_vec = eye_vec / p.distance;
    // The position of the pixel in the picture. The offsets of the shader are in the
    // projected space: here they are divided by the clip w of the pixel.
    p.uv = in.position.xy / water.size.xy;
    p.clip_w = in.clip_w;
    p.texcoords = in.texcoords;
    p.world = in.world;
    // With the dynamic waves the reflection is read at the point that is not above the plane.
    p.reflect_uv = p.uv;
    p.reflect_w = p.clip_w;
    if water.wave.w > 0.5 {
        p.reflect_w = in.clamped.z;
        p.reflect_uv = vec2f(0.5 + 0.5 * in.clamped.x / p.reflect_w, 0.5 - 0.5 * in.clamped.y / p.reflect_w);
    }
    // The debug view `fog`, before the hooks: a core mod's water shows the fog of the plane too.
    if fog_view_on() {
        return water_fog_view(p.eye_vec, p.distance);
    }
    if water.modes.y > 0.5 {
        return underwater_color(p);
    }
    if WATER_DEBUG && water.modes.x == 9.0 {
        // A look at the waves, for a test (the debug view `water 9`). Red: the height that the
        // vertices read. Green: the same read for the pixel.
        let here = textureSampleLevel(waves, wave_sampler, vec3f(in.world.xy / 1104.0, 0.4 * water.time.x), 0.0).a;
        return vec4f(in.probe, here, 0.0, 1.0);
    }
    return water_color(p);
}

// The library of the water hooks (`core_mods.rs`, `WATER_LIBRARY`), with the functions of the
// shared shading: the picture, the distance of the ground and the values of the frame.
fn picture_at(uv: vec2f) -> vec3f {
    return textureSample(picture, clamp_sampler, uv).rgb;
}
fn ground_distance_at(uv: vec2f) -> f32 {
    return ground_at(uv, true);
}
fn water_time() -> f32 {
    return water.time.x;
}
fn water_sun_direction() -> vec3f {
    return water.sun_pos.xyz;
}
fn water_sun_visibility() -> f32 {
    return water.sun_pos.w;
}
fn water_sun_color() -> vec3f {
    return water.sun_color.rgb;
}
fn water_wind() -> vec2f {
    return water.wind.xy;
}
fn water_sky_color() -> vec3f {
    return water.sky_col.rgb;
}
fn water_fog_color() -> vec3f {
    return u.fog_color.rgb;
}
fn water_eye() -> vec3f {
    return u.eye.xyz;
}
fn water_ambient_color() -> vec3f {
    return u.ambient.rgb;
}
fn water_texel() -> vec2f {
    return 1.0 / water.size.xy;
}
fn water_wave_height() -> f32 {
    return water.wave.z;
}
fn water_is_interior() -> bool {
    return water.modes.z > 0.5;
}
// The functions of the library that `water_vertex` can call too.
fn wave_level_at(coords: vec3f) -> vec4f {
    return textureSampleLevel(waves, wave_sampler, coords, 0.0);
}
fn water_level() -> f32 {
    return water.world.z;
}
// fogMWScalar: the linear fog of the game, from its start (u.fog.z) to its end (u.fog.w).
fn water_near_fog(distance: f32) -> f32 {
    return clamp((u.fog.w - distance) / max(u.fog.w - u.fog.z, 0.001), 0.0, 1.0);
}
fn water_dynamic_waves() -> bool {
    return water.wave.w > 0.5;
}
//! part: level

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}

@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
    // The target has half of the size of the source.
    let size = vec2f(textureDimensions(source)) * 0.5;
    return textureSampleLevel(source, source_sampler, position.xy / size, 0.0);
}
