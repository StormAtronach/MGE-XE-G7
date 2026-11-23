//! part: sky

// The dome of the game gets a height that puts its lower edge on the horizon.
fn extension_sky_position(position: vec3f) -> vec3f {
    return vec3f(position.xy, 50.0 * (position.z + 200.0));
}

// fogColourSky: the colour of the fog of the sky in a direction from the eye. The far fog
// colour at the horizon goes into the sky colour above it; with the scattering,
// fogColourScatter with a fog distance of 1 and no fog.
fn sky_fog_color(direction: vec3f) -> vec4f {
    let e = draw.extension;
    let up = 1.0 - pow(clamp(1.0 - 2.22 * clamp(direction.z - 0.075, 0.0, 1.0), 0.0, 1.0), 1.15);
    var color = mix(e[0].rgb, e[1].rgb, up);
    if e[1].w > 0.001 && e[2].w > 0.5 {
        let sun_cos = dot(direction, e[3].xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * e[6].z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(e[5].rgb, e[4].rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (e[6].x + 0.7 * mie);
        lost = (1.0 - exp(-lost)) / lost;
        let scattered = (0.125 * mie + e[2].rgb * rayleigh) * lost * (1.17 * air + 0.89) * e[6].y;
        color = mix(color, scattered, e[1].w);
    }
    // The alpha is the fog amount of fogColourScatter: 0 for the sky.
    return vec4f(color, 0.0);
}

fn extension_sky_color(direction: vec3f, pixel: vec2f) -> vec3f {
    let color = sky_fog_color(direction).rgb;
    // ditherSky: an ordered dither against bands in the sky.
    let dither = array<f32, 16>(0.001176, 0.001961, -0.001176, -0.001699, -0.000654, -0.000915, 0.000392, 0.000131, -0.000131, -0.001961, 0.000654, 0.000915, 0.001699, 0.001438, -0.000392, -0.001438);
    let at = vec2u(pixel) % 4u;
    return color + dither[at.x * 4u + at.y];
}
//! part: clouds_color

// CloudsPS: the colour of a pixel of the clouds.
fn clouds_color(p: CloudPixel) -> vec4f {
    return p.color * cloud_texture(p.texcoords);
}
