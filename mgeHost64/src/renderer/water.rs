//! The water of MGE XE: the plane in place of the water plane of the game, with a reflection
//! of the sky and the distant land, the refraction of the picture so far, and the depth of
//! the game for the shore. `XE Mod Water.fx`; the port specification is in
//! `moreFPS/docs/plans/mge-xe-water-port-spec.md`. With the dynamic waves
//! (`DYNAMIC_RIPPLES` of the effect file, a setting that a mod can change here while the
//! game runs): a finer plane that the waves move, and the ripples of `ripples.rs` in its
//! normal. The shading that each surface of water shares is in `water/shading.rs`; this file
//! has the frame's resources (the reflection, the waves) and the plane.

use nirender_wgpu::{Replay, ReplayView, SceneView, wgpu};

use super::{multiply, planes};

/// The size of the reflection, as the Direct3D 9 path has it.
const REFLECTION_SIZE: u32 = 1024;
/// The levels of the reflection, for the blur that is sharp at the waterline: 1024 to 32.
const REFLECTION_LEVELS: u32 = 6;

/// Makes a level of the reflection from the level before: the mean of 2 x 2 pixels.
const LEVEL_SHADER: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}

@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
    // The target has half of the size of the source.
    let size = vec2f(textureDimensions(source)) * 0.5;
    return textureSampleLevel(source, source_sampler, position.xy / size, 0.0);
}
";
/// The near plane of the distant land, as the Direct3D 9 path has it.
const NEAR: f32 = 4.0;
/// How near the level of the water the eye in a dry space has the plane edge-on
/// (`kDrySpaceLevelMargin`).
const DRY_LEVEL_MARGIN: f32 = 40.0;

mod shading;
use shading::Uniforms;
mod volume;
mod dry;
pub use dry::read_masks;
pub use volume::{FarWater, Look, VolumeLook, marked as volume_marked};

/// The shader of the plane: the shared shading and the entry points of the plane.
fn shader() -> String {
    format!("{}{}", shading::COMMON, PLANE)
}

/// The entry points of the plane, after the shared shading.
const PLANE: &str = "
struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) texcoords: vec4f,
    @location(2) clip_w: f32,
    // With the dynamic waves: the clip x, y and w of the point at or under the plane, for
    // the place of the reflection (screenposclamp).
    @location(3) clamped: vec3f,
    // For a test (MGE_XE_WATER_DEBUG 9): the height that the vertex read in the wave texture.
    @location(4) probe: f32,
}

@vertex fn vs(@location(0) local: vec3f) -> Out {
    var out: Out;
    var world = local + u.world.xyz;
    var lowered = 0.0;
    if u.wave.w > 0.5 {
        // WaterVS with DYNAMIC_RIPPLES: the waves move the plane, most between 200 and
        // 6400 units from the camera.
        let t = 0.4 * u.eye.w;
        let height = textureSampleLevel(waves, wave_sampler, vec3f(world.xy / 1104.0, t), 0.0).a;
        let height2 = textureSampleLevel(waves, wave_sampler, vec3f(world.xy / 3900.0, t), 0.0).a;
        let dist = length(u.eye.xyz - world);
        let add = u.wave.z * (mix(height, height2, clamp(dist / 8000.0, 0.0, 1.0)) - 0.5) * clamp(1.0 - dist / 6400.0, 0.0, 1.0) * clamp(dist / 200.0, 0.0, 1.0);
        world.z += add;
        lowered = abs(add);
        out.probe = height;
    } else {
        // The plane lies at the water level less 1, a little under the ripples of the game.
        world.z -= 0.1;
    }
    out.world = world;
    out.texcoords = vec4f(world.xy / 3900.0, world.xy / 527.0);
    out.position = u.view_projection * vec4f(world, 1.0);
    // The far plane of the game is its view distance; the sea goes on to the draw distance of
    // the distant land, as on Direct3D 9, where MGE XE moves the far plane of the game there.
    // The near plane stays that of the game. The depth that the pixel writes is that of the
    // game (see `fs`).
    out.position.z = out.position.w * u.land.z + u.land.w;
    out.clip_w = out.position.w;
    // The point of the reflection is not above the plane.
    out.clamped = (u.view_projection * vec4f(world.xy, world.z - lowered, 1.0)).xyw;
    return out;
}

struct Pixel {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
}

@fragment fn fs(in: Out) -> Pixel {
    // The distant land in front of the water hides it. The renderer cleared its depth for the
    // game, so the test is here; the test against the draws of the game is that of the pass.
    if in.clip_w > land_distance_at(in.position.xy / u.size.xy) * 1.0005 + 1.0 {
        discard;
    }
    var out: Pixel;
    out.color = plane_color(in);
    // The depth in the range of the game: what blends after the water tests it. Past the far
    // plane of the game it is the far end.
    out.depth = clamp(u.projection.x + u.projection.y / in.clip_w, 0.0, 1.0);
    return out;
}

fn plane_color(in: Out) -> vec4f {
    // The end of the plane in an interior and under the water.
    if u.size.z > 0.0 && in.clip_w > u.size.z {
        discard;
    }
    var eye_vec = in.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    // The position of the pixel in the picture. The offsets of the shader are in the
    // projected space: here they are divided by the clip w of the pixel.
    let uv = in.position.xy / u.size.xy;
    let w = in.clip_w;
    if u.fog.w > 0.5 {
        return underwater(uv, w, in.texcoords, in.world.xy, eye_vec, dist);
    }
    let fog = fog_water(eye_vec, dist);
    let normal = water_normal(in.texcoords, dist, in.world.xy);
    let reffactor = (u.sun_col.w * dist + 0.1) * normal.xy;
    let below = refraction(uv, w, reffactor, eye_vec, normal, deep_color(fog, vec3f(0.0)), vec3f(0.0), 800.0, vec3f(1.0), true);
    let refracted = below.color;
    // With the dynamic waves the reflection is read at the point that is not above the plane.
    var reflect_uv = uv;
    var reflect_w = w;
    if u.wave.w > 0.5 {
        reflect_w = in.clamped.z;
        reflect_uv = vec2f(0.5 + 0.5 * in.clamped.x / reflect_w, 0.5 - 0.5 * in.clamped.y / reflect_w);
    }
    // A flat look at the water makes the blur taller: 1 from straight above, and at most
    // the number of the parameter.
    let stretch = clamp(1.0 / max(abs(eye_vec.z), 0.05), 1.0, max(u.projection.z, 1.0));
    var reflected = reflection_at(reflect_uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / reflect_w, reflect_w, stretch);
    reflected = mix(fog.rgb, reflected, fog.a);
    var adjust = mix(vec3f(0.0, 0.0, 0.1), normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, vec3f(0.0, 0.0, 1.0), (1.0 + eye_vec.z) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0)));
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(refracted, reflected, fresnel);
    result += sun_glint(eye_vec, normal, fog.a);
    result = mix(result, refracted, below.shore * fog.a);
    // A look at one term, for a test: MGE_XE_WATER_DEBUG.
    if u.fog.z == 1.0 {
        return vec4f(reflected, 1.0);
    } else if u.fog.z == 2.0 {
        return vec4f(refracted, 1.0);
    } else if u.fog.z == 3.0 {
        return vec4f(vec3f(below.depth_first / 1000.0), 1.0);
    } else if u.fog.z == 4.0 {
        return vec4f(vec3f(fresnel), 1.0);
    } else if u.fog.z == 5.0 {
        return vec4f(vec3f(fog.a), 1.0);
    } else if u.fog.z == 6.0 {
        return vec4f(vec3f(dist / 10000.0), 1.0);
    } else if u.fog.z == 7.0 {
        return vec4f(normal * 0.5 + 0.5, 1.0);
    } else if u.fog.z == 8.0 {
        return vec4f(vec3f(w / 10000.0), 1.0);
    } else if u.fog.z == 9.0 {
        // Red: the height that the vertices read. Green: the same read for the pixel.
        let here = textureSampleLevel(waves, wave_sampler, vec3f(in.world.xy / 1104.0, 0.4 * u.eye.w), 0.0).a;
        return vec4f(in.probe, here, 0.0, 1.0);
    }
    return vec4f(result, 1.0);
}
";

/// What the water needs from the environment and the frame.
pub struct WaterFrame {
    pub water_level: f32,
    pub underwater: bool,
    pub time: f32,
    /// The direction to the sun (z negated when the sun is not visible), its visibility.
    pub sun_pos: [f32; 3],
    pub sun_vis: f32,
    pub sun_color: [f32; 3],
    pub sky_color: [f32; 3],
    pub fog_color_far: [f32; 3],
    pub nice_weather: f32,
    pub scattering: Option<[[f32; 4]; 5]>,
    pub wind: [f32; 2],
    /// The start and the divisor of the exponential fog.
    pub fog_exp: (f32, f32),
    /// The draw distance of the distant land, in units.
    pub draw_distance: f32,
    /// The projection of the distant land (`DistantLand::land_projection`): the water
    /// among the distant statics is drawn with it, and its depth is that of the land.
    pub land_projection: [f32; 16],
    /// The view depth where the plane ends, in an interior and under the water (the clip
    /// plane of `renderStageWater`); `None` for no end.
    pub view_end: Option<f32>,
    /// The camera is in a dry space (`MGE_WaterDrySet`).
    pub camera_dry: bool,
    /// The wave height of the settings (0 to 250), with the dynamic waves; `None` without.
    pub wave_height: Option<f32>,
    /// The blur of the reflection (`distant_land.water.blur_reflections`).
    pub blur_reflections: bool,
    /// With the blur: the one that is sharp at the waterline
    /// (`distant_land.water.blur_contact_hardening`), not the six reads of MGE XE.
    pub blur_contact: bool,
    /// The numbers of that blur (tunables.rs): the angle of the cone of the mirror without
    /// wind in radians, what the wind adds, and the most height of the blur for its width.
    pub blur_tune: [f32; 3],
}

/// A disc of triangles around the camera: a centre, then rings of `segments` vertices at
/// the radius that `radius` gives for each ring. The local z is -1 (the plane lies at the
/// water level less 1).
fn disc(segments: usize, rings: usize, radius: impl Fn(usize) -> f32) -> (Vec<[f32; 3]>, Vec<u16>) {
    let mut vertices: Vec<[f32; 3]> = vec![[0.0, 0.0, -1.0]];
    for t in 0..rings {
        let r = radius(t);
        for s in 0..segments {
            let angle = 2.0 * std::f32::consts::PI * s as f32 / segments as f32;
            vertices.push([r * angle.cos(), r * angle.sin(), -1.0]);
        }
    }
    let mut indexes: Vec<u16> = Vec::new();
    for s in 0..segments {
        indexes.extend_from_slice(&[0, (1 + s) as u16, (1 + (s + 1) % segments) as u16]);
    }
    for t in 1..rings {
        let base = 1 + segments * (t - 1);
        for s in 0..segments {
            let s2 = (s + 1) % segments;
            indexes.extend_from_slice(&[(base + s) as u16, (segments + base + s) as u16, (base + s2) as u16]);
            indexes.extend_from_slice(&[(segments + base + s) as u16, (segments + base + s2) as u16, (base + s2) as u16]);
        }
    }
    (vertices, indexes)
}

/// The disc without the dynamic waves: 15 rings of 16 vertices, the rings further apart.
fn coarse_disc() -> (Vec<[f32; 3]>, Vec<u16>) {
    disc(16, 15, |t| 4096.0 * (1.0 + (t * t) as f32))
}

/// The disc with the dynamic waves: 120 rings of 150 vertices, dense near the camera (the
/// waves move its vertices), and the last ring past the horizon.
fn fine_disc() -> (Vec<[f32; 3]>, Vec<u16>) {
    const RINGS: usize = 120;
    disc(150, RINGS, |t| {
        if t + 1 == RINGS {
            return 500_000.0;
        }
        let r = t as f32 / RINGS as f32;
        9600.0 * (0.9 * r.powi(3) + 0.1 * r)
    })
}

pub struct Water {
    pipeline: wgpu::RenderPipeline,
    /// The same as lines, when the device has the line mode: for a frame in which the game
    /// draws in wireframe (`SceneView::wireframe`).
    pipeline_lines: Option<wgpu::RenderPipeline>,
    /// The plane outside the dry spaces.
    pipeline_cut: wgpu::RenderPipeline,
    /// The dry spaces, and the count of their faces.
    dry: dry::Dry,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The values of the frame for the surfaces of water volumes: they are drawn before the
    /// plane in the same frame, and a write of the buffer is done before the frame runs.
    volume_uniforms: wgpu::Buffer,
    /// The surfaces of water volumes near the camera.
    volumes: volume::Volumes,
    vertices: wgpu::Buffer,
    indexes: wgpu::Buffer,
    index_count: u32,
    /// The finer disc of the dynamic waves.
    fine_vertices: wgpu::Buffer,
    fine_indexes: wgpu::Buffer,
    fine_index_count: u32,
    /// One pixel of zeros, in the place of the ripples without the dynamic waves.
    flat: wgpu::TextureView,
    waves: wgpu::TextureView,
    clamp_sampler: wgpu::Sampler,
    wave_sampler: wgpu::Sampler,
    /// The reflection with its levels, for the water shader.
    reflection_view: wgpu::TextureView,
    /// Each level of the reflection alone: the first is the target of the mirror pass.
    reflection_levels: Vec<wgpu::TextureView>,
    /// A level as the source of the next one.
    level_groups: Vec<wgpu::BindGroup>,
    level_pipeline: wgpu::RenderPipeline,
    /// The levels are made in this frame: the blur that is sharp at the waterline is on.
    levels_wanted: bool,
    reflection_depth: wgpu::TextureView,
    /// The depth of the mirror pass alone, for the water shader.
    reflection_depth_read: wgpu::TextureView,
    /// One pixel of depth 1, in the place of the depth of the distant land without it.
    no_land: wgpu::TextureView,
    /// True when the reflection of this frame is made.
    reflected: bool,
    /// MGE_XE_WATER_DEBUG: a look at one term of the shader.
    debug: f32,
    pub frames: u64,
    pub reflections: u64,
}

impl Water {
    /// Makes the plane, the pipeline and the reflection target. `waves` is the DDS file of
    /// the wave texture, a volume of 32-bit pixels.
    pub fn new(scene: &SceneView, waves: &[u8]) -> Result<Water, String> {
        let device = scene.device;
        let waves = wave_texture(scene, waves)?;
        let (vertices, indexes) = coarse_disc();
        let (fine_vertices, fine_indexes) = fine_disc();
        use wgpu::util::DeviceExt;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water"), contents: bytemuck::cast_slice(&vertices), usage: wgpu::BufferUsages::VERTEX });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water"), contents: bytemuck::cast_slice(&indexes), usage: wgpu::BufferUsages::INDEX });
        let fine_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water fine"), contents: bytemuck::cast_slice(&fine_vertices), usage: wgpu::BufferUsages::VERTEX });
        let fine_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water fine"), contents: bytemuck::cast_slice(&fine_indexes), usage: wgpu::BufferUsages::INDEX });
        let flat = device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("water flat ripples"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[0u8; 8],
            )
            .create_view(&Default::default());

        let texture = |binding, sample_type, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: dimension, multisampled: false },
            count: None,
        };
        let sampler = |binding, kind| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(kind), count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                sampler(2, wgpu::SamplerBindingType::Filtering),
                // The vertices read the wave texture too, with the dynamic waves.
                wgpu::BindGroupLayoutEntry { visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ..texture(3, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3) },
                wgpu::BindGroupLayoutEntry { visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ..sampler(4, wgpu::SamplerBindingType::Filtering) },
                texture(5, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(6, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(7, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(8, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(9, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(10, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3];
        let make_pipeline = |polygon_mode: wgpu::PolygonMode, stencil: wgpu::StencilState| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, polygon_mode, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: scene.depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil,
                bias: Default::default(),
            }),
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
        let pipeline = make_pipeline(wgpu::PolygonMode::Fill, Default::default());
        let pipeline_lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE).then(|| make_pipeline(wgpu::PolygonMode::Line, Default::default()));
        // Drawn only outside the dry spaces.
        let pipeline_cut = make_pipeline(wgpu::PolygonMode::Fill, dry::outside());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water"), size: size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let volume_uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume"), size: size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let volumes = volume::Volumes::new(scene, &layout);
        let dry = dry::Dry::new(scene, &layout);
        // Between two levels too: the blur that is sharp at the waterline reads the levels
        // of the reflection. Each other read names the first level.
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let wave_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let target = |format, levels| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water reflection"),
                size: wgpu::Extent3d { width: REFLECTION_SIZE, height: REFLECTION_SIZE, depth_or_array_layers: 1 },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let reflection = target(scene.target_format, REFLECTION_LEVELS);
        let reflection_view = reflection.create_view(&Default::default());
        let reflection_levels: Vec<wgpu::TextureView> =
            (0..REFLECTION_LEVELS).map(|level| reflection.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() })).collect();
        let depth_texture = target(scene.depth_format, 1);
        let reflection_depth = depth_texture.create_view(&Default::default());
        let reflection_depth_read = depth_texture.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() });
        // A depth texture takes no write of the queue: a pass clears it to 1.
        let no_land = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("water no land"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("water no land") });
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("water no land"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &no_land,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        scene.queue.submit([encoder.finish()]);
        // The levels of the reflection: each from the level before.
        let level_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water reflection level"),
            entries: &[texture(0, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2), sampler(1, wgpu::SamplerBindingType::Filtering)],
        });
        let level_groups: Vec<wgpu::BindGroup> = reflection_levels[..reflection_levels.len() - 1]
            .iter()
            .map(|source| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water reflection level"),
                    layout: &level_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&clamp_sampler) }],
                })
            })
            .collect();
        let level_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water reflection level"), source: wgpu::ShaderSource::Wgsl(LEVEL_SHADER.into()) });
        let level_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&level_layout)], immediate_size: 0 });
        let level_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water reflection level"),
            layout: Some(&level_pipeline_layout),
            vertex: wgpu::VertexState { module: &level_module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &level_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Ok(Water {
            pipeline,
            pipeline_lines,
            pipeline_cut,
            dry,
            layout,
            uniforms,
            volume_uniforms,
            volumes,
            vertices: vertex_buffer,
            indexes: index_buffer,
            index_count: indexes.len() as u32,
            fine_vertices: fine_vertex_buffer,
            fine_indexes: fine_index_buffer,
            fine_index_count: fine_indexes.len() as u32,
            flat,
            waves,
            clamp_sampler,
            wave_sampler,
            reflection_view,
            reflection_levels,
            level_groups,
            level_pipeline,
            levels_wanted: false,
            reflection_depth,
            reflection_depth_read,
            no_land,
            reflected: false,
            debug: std::env::var("MGE_XE_WATER_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0),
            frames: 0,
            reflections: 0,
        })
    }

    /// The matrix that mirrors the world in the plane of the water (z = level - 1), with the
    /// layout of Direct3D.
    fn mirror(water_level: f32) -> [f32; 16] {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = -1.0;
        m[14] = 2.0 * (water_level - 1.0);
        m[15] = 1.0;
        m
    }

    /// The projection of the game with the far plane at the draw distance.
    fn far_projection(scene: &SceneView, draw_distance: f32) -> [f32; 16] {
        let mut projection = scene.projection;
        let far = draw_distance.max(NEAR * 2.0);
        projection[10] = far / (far - NEAR);
        projection[14] = -NEAR * far / (far - NEAR);
        projection
    }

    /// Starts the pass of the reflection: cleared to the colour of the horizon. Returns the
    /// pass and the mirrored view with the far projection. The caller draws the distant
    /// land, then calls `reflect_sky`, then ends the pass and submits the encoder.
    pub fn begin_reflection(&mut self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, frame: &WaterFrame) -> (wgpu::RenderPass<'static>, [f32; 16], [f32; 16]) {
        let clear = scene.fog_color;
        let pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water reflection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.reflection_levels[0],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.reflection_depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        let mirrored_view = multiply(&Self::mirror(frame.water_level), &scene.view);
        let projection = Self::far_projection(scene, frame.draw_distance);
        self.levels_wanted = frame.blur_reflections && frame.blur_contact;
        self.reflected = true;
        self.reflections += 1;
        (pass, mirrored_view, projection)
    }

    /// Makes the levels of the reflection after the mirror pass, when the blur that is
    /// sharp at the waterline reads them. The caller ends the mirror pass first.
    pub fn finish_reflection(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.levels_wanted {
            return;
        }
        for (source, target) in self.level_groups.iter().zip(&self.reflection_levels[1..]) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water reflection level"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.level_pipeline);
            pass.set_bind_group(0, source, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Fills the reflection with one colour: a cell without the distant land (`clearReflection`
    /// of the Direct3D 9 path). With the blur that is sharp at the waterline, each level
    /// gets the colour, and the depth of the mirror says that all is far.
    pub fn fill_reflection(&mut self, scene: &SceneView, color: [f32; 3], frame: &WaterFrame) {
        let levels = if frame.blur_reflections && frame.blur_contact { self.reflection_levels.len() } else { 1 };
        let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("water reflection fill") });
        for (index, level) in self.reflection_levels[..levels].iter().enumerate() {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water reflection fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: level,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: color[0] as f64, g: color[1] as f64, b: color[2] as f64, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: (index == 0 && levels > 1).then(|| wgpu::RenderPassDepthStencilAttachment {
                    view: &self.reflection_depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        scene.queue.submit([encoder.finish()]);
        self.reflected = true;
    }

    /// The sky of the game in the reflection: its draws again, mirrored, above the water.
    pub fn reflect_sky(&self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, replay: &mut dyn Replay, frame: &WaterFrame) {
        if frame.underwater {
            return;
        }
        let mirror = Self::mirror(frame.water_level).map(|v| v as f64);
        replay.replay(
            pass,
            &ReplayView {
                world: Some(mirror),
                view: None,
                projection: Some(Self::far_projection(scene, frame.draw_distance)),
                target_size: (REFLECTION_SIZE, REFLECTION_SIZE),
                flip_cull: true,
                // With the dynamic waves the plane can be lower by half of the wave height.
                clip_plane: Some([0.0, 0.0, 1.0, -(frame.water_level as f64 - 1.0) + 0.5 * frame.wave_height.unwrap_or(0.0) as f64]),
                opaque_only: false,
                translucent_only: false,
                force_depth_test: true,
                classes: ReplayView::SKY,
            },
        );
    }

    /// The planes of the view of the reflection, for the cull of the distant land.
    /// The 3D wave texture, for the post-processing shaders.
    pub fn waves(&self) -> wgpu::TextureView {
        self.waves.clone()
    }

    pub fn reflection_planes(mirrored_view: &[f32; 16], projection: &[f32; 16]) -> [[f32; 4]; 6] {
        planes(&multiply(mirrored_view, projection))
    }

    /// Writes the values of the frame and makes the group of the shared shading: the values,
    /// the reflection, the waves, the picture and the depth of the scene. `ripples` has the
    /// textures of the dynamic waves; they count when the frame has a wave height. The values
    /// go into `uniforms`. `None` without the picture or the depth.
    fn frame_group(&self, scene: &SceneView, frame: &WaterFrame, ripples: Option<&super::ripples::Ripples>, uniforms_buffer: &wgpu::Buffer) -> Option<wgpu::BindGroup> {
        let (Some(picture), Some(depth)) = (scene.picture, scene.depth) else { return None };
        let dynamic = frame.wave_height.zip(ripples);
        let eye = super::eye_of(&scene.view);
        let sunlight = 1.0 - (1.0 - frame.sun_vis).powi(2);
        let wind_factor = ((frame.wind[0] * frame.wind[0] + frame.wind[1] * frame.wind[1]).sqrt() + 1.5) / 140.0;
        let scattering = frame.scattering.unwrap_or([[0.0; 4]; 5]);
        let uniforms = Uniforms {
            view_projection: multiply(&scene.view, &scene.projection),
            view: scene.view,
            world: [eye[0], eye[1], frame.water_level, 0.0],
            eye: [eye[0], eye[1], eye[2], frame.time],
            sun_pos: [frame.sun_pos[0], frame.sun_pos[1], frame.sun_pos[2], frame.sun_vis],
            sun_col: [frame.sun_color[0] * sunlight, frame.sun_color[1] * sunlight, frame.sun_color[2] * sunlight, wind_factor],
            sky_col: [frame.sky_color[0], frame.sky_color[1], frame.sky_color[2], frame.nice_weather],
            fog_col_far: [frame.fog_color_far[0], frame.fog_color_far[1], frame.fog_color_far[2], if frame.scattering.is_some() { 1.0 } else { 0.0 }],
            fog: [frame.fog_exp.0, frame.fog_exp.1, self.debug, if frame.underwater { 1.0 } else { 0.0 }],
            projection: [scene.projection[10], scene.projection[14], frame.blur_tune[2], 0.0],
            size: [scene.target_size.0 as f32, scene.target_size.1 as f32, frame.view_end.unwrap_or(0.0), match (frame.blur_reflections, frame.blur_contact) {
                (false, _) => 0.0,
                (true, false) => 1.0,
                (true, true) => 2.0,
            }],
            sky: scattering[1],
            outscatter: scattering[2],
            inscatter: scattering[3],
            sun_altitude: scattering[4],
            wave: dynamic.map_or([0.0; 4], |(height, ripples)| [ripples.origin[0], ripples.origin[1], height, 1.0]),
            mirror: {
                let projection = Self::far_projection(scene, frame.draw_distance);
                // The cone of the mirror: wider with more wind, as the waves are.
                [projection[10], projection[14], scene.projection[0], frame.blur_tune[0] + frame.blur_tune[1] * wind_factor]
            },
            land: {
                // The depth of the distant land, and the clip depth of the plane: the near plane
                // of the game and the far plane of the distant land.
                let (land_a, land_b) = (frame.land_projection[10], frame.land_projection[14]);
                let near = -scene.projection[14] / scene.projection[10];
                let far = land_b / (1.0 - land_a);
                [land_a, land_b, far / (far - near), -near * far / (far - near)]
            },
        };
        scene.queue.write_buffer(uniforms_buffer, 0, bytemuck::bytes_of(&uniforms));
        let group = scene.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.reflection_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.waves) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.wave_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(dynamic.map_or(&self.flat, |(_, ripples)| ripples.rain_view())) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(dynamic.map_or(&self.flat, |(_, ripples)| ripples.player_view())) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&self.reflection_depth_read) },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(scene.before_depth.unwrap_or(&self.no_land)) },
            ],
        });
        Some(group)
    }

    /// Draws the plane in place of the water plane of the game. `ripples` has the
    /// textures of the dynamic waves; they count when the frame has a wave height.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &WaterFrame, ripples: Option<&super::ripples::Ripples>) {
        let Some(group) = self.frame_group(scene, frame, ripples, &self.uniforms) else { return };
        let dynamic = frame.wave_height.zip(ripples);
        // The dry spaces cut the plane, but not from under the water (`renderStageWater`).
        let cut = self.dry.any() && !frame.underwater;
        let eye = super::eye_of(&scene.view);
        if cut && frame.camera_dry && (eye[2] - frame.water_level).abs() < DRY_LEVEL_MARGIN {
            // The eye is in a dry space at the level of the water: the waves put the plane
            // now over the eye and now under it, and the side that the count is made for is
            // not sure. The plane is edge-on and is left out.
            return;
        }
        pass.set_bind_group(0, &group, &[]);
        if cut {
            self.dry.zero(pass);
            self.dry.count_plane(pass);
            pass.set_pipeline(&self.pipeline_cut);
            pass.set_stencil_reference(0);
        } else {
            pass.set_pipeline(if scene.wireframe { self.pipeline_lines.as_ref().unwrap_or(&self.pipeline) } else { &self.pipeline });
        }
        // The finer disc with the dynamic waves: the waves move its vertices.
        let (vertices, indexes, index_count) = if dynamic.is_some() { (&self.fine_vertices, &self.fine_indexes, self.fine_index_count) } else { (&self.vertices, &self.indexes, self.index_count) };
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..index_count, 0, 0..1);
        if cut {
            self.dry.zero(pass);
        }
        self.frames += 1;
        self.reflected = false;
    }

    /// The triangles of the dry spaces, three corners each, in the world.
    pub fn set_masks(&mut self, scene: &SceneView, corners: &[[f32; 3]]) {
        self.dry.set_masks(scene, corners);
    }
}

impl Water {
    /// Draws the surfaces of water volumes among the shapes that the renderer held for the
    /// extension, with the picture and the depth of the scene so far. Returns the number
    /// drawn.
    pub fn draw_volumes(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &WaterFrame, ripples: Option<&super::ripples::Ripples>, held: &[nirender_wgpu::HeldDraw], far: &[FarWater], look: &VolumeLook, looks: &std::collections::HashMap<u32, Look>) -> usize {
        if far.is_empty() && !held.iter().any(|h| volume::marked(h.facts.specular_power)) {
            return 0;
        }
        let Some(group) = self.frame_group(scene, frame, ripples, &self.volume_uniforms) else { return 0 };
        // The dry spaces cut the surfaces near the camera, but not from under the water.
        let dry = (self.dry.any() && !frame.underwater).then_some(&self.dry);
        self.volumes.draw(scene, pass, &group, held, far, look, looks, dry)
    }

    /// The water among the distant statics drawn since the start.
    pub fn far_drawn(&self) -> u64 {
        self.volumes.far_drawn
    }

    /// The surfaces of water volumes drawn since the start.
    pub fn volumes_drawn(&self) -> u64 {
        self.volumes.drawn
    }
}

/// The wave texture: a DDS volume of 32-bit BGRA pixels, its first level only.
pub(super) fn wave_texture(scene: &SceneView, dds: &[u8]) -> Result<wgpu::TextureView, String> {
    let word = |at: usize| u32::from_le_bytes(dds[at..at + 4].try_into().unwrap());
    if dds.len() < 128 || &dds[0..4] != b"DDS " {
        return Err("the wave texture is not a DDS file".into());
    }
    let (height, width, depth) = (word(12), word(16), word(24).max(1));
    // DDSD_DEPTH is 0x800000; 32 bits with the red mask 0x00FF0000 is BGRA.
    if word(8) & 0x80_0000 == 0 || word(88) != 32 || word(92) != 0x00FF_0000 {
        return Err("the wave texture is not a volume of 32-bit BGRA pixels".into());
    }
    let bytes = (width * height * depth * 4) as usize;
    if dds.len() < 128 + bytes {
        return Err("the wave texture is too short".into());
    }
    use wgpu::util::DeviceExt;
    let texture = scene.device.create_texture_with_data(
        scene.queue,
        &wgpu::TextureDescriptor {
            label: Some("water waves"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: depth },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &dds[128..128 + bytes],
    );
    Ok(texture.create_view(&Default::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_discs_have_the_sizes_of_the_direct3d_9_path() {
        // numWaterVerts = resS * resT + 1; numWaterTris = 2 * resS * resT - resS.
        let (vertices, indexes) = coarse_disc();
        assert_eq!((vertices.len(), indexes.len()), (16 * 15 + 1, 3 * (2 * 16 * 15 - 16)));
        assert_eq!(vertices[1], [4096.0, 0.0, -1.0]);
        let (vertices, indexes) = fine_disc();
        assert_eq!((vertices.len(), indexes.len()), (150 * 120 + 1, 3 * (2 * 150 * 120 - 150)));
        // The first ring is at the centre, the last one past the horizon; each index is
        // one of the vertices.
        assert_eq!(vertices[1], [0.0, 0.0, -1.0]);
        assert_eq!(vertices[vertices.len() - 150][0], 500_000.0);
        assert!(indexes.iter().all(|i| (*i as usize) < vertices.len()));
        // The ring before the last is within the view range of the game times 1.2.
        let before = vertices[vertices.len() - 300][0];
        assert!(before > 8192.0 && before < 9600.0, "{before}");
    }

    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        use wgpu::naga;
        let source = shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        for entry in ["vs", "fs"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
        let level = naga::front::wgsl::parse_str(LEVEL_SHADER).unwrap_or_else(|error| panic!("{}", error.emit_to_string(LEVEL_SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&level).unwrap_or_else(|error| panic!("{error:?}"));
    }
}
