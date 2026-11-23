//! part: landscape

struct Land {
    terrain: vec4f,
    atlas: vec4f,
    atlas_more: vec4f,
    patterns: vec4f,
    range: vec4f,
}
@group(1) @binding(0) var<uniform> land: Land;
@group(1) @binding(1) var atlas: texture_2d<f32>;
@group(1) @binding(2) var material: texture_2d<f32>;
@group(1) @binding(3) var material_flags: texture_2d<f32>;
@group(1) @binding(4) var patch_albedo: texture_2d<f32>;
@group(1) @binding(5) var blend_patterns: texture_2d<f32>;
@group(1) @binding(6) var smooth_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) normal: vec3f,
    @location(1) color: vec3f,
    @location(2) local: vec2f,
    @location(3) world: vec3f,
}

// The matrix has the numbers of Direct3D, row after row. As the columns of a matrix here
// they are the transposed matrix, which is the one for a column vector.
@vertex fn vs(@location(0) position: vec3f, @location(1) normal: vec4f, @location(2) color: vec4f) -> Out {
    var out: Out;
    var world = position;
    if u.flags.y != 0.0 {
        // LandscapeReflVS: in a reflection the land near the water line sinks, so that the
        // edge of the water is clean; no landBias.
        world.z += -16.0 * clamp(1.0 - world.z / 16.0, 0.0, 1.0);
    } else {
        // landBias: the land goes down near the camera, so that a mesh with less detail
        // does not come through the land of the game.
        let distance = length(world - u.eye.xyz);
        world.z += -1.0 - 2.0 * max(0.0, land.range.x - 1152.0 - distance);
    }
    out.position = clip_of(world);
    out.normal = normal.xyz * 2.0 - 1.0;
    // The colour of a vertex has the bytes blue, green, red.
    out.color = color.bgr;
    out.local = world.xy - land.terrain.xy;
    out.world = world;
    return out;
}

// The view range of the game, for the fog (`fog_of`).
fn fog_range() -> f32 {
    return land.range.x;
}

fn byte_of(value: f32) -> f32 {
    return floor(value * 255.0 + 0.5);
}

fn atlas_uv(id: f32, patch_uv: vec2f) -> vec2f {
    let tile_y = floor(id / land.atlas.w);
    let origin = vec2f(id - tile_y * land.atlas.w, tile_y) * land.atlas.z;
    return (origin + land.atlas.y + patch_uv * land.atlas.x) * land.atlas_more.xy;
}

fn pattern_uv(id: f32, patch_uv: vec2f) -> vec2f {
    let rows = floor((land.atlas_more.z - 1.0) / land.patterns.w) + 1.0;
    let inverse_size = 1.0 / vec2f(land.patterns.w * land.patterns.z, rows * land.patterns.z);
    let tile_y = floor(id / land.patterns.w);
    let origin = vec2f(id - tile_y * land.patterns.w, tile_y) * land.patterns.z;
    return (origin + land.patterns.y + patch_uv * land.patterns.x) * inverse_size;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    // A patch has 512 units.
    let patch_coord = in.local / 512.0;
    let ddx_patch = dpdx(patch_coord);
    let ddy_patch = dpdy(patch_coord);
    let patch_uv = fract(patch_coord);
    let ddx_atlas = ddx_patch * land.atlas.x * land.atlas_more.xy;
    let ddy_atlas = ddy_patch * land.atlas.x * land.atlas_more.xy;
    let far_albedo = textureSample(patch_albedo, smooth_sampler, patch_coord * land.terrain.zw).rgb;

    let size = vec2i(textureDimensions(material));
    let cell = clamp(vec2i(floor(patch_coord)), vec2i(0), size - vec2i(1));
    let material_sample = textureLoad(material, cell, 0);
    let flags_sample = textureLoad(material_flags, cell, 0);
    let base_id = byte_of(material_sample.r) + 256.0 * byte_of(material_sample.g);
    let decal_id = byte_of(material_sample.b) + 256.0 * byte_of(material_sample.a);
    let pattern_id = min(byte_of(flags_sample.r), max(land.atlas_more.z - 1.0, 0.0));
    let has_decal = byte_of(flags_sample.g) % 2.0;
    let base = textureSampleGrad(atlas, smooth_sampler, atlas_uv(base_id, patch_uv), ddx_atlas, ddy_atlas).rgb;
    let decal = textureSampleGrad(atlas, smooth_sampler, atlas_uv(decal_id, patch_uv), ddx_atlas, ddy_atlas).rgb;
    let alpha = textureSampleLevel(blend_patterns, smooth_sampler, pattern_uv(pattern_id, patch_uv), 0.0).r * has_decal;
    let near_albedo = mix(base, decal, alpha);

    let footprint = max(length(ddx_patch), length(ddy_patch));
    let albedo = mix(near_albedo, far_albedo, smoothstep(0.5, 1.5, footprint));
    let normal = normalize(in.normal);
    let lamps = lamp_sums(in.world, normal);
    let light = u.ambient.rgb + u.sun_color.rgb * clamp(dot(normal, -u.sun_direction.xyz), 0.0, 1.0) + lamps.surface;
    let lit = albedo * in.color * light;
    if lamp_values.values.w > 1.5 {
        return vec4f(albedo * in.color * lamp_light(in.world, normal), 1.0);
    }
    if lamp_values.values.w > 0.5 {
        return vec4f(lamp_light(in.world, normal), 1.0);
    }

    // In a reflection, only one side of the water plane: flags.y is the side, flags.z the plane.
    if u.flags.y != 0.0 && (in.world.z - u.flags.z) * u.flags.y < 0.0 {
        discard;
    }
    return vec4f(fog_of(lit, in.world, lamps.air), 1.0);
}
