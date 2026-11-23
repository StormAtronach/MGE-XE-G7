//! interface: mge-xe-core 1

// XE Mod Sky.wgsl: the wgpu renderer's port of XE Mod Sky.fx of the older Watch the Skies of
// Morrowind Refreshed (md5 56b31f168d28ae2453e019552a53308e; the clouds rendering of XE Mod Sky by
// Dexter (vtastek), ported to MGE XE 0.16.0 and edited by tewlwolow). Put this file beside
// XE Mod Sky.fx in Data Files/shaders/core-mods: Direct3D 9 reads the .fx, the wgpu renderer this
// file. The .fx changes only CloudsPS, so this file has only the clouds hook; the sky dome keeps
// the renderer's colour.
//
// This .fx is the current Watch the Skies' XE Mod Sky.fx with other numbers. This file is the
// port of the pack "Watch the Skies - wgpu shaders" (render-link 306d40a) with the numbers of this
// .fx: disSample, dis1, dis3, timeFactor, incolFactor, N, the fog mix, the sun rim and the
// brightness of the halo.

//------------------------------------------------------------
// Tweakables: the numbers of the .fx. Change them here for the wgpu renderer.

const mod_dis_sample = 0.02; // the first displacement of the texture reads; default: 0.02
const mod_dis1 = 8.7; // the displacement of the clouds; higher: more fragmented; default: 8.7
const mod_dis3 = 0.0098; // default: 0.0098
const mod_time_factor = 0.0067; // the speed of the displacement; default: 0.0067
const mod_sun_amb_mult = 2.0; // how much the ambient light colours the far clouds; default: 2
const mod_incol_factor = 0.42; // how much the sun colours the result, a kind of contrast; default: 0.42

//------------------------------------------------------------
// CloudsPS of the .fx.

const mod_reads: i32 = 8; // the reads of the texture that are added up

fn clouds_color(p: CloudPixel) -> vec4f {
    let eye = p.direction;

    // The texture read eight times along the eye's direction, each fainter.
    var clouds = vec4f(0.0);
    for (var i = 1; i <= mod_reads; i++) {
        let sd = f32(i) / f32(mod_reads);
        // The .fx adds the float3 direction to the float2 coordinate: HLSL takes its x and y.
        clouds += exp(-sd * 0.1) * cloud_texture(p.texcoords + mod_dis_sample * (1.0 + sd) * eye.xy);
    }
    clouds /= f32(mod_reads);
    clouds = clamp(vec4f(1.0) - clouds, vec4f(0.0), vec4f(1.0));

    // The texture read again for the displacement, then at the displaced coordinate.
    let time = sky_time();
    let tur = cloud_texture(p.texcoords * mod_dis1 - vec2f(time * mod_time_factor, time * mod_time_factor));
    let tur2 = tur / 2.0;
    let ca = cloud_texture(p.texcoords - mod_dis3 * tur.r * tur2.a);

    // The colour of the sun and of the ambient light, then of the sky's fog.
    let sun = sky_sun_color();
    var rgb = mix(clamp(sky_ambient_color() * mod_sun_amb_mult, vec3f(0.0), vec3f(1.0)), sun, clouds.rgb);
    let fog = sky_fog_color(normalize(p.direction));
    rgb = mix(rgb, fog.rgb, 0.7);
    clouds = vec4f(ca.rgb * rgb * ca.a, ca.a);

    // The halo around the sun.
    let sunrim = clouds.r - 0.3;
    let sund = max(0.0, dot(eye, normalize(sky_sun_position())));
    let sunarea = 1.0 - pow(sund, 52.0);
    let sunarea2 = sunarea / 12.0;
    var incol = p.color;
    incol = vec4f(incol.rgb + (1.2 + sunrim + sunarea2) * (1.0 - sunarea) * ca.a, incol.a);

    return incol * mix(ca, clouds, sun.r * mod_incol_factor);
}
