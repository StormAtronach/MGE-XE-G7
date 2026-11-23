//! interface: mge-xe-wgsl 1
//! passes: glow
//! category: scene
//! variables: radmult f32 = 0.3, int f32 = 0.25, ext f32 = 0.4, rain f32 = 0.5, near_eye f32 = 100, fog f32 = 0.75, storm f32 = 0.75
//! settings: radmult, int, ext, rain, near_eye, fog, storm

// Eternal Shine.wgsl: the wgpu renderer's port of Eternal Shine.fx of the mod Eternal Shine.
// Put this file beside Eternal Shine.fx in Data Files/shaders/XEshaders: Direct3D 9 reads the
// .fx and the mod's Lua gives it the lights; the wgpu renderer reads this file and gives it the
// lights of the frame itself.
//
// The settings are kept between game sessions in Eternal Shine.values.toml beside this file. A
// line of Lua sets one, for example mge.shaders.find({ name = "Eternal Shine" }).radmult = 0.4:
//   radmult   the multiplier of a light's radius for the glow (the mod's default 0.3)
//   int       the strength of the air in an interior (0.25)
//   ext       the strength of the air outside in clear, cloudy and overcast weather (0.4)
//   rain      the strength of the air in rain, thunder and snow (0.5)
//   near_eye  a light nearer to the eye than this, in units of the game, gives no glow (100)
//   fog       the strength of the air in fog (0.75)
//   storm     the strength of the air in ash, blight and blizzard (0.75)
//
// The differences from the mod on Direct3D 9:
// - The lights are those of the lit draws of the world in the frame. A light with no lit draw
//   in view gives no glow; the mod's Lua took every light of the active cells.
// - The strength of the air follows a change of the weather as it comes, and mixes the
//   strengths of the two weathers by the ratio of the change. The mod's Lua took the weather at
//   a change of cell only, with no mix.
// - Without the weather of the game (a d3d8.dll or a capture from before W622), rain, thunder,
//   snow and blizzard get rain, and the other weathers ext.
// - The colour of a light holds its dimmer, so the glow takes the dimmer squared.
// - No fade-in of 3 seconds after a light comes, and no skip of the lights of value 40000.
// - The mod's menu does not act on wgpu: its values are the settings above.

// The density that a ray gathers through a sphere of radius 1, from the eye to `far`, all in
// the units of the sphere: `o` is the eye relative to the centre, `d` the direction (length 1).
// Along the ray p(t) = o + t d the density is 1 - |p|^2 = h2 - (t + b)^2, with b = d . o and
// h2 = b^2 - |o|^2 + 1, the square of the half chord. With u = t + b the integral is
// h2 u - u^3 / 3, between the start of the ray or of the chord (u = max(b, -h)) and its end
// or the surface (u = min(h, far + b)). The chord through the centre gives 4/3; the scale 3/4
// makes it 1.
fn glow_density(o: vec3f, d: vec3f, far: f32) -> f32 {
    let b = dot(d, o);
    let h2 = b * b - dot(o, o) + 1.0;
    if h2 <= 0.0 {
        return 0.0;
    }
    let h = sqrt(h2);
    let u1 = max(-h, b);
    let u2 = min(h, far + b);
    if u2 <= u1 {
        return 0.0;
    }
    return 0.75 * (h2 * (u2 - u1) - (u2 * u2 * u2 - u1 * u1 * u1) / 3.0);
}

// The glow word of a light of the game's radius `radius` (`mult` of it, the radius at most
// 1000), and the radius of its sphere, which grows faster than the word.
fn glow_word(radius: f32, mult: f32) -> f32 {
    return min(radius, 1000.0) * mult;
}
fn glow_sphere(word: f32) -> f32 {
    return pow(word / 256.0, 1.0 / 0.6) * 2500.0;
}

// The glow of one light along the eye ray: `centre` relative to the eye, `dir` of length 1,
// `far` the distance to the surface of the pixel, `air` the strength of the air, `mult` the
// multiplier of the radius.
fn glow_light(colour: vec3f, radius: f32, mult: f32, centre: vec3f, dir: vec3f, far: f32, air: f32) -> vec3f {
    let word = glow_word(radius, mult);
    if word <= 0.0 {
        return vec3f(0.0);
    }
    let sphere = glow_sphere(word);
    let density = glow_density(-centre / sphere, dir, far / sphere);
    return air * 10.0 / word * colour * colour * (density * density * density);
}

// The strength of the air: `int` in an interior, outside the strength of the weather by the
// mod's table, and none in the light of a bright day (the sun's red).
fn glow_air() -> f32 {
    var air = ext();
    if u.sunamb.w > 0.5 {
        air = int();
    } else if weather_known() {
        // Clear, cloudy, foggy, overcast, rain, thunder, ash, blight, snow, blizzard.
        air = weather_mix(array<f32, 10>(ext(), ext(), fog(), ext(), rain(), rain(), storm(), storm(), rain(), storm()));
    } else if u.switches.z > 0.0 {
        air = rain();
    }
    return air * saturate(1.0 - u.suncol.r);
}

// The eye ray of a point of the picture in the world, with a view depth of 1.
fn eye_ray(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

// Half a level of 8 bits, up or down in the order of a 4 x 4 Bayer cell: no bands in the
// soft edge of a glow.
fn dither(pixel: vec2f) -> f32 {
    let order = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let cell = vec2u(pixel) % 4u;
    return ((order[cell.y * 4u + cell.x] + 0.5) / 16.0 - 0.5) / 255.0;
}

const GLOW_SKY: f32 = 1.0e7;

// The glows of all the lights of the frame, added to the picture in linear light. The sum
// stops at 1, as the 8-bit passes of the mod cut it.
@fragment fn glow(in: Quad) -> @location(0) vec4f {
    let picture = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let count = frame_light_info.count;
    if count == 0u {
        return picture;
    }
    let shaken = saturate3(picture.rgb + dither(in.position.xy));
    let air = glow_air();
    if air <= 0.0 {
        return vec4f(shaken, picture.a);
    }
    let ray = eye_ray(in.tex);
    let along = length(ray);
    let dir = ray / along;
    let far = min(textureSampleLevel(depthframe, point_clamp, in.tex, 0.0).r, GLOW_SKY) * along;
    let offset = frame_light_info.offset.xyz;
    let mult = radmult();
    // A light nearer to the eye than this gives no glow: the light that the player holds in
    // first person.
    let near = near_eye();
    var glow = vec3f(0.0);
    for (var i = 0u; i < count; i += 1u) {
        if frame_lights[i].position.w > 2.5 || frame_lights[i].cone.z <= 0.0 {
            continue;
        }
        let centre = frame_lights[i].position.xyz + offset;
        if dot(centre, centre) < near * near {
            continue;
        }
        glow += glow_light(frame_lights[i].diffuse.rgb, frame_lights[i].cone.z, mult, centre, dir, far, air);
    }
    let lit = pow(pow(picture.rgb, vec3f(2.2)) + min(glow, vec3f(1.0)), vec3f(1.0 / 2.2));
    return vec4f(saturate3(lit + dither(in.position.xy)), picture.a);
}
