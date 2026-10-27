//! The shadow map of MGE XE: an atlas of two layers (`R16Float`, 2 x res by res) with the
//! depth of the distant terrain and the distant statics from the sun, as `renderShadowMap`
//! of the Direct3D 9 path makes it (`XE Shadowmap.fx`). The port specification is in
//! `moreFPS/docs/plans/mge-xe-shadow-port-spec.md`. This module has the resources, the
//! matrices, the clear and the blur, and a debug view; the casters are drawn by the terrain
//! and the statics modules into the passes that this module starts.

use std::rc::Rc;

use nirender_wgpu::{SceneView, wgpu};

/// `MGE_XE_SHADOW_DEBUG` and `MGE_XE_SHADOW_LAYERS`, read once: the shadows are made again
/// at a change of the resolution.
static SHADOW_DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static SHADOW_LAYERS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

use super::multiply;

/// The constants of `XE Mod Shadow Data.fx`.
pub const ESM_C: f32 = 60.0;
pub const ESM_BIAS: f32 = 2e-3 * ESM_C;
pub const ESM_SCALE: f32 = 32768.0;
pub const SHADE: f32 = 0.4;
pub const SHADE_COLOR: [f32; 3] = [1.0, 0.97, 0.81];
/// The radii of the two layers, and the depth range of a layer in units.
pub const RADII: [f32; 2] = [1000.0, 4000.0];
const ZRANGE: f32 = 8192.0;

const BLUR_SHADER: &str = "
struct Uniforms {
    // The offset of a tap in uv, and the axis.
    step: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    var out: Out;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}

// shadowSoften: five taps along one axis, a mean with weights 0.2, 0.8, 1, 0.8, 0.2.
@fragment fn fs(in: Out) -> @location(0) vec4f {
    let off = u.step.xy;
    var d = textureSample(source, source_sampler, in.uv).r;
    d += 0.2 * textureSample(source, source_sampler, in.uv - 1.42 * off).r;
    d += 0.8 * textureSample(source, source_sampler, in.uv - 0.71 * off).r;
    d += 0.8 * textureSample(source, source_sampler, in.uv + 0.71 * off).r;
    d += 0.2 * textureSample(source, source_sampler, in.uv + 1.42 * off).r;
    return vec4f(d / 3.0, 0.0, 0.0, 1.0);
}
";

const DEBUG_SHADER: &str = "
@group(0) @binding(0) var<uniform> u: vec4f;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

// The lower left part of the picture: the atlas at a quarter of the width.
@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    var out: Out;
    let corner = vec2f(f32(index & 1u), f32(index >> 1u));
    out.position = vec4f(-1.0 + corner.x * 0.5, -1.0 + corner.y * 0.25, 0.0, 1.0);
    out.uv = vec2f(corner.x, 1.0 - corner.y);
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let d = textureSample(source, source_sampler, in.uv).r / u.x;
    return vec4f(d, d, d, 1.0);
}
";

pub struct Shadow {
    pub resolution: u32,
    pub atlas_view: Rc<wgpu::TextureView>,
    blur_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    blur_pipeline: wgpu::RenderPipeline,
    blur_groups: [wgpu::BindGroup; 2],
    /// The uniforms of the blur groups, kept alive with them.
    _blur_uniforms: [wgpu::Buffer; 2],
    debug_pipeline: wgpu::RenderPipeline,
    debug_group: wgpu::BindGroup,
    pub debug: bool,
    /// `MGE_XE_SHADOW_LAYERS=1`: the draw shader colours each pixel by the layer of the
    /// shadow map that serves it (red the first, green the second, blue outside both, grey
    /// a draw without the shadow test), darker where the map shadows the pixel.
    pub layers_debug: bool,
    /// The matrices of the two layers for this frame, with the layout of Direct3D.
    pub viewproj: [[f32; 16]; 2],
    pub frames: u64,
}

/// `D3DXMatrixLookAtRH`, with the layout of Direct3D (a position is a row vector).
fn look_at_rh(eye: [f32; 3], at: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normalize = |a: [f32; 3]| {
        let l = dot(a, a).sqrt().max(1e-12);
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let z = normalize(sub(eye, at));
    let x = normalize(cross(up, z));
    let y = cross(z, x);
    [x[0], y[0], z[0], 0.0, x[1], y[1], z[1], 0.0, x[2], y[2], z[2], 0.0, -dot(x, eye), -dot(y, eye), -dot(z, eye), 1.0]
}

/// The inverse of a matrix with the layout of Direct3D (the identity when there is none).
pub fn inverse(m: &[f32; 16]) -> [f32; 16] {
    let a = |r: usize, c: usize| m[r * 4 + c] as f64;
    let s0 = a(0, 0) * a(1, 1) - a(1, 0) * a(0, 1);
    let s1 = a(0, 0) * a(1, 2) - a(1, 0) * a(0, 2);
    let s2 = a(0, 0) * a(1, 3) - a(1, 0) * a(0, 3);
    let s3 = a(0, 1) * a(1, 2) - a(1, 1) * a(0, 2);
    let s4 = a(0, 1) * a(1, 3) - a(1, 1) * a(0, 3);
    let s5 = a(0, 2) * a(1, 3) - a(1, 2) * a(0, 3);
    let c5 = a(2, 2) * a(3, 3) - a(3, 2) * a(2, 3);
    let c4 = a(2, 1) * a(3, 3) - a(3, 1) * a(2, 3);
    let c3 = a(2, 1) * a(3, 2) - a(3, 1) * a(2, 2);
    let c2 = a(2, 0) * a(3, 3) - a(3, 0) * a(2, 3);
    let c1 = a(2, 0) * a(3, 2) - a(3, 0) * a(2, 2);
    let c0 = a(2, 0) * a(3, 1) - a(3, 0) * a(2, 1);
    let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;
    if det.abs() < 1e-30 {
        let mut identity = [0.0; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        return identity;
    }
    let d = 1.0 / det;
    let out = [
        (a(1, 1) * c5 - a(1, 2) * c4 + a(1, 3) * c3) * d,
        (-a(0, 1) * c5 + a(0, 2) * c4 - a(0, 3) * c3) * d,
        (a(3, 1) * s5 - a(3, 2) * s4 + a(3, 3) * s3) * d,
        (-a(2, 1) * s5 + a(2, 2) * s4 - a(2, 3) * s3) * d,
        (-a(1, 0) * c5 + a(1, 2) * c2 - a(1, 3) * c1) * d,
        (a(0, 0) * c5 - a(0, 2) * c2 + a(0, 3) * c1) * d,
        (-a(3, 0) * s5 + a(3, 2) * s2 - a(3, 3) * s1) * d,
        (a(2, 0) * s5 - a(2, 2) * s2 + a(2, 3) * s1) * d,
        (a(1, 0) * c4 - a(1, 1) * c2 + a(1, 3) * c0) * d,
        (-a(0, 0) * c4 + a(0, 1) * c2 - a(0, 3) * c0) * d,
        (a(3, 0) * s4 - a(3, 1) * s2 + a(3, 3) * s0) * d,
        (-a(2, 0) * s4 + a(2, 1) * s2 - a(2, 3) * s0) * d,
        (-a(1, 0) * c3 + a(1, 1) * c1 - a(1, 2) * c0) * d,
        (a(0, 0) * c3 - a(0, 1) * c1 + a(0, 2) * c0) * d,
        (-a(3, 0) * s3 + a(3, 1) * s1 - a(3, 2) * s0) * d,
        (a(2, 0) * s3 - a(2, 1) * s1 + a(2, 2) * s0) * d,
    ];
    out.map(|v| v as f32)
}

/// `D3DXMatrixOrthoRH`.
fn ortho_rh(width: f32, height: f32, near: f32, far: f32) -> [f32; 16] {
    let mut m = [0.0f32; 16];
    m[0] = 2.0 / width;
    m[5] = 2.0 / height;
    m[10] = 1.0 / (near - far);
    m[14] = near / (near - far);
    m[15] = 1.0;
    m
}

/// The matrix of a layer, as `renderShadowLayer` of the Direct3D 9 path makes it: an
/// orthographic view from the sun, centred `radius` ahead of the camera (half of that up),
/// snapped to whole texels.
pub fn layer_matrix(eye: [f32; 3], eye_vec: [f32; 3], light_vec: [f32; 3], radius: f32, resolution: u32) -> [f32; 16] {
    let look_at = [eye[0] + radius * eye_vec[0], eye[1] + radius * eye_vec[1], eye[2] + 0.5 * radius * eye_vec[2]];
    let look_at_eye = eye.map(|v| 16.0 * (0.0625 * v).floor());
    let camera = [look_at_eye[0] - ZRANGE * light_vec[0], look_at_eye[1] - ZRANGE * light_vec[1], look_at_eye[2] - ZRANGE * light_vec[2]];
    let view = look_at_rh(camera, look_at_eye, [0.0, 0.0, 1.0]);
    let projection = ortho_rh(2.0 * radius, (1.0 + light_vec[2].abs()) * radius, 0.0, 2.0 * ZRANGE);
    let mut viewproj = multiply(&view, &projection);
    // The point `look_at` goes to the middle of the layer, in whole texels.
    let delta = [look_at_eye[0] - look_at[0], look_at_eye[1] - look_at[1], look_at_eye[2] - look_at[2]];
    let dv: [f32; 3] = core::array::from_fn(|i| delta[0] * viewproj[i] + delta[1] * viewproj[4 + i] + delta[2] * viewproj[8 + i]);
    let quantizer = 2.0 / resolution as f32;
    viewproj[12] += quantizer * (dv[0] / quantizer).floor();
    viewproj[13] += quantizer * (dv[1] / quantizer).floor();
    viewproj[14] += dv[2];
    viewproj
}

impl Shadow {
    pub fn new(scene: &SceneView, resolution: u32) -> Shadow {
        let device = scene.device;
        let resolution = resolution.clamp(256, 4096);
        let target = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: 2 * resolution, height: resolution, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let atlas = target("shadow atlas", wgpu::TextureFormat::R16Float, usage);
        let blur = target("shadow blur", wgpu::TextureFormat::R16Float, usage);
        let depth_view = target("shadow depth", scene.depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let atlas_view = Rc::new(atlas.create_view(&Default::default()));
        let blur_view = blur.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let make_pipeline = |label: &str, source: &str, format: wgpu::TextureFormat, depth: Option<wgpu::DepthStencilState>| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, cull_mode: None, ..Default::default() },
                depth_stencil: depth,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let blur_pipeline = make_pipeline("shadow blur", BLUR_SHADER, wgpu::TextureFormat::R16Float, None);
        let debug_pipeline = make_pipeline(
            "shadow debug",
            DEBUG_SHADER,
            scene.target_format,
            Some(wgpu::DepthStencilState { format: scene.depth_format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
        );
        let uniform = |values: [f32; 4]| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("shadow"), contents: bytemuck::cast_slice(&values), usage: wgpu::BufferUsages::UNIFORM })
        };
        // The blur steps: shadowRcpRes is 1 / res on both axes of an atlas 2 res wide, as
        // the Direct3D 9 path has it.
        let rcp = 1.0 / resolution as f32;
        let blur_uniforms = [uniform([rcp, 0.0, 0.0, 0.0]), uniform([0.0, rcp, 0.0, 0.0])];
        let group = |uniform: &wgpu::Buffer, view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            })
        };
        // Horizontal: the atlas into the blur texture. Vertical: back into the atlas.
        let blur_groups = [group(&blur_uniforms[0], &atlas_view), group(&blur_uniforms[1], &blur_view)];
        let debug_uniform = uniform([ESM_SCALE, 0.0, 0.0, 0.0]);
        let debug_group = group(&debug_uniform, &atlas_view);
        Shadow {
            resolution,
            atlas_view,
            blur_view,
            depth_view,
            blur_pipeline,
            blur_groups,
            _blur_uniforms: blur_uniforms,
            debug_pipeline,
            debug_group,
            debug: *SHADOW_DEBUG.get_or_init(|| nirender_wgpu::switches::on("MGE_XE_SHADOW_DEBUG") == Some(true)),
            layers_debug: *SHADOW_LAYERS.get_or_init(|| nirender_wgpu::switches::on("MGE_XE_SHADOW_LAYERS") == Some(true)),
            viewproj: [[0.0; 16]; 2],
            frames: 0,
        }
    }

    /// The matrices of the two layers for a frame.
    pub fn set_frame(&mut self, eye: [f32; 3], eye_vec: [f32; 3], light_vec: [f32; 3]) {
        for (layer, radius) in RADII.iter().enumerate() {
            self.viewproj[layer] = layer_matrix(eye, eye_vec, light_vec, *radius, self.resolution);
        }
    }

    /// Starts the pass of the casters: the atlas cleared to [`ESM_SCALE`], the depth to 1. The
    /// caller sets the layer with `set_layer`, draws the casters, and then ends the pass.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) -> wgpu::RenderPass<'static> {
        self.frames += 1;
        nirender_wgpu::gpu::note_other_pass();
        encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.atlas_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: ESM_SCALE as f64, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime()
    }

    /// The viewport of a layer of the atlas.
    pub fn set_layer(&self, pass: &mut wgpu::RenderPass<'static>, layer: usize) {
        let (x, res) = ((layer as u32) * self.resolution, self.resolution);
        pass.set_viewport(x as f32, 0.0, res as f32, res as f32, 0.0, 1.0);
        pass.set_scissor_rect(x, 0, res, res);
    }

    /// The two blurs, after the casters.
    pub fn blur(&self, encoder: &mut wgpu::CommandEncoder) {
        for (step, target) in [(&self.blur_view, 0usize), (&*self.atlas_view, 1usize)].into_iter().map(|(view, i)| (i, view)) {
            nirender_wgpu::gpu::note_other_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &self.blur_groups[step], &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The atlas in the lower left of the picture, for a test.
    pub fn draw_debug(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_pipeline(&self.debug_pipeline);
        pass.set_bind_group(0, &self.debug_group, &[]);
        pass.draw(0..4, 0..1);
    }

    /// The values of the frame for the shader functions of the draws of the game: the two
    /// matrices from the view space of the game to the layers (8 vectors, the rows of
    /// Direct3D), then the constants, the visibility of the sun, and 1 when the camera is
    /// above the sea.
    pub fn frame_values(&self, sun_vis: f32, view: &[f32; 16], above_sea: bool) -> [[f32; 4]; 32] {
        let mut values = [[0.0f32; 4]; 32];
        let from_view = inverse(view);
        for layer in 0..2 {
            let to_shadow = multiply(&from_view, &self.viewproj[layer]);
            for row in 0..4 {
                values[layer * 4 + row] = [to_shadow[row * 4], to_shadow[row * 4 + 1], to_shadow[row * 4 + 2], to_shadow[row * 4 + 3]];
            }
        }
        values[8] = [ESM_C, ESM_BIAS, ESM_SCALE, 1.0 / self.resolution as f32];
        values[9] = [SHADE_COLOR[0], SHADE_COLOR[1], SHADE_COLOR[2], SHADE];
        values[10] = [sun_vis, if above_sea { 1.0 } else { 0.0 }, if self.layers_debug { 1.0 } else { 0.0 }, 0.0];
        values
    }
}
