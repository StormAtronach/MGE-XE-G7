//! The caustics of the sun under the water: `XE Mod Caustics.fx`.
//!
//! A pass over the picture of the game at the end of its world scene, before the blend with
//! the distant land (`renderStageBlend` of the Direct3D 9 path). For a pixel of the game
//! under the level of the water, the light that the waves send down modulates its colour:
//! brighter where the waves focus it, and darker in the mean, less with the depth of the
//! water and with the distance.

use nirender_wgpu::{SceneView, wgpu};

const SHADER: &str = "
struct Uniforms {
    view: mat4x4f,
    // The two numbers of the projection for the width and the height, and the two that
    // give the depth.
    projection: vec4f,
    size: vec4f,
    // The camera, and the level of the water.
    eye: vec4f,
    // The direction that the light of the sun goes in, and the time of the waves.
    sun: vec4f,
    // The strength, the view range of the game, and the start and the end of its fog.
    values: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var waves: texture_3d<f32>;
@group(0) @binding(4) var wave_sampler: sampler;

@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    // One triangle that covers the picture.
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}

@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
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
    // Where the ray of the sun through the point meets the surface: the waves there.
    let sunray = under - u.sun.xyz * (under.z / u.sun.z);
    var caust = u.values.x * textureSample(waves, wave_sampler, vec3f(sunray.xy / 1104.0, 0.4 * u.sun.w)).b;
    caust *= clamp(125.0 / z * min(fwidth(sunray.x), fwidth(sunray.y)), 0.0, 1.0);
    let factor = 1.0 + (caust - 0.3) * clamp(exp(under.z / 400.0), 0.0, 1.0) * clamp(under.z / -30.0, 0.0, 1.0) * fog;
    return vec4f(textureLoad(picture, at, 0).rgb * factor, 1.0);
}
";

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
    pub fn new(scene: &SceneView, intensity: f32) -> Self {
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
                texture(1, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(3, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3),
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("caustics"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("caustics"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::COLOR })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("caustics"), size: 64 + 5 * 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let wave_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, uniforms, wave_sampler, intensity, frames: 0 }
    }

    /// Draws the caustics on the picture. `picture` is a copy of the picture as it is,
    /// `depth` the depth of the draws of the game, `waves` the wave texture of the water.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, picture: &wgpu::TextureView, depth: &wgpu::TextureView, waves: &wgpu::TextureView, frame: &CausticsFrame) {
        let eye = super::eye_of(&scene.view);
        let sunlight = 1.0 - (1.0 - frame.sun_vis).powi(2);
        let fog_far = frame.fog_color_far;
        let strength = 0.05 * self.intensity * (0.75 * sunlight + 0.35 * (fog_far[0] * fog_far[0] + fog_far[1] * fog_far[1] + fog_far[2] * fog_far[2]).sqrt()).clamp(0.0, 1.0);
        let mut values = [0.0f32; 16 + 20];
        values[..16].copy_from_slice(&scene.view);
        values[16..20].copy_from_slice(&[scene.projection[0], scene.projection[5], scene.projection[10], scene.projection[14]]);
        values[20..24].copy_from_slice(&[scene.target_size.0 as f32, scene.target_size.1 as f32, 0.0, 0.0]);
        values[24..28].copy_from_slice(&[eye[0], eye[1], eye[2], frame.water_level]);
        values[28..32].copy_from_slice(&[frame.sun_direction[0], frame.sun_direction[1], frame.sun_direction[2], frame.time]);
        values[32..36].copy_from_slice(&[strength, frame.near_view_range, frame.fog_near.0, frame.fog_near.1]);
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&values));
        let group = scene.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(waves) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.wave_sampler) },
            ],
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        self.frames += 1;
    }
}
