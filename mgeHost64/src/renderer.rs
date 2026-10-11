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

mod blend;
mod statics;
mod water;

use distantland::mge_xe::distant_terrain::{
    TERRAIN_ATLAS_FILE_NAME, TERRAIN_BLEND_PATTERNS_FILE_NAME, TERRAIN_FILE_NAME, TERRAIN_MATERIAL_FILE_NAME, TERRAIN_MATERIAL_FLAGS_FILE_NAME,
    TERRAIN_PATCH_ALBEDO_FILE_NAME, TerrainFile, load_terrain_file,
};
use mge_config::{ConfigDocument, FILE_NAME};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::{DrawData, DrawFacts, Extension, FrameParameters, LightData, Replay, SceneView};
use water::{Water, WaterFrame};

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
    /// Version 4, for the water: the level of the water of the cell, the visibility of the
    /// sun (0 to 1), and 1 when the cell has water.
    water_level: f32,
    sun_vis: f32,
    has_water: u32,
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
    /// The same with other uniforms: for the reflection of the land in the water.
    mirror_group: wgpu::BindGroup,
    mirror_uniforms: wgpu::Buffer,
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
    /// The settings for the light for each pixel: on, only in interior cells, and the number
    /// that the radius of a light is multiplied with for its end (0: no end).
    per_pixel: (bool, bool, f32),
    /// The world of the frame is in an interior cell.
    interior: bool,
    /// The light for each pixel of this frame, or `None`.
    shade: Option<PerPixel>,
    /// The loads complete before a frame: a capture file, where each frame must be the same.
    synchronous: bool,
    blend: Option<blend::Blend>,
    statics: Option<statics::Statics>,
    /// The water, when its wave texture was found; `None` before the first frame.
    water: Option<Water>,
    water_enabled: bool,
    waves: Vec<u8>,
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
    /// x: 1 for the exponential fog past the view range. In a reflection: y the side of the
    /// water plane to keep (1 above, -1 below), z the plane (the water level less 1).
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
    if u.flags.y != 0.0 {
        // LandscapeReflVS: in a reflection the land near the water line sinks, so that the
        // edge of the water is clean; no landBias.
        world.z += -16.0 * clamp(1.0 - world.z / 16.0, 0.0, 1.0);
    } else {
        // landBias: the land goes down near the camera, so that a mesh with less detail
        // does not come through the land of the game.
        let distance = length(world - u.eye.xyz);
        world.z += -1.0 - 2.0 * max(0.0, u.eye.w - 1152.0 - distance);
    }
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

    // In a reflection, only one side of the water plane: flags.y is the side, flags.z the plane.
    if u.flags.y != 0.0 && (in.world.z - u.flags.z) * u.flags.y < 0.0 {
        discard;
    }
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
            per_pixel: (
                settings.per_pixel_lighting,
                matches!(settings.per_pixel_mode, mge_config::PerPixelMode::InteriorsOnly),
                if settings.per_pixel_light_fade { settings.per_pixel_light_fade_radius } else { 0.0 },
            ),
            interior: false,
            shade: None,
            synchronous: false,
            blend: None,
            statics: None,
            water: None,
            water_enabled: true,
            waves: Vec::new(),
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
        if land.enabled && land.water_enabled {
            // The wave texture: in the game folder (the mod of MGE XE in the file system of
            // Mod Organizer), or beside this program for a replay.
            let name = ["Data Files", "textures", "MGE", "water_NRM.dds"].iter().collect::<PathBuf>();
            let mut places = vec![root.join(&name)];
            if let Some(game) = std::env::var_os("MGE_XE_GAME_DIR") {
                places.push(PathBuf::from(game).join(&name));
            }
            if let Some(exe) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
                places.push(exe.join(&name));
            }
            land.waves = places.iter().find_map(|place| std::fs::read(place).ok()).unwrap_or_default();
            if land.waves.is_empty() {
                println!("water: the wave texture water_NRM.dds was not found; no water");
                land.water_enabled = false;
            }
        }
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
        let result = if self.synchronous { receiver.recv().ok() } else { receiver.try_recv().ok() };
        let Some(result) = result else { return };
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
        println!("blend: {} frames", self.blend.as_ref().map_or(0, |blend| blend.frames));
        println!("water: {} frames, {} reflections", self.water.as_ref().map_or(0, |w| w.frames), self.water.as_ref().map_or(0, |w| w.reflections));
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
    let mirror_uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("distant land mirror"), size: size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    entries[0] = wgpu::BindGroupEntry { binding: 0, resource: mirror_uniforms.as_entire_binding() };
    let mirror_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries });
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
        mirror_group,
        mirror_uniforms,
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

/// The light for each pixel of MGE XE, for the draws of the world of a frame.
#[derive(Clone, Copy)]
struct PerPixel {
    /// The numbers that the light of the sun and the ambient light are multiplied with: the
    /// settings of MGE XE for the weather.
    sun: f32,
    ambient: f32,
    /// A point light ends at this number times its radius. 0: a light has no end.
    fade: f32,
}

/// The light of a pixel, as the shader of MGE XE for its light for each pixel makes it
/// (`PerPixelPS` of `XE FixedFuncEmu.fx`), and its curve. `shade` packs the lights of the
/// draw for it: a directional light is the sun; for a point light `ambient.x` is a part that
/// does not depend on the normal, `attenuation.z` and `attenuation.x` are the two numbers
/// of the falloff, and `cone.x` is 1 over the distance where the light ends (0: no end).
/// Bit 2 of `counts.w`: the fog makes the colour darker and adds no fog colour (a draw that
/// adds to the picture).
const DRAW_SHADER: &str = "
fn extension_shade(in: VertexOut) -> vec4f {
    if draw.flags.x == 0u {
        return in.vertex_color;
    }
    var normal = vec3f(0.0, 0.0, 1.0);
    if draw.flags.y != 0u {
        normal = normalize(in.world_normal + vec3f(0.0, 0.0, 1e-20));
    }
    var d = vec3f(0.0);
    var a = draw.scene_ambient.rgb;
    for (var i = 0u; i < draw.light_range.y; i++) {
        let light = lights[draw.light_range.x + i];
        if light.position.w == 3.0 {
            d += light.diffuse.rgb * clamp(dot(normal, -light.direction.xyz), 0.0, 1.0);
            a += light.ambient.rgb;
        } else {
            let to_light = light.position.xyz - in.world_position;
            let distance2 = dot(to_light, to_light);
            let distance = sqrt(distance2);
            let lambert = clamp(dot(normal, to_light) / max(distance, 0.0001), 0.0, 1.0);
            var strength = 1.0 / max(light.attenuation.z * distance2 + light.attenuation.x, 0.0001);
            // The light goes to zero over the last quarter of its distance.
            var fade = clamp(4.0 * distance * light.cone.x - 3.0, 0.0, 1.0);
            fade = 1.0 - fade * fade;
            strength *= fade * fade;
            d += (lambert + light.ambient.x) * strength * light.diffuse.rgb;
        }
    }
    // The three ways of a material with a vertex colour.
    if draw.more.x == 1u {
        return vec4f(in.vertex_color.rgb * (d + a) + draw.material_emissive.rgb, in.vertex_color.a);
    }
    if draw.more.y == 1u {
        return vec4f(draw.material_diffuse.rgb * d + draw.material_ambient.rgb * a + in.vertex_color.rgb, draw.material_diffuse.a);
    }
    return vec4f(draw.material_diffuse.rgb * d + draw.material_ambient.rgb * a + draw.material_emissive.rgb, draw.material_diffuse.a);
}

// The curve of MGE XE for a colour with light for each pixel: 0 stays 0, 1 becomes 0.84,
// and 2.2 becomes 1.
fn extension_finish(color: vec3f, alpha: f32, clear: f32) -> vec4f {
    let c = clamp(color, vec3f(0.0), vec3f(2.2));
    let lit = (((0.0548303 * c - 0.189786) * c - 0.154732) * c + 1.12969) * c;
    if (draw.counts.w & 2u) != 0u {
        return vec4f(lit * clear, alpha);
    }
    return vec4f(mix(draw.fog_color.rgb, lit, clear), alpha);
}
";

/// The sky of MGE XE: `SkyVS` and `SkyPS` of `XE Mod Sky.fx` for the sky dome, with
/// `fogColourSky` and `fogColourScatter` of `XE Common.fx`. The values of `draw.extension`:
/// 0 the far fog colour; 1 the sky colour, and the nice weather number in w; 2 the sky colour
/// for the scattering, and 1 in w when the camera is above the water; 3 the direction to the
/// sun; 4 `outscatter`; 5 `inscatter`; 6 the three numbers of the altitude of the sun.
const SKY_SHADER: &str = "
// The dome of the game gets a height that puts its lower edge on the horizon.
fn extension_sky_position(position: vec3f) -> vec3f {
    return vec3f(position.xy, 50.0 * (position.z + 200.0));
}

fn extension_sky_color(direction: vec3f, pixel: vec2f) -> vec3f {
    let e = draw.extension;
    // fogColourSky: the far fog colour at the horizon goes into the sky colour above it.
    let up = 1.0 - pow(clamp(1.0 - 2.22 * clamp(direction.z - 0.075, 0.0, 1.0), 0.0, 1.0), 1.15);
    var color = mix(e[0].rgb, e[1].rgb, up);
    // fogColourScatter with a fog distance of 1 and no fog.
    if e[1].w > 0.001 && e[2].w > 0.5 {
        let sun_cos = dot(direction, e[3].xyz);
        let mie = (1.58 / (1.24 - sun_cos)) * e[6].z;
        let rayleigh = 1.0 - 0.09 * mie;
        let air = 1.33 * exp(-2.0 * clamp(direction.z, 0.0, 1.0));
        let sun_scatter = mix(e[5].rgb, e[4].rgb, 0.5 * (1.0 + sun_cos));
        var lost = air * sun_scatter * (e[6].x + 0.7 * mie);
        lost = (1.0 - exp(-lost)) / lost;
        let scattered = (0.125 * mie + e[2].rgb * rayleigh) * lost * (1.17 * air + 0.89) * e[6].y;
        color = mix(color, scattered, e[1].w);
    }
    // ditherSky: an ordered dither against bands in the sky.
    let dither = array<f32, 16>(0.001176, 0.001961, -0.001176, -0.001699, -0.000654, -0.000915, 0.000392, 0.000131, -0.000131, -0.001961, 0.000654, 0.000915, 0.001699, 0.001438, -0.000392, -0.001438);
    let at = vec2u(pixel) % 4u;
    return color + dither[at.x * 4u + at.y];
}
";

impl Extension for DistantLand {
    fn offline(&mut self) {
        self.synchronous = true;
        if let Some(statics) = self.statics.as_mut() {
            statics.synchronous = true;
        }
    }

    /// The answers for the frame: the fog distances and the blend when the terrain is in
    /// the frame, the light for each pixel by its settings, the sky with the scattering.
    fn frame(&mut self, scene: &SceneView) -> FrameParameters {
        self.interior = scene.interior;
        self.finish_load(scene);
        let land = self.enabled && !scene.interior && self.gpu.is_some();
        if land && self.water_enabled && self.water.is_none() {
            match Water::new(scene, &self.waves) {
                Ok(water) => self.water = Some(water),
                Err(problem) => {
                    println!("water: {problem}; no water");
                    self.water_enabled = false;
                }
            }
        }
        let water = land && self.water.is_some() && self.environment.is_some_and(|e| e.has_water != 0);
        self.shade = self.per_pixel();
        FrameParameters {
            fog_range: land.then(|| self.environment.map_or(self.settings_fog, |e| (e.fog_near_start, e.fog_near_end))),
            shade: self.shade.is_some(),
            sky: self.sky(),
            blend: land,
            // The water plane of the game is class 10.
            watched: if water { 1 << 10 } else { 0 },
        }
    }

    fn draw_shader(&self) -> Option<String> {
        Some(DRAW_SHADER.to_string())
    }

    /// Packs the lights of a world draw for the light of a pixel, as `buildPplDrawData` and
    /// `decodeMorrowindPointLight` of MGE XE do (`ffeshader.cpp`). The game gives a point
    /// light in one of four forms of its three falloff numbers, and each form has its own
    /// sum; one constant is for all lights of the draw, from the last light that has one.
    /// All lights of the draw: the game gives at most 8 for each node, or 32 with the patch
    /// of `expanded_light_limit`.
    fn shade(&mut self, draw: &mut DrawData, lights: &mut [LightData], facts: &DrawFacts) {
        let Some(p) = self.shade else { return };
        let mut shared_constant = 0.33f32;
        // The z axis of the camera in the space of the lights: MGE XE moves one form of
        // light 25 units along it.
        let camera_z = [draw.view[2], draw.view[6], draw.view[10]];
        // A scene with white ambient light (the sky dome, a menu) has a fixed ambient light
        // and no sun.
        let white = draw.scene_ambient == [1.0; 4];
        for light in lights.iter_mut() {
            if light.position[3] == 3.0 {
                let direction = light.direction;
                let length = (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2]).sqrt().max(1e-12);
                let (sun, amb) = if white { (0.0, 0.0) } else { (p.sun, p.ambient) };
                light.diffuse = [light.diffuse[0] * sun, light.diffuse[1] * sun, light.diffuse[2] * sun, 0.0];
                light.ambient = [light.ambient[0] * amb, light.ambient[1] * amb, light.ambient[2] * amb, 0.0];
                light.position = [0.0, 0.0, 0.0, 3.0];
                light.direction = [direction[0] / length, direction[1] / length, direction[2] / length, 0.0];
            } else {
                let falloff = light.attenuation;
                let mut color = [light.diffuse[0], light.diffuse[1], light.diffuse[2]];
                let (mut wrap, mut quadratic, mut bias) = (0.0f32, 0.0f32, 0.0f32);
                if falloff[0] > 0.0 {
                    shared_constant = falloff[0];
                    quadratic = falloff[2];
                } else if falloff[2] > 0.0 {
                    color = color.map(|c| c * shared_constant);
                    wrap = 1.0 + 1e-4 / falloff[2].sqrt();
                    quadratic = shared_constant * falloff[2];
                } else if falloff[1] == 0.100_000_01 {
                    quadratic = 5e-5;
                } else if falloff[1] > 0.0 {
                    color = [0.25 + 1e-4 / falloff[1]; 3];
                    wrap = 1.0;
                    quadratic = 0.5555 * falloff[1] * falloff[1];
                    bias = 25.0;
                }
                let radius = light.cone[2];
                let cutoff = if p.fade > 0.0 && radius > 0.0 { (p.fade * radius.max(16.0)).min(8192.0) } else { 0.0 };
                let position = light.position;
                light.diffuse = [color[0], color[1], color[2], 0.0];
                light.ambient = [wrap, 0.0, 0.0, 0.0];
                light.position = [position[0] + bias * camera_z[0], position[1] + bias * camera_z[1], position[2] + bias * camera_z[2], 1.0];
                light.attenuation = [0.0, 0.0, quadratic, 0.0];
                light.cone = [if cutoff > 0.0 { 1.0 / cutoff } else { 0.0 }, 0.0, 0.0, 0.0];
            }
        }
        for light in lights.iter_mut() {
            light.attenuation[0] = shared_constant;
        }
        draw.scene_ambient = if white {
            [1.25, 1.25, 1.25, 1.0]
        } else {
            [draw.scene_ambient[0] * p.ambient, draw.scene_ambient[1] * p.ambient, draw.scene_ambient[2] * p.ambient, draw.scene_ambient[3]]
        };
        // A draw that adds to the picture gets darker in the fog and no fog colour.
        let adds = facts.blend_enable && (facts.source_blend == 2 || facts.destination_blend == 2);
        draw.counts[3] |= 1 | if adds { 2 } else { 0 };
        // The fog of this shading is on the distance from the camera.
        draw.fog[3] = 1.0;
    }

    fn sky_shader(&self) -> Option<String> {
        Some(SKY_SHADER.to_string())
    }


    fn data(&mut self, id: u32, bytes: &[u8]) {
        if id == ENVIRONMENT_ID && bytes.len() >= size_of::<Environment>() {
            let environment: Environment = bytemuck::pod_read_unaligned(&bytes[..size_of::<Environment>()]);
            if environment.version == 4 {
                self.environment = Some(environment);
            }
        }
    }

    fn before_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, replay: &mut dyn Replay) -> bool {
        self.near_fog = None;
        self.interior = scene.interior;
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

        // The reflection in the water: the land and the sky, mirrored, in a pass of their
        // own that goes to the GPU before the frame.
        let water_frame = self.water_frame(scene);
        if let (Some(water), Some(frame)) = (self.water.as_mut(), water_frame.as_ref()) {
            let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("water reflection") });
            let (mut mirror_pass, mirrored_view, far_projection) = water.begin_reflection(scene, &mut encoder, frame);
            let mut mirror_uniforms = uniforms;
            mirror_uniforms.view_projection = multiply(&mirrored_view, &far_projection);
            // Only the side of the water plane that the mirror shows.
            mirror_uniforms.flags[1] = if frame.underwater { -1.0 } else { 1.0 };
            mirror_uniforms.flags[2] = frame.water_level - 1.0;
            scene.queue.write_buffer(&gpu.mirror_uniforms, 0, bytemuck::bytes_of(&mirror_uniforms));
            let mirror_planes = Water::reflection_planes(&mirrored_view, &far_projection);
            mirror_pass.set_pipeline(&gpu.pipeline);
            mirror_pass.set_bind_group(0, &gpu.mirror_group, &[]);
            for mesh in &gpu.meshes {
                let inside = mirror_planes.iter().all(|p| p[0] * mesh.center[0] + p[1] * mesh.center[1] + p[2] * mesh.center[2] + p[3] >= -mesh.radius);
                if !inside {
                    continue;
                }
                mirror_pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                mirror_pass.set_index_buffer(mesh.indexes.slice(..), wgpu::IndexFormat::Uint32);
                mirror_pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
            // The statics, all of them: the game draws nothing in the reflection, so there is
            // no handoff (a view range of 0, as the Direct3D 9 path has it).
            if let Some(statics) = self.statics.as_mut() {
                let mirror_frame = statics::Frame {
                    view_projection: mirror_uniforms.view_projection,
                    eye: [uniforms.eye[0], uniforms.eye[1], uniforms.eye[2], 0.0],
                    fog_color,
                    fog,
                    flags: mirror_uniforms.flags,
                    sun_direction: uniforms.sun_direction,
                    sun_color: uniforms.sun_color,
                    ambient: uniforms.ambient,
                    scattering,
                    planes: mirror_planes,
                    fog_end: self.environment.map_or(self.settings_fog.1, |e| e.fog_end),
                    time: self.started.elapsed().as_secs_f32(),
                };
                statics.draw(scene, &mut mirror_pass, &mirror_frame, 1);
            }
            water.reflect_sky(scene, &mut mirror_pass, replay, frame);
            drop(mirror_pass);
            scene.queue.submit([encoder.finish()]);
        }
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
            statics.draw(scene, pass, &frame, 0);
        }
        true
    }

    /// The grass, at the end of the world scene of the game: with the projection and the
    /// depth of the game, as `renderStage1` of the Direct3D 9 path draws it.
    fn after_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, _replay: &mut dyn Replay) {
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



    /// The draws of the game near the end of their view range go into the distant land.
    /// The water plane of MGE XE in place of the water plane of the game.
    fn draw_class(&mut self, class: u32, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, _replay: &mut dyn Replay) {
        if class != 10 {
            return;
        }
        let frame = self.water_frame(scene);
        if let (Some(water), Some(frame)) = (self.water.as_mut(), frame.as_ref()) {
            water.draw(scene, pass, frame);
        }
    }

    fn blend(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, before: &wgpu::TextureView, depth: &wgpu::TextureView) {
        // Without the view range from the game, the default of the game with distant land.
        let near_view_range = self.environment.map_or(7168.0, |e| e.near_view_range);
        self.blend.get_or_insert_with(|| blend::Blend::new(scene)).draw(scene, pass, before, depth, near_view_range);
    }

}

impl DistantLand {
    /// What the water needs from the environment of the frame, or `None` without water in
    /// the cell, without the land, or without the environment.
    fn water_frame(&self, scene: &SceneView) -> Option<WaterFrame> {
        let e = self.environment.filter(|e| e.has_water != 0 && e.flags & 4 != 0)?;
        if self.interior || self.gpu.is_none() {
            return None;
        }
        // The sun: z is negated when the sun is not visible, as the Direct3D 9 path has it.
        let mut sun_pos = e.sun_pos;
        if e.sun_vis == 0.0 {
            sun_pos[2] = -sun_pos[2];
        }
        let scattering = (e.flags & 12 == 12).then(|| {
            let far = e.sky_scatter_far;
            let sky: [f32; 3] = core::array::from_fn(|i| e.sky_col[i] + (far[i] - e.sky_col[i]) * far[3]);
            let altitude = (1.0 + e.sun_pos[2]).powi(10);
            [
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
            ]
        });
        Some(WaterFrame {
            water_level: e.water_level,
            underwater: e.flags & 2 != 0,
            time: e.time,
            sun_pos,
            sun_vis: e.sun_vis,
            sun_color: scene.sun_color.map(|c| c * e.light_sun_mult),
            sky_color: e.sky_col,
            fog_color_far: e.fog_col_far,
            nice_weather: e.nice_weather,
            scattering,
            wind: e.wind,
            fog_exp: (e.fog_exp_start, e.fog_exp_divisor),
            draw_distance: self.draw_distance,
        })
    }

    /// The sky of MGE XE is for an exterior with the scattering on and the distant land in
    /// the frame, as `renderStage0` of the Direct3D 9 path has it.
    fn sky(&mut self) -> Option<[[f32; 4]; 8]> {
        if !self.enabled || self.interior || self.gpu.is_none() {
            return None;
        }
        let e = self.environment.filter(|e| e.flags & 12 == 12)?;
        let far = e.sky_scatter_far;
        let sky: [f32; 3] = core::array::from_fn(|i| e.sky_col[i] + (far[i] - e.sky_col[i]) * far[3]);
        let altitude = (1.0 + e.sun_pos[2]).powi(10);
        Some([
            [e.fog_col_far[0], e.fog_col_far[1], e.fog_col_far[2], 0.0],
            [e.sky_col[0], e.sky_col[1], e.sky_col[2], e.nice_weather],
            [sky[0], sky[1], sky[2], if e.eye[2] > -1.0 { 1.0 } else { 0.0 }],
            [e.sun_pos[0], e.sun_pos[1], e.sun_pos[2], 0.0],
            [e.outscatter[0], e.outscatter[1], e.outscatter[2], 0.0],
            [e.inscatter[0], e.inscatter[1], e.inscatter[2], 0.0],
            [
                2.8 + 4.3 / altitude.max(1e-6),
                (1.0 - (-1.9 * altitude).exp2()).clamp(0.0, 1.0),
                (-4.0 * e.sun_pos[2]).exp().clamp(0.0, 1.0) * altitude.clamp(0.0, 1.0),
                0.0,
            ],
            [0.0; 4],
        ])
    }

    /// The light for each pixel, by the setting `distant_land.per_pixel_lighting` and its
    /// mode, with the numbers of the weather for the sun and the ambient light.
    fn per_pixel(&self) -> Option<PerPixel> {
        let (on, interiors_only, fade) = self.per_pixel;
        if !on || (interiors_only && !self.interior) {
            return None;
        }
        // In an interior the numbers are 1, as `adjustFog` of the Direct3D 9 path has them.
        let (sun, ambient) = self.environment.filter(|_| !self.interior).map_or((1.0, 1.0), |e| (e.light_sun_mult, e.light_amb_mult));
        Some(PerPixel { sun, ambient, fade })
    }
}
