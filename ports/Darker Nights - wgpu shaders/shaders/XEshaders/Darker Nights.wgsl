//! interface: mge-xe-wgsl 1
//! passes: main
//! category: tone
//! mgeflags: 13
//! variables: nightDarkness f32 = 0.22, nightSaturation f32 = 1.12, nightContrast f32 = 1

// Darker Nights: the wgpu renderer's port of Darker Nights.fx.
// Exterior night darkening and saturation grade. Skip interiors, interior-like cells, and
// underwater scenes (mgeflags 13).

@fragment fn main(in: Quad) -> @location(0) vec4f {
    var scene = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0);

    // Fade the grade out as the sun climbs above the horizon.
    let night_amount = 1.0 - saturate(max(u.sunpos.z, 0.0) * 4.0);
    let saturation = mix(1.0, nightSaturation(), night_amount);
    let luminance = dot(scene.rgb, vec3f(0.2126, 0.7152, 0.0722));
    var graded = mix(vec3f(luminance), scene.rgb, saturation);
    graded = pow(saturate3(graded), vec3f(mix(1.0, nightContrast(), night_amount)));
    graded *= 1.0 - saturate(nightDarkness()) * night_amount;

    scene = vec4f(saturate3(graded), scene.a);
    return scene;
}
