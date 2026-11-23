//! interface: mge-xe-core 1

// XE Mod Water.wgsl: the wgpu renderer's port of XE Mod Water.fx of Pixel Shader Style Water for
// MGE XE 1.0 by Hrnchamd ("MGE XE 0.13.0" in the header of the .fx). The mod's permissions: "Free
// for distribution and code re-use. Please give credit." Put this file beside XE Mod Water.fx in
// Data Files/shaders/core-mods: Direct3D 9 reads the .fx, the wgpu renderer this file.
//
// The .fx is MGE XE's stock water with other numbers. Its getProjectedReflection is the stock
// text: the renderer's reflection_at gives it (the one read, or the six reads with the blur of
// the reflections). The port passes a stretch of 1: the stretch acts only in the renderer's soft
// blur, which Direct3D 9 does not have.

//------------------------------------------------------------
// The values of the .fx that come from the frame.

// sunColAdjusted: 0.4 * sunCol * sunlightFactor, with sunlightFactor = 1 - (1 - sunVis)^2.
fn mod_sun_adjusted() -> vec3f {
    let dark = 1.0 - water_sun_visibility();
    return 0.4 * water_sun_color() * (1.0 - dark * dark);
}

// windFactor: (length(windVec) + 1.5) / 140.
fn mod_wind_factor() -> f32 {
    return (length(water_wind()) + 1.5) / 140.0;
}

// depthBaseColor.
fn mod_depth_base() -> vec3f {
    return mod_sun_adjusted() * vec3f(0.03, 0.04, 0.05) + (2.0 * water_sky_color() + water_fog_color()) * vec3f(0.075, 0.08, 0.085);
}

//------------------------------------------------------------
// The hooks.

// WaterVS: the stock vertex with the wave coordinates four times as fine.
fn water_vertex(start: vec3f) -> WaterVertex {
    var out: WaterVertex;
    var world = start;
    if water_dynamic_waves() {
        // DYNAMIC_RIPPLES: the vertex displacement, and the reflection read at the point that is
        // not above the plane (screenposclamp).
        let t = 0.4 * water_time();
        let height = wave_level_at(vec3f(world.xy / 1104.0, t)).a;
        let height2 = wave_level_at(vec3f(world.xy / 3900.0, t)).a;
        let dist = length(water_eye() - world);
        let add = water_wave_height() * (mix(height, height2, clamp(dist / 8000.0, 0.0, 1.0)) - 0.5) * clamp(1.0 - dist / 6400.0, 0.0, 1.0) * clamp(dist / 200.0, 0.0, 1.0);
        world.z += add;
        out.lowered = abs(add);
    } else {
        // The z bias against the ripple quads of the game.
        world.z -= 0.1;
    }
    out.world = world;
    out.texcoords = vec4f(world.xy / 3900.0 * 4.0, world.xy / 527.0 * 4.0);
    return out;
}

// getFinalWaterNormal: the stock normal with the rain read at a fifth of the close coordinates.
fn water_normal(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.4 * water_time();
    let far = wave_at(vec3f(texcoords.xy, t)).rg;
    var close = wave_at(vec3f(texcoords.zw, t)).rg;
    close = ripples_on(close, 0.2 * texcoords.zw, world_xy);
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

// WaterPS.
fn water_color(p: WaterPixel) -> vec4f {
    let eye_vec = p.eye_vec;
    let dist = p.distance;

    // The fog, and the colour of deep water under it (fogApply).
    let fog = fog_water(eye_vec, dist);
    let depth_color = fog.a * mod_depth_base() + fog.rgb;

    let normal = water_normal(p.texcoords, dist, p.world.xy);

    // The distortion of the reflection and the refraction; the wind makes it stronger.
    let reffactor = (mod_wind_factor() * dist + 5.0) * normal.xy;

    // The depth of the water under the pixel, at the place without the distortion.
    let depth = max(0.0, ground_distance_at(p.uv) - p.clip_w);

    // The refraction.
    var refracted = picture_at(p.uv + 0.2 * reffactor.yx / p.clip_w);
    let shorefactor = clamp(exp(-depth / 4.0), 0.0, 1.0);
    let depth_attenuation = clamp(1.0 - depth / 2700.0, 0.0, 1.0);
    refracted = mix(depth_color, refracted, max(shorefactor, clamp(depth_attenuation - 0.4, 0.0, 1.0)));

    // The reflection, faded into the horizon.
    var reflected = reflection_at(p.reflect_uv - vec2f(12.0 * reffactor.x, 4.0 * reffactor.y - 12.0) / p.reflect_w, p.reflect_w, 1.0);
    reflected = mix(fog.rgb, reflected, fog.a);

    // The Fresnel term.
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * -eye_vec.z, 0.0, 1.0), 6.0);
    var result = mix(refracted, reflected, fresnel);

    // The light of the sun. The exponent is even, so abs gives the .fx's value for a negative
    // base too.
    let sun = water_sun_direction();
    let vdotr = dot(-eye_vec, vec3f(sun.x, sun.y, -sun.z));
    let spec = mod_sun_adjusted() * normal.z * pow(abs(vdotr), 300.0);
    result += spec * fog.a;

    // The shore line.
    result = mix(result, refracted, shorefactor * fog.a);
    return vec4f(result, 1.0);
}

// UnderwaterPS.
fn underwater_color(p: WaterPixel) -> vec4f {
    let eye_vec = p.eye_vec;
    let dist = p.distance;

    // The fog of this view, without the fog offset.
    let fog = clamp(exp(-dist / 4096.0), 0.0, 1.0);

    let normal = -water_normal(p.texcoords, dist, p.world.xy);
    let reffactor = 2.0 * (mod_wind_factor() * dist + 5.0) * normal.xy;

    // The refraction.
    var refracted = picture_at(p.uv + 2.0 * -reffactor.xy / p.clip_w);
    refracted = mix(water_fog_color(), refracted, exp(-dist / 500.0));

    // The reflection, at the place of the pixel (not the clamped one).
    let reflected = reflection_at(p.uv - vec2f(4.5 * reffactor.x, reffactor.y) / p.clip_w, p.clip_w, 1.0);

    // The Fresnel term, with the total internal reflection.
    let fresnel = pow(clamp(1.12 - 0.65 * dot(-eye_vec, normal), 0.0, 1.0), 8.0);
    let result = mix(refracted, reflected, fresnel);

    // The sun seen through the water (an even exponent, as above).
    let refractsun = dot(-eye_vec, normalize(-water_sun_direction() + normal));
    let spec = mod_sun_adjusted() * pow(abs(refractsun), 6.0) * fog;
    return vec4f(result + spec, 1.0);
}
