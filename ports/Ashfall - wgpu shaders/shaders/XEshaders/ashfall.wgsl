//! interface: mge-xe-wgsl 1
//! passes: main
//! category: final
//! priority_adjust: 40
//! variables: temperature f32 = 20

fn temperatureEffect(uv: vec2f, color: vec3f, temperature: f32) -> vec3f {
    let coldColor = vec3f(0.0, 0.50, 1.0);
    let hotColor = vec3f(1.0, 0.5, 0.0);

    let colorTemperature = mix(coldColor, hotColor, f32(temperature > 0.0));

    let luminosity = dot(color, vec3f(0.299, 0.587, 0.114));
    let distanceFromCenter = distance(uv, vec2f(0.5, 0.5));
    let absTemp = abs(temperature);
    let distanceEffect = saturate(distanceFromCenter * (absTemp / 100.0) - 0.5);
    //Limit the color values to cold/hot depending on temperature
    let fadeAmount = saturate(abs(temperature) / 100.0) * luminosity * 2.0;
    return mix(color, colorTemperature, fadeAmount * distanceEffect);
}

@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(temperatureEffect(in.tex, color.rgb, temperature()), color.a);
}
