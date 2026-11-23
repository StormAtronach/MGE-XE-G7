//! part: view

struct View {
    view_projection: mat4x4f,
    eye: vec4f,
    fog_color: vec4f,
    fog: vec4f,
    flags: vec4f,
    sun_direction: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    sun_pos: vec4f,
    sky: vec4f,
    outscatter: vec4f,
    inscatter: vec4f,
    sun_altitude: vec4f,
    relative: mat4x4f,
    debug: vec4f,
}
@group(0) @binding(0) var<uniform> u: View;

// The debug view `fog` (`View::debug` x): a draw writes the part of the fog that it applied.
fn fog_view_on() -> bool {
    return u.debug.x > 0.5;
}

// The clip position of a position in the space of the world, by a matrix that is relative to
// the eye (made as `View::relative` makes it, for the projection that the caller needs): the
// position less the eye goes through the matrix, never the absolute position.
fn clip_with(relative: mat4x4f, world: vec3f) -> vec4f {
    return relative * vec4f(world - u.eye.xyz, 1.0);
}

// The clip position of a position in the space of the world, for the projection of this view
// (see `View::relative`).
fn clip_of(world: vec3f) -> vec4f {
    return clip_with(u.relative, world);
}
//! part: fog

fn fog_of(lit: vec3f, world: vec3f, lamp_air: vec3f) -> vec3f {
    return fog_with(lit, world, true, lamp_air);
}

// `fog_of` with a switch for the scattering: without it, the light that the air adds is left out.
// `lamp_air` is the lamps' light in the air (`lamp_sums`), for their haze in the fog.
fn fog_with(lit: vec3f, world: vec3f, scatter: bool, lamp_air: vec3f) -> vec3f {
    // fogColour: the exponential curve past the view range of the game, and the linear fog
    // of the near cells before it.
    let to_pixel = world - u.eye.xyz;
    let distance = length(to_pixel);
    let linear = clamp((u.fog.w - distance) / max(u.fog.w - u.fog.z, 1.0), 0.0, 1.0);
    let fog_distance = (distance - u.fog.x) / max(u.fog.y, 1.0);
    let exponential = clamp(exp(-fog_distance), 0.0, 1.0);
    let fog = select(linear, exponential, u.flags.x > 0.5 && distance > fog_range());
    if fog_view_on() {
        return vec3f(1.0 - fog);
    }
    // fogColourScatter: the light that the air adds, in good weather and over the water.
    var added = u.fog_color.rgb * (1.0 - fog);
    if scatter && u.sky.w > 0.5 && u.sun_pos.w > 0.001 && u.eye.z > u.flags.w {
        let direction = to_pixel / max(distance, 1.0);
        let depth = clamp(0.224 * fog_distance, 0.0, 1.0);
        let sun_cos = dot(direction, u.sun_pos.xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * u.sun_altitude.z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(u.inscatter.rgb, u.outscatter.rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (u.sun_altitude.x + 0.7 * mie);
        lost = (1.0 - exp(-depth * lost)) / lost;
        let scattered = (0.125 * mie + u.sky.rgb * rayleigh) * lost * (1.17 * air + 0.89) * u.sun_altitude.y;
        added = mix(added, scattered, u.sun_pos.w);
    }
    return fog * lit + added + (1.0 - fog) * lamp_haze(lamp_air, fog, distance);
}

// The haze of the lamps in the fog (D498) at the distance d, where `fog` is the part of the
// pixel's own colour (1 near, 0 in full fog). The light `lamp_air` in the air is held below 1
// with its colour kept, fades with the fog to the power 1 / reach, and ramps in after the
// game's view range and out before the draw distance. No haze with the eye at or under the
// water plane: the test of the light that the air adds from the sun in `fog_with`, so the
// main view and the mirror under the water have none.
fn lamp_haze(lamp_air: vec3f, fog: f32, d: f32) -> vec3f {
    let k = lamp_values.haze.x;
    if k <= 0.0 || u.eye.z <= u.flags.w {
        return vec3f(0.0);
    }
    let v = k * lamp_air;
    let held = v / (1.0 + max(v.r, max(v.g, v.b)));
    let near = clamp((d - fog_range()) / max(lamp_values.haze.z, 1.0), 0.0, 1.0);
    let far = clamp((lamp_values.haze_end.x - d) / max(lamp_values.haze.w, 1.0), 0.0, 1.0);
    return held * pow(fog, lamp_values.haze.y) * near * far;
}
