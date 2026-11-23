//! interface: mge-xe-core 1

// XE Mod Water.wgsl: the wgpu renderer's port of XE Mod Water.fx of
// Enhanced Water Shader 2.2 Green-Blue by Krokantor, updated for MGE XE 0.13.0 by Hrnchamd.
// Put this file beside XE Mod Water.fx in Data Files/shaders/core-mods: Direct3D 9 reads the
// .fx, the wgpu renderer this file. The mod needs the dynamic waves at a wave height of 40.
//
// The switches of the .fx as it ships: FOAM, HEIGHT_WAVES and DEPTHCOLOR on; FAR_WAVE_NORMALS,
// SEWER_WAVES and SWELL_FOAM off.

//------------------------------------------------------------
// Tweakables: the numbers of the .fx. Change them here for the wgpu renderer.

const mod_green = vec3f(0.09, 0.22, 0.17); // default: 0.09, 0.22, 0.17
const mod_blue = vec3f(0.07, 0.13, 0.29); // default: 0.07, 0.13, 0.29

const mod_shallowcol = vec3f(0.15, 0.7, 0.1); // default: 0.15, 0.7, 0.1
const mod_deepcol = vec3f(0.1, 0.15, 0.3); // default: 0.1, 0.15, 0.3

const mod_dcolstr = 0.13; // depth color strength; default: 0.13
const mod_watcolstr = 1.0; // water color brightness; default: 1
const mod_depthfactor = 2500.0; // depth scale; higher values: greater depths look like shallow water; default: 2500

const mod_regwavemult = 1.0; // height of the regular waves; default: 1
const mod_regwavewindmult = 30.0; // higher values increase the wind's influence on the regular waves; default: 30
const mod_heightwavemult = 2.5; // height of the swell; default: 2.5
const mod_heightwavewindmult = 60.0; // higher values reduce the swell in bad weather; default: 60

const mod_edgefoamstr = 1.0; // shore foam by day; default: 1
const mod_edgefoamstrn = 0.3; // shore foam at night and in bad weather; default: 0.3
const mod_wavefoamstrn = 0.795; // foam on the waves in a storm; default: 0.795

//------------------------------------------------------------

// random() of the .fx: a constant, frac(cos(dot(PI, float2(23.14069263277926,
// 2.665144142690225))) * 12345.6789), with the 0.1 that the .fx adds to it.
const mod_random = 0.34024046;

// WaterVS of the .fx: with the dynamic waves, the waves with the wind, the swell (HEIGHT_WAVES),
// the plane lowered under the ripples of the game, and raised by 20 units with the eye under
// the water; without them, the plane 0.1 under the water level.
fn water_vertex(start: vec3f) -> WaterVertex {
    var out: WaterVertex;
    var world = start;
    if water_dynamic_waves() {
        out.texcoords = vec4f(world.xy / 2600.0, world.xy / 440.0);
        let time = water_time();
        let wave_height = water_wave_height();
        let wind = mod_wind_factor();
        let t = 0.325 * time;
        let height = wave_level_at(vec3f(world.xy / 860.0, t)).a;
        let height2 = wave_level_at(vec3f(world.xy / 2800.0, t)).a;
        let dist = length(water_eye() - world);
        var add = wave_height * (mix(height, height2, clamp(dist / 8000.0, 0.0, 1.0)) - 0.5) * clamp(1.0 - dist / 6400.0, 0.0, 1.0) * clamp(dist / 200.0, 0.0, 1.0);
        add += (0.4 + mod_regwavewindmult * wind) * mix(height, height2, wave_height);
        // Less height far away, against the flicker.
        add = mix(-1.0, add, water_near_fog(dist * 2.0));
        add *= mod_regwavemult;
        world.z += add;
        // The swell, less in bad weather. The .fx adds a float2 to the height, so only its x
        // counts: the swell goes along x only.
        let decrease = mod_heightwavemult - mod_heightwavewindmult * wind;
        let s = -world.x / 256.0;
        let swell = decrease * wave_height * ((sin(s + 1.75 * time) + sin(2.5 * s + 2.2 * time) + sin(3.0 * s + 1.4 * time) + sin(4.5 * s + 3.0 * time)) / 32.0);
        world.z += swell;
        // The plane under the ripples of the game, and over the eye under the water (the .fx:
        // "Waterlevel is 1.5 units wrong").
        world.z -= 9.0 + mod_heightwavemult + mod_regwavemult + 0.5 * (add + swell);
        if water_eye().z - water_level() + 1.5 <= 0.5 {
            world.z += 20.0;
        }
        out.lowered = abs(add);
    } else {
        out.texcoords = vec4f(world.xy / 2900.0, world.xy / 427.0);
        world.z -= 0.1;
    }
    out.world = world;
    return out;
}
// windFactor of the file.
fn mod_wind_factor() -> f32 {
    return (length(water_wind()) + 1.5) / 120.0;
}

// sunlightFactor of the file.
fn mod_sunlight() -> f32 {
    let dark = 1.0 - water_sun_visibility();
    return 1.0 - dark * dark;
}

// fogApply of XE Common.fx.
fn mod_fog_apply(color: vec3f, fog: vec4f) -> vec3f {
    return fog.a * color + fog.rgb;
}

// A power of a number of 0 to 1: 0 stays 0.
fn mod_pow(x: f32, power: f32) -> f32 {
    return pow(max(x, 1e-30), power);
}

// getFinalWaterNormal: two reads of the waves for the far normal and two for the close one.
fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.315 * water_time();
    let far = vec3f(texcoords.xy * 0.5, t * 0.5);
    let far_normal = 0.5 * (wave_at(far * 3.0).rg + wave_at(far).rg);
    var close_normal = 0.5 * (wave_at(vec3f(texcoords.zw, t)).rg + wave_at(vec3f(texcoords.xy, t * 0.6)).rg);
    close_normal = ripples_on(close_normal, texcoords.zw, world_xy);
    let normal = 2.0 * mix(close_normal, far_normal, clamp(distance / 11000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(normal, 0.295));
}

// calcFoam: x the foam at the shore, y the foam on the tips of the waves.
fn mod_foam(world: vec3f, fog: vec4f, depth_scale: f32) -> vec2f {
    let wave_height = water_wave_height();
    let wave_low = wave_height * -0.1;
    let wave_high = wave_height * 0.5;
    let height_now = world.z + 10.0 - water_level();
    var wave_col = wave_at(vec3f(world.xy / 45.0, water_time())).rgb;
    wave_col = mix(vec3f(-1.0), wave_col * 1.25, fog.a * 1.1);
    let wave_mult = wave_col.b * 0.5 + wave_col.r * 0.25 + wave_col.g * 0.25;
    var edge_col = wave_at(vec3f(world.xy / 60.0, water_time() * 0.2)).rgb;
    edge_col = mix(vec3f(-1.0), edge_col * 1.29, fog.a * 0.9);
    let edge_mult = edge_col.b * 0.4 + edge_col.r * 0.3 + edge_col.g * 0.3;
    let edge_factor = mod_pow(depth_scale, 25.0);
    let away = edge_factor - 0.5;
    var edge = clamp(-5.0 * away * away + 1.0, 0.0, 1.0) * 0.5;
    var wave = clamp((height_now - wave_low) / (wave_high - wave_low), 0.0, 1.0);
    if edge_mult > 1.0 - edge {
        edge *= edge_mult;
    } else {
        edge *= edge_mult / 3.0;
    }
    if wave_mult > 1.0 - wave {
        wave *= wave_mult;
    } else {
        wave *= wave_mult / 3.0;
    }
    return vec2f(edge, wave);
}

// WaterPS. The offsets of the file are in the projected space: here they are divided by the
// clip w of the pixel. The depth of the file is the view depth; along the eye vector it is
// that times the distance over the clip w.
fn water_color(p: WaterPixel) -> vec4f {
    let eye_vec = p.eye_vec;
    let dist = p.distance;
    let w = p.clip_w;
    let wind = mod_wind_factor();
    let sun_vis = water_sun_visibility();
    let ambient = water_ambient_color();
    let normal = water_normal(p.texcoords, dist * 0.65, p.world.xy);
    let reffactor = 0.5 * ((wind * dist + 0.1) * normal.xy);
    let offset = reffactor.yx / w;
    let depth_first = max(0.0, ground_distance_at(p.uv + offset) - w);
    let fog = fog_water(eye_vec, dist);

    // The colour of the water, with the colour of the depth (DEPTHCOLOR).
    var depth_color = sun_vis * mix(mod_blue, mod_green, ambient);
    let shift = clamp(depth_first / 150.0, 0.0, 1.0) * offset;
    let depth_fixed = max(0.0, ground_distance_at(p.uv + shift) - w);
    let depth_fixed_scale = clamp(1.0 - depth_fixed / mod_depthfactor, 0.0, 1.0);
    depth_color += mod_dcolstr * sun_vis * mix(mod_deepcol, mod_shallowcol, depth_fixed_scale);
    depth_color = mod_fog_apply(mod_watcolstr * depth_color, fog);

    // The refraction, with the chromatic aberration, and the foam (FOAM). The file reads them
    // only where the water is not deep; here the reads come first, out of the branch, because
    // a read with the derivatives of the pixel must be in uniform control flow.
    let at = p.uv + shift;
    let aber = reflect(eye_vec, normal).xy / w;
    let picture = vec3f(picture_at(at).r, picture_at(at - aber * 0.05).g, picture_at(at - aber * 0.1).b);
    // The depth along the eye vector, with the small movement of the shore line.
    let depth = max(0.0, ground_distance_at(at) - w) * dist / w + 50.0 * (0.99 - normal.z);
    let depth_scale = clamp(exp(-depth / 800.0), 0.0, 1.0);
    let foam_here = mod_foam(p.world, fog, depth_scale);
    var refracted = depth_color;
    var shore = 0.0;
    var foam = vec2f(0.0);
    if depth_first < 10000.0 {
        shore = mod_pow(depth_scale, 90.0);
        refracted = mix(depth_color, picture, 0.8 * depth_scale + 0.2 * shore);
        foam = foam_here;
    }

    // The reflection, into the fog at the horizon.
    let reflect_w = p.reflect_w;
    var reflected = reflection_at(p.reflect_uv - vec2f(1.5 * reffactor.x, -abs(reffactor.y)) / reflect_w, reflect_w, 1.0);
    reflected = mix(fog.rgb, reflected, fog.a);

    // Less of the high frequencies far away.
    var adjust = mix(vec3f(0.0, 0.0, 0.1), normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, vec3f(0.0, 0.0, 1.0), (1.0 + eye_vec.z) * (1.0 - clamp(1.0 / (dist / 1300.0 + 6.0), 0.0, 1.0)));

    // The water cut (vtastek): with the eye under the level 0.
    let under = step(0.5, 1.0 - clamp(water_eye().z + 1.0, 0.0, 1.0));
    var cut = smoothstep(0.09, 0.1, dist / 1200.0);
    cut = 1.0 - clamp((1.0 - clamp(abs(cut * 5.0), 0.0, 1.0)) * under, 0.0, 1.0);
    var cut_dark = smoothstep(0.0889, 0.101, dist / 1200.0) * under;
    cut_dark = clamp(3.0 * cut_dark * (1.0 - cut_dark), 0.0, 1.0);

    // Fresnel.
    let fresnel = 0.015 + mod_pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 20.0);
    var result = mix(refracted, reflected, fresnel);

    // The light of the sun: a wide Phong lobe, and a little shimmer.
    let vdotr = clamp(1.0025 * dot(-eye_vec, reflect(-water_sun_direction(), normal)), 0.0, 1.0);
    var spec = water_sun_color() * mod_sunlight() * (mod_pow(vdotr, 200.0) + 0.07 * mod_pow(vdotr, 30.0));
    spec += 0.25 * mod_pow(normal.z, 100.0) * ambient / 12.0;
    spec = mod_fog_apply(spec, fog);
    result += spec * fog.a;

    // The foam: at night at the shore, and on the waves with wind; by day at the shore.
    if sun_vis <= 0.0 {
        if wind <= 0.0235 {
            result += clamp(foam.x, 0.0, 1.0) * 0.1;
        } else {
            result += clamp(foam.x, 0.0, 1.0) * mod_edgefoamstrn;
            result += clamp(foam.y, 0.0, 1.0) * mod_wavefoamstrn;
        }
    } else {
        result += clamp(foam.x * clamp(water_sky_color().b, 0.0, 1.0), 0.0, 1.0) * (mod_random + 0.3) * mod_edgefoamstr;
    }

    // The shore line, the fog over far water, and the water cut.
    result = mix(result, refracted, shore * fog.a);
    let fade_way = (1.0 + eye_vec.z) * (1.0 - clamp(3000.0 / dist, 0.0, 1.0));
    let fog_fade = pow(clamp(mix(1.0, fog.a, fade_way), 0.0, 1.0), 1.1);
    result = mix(fog.rgb, result, fog_fade);
    result = mix(refracted, result, cut);
    result = mix(result, result * 0.1, cut_dark);
    return vec4f(result, 1.0);
}

// UnderwaterPS.
fn underwater_color(p: WaterPixel) -> vec4f {
    let eye_vec = p.eye_vec;
    let dist = p.distance;
    let fog = clamp(exp(-dist / 4096.0), 0.0, 1.0);
    let normal = -water_normal(p.texcoords, dist, p.world.xy);
    let reffactor = 2.0 * (mod_wind_factor() * dist + 0.1) * normal.xy;
    var refracted = picture_at(p.uv - 2.0 * reffactor / p.clip_w);
    refracted = mix(water_fog_color(), refracted, exp(-dist / 500.0));
    let reflected = reflection_at(p.uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / p.clip_w, p.clip_w, 1.0);
    let fresnel = mod_pow(clamp(1.12 - 0.65 * dot(-eye_vec, normal), 0.0, 1.0), 8.0);
    let result = mix(refracted, reflected, fresnel);
    let refract_sun = dot(-eye_vec, normalize(-water_sun_direction() + normal));
    let spec = water_sun_color() * mod_sunlight() * mod_pow(refract_sun, 6.0) * fog;
    return vec4f(result + spec, 1.0);
}
