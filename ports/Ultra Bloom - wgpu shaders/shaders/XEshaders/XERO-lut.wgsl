//! interface: mge-xe-wgsl 1
//! passes: grade
//! category: none
//! texture: XERO-lut.tga

fn lut_grade(table: texture_2d<f32>, table_sampler: sampler, color: vec3f) -> vec3f {
    let size = vec2f(textureDimensions(table));
    let n = size.y;
    let cell = clamp(color, vec3f(0.0), vec3f(1.0)) * (n - 1.0);
    let square = min(floor(cell.b), n - 2.0);
    let between = cell.b - square;
    let low = (cell.rg + 0.5) / size + vec2f(square * n / size.x, 0.0);
    let high = low + vec2f(n / size.x, 0.0);
    let a = textureSampleLevel(table, table_sampler, low, 0.0).rgb;
    let b = textureSampleLevel(table, table_sampler, high, 0.0).rgb;
    return mix(a, b, between);
}

@fragment fn grade(in: Quad) -> @location(0) vec4f {
    let picture = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(lut_grade(tex1, linear_clamp, picture.rgb), picture.a);
}
