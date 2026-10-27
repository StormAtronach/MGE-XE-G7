//! The blend between the draws of the game and the distant land: `MGEBlendPS` of `XE Main.fx`.
//!
//! The draws of the game end at the view range of the game. In the last 512 units before it,
//! a pixel of the game goes into the picture of the distant land that was there before the
//! game drew.

use nirender_wgpu::{SceneView, wgpu};

const SHADER: &str = "
struct Uniforms {
    // The two numbers of the projection that give the depth, the distance where the blend
    // starts, and its length.
    values: vec4f,
    // The two numbers of the projection for the width and the height, and the size of the
    // picture.
    more: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var before: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;

@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    // One triangle that covers the picture.
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}

@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let at = vec2i(position.xy);
    let bound = u.values.z;
    let w = blend_distance(depth, at, u.values.xy, u.more.xy, u.more.zw, bound);
    // Where the game drew nothing, the picture is that of the distant land already.
    if w > 1e29 {
        discard;
    }
    let v = (w - bound) / u.values.w;
    if v < 0.0 {
        discard;
    }
    return vec4f(textureLoad(before, at, 0).rgb, clamp(v, 0.0, 1.0));
}
";

/// The distance by which the blend takes a pixel of the game into the distant land: for the
/// blend, and for the caustics, which go out with it (`caustics.rs`).
pub(super) const DISTANCE: &str = "
// The distance from the camera at a pixel, from the depth of the game. `depth_of` has the depth
// terms of the projection (`depth_empty`, `view_distance`), `scale` the two for the width and the height,
// and `size` is the size of the picture. A pixel that the game did not draw is very far. The
// original has the distance along the view of the camera; the fog of the draws of the game is
// on the distance from the camera here, and the blend must start where that fog ends for each
// direction of the camera.
fn blend_distance_at(depth_texture: texture_depth_2d, at: vec2i, depth_of: vec2f, scale: vec2f, size: vec2f) -> f32 {
    let limit = vec2i(textureDimensions(depth_texture));
    let d = textureLoad(depth_texture, clamp(at, vec2i(0), limit - 1), 0);
    if depth_empty(d) {
        return 1e30;
    }
    let ndc = (vec2f(at) + 0.5) / size * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    return view_distance(d, depth_of) * length(vec3f(ndc / scale, 1.0));
}

// The distance of a pixel for the blend: past `bound`, the nearest of the pixel and its four
// neighbours, so that the edge of a thing goes into the distant land with the thing.
fn blend_distance(depth_texture: texture_depth_2d, at: vec2i, depth_of: vec2f, scale: vec2f, size: vec2f, bound: f32) -> f32 {
    var w = blend_distance_at(depth_texture, at, depth_of, scale, size);
    if w > bound {
        w = min(w, blend_distance_at(depth_texture, at + vec2i(-1, 0), depth_of, scale, size));
        w = min(w, blend_distance_at(depth_texture, at + vec2i(0, -1), depth_of, scale, size));
        w = min(w, blend_distance_at(depth_texture, at + vec2i(1, 0), depth_of, scale, size));
        w = min(w, blend_distance_at(depth_texture, at + vec2i(0, 1), depth_of, scale, size));
    }
    return w;
}
";

fn shader() -> String {
    format!("{}{}{}", nirender_wgpu::depth::wgsl(), SHADER, DISTANCE)
}

/// The length of the blend, in the units of the game: `zone` of `MGEBlendPS`.
pub(super) const ZONE: f32 = 512.0;

pub struct Blend {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The frames with a blend, for the report.
    pub frames: u64,
}

impl Blend {
    pub fn new(scene: &SceneView) -> Self {
        let device = scene.device;
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Float { filterable: false }),
                texture(2, wgpu::TextureSampleType::Depth),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blend"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("blend"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blend"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            // The blend draws into the colour target of the game.
            multisample: scene.multisample(false),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene.target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("blend"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, layout, uniforms, frames: 0 }
    }

    /// Draws the blend. `near_view_range` is the view range of the game.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, before: &wgpu::TextureView, depth: &wgpu::TextureView, near_view_range: f32) {
        let [depth_a, depth_b] = nirender_wgpu::depth::terms(&scene.projection);
        let values = [
            depth_a,
            depth_b,
            near_view_range - ZONE,
            ZONE,
            scene.projection[0],
            scene.projection[5],
            scene.target_size.0 as f32,
            scene.target_size.1 as f32,
        ];
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&values));
        let group = nirender_wgpu::gpu::frame_group(scene.device, &wgpu::BindGroupDescriptor {
            label: Some("blend"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(before) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
            ],
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        self.frames += 1;
    }
}

#[cfg(test)]
mod tests {
    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        use nirender_wgpu::wgpu::naga;
        let source = super::shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
    }
}
