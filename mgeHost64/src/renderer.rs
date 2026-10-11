//! The wgpu renderer mode of the helper (`runtime.renderer_wgpu`).
//!
//! The nirender plugin starts the helper with `--live <name>`. The crate `nirender_wgpu`
//! draws the stream of the game in the window of the game. This module adds what MGE XE
//! draws and the game does not. For now that is the terrain of the distant land.
//!
//! The terrain is `terrain.bin` and its five textures in the distant land folder. A thread
//! reads the files; the renderer thread makes one vertex buffer and one index buffer for
//! each mesh and draws the meshes that are in the view before the world of the game. The
//! shader is `LandscapePS` of `XE Mod Landscape.fx`: near the camera the texture atlas with
//! the material map and the blend patterns, far from it the patch albedo, times the vertex
//! colour and the light of the sun and the sky.
//!
//! The fog is `fogColour` of `XE Common.fx`: the exponential curve past the view range of the
//! game and the linear fog of the near cells before it, with the scattering of the
//! atmosphere in good weather. The distances and the scattering values come from
//! the 32-bit part of MGE XE with each frame (`Deviceless::sendEnvironment` in
//! `mged3d8device.cpp`), because they follow the weather. Without them (the game runs with
//! the `d3d8.dll` of the plugin and not with that of MGE XE) the fog is linear with the
//! distances of `mgeXE.toml`.
//!
//! Not here yet: the quadtree and the horizon
//! culling of the IPC host mode (a test of each mesh against the view is enough for the 358
//! meshes of the test data), distant statics, grass, water and shadows.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Instant;

mod statics;

use distantland::mge_xe::distant_terrain::{
    TERRAIN_ATLAS_FILE_NAME, TERRAIN_BLEND_PATTERNS_FILE_NAME, TERRAIN_FILE_NAME, TERRAIN_MATERIAL_FILE_NAME, TERRAIN_MATERIAL_FLAGS_FILE_NAME,
    TERRAIN_PATCH_ALBEDO_FILE_NAME, TerrainFile, load_terrain_file,
};
use mge_config::{ConfigDocument, FILE_NAME};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::{Extension, SceneView};

/// The side of an exterior cell of the game, in the units of the game.
const CELL: f32 = 8192.0;
/// The near plane of the projection of the distant land.
const NEAR: f32 = 64.0;
/// The number of the environment data in the stream. `Deviceless::EnvironmentId` of MGE XE.
const ENVIRONMENT_ID: u32 = 1;

/// Runs the renderer with the distant land of MGE XE.
pub fn run(arguments: Vec<String>) {
    nirender_wgpu::run_with(arguments, Some(Box::new(DistantLand::start(Path::new(".")))));
}

/// The state of the game for a frame, from the 32-bit part of MGE XE. The layout is that of
/// `Deviceless::Environment` in `mged3d8device.cpp`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Environment {
    version: u32,
    /// 1: exponential fog. 2: the camera is under water. 4: an exterior.
    flags: u32,
    near_view_range: f32,
    fog_start: f32,
    fog_end: f32,
    fog_exp_start: f32,
    fog_exp_divisor: f32,
    fog_near_start: f32,
    fog_near_end: f32,
    nice_weather: f32,
    light_sun_mult: f32,
    light_amb_mult: f32,
    wind_scaling: f32,
    eye: [f32; 3],
    /// Version 2. Flag 8: the scattering of the atmosphere is on for this frame.
    sun_pos: [f32; 3],
    sky_col: [f32; 3],
    /// The fog colour of the weather, before the scattering changed it.
    fog_col_far: [f32; 3],
    outscatter: [f32; 3],
    inscatter: [f32; 3],
    sky_scatter_far: [f32; 4],
    /// Version 3, for the grass: the smooth wind, the position of the player with 1 when it
    /// is known, and the time of the simulation of the game.
    wind: [f32; 2],
    foot_pos: [f32; 3],
    foot_valid: u32,
    time: f32,
}

/// What the loader thread gives to the renderer: the terrain and the bytes of its five DDS
/// files.
struct Loaded {
    terrain: TerrainFile,
    atlas: Vec<u8>,
    material: Vec<u8>,
    material_flags: Vec<u8>,
    patch_albedo: Vec<u8>,
    blend_patterns: Vec<u8>,
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
    /// The numbers of the header of `terrain.bin` for the shader.
    terrain: [f32; 4],
    atlas: [f32; 4],
    atlas_more: [f32; 4],
    patterns: [f32; 4],
}

pub struct DistantLand {
    enabled: bool,
    /// The distances of the settings of MGE XE, in the units of the game.
    draw_distance: f32,
    settings_fog: (f32, f32),
    environment: Option<Environment>,
    loading: Option<Receiver<Result<Loaded, String>>>,
    gpu: Option<Gpu>,
    /// The fog distances for the near cells of the frame, when the terrain is in it.
    near_fog: Option<(f32, f32)>,
    statics: Option<statics::Statics>,
    started: Instant,
    meshes_drawn: u64,
    frames: u64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_projection: [f32; 16],
    /// The position of the camera, and the view range of the game.
    eye: [f32; 4],
    /// The world origin of the terrain, and 1 over the size of the material map.
    terrain: [f32; 4],
    /// Of the atlas: the logical tile size, the gutter, the physical tile size, the tiles in a row.
    atlas: [f32; 4],
    /// 1 over the size of the atlas, and the number of patterns.
    atlas_more: [f32; 4],
    /// Of the patterns: the tile size, the gutter, the physical size, the patterns in a row.
    patterns: [f32; 4],
    fog_color: [f32; 4],
    /// The exponential fog: its start and its divisor. The linear fog: its start and its end.
    fog: [f32; 4],
    /// x: 1 for the exponential fog past the view range.
    flags: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    /// The scattering: the position of the sun and the good weather value; the sky colour
    /// with the far scatter colour in it and 1 when the scattering is on; the two scatter
    /// colours; the three values of the height of the sun.
    sun_pos: [f32; 4],
    sky: [f32; 4],
    outscatter: [f32; 4],
    inscatter: [f32; 4],
    sun_altitude: [f32; 4],
}

const SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    eye: vec4f,
    terrain: vec4f,
    atlas: vec4f,
    atlas_more: vec4f,
    patterns: vec4f,
    fog_color: vec4f,
    fog: vec4f,
    flags: vec4f,
    sun_direction: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    sun_pos: vec4f,
    sky: vec4f,
    outscatter: vec4f,
    inscatter: vec4f,
    sun_altitude: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var material: texture_2d<f32>;
@group(0) @binding(3) var material_flags: texture_2d<f32>;
@group(0) @binding(4) var patch_albedo: texture_2d<f32>;
@group(0) @binding(5) var blend_patterns: texture_2d<f32>;
@group(0) @binding(6) var smooth_sampler: sampler;

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
    // landBias: the land goes down near the camera, so that a mesh with less detail does
    // not come through the land of the game.
    let distance = length(world - u.eye.xyz);
    world.z += -1.0 - 2.0 * max(0.0, u.eye.w - 1152.0 - distance);
    out.position = u.view_projection * vec4f(world, 1.0);
    out.normal = normal.xyz * 2.0 - 1.0;
    // The colour of a vertex has the bytes blue, green, red.
    out.color = color.bgr;
    out.local = world.xy - u.terrain.xy;
    out.world = world;
    return out;
}

fn byte_of(value: f32) -> f32 {
    return floor(value * 255.0 + 0.5);
}

fn atlas_uv(id: f32, patch_uv: vec2f) -> vec2f {
    let tile_y = floor(id / u.atlas.w);
    let origin = vec2f(id - tile_y * u.atlas.w, tile_y) * u.atlas.z;
    return (origin + u.atlas.y + patch_uv * u.atlas.x) * u.atlas_more.xy;
}

fn pattern_uv(id: f32, patch_uv: vec2f) -> vec2f {
    let rows = floor((u.atlas_more.z - 1.0) / u.patterns.w) + 1.0;
    let inverse_size = 1.0 / vec2f(u.patterns.w * u.patterns.z, rows * u.patterns.z);
    let tile_y = floor(id / u.patterns.w);
    let origin = vec2f(id - tile_y * u.patterns.w, tile_y) * u.patterns.z;
    return (origin + u.patterns.y + patch_uv * u.patterns.x) * inverse_size;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    // A patch has 512 units.
    let patch_coord = in.local / 512.0;
    let ddx_patch = dpdx(patch_coord);
    let ddy_patch = dpdy(patch_coord);
    let patch_uv = fract(patch_coord);
    let ddx_atlas = ddx_patch * u.atlas.x * u.atlas_more.xy;
    let ddy_atlas = ddy_patch * u.atlas.x * u.atlas_more.xy;
    let far_albedo = textureSample(patch_albedo, smooth_sampler, patch_coord * u.terrain.zw).rgb;

    let size = vec2i(textureDimensions(material));
    let cell = clamp(vec2i(floor(patch_coord)), vec2i(0), size - vec2i(1));
    let material_sample = textureLoad(material, cell, 0);
    let flags_sample = textureLoad(material_flags, cell, 0);
    let base_id = byte_of(material_sample.r) + 256.0 * byte_of(material_sample.g);
    let decal_id = byte_of(material_sample.b) + 256.0 * byte_of(material_sample.a);
    let pattern_id = min(byte_of(flags_sample.r), max(u.atlas_more.z - 1.0, 0.0));
    let has_decal = byte_of(flags_sample.g) % 2.0;
    let base = textureSampleGrad(atlas, smooth_sampler, atlas_uv(base_id, patch_uv), ddx_atlas, ddy_atlas).rgb;
    let decal = textureSampleGrad(atlas, smooth_sampler, atlas_uv(decal_id, patch_uv), ddx_atlas, ddy_atlas).rgb;
    let alpha = textureSampleLevel(blend_patterns, smooth_sampler, pattern_uv(pattern_id, patch_uv), 0.0).r * has_decal;
    let near_albedo = mix(base, decal, alpha);

    let footprint = max(length(ddx_patch), length(ddy_patch));
    let albedo = mix(near_albedo, far_albedo, smoothstep(0.5, 1.5, footprint));
    let light = u.ambient.rgb + u.sun_color.rgb * clamp(dot(normalize(in.normal), -u.sun_direction.xyz), 0.0, 1.0);
    let lit = albedo * in.color * light;

    return vec4f(fog_of(lit, in.world), 1.0);
}
";

/// `fogColour` and `fogApply` of `XE Common.fx` for a shader of this module: the colour of a
/// pixel at a place in the world with the fog on it. The uniforms `u` of the shader must
/// have `eye`, `fog_color`, `fog`, `flags`, `sun_pos`, `sky`, `outscatter`, `inscatter` and
/// `sun_altitude`.
const FOG_FUNCTION: &str = "
fn fog_of(lit: vec3f, world: vec3f) -> vec3f {
    // fogColour: the exponential curve past the view range of the game, and the linear fog
    // of the near cells before it.
    let to_pixel = world - u.eye.xyz;
    let distance = length(to_pixel);
    let linear = clamp((u.fog.w - distance) / max(u.fog.w - u.fog.z, 1.0), 0.0, 1.0);
    let fog_distance = (distance - u.fog.x) / max(u.fog.y, 1.0);
    let exponential = clamp(exp(-fog_distance), 0.0, 1.0);
    let fog = select(linear, exponential, u.flags.x > 0.5 && distance > u.eye.w);
    // fogColourScatter: the light that the air adds, in good weather and over the water.
    var added = u.fog_color.rgb * (1.0 - fog);
    if u.sky.w > 0.5 && u.sun_pos.w > 0.001 && u.eye.z > -1.0 {
        let direction = to_pixel / max(distance, 1.0);
        let depth = clamp(0.224 * fog_distance, 0.0, 1.0);
        let sun_cos = dot(direction, u.sun_pos.xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * u.sun_altitude.z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(u.inscatter.rgb, u.outscatter.rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (u.sun_altitude.x + 0.7 * mie);
        lost = (1.0 - exp(-depth * lost)) / lost;
        let scattered = (0.125 * mie + u.sky.rgb * rayleigh) * lost * (1.17 * air + 0.89) * u.sun_altitude.y;
        added = mix(added, scattered, u.sun_pos.w);
    }
    return fog * lit + added;
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
            settings_fog: (settings.fog.above_water_start * CELL, settings.fog.above_water_end * CELL),
            environment: None,
            loading: None,
            gpu: None,
            near_fog: None,
            statics: None,
            started: Instant::now(),
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
            // For a test, the distant land of another folder: MGE_XE_DISTANT_LAND_DIR.
            let folder: PathBuf = std::env::var_os("MGE_XE_DISTANT_LAND_DIR").map_or_else(|| root.join("Data Files").join("distantland"), PathBuf::from);
            if settings.statics || settings.render_grass {
                land.statics = Some(statics::Statics::start(&document, folder.clone()));
            }
            let (sender, receiver) = channel();
            land.loading = Some(receiver);
            thread::spawn(move || {
                let _ = sender.send(load(&folder));
            });
        }
        land
    }

    /// Makes the buffers, the textures and the pipeline when the loader thread is done.
    fn finish_load(&mut self, scene: &SceneView) {
        let Some(receiver) = self.loading.as_ref() else { return };
        let Ok(result) = receiver.try_recv() else { return };
        self.loading = None;
        match result.and_then(|loaded| make_gpu(scene, &loaded)) {
            Ok(gpu) => {
                println!("distant land: {} terrain meshes on the GPU", gpu.meshes.len());
                self.gpu = Some(gpu);
            }
            Err(problem) => {
                println!("distant land: not there: {problem}");
                self.enabled = false;
            }
        }
    }
}

impl Drop for DistantLand {
    fn drop(&mut self) {
        println!(
            "distant land: {} frames with terrain, {} mesh draws, fog distances {}",
            self.frames,
            self.meshes_drawn,
            if self.environment.is_some() { "from the game with each frame" } else { "from mgeXE.toml" }
        );
    }
}

fn load(folder: &Path) -> Result<Loaded, String> {
    let read = |name: &str| std::fs::read(folder.join(name)).map_err(|error| format!("{name}: {error}"));
    Ok(Loaded {
        terrain: load_terrain_file(&folder.join(TERRAIN_FILE_NAME)).map_err(|error| format!("{TERRAIN_FILE_NAME}: {error}"))?,
        atlas: read(TERRAIN_ATLAS_FILE_NAME)?,
        material: read(TERRAIN_MATERIAL_FILE_NAME)?,
        material_flags: read(TERRAIN_MATERIAL_FLAGS_FILE_NAME)?,
        patch_albedo: read(TERRAIN_PATCH_ALBEDO_FILE_NAME)?,
        blend_patterns: read(TERRAIN_BLEND_PATTERNS_FILE_NAME)?,
    })
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Makes a texture from the bytes of a DDS file. The files of the terrain have DXT1 pixels,
/// or 4 bytes for each pixel with red first.
///
/// A DDS file: `DDS `, a header of 124 bytes with the height at 12, the width at 16 and the
/// number of levels at 28; the four letters of a block format at 84, or the bits of a pixel
/// at 88 and the mask of red at 92.
fn dds_texture(scene: &SceneView, name: &str, dds: &[u8]) -> Result<wgpu::TextureView, String> {
    if dds.len() < 128 || &dds[0..4] != b"DDS " {
        return Err(format!("{name} is not a DDS file"));
    }
    let (height, width, levels) = (word(dds, 12), word(dds, 16), word(dds, 28).max(1));
    let block = match &dds[84..88] {
        b"DXT1" => Some(wgpu::TextureFormat::Bc1RgbaUnorm),
        b"DXT3" => Some(wgpu::TextureFormat::Bc2RgbaUnorm),
        b"DXT5" => Some(wgpu::TextureFormat::Bc3RgbaUnorm),
        _ => None,
    };
    let limit = scene.device.limits().max_texture_dimension_2d;
    if width > limit || height > limit {
        return Err(format!("{name} has the size {width} x {height}, and the GPU takes {limit} at most"));
    }
    let format = if let Some(format) = block {
        if width % 4 != 0 || height % 4 != 0 {
            return Err(format!("{name} has the size {width} x {height}, not a multiple of 4"));
        }
        format
    } else if word(dds, 88) == 32 && word(dds, 92) == 0xFF {
        wgpu::TextureFormat::Rgba8Unorm
    } else if word(dds, 88) == 32 && word(dds, 92) == 0x00FF_0000 {
        wgpu::TextureFormat::Bgra8Unorm
    } else {
        return Err(format!("{name} has a pixel format that this program does not read"));
    };
    let texture = scene.device.create_texture_with_data(
        scene.queue,
        &wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &dds[128..],
    );
    Ok(texture.create_view(&Default::default()))
}

fn make_gpu(scene: &SceneView, loaded: &Loaded) -> Result<Gpu, String> {
    let device = scene.device;
    let textures = [
        dds_texture(scene, TERRAIN_ATLAS_FILE_NAME, &loaded.atlas)?,
        dds_texture(scene, TERRAIN_MATERIAL_FILE_NAME, &loaded.material)?,
        dds_texture(scene, TERRAIN_MATERIAL_FLAGS_FILE_NAME, &loaded.material_flags)?,
        dds_texture(scene, TERRAIN_PATCH_ALBEDO_FILE_NAME, &loaded.patch_albedo)?,
        dds_texture(scene, TERRAIN_BLEND_PATTERNS_FILE_NAME, &loaded.blend_patterns)?,
    ];
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
    let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            texture_entry(1),
            texture_entry(2),
            texture_entry(3),
            texture_entry(4),
            texture_entry(5),
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }];
    for (index, view) in textures.iter().enumerate() {
        entries.push(wgpu::BindGroupEntry { binding: index as u32 + 1, resource: wgpu::BindingResource::TextureView(view) });
    }
    entries.push(wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&sampler) });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries });
    let source = format!("{SHADER}{FOG_FUNCTION}");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant land"), source: wgpu::ShaderSource::Wgsl(source.into()) });
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
        // The two sides: the winding of the file is not checked yet.
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
        terrain: [terrain.world_origin.x, terrain.world_origin.y, 1.0 / material[0].max(1) as f32, 1.0 / material[1].max(1) as f32],
        atlas: [terrain.logical_tile_size as f32, terrain.gutter_size as f32, terrain.physical_tile_size as f32, terrain.tiles_per_row.max(1) as f32],
        atlas_more: [1.0 / terrain.atlas_size.max(1) as f32, 1.0 / terrain.atlas_size.max(1) as f32, terrain.pattern_count as f32, 0.0],
        patterns: [
            terrain.pattern_tile_size as f32,
            terrain.pattern_gutter_size as f32,
            terrain.pattern_physical_size.max(1) as f32,
            terrain.patterns_per_row.max(1) as f32,
        ],
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

/// The position of the camera of a view matrix of Direct3D: minus its translation times its
/// transposed rotation.
fn eye_of(view: &[f32; 16]) -> [f32; 3] {
    let translation = [view[12], view[13], view[14]];
    core::array::from_fn(|i| -(translation[0] * view[i * 4] + translation[1] * view[i * 4 + 1] + translation[2] * view[i * 4 + 2]))
}

impl Extension for DistantLand {
    fn data(&mut self, id: u32, bytes: &[u8]) {
        if id == ENVIRONMENT_ID && bytes.len() >= size_of::<Environment>() {
            let environment: Environment = bytemuck::pod_read_unaligned(&bytes[..size_of::<Environment>()]);
            if environment.version == 3 {
                self.environment = Some(environment);
            }
        }
    }

    fn before_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>) -> bool {
        self.near_fog = None;
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
        let eye = eye_of(&scene.view);

        // The fog: from the game with this frame, or the linear fog of the settings.
        let (view_range, fog, exponential, near_fog, sun_mult, ambient_mult) = match self.environment {
            Some(e) => (
                e.near_view_range,
                [e.fog_exp_start, e.fog_exp_divisor, e.fog_near_start, e.fog_near_end],
                e.flags & 1 != 0 && e.flags & 2 == 0,
                (e.fog_near_start, e.fog_near_end),
                e.light_sun_mult,
                e.light_amb_mult,
            ),
            None => (0.0, [0.0, 1.0, self.settings_fog.0, self.settings_fog.1], false, self.settings_fog, 1.0, 1.0),
        };
        let sun = scene.sun_direction.unwrap_or([0.0, 0.0, -1.0]);
        let (sun_color, ambient) = match scene.sun_direction {
            // The ambient light of the terrain shader is that of the sun and that of the scene.
            Some(_) => (scene.sun_color.map(|c| c * sun_mult), core::array::from_fn(|i| (scene.ambient[i] + scene.sun_ambient[i]) * ambient_mult)),
            // Without a sun in the draw of the game: the colour of the terrain as it is.
            None => ([0.0; 3], [1.0; 3]),
        };
        // The far fog colour is that of the weather. The fog colour of the game is the near
        // one: MGE XE changes it for the scattering.
        let mut fog_color = scene.fog_color;
        let mut scattering = [[0.0f32; 4]; 5];
        if let Some(e) = self.environment.filter(|e| e.flags & 4 != 0) {
            fog_color = [e.fog_col_far[0], e.fog_col_far[1], e.fog_col_far[2], 1.0];
            if e.flags & 8 != 0 {
                let far = e.sky_scatter_far;
                let sky: [f32; 3] = core::array::from_fn(|i| e.sky_col[i] + (far[i] - e.sky_col[i]) * far[3]);
                let altitude = (1.0 + e.sun_pos[2]).powi(10);
                scattering = [
                    [e.sun_pos[0], e.sun_pos[1], e.sun_pos[2], e.nice_weather],
                    [sky[0], sky[1], sky[2], 1.0],
                    [e.outscatter[0], e.outscatter[1], e.outscatter[2], 0.0],
                    [e.inscatter[0], e.inscatter[1], e.inscatter[2], 0.0],
                    [
                        2.8 + 4.3 / altitude.max(1e-6),
                        (1.0 - (-1.9 * altitude).exp2()).clamp(0.0, 1.0),
                        (-4.0 * e.sun_pos[2]).exp().clamp(0.0, 1.0) * altitude.clamp(0.0, 1.0),
                        0.0,
                    ],
                ];
            }
        }
        let uniforms = Uniforms {
            view_projection,
            eye: [eye[0], eye[1], eye[2], view_range],
            terrain: gpu.terrain,
            atlas: gpu.atlas,
            atlas_more: gpu.atlas_more,
            patterns: gpu.patterns,
            fog_color,
            fog,
            flags: [exponential as u32 as f32, 0.0, 0.0, 0.0],
            sun_direction: [sun[0], sun[1], sun[2], 0.0],
            sun_color: [sun_color[0], sun_color[1], sun_color[2], 0.0],
            ambient: [ambient[0], ambient[1], ambient[2], 0.0],
            sun_pos: scattering[0],
            sky: scattering[1],
            outscatter: scattering[2],
            inscatter: scattering[3],
            sun_altitude: scattering[4],
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
        self.near_fog = Some(near_fog);
        if let Some(statics) = self.statics.as_mut() {
            let frame = statics::Frame {
                view_projection,
                eye: uniforms.eye,
                fog_color,
                fog,
                flags: uniforms.flags,
                sun_direction: uniforms.sun_direction,
                sun_color: uniforms.sun_color,
                ambient: uniforms.ambient,
                scattering,
                planes,
                fog_end: self.environment.map_or(self.settings_fog.1, |e| e.fog_end),
                time: self.started.elapsed().as_secs_f32(),
            };
            statics.draw(scene, pass, &frame);
        }
        true
    }

    /// The grass, at the end of the world scene of the game: with the projection and the
    /// depth of the game, as `renderStage1` of the Direct3D 9 path draws it.
    fn after_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>) {
        if !self.enabled || scene.interior {
            return;
        }
        let Some(statics) = self.statics.as_mut() else { return };
        let view_projection = multiply(&scene.view, &scene.projection);
        let eye = eye_of(&scene.view);
        let environment = self.environment;
        // The grass has the linear fog of the near cells, with their fog colour.
        let (fog_start, fog_end) = environment.map_or(self.settings_fog, |e| (e.fog_near_start, e.fog_near_end));
        let (sun_mult, ambient_mult) = environment.map_or((1.0, 1.0), |e| (e.light_sun_mult, e.light_amb_mult));
        let sun = scene.sun_direction.unwrap_or([0.0, 0.0, -1.0]);
        let ambient: [f32; 3] = core::array::from_fn(|i| (scene.ambient[i] + scene.sun_ambient[i]) * ambient_mult);
        let frame = statics::GrassFrame {
            view_projection,
            // The view range of the game is the end of the grass. Without it from the game,
            // the default of the game with distant land: 7168 units.
            eye: [eye[0], eye[1], eye[2], environment.map_or(7168.0, |e| e.near_view_range)],
            fog_color: scene.fog_color,
            fog: [fog_start, fog_end, 0.0, 0.0],
            sun_direction: [sun[0], sun[1], sun[2], 0.0],
            sun_color: [scene.sun_color[0] * sun_mult, scene.sun_color[1] * sun_mult, scene.sun_color[2] * sun_mult, 0.0],
            ambient: [ambient[0], ambient[1], ambient[2], 0.0],
            wind: environment.map_or([0.0; 4], |e| [e.wind[0], e.wind[1], e.time, 0.0]),
            foot: environment.map_or([0.0; 4], |e| [e.foot_pos[0], e.foot_pos[1], e.foot_pos[2], e.foot_valid as f32]),
            planes: planes(&view_projection),
        };
        statics.draw_grass(scene, pass, &frame);
    }

    /// The draws of the world of the game get the fog distances of MGE XE for the near
    /// cells, so that they go into the fog as the terrain behind them does.
    fn fog_range(&self) -> Option<(f32, f32)> {
        self.near_fog
    }
}
