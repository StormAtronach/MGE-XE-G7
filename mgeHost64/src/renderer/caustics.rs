//! The caustics of the sun under the sea: `XE Mod Caustics.fx`.
//!
//! A pass over the picture of the game at the end of its world scene, before the surfaces of
//! water volumes. For a pixel of the game under the level of the water, the light that the
//! waves send down modulates its colour (`water::CAUSTICS`, the function that the surfaces of
//! water volumes have too): brighter where the waves focus it, and darker in the mean, less
//! with the depth of the water and with the distance. Not in a dry space: the pass is drawn
//! with the count of the dry spaces in the stencil buffer (`water::Water::draw_sea_caustics`).
//!
//! The pass reads no copy of the picture: a blend multiplies the target by twice the colour
//! of the shader (`2 * colour * target`). The colour of an 8-bit target is at most 1, so one
//! draw multiplies by at most 2: a larger factor takes more draws (`draws`), each with the
//! part of the factor between 2^n and 2^(n+1).

use nirender_wgpu::{SceneView, wgpu};

const SHADER: &str = "
struct Uniforms {
    view: mat4x4f,
    // The two numbers of the projection for the width and the height, and the two that
    // give the depth.
    projection: vec4f,
    // The size of the picture, and the length of the blend into the distant land.
    size: vec4f,
    // The camera, and the level of the water.
    eye: vec4f,
    // The direction that the light of the sun goes in, and the time of the waves.
    sun: vec4f,
    // The strength, the view range of the game, and the start and the end of its fog.
    values: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var waves: texture_3d<f32>;
@group(0) @binding(3) var wave_sampler: sampler;

struct Corner {
    @builtin(position) position: vec4f,
    // The draw of the chain (the instance): see `draws`.
    @location(0) @interpolate(flat) step: u32,
}

@vertex fn vs(@builtin(vertex_index) index: u32, @builtin(instance_index) step: u32) -> Corner {
    // One triangle that covers the picture.
    return Corner(vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0), step);
}

@fragment fn fs(in: Corner) -> @location(0) vec4f {
    let position = in.position;
    let at = vec2i(position.xy);
    let d = textureLoad(depth, at, 0);
    // The depth of the pixel along the view of the camera. Nothing drawn, or past the view
    // range of the game: no caustics.
    if d >= 1.0 {
        discard;
    }
    let z = u.projection.w / (d - u.projection.z);
    if z > u.values.y {
        discard;
    }
    // The point of the pixel in the world: the ray of the pixel for a unit of view depth.
    let ndc = position.xy / u.size.xy * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    let ray = vec3f(u.view[0].z, u.view[1].z, u.view[2].z)
        + (ndc.x / u.projection.x) * vec3f(u.view[0].x, u.view[1].x, u.view[2].x)
        + (ndc.y / u.projection.y) * vec3f(u.view[0].y, u.view[1].y, u.view[2].y);
    var under = u.eye.xyz + ray * z;
    under.z -= u.eye.w;
    if under.z > 0.0 {
        discard;
    }
    let fog = clamp((u.values.w - z) / (u.values.w - u.values.z), 0.0, 1.0);
    // The blend takes the picture of the game into the distant land over the last stretch
    // before the view range (`blend.rs`, drawn before this pass): the caustics go out with it.
    let bound = u.values.y - u.size.z;
    let kept = 1.0 - clamp((blend_distance(depth, at, u.projection.zw, u.projection.xy, u.size.xy, bound) - bound) / u.size.z, 0.0, 1.0);
    let factor = 1.0 + caustic_light(under, u.sun.xyz, u.sun.w, z, u.values.x) * fog * kept;
    // The draw `step` multiplies by the part of the factor between 2^step and 2^(step+1); the
    // first one by all of it to 2, a darker pixel too.
    let low = exp2(f32(in.step));
    if in.step > 0u && factor <= low {
        discard;
    }
    let part = select(min(factor, 2.0), clamp(factor / low, 1.0, 2.0), in.step > 0u);
    return vec4f(vec3f(0.5 * part), 1.0);
}
";

/// The draws of the chain for a strength: the factor of a pixel is at most 1 + `strength`
/// (`water::CAUSTICS`), and each draw multiplies by at most 2.
fn draws(strength: f32) -> u32 {
    (1.0 + strength.max(0.0)).log2().ceil().max(1.0) as u32
}

fn shader() -> String {
    format!("{}{}{}", SHADER, super::water::CAUSTICS, super::blend::DISTANCE)
}

/// What the caustics need from the frame.
pub struct CausticsFrame {
    pub water_level: f32,
    /// The time of the waves, as the water has it.
    pub time: f32,
    /// The direction that the light of the sun goes in.
    pub sun_direction: [f32; 3],
    pub sun_vis: f32,
    pub fog_color_far: [f32; 3],
    /// The view range of the game, and the start and the end of its fog.
    pub near_view_range: f32,
    pub fog_near: (f32, f32),
}

pub struct Caustics {
    pipeline: wgpu::RenderPipeline,
    /// The same, outside the dry spaces.
    pipeline_cut: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    wave_sampler: wgpu::Sampler,
    /// The strength setting, `distant_land.water.caustics_intensity` (0 to 100). A mod can
    /// change it while the game runs: the renderer sets it before each draw.
    pub intensity: f32,
    /// The frames with caustics, for the report.
    pub frames: u64,
}

impl Caustics {
    /// `cut` is the stencil test of a pixel outside the dry spaces (`water::Water::dry_cut`).
    pub fn new(scene: &SceneView, intensity: f32, cut: wgpu::StencilState) -> Self {
        let device = scene.device;
        let texture = |binding, sample_type, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: dimension, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("caustics"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("caustics"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("caustics"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        // In the pass of the world: no test of the depth and no write, and the stencil of the
        // dry spaces for the cut. The blend multiplies the target: colour * target + target *
        // colour.
        let modulate = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Dst, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let make_pipeline = |stencil: wgpu::StencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("caustics"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: scene.depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil,
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: Some(modulate), write_mask: wgpu::ColorWrites::COLOR })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(Default::default());
        let pipeline_cut = make_pipeline(cut);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("caustics"), size: 64 + 5 * 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let wave_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, pipeline_cut, layout, uniforms, wave_sampler, intensity, frames: 0 }
    }

    /// Draws the caustics on the picture. `depth` is the depth of the draws of the game,
    /// `waves` the wave texture of the water. With `cut`, only outside the dry spaces: the
    /// caller has their count in the stencil.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, depth: &wgpu::TextureView, waves: &wgpu::TextureView, frame: &CausticsFrame, cut: bool) {
        let eye = super::eye_of(&scene.view);
        let strength = super::water::caustic_strength(self.intensity, frame.sun_vis, frame.fog_color_far);
        let mut values = [0.0f32; 16 + 20];
        values[..16].copy_from_slice(&scene.view);
        values[16..20].copy_from_slice(&[scene.projection[0], scene.projection[5], scene.projection[10], scene.projection[14]]);
        values[20..24].copy_from_slice(&[scene.target_size.0 as f32, scene.target_size.1 as f32, super::blend::ZONE, 0.0]);
        values[24..28].copy_from_slice(&[eye[0], eye[1], eye[2], frame.water_level]);
        values[28..32].copy_from_slice(&[frame.sun_direction[0], frame.sun_direction[1], frame.sun_direction[2], frame.time]);
        values[32..36].copy_from_slice(&[strength, frame.near_view_range, frame.fog_near.0, frame.fog_near.1]);
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&values));
        let group = nirender_wgpu::gpu::frame_group(scene.device, &wgpu::BindGroupDescriptor {
            label: Some("caustics"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(waves) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.wave_sampler) },
            ],
        });
        pass.set_pipeline(if cut { &self.pipeline_cut } else { &self.pipeline });
        pass.set_stencil_reference(0);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..draws(strength));
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

    /// The chain has a draw for each doubling of the most factor, 1 + the strength: one to a
    /// strength of 1, two to 3 (the setting 50 gives at most 2.5), three to 7 (100 gives 5).
    #[test]
    fn the_draws_cover_the_factor() {
        assert_eq!(super::draws(0.0), 1);
        assert_eq!(super::draws(1.0), 1);
        assert_eq!(super::draws(1.01), 2);
        assert_eq!(super::draws(2.5), 2);
        assert_eq!(super::draws(3.0), 2);
        assert_eq!(super::draws(5.0), 3);
        assert_eq!(super::draws(-1.0), 1);
    }
}
