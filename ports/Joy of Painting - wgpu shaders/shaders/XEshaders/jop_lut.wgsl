//! interface: mge-xe-wgsl 1
//! passes: lut
//! category: final
//! priority_adjust: 160
//! texture: jop/luts/neutral.tga, jop/luts/saturated.tga, jop/luts/desaturated.tga, jop/luts/warm.tga, jop/luts/cold.tga, jop/luts/hueShifted_1.tga, jop/luts/hueShifted_2.tga, jop/luts/hueShifted_3.tga, jop/luts/sepia.tga, jop/luts/radioactive.tga
//! variables: aspectRatio f32 = 1.3, viewportSize f32 = 0.8, selectedLut f32 = 1

fn ClutFunc(colorIN: vec3f, t: texture_2d<f32>) -> vec3f {
    let pSize = vec2f(0.00390625, 0.0625);
    let c = saturate3(colorIN) * 15.0;
    let w = floor(c.b);
    var xy = (c.rg + 0.5) * pSize;
    xy.x += w * pSize.y;
    let z = xy.x + pSize.y;
    return mix(textureSampleLevel(t, linear_wrap, xy, 0.0).rgb, textureSampleLevel(t, linear_wrap, vec2f(z, xy.y), 0.0).rgb, c.b - w);
}

@fragment fn lut(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var rgb = scene.rgb;
    switch i32(round(selectedLut())) {
        case 1 { rgb = ClutFunc(rgb, tex1); }
        case 2 { rgb = ClutFunc(rgb, tex2); }
        case 3 { rgb = ClutFunc(rgb, tex3); }
        case 4 { rgb = ClutFunc(rgb, tex4); }
        case 5 { rgb = ClutFunc(rgb, tex5); }
        case 6 { rgb = ClutFunc(rgb, tex6); }
        case 7 { rgb = ClutFunc(rgb, tex7); }
        case 8 { rgb = ClutFunc(rgb, tex8); }
        case 9 { rgb = ClutFunc(rgb, tex9); }
        case 10 { rgb = ClutFunc(rgb, tex10); }
        default {}
    }
    return vec4f(rgb, scene.a);
}
