//! The water of MGE XE: the plane in place of the water plane of the game, with a reflection
//! of the sky and the distant land, the refraction of the picture so far, and the depth of
//! the game for the shore. `XE Mod Water.fx` without the ripples; the port specification is
//! in `moreFPS/docs/plans/mge-xe-water-port-spec.md`.

use nirender_wgpu::{Replay, ReplayView, SceneView, wgpu};

use super::{multiply, planes};

/// The size of the reflection, as the Direct3D 9 path has it.
const REFLECTION_SIZE: u32 = 1024;
/// The near plane of the distant land, as the Direct3D 9 path has it.
const NEAR: f32 = 4.0;

/// The values of a frame for the water shader. The names are those of the uniforms of
/// `XE Mod Water.fx`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_projection: [f32; 16],
    view: [f32; 16],
    /// The translation of the plane: the camera's x and y, and the water level.
    world: [f32; 4],
    /// The camera, and the time of the simulation.
    eye: [f32; 4],
    /// The direction to the sun, and its visibility.
    sun_pos: [f32; 4],
    /// The colour of the sun with its visibility in it, and the wind factor.
    sun_col: [f32; 4],
    /// The sky colour, and the good weather value.
    sky_col: [f32; 4],
    /// The far fog colour, and 1 when the scattering is on.
    fog_col_far: [f32; 4],
    /// The start and the divisor of the exponential fog; z the debug term; w 1 under water.
    fog: [f32; 4],
    /// The two numbers of the depth of the projection of the game, for the distance of a
    /// pixel from its depth.
    projection: [f32; 4],
    /// The size of the picture.
    size: [f32; 4],
    /// The scattering: as the terrain has it.
    sky: [f32; 4],
    outscatter: [f32; 4],
    inscatter: [f32; 4],
    sun_altitude: [f32; 4],
}

const SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    view: mat4x4f,
    world: vec4f,
    eye: vec4f,
    sun_pos: vec4f,
    sun_col: vec4f,
    sky_col: vec4f,
    fog_col_far: vec4f,
    fog: vec4f,
    projection: vec4f,
    size: vec4f,
    sky: vec4f,
    outscatter: vec4f,
    inscatter: vec4f,
    sun_altitude: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var reflection: texture_2d<f32>;
@group(0) @binding(2) var clamp_sampler: sampler;
@group(0) @binding(3) var waves: texture_3d<f32>;
@group(0) @binding(4) var wave_sampler: sampler;
@group(0) @binding(5) var picture: texture_2d<f32>;
@group(0) @binding(6) var depth: texture_depth_2d;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) texcoords: vec4f,
    @location(2) clip_w: f32,
}

@vertex fn vs(@location(0) local: vec3f) -> Out {
    var out: Out;
    // The plane lies at the water level less 1, a little under the ripples of the game.
    var world = local + u.world.xyz;
    world.z -= 0.1;
    out.world = world;
    out.texcoords = vec4f(world.xy / 3900.0, world.xy / 527.0);
    out.position = u.view_projection * vec4f(world, 1.0);
    out.clip_w = out.position.w;
    return out;
}

// fogColourWater: the exponential fog with the scattering of the atmosphere. rgb is the
// colour that the fog adds, a is what is left of the colour of the thing.
fn fog_water(direction: vec3f, distance: f32) -> vec4f {
    var fog_distance = (distance - u.fog.x) / max(u.fog.y, 1.0);
    let fog = clamp(exp(-fog_distance), 0.0, 1.0);
    fog_distance = clamp(0.224 * fog_distance, 0.0, 1.0);
    var added = u.fog_col_far.rgb * (1.0 - fog);
    if u.fog_col_far.w > 0.5 && u.sky_col.w > 0.001 && u.eye.z > -1.0 {
        let sun_cos = dot(direction, u.sun_pos.xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * u.sun_altitude.z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(u.inscatter.rgb, u.outscatter.rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (u.sun_altitude.x + 0.7 * mie);
        lost = (1.0 - exp(-fog_distance * lost)) / lost;
        let scattered = (0.125 * mie + u.sky.rgb * rayleigh) * lost * (1.17 * air + 0.89) * u.sun_altitude.y;
        added = mix(added, scattered, u.sky_col.w);
    }
    return vec4f(added, fog);
}

// getFinalWaterNormal: two reads of the wave texture, the far one past 8000 units.
fn water_normal(texcoords: vec4f, distance: f32) -> vec3f {
    let t = 0.4 * u.eye.w;
    let far = textureSample(waves, wave_sampler, vec3f(texcoords.xy, t)).rg;
    let close = textureSample(waves, wave_sampler, vec3f(texcoords.zw, t)).rg;
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

// The distance along the view of the camera at a point of the picture, from the depth of
// the game. Very far where the game drew nothing.
fn distance_at(uv: vec2f) -> f32 {
    let size = vec2i(textureDimensions(depth));
    let at = clamp(vec2i(uv * u.size.xy), vec2i(0), size - 1);
    let d = textureLoad(depth, at, 0);
    if d >= 1.0 {
        return 1e30;
    }
    return u.projection.y / (d - u.projection.x);
}

// UnderwaterPS: the plane seen from below.
fn underwater(in: Out, eye_vec: vec3f, dist: f32) -> vec4f {
    let fog = clamp(exp(-dist / 4096.0), 0.0, 1.0);
    let normal = -water_normal(in.texcoords, dist);
    let reffactor = 2.0 * (u.sun_col.w * dist + 0.1) * normal.xy;
    let uv = in.position.xy / u.size.xy;
    let w = in.clip_w;
    var refracted = textureSample(picture, clamp_sampler, uv + 2.0 * -reffactor.xy / w).rgb;
    refracted = mix(u.fog_col_far.rgb, refracted, exp(-dist / 500.0));
    let reflected = textureSample(reflection, clamp_sampler, uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / w).rgb;
    // With the total reflection of the inside of the water.
    let fresnel = pow(clamp(1.12 - 0.65 * dot(-eye_vec, normal), 0.0, 1.0), 8.0);
    let result = mix(refracted, reflected, fresnel);
    let refract_sun = dot(-eye_vec, normalize(-u.sun_pos.xyz + normal));
    let spec = u.sun_col.rgb * pow(max(refract_sun, 0.0), 6.0) * fog;
    return vec4f(result + spec, 1.0);
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    var eye_vec = in.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    if u.fog.w > 0.5 {
        return underwater(in, eye_vec, dist);
    }
    let fog = fog_water(eye_vec, dist);
    let sun_col_adjusted = u.sun_col.rgb;
    let depth_base = sun_col_adjusted * vec3f(0.03, 0.04, 0.05) + (2.0 * u.sky_col.rgb + u.fog_col_far.rgb) * vec3f(0.075, 0.08, 0.085);
    let depth_color = fog.a * depth_base + fog.rgb;
    let normal = water_normal(in.texcoords, dist);
    let reffactor = (u.sun_col.w * dist + 0.1) * normal.xy;
    // The position of the pixel in the picture. The offsets of the shader are in the
    // projected space: here they are divided by the clip w of the pixel.
    let uv = in.position.xy / u.size.xy;
    let w = in.clip_w;
    var new_uv = uv + reffactor.yx / w;
    var depth_of = max(0.0, distance_at(new_uv) - w);
    let depth_first = depth_of;
    var refracted = depth_color;
    var shorefactor = 0.0;
    if depth_of < 4000.0 {
        new_uv = uv + clamp(depth_of / 100.0, 0.0, 1.0) * reffactor.yx / w;
        refracted = textureSample(picture, clamp_sampler, new_uv).rgb;
        depth_of = max(0.0, distance_at(new_uv) - w);
        // From a difference of the depth to a distance along the ray.
        depth_of /= dot(eye_vec, vec3f(u.view[0].z, u.view[1].z, u.view[2].z));
        depth_of += 300.0 * (0.95 - normal.z);
        let depthscale = clamp(exp(-depth_of / 800.0), 0.0, 1.0);
        shorefactor = pow(depthscale, 90.0);
        refracted = mix(depth_color, refracted, 0.8 * depthscale + 0.2 * shorefactor);
    }
    var reflected = textureSample(reflection, clamp_sampler, uv - vec2f(2.1 * reffactor.x, -abs(reffactor.y)) / w).rgb;
    reflected = mix(fog.rgb, reflected, fog.a);
    var adjust = mix(vec3f(0.0, 0.0, 0.1), normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, vec3f(0.0, 0.0, 1.0), (1.0 + eye_vec.z) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0)));
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(refracted, reflected, fresnel);
    let vdotr = clamp(1.0025 * dot(-eye_vec, reflect(-u.sun_pos.xyz, normal)), 0.0, 1.0);
    result += sun_col_adjusted * (pow(vdotr, 170.0) + 0.07 * pow(vdotr, 4.0)) * fog.a;
    result = mix(result, refracted, shorefactor * fog.a);
    // A look at one term, for a test: MGE_XE_WATER_DEBUG.
    if u.fog.z == 1.0 {
        return vec4f(reflected, 1.0);
    } else if u.fog.z == 2.0 {
        return vec4f(refracted, 1.0);
    } else if u.fog.z == 3.0 {
        return vec4f(vec3f(depth_first / 1000.0), 1.0);
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
}

pub struct Water {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indexes: wgpu::Buffer,
    index_count: u32,
    waves: wgpu::TextureView,
    clamp_sampler: wgpu::Sampler,
    wave_sampler: wgpu::Sampler,
    reflection_view: wgpu::TextureView,
    reflection_depth: wgpu::TextureView,
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
        // The disc: a centre, then 15 rings of 16 vertices, the rings further apart.
        let (segments, rings) = (16usize, 15usize);
        let mut vertices: Vec<[f32; 3]> = vec![[0.0, 0.0, -1.0]];
        for t in 0..rings {
            let r = 4096.0 * (1.0 + (t * t) as f32);
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
        use wgpu::util::DeviceExt;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water"), contents: bytemuck::cast_slice(&vertices), usage: wgpu::BufferUsages::VERTEX });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water"), contents: bytemuck::cast_slice(&indexes), usage: wgpu::BufferUsages::INDEX });

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
                texture(3, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3),
                sampler(4, wgpu::SamplerBindingType::Filtering),
                texture(5, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(6, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: scene.depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
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
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water"), size: size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water reflection"),
                size: wgpu::Extent3d { width: REFLECTION_SIZE, height: REFLECTION_SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let reflection_view = target(scene.target_format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default());
        let reflection_depth = target(scene.depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        Ok(Water {
            pipeline,
            layout,
            uniforms,
            vertices: vertex_buffer,
            indexes: index_buffer,
            index_count: indexes.len() as u32,
            waves,
            clamp_sampler,
            wave_sampler,
            reflection_view,
            reflection_depth,
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
                    view: &self.reflection_view,
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
        self.reflected = true;
        self.reflections += 1;
        (pass, mirrored_view, projection)
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
                clip_plane: Some([0.0, 0.0, 1.0, -(frame.water_level as f64 - 1.0)]),
                opaque_only: false,
                force_depth_test: true,
                classes: ReplayView::SKY,
            },
        );
    }

    /// The planes of the view of the reflection, for the cull of the distant land.
    pub fn reflection_planes(mirrored_view: &[f32; 16], projection: &[f32; 16]) -> [[f32; 4]; 6] {
        planes(&multiply(mirrored_view, projection))
    }

    /// Draws the plane in place of the water plane of the game.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &WaterFrame) {
        let (Some(picture), Some(depth)) = (scene.picture, scene.depth) else { return };
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
            projection: [scene.projection[10], scene.projection[14], 0.0, 0.0],
            size: [scene.target_size.0 as f32, scene.target_size.1 as f32, 0.0, 0.0],
            sky: scattering[1],
            outscatter: scattering[2],
            inscatter: scattering[3],
            sun_altitude: scattering[4],
        };
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let group = scene.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.reflection_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.waves) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.wave_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(depth) },
            ],
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indexes.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
        self.frames += 1;
        self.reflected = false;
    }
}

/// The wave texture: a DDS volume of 32-bit BGRA pixels, its first level only.
fn wave_texture(scene: &SceneView, dds: &[u8]) -> Result<wgpu::TextureView, String> {
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
