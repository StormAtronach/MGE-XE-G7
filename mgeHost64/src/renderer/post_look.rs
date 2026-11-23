//! The looks of post shaders of MGE XE mods on the wgpu renderer (W576, the owner's D606 and
//! D607): XERO-lut, Ultra Bloom and Eternal Shine. Each port gives the look of its effect file
//! with new code: no line of the effect files is in it. A port is found through `MOD_FILES` of `post.rs`, by the name and
//! the code key of the mod's file, so a chain that names the mod's shader gets the port.
//!
//! The inputs that a mod's Lua gave the shader on Direct3D 9 come from the helper here.

use super::post::{Shader, full};

/// The place of an effect file without a category in the order of the chain.
const NO_CATEGORY: i64 = 6_000_000;

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

/// The ports of this file, after those of `post.rs` and `post_jop.rs`.
pub const SHADERS: &[Shader] = &[Shader {
    name: "XERO-lut",
    source: XERO_LUT,
    passes: &[full("grade")],
    disable_flags: 0,
    no_sun_glare: false,
    priority: NO_CATEGORY,
    textures: &["XERO-lut.tga"],
    requires_hdr: false,
    variables: &[],
}];

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
}
