//! The looks of post shaders of MGE XE mods on the wgpu renderer (W576, the owner's D606 and
//! D607): XERO-lut, Ultra Bloom and Eternal Shine. Each port gives the look of its effect file
//! with new code: no line of the effect files is in it. A port is found through `MOD_FILES` of
//! `post.rs`, by the name and the code key of the mod's file, so a chain that names the mod's
//! shader gets the port.
//!
//! The inputs that a mod's Lua gave the shader on Direct3D 9 come from the helper here.
//!
//! Eternal Shine takes the lights of the frame from the renderer (`frame_lights` of the common
//! part of `post.rs`) in place of the mod's Lua. Its values are variables of the shader with the
//! Lua's defaults: the glow word of a light is `radmult` (0.3) of its radius (at most 1000), and
//! the air is `int` (0.25) in an interior, `rain` (0.5) in rain, thunder and snow, else `ext`
//! (0.4). A shader-state record that names one sets it. A Lua set does not reach the port yet:
//! d3d8.dll lists only the variables of the effect file, and MWSE sets only listed variables
//! (W576 part 4 adds the names and a file that keeps the values). The differences from the
//! mod on Direct3D 9:
//! - The lights are those of the lit draws of the world in the frame. A light with no lit draw
//!   in view gives no glow; the Lua took every light of the active cells.
//! - Fog, ash, blight and blizzard get 0.4 or 0.5: the helper does not know these weathers
//!   (the Lua gave them 0.75).
//! - The colour of a light holds its dimmer, so the glow takes the dimmer squared; the Lua took
//!   it once.
//! - No fade-in of 3 seconds after a light comes, and no skip of the lights of value 40000.
//! - A light within `near_eye` (100) units of the eye gives no glow: the player's own light in
//!   first person, which would put the eye inside its glow. The Lua left out the player's light
//!   in every view; here the player's light in a far third-person view glows as the others (the
//!   owner's choice, 2026-11-23).
//! - The mod's menu does not act on wgpu: its values are the variables above.

use super::post::{Shader, Variable, full};

/// A variable that a mod sets from Lua: one value, with the value of the effect file.
const fn value(name: &'static str, default: &'static [f32; 1]) -> Variable {
    Variable {
        name,
        slots: 1,
        per: 1,
        default,
    }
}

/// The place of an effect file without a category in the order of the chain.
const NO_CATEGORY: i64 = 6_000_000;
/// The place of an effect file of the category scene.
const SCENE: i64 = 1_000_000;

/// The colour grade through a table of N x N x N colours, stored as a strip of N squares
/// side by side, N x N each: red across a square, green down, blue from square to square.
/// The strip is N * N texels wide and N high (256 x 16 for N = 16). A colour reads the two
/// squares around its blue, each with a linear read, and mixes them by the blue between.
/// The two reads stay inside the strip: the last colour of blue reads the last square with
/// the full weight.
macro_rules! lut_lookup {
    () => {
        "
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
"
    };
}

/// XERO-lut: the picture through the colour table `XERO-lut.tga` (`tex1`).
const XERO_LUT: &str = concat!(
    lut_lookup!(),
    "
@fragment fn grade(in: Quad) -> @location(0) vec4f {
    let picture = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(lut_grade(tex1, linear_clamp, picture.rgb), picture.a);
}
"
);

/// Ultra Bloom: a bloom that squares its blur taps, a red halo on the brightest pixels, and a
/// tone pass. The mod's Lua set `exposure` by the hour and the cell; here the exposure comes
/// from the night of the environment (`u.switches.y`) and the interior flag, with the values of
/// the mod's menu at their defaults: 1 in the day, 2 at night, 1.5 in an interior. The other
/// values of the menu stay variables (`sun` is 1 less the bloom power).
const ULTRA_BLOOM: &str = "
fn night() -> f32 {
    return u.switches.y;
}
fn is_interior() -> bool {
    return u.sunamb.w > 0.5;
}
fn fog_range() -> f32 {
    return u.fognearcol.w;
}

// The sky in the depth frame: no surface there (1e38).
const SKY_DEPTH: f32 = 1.0e7;

// The share of the effect that a pixel keeps by its depth, in an exterior: sky() for the sky;
// all of it up to dist1() fog ranges, then less, to none dist2() fog ranges farther.
fn far_share(tex: vec2f) -> f32 {
    if is_interior() {
        return 1.0;
    }
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    if depth > SKY_DEPTH {
        return min(sky(), 1.0);
    }
    let start = fog_range() * dist1();
    return saturate(1.0 - max(depth - start, 0.0) / (fog_range() * dist2()));
}

// The light that blooms: each channel softly from the threshold sun() up to 1. The alpha is
// the hot mask: the brightest pixels only.
@fragment fn bright(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    let glow = smoothstep(vec3f(sun()), vec3f(1.0), color) * far_share(in.tex);
    let hot = smoothstep(0.9, 1.0, pow(length(color), 1.0 / 4.5));
    return vec4f(glow, hot);
}

// The blur: 17 taps 2 texels apart (the middle one, and 8 to each side at 1.5, 3.5 ... 15.5
// texels), weighted by a bell of variance 103 texels squared; the middle tap has half a
// weight. The step is a texel over lerp(1.66, 6, the red of the sun), times radmult(): a
// bright sun makes the glow tighter.
const VARIANCE: f32 = 103.0;
fn weight(x: f32) -> f32 {
    return exp(-x * x / (2.0 * VARIANCE));
}
fn weight_sum() -> f32 {
    var sum = 0.5 * weight(0.0);
    for (var i = 0; i < 8; i++) {
        sum += 2.0 * weight(1.5 + 2.0 * f32(i));
    }
    return sum;
}
fn tap_step(axis: vec2f) -> vec2f {
    let radius = mix(1.66, 6.0, u.suncol.r) / radmult();
    return axis * rcpres() / radius;
}

// Across: each tap is squared after its weight, so that the brightest parts lead. The 8-bit
// target cuts the sum at 1, as the target of Direct3D 9 did.
@fragment fn blur_across(in: Quad) -> @location(0) vec4f {
    let step = tap_step(vec2f(1.0, 0.0));
    let middle = 0.5 * weight(0.0) * textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    var sum = middle * middle;
    for (var i = 0; i < 8; i++) {
        let x = 1.5 + 2.0 * f32(i);
        let w = weight(x);
        let a = w * textureSampleLevel(lastpass, linear_clamp, in.tex + x * step, 0.0);
        let b = w * textureSampleLevel(lastpass, linear_clamp, in.tex - x * step, 0.0);
        sum += a * a + b * b;
    }
    let total = weight_sum();
    return 16.0 * sum / (total * total);
}

// Down, with the squared weights; then onto the picture in gamma 2: the glow added, the red
// halo of the hot mask, a soft curve 2x / (1 + 2x), and back.
@fragment fn blur_down_combine(in: Quad) -> @location(0) vec4f {
    let step = tap_step(vec2f(0.0, 1.0));
    let w0 = 0.5 * weight(0.0);
    var sum = w0 * w0 * textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    for (var i = 0; i < 8; i++) {
        let x = 1.5 + 2.0 * f32(i);
        let w = weight(x);
        let pair = textureSampleLevel(lastpass, linear_clamp, in.tex + x * step, 0.0)
            + textureSampleLevel(lastpass, linear_clamp, in.tex - x * step, 0.0);
        sum += w * w * pair;
    }
    let total = weight_sum();
    let glow = 16.0 * sum / (total * total);
    let picture = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    var color = picture * picture + glow.rgb;
    let halo = 3.0 * glow.rgb * glow.a * vec3f(0.6, 0.0091, 0.0);
    color += saturate3(halo - color);
    color = 2.0 * color / (1.0 + 2.0 * color);
    return vec4f(sqrt(color), 1.0);
}

// The tone: an exposure curve in gamma 2.2, the contrast toward a quintic S curve, and the
// saturation, stronger in bright pixels. The sky and the far land go back to the picture
// before the shader, as in the bright pass.
fn exposure() -> f32 {
    if is_interior() {
        return 1.5;
    }
    return 1.0 + night();
}
@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let lit = pow(textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0).rgb, vec3f(2.2)) * exposure();
    var x = pow(lit / (1.0 + lit), vec3f(1.0 / 2.2));
    let s = x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
    x = mix(x, s, contrast());
    let grey = dot(vec3f(0.2125, 0.7154, 0.0721), x);
    let saturated = mix(vec3f(grey), x, saturation());
    let color = mix(x, saturated, 0.33 * (x.r + x.g + x.b));
    let before = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    return vec4f(mix(before, color, far_share(in.tex)), 1.0);
}
";

/// The glow of a light: a sphere around it, its density 1 at the centre and 0 at the edge, as
/// 1 - (d / R)^2 for the distance d from the centre and the radius R of the sphere. The eye
/// ray of a pixel gathers the density up to the surface of the pixel; the glow is that density
/// cubed, in the colour of the light squared. No term reads the frame: a test runs them alone.
macro_rules! glow_math {
    () => {
        "
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
"
    };
}

const ETERNAL_SHINE: &str = concat!(
    glow_math!(),
    "
// The strength of the air: thin in an interior, thick in rain, thunder and snow, and none in
// the light of a bright day (the sun's red). The variables `int`, `rain` and `ext` hold the
// three strengths.
fn glow_air() -> f32 {
    var air = ext();
    if u.sunamb.w > 0.5 {
        air = int();
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
"
);

/// The values of Eternal Shine that a player or a mod can set from Lua, with the defaults of
/// the mod's Lua (its names; `near_eye` is ours): the multiplier of a light's radius, the
/// strength of the air in an interior, outside, and in rain, thunder and snow, and the
/// distance from the eye within which a light gives no glow.
const ETERNAL_SHINE_VALUES: &[Variable] = &[
    value("radmult", &[0.3]),
    value("int", &[0.25]),
    value("ext", &[0.4]),
    value("rain", &[0.5]),
    value("near_eye", &[100.0]),
];

/// The ports of this file, after those of `post.rs` and `post_jop.rs`.
pub const SHADERS: &[Shader] = &[
    Shader {
        name: "XERO-lut",
        source: XERO_LUT,
        passes: &[full("grade")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: NO_CATEGORY,
        textures: &["XERO-lut.tga"],
        requires_hdr: false,
        variables: &[],
    },
    // The passes go into 8-bit targets of the picture's size, as on Direct3D 9: the blur's
    // sums cut at 1 there, and that is a part of the look.
    Shader {
        name: "Ultra Bloom",
        source: ULTRA_BLOOM,
        passes: &[full("bright"), full("blur_across"), full("blur_down_combine"), full("tone")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: NO_CATEGORY,
        textures: &[],
        requires_hdr: false,
        variables: &[
            value("radmult", &[1.0]),
            value("contrast", &[0.5]),
            value("saturation", &[1.5]),
            value("sun", &[0.3]),
            value("sky", &[0.0]),
            value("dist1", &[1.5]),
            value("dist2", &[0.5]),
        ],
    },
    // One pass loops all the lights of the frame; the mod's Lua split them over four passes.
    Shader {
        name: "Eternal Shine",
        source: ETERNAL_SHINE,
        passes: &[full("glow")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: SCENE,
        textures: &[],
        requires_hdr: false,
        variables: ETERNAL_SHINE_VALUES,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use nirender_wgpu::objects::{Create, CreateView};
    use nirender_wgpu::wgpu;

    /// The size of a side of the test table.
    const N: u32 = 16;

    /// The colours of the test: the corners of the cube, the middle, and colours between the
    /// texels and between two squares of blue.
    const COLOURS: [[f32; 3]; 8] = [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 1.0],
        [0.5, 0.5, 0.5],
        [0.1, 0.62, 0.93],
        [0.97, 0.03, 0.51],
        [0.33, 0.77, 0.04],
    ];

    /// The table that gives each colour back: the texel of red `r`, green `g` and blue `b`
    /// holds that colour.
    fn identity_table() -> Vec<u8> {
        let mut texels = Vec::with_capacity((N * N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N * N {
                let (r, g, b) = (x % N, y, x / N);
                let level = |i: u32| (i as f32 * 255.0 / (N - 1) as f32).round() as u8;
                texels.extend_from_slice(&[level(r), level(g), level(b), 255]);
            }
        }
        texels
    }

    /// Runs `lut_grade` on the GPU for each of `colours` with `table` (N * N x N texels) and
    /// gives the results, 0 to 255.
    fn graded(table: &[u8], colours: &[[f32; 3]]) -> Vec<[u8; 3]> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let texture = |width: u32, height: u32, usage: wgpu::TextureUsages| {
            device.counted_texture(&wgpu::TextureDescriptor {
                label: Some("test lut"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            })
        };
        let lut = texture(N * N, N, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            lut.as_image_copy(),
            table,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N * N * 4),
                rows_per_image: None,
            },
            lut.size(),
        );
        // One pixel for each colour, in a row: the colours are constants of the shader.
        let width = colours.len() as u32;
        let target = texture(
            width,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let list = colours
            .iter()
            .map(|[r, g, b]| format!("vec3f({r:?}, {g:?}, {b:?})"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            "{}
@group(0) @binding(0) var table: texture_2d<f32>;
@group(0) @binding(1) var table_sampler: sampler;
const COLOURS = array<vec3f, {width}>({list});
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {{
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {{
    return vec4f(lut_grade(table, table_sampler, COLOURS[u32(position.x)]), 1.0);
}}
",
            lut_lookup!()
        );
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test lut"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test lut"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let linear = device.counted_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let lut_view = lut.counted_view(&Default::default());
        let group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("test lut"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&linear),
                },
            ],
        });
        // A row of a copy to a buffer is a multiple of 256 bytes.
        let readback = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("test lut"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let target_view = target.counted_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test lut"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Default::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        queue.submit([encoder.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("the map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("the poll");
        let bytes = readback.slice(..).get_mapped_range().expect("the mapped range");
        bytes.chunks(4).take(colours.len()).map(|t| [t[0], t[1], t[2]]).collect()
    }

    /// The table that gives each colour back gives back every colour: the corners, the
    /// middle, and colours between texels and between squares of blue. A wrong square, a
    /// wrong place in a square or a wrong weight of blue moves a colour.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_identity_table_gives_each_colour_back() {
        let results = graded(&identity_table(), &COLOURS);
        for (colour, result) in COLOURS.iter().zip(&results) {
            for (want, got) in colour.iter().zip(result) {
                let want = want * 255.0;
                assert!((want - *got as f32).abs() <= 2.0, "the colour {colour:?} gave {result:?}");
            }
        }
    }

    /// The port and the test use the same lookup.
    #[test]
    fn the_port_has_the_lookup() {
        assert!(XERO_LUT.starts_with(lut_lookup!()));
    }

    /// Runs `cases`, WGSL with `fn case_of(i: u32) -> vec4f` after the glow terms, for `count`
    /// cases (at most 16) on the GPU, and gives the results.
    fn glow_cases(cases: &str, count: u32) -> Vec<[f32; 4]> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let target = device.counted_texture(&wgpu::TextureDescriptor {
            label: Some("test glow"),
            size: wgpu::Extent3d {
                width: count,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let source = format!(
            "{}{cases}
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {{
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {{
    return case_of(u32(position.x));
}}
",
            glow_math!()
        );
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test glow"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test glow"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba32Float.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let readback = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("test glow"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let target_view = target.counted_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test glow"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Default::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        queue.submit([encoder.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("the map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("the poll");
        let bytes = readback.slice(..).get_mapped_range().expect("the mapped range");
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        floats
            .chunks(4)
            .take(count as usize)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }

    /// The density by a sum of small steps along the ray, in the units of the sphere.
    fn density_by_steps(o: [f32; 3], d: [f32; 3], far: f32) -> f32 {
        const STEPS: usize = 200_000;
        let (o, d, far) = (o.map(f64::from), d.map(f64::from), f64::from(far));
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let end = far.min(10.0);
        let step = end / STEPS as f64;
        let mut sum = 0.0;
        for i in 0..STEPS {
            let t = (i as f64 + 0.5) * step;
            let p: Vec<f64> = (0..3).map(|a| o[a] + t * d[a] / length).collect();
            sum += (1.0 - (p[0] * p[0] + p[1] * p[1] + p[2] * p[2])).max(0.0) * step;
        }
        (sum * 0.75) as f32
    }

    /// The density of the shader is the sum along the ray: through the centre, off the centre,
    /// from inside the sphere, cut by a surface, behind the eye, and past the sphere.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_density_is_the_sum_along_the_ray() {
        let cases: [([f32; 3], [f32; 3], f32); 6] = [
            ([0.0, -3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
            ([0.4, -3.0, 0.2], [0.0, 1.0, 0.0], 100.0),
            ([0.1, 0.2, -0.3], [0.6, 0.0, 0.8], 100.0),
            ([0.0, -3.0, 0.0], [0.0, 1.0, 0.0], 2.7),
            ([0.0, 3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
            ([1.5, -3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
        ];
        let list = cases
            .iter()
            .map(|(o, d, far)| {
                format!(
                    "glow_density(vec3f({:?}, {:?}, {:?}), vec3f({:?}, {:?}, {:?}), {far:?})",
                    o[0], o[1], o[2], d[0], d[1], d[2]
                )
            })
            .collect::<Vec<_>>();
        let body = list
            .iter()
            .enumerate()
            .map(|(i, call)| format!("    if i == {i}u {{ return vec4f({call}); }}\n"))
            .collect::<String>();
        let results = glow_cases(
            &format!("fn case_of(i: u32) -> vec4f {{\n{body}    return vec4f(0.0);\n}}\n"),
            6,
        );
        for ((o, d, far), result) in cases.iter().zip(&results) {
            let want = density_by_steps(*o, *d, *far);
            assert!(
                (want - result[0]).abs() < 1e-3,
                "the ray from {o:?} along {d:?} to {far}: the shader gave {}, the sum {want}",
                result[0]
            );
        }
        // The ray through the centre gathers 1, and the ray past the sphere none.
        assert!((results[0][0] - 1.0).abs() < 1e-4);
        assert_eq!(results[5][0], 0.0);
    }

    /// A light of radius 1000 reaches 1000 units, and its glow sphere is about 3256: a ray that
    /// passes 2000 units from it, out of its reach, gets the glow. A ray past the sphere, and a
    /// pixel whose surface is before the sphere, get none.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_pixel_in_the_glow_and_out_of_the_reach_gets_the_glow() {
        let cases = "
fn case_of(i: u32) -> vec4f {
    let colour = vec3f(1.0, 0.8, 0.5);
    let centre = vec3f(2000.0, 6000.0, 0.0);
    let dir = vec3f(0.0, 1.0, 0.0);
    if i == 0u {
        return vec4f(glow_light(colour, 1000.0, 0.3, centre, dir, 1.0e7, 1.0), glow_sphere(glow_word(1000.0, 0.3)));
    }
    if i == 1u {
        return vec4f(glow_light(colour, 1000.0, 0.3, vec3f(4000.0, 6000.0, 0.0), dir, 1.0e7, 1.0), 0.0);
    }
    return vec4f(glow_light(colour, 1000.0, 0.3, centre, dir, 500.0, 1.0), 0.0);
}
";
        let results = glow_cases(cases, 3);
        let sphere = results[0][3];
        assert!(
            (sphere - 3256.0).abs() < 5.0,
            "the sphere of a light of radius 1000: {sphere}"
        );
        assert!(
            results[0][0] > 0.0 && results[0][1] > 0.0,
            "the glow out of the reach: {:?}",
            results[0]
        );
        assert_eq!(results[1][..3], [0.0; 3], "a ray past the sphere");
        assert_eq!(results[2][..3], [0.0; 3], "a surface before the sphere");
    }

    /// The port reads the lights of the frame (the chain then asks for them) and has the
    /// glow terms of the tests.
    #[test]
    fn the_glow_port_reads_the_lights_and_has_the_terms() {
        assert!(ETERNAL_SHINE.starts_with(glow_math!()));
        assert!(ETERNAL_SHINE.contains("frame_lights"));
    }

    /// The values that a player sets from Lua start at the defaults of the mod's Lua
    /// (`mwse.loadConfig` of its main.lua: radmult 0.3, int 0.25, ext 0.4, rain 0.5) and the
    /// owner's 100 units from the eye, and the port reads each one.
    #[test]
    fn the_glow_values_start_at_the_defaults_of_the_lua() {
        let want = [
            ("radmult", 0.3),
            ("int", 0.25),
            ("ext", 0.4),
            ("rain", 0.5),
            ("near_eye", 100.0),
        ];
        let port = SHADERS.iter().find(|s| s.name == "Eternal Shine").expect("the port");
        let have: Vec<(&str, f32)> = port.variables.iter().map(|v| (v.name, v.default[0])).collect();
        assert_eq!(have, want);
        for (name, _) in want {
            assert!(ETERNAL_SHINE.contains(&format!("{name}()")), "the port reads {name}");
        }
    }
}
