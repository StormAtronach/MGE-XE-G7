//! The wgpu renderer mode of the helper (`runtime.renderer_wgpu`).
//!
//! The nirender plugin starts the helper with `--live <name>`. The crate `nirender_wgpu`
//! draws the stream of the game in the window of the game. This module adds what MGE XE
//! draws and the game does not. It is a spike with one feature: the terrain of the distant
//! land.
//!
//! The spike reads `terrain.bin` and `terrain_patch_albedo.dds` of the distant land folder
//! on a thread of its own, makes one vertex buffer and one index buffer for each mesh of the
//! file, and draws the meshes that are in the view before the world of the game. The colour
//! is the far colour of the terrain shader of MGE XE (`XE Mod Landscape.fx`): the patch
//! albedo times the vertex colour times the light of the sun and the ambient light, with a
//! linear fog. The near colour of that shader (the texture atlas with its blend patterns) is
//! not in the spike.
//!
//! Not in the spike: the quadtree and the horizon culling of the IPC host mode (a test of
//! each mesh against the view is enough for 358 meshes), distant statics, grass, water,
//! shadows, and the exponential fog with scattering.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use distantland::mge_xe::distant_terrain::{TERRAIN_FILE_NAME, TERRAIN_PATCH_ALBEDO_FILE_NAME, TerrainFile, load_terrain_file};
use mge_config::{ConfigDocument, FILE_NAME};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::{Extension, SceneView};

/// The side of an exterior cell of the game, in the units of the game.
const CELL: f32 = 8192.0;
/// The side of a terrain patch: the patch albedo has one pixel for each patch.
const PATCH: f32 = 512.0;
/// The near plane of the projection of the distant land.
const NEAR: f32 = 64.0;

/// Runs the renderer with the distant land of MGE XE.
pub fn run(arguments: Vec<String>) {
    nirender_wgpu::run_with(arguments, Some(Box::new(DistantLand::start(Path::new(".")))));
}

/// What the loader thread gives to the renderer.
struct Loaded {
    terrain: TerrainFile,
    /// The bytes of the patch albedo file: a DDS file with DXT1 pixels.
    albedo: Vec<u8>,
}

struct Mesh {
    vertices: wgpu::Buffer,
    indexes: wgpu::Buffer,
    index_count: u32,
    center: [f32; 3],
    radius: f32,
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    meshes: Vec<Mesh>,
    /// `terrainWorldOrigin` of the terrain shader, and 1 over the size of the patch albedo
    /// in the units of the game.
    world_origin: [f32; 2],
    inverse_size: [f32; 2],
}

pub struct DistantLand {
    enabled: bool,
    /// The distances of the settings of MGE XE, in the units of the game.
    draw_distance: f32,
    fog_start: f32,
    fog_end: f32,
    loading: Option<Receiver<Result<Loaded, String>>>,
    gpu: Option<Gpu>,
    drew: bool,
    meshes_drawn: u64,
    frames: u64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_projection: [f32; 16],
    view: [f32; 16],
    /// The world origin of the terrain, and 1 over the size of the patch albedo.
    terrain: [f32; 4],
    fog_color: [f32; 4],
    /// The start and the end of the fog.
    fog: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
}

const SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    view: mat4x4f,
    terrain: vec4f,
    fog_color: vec4f,
    fog: vec4f,
    sun_direction: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var albedo: texture_2d<f32>;
@group(0) @binding(2) var albedo_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) normal: vec3f,
    @location(1) color: vec3f,
    @location(2) local: vec2f,
    @location(3) distance: f32,
}

// The matrices have the numbers of Direct3D, row after row. As the columns of a matrix here
// they are the transposed matrix, which is the one for a column vector.
@vertex fn vs(@location(0) position: vec3f, @location(1) normal: vec4f, @location(2) color: vec4f) -> Out {
    var out: Out;
    let world = vec4f(position, 1.0);
    out.position = u.view_projection * world;
    out.normal = normal.xyz * 2.0 - 1.0;
    // The colour of a vertex has the bytes blue, green, red.
    out.color = color.bgr;
    out.local = position.xy - u.terrain.xy;
    out.distance = length((u.view * world).xyz);
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let far_albedo = textureSample(albedo, albedo_sampler, in.local * u.terrain.zw).rgb;
    let light = u.ambient.rgb + u.sun_color.rgb * clamp(dot(normalize(in.normal), -u.sun_direction.xyz), 0.0, 1.0);
    let lit = far_albedo * in.color * light;
    let fog = clamp((u.fog.y - in.distance) / max(u.fog.y - u.fog.x, 1.0), 0.0, 1.0);
    return vec4f(mix(u.fog_color.rgb, lit, fog), 1.0);
}
";

impl DistantLand {
    /// Reads the settings of MGE XE in `root` and starts the load of the terrain.
    pub fn start(root: &Path) -> DistantLand {
        let document = ConfigDocument::open(root.join(FILE_NAME));
        let settings = &document.settings().distant_land;
        let mut land = DistantLand {
            enabled: settings.enabled,
            draw_distance: settings.draw_distance * CELL,
            fog_start: settings.fog.above_water_start * CELL,
            fog_end: settings.fog.above_water_end * CELL,
            loading: None,
            gpu: None,
            drew: false,
            meshes_drawn: 0,
            frames: 0,
        };
        println!(
            "distant land: {}, draw distance {:.1} cells, fog from {:.2} to {:.2} cells",
            if land.enabled { "on" } else { "off in mgeXE.toml" },
            settings.draw_distance,
            settings.fog.above_water_start,
            settings.fog.above_water_end
        );
        if land.enabled {
            let folder: PathBuf = root.join("Data Files").join("distantland");
            let (sender, receiver) = channel();
            land.loading = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(load(&folder));
            });
        }
        land
    }

    /// Makes the buffers, the texture and the pipeline when the loader thread is done.
    fn finish_load(&mut self, scene: &SceneView) {
        let Some(receiver) = self.loading.as_ref() else { return };
        let Ok(result) = receiver.try_recv() else { return };
        self.loading = None;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(problem) => {
                println!("distant land: not loaded: {problem}");
                self.enabled = false;
                return;
            }
        };
        match make_gpu(scene, &loaded) {
            Ok(gpu) => {
                println!("distant land: {} terrain meshes on the GPU", gpu.meshes.len());
                self.gpu = Some(gpu);
            }
            Err(problem) => {
                println!("distant land: not made: {problem}");
                self.enabled = false;
            }
        }
    }
}

impl Drop for DistantLand {
    fn drop(&mut self) {
        println!("distant land: {} frames with terrain, {} mesh draws", self.frames, self.meshes_drawn);
    }
}

fn load(folder: &Path) -> Result<Loaded, String> {
    let terrain = load_terrain_file(&folder.join(TERRAIN_FILE_NAME)).map_err(|error| format!("{}: {error}", TERRAIN_FILE_NAME))?;
    let albedo = std::fs::read(folder.join(TERRAIN_PATCH_ALBEDO_FILE_NAME)).map_err(|error| format!("{}: {error}", TERRAIN_PATCH_ALBEDO_FILE_NAME))?;
    Ok(Loaded { terrain, albedo })
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn make_gpu(scene: &SceneView, loaded: &Loaded) -> Result<Gpu, String> {
    let device = scene.device;
    // A DDS file: `DDS `, a header of 124 bytes with the height at 12, the width at 16 and
    // the number of levels at 28, and the four letters of the format at 84.
    let dds = &loaded.albedo;
    if dds.len() < 128 || &dds[0..4] != b"DDS " || &dds[84..88] != b"DXT1" {
        return Err("the patch albedo is not a DDS file with DXT1 pixels".into());
    }
    let (height, width, levels) = (word(dds, 12), word(dds, 16), word(dds, 28).max(1));
    if width % 4 != 0 || height % 4 != 0 {
        return Err(format!("the patch albedo has the size {width} x {height}, not a multiple of 4"));
    }
    let texture = device.create_texture_with_data(
        scene.queue,
        &wgpu::TextureDescriptor {
            label: Some("terrain patch albedo"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc1RgbaUnorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &dds[128..],
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("distant land"),
        size: size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
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
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant land"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    // A terrain vertex of `terrain.bin` has 20 bytes: the position, the normal and the colour.
    let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Unorm8x4];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant land"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
        },
        // The two sides: the winding of the file is not checked in the spike.
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
            targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let terrain = &loaded.terrain;
    let mut meshes = Vec::with_capacity(terrain.meshes.len());
    for mesh in &terrain.meshes {
        if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
            continue;
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indexes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.triangles),
            usage: wgpu::BufferUsages::INDEX,
        });
        let center = mesh.bounding_sphere.center;
        meshes.push(Mesh {
            vertices,
            indexes,
            index_count: mesh.triangles.len() as u32 * 3,
            center: [center.x, center.y, center.z],
            radius: mesh.bounding_sphere.radius,
        });
    }
    let material = terrain.material_size_xy;
    Ok(Gpu {
        pipeline,
        group,
        uniforms,
        meshes,
        world_origin: [terrain.world_origin.x, terrain.world_origin.y],
        inverse_size: [1.0 / (PATCH * material[0].max(1) as f32), 1.0 / (PATCH * material[1].max(1) as f32)],
    })
}

/// `a * b` for two matrices of Direct3D, row after row.
fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            out[row * 4 + column] = (0..4).map(|k| a[row * 4 + k] * b[k * 4 + column]).sum();
        }
    }
    out
}

/// The six planes of the view of a matrix of Direct3D, each with a normal of length 1 that
/// goes into the view.
fn planes(m: &[f32; 16]) -> [[f32; 4]; 6] {
    let column = |j: usize| [m[j], m[4 + j], m[8 + j], m[12 + j]];
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let subtract = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    let (x, y, z, w) = (column(0), column(1), column(2), column(3));
    let mut planes = [add(w, x), subtract(w, x), add(w, y), subtract(w, y), z, subtract(w, z)];
    for plane in &mut planes {
        let length = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt().max(1e-12);
        for value in plane.iter_mut() {
            *value /= length;
        }
    }
    planes
}

impl Extension for DistantLand {
    fn before_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>) -> bool {
        self.drew = false;
        if !self.enabled || scene.interior {
            return false;
        }
        self.finish_load(scene);
        let Some(gpu) = self.gpu.as_ref() else { return false };

        // The projection of the game with a far plane at the draw distance of the distant
        // land. In a projection of Direct3D the two numbers of the depth are at 10 and 14.
        let mut projection = scene.projection;
        let far = self.draw_distance.max(NEAR * 2.0);
        projection[10] = far / (far - NEAR);
        projection[14] = -NEAR * far / (far - NEAR);
        let view_projection = multiply(&scene.view, &projection);
        let sun = scene.sun_direction.unwrap_or([0.0, 0.0, -1.0]);
        let (sun_color, ambient) = match scene.sun_direction {
            Some(_) => (scene.sun_color, scene.ambient),
            // Without a sun in the draw of the game: the colour of the terrain as it is.
            None => ([0.0; 3], [1.0; 3]),
        };
        let uniforms = Uniforms {
            view_projection,
            view: scene.view,
            terrain: [gpu.world_origin[0], gpu.world_origin[1], gpu.inverse_size[0], gpu.inverse_size[1]],
            fog_color: scene.fog_color,
            fog: [self.fog_start, self.fog_end, 0.0, 0.0],
            sun_direction: [sun[0], sun[1], sun[2], 0.0],
            sun_color: [sun_color[0], sun_color[1], sun_color[2], 0.0],
            ambient: [ambient[0], ambient[1], ambient[2], 0.0],
        };
        scene.queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let planes = planes(&view_projection);
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &gpu.group, &[]);
        let mut drawn = 0;
        for mesh in &gpu.meshes {
            let inside = planes.iter().all(|p| p[0] * mesh.center[0] + p[1] * mesh.center[1] + p[2] * mesh.center[2] + p[3] >= -mesh.radius);
            if !inside {
                continue;
            }
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indexes.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            drawn += 1;
        }
        self.meshes_drawn += drawn;
        self.frames += 1;
        self.drew = true;
        true
    }

    /// The draws of the world of the game get the fog of the distant land, so that the near
    /// cells go into the fog at the same distances as the terrain behind them.
    fn fog_range(&self) -> Option<(f32, f32)> {
        self.drew.then_some((self.fog_start, self.fog_end))
    }
}
