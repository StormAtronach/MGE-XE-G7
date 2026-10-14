//! The wgpu renderer mode of the helper (`runtime.renderer_wgpu`).
//!
//! The nirender plugin starts the helper with `--live <name>`. The crate `nirender_wgpu`
//! draws the stream of the game in the window of the game. This module adds what MGE XE
//! draws and the game does not: the distant land, its statics and grass, the water, the
//! shadows, the sky and the post shaders (the modules of this folder).
//!
//! In a game session with `automatic_rebuild` on, the loader thread first runs the startup
//! generation of the IPC host mode, so the files match the load order; the terrain and the
//! statics load after it.
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
//! Not here yet: the quadtree and the horizon culling of the IPC host mode (a test of each
//! mesh against the view is enough for the 358 meshes of the test data).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::Instant;

mod blend;
mod caustics;
mod post;
mod post_jop;
mod shadow;
mod statics;
mod ripples;
mod tunables;
mod water;

use distantland::mge_xe::distant_terrain::{
    TERRAIN_ATLAS_FILE_NAME, TERRAIN_BLEND_PATTERNS_FILE_NAME, TERRAIN_FILE_NAME, TERRAIN_MATERIAL_FILE_NAME, TERRAIN_MATERIAL_FLAGS_FILE_NAME,
    TERRAIN_PATCH_ALBEDO_FILE_NAME, TerrainFile, load_terrain_file,
};
use mge_config::{ConfigDocument, FILE_NAME};
use crate::abi::constants::{BLUR_REFLECTIONS, DYNAMIC_RIPPLES, USE_DISTANT_LAND, USE_DISTANT_STATICS, USE_FFESHADER, USE_GRASS, USE_HDR, USE_HW_SHADER, USE_SHADOWS};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::{DrawData, DrawFacts, Extension, FrameParameters, LightData, PictureRequest, Replay, SceneView};
use water::{Water, WaterFrame};
use post::{Post, Variables};
use shadow::Shadow;

/// The side of an exterior cell of the game, in the units of the game.
const CELL: f32 = 8192.0;
/// The near plane of the projection of the distant land.
const NEAR: f32 = 64.0;
/// The number of the environment data in the stream. `Deviceless::EnvironmentId` of MGE XE.
const ENVIRONMENT_ID: u32 = 1;
/// The state of a post shader that a mod loaded: see `post::ShaderState`.
const SHADER_STATE_ID: u32 = 2;
/// The settings that a mod can change while the game runs: see `Settings`.
const SETTINGS_ID: u32 = 4;
/// A command for a parameter of the renderer, from `nirender.tune` of the plugin: see
/// `tunables::Tunables::command`.
const TUNE_ID: u32 = 5;

/// The settings record of MGE XE (`Deviceless::sendSettings`): the feature flags
/// (`Configuration.MGEFlags`), the lighting mode, the HDR reaction speed, the distances of
/// the distant land, the water numbers and the interior fog end, as a mod can change them
/// while the game runs. The renderer starts with the values of mgeXE.toml and takes the
/// record over them. 4 bytes for each member, as in mged3d8device.cpp.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Settings {
    pub version: u32,
    pub flags: u32,
    /// 0 for each cell, 1 for the interiors only.
    pub per_pixel_mode: u32,
    pub hdr_reaction_speed: f32,
    /// In cells.
    pub draw_distance: f32,
    pub near_static_end: f32,
    pub far_static_end: f32,
    pub very_far_static_end: f32,
    pub far_static_min_size: f32,
    pub very_far_static_min_size: f32,
    /// As the settings file has them, 0 to 100.
    pub water_caustics: f32,
    pub water_wave_height: f32,
    /// In cells.
    pub interior_fog_end: f32,
}

impl Settings {
    /// Reads the record; `None` for another version or a short record.
    pub fn read(bytes: &[u8]) -> Option<Settings> {
        let settings: Settings = bytemuck::pod_read_unaligned(bytes.get(..size_of::<Settings>())?);
        (settings.version == 1).then_some(settings)
    }

    /// The names of the flags that differ between two records, each with its new state.
    pub fn changed_flags(before: Option<Settings>, now: Settings) -> Vec<String> {
        const NAMES: [(u32, &str); 9] = [
            (USE_DISTANT_LAND, "distant land"),
            (USE_DISTANT_STATICS, "distant statics"),
            (USE_GRASS, "grass"),
            (USE_SHADOWS, "shadows"),
            (USE_HW_SHADER, "post shaders"),
            (USE_FFESHADER, "light for each pixel"),
            (USE_HDR, "HDR measure"),
            (DYNAMIC_RIPPLES, "dynamic waves"),
            (BLUR_REFLECTIONS, "blur of the reflections"),
        ];
        let before = before.map_or(!now.flags, |b| b.flags);
        NAMES.iter().filter(|(flag, _)| (before ^ now.flags) & flag != 0).map(|(flag, name)| format!("{name} {}", if now.flags & flag != 0 { "on" } else { "off" })).collect()
    }
}
/// The record of MGE XE for a picture file that a mod asked for (`mge.saveScreenshot`): the
/// version 1, 1 for a picture with the interface of the game, the path.
const PICTURE_ID: u32 = 3;

/// Runs the renderer with the distant land of MGE XE.
pub fn run(mut arguments: Vec<String>) {
    // The graphics API of the settings (`runtime.renderer_backend`), when the command line
    // names none: the plugin gives `--backend` only for a test.
    if !arguments.iter().any(|argument| argument == "--backend") {
        let document = ConfigDocument::open(Path::new(".").join(FILE_NAME));
        if let Some(backend) = document.settings().runtime.renderer_backend.argument() {
            arguments.extend(["--backend".to_string(), backend.to_string()]);
        }
    }
    // A replay draws the files that are there. A game session makes them again first when
    // they do not match the job of mgeXE.toml, as the IPC host mode does.
    let live = arguments.iter().any(|argument| argument == "--live");
    nirender_wgpu::run_with(arguments, Some(Box::new(DistantLand::start(Path::new("."), live))));
}

/// The startup generation of distant land, as the IPC host mode does it in `main.rs`: when
/// the output does not match the generation job of mgeXE.toml, the generator makes it again.
/// Its log goes to mgeHost64.log, as in that mode. Runs on the loader thread.
fn generate_at_start() {
    let Ok(root) = std::env::current_dir() else {
        println!("distant land: no folder of the game; no startup generation");
        return;
    };
    let _log = crate::logging::init_file_logger(root.join("mgeHost64.log")).ok();
    let mut configuration = crate::config::Configuration::load_at(&root);
    let started = Instant::now();
    let status = crate::startup_generation::ensure_distant_land_ready(&mut configuration, &root);
    println!("distant land: startup generation {status:?} in {:.1} s", started.elapsed().as_secs_f32());
}

/// The state of the game for a frame, from the 32-bit part of MGE XE. The layout is that of
/// `Deviceless::Environment` in `mged3d8device.cpp`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Environment {
    version: u32,
    /// 1: exponential fog. 2: the camera is under water. 4: an exterior, or an interior that
    /// is like one (`IntLikeExterior` of MGE XE).
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
    /// Version 2. Flag 8: the scattering of the atmosphere is on for this frame. The
    /// direction to the sun, turned at the horizon when the sun is not visible, as `sunPos`
    /// of the shaders of the Direct3D 9 path is (`DistantLand::setView`).
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
    /// Version 5: the name of the interior cell as the game has it, padded with zeros (the
    /// key of its world space in `usage.data`); all zeros in an exterior.
    interior_name: [u8; 64],
    /// Version 6, for the dynamic waves: the rain of the weather (1 for rain and snow, 1.5
    /// for a thunderstorm, below 0 for none, mixed between the two weathers of a change),
    /// and the height of the player.
    precipitation: f32,
    player_height: f32,
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
    /// The same as lines, when the device has the line mode.
    pipeline_lines: Option<wgpu::RenderPipeline>,
    /// The land in the mirror of the water: one side only.
    mirror_pipeline: wgpu::RenderPipeline,
    mirror_pipeline_lines: Option<wgpu::RenderPipeline>,
    group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    /// The same with other uniforms: for the reflection of the land in the water.
    mirror_group: wgpu::BindGroup,
    mirror_uniforms: wgpu::Buffer,
    /// The depth of the land from the sun, for the two layers of the shadow map.
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_groups: [wgpu::BindGroup; 2],
    shadow_uniforms: [wgpu::Buffer; 2],
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
    /// The startup generation of a game session: a message when it is done.
    generation: Option<Receiver<()>>,
    /// The folder of the statics to start after the startup generation.
    statics_after_generation: Option<PathBuf>,
    gpu: Option<Gpu>,
    /// The fog distances for the near cells of the frame, when the terrain is in it.
    near_fog: Option<(f32, f32)>,
    /// The settings for the light for each pixel: on, only in interior cells, and the number
    /// that the radius of a light is multiplied with for its end (0: no end).
    per_pixel: (bool, bool, f32),
    /// The world of the frame is in an interior cell.
    interior: bool,
    /// The cell of the frame is an exterior, or an interior that is like one: the fact of the
    /// environment, and an exterior without it. The sky with the scattering, the shadow map
    /// and the distant statics are for such a cell, as `IntLikeExterior(true)` of the
    /// Direct3D 9 path has them.
    like_exterior: bool,
    /// The cell of the frame has a world space of the distant land: an exterior, or an
    /// interior cell with statics in `usage.data` (`isDistantCell` of the Direct3D 9 path).
    distant_cell: bool,
    /// The light for each pixel of this frame, or `None`.
    shade: Option<PerPixel>,
    /// The loads complete before a frame: a capture file, where each frame must be the same.
    synchronous: bool,
    blend: Option<blend::Blend>,
    /// The caustics under the water; `None` before the first frame with them.
    caustics: Option<caustics::Caustics>,
    /// The setting `distant_land.water.caustics_intensity`: 0 is none.
    caustics_intensity: f32,
    /// The setting `distant_land.fog.interior_end`, in units: where the draws of the game
    /// end in an interior and under the water.
    interior_fog_end: f32,
    /// The last state of each post shader that a mod loaded, and the names whose state the
    /// post chain does not have yet.
    shader_states: Vec<post::ShaderState>,
    shader_states_new: Vec<String>,
    /// The picture files that a mod asked for and that the renderer did not take yet.
    pictures: Vec<PictureRequest>,
    /// The parameters that a person can change while the game runs (tunables.rs).
    tunables: tunables::Tunables,
    /// The last settings record of MGE XE, and the one that the renderer follows.
    settings: Option<Settings>,
    settings_applied: Option<Settings>,
    /// The draw distance and the end of the last static tier of the last log line, in
    /// cells: a mod can change them in each frame (a zoom), and the log tells a change of
    /// a quarter of a cell or more.
    settings_told_distance: (f32, f32),
    statics: Option<statics::Statics>,
    /// The shadow map, when the settings have shadows; `None` before the first frame. True
    /// when the map is made in this frame.
    shadow: Option<Shadow>,
    shadow_in_frame: bool,
    shadows_enabled: bool,
    shadow_resolution: u32,
    /// The anisotropic filter of the settings (`graphics.anisotropy`): 1 is none.
    anisotropy: u16,
    /// The post-processing chain, when the settings have shaders; `None` before the first
    /// frame with the water (it needs the wave texture).
    post: Option<Post>,
    post_enabled: bool,
    post_chain: Vec<String>,
    /// `render.hdr_reaction_time`: the seconds that the HDR measure takes to follow the picture.
    hdr_reaction: f32,
    /// The folder of the game (the current folder), for the files of the shaders.
    root: PathBuf,
    /// The water, when its wave texture was found; `None` before the first frame.
    water: Option<Water>,
    water_enabled: bool,
    /// The dynamic waves (`distant_land.water.dynamic_ripples`), their wave height
    /// (`wave_height`, 0 to 250), and their simulation; `None` before the first frame
    /// with them.
    ripples_enabled: bool,
    wave_height: f32,
    ripples: Option<ripples::Ripples>,
    /// The blur of the reflection in the water (`distant_land.water.blur_reflections`),
    /// and its kind: sharp at the waterline (`blur_contact_hardening`), or the even blur
    /// of the Direct3D 9 path.
    blur_reflections: bool,
    blur_contact: bool,
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
/// The bytes of a file under `Data Files`: in the game folder (the mods of Mod Organizer are
/// in its file system), in `MGE_XE_GAME_DIR`, or beside this program (for a replay).
fn data_file(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let name = Path::new("Data Files").join(relative);
    let mut places = vec![root.join(&name)];
    if let Some(game) = std::env::var_os("MGE_XE_GAME_DIR") {
        places.push(PathBuf::from(game).join(&name));
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        places.push(exe.join(&name));
    }
    places.iter().find_map(|place| std::fs::read(place).ok())
}

/// The casters of the shadow map: `ShadowVS` and `ShadowPS` of `XE Shadowmap.fx` for the
/// land. The uniforms are those of the land (the matrix of the layer in `view_projection`).
const LAND_SHADOW_SHADER: &str = "
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

struct Out {
    @builtin(position) position: vec4f,
    @location(0) depth: f32,
}

@vertex fn vs(@location(0) position: vec3f, @location(1) normal: vec4f, @location(2) color: vec4f) -> Out {
    var out: Out;
    var clip = u.view_projection * vec4f(position, 1.0);
    clip.z = max(0.0, clip.z);
    out.position = clip;
    out.depth = clip.z / clip.w;
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    return vec4f(32768.0 * in.depth, 0.0, 0.0, 1.0);
}
";

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
    /// `live`: a game session, which runs the startup generation when `automatic_rebuild` is
    /// on. The terrain and the statics load after it.
    pub fn start(root: &Path, live: bool) -> DistantLand {
        let document = ConfigDocument::open(root.join(FILE_NAME));
        let settings = &document.settings().distant_land;
        let mut land = DistantLand {
            enabled: settings.enabled,
            draw_distance: settings.draw_distance * CELL,
            settings_fog: (settings.fog.above_water_start * CELL, settings.fog.above_water_end * CELL),
            environment: None,
            loading: None,
            generation: None,
            statics_after_generation: None,
            gpu: None,
            near_fog: None,
            per_pixel: (
                settings.per_pixel_lighting,
                matches!(settings.per_pixel_mode, mge_config::PerPixelMode::InteriorsOnly),
                if settings.per_pixel_light_fade { settings.per_pixel_light_fade_radius } else { 0.0 },
            ),
            interior: false,
            like_exterior: true,
            distant_cell: true,
            shade: None,
            synchronous: false,
            blend: None,
            caustics: None,
            caustics_intensity: settings.water.caustics_intensity as f32,
            interior_fog_end: settings.fog.interior_end * CELL,
            shader_states: Vec::new(),
            shader_states_new: Vec::new(),
            pictures: Vec::new(),
            tunables: tunables::Tunables::new(),
            settings: None,
            settings_applied: None,
            settings_told_distance: (settings.draw_distance, settings.very_far_static_end),
            statics: None,
            shadow: None,
            shadow_in_frame: false,
            shadows_enabled: settings.shadows.enabled,
            shadow_resolution: settings.shadows.map_resolution,
            anisotropy: document.settings().graphics.anisotropy.runtime_value() as u16,
            post: None,
            root: root.to_path_buf(),
            post_enabled: document.settings().render.enable_shaders,
            post_chain: document.settings().shaders.chain.clone(),
            hdr_reaction: document.settings().render.hdr_reaction_time,
            water: None,
            water_enabled: true,
            ripples_enabled: settings.water.dynamic_ripples,
            wave_height: settings.water.wave_height as f32,
            ripples: None,
            blur_reflections: settings.water.blur_reflections,
            blur_contact: settings.water.blur_contact_hardening,
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
            land.waves = data_file(root, "textures/MGE/water_NRM.dds").unwrap_or_default();
            if land.waves.is_empty() {
                println!("water: the wave texture water_NRM.dds was not found; no water");
                land.water_enabled = false;
            }
        }
        if land.enabled {
            // For a test, the distant land of another folder: MGE_XE_DISTANT_LAND_DIR.
            let folder: PathBuf = std::env::var_os("MGE_XE_DISTANT_LAND_DIR").map_or_else(|| root.join("Data Files").join("distantland"), PathBuf::from);
            let statics_wanted = settings.statics || settings.render_grass;
            let (sender, receiver) = channel();
            land.loading = Some(receiver);
            if live && settings.automatic_rebuild {
                let (done, generation) = channel();
                land.generation = Some(generation);
                land.statics_after_generation = statics_wanted.then(|| folder.clone());
                thread::spawn(move || {
                    generate_at_start();
                    let _ = done.send(());
                    let _ = sender.send(load(&folder));
                });
            } else {
                if statics_wanted {
                    land.statics = Some(statics::Statics::start(&document, folder.clone()));
                }
                thread::spawn(move || {
                    let _ = sender.send(load(&folder));
                });
            }
        }
        land
    }

    /// Starts the statics when the startup generation is done: their files are then the
    /// new ones.
    fn finish_generation(&mut self) {
        let Some(generation) = self.generation.as_ref() else { return };
        if matches!(generation.try_recv(), Err(TryRecvError::Empty)) {
            return;
        }
        self.generation = None;
        if let Some(folder) = self.statics_after_generation.take() {
            let document = ConfigDocument::open(self.root.join(FILE_NAME));
            self.statics = Some(statics::Statics::start(&document, folder));
        }
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
        println!(
            "water: {} frames, {} reflections, {} frames with caustics",
            self.water.as_ref().map_or(0, |w| w.frames),
            self.water.as_ref().map_or(0, |w| w.reflections),
            self.caustics.as_ref().map_or(0, |c| c.frames)
        );
        if let Some(ripples) = self.ripples.as_ref() {
            println!("dynamic waves: {} frames, {} steps, {} drops", ripples.frames, ripples.steps, ripples.drops_made);
        }
        println!("shadow map: {} frames", self.shadow.as_ref().map_or(0, |s| s.frames));
        println!("post: {} frames, {} shaders run", self.post.as_ref().map_or(0, |p| p.frames), self.post.as_ref().map_or(0, |p| p.shaders_run));
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

/// The pixels of a TGA file as red, green, blue, alpha, the top row first: a true-colour
/// file of 24 or 32 bits, with or without run lengths. A mod can name such a file as the
/// texture of its post shader.
fn tga_pixels(name: &str, tga: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if tga.len() < 18 {
        return Err(format!("{name} is not a TGA file"));
    }
    let (kind, depth, top_first) = (tga[2], tga[16] as usize / 8, tga[17] & 0x20 != 0);
    let (width, height) = (u16::from_le_bytes([tga[12], tga[13]]) as usize, u16::from_le_bytes([tga[14], tga[15]]) as usize);
    if !matches!(kind, 2 | 10) || !matches!(depth, 3 | 4) || width == 0 || height == 0 {
        return Err(format!("{name} is a TGA file of a kind that this program does not read (type {kind}, {} bits)", tga[16]));
    }
    // The pixels start after the head, the identification and the colour map.
    let map = if tga[1] != 0 { u16::from_le_bytes([tga[5], tga[6]]) as usize * (tga[7] as usize).div_ceil(8) } else { 0 };
    let mut at = 18 + tga[0] as usize + map;
    let short = || format!("{name} ends before its pixels");
    let mut pixels: Vec<u8> = Vec::with_capacity(width * height * 4);
    let pixel = |bytes: &[u8]| [bytes[2], bytes[1], bytes[0], if depth == 4 { bytes[3] } else { 255 }];
    while pixels.len() < width * height * 4 {
        if kind == 2 {
            pixels.extend(pixel(tga.get(at..at + depth).ok_or_else(short)?));
            at += depth;
        } else {
            // A packet: one pixel that repeats, or a count of plain pixels.
            let head = *tga.get(at).ok_or_else(short)?;
            let count = (head & 0x7F) as usize + 1;
            at += 1;
            if head & 0x80 != 0 {
                let one = pixel(tga.get(at..at + depth).ok_or_else(short)?);
                at += depth;
                for _ in 0..count {
                    pixels.extend(one);
                }
            } else {
                for _ in 0..count {
                    pixels.extend(pixel(tga.get(at..at + depth).ok_or_else(short)?));
                    at += depth;
                }
            }
        }
    }
    pixels.truncate(width * height * 4);
    if !top_first {
        let rows: Vec<&[u8]> = pixels.chunks_exact(width * 4).rev().collect();
        pixels = rows.concat();
    }
    Ok((width as u32, height as u32, pixels))
}

/// The texture of a post shader: a DDS file, or a TGA file by its name.
fn image_texture(scene: &SceneView, name: &str, bytes: &[u8]) -> Result<wgpu::TextureView, String> {
    if !name.to_ascii_lowercase().ends_with(".tga") {
        return dds_texture(scene, name, bytes);
    }
    let (width, height, pixels) = tga_pixels(name, bytes)?;
    let texture = scene.device.create_texture_with_data(
        scene.queue,
        &wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::MipMajor,
        &pixels,
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
    let shadow_uniforms: [wgpu::Buffer; 2] = core::array::from_fn(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("distant land shadow"), size: size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
    let shadow_groups: [wgpu::BindGroup; 2] = core::array::from_fn(|i| {
        entries[0] = wgpu::BindGroupEntry { binding: 0, resource: shadow_uniforms[i].as_entire_binding() };
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries })
    });
    let source = format!("{SHADER}{FOG_FUNCTION}");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant land"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    // A terrain vertex of `terrain.bin` has 20 bytes: the position, the normal and the colour.
    let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Unorm8x4];
    let make_pipeline = |cull_mode: Option<wgpu::Face>, polygon_mode: wgpu::PolygonMode| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant land"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
        },
        // The frame has the two sides of the land. The mirror of the water has one side,
        // as the Direct3D 9 path has it (`CullMode` of the pass of the land reflection):
        // the land under the water, which the lower cut of the dynamic waves lets in, is
        // then seen from its back and is not drawn over the reflection.
        primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode, polygon_mode, ..Default::default() },
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
    let pipeline = make_pipeline(None, wgpu::PolygonMode::Fill);
    // The same as lines, for a frame in which the game draws in wireframe (see
    // `SceneView::wireframe`); a device without the line mode draws the land filled then.
    let lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
    let pipeline_lines = lines.then(|| make_pipeline(None, wgpu::PolygonMode::Line));
    // The mirror turns the winding: the front of the land is the other face there, as
    // the statics have it.
    let mirror_pipeline = make_pipeline(Some(wgpu::Face::Front), wgpu::PolygonMode::Fill);
    let mirror_pipeline_lines = lines.then(|| make_pipeline(Some(wgpu::Face::Front), wgpu::PolygonMode::Line));

    // The casters of the shadow map: the land with the depth from the sun.
    let shadow_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant land shadow"), source: wgpu::ShaderSource::Wgsl(LAND_SHADOW_SHADER.into()) });
    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant land shadow"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shadow_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
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
            module: &shadow_module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
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
        pipeline_lines,
        mirror_pipeline,
        mirror_pipeline_lines,
        group,
        uniforms,
        mirror_group,
        mirror_uniforms,
        shadow_pipeline,
        shadow_groups,
        shadow_uniforms,
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
impl Gpu {
    /// The pipeline of the land for the frame: lines when the game draws in wireframe and
    /// the device has the line mode, filled otherwise.
    fn pipeline_for(&self, scene: &SceneView) -> &wgpu::RenderPipeline {
        if scene.wireframe { self.pipeline_lines.as_ref().unwrap_or(&self.pipeline) } else { &self.pipeline }
    }

    /// The same for the land in the mirror of the water.
    fn mirror_pipeline_for(&self, scene: &SceneView) -> &wgpu::RenderPipeline {
        if scene.wireframe { self.mirror_pipeline_lines.as_ref().unwrap_or(&self.mirror_pipeline) } else { &self.mirror_pipeline }
    }
}

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
/// adds to the picture). Bit 256 with it: the camera is under water, and the light of the
/// draw goes to the colour of the water with the distance.
const DRAW_SHADER: &str = "
// The shadow of the draw at the pixel: `RenderShadowsPS` of `XE Mod Shadow.fx`. From
// `extension_shade` for `extension_finish`.
var<private> shadow_v: f32;

// shadowDeltaZ: the depth of the shadow map less that of the pixel, in the layer that has
// the pixel; a positive number when the map has nothing nearer to the sun.
fn shadow_delta(s0: vec3f, s1: vec3f) -> f32 {
    let rcp = frame.values[8].w;
    let margin = vec3f(1.0 - 8.0 * rcp, 1.0 - 8.0 * rcp, 1.0);
    if all(abs(s0) < margin) {
        let uv = vec2f(0.5 + 0.5 * s0.x, 0.5 - 0.5 * s0.y);
        return textureSample(extension_texture, extension_sampler, vec2f(uv.x * 0.5, uv.y)).r / frame.values[8].z - s0.z;
    }
    if all(abs(s1) < margin) {
        let uv = vec2f(0.5 + 0.5 * s1.x, 0.5 - 0.5 * s1.y);
        return textureSample(extension_texture, extension_sampler, vec2f(0.5 + uv.x * 0.5, uv.y)).r / frame.values[8].z - s1.z;
    }
    return 1e-6;
}

fn extension_shade(in: VertexOut) -> vec4f {
    shadow_v = 0.0;
    if (draw.counts.w & 32u) != 0u {
        // The light that the sun gives the pixel, as shadowSunEstimate: the sun of the draw
        // is in draw.extension[5] (its direction) and [7].x (its colour and visibility).
        var normal = vec3f(0.0, 0.0, 1.0);
        if draw.flags.y != 0u {
            normal = normalize(in.world_normal + vec3f(0.0, 0.0, 1e-20));
        }
        let x = clamp(dot(normal, -draw.extension[5].xyz), 0.0, 1.0) * draw.extension[7].x;
        var light = x / (frame.values[9].w + x);
        // The shadow in the space of the two layers: from the view space of the draw.
        let view_position = draw.view * vec4f(in.world_position, 1.0);
        let to0 = mat4x4f(frame.values[0], frame.values[1], frame.values[2], frame.values[3]);
        let to1 = mat4x4f(frame.values[4], frame.values[5], frame.values[6], frame.values[7]);
        var s0 = to0 * view_position;
        var s1 = to1 * view_position;
        s0 = vec4f(s0.xy, s0.z / s0.w, s0.w);
        s1 = vec4f(s1.xy, s1.z / s1.w, s1.w);
        let dz = shadow_delta(s0.xyz, s1.xyz);
        if dz < 0.0 {
            // shadowESM, and the fade at the edge of the outer layer.
            var v = (1.0 - clamp(exp(frame.values[8].x * dz + frame.values[8].y), 0.0, 1.0)) * light;
            let fade = clamp(25.0 * (1.0 - abs(s1.xy)), vec2f(0.0), vec2f(1.0));
            shadow_v = v * fade.x * fade.y;
        }
    }
    if (draw.counts.w & 16u) == 0u {
        // No light for each pixel: the colour of the vertex stage.
        return in.color;
    }
    if draw.flags.x == 0u {
        return in.vertex_color;
    }
    var normal = vec3f(0.0, 0.0, 1.0);
    if draw.flags.y != 0u {
        normal = normalize(in.world_normal + vec3f(0.0, 0.0, 1e-20));
    }
    var d = vec3f(0.0);
    var a = draw.scene_ambient.rgb;
    // The lights of the draw, or of the cluster of the pixel when the frame has clusters.
    var first = draw.light_range.x;
    var count = draw.light_range.y;
    var clustered = false;
    if draw.light_range.w != 0u {
        let view_z = (draw.view * vec4f(in.world_position, 1.0)).z;
        let cluster = cluster_lights(in.clip.xy, view_z);
        if clusters.grid.w != 0u {
            first = cluster.x;
            count = cluster.y;
            clustered = true;
        }
    }
    for (var i = 0u; i < count; i++) {
        var at = first + i;
        if clustered {
            // The slot of the light in the masks of the draws, and its index: the pixel
            // takes the light only when the draw has it (the lights of its effect state).
            let entry = cluster_indexes[at];
            let slot = entry >> 16u;
            if (draw.light_mask[slot >> 7u][(slot >> 5u) & 3u] & (1u << (slot & 31u))) == 0u {
                continue;
            }
            at = entry & 0xFFFFu;
        }
        let light = lights[at];
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

// What the water leaves of a light under water, for each colour: the light goes as the fog
// comes (`clear` 1 is no fog), and a colour that the water has less of goes first, so the
// light takes the colour of the water. A grey fog makes it darker only.
// The parameter water.light.colour (frame.values[11].x) makes the difference of the colours
// stronger. The way through the water is from the camera: the fog under water starts behind
// the camera (it is also the haze of the water around it), so its own `clear` is less than
// 1 at the camera.
fn water_transmission(clear: f32) -> vec3f {
    if draw.fog.z == 0.0 {
        return vec3f(1.0);
    }
    let distance = max(draw.fog.y - clear * (draw.fog.y - draw.fog.x), 0.0);
    let left = clamp(1.0 - distance / max(draw.fog.y, 1.0), 0.0, 1.0);
    let water = max(draw.fog_color.rgb, vec3f(1e-3));
    let rate = pow(vec3f(max(water.r, max(water.g, water.b))) / water, vec3f(max(frame.values[11].x, 1.0)));
    return pow(vec3f(left), min(rate, vec3f(16.0)));
}

// The curve of MGE XE for a colour with light for each pixel: 0 stays 0, 1 becomes 0.84,
// and 2.2 becomes 1. Then the shadow, as the overlay of the Direct3D 9 path: the colour
// times 1 - v * shadecolor, with the light in the fog and the alpha of a draw that blends.
fn extension_finish(color: vec3f, alpha: f32, clear: f32) -> vec4f {
    var out: vec4f;
    let under_water = (draw.counts.w & 258u) == 258u;
    if (draw.counts.w & 16u) != 0u {
        let c = clamp(color, vec3f(0.0), vec3f(2.2));
        let lit = (((0.0548303 * c - 0.189786) * c - 0.154732) * c + 1.12969) * c;
        if under_water {
            out = vec4f(lit * water_transmission(clear), alpha);
        } else if (draw.counts.w & 2u) != 0u {
            out = vec4f(lit * clear, alpha);
        } else {
            out = vec4f(mix(draw.fog_color.rgb, lit, clear), alpha);
        }
    } else if under_water {
        out = vec4f(color * water_transmission(clear), alpha);
    } else {
        out = vec4f(mix(draw.fog_color.rgb, color, clear), alpha);
    }
    if (draw.counts.w & 32u) != 0u && shadow_v > 0.0 {
        let fogatt = clear * clear;
        var v = shadow_v * select(clamp(4.0 * fogatt, 0.0, 1.0), fogatt, frame.values[10].y > 0.5);
        if (draw.counts.w & 64u) != 0u {
            v *= alpha;
        }
        if v >= 2.0 / 255.0 {
            out = vec4f(out.rgb * (1.0 - v * frame.values[9].xyz), out.a);
        }
    }
    return out;
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
    fn name(&self) -> String {
        format!("mgeHost64 v{}", env!("CARGO_PKG_VERSION"))
    }

    fn overlay_lines(&self) -> Vec<String> {
        self.tunables.lines()
    }

    fn anisotropy(&self) -> u16 {
        self.anisotropy
    }

    fn offline(&mut self) {
        self.synchronous = true;
        if let Some(statics) = self.statics.as_mut() {
            statics.synchronous = true;
        }
    }

    /// The answers for the frame: the fog distances and the blend when the terrain is in
    /// the frame, the light for each pixel by its settings, the sky with the scattering.
    fn frame(&mut self, scene: &SceneView) -> FrameParameters {
        self.finish_generation();
        self.finish_load(scene);
        self.apply_settings();
        self.cell_facts(scene);
        // The terrain and the water are for an exterior. The statics, the fog distances and
        // the blend are for a cell with a world space (`isDistantCell` of the Direct3D 9
        // path): an exterior, an interior that is like one, or a plain interior cell with
        // statics in the distant land data. The shadow map and the sky are for a cell that
        // is like an exterior.
        let land = self.enabled && !scene.interior && self.gpu.is_some();
        let distant = self.enabled && self.distant_cell && self.gpu.is_some();
        // The water: an exterior with its land, or an interior cell with water (`CellHasWater`
        // of MGE XE, which draws its water there too, with a flat reflection).
        let water_cell = self.enabled && self.environment.is_some_and(|e| e.has_water != 0) && (land || scene.interior);
        if water_cell && self.water_enabled && self.water.is_none() {
            match Water::new(scene, &self.waves) {
                Ok(water) => self.water = Some(water),
                Err(problem) => {
                    println!("water: {problem}; no water");
                    self.water_enabled = false;
                }
            }
        }
        let water = water_cell && self.water.is_some();
        // The dynamic waves: the simulation of the frame, before a water surface reads it.
        // It runs in a cell without water too, as on the Direct3D 9 path.
        if self.enabled && self.ripples_enabled {
            if let Some(e) = self.environment {
                let ripples = self.ripples.get_or_insert_with(|| ripples::Ripples::new(scene));
                ripples.simulate(
                    scene,
                    &ripples::Inputs {
                        time: e.time,
                        like_exterior: e.flags & 4 != 0,
                        precipitation: e.precipitation,
                        foot: (e.foot_valid != 0).then_some(e.foot_pos),
                        water_level: e.water_level,
                        player_height: e.player_height,
                    },
                );
            }
        }
        // The chain of post shaders runs in an interior too, as it does with MGE XE on
        // Direct3D 9: its wave texture is that of the water, or one of its own when the
        // frame has no water object (the first frame is in an interior).
        if self.post_enabled && self.post.is_none() {
            let waves = match self.water.as_ref() {
                Some(water) => Some(water.waves()),
                None => water::wave_texture(scene, &self.waves).ok(),
            };
            if let Some(waves) = waves {
                let root = self.root.clone();
                self.post = Some(Post::new(scene, &self.post_chain, waves, &|name| data_file(&root, name), self.hdr_reaction));
            }
        }
        // The shaders that a mod loaded or changed since the last frame.
        if let Some(post) = self.post.as_mut() {
            for name in std::mem::take(&mut self.shader_states_new) {
                if let Some(state) = self.shader_states.iter().find(|state| state.name == name) {
                    let root = self.root.clone();
                    post.set_state(scene, state, &|name| data_file(&root, name));
                }
            }
            // The results of the loads, for the game side. A replay of a capture writes no
            // file.
            if !self.synchronous {
                post.status.write(&self.root);
            }
        }
        let post = self.post_enabled && self.post.as_ref().is_some_and(|post| !post.is_empty());
        // The glare of the sun of the game (class 8) goes when a shader draws its own sun.
        let no_glare = post && self.post.as_ref().is_some_and(|post| post.no_sun_glare);
        // The shadow map: its matrices from the camera and the sun of this frame.
        let mut values = [[0.0f32; 4]; 32];
        let mut texture = None;
        self.shadow_in_frame = false;
        if distant && self.like_exterior && self.shadows_enabled {
            if self.shadow.is_none() {
                self.shadow = Some(Shadow::new(scene, self.shadow_resolution));
            }
            if let (Some(shadow), Some(e)) = (self.shadow.as_mut(), self.environment) {
                let eye = eye_of(&scene.view);
                let eye_vec = [scene.view[2], scene.view[6], scene.view[10]];
                // lightVec: against the sun when it is up; the sun light of the game when
                // it is not.
                let sun_pos = e.sun_pos;
                let light_vec = if sun_pos[2] > 0.0 { sun_pos.map(|v| -v) } else { scene.sun_direction.unwrap_or(sun_pos.map(|v| -v)) };
                shadow.set_frame(eye, eye_vec, light_vec);
                values = shadow.frame_values(e.sun_vis, &scene.view, eye[2] > -1.0);
                texture = Some(shadow.atlas_view.clone());
                self.shadow_in_frame = true;
            }
        }
        self.shade = self.per_pixel();
        values[11][0] = self.tunables.get(tunables::WATER_LIGHT_COLOUR);
        FrameParameters {
            fog_range: distant.then(|| self.environment.map_or(self.settings_fog, |e| (e.fog_near_start, e.fog_near_end))),
            shade: self.shade.is_some() || self.shadow_in_frame || self.under_water(),
            sky: self.sky(),
            blend: distant,
            // The water plane of the game is class 10; the glare of the sun is class 8.
            watched: (if water { 1 << 10 } else { 0 }) | (if no_glare { 1 << 8 } else { 0 }),
            values,
            texture,
            post,
            // The surface of the water (the plane lies at the level less 1): what blends on
            // its far side goes into the picture before the water, so the water shows it,
            // and what blends on the side of the camera goes over the water.
            class_plane: water.then(|| {
                let level = self.environment.map_or(0.0, |e| e.water_level) as f64 - 1.0;
                (10, if self.under_water() { [0.0, 0.0, -1.0, level] } else { [0.0, 0.0, 1.0, -level] })
            }),
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
        // A draw that adds to the picture (a glow, a spell, a bug) under water: its light goes
        // to the colour of the water with the distance, and the fog adds no colour to it.
        let adds = facts.blend_enable && (facts.source_blend == 2 || facts.destination_blend == 2);
        if adds && self.under_water() {
            draw.counts[3] |= 1 | 2 | 256;
        }
        let shadows = self.shadow.is_some() && self.shadow_in_frame;
        if self.shade.is_none() && !shadows {
            return;
        }
        // The sun of the draw, for the shadow: its direction and the light it gives, as
        // shadowSunEstimate takes it (with the multiplier of the per-pixel light).
        if shadows && facts.depth_write && !(facts.blend_enable && facts.destination_blend == 2) {
            if let Some(sun) = lights.iter().find(|light| light.position[3] == 3.0) {
                let d = sun.direction;
                let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-12);
                let mult = self.shade.map_or(1.0, |p| p.sun);
                let luminance = (sun.diffuse[0] * 0.36 + sun.diffuse[1] * 0.53 + sun.diffuse[2] * 0.11) * mult;
                let sun_vis = self.environment.map_or(0.0, |e| e.sun_vis);
                draw.extension[5] = [d[0] / length, d[1] / length, d[2] / length, 0.0];
                draw.extension[7] = [luminance * (0.25 + 0.75 * sun_vis), 0.0, 0.0, 0.0];
                draw.counts[3] |= 1 | 32;
                if facts.blend_enable {
                    draw.counts[3] |= 64;
                }
            }
        }
        let Some(p) = self.shade else {
            draw.counts[3] |= 1;
            return;
        };
        draw.counts[3] |= 16 | 128;
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
        draw.counts[3] |= 1 | if adds { 2 } else { 0 };
        // The fog of this shading is on the distance from the camera.
        draw.fog[3] = 1.0;
    }

    fn sky_shader(&self) -> Option<String> {
        Some(SKY_SHADER.to_string())
    }


    fn pictures(&mut self) -> Vec<PictureRequest> {
        std::mem::take(&mut self.pictures)
    }

    fn data(&mut self, id: u32, bytes: &[u8]) {
        if id == TUNE_ID {
            // A command for a parameter, as a text. It changes a value of this run only.
            if let Some(line) = core::str::from_utf8(bytes).ok().and_then(|text| self.tunables.command(text)) {
                println!("tune: {line}");
            }
            return;
        }
        // Version 4 (a capture from before the name of the cell) is the same without the
        // name: the bytes that are not there stay zero.
        if id == PICTURE_ID {
            let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            if let (Some(1), Some(with_ui), Some(path)) = (word(0), word(4), bytes.get(8..)) {
                let path = String::from_utf8_lossy(&path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())]).into_owned();
                // A replay of a capture writes no file of a mod, and a file that is there
                // stays, as MGE XE on Direct3D 9 has it.
                if self.synchronous {
                    println!("picture: {path} is not written in a replay");
                } else if Path::new(&path).exists() {
                    println!("picture: {path} is there already; not written");
                } else {
                    self.pictures.push(PictureRequest { path, with_ui: with_ui != 0 });
                }
            }
            return;
        }
        if id == SETTINGS_ID {
            if let Some(settings) = Settings::read(bytes) {
                self.settings = Some(settings);
            }
            return;
        }
        if id == SHADER_STATE_ID {
            if let Some(state) = post::ShaderState::read(bytes) {
                if !self.shader_states_new.contains(&state.name) {
                    self.shader_states_new.push(state.name.clone());
                }
                self.shader_states.retain(|known| known.name != state.name);
                self.shader_states.push(state);
            }
            return;
        }
        // Version 4 ends before the name of the interior cell and the values of version 6.
        const VERSION_4: usize = size_of::<Environment>() - 64 - 8;
        if id == ENVIRONMENT_ID && bytes.len() >= VERSION_4 {
            let mut raw = [0u8; size_of::<Environment>()];
            let length = bytes.len().min(raw.len());
            raw[..length].copy_from_slice(&bytes[..length]);
            let environment: Environment = bytemuck::pod_read_unaligned(&raw);
            if (4..=6).contains(&environment.version) {
                self.environment = Some(environment);
            }
        }
    }

    fn before_world(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, replay: &mut dyn Replay) -> bool {
        self.near_fog = None;
        self.cell_facts(scene);
        if !self.enabled || !self.distant_cell {
            // A cell without the distant land: the reflection of the water is one colour,
            // as `clearReflection` of the Direct3D 9 path has it. Under the water, the
            // colour of the horizon (the fog colour); above it, the sun and the ambient
            // light of the cell (Direct3D 9 adds the cell's sun colour to the ambient of the
            // device; the lights of the first draw of the world stand for them here).
            let frame = self.water_frame(scene);
            if let (Some(frame), Some(water)) = (frame, self.water.as_mut()) {
                let color = if frame.underwater {
                    [scene.fog_color[0], scene.fog_color[1], scene.fog_color[2]]
                } else {
                    core::array::from_fn(|i| (scene.sun_color[i] + scene.sun_ambient[i] + scene.ambient[i]).min(1.0))
                };
                water.fill_reflection(scene, color, &frame);
            }
            return false;
        }
        self.finish_load(scene);
        let Some(gpu) = self.gpu.as_ref() else { return false };

        // The projection of the game with a far plane at the draw distance of the distant
        // land. In a projection of Direct3D the two numbers of the depth are at 10 and 14.
        // The near plane is a little before that of the game, as `renderStage0` of the
        // Direct3D 9 path has it: a draw of the game then wins the depth test against the
        // same geometry among the statics (a static of the cell itself, in an interior).
        let mut projection = scene.projection;
        let near = NEAR - 1e-2;
        let far = self.draw_distance.max(NEAR * 2.0);
        projection[10] = far / (far - near);
        projection[14] = -near * far / (far - near);
        let view_projection = multiply(&scene.view, &projection);
        let eye = eye_of(&scene.view);

        // The fog: from the game with this frame, or the linear fog of the settings.
        // A plain interior cell has the linear fog of the game on the statics
        // (`StaticInteriorVS`), not the exponential curve.
        let (view_range, fog, exponential, near_fog, sun_mult, ambient_mult) = match self.environment {
            Some(e) => (
                e.near_view_range,
                [e.fog_exp_start, e.fog_exp_divisor, e.fog_near_start, e.fog_near_end],
                e.flags & 1 != 0 && e.flags & 2 == 0 && self.like_exterior,
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
        let mut drawn = 0;
        // The terrain is for an exterior; an interior cell has the statics of its own world
        // space over the clear colour.
        if !scene.interior {
            pass.set_pipeline(gpu.pipeline_for(scene));
            pass.set_bind_group(0, &gpu.group, &[]);
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
        }
        self.meshes_drawn += drawn;
        self.frames += 1;
        self.near_fog = Some(near_fog);

        // The shadow map: the land and the statics from the sun, in passes of their own
        // that go to the GPU before the frame.
        if let Some(shadow) = self.shadow.as_mut().filter(|_| self.shadow_in_frame) {
            let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shadow map") });
            let mut pass = shadow.begin(&mut encoder);
            for layer in 0..2 {
                shadow.set_layer(&mut pass, layer);
                let viewproj = shadow.viewproj[layer];
                let mut layer_uniforms = uniforms;
                layer_uniforms.view_projection = viewproj;
                layer_uniforms.flags[1] = 0.0;
                scene.queue.write_buffer(&gpu.shadow_uniforms[layer], 0, bytemuck::bytes_of(&layer_uniforms));
                let layer_planes = crate::renderer::planes(&viewproj);
                // The terrain casts a shadow in an exterior, as `renderShadowLayerGeneric`
                // of the Direct3D 9 path has it.
                if !scene.interior {
                    pass.set_pipeline(&gpu.shadow_pipeline);
                    pass.set_bind_group(0, &gpu.shadow_groups[layer], &[]);
                    for mesh in &gpu.meshes {
                        let inside = layer_planes.iter().all(|p| p[0] * mesh.center[0] + p[1] * mesh.center[1] + p[2] * mesh.center[2] + p[3] >= -mesh.radius);
                        if !inside {
                            continue;
                        }
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(mesh.indexes.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                }
                if let Some(statics) = self.statics.as_mut() {
                    let layer_frame = statics::Frame {
                        view_projection: viewproj,
                        eye: [uniforms.eye[0], uniforms.eye[1], uniforms.eye[2], 0.0],
                        fog_color,
                        fog,
                        flags: layer_uniforms.flags,
                        sun_direction: uniforms.sun_direction,
                        sun_color: uniforms.sun_color,
                        ambient: uniforms.ambient,
                        scattering,
                        planes: layer_planes,
                        fog_end: 1.0e9,
                        time: self.started.elapsed().as_secs_f32(),
                    };
                    statics.draw(scene, &mut pass, &layer_frame, 2 + layer);
                }
            }
            drop(pass);
            shadow.blur(&mut encoder);
            scene.queue.submit([encoder.finish()]);
        }

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
            // With the dynamic waves the plane can be lower by half of the wave height:
            // the cut moves to the side that the mirror does not show.
            mirror_uniforms.flags[2] = frame.water_level - 1.0 - mirror_uniforms.flags[1] * 0.5 * frame.wave_height.unwrap_or(0.0);
            scene.queue.write_buffer(&gpu.mirror_uniforms, 0, bytemuck::bytes_of(&mirror_uniforms));
            let mirror_planes = Water::reflection_planes(&mirrored_view, &far_projection);
            // The land is in the reflection of an exterior (`IsExterior` of the Direct3D 9
            // path); an interior cell reflects its statics only.
            if !scene.interior {
                mirror_pass.set_pipeline(gpu.mirror_pipeline_for(scene));
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
            water.finish_reflection(&mut encoder);
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
        // The grass of a cell with a world space, a plain interior cell among them, as
        // `renderStage1` of the Direct3D 9 path has it.
        if !self.enabled || !self.distant_cell {
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
        if let Some(shadow) = self.shadow.as_ref().filter(|shadow| shadow.debug) {
            shadow.draw_debug(pass);
        }
    }



    /// The draws of the game near the end of their view range go into the distant land.
    /// The shader chain on the picture of the frame.
    fn post(&mut self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let (Some(post), Some(e)) = (self.post.as_mut(), self.environment) else { return };
        let eye = eye_of(&scene.view);
        let rows = |m: &[f32; 16]| -> [[f32; 4]; 4] { core::array::from_fn(|r| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]]) };
        // `sun_pos` of the environment is the `sunPos` of the Direct3D 9 path: the game side
        // turned it at the horizon when the sun is not visible. The direction of the light is
        // the opposite of the direction before that turn.
        let sun_pos = e.sun_pos;
        let mut light_source = sun_pos;
        if e.sun_vis == 0.0 {
            light_source[2] = -light_source[2];
        }
        let sunvec = scene.sun_direction.unwrap_or(light_source.map(|v| -v));
        let exponential = e.flags & 1 != 0;
        let underwater = e.flags & 2 != 0;
        // An exterior, or an interior cell with the flag "behaves like exterior" of the game
        // (`Cell::isExteriorInterior`): `IntLikeExterior(true)` of MGE XE. `isInterior` of
        // a shader is its opposite.
        let like_exterior = e.flags & 4 != 0;
        let sunvis = e.sun_vis + (1.0 - e.sun_vis) * 0.333 * e.nice_weather;
        let fov = 2.0 * (1.0 / scene.projection[0].max(1e-6)).atan().to_degrees();
        let variables = Variables {
            mview: rows(&scene.view),
            mproj: rows(&scene.projection),
            eyevec: [scene.view[2], scene.view[6], scene.view[10], sunvis],
            eyepos: [eye[0], eye[1], eye[2], fov],
            sunvec: [sunvec[0], sunvec[1], sunvec[2], e.time],
            suncol: [scene.sun_color[0], scene.sun_color[1], scene.sun_color[2], if e.has_water != 0 { e.water_level } else { -1.0e9 }],
            sunamb: [scene.sun_ambient[0] + scene.ambient[0], scene.sun_ambient[1] + scene.ambient[1], scene.sun_ambient[2] + scene.ambient[2], if like_exterior { 0.0 } else { 1.0 }],
            sunpos: [sun_pos[0], sun_pos[1], sun_pos[2], if underwater { 1.0 } else { 0.0 }],
            hdr: [1.0; 4],
            fogcol: [e.fog_col_far[0], e.fog_col_far[1], e.fog_col_far[2], if exponential { e.fog_exp_start } else { e.fog_start }],
            fognearcol: [scene.fog_color[0], scene.fog_color[1], scene.fog_color[2], if exponential { e.fog_exp_divisor } else { e.fog_end }],
            fog_near: [e.fog_near_start, e.fog_near_end, 1.0 / scene.target_size.0.max(1) as f32, 1.0 / scene.target_size.1.max(1) as f32],
            switches: [if exponential { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        // The flags of the environment, as `postProcess` of the Direct3D 9 path sets them: 1
        // an interior that is not like an exterior, 2 an exterior, 4 an interior that
        // behaves as an exterior, 8 under water, 16 above, 32 the sun visible, 64 not.
        let flags = (if like_exterior { 0 } else { 1 })
            | (if scene.interior { 0 } else { 2 })
            | (if scene.interior && like_exterior { 4 } else { 0 })
            | (if underwater { 8 } else { 16 })
            | (if e.sun_vis >= 0.001 { 32 } else { 64 });
        // The depth of the distant land and the distant statics is a part of the depth frame
        // above the water only, as `DistantLand::renderDepth` of the Direct3D 9 path has
        // it. The numbers are those of the projection of `before_world`.
        let (near, far) = (NEAR - 1e-2, self.draw_distance.max(NEAR * 2.0));
        let distant = (!underwater).then_some([far / (far - near), -near * far / (far - near)]);
        post.run(scene, encoder, target, &variables, flags, distant);
    }

    /// The water plane of MGE XE in place of the water plane of the game.
    fn draw_class(&mut self, class: u32, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, _replay: &mut dyn Replay) {
        if class != 10 {
            return;
        }
        let frame = self.water_frame(scene);
        if let (Some(water), Some(frame)) = (self.water.as_mut(), frame.as_ref()) {
            water.draw(scene, pass, frame, self.ripples.as_ref());
        }
        // The debug view of the shadow map again: the water is drawn over that corner.
        if let Some(shadow) = self.shadow.as_ref().filter(|shadow| shadow.debug) {
            shadow.draw_debug(pass);
        }
    }

    fn blend(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, before: &wgpu::TextureView, depth: &wgpu::TextureView) {
        // The caustics first, so that the blend takes them out at the view range, as
        // renderStageBlend of the Direct3D 9 path has it: in an exterior, on the picture of
        // the game, with the waves of the water.
        if !scene.interior && self.caustics_intensity > 0.0 {
            let frame = self.water_frame(scene).map(|w| caustics::CausticsFrame {
                water_level: w.water_level,
                time: w.time,
                sun_direction: scene.sun_direction.unwrap_or([0.0, 0.0, -1.0]),
                sun_vis: w.sun_vis,
                fog_color_far: w.fog_color_far,
                near_view_range: self.environment.map_or(7168.0, |e| e.near_view_range),
                fog_near: self.environment.map_or(self.settings_fog, |e| (e.fog_near_start, e.fog_near_end)),
            });
            if let (Some(picture), Some(waves), Some(frame)) = (scene.picture, self.water.as_ref().map(|w| w.waves()), frame) {
                let intensity = self.caustics_intensity;
                let caustics = self.caustics.get_or_insert_with(|| caustics::Caustics::new(scene, intensity));
                caustics.intensity = intensity;
                caustics.draw(scene, pass, picture, depth, &waves, &frame);
            }
        }
        // Without the view range from the game, the default of the game with distant land.
        let near_view_range = self.environment.map_or(7168.0, |e| e.near_view_range);
        self.blend.get_or_insert_with(|| blend::Blend::new(scene)).draw(scene, pass, before, depth, near_view_range);
    }

}

impl DistantLand {
    /// The facts of the cell of the frame, from the scene and from the environment. The
    /// world space of the statics is that of the cell: the exterior, or the interior cell
    /// by its name.
    fn cell_facts(&mut self, scene: &SceneView) {
        self.interior = scene.interior;
        let environment = self.environment;
        self.like_exterior = environment.map_or(!scene.interior, |e| e.flags & 4 != 0);
        let name: &[u8] = match environment.as_ref() {
            Some(e) if scene.interior => statics::unpadded(&e.interior_name),
            _ => &[],
        };
        let has_world = self.statics.as_mut().is_some_and(|statics| statics.set_world(name));
        self.distant_cell = !scene.interior || has_world;
    }

    /// What the water needs from the environment of the frame, or `None` without water in
    /// the cell, without the land, or without the environment.
    fn water_frame(&self, scene: &SceneView) -> Option<WaterFrame> {
        let e = self.environment.filter(|e| e.has_water != 0)?;
        self.water.as_ref()?;
        // The draws of the game end at the fog end of an interior, and under the water: the
        // plane ends there too (the clip plane of `renderStageWater`).
        let underwater = e.flags & 2 != 0;
        let view_end = (self.interior || underwater).then_some(self.interior_fog_end);
        if e.flags & 4 == 0 {
            // A plain interior cell: no sun on the water (`sunPos` is straight down and
            // `sunVis` 0 on Direct3D 9), the sky colour and the far fog colour are the
            // colour of the horizon of the game (its fog colour), no scattering.
            let fog = [scene.fog_color[0], scene.fog_color[1], scene.fog_color[2]];
            return Some(WaterFrame {
                water_level: e.water_level,
                underwater,
                time: e.time,
                sun_pos: [0.0, 0.0, -1.0],
                sun_vis: 0.0,
                sun_color: scene.sun_color,
                sky_color: fog,
                fog_color_far: fog,
                nice_weather: 0.0,
                scattering: None,
                wind: e.wind,
                fog_exp: (e.fog_exp_start, e.fog_exp_divisor),
                draw_distance: self.draw_distance,
                view_end,
                wave_height: self.wave_height_of_frame(),
                blur_reflections: self.blur_reflections,
                blur_contact: self.blur_contact,
                blur_tune: self.tunables.blur(),
            });
        }
        self.gpu.as_ref()?;
        let sun_pos = e.sun_pos;
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
            underwater,
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
            view_end,
            wave_height: self.wave_height_of_frame(),
            blur_reflections: self.blur_reflections,
            blur_contact: self.blur_contact,
            blur_tune: self.tunables.blur(),
        })
    }

    /// The wave height for the water of the frame, when the dynamic waves run.
    fn wave_height_of_frame(&self) -> Option<f32> {
        (self.ripples_enabled && self.ripples.is_some()).then_some(self.wave_height)
    }

    /// The sky of MGE XE is for a cell with a world space that is like an exterior, with the
    /// scattering on and the distant land in the frame, as `renderStage0` of the Direct3D 9
    /// path has it.
    fn sky(&mut self) -> Option<[[f32; 4]; 8]> {
        if !self.enabled || !self.distant_cell || !self.like_exterior || self.gpu.is_none() {
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
    /// Takes the settings record over the values of mgeXE.toml when a new one came: the
    /// feature flags, the lighting mode, the HDR reaction speed, the draw distance and the
    /// ends of the static tiers. The distant land stays off when its data was not loaded
    /// at the start. The sizes of the static tiers are for the load only, as on Direct3D 9
    /// (`classifyStaticComponent` runs when the statics load). The dynamic waves, their
    /// wave height, the strength of the caustics and the interior fog end (where the water
    /// plane ends in an interior and under the water) are followed too.
    fn apply_settings(&mut self) {
        let Some(settings) = self.settings else { return };
        if self.settings_applied == Some(settings) {
            return;
        }
        let before = self.settings_applied;
        self.settings_applied = Some(settings);
        let on = |flag: u32| settings.flags & flag != 0;
        self.enabled = on(USE_DISTANT_LAND) && (self.gpu.is_some() || self.loading.is_some());
        self.shadows_enabled = on(USE_SHADOWS);
        self.post_enabled = on(USE_HW_SHADER);
        self.per_pixel.0 = on(USE_FFESHADER);
        self.per_pixel.1 = settings.per_pixel_mode == 1;
        self.hdr_reaction = settings.hdr_reaction_speed;
        self.ripples_enabled = on(DYNAMIC_RIPPLES);
        self.blur_reflections = on(BLUR_REFLECTIONS);
        if settings.water_wave_height.is_finite() {
            self.wave_height = settings.water_wave_height.clamp(0.0, 250.0);
        }
        if settings.water_caustics.is_finite() {
            self.caustics_intensity = settings.water_caustics.clamp(0.0, 100.0);
        }
        if settings.interior_fog_end.is_finite() && settings.interior_fog_end > 0.0 {
            self.interior_fog_end = settings.interior_fog_end * CELL;
        }
        if let Some(post) = self.post.as_mut() {
            post.set_hdr(on(USE_HDR), settings.hdr_reaction_speed);
        }
        if let Some(statics) = self.statics.as_mut() {
            statics.set_enabled(on(USE_DISTANT_STATICS), on(USE_GRASS));
        }
        // The distances, in cells in the record. A number that is not a distance (not
        // finite, or not above 0) keeps the value before.
        let distance = |cells: f32, before: f32| if cells.is_finite() && cells > 0.0 { cells * CELL } else { before };
        self.draw_distance = distance(settings.draw_distance, self.draw_distance);
        if let Some(statics) = self.statics.as_mut() {
            statics.set_distances(self.draw_distance, [settings.near_static_end, settings.far_static_end, settings.very_far_static_end].map(|cells| (cells.is_finite() && cells > 0.0).then_some(cells * CELL)));
        }
        let mut changes = Settings::changed_flags(before, settings);
        if before.map_or(true, |b| b.per_pixel_mode != settings.per_pixel_mode) {
            changes.push(format!("light for each pixel in {}", if settings.per_pixel_mode == 1 { "the interiors only" } else { "each cell" }));
        }
        if before.map_or(true, |b| b.hdr_reaction_speed != settings.hdr_reaction_speed) {
            changes.push(format!("HDR reaction {} s", settings.hdr_reaction_speed));
        }
        if before.is_some_and(|b| (b.water_caustics, b.water_wave_height) != (settings.water_caustics, settings.water_wave_height)) {
            changes.push(format!("caustics {}, wave height {}", settings.water_caustics, settings.water_wave_height));
        }
        if before.is_some_and(|b| b.interior_fog_end != settings.interior_fog_end) {
            changes.push(format!("interior fog end {:.3} cells", settings.interior_fog_end));
        }
        let told = self.settings_told_distance;
        if (settings.draw_distance - told.0).abs() >= 0.25 || (settings.very_far_static_end - told.1).abs() >= 0.25 {
            self.settings_told_distance = (settings.draw_distance, settings.very_far_static_end);
            changes.push(format!("draw distance {:.2} cells, static ends {:.2} {:.2} {:.2}", settings.draw_distance, settings.near_static_end, settings.far_static_end, settings.very_far_static_end));
        }
        if !changes.is_empty() {
            println!("settings: {}", changes.join(", "));
        }
    }

    /// The camera is under the water in this frame.
    fn under_water(&self) -> bool {
        self.environment.is_some_and(|e| e.flags & 2 != 0)
    }

    fn per_pixel(&self) -> Option<PerPixel> {
        let (on, interiors_only, fade) = self.per_pixel;
        if !on || (interiors_only && !self.interior) {
            return None;
        }
        // The numbers of the environment packet: those of the weather in an exterior and in
        // an interior that is like one, and 1 in each other interior, as `adjustFog` of the
        // Direct3D 9 path has them.
        let (sun, ambient) = self.environment.map_or((1.0, 1.0), |e| (e.light_sun_mult, e.light_amb_mult));
        Some(PerPixel { sun, ambient, fade })
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn the_settings_record_is_read() {
        let mut bytes = Vec::new();
        for word in [1u32, USE_DISTANT_LAND | USE_SHADOWS, 1] {
            bytes.extend(word.to_le_bytes());
        }
        for number in [0.5f32, 5.0, 1.0, 2.0, 3.0, 150.0, 400.0, 50.0, 30.0, 0.8] {
            bytes.extend(number.to_le_bytes());
        }
        assert_eq!(bytes.len(), size_of::<Settings>());
        let settings = Settings::read(&bytes).unwrap();
        assert_eq!(settings.flags, USE_DISTANT_LAND | USE_SHADOWS);
        assert_eq!(settings.per_pixel_mode, 1);
        assert_eq!(settings.hdr_reaction_speed, 0.5);
        assert_eq!(settings.draw_distance, 5.0);
        assert_eq!(settings.interior_fog_end, 0.8);
        assert!(Settings::read(&bytes[..40]).is_none());
        bytes[0] = 2;
        assert!(Settings::read(&bytes).is_none());
    }

    #[test]
    fn the_changed_flags_are_named() {
        let a = Settings { version: 1, flags: USE_DISTANT_LAND | USE_SHADOWS | USE_GRASS, ..bytemuck::Zeroable::zeroed() };
        let b = Settings { flags: USE_DISTANT_LAND | USE_GRASS | USE_HDR, ..a };
        assert_eq!(Settings::changed_flags(Some(a), b), vec!["shadows off".to_string(), "HDR measure on".to_string()]);
        assert_eq!(Settings::changed_flags(None, a).len(), 9);
    }
}
