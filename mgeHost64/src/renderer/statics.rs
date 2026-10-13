//! The distant statics of the wgpu renderer mode.
//!
//! The data is that of the generator: 128 files `statics/static_meshes_NNN` (the format is
//! in `distantland_formats::distant_statics`) and `statics/usage.data` with the places of the
//! statics in the world. This module reads the tables of the files, and reads the vertices
//! and the indexes of a subset when it needs them.
//!
//! There are two kinds of static, as in the Direct3D 9 path (`distantstatics.cpp`):
//!
//! - An ordinary static is one mesh for many places. All of them are on the GPU from the
//!   start; they are a small part of the data.
//! - A merged static has the statics of one exterior cell in one mesh, with one place. It is
//!   most of the data. A merged subset is on the GPU only while the camera is near it: a
//!   reader thread gets its bytes when the camera comes within the draw distance, and the
//!   renderer drops it when the camera is farther than that and one more cell.
//!
//! A merged subset has three levels of detail. Its triangles are in components, each with
//! the radius of the model that it came from. The reader puts the triangles of the largest
//! models first, so that a draw with fewer triangles has only the large models.
//!
//! The shader is `StaticExteriorVS` and `StaticPS` of `XE Mod Statics.fx`: the texture
//! coordinate of a vertex goes into a rectangle of an atlas page that the palette of the
//! subset names, the light of the sun is for each vertex, and a static goes away near the
//! camera where the game draws the real one.
//!
//! The grass is in the same files: a static of the type grass, with places in `usage.data`
//! as the others. It is never merged and has no palette. The grass of a frame is one draw
//! for each subset, with one record for each place (`rendergrass.cpp`): at most 16384
//! records, out to the view range of the game. The shader is that of `XE Mod Grass.fx`
//! without its shadows: the wind, the grass that the player pushes away, and the light of
//! the sun from the two sides.
//!
//! An interior cell with statics in `usage.data` is a world space of its own, with its
//! places and its grass; the renderer selects it by the name of the cell. In a plain
//! interior cell the statics have the linear fog of the game (`StaticInteriorVS`), given
//! by the flags of the frame. Not here yet: statics with a water flag, the groups of
//! statics that the game shows or hides (all of them are shown), the quadtree and the
//! horizon culling of the IPC host mode, and a limit of the memory on the GPU.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use distantland::mge_xe::usage_data::load_usage_data;
use mge_config::ConfigDocument;
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::SceneView;

use super::{CELL, dds_texture};

/// `STATIC_MESH_SHARD_COUNT` of the generator.
const SHARDS: usize = 128;
/// The bytes of a vertex and of a triangle in a file.
const VERTEX: usize = 20;
const TRIANGLE: usize = 6;
/// `staticHandoff`, `staticHandoffBand` and `staticHandoffCull` of `XE Mod Statics.fx`.
const HANDOFF: f32 = 1800.0;
const HANDOFF_BAND: f32 = 512.0;
const HANDOFF_CULL: f32 = 1024.0;
/// `MaxGrassElements` of the Direct3D 9 path: the grass records of a frame.
const GRASS_RECORDS: usize = 16384;
/// The bytes of a grass record: three rows of four numbers.
const GRASS_RECORD: usize = 48;
/// The work of one frame for subsets that arrive from the reader thread.
const ARRIVALS_IN_FRAME: usize = 24;
const ARRIVAL_BYTES_IN_FRAME: usize = 16 << 20;

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn long(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(core::array::from_fn(|i| bytes[at + i]))
}

fn float(bytes: &[u8], at: usize) -> f32 {
    f32::from_bits(word(bytes, at))
}

/// One part of a merged subset: triangles of one source model.
#[derive(Clone, Copy)]
struct Component {
    first_triangle: u32,
    triangle_count: u32,
    /// 0 near, 1 far, 2 very far: the trees of the Direct3D 9 path.
    tier: u8,
}

struct Subset {
    shard: u16,
    vertex_offset: u64,
    index_offset: u64,
    vertex_count: u32,
    triangle_count: u32,
    has_alpha: bool,
    animate_uv: bool,
    water: bool,
    /// The sphere of the subset in the space of its model: the centre and the radius.
    sphere: [f32; 4],
    texture: u32,
    /// The place of its first palette entry in the palette buffer.
    palette: u32,
    components: Vec<Component>,
}

struct Static {
    kind: u32,
    radius: f32,
    center: [f32; 3],
    first_subset: u32,
    subset_count: u32,
}

/// A subset of a static at one place in the world.
struct Instance {
    subset: u32,
    /// The world matrix, row after row, as Direct3D has it.
    world: [f32; 16],
    center: [f32; 3],
    radius: f32,
    /// 0 near, 1 far, 2 very far.
    tier: u8,
}

/// A place of a grass static.
struct GrassPlace {
    first_subset: u32,
    subset_count: u32,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: f32,
    /// A sphere around the position that has all of the model.
    radius: f32,
}

/// The places of one world space: the exterior, or one interior cell.
#[derive(Default)]
struct World {
    /// The instances by the cell of their centre.
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// The grass places by the cell of their position.
    grass_cells: HashMap<(i32, i32), Vec<u32>>,
}

/// All that the files say without their vertices and indexes.
struct Catalog {
    folder: PathBuf,
    subsets: Vec<Subset>,
    instances: Vec<Instance>,
    /// The world spaces by their name: the exterior has the empty name, an interior cell has
    /// its name as the game has it.
    worlds: HashMap<Vec<u8>, World>,
    textures: Vec<String>,
    palettes: Vec<[f32; 4]>,
    /// The one place of each merged subset.
    merged_place: HashMap<u32, u32>,
    grass: Vec<GrassPlace>,
    ordinary_bytes: u64,
    merged_bytes: u64,
}

/// The settings of MGE XE for the statics, in the units of the game.
#[derive(Clone, Copy)]
struct Settings {
    statics: bool,
    grass: bool,
    band_ends: [f32; 3],
    far_min_size: f32,
    very_far_min_size: f32,
    draw_distance: f32,
}

/// The tier of a model with this radius and this type of the generator: 1, 2 and 3 are fixed
/// tiers, 6 is a building (its radius counts two times).
fn tier_of(kind: u32, radius: f32, settings: &Settings) -> Option<u8> {
    match kind {
        1 => Some(0),
        2 => Some(1),
        3 => Some(2),
        4 => None,
        _ => {
            let radius = if kind == 6 { radius * 2.0 } else { radius };
            Some(if radius <= settings.far_min_size {
                0
            } else if radius <= settings.very_far_min_size {
                1
            } else {
                2
            })
        }
    }
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

/// The world matrix of a place of `usage.data`: scale, then the three rotations with the
/// angles negated, then the position (`read_distant_statics` of the IPC host mode).
fn world_matrix(position: [f32; 3], rotation: [f32; 3], scale: f32) -> [f32; 16] {
    let (sx, cx) = (-rotation[0]).sin_cos();
    let (sy, cy) = (-rotation[1]).sin_cos();
    let (sz, cz) = (-rotation[2]).sin_cos();
    let scaling = [scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, 1.0];
    let about_z = [cz, sz, 0.0, 0.0, -sz, cz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let about_y = [cy, 0.0, -sy, 0.0, 0.0, 1.0, 0.0, 0.0, sy, 0.0, cy, 0.0, 0.0, 0.0, 0.0, 1.0];
    let about_x = [1.0, 0.0, 0.0, 0.0, 0.0, cx, sx, 0.0, 0.0, -sx, cx, 0.0, 0.0, 0.0, 0.0, 1.0];
    let mut world = multiply(&multiply(&multiply(&scaling, &about_z), &about_y), &about_x);
    world[12] = position[0];
    world[13] = position[1];
    world[14] = position[2];
    world
}

/// A point of a model at a place: `point * world`.
fn transform(world: &[f32; 16], point: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|i| point[0] * world[i] + point[1] * world[4 + i] + point[2] * world[8 + i] + world[12 + i])
}

/// Reads the tables of the 128 files and the places of `usage.data`.
fn read_catalog(folder: &Path, settings: &Settings) -> Result<Catalog, String> {
    let statics_folder = folder.join("statics");
    let mut statics: Vec<Static> = Vec::new();
    let mut subsets: Vec<Subset> = Vec::new();
    let mut textures: Vec<String> = Vec::new();
    let mut texture_ids: HashMap<String, u32> = HashMap::new();
    // The first entry is for a subset without a palette: all of the texture.
    let mut palettes: Vec<[f32; 4]> = vec![[0.0, 1.0, 0.0, 1.0]];
    let (mut ordinary_bytes, mut merged_bytes) = (0u64, 0u64);

    for shard in 0..SHARDS {
        let path = statics_folder.join(format!("static_meshes_{shard:03}"));
        let file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut header = [0u8; 160];
        file.seek_read(&mut header, 0).map_err(|error| format!("{}: {error}", path.display()))?;
        if &header[0..8] != b"XESTAT06" || word(&header, 8) != 6 || word(&header, 16) != 52 || word(&header, 20) != 168 || word(&header, 24) != VERTEX as u32 {
            return Err(format!("{} is not a statics file of version 6", path.display()));
        }
        let geometry = long(&header, 88) as usize;
        let mut tables = vec![0u8; geometry];
        file.seek_read(&mut tables, 0).map_err(|error| format!("{}: {error}", path.display()))?;
        let (static_count, subset_count) = (word(&header, 32) as usize, word(&header, 36) as usize);
        let (static_table, subset_table) = (long(&header, 40) as usize, long(&header, 56) as usize);
        let (component_table, palette_table) = (long(&header, 112) as usize, long(&header, 136) as usize);
        if subset_table + subset_count * 168 > geometry || static_table + static_count * 52 > geometry {
            return Err(format!("{} has tables past its geometry", path.display()));
        }
        let subset_base = subsets.len() as u32;

        for i in 0..subset_count {
            let r = &tables[subset_table + i * 168..];
            let (texture_at, texture_length) = (long(r, 40) as usize, word(r, 76) as usize);
            let name = String::from_utf8_lossy(tables.get(texture_at..texture_at + texture_length).unwrap_or(&[])).into_owned();
            let texture = *texture_ids.entry(name.clone()).or_insert_with(|| {
                textures.push(name);
                textures.len() as u32 - 1
            });
            let flags = word(r, 72);
            let (vertex_count, triangle_count) = (word(r, 64), word(r, 68));
            let (first_component, component_count) = (word(r, 136) as usize, word(r, 140) as usize);
            let (first_palette, palette_count) = (word(r, 144) as usize, word(r, 148) as usize);
            let palette = if palette_count == 0 {
                0
            } else {
                let at = palettes.len() as u32;
                for p in 0..palette_count {
                    let entry = palette_table + (first_palette + p) * 16;
                    palettes.push(core::array::from_fn(|lane| float(&tables, entry + lane * 4)));
                }
                at
            };
            let mut components = Vec::with_capacity(component_count);
            for c in 0..component_count {
                let entry = &tables[component_table + (first_component + c) * 16..];
                let tier = tier_of(entry[12] as u32, float(entry, 8), settings).unwrap_or(0);
                components.push(Component { first_triangle: word(entry, 0), triangle_count: word(entry, 4), tier });
            }
            let bytes = vertex_count as u64 * VERTEX as u64 + triangle_count as u64 * TRIANGLE as u64;
            if components.is_empty() {
                ordinary_bytes += bytes;
            } else {
                merged_bytes += bytes;
            }
            subsets.push(Subset {
                shard: shard as u16,
                vertex_offset: long(r, 48),
                index_offset: long(r, 56),
                vertex_count,
                triangle_count,
                has_alpha: flags & 1 != 0,
                animate_uv: flags & 2 != 0,
                water: flags & 0x1C != 0,
                sphere: [float(r, 4), float(r, 8), float(r, 12), float(r, 0)],
                texture,
                palette,
                components,
            });
        }
        for i in 0..static_count {
            let r = &tables[static_table + i * 52..];
            statics.push(Static {
                kind: word(r, 0),
                radius: float(r, 4),
                center: [float(r, 8), float(r, 12), float(r, 16)],
                first_subset: subset_base + word(r, 44),
                subset_count: word(r, 48),
            });
        }
    }

    // The places. `usage.data` has the static of a place as its number in all the files, in
    // their order.
    let usage = load_usage_data(&statics_folder.join("usage.data")).map_err(|error| format!("usage.data: {error}"))?;
    if usage.num_meshes as usize != statics.len() {
        return Err(format!("usage.data is for {} statics and the files have {}", usage.num_meshes, statics.len()));
    }
    let mut instances: Vec<Instance> = Vec::new();
    let mut worlds: HashMap<Vec<u8>, World> = HashMap::new();
    let mut merged_place: HashMap<u32, u32> = HashMap::new();
    let mut grass: Vec<GrassPlace> = Vec::new();
    // The places of the exterior, then those of each interior cell that has statics.
    let exterior = (&[][..], &usage.exterior);
    let interiors = usage.interiors.iter().map(|interior| (unpadded(&interior.name), &interior.references));
    for (name, places) in core::iter::once(exterior).chain(interiors) {
        let world_space = worlds.entry(name.to_vec()).or_default();
        for place in places {
            let Some(model) = statics.get(place.id as usize) else { continue };
            if model.kind == 4 {
                // Grass: one record for the place. Its world matrix is made when it is drawn.
                if settings.grass {
                    let center = (model.center[0].powi(2) + model.center[1].powi(2) + model.center[2].powi(2)).sqrt();
                    let cell = ((place.location.x / CELL).floor() as i32, (place.location.y / CELL).floor() as i32);
                    world_space.grass_cells.entry(cell).or_default().push(grass.len() as u32);
                    grass.push(GrassPlace {
                        first_subset: model.first_subset,
                        subset_count: model.subset_count,
                        position: [place.location.x, place.location.y, place.location.z],
                        rotation: [place.rotation.x, place.rotation.y, place.rotation.z],
                        scale: place.scale,
                        radius: (center + model.radius) * place.scale,
                    });
                }
                continue;
            }
            if !settings.statics {
                continue;
            }
            // A building counts as one object: each of its subsets has the bounds of the model.
            let Some(tier) = tier_of(model.kind, model.radius * place.scale, settings) else { continue };
            let world = world_matrix([place.location.x, place.location.y, place.location.z], [place.rotation.x, place.rotation.y, place.rotation.z], place.scale);
            for subset in model.first_subset..model.first_subset + model.subset_count {
                let Some(s) = subsets.get(subset as usize) else { continue };
                if s.water || s.vertex_count == 0 || s.triangle_count == 0 {
                    continue;
                }
                let (center, radius) = if model.kind == 6 {
                    (transform(&world, model.center), model.radius * place.scale)
                } else {
                    (transform(&world, [s.sphere[0], s.sphere[1], s.sphere[2]]), s.sphere[3] * place.scale)
                };
                let cell = ((center[0] / CELL).floor() as i32, (center[1] / CELL).floor() as i32);
                world_space.cells.entry(cell).or_default().push(instances.len() as u32);
                if !s.components.is_empty() {
                    merged_place.insert(subset, instances.len() as u32);
                }
                instances.push(Instance { subset, world, center, radius, tier });
            }
        }
    }
    Ok(Catalog { folder: folder.to_path_buf(), subsets, instances, worlds, textures, palettes, merged_place, grass, ordinary_bytes, merged_bytes })
}

/// A name of `usage.data` or of the environment without the zeros that pad it.
pub fn unpadded(name: &[u8]) -> &[u8] {
    &name[..name.iter().rposition(|&byte| byte != 0).map_or(0, |last| last + 1)]
}

/// The bytes of a subset, from the reader thread.
struct Arrival {
    subset: u32,
    vertices: Vec<u8>,
    indexes: Vec<u8>,
    /// The triangles of the three levels of detail: all, far, very far.
    faces: [u32; 3],
}

/// Reads the vertices and the indexes of the subsets that the renderer asks for. The indexes
/// of a merged subset come out with the triangles of the very far tier first, then those of
/// the far tier, then the rest.
fn reader(catalog: Arc<Catalog>, requests: Receiver<u32>, arrivals: Sender<Arrival>) {
    let mut files: HashMap<u16, File> = HashMap::new();
    for subset in requests {
        let s = &catalog.subsets[subset as usize];
        let file = match files.entry(s.shard) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let path = catalog.folder.join("statics").join(format!("static_meshes_{:03}", s.shard));
                let Ok(file) = File::open(path) else { continue };
                entry.insert(file)
            }
        };
        let mut vertices = vec![0u8; s.vertex_count as usize * VERTEX];
        let mut source = vec![0u8; s.triangle_count as usize * TRIANGLE];
        if file.seek_read(&mut vertices, s.vertex_offset).is_err() || file.seek_read(&mut source, s.index_offset).is_err() {
            continue;
        }
        let mut faces = [s.triangle_count; 3];
        let indexes = if s.components.is_empty() {
            source
        } else {
            let mut ordered = Vec::with_capacity(source.len());
            let mut counts = [0u32; 3];
            for tier in [2u8, 1, 0] {
                for component in s.components.iter().filter(|c| c.tier == tier) {
                    let from = component.first_triangle as usize * TRIANGLE;
                    let to = from + component.triangle_count as usize * TRIANGLE;
                    if let Some(bytes) = source.get(from..to) {
                        ordered.extend_from_slice(bytes);
                        counts[tier as usize] += component.triangle_count;
                    }
                }
            }
            faces = [counts[0] + counts[1] + counts[2], counts[2] + counts[1], counts[2]];
            ordered
        };
        if arrivals.send(Arrival { subset, vertices, indexes, faces }).is_err() {
            return;
        }
    }
}

struct Resident {
    vertices: wgpu::Buffer,
    indexes: wgpu::Buffer,
    faces: [u32; 3],
    bytes: u64,
}

/// What the loader thread gives to the renderer: the catalog, and the bytes of the texture
/// files in the order of the catalog (`None` for a file that is not there).
struct Loaded {
    catalog: Arc<Catalog>,
    textures: Vec<Option<Vec<u8>>>,
}

/// The buffers of one draw of the statics in a frame: the main view, and the reflection in
/// the water. A write of a buffer is before all commands of the frame, so each draw of a
/// frame has its own.
struct BufferSet {
    group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_capacity: usize,
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    /// The same with the other side culled: for the reflection, where the mirror turns the
    /// winding of each triangle.
    mirror_pipeline: wgpu::RenderPipeline,
    /// The two as lines, when the device has the line mode: for a frame in which the game
    /// draws in wireframe (`SceneView::wireframe`).
    pipeline_lines: Option<wgpu::RenderPipeline>,
    mirror_pipeline_lines: Option<wgpu::RenderPipeline>,
    /// The depth from the sun into the shadow map.
    shadow_pipeline: wgpu::RenderPipeline,
    /// The main view, the reflection, and the two layers of the shadow map.
    sets: [BufferSet; 4],
    textures: Vec<wgpu::BindGroup>,
    grass_pipeline: wgpu::RenderPipeline,
    grass_pipeline_lines: Option<wgpu::RenderPipeline>,
    grass_group: wgpu::BindGroup,
    grass_uniforms: wgpu::Buffer,
    grass_records: wgpu::Buffer,
}

/// The values of a frame for the grass.
#[derive(Clone, Copy)]
pub struct GrassFrame {
    /// The view and the projection of the game.
    pub view_projection: [f32; 16],
    /// The position of the camera, and the view range of the game: the end of the grass.
    pub eye: [f32; 4],
    /// The fog colour of the near cells, and the start and the end of their linear fog.
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_color: [f32; 4],
    pub ambient: [f32; 4],
    /// The smooth wind, and the time of the game in z.
    pub wind: [f32; 4],
    /// The position of the player, and 1 in w when it is known.
    pub foot: [f32; 4],
    pub planes: [[f32; 4]; 6],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GrassUniforms {
    view_projection: [f32; 16],
    eye: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    wind: [f32; 4],
    foot: [f32; 4],
}

const GRASS_SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    eye: vec4f,
    fog_color: vec4f,
    fog: vec4f,
    sun_direction: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    wind: vec4f,
    foot: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var base_sampler: sampler;
@group(1) @binding(0) var base_texture: texture_2d<f32>;

struct In {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    // The world matrix of the place: its three columns that a position uses.
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
    @location(1) coordinate: vec2f,
    @location(2) fog: f32,
}

// grassDisplacement: the wind in four waves, and the grass that the player pushes away. The
// height of a vertex in its model is the weight.
fn displacement(world: vec3f, height: f32) -> vec2f {
    let time = u.wind.z;
    let speed = length(u.wind.xy);
    let displace = 2.0 * u.wind.xy + 0.1;
    var harmonics = vec2f(0.0);
    harmonics += (1.0 - 0.10 * speed) * sin(1.0 * time + world.xy / 1104.0);
    harmonics += (1.0 - 0.04 * speed) * cos(2.0 * time + world.xy / 751.0);
    harmonics += (1.0 + 0.14 * speed) * sin(3.0 * time + world.xy / 526.0);
    harmonics += (1.0 + 0.28 * speed) * sin(5.0 * time + world.xy / 209.0);
    var stomp = vec2f(0.0);
    if u.foot.w > 0.5 {
        var d = length(world.xy - u.foot.xy);
        let above = 0.06 * max(0.0, u.foot.z - world.z - 60.0);
        d += above * above;
        if d < 150.0 {
            stomp = (60.0 / max(d, 0.001) - 0.4) * (world.xy - u.foot.xy);
        }
    }
    return clamp(0.02 * height, 0.0, 1.0) * (harmonics * displace + stomp);
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    let model = vec4f(in.position.xyz, 1.0);
    var world = vec3f(dot(model, in.world0), dot(model, in.world1), dot(model, in.world2));
    let moved = displacement(world, in.position.z);
    world = vec3f(world.xy + moved, world.z);
    out.position = u.view_projection * vec4f(world, 1.0);
    // The normal goes through the world matrix and is not made of length 1 again.
    let n = vec4f(normalize(in.normal.xyz * 2.0 - 1.0), 0.0);
    let normal = vec3f(dot(n, in.world0), dot(n, in.world1), dot(n, in.world2));
    let to_vertex = world - u.eye.xyz;
    // The light of the sun for the side that the camera sees; the other side gets a part.
    var lambert = dot(normal, -u.sun_direction.xyz) * -sign(dot(to_vertex, normal));
    if lambert < 0.0 {
        lambert *= -0.3;
    }
    out.color = u.sun_color.rgb * lambert + u.ambient.rgb;
    out.coordinate = in.coordinate;
    // fogMWScalar: the linear fog of the near cells.
    out.fog = clamp((u.fog.y - length(to_vertex)) / max(u.fog.y - u.fog.x, 1.0), 0.0, 1.0);
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let texel = textureSample(base_texture, base_sampler, in.coordinate);
    if texel.a < 128.0 / 255.0 {
        discard;
    }
    return vec4f(mix(u.fog_color.rgb, texel.rgb * in.color, in.fog), 1.0);
}
";

/// The casters of the shadow map: `StaticShadowVS` and `StaticShadowPS` of `XE Shadowmap.fx`.
/// The uniforms are those of the statics (the matrix of the layer in `view_projection`).
const SHADOW_SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    eye: vec4f,
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
    handoff: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> palettes: array<vec4f>;
@group(0) @binding(2) var base_sampler: sampler;
@group(1) @binding(0) var base_texture: texture_2d<f32>;

struct In {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
    @location(7) world3: vec4f,
    @location(8) subset: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) coordinate: vec2f,
    @location(1) depth: f32,
    @location(2) @interpolate(flat) bounds: vec4f,
    @location(3) @interpolate(flat) alpha_test: f32,
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    var clip = u.view_projection * vec4f(world, 1.0);
    // What is nearer to the sun than the near plane stays, at the near plane.
    clip.z = max(0.0, clip.z);
    out.position = clip;
    out.depth = clip.z / clip.w;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * u.handoff.x);
    }
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    if in.alpha_test > 0.5 {
        let scale = in.bounds.yw - in.bounds.zx;
        let atlas = in.bounds.zx + fract(in.coordinate) * scale;
        let texel = textureSampleGrad(base_texture, base_sampler, atlas, dpdx(in.coordinate) * scale, dpdy(in.coordinate) * scale);
        if texel.a < 180.0 / 255.0 {
            discard;
        }
    }
    return vec4f(32768.0 * in.depth, 0.0, 0.0, 1.0);
}
";

/// The three columns of a world matrix of Direct3D that a position uses, as a grass record.
fn grass_record(world: &[f32; 16]) -> [f32; 12] {
    [world[0], world[4], world[8], world[12], world[1], world[5], world[9], world[13], world[2], world[6], world[10], world[14]]
}

/// The values of a frame that the terrain and the statics have in common. The names are
/// those of the uniforms of the shaders.
#[derive(Clone, Copy)]
pub struct Frame {
    pub view_projection: [f32; 16],
    /// The position of the camera, and the view range of the game.
    pub eye: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
    pub flags: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_color: [f32; 4],
    pub ambient: [f32; 4],
    pub scattering: [[f32; 4]; 5],
    pub planes: [[f32; 4]; 6],
    /// The end of the fog of the frame: nothing is drawn past it.
    pub fog_end: f32,
    /// Seconds, for a texture that moves.
    pub time: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_projection: [f32; 16],
    eye: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    flags: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    sun_pos: [f32; 4],
    sky: [f32; 4],
    outscatter: [f32; 4],
    inscatter: [f32; 4],
    sun_altitude: [f32; 4],
    /// x: the time. y: the distance where a static is complete. z: the width of the band
    /// before it. w: the distance before the band where a vertex goes away.
    handoff: [f32; 4],
}

const SHADER: &str = "
struct Uniforms {
    view_projection: mat4x4f,
    eye: vec4f,
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
    handoff: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> palettes: array<vec4f>;
@group(0) @binding(2) var base_sampler: sampler;
@group(1) @binding(0) var base_texture: texture_2d<f32>;

struct In {
    // The position of a vertex, and the number of its palette entry.
    @location(0) position: vec4f,
    // The normal, and the light that the material gives off.
    @location(1) normal: vec4f,
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    // The world matrix of the place, row after row.
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
    @location(7) world3: vec4f,
    // x: the first palette entry of the subset. y: 1 for an alpha test. z: 1 for a texture
    // that moves.
    @location(8) subset: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
    @location(1) coordinate: vec2f,
    @location(2) world: vec3f,
    @location(3) @interpolate(flat) bounds: vec4f,
    @location(4) @interpolate(flat) alpha_test: f32,
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    // A position is a row vector in Direct3D: position * world.
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    out.position = u.view_projection * vec4f(world, 1.0);
    // Near the camera the game draws the real static. A vertex before the band goes away.
    if length(world - u.eye.xyz) < u.handoff.y - u.handoff.z - u.handoff.w {
        out.position = vec4f(0.0, 0.0, -10000.0, 0.0);
    }
    // lightStaticVert: the normal goes through the world matrix and is not made of length 1
    // again, as in the shader of MGE XE.
    let n = normalize(in.normal.xyz * 2.0 - 1.0);
    let normal = n.x * in.world0.xyz + n.y * in.world1.xyz + n.z * in.world2.xyz;
    let light = u.sun_color.rgb * clamp(dot(normal, -u.sun_direction.xyz), 0.0, 1.0) + u.ambient.rgb + in.normal.w;
    // The colour of a vertex has the bytes blue, green, red.
    out.color = in.color.bgr * light;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * u.handoff.x);
    }
    out.world = world;
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    // The palette entry is min v, max u, min u, max v of a rectangle of the atlas page.
    let scale = in.bounds.yw - in.bounds.zx;
    let dx = dpdx(in.coordinate) * scale;
    let dy = dpdy(in.coordinate) * scale;
    let atlas = in.bounds.zx + fract(in.coordinate) * scale;
    let texel = textureSampleGrad(base_texture, base_sampler, atlas, dx, dy);
    // In a reflection, only one side of the water plane: flags.y is the side, flags.z the plane.
    if u.flags.y != 0.0 && (in.world.z - u.flags.z) * u.flags.y < 0.0 {
        discard;
    }
    let range = length(in.world - u.eye.xyz);
    let dissolve = clamp((range - (u.handoff.y - u.handoff.z)) / u.handoff.z, 0.0, 1.0);
    if dissolve < 1.0 / 255.0 || (in.alpha_test > 0.5 && texel.a < 133.0 / 255.0) {
        discard;
    }
    return vec4f(fog_of(texel.rgb * in.color, in.world), 1.0);
}
";

pub struct Statics {
    settings: Settings,
    loading: Option<Receiver<Result<Loaded, String>>>,
    catalog: Option<Arc<Catalog>>,
    gpu: Option<Gpu>,
    requests: Option<Sender<u32>>,
    arrivals: Option<Receiver<Arrival>>,
    resident: HashMap<u32, Resident>,
    requested: HashSet<u32>,
    resident_bytes: u64,
    /// The draws of the frame: texture, subset, instance, triangles.
    draws: Vec<(u32, u32, u32, u32)>,
    instance_data: Vec<f32>,
    /// The grass of the frame: texture, subset, record.
    grass_draws: Vec<(u32, u32, [f32; 12])>,
    grass_frames: u64,
    grass_drawn: u64,
    grass_cut: u64,
    frames: u64,
    drawn: u64,
    most_bytes: u64,
    failed: bool,
    /// The loads complete before a frame, and all arrivals go to the GPU in the frame.
    pub synchronous: bool,
    /// The name of the world space of the frame: empty for the exterior.
    world: Vec<u8>,
}

impl Statics {
    /// Reads the settings and starts the load of the catalog and of the textures.
    pub fn start(document: &ConfigDocument, folder: PathBuf) -> Statics {
        let land = &document.settings().distant_land;
        let settings = Settings {
            statics: land.statics,
            grass: land.render_grass,
            band_ends: [land.near_static_end * CELL, land.far_static_end * CELL, land.very_far_static_end * CELL],
            far_min_size: land.far_static_min_size,
            very_far_min_size: land.very_far_static_min_size,
            draw_distance: land.draw_distance * CELL,
        };
        let (sender, receiver) = channel();
        thread::spawn(move || {
            let result = read_catalog(&folder, &settings).map(|catalog| {
                let mut textures: Vec<Option<Vec<u8>>> = catalog.textures.iter().map(|name| read_texture(&folder, name)).collect();
                archive_textures(&folder, &catalog.textures, &mut textures);
                Loaded { catalog: Arc::new(catalog), textures }
            });
            let _ = sender.send(result);
        });
        Statics {
            settings,
            loading: Some(receiver),
            catalog: None,
            gpu: None,
            requests: None,
            arrivals: None,
            resident: HashMap::new(),
            requested: HashSet::new(),
            resident_bytes: 0,
            draws: Vec::new(),
            instance_data: Vec::new(),
            grass_draws: Vec::new(),
            grass_frames: 0,
            grass_drawn: 0,
            grass_cut: 0,
            frames: 0,
            drawn: 0,
            most_bytes: 0,
            failed: false,
            synchronous: false,
            world: Vec::new(),
        }
    }

    /// Sets the world space of the frame: the exterior with an empty name, or an interior
    /// cell with its name as the game has it. Returns true when the catalog has it; false
    /// before the catalog is read.
    pub fn set_world(&mut self, name: &[u8]) -> bool {
        if self.world != name {
            self.world = name.to_vec();
            self.request_world();
        }
        self.catalog.as_ref().is_some_and(|catalog| catalog.worlds.contains_key(name))
    }

    /// Asks the reader thread for the ordinary subsets of the places of the world space of
    /// the frame that are not on the GPU. A merged subset comes when the camera is near it.
    fn request_world(&mut self) {
        let Some(catalog) = self.catalog.clone() else { return };
        let Some(requests) = self.requests.as_ref() else { return };
        let Some(world) = catalog.worlds.get(&self.world) else { return };
        let mut wanted: Vec<u32> = world.cells.values().flatten().map(|i| catalog.instances[*i as usize].subset).filter(|s| catalog.subsets[*s as usize].components.is_empty()).collect();
        // The subsets of the grass too: they are never merged.
        wanted.extend(world.grass_cells.values().flatten().flat_map(|i| {
            let place = &catalog.grass[*i as usize];
            place.first_subset..place.first_subset + place.subset_count
        }));
        wanted.sort_unstable();
        wanted.dedup();
        for subset in wanted {
            if !self.resident.contains_key(&subset) && self.requested.insert(subset) {
                let _ = requests.send(subset);
            }
        }
    }

    fn finish_load(&mut self, scene: &SceneView) {
        let Some(receiver) = self.loading.as_ref() else { return };
        let result = if self.synchronous { receiver.recv().ok() } else { receiver.try_recv().ok() };
        let Some(result) = result else { return };
        self.loading = None;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(problem) => {
                println!("distant statics: not there: {problem}");
                self.failed = true;
                return;
            }
        };
        let catalog = loaded.catalog.clone();
        println!(
            "distant statics: {} subsets, {} places in the exterior and {} interior cells, {} grass places, {} textures; {:.0} MB ordinary, {:.0} MB merged",
            catalog.subsets.len(),
            catalog.instances.len(),
            catalog.worlds.len() - 1,
            catalog.grass.len(),
            catalog.textures.len(),
            catalog.ordinary_bytes as f64 / 1e6,
            catalog.merged_bytes as f64 / 1e6
        );
        // The interior cells with a world space, for a test that needs one: the first few,
        // or all of them with MGE_XE_LIST_WORLDS=1.
        let mut interiors: Vec<&[u8]> = catalog.worlds.keys().map(|name| name.as_slice()).filter(|name| !name.is_empty()).collect();
        interiors.sort();
        if !interiors.is_empty() {
            let shown = if std::env::var_os("MGE_XE_LIST_WORLDS").is_some() { interiors.len() } else { 12 };
            let names: Vec<String> = interiors.iter().take(shown).map(|name| String::from_utf8_lossy(name).into_owned()).collect();
            println!("distant statics: interior cells with statics: {}{}", names.join("; "), if interiors.len() > shown { "; ..." } else { "" });
        }
        self.gpu = Some(make_gpu(scene, &loaded));
        let (request_sender, request_receiver) = channel();
        let (arrival_sender, arrival_receiver) = channel();
        let for_reader = catalog.clone();
        thread::spawn(move || reader(for_reader, request_receiver, arrival_sender));
        self.requests = Some(request_sender);
        self.arrivals = Some(arrival_receiver);
        self.catalog = Some(catalog);
        // All ordinary subsets that a place of the world space uses go to the GPU now.
        self.request_world();
    }

    /// Makes the buffers of the subsets that the reader thread has ready, for a part of a frame.
    fn take_arrivals(&mut self, scene: &SceneView) {
        let Some(arrivals) = self.arrivals.as_ref() else { return };
        let (mut count, mut bytes) = (0, 0);
        while self.synchronous || (count < ARRIVALS_IN_FRAME && bytes < ARRIVAL_BYTES_IN_FRAME) {
            // Synchronous: all that was asked for comes now. A subset that the reader thread
            // could not read never comes: a limit of time.
            let arrival = if self.synchronous {
                if self.requested.is_empty() {
                    break;
                }
                arrivals.recv_timeout(std::time::Duration::from_secs(5)).ok()
            } else {
                arrivals.try_recv().ok()
            };
            let Some(arrival) = arrival else {
                if self.synchronous {
                    println!("distant statics: {} subsets did not come in time", self.requested.len());
                    self.requested.clear();
                }
                break;
            };
            // The camera can be far away again.
            if !self.requested.remove(&arrival.subset) || arrival.indexes.is_empty() {
                continue;
            }
            let size = (arrival.vertices.len() + arrival.indexes.len()) as u64;
            let vertices = scene.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &arrival.vertices, usage: wgpu::BufferUsages::VERTEX });
            let indexes = scene.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &arrival.indexes, usage: wgpu::BufferUsages::INDEX });
            self.resident.insert(arrival.subset, Resident { vertices, indexes, faces: arrival.faces, bytes: size });
            self.resident_bytes += size;
            self.most_bytes = self.most_bytes.max(self.resident_bytes);
            count += 1;
            bytes += size as usize;
        }
    }

    /// Draws the grass of the frame into the pass: the places within the view range of the
    /// game that are in the view, one draw for each subset.
    pub fn draw_grass(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &GrassFrame) {
        if self.failed || !self.settings.grass {
            return;
        }
        let (Some(catalog), Some(gpu)) = (self.catalog.clone(), self.gpu.as_ref()) else { return };
        let Some(world) = catalog.worlds.get(&self.world) else { return };
        let eye = [frame.eye[0], frame.eye[1], frame.eye[2]];
        let view_range = frame.eye[3];
        self.grass_draws.clear();
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = (view_range / CELL).ceil() as i32;
        let mut cut = false;
        'cells: for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = world.grass_cells.get(&(x, y)) else { continue };
                for index in list {
                    let place = &catalog.grass[*index as usize];
                    let to = [place.position[0] - eye[0], place.position[1] - eye[1], place.position[2] - eye[2]];
                    let reach = view_range + place.radius;
                    if to[0] * to[0] + to[1] * to[1] + to[2] * to[2] > reach * reach {
                        continue;
                    }
                    let p = place.position;
                    if !frame.planes.iter().all(|plane| plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3] >= -place.radius) {
                        continue;
                    }
                    let record = grass_record(&world_matrix(place.position, place.rotation, place.scale));
                    for subset in place.first_subset..place.first_subset + place.subset_count {
                        if !self.resident.contains_key(&subset) {
                            continue;
                        }
                        if self.grass_draws.len() == GRASS_RECORDS {
                            cut = true;
                            break 'cells;
                        }
                        self.grass_draws.push((catalog.subsets[subset as usize].texture, subset, record));
                    }
                }
            }
        }
        self.grass_frames += 1;
        self.grass_cut += cut as u64;
        if self.grass_draws.is_empty() {
            return;
        }
        self.grass_draws.sort_unstable_by_key(|draw| (draw.0, draw.1));
        let records: Vec<f32> = self.grass_draws.iter().flat_map(|draw| draw.2).collect();
        scene.queue.write_buffer(&gpu.grass_records, 0, bytemuck::cast_slice(&records));
        let uniforms = GrassUniforms {
            view_projection: frame.view_projection,
            eye: frame.eye,
            fog_color: frame.fog_color,
            fog: frame.fog,
            sun_direction: frame.sun_direction,
            sun_color: frame.sun_color,
            ambient: frame.ambient,
            wind: frame.wind,
            foot: frame.foot,
        };
        scene.queue.write_buffer(&gpu.grass_uniforms, 0, bytemuck::bytes_of(&uniforms));

        pass.set_pipeline(if scene.wireframe { gpu.grass_pipeline_lines.as_ref().unwrap_or(&gpu.grass_pipeline) } else { &gpu.grass_pipeline });
        pass.set_bind_group(0, &gpu.grass_group, &[]);
        pass.set_vertex_buffer(1, gpu.grass_records.slice(..));
        // One draw for each run of places with the same subset.
        let mut start = 0;
        while start < self.grass_draws.len() {
            let (texture, subset, _) = self.grass_draws[start];
            let mut end = start + 1;
            while end < self.grass_draws.len() && self.grass_draws[end].1 == subset {
                end += 1;
            }
            if let Some(resident) = self.resident.get(&subset) {
                pass.set_bind_group(1, &gpu.textures[texture as usize], &[]);
                pass.set_vertex_buffer(0, resident.vertices.slice(..));
                pass.set_index_buffer(resident.indexes.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..resident.faces[0] * 3, 0, start as u32..end as u32);
            }
            start = end;
        }
        self.grass_drawn += self.grass_draws.len() as u64;
    }

    /// Draws the statics of the frame into the pass. Returns the number of draws.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &Frame, set: usize) -> u32 {
        // The sets 2 and 3 are the layers of the shadow map.
        let shadow = set >= 2;
        if self.failed {
            return 0;
        }
        self.finish_load(scene);
        self.take_arrivals(scene);
        let (Some(catalog), Some(requests)) = (self.catalog.clone(), self.requests.as_ref()) else { return 0 };
        let Some(world) = catalog.worlds.get(&self.world) else { return 0 };
        let settings = self.settings;
        let eye = [frame.eye[0], frame.eye[1], frame.eye[2]];
        let view_range = frame.eye[3];
        let complete_at = view_range - HANDOFF;
        // A static before the band of the handoff is not there: the game draws the real one.
        let gone_before = complete_at - HANDOFF_BAND - HANDOFF_CULL;
        let admit = settings.draw_distance + CELL;
        let retain = admit + CELL;

        self.draws.clear();
        let reach = settings.band_ends[2].max(admit) + CELL;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = (reach / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = world.cells.get(&(x, y)) else { continue };
                for index in list {
                    let instance = &catalog.instances[*index as usize];
                    let subset = &catalog.subsets[instance.subset as usize];
                    let to = [instance.center[0] - eye[0], instance.center[1] - eye[1], instance.center[2] - eye[2]];
                    let range = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                    let merged = !subset.components.is_empty();
                    if merged {
                        let flat = (to[0] * to[0] + to[1] * to[1]).sqrt();
                        if flat <= admit + instance.radius {
                            if !self.resident.contains_key(&instance.subset) && self.requested.insert(instance.subset) {
                                let _ = requests.send(instance.subset);
                            }
                        }
                    }
                    let Some(resident) = self.resident.get(&instance.subset) else { continue };
                    // The band of the tier of the place, and the end of the fog.
                    let band_end = settings.band_ends[instance.tier as usize].min(frame.fog_end);
                    if range > band_end + instance.radius || range + instance.radius < gone_before {
                        continue;
                    }
                    if !frame.planes.iter().all(|p| p[0] * instance.center[0] + p[1] * instance.center[1] + p[2] * instance.center[2] + p[3] >= -instance.radius) {
                        continue;
                    }
                    // The level of detail by the distance.
                    let faces = if range <= settings.band_ends[0] + instance.radius {
                        resident.faces[0]
                    } else if range <= settings.band_ends[1] + instance.radius {
                        resident.faces[1]
                    } else {
                        resident.faces[2]
                    };
                    if faces != 0 {
                        self.draws.push((subset.texture, instance.subset, *index, faces));
                    }
                }
            }
        }
        // The merged subsets that are too far go away. The check is for all of them, because
        // the camera can go a long way in one frame.
        if self.frames % 30 == 0 {
            let far: Vec<u32> = self
                .resident
                .keys()
                .copied()
                .filter(|subset| {
                    catalog.merged_place.get(subset).is_some_and(|index| {
                        let instance = &catalog.instances[*index as usize];
                        let flat = ((instance.center[0] - eye[0]).powi(2) + (instance.center[1] - eye[1]).powi(2)).sqrt();
                        flat > retain + instance.radius
                    })
                })
                .collect();
            for subset in far {
                if let Some(resident) = self.resident.remove(&subset) {
                    self.resident_bytes -= resident.bytes;
                }
            }
        }
        self.frames += 1;
        if self.draws.is_empty() {
            return 0;
        }
        self.draws.sort_unstable();

        // The data of each draw: the world matrix and the values of the subset.
        self.instance_data.clear();
        for (_, subset, instance, _) in &self.draws {
            let s = &catalog.subsets[*subset as usize];
            self.instance_data.extend_from_slice(&catalog.instances[*instance as usize].world);
            self.instance_data.extend_from_slice(&[s.palette as f32, s.has_alpha as u32 as f32, s.animate_uv as u32 as f32, 0.0]);
        }
        let Some(gpu) = self.gpu.as_mut() else { return 0 };
        let buffers = &mut gpu.sets[set];
        let needed = self.draws.len();
        if needed > buffers.instance_capacity {
            buffers.instance_capacity = needed.next_power_of_two().max(1024);
            buffers.instances = scene.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("distant statics, the places of a frame"),
                size: (buffers.instance_capacity * 80) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        scene.queue.write_buffer(&buffers.instances, 0, bytemuck::cast_slice(&self.instance_data));
        let uniforms = Uniforms {
            view_projection: frame.view_projection,
            eye: frame.eye,
            fog_color: frame.fog_color,
            fog: frame.fog,
            flags: frame.flags,
            sun_direction: frame.sun_direction,
            sun_color: frame.sun_color,
            ambient: frame.ambient,
            sun_pos: frame.scattering[0],
            sky: frame.scattering[1],
            outscatter: frame.scattering[2],
            inscatter: frame.scattering[3],
            sun_altitude: frame.scattering[4],
            handoff: [frame.time, complete_at, HANDOFF_BAND, HANDOFF_CULL],
        };
        scene.queue.write_buffer(&buffers.uniforms, 0, bytemuck::bytes_of(&uniforms));

        // Lines when the game draws in wireframe (see `SceneView::wireframe`), as MGE XE on
        // Direct3D 9 draws with the fill mode the engine left; the shadow map stays filled.
        let (fill, lines) = if set == 1 { (&gpu.mirror_pipeline, &gpu.mirror_pipeline_lines) } else { (&gpu.pipeline, &gpu.pipeline_lines) };
        pass.set_pipeline(if shadow { &gpu.shadow_pipeline } else if scene.wireframe { lines.as_ref().unwrap_or(fill) } else { fill });
        pass.set_bind_group(0, &buffers.group, &[]);
        pass.set_vertex_buffer(1, buffers.instances.slice(..));
        let (mut texture, mut bound) = (u32::MAX, u32::MAX);
        for (i, (draw_texture, subset, _, faces)) in self.draws.iter().enumerate() {
            let Some(resident) = self.resident.get(subset) else { continue };
            if *draw_texture != texture {
                texture = *draw_texture;
                pass.set_bind_group(1, &gpu.textures[texture as usize], &[]);
            }
            if *subset != bound {
                bound = *subset;
                pass.set_vertex_buffer(0, resident.vertices.slice(..));
                pass.set_index_buffer(resident.indexes.slice(..), wgpu::IndexFormat::Uint16);
            }
            pass.draw_indexed(0..faces * 3, 0, i as u32..i as u32 + 1);
        }
        self.drawn += self.draws.len() as u64;
        self.draws.len() as u32
    }
}

impl Drop for Statics {
    fn drop(&mut self) {
        println!(
            "distant statics: {} frames, {:.0} draws in a frame as a mean, at most {:.0} MB of buffers on the GPU",
            self.frames,
            self.drawn as f64 / self.frames.max(1) as f64,
            self.most_bytes as f64 / 1e6
        );
        println!(
            "grass: {} frames, {:.0} records in a frame as a mean, {} frames with more than {GRASS_RECORDS}",
            self.grass_frames,
            self.grass_drawn as f64 / self.grass_frames.max(1) as f64,
            self.grass_cut
        );
    }
}

/// Reads a texture file of a subset: from the textures of the distant land, then from the
/// textures of the game. The name can have another extension than the file.
fn read_texture(folder: &Path, name: &str) -> Option<Vec<u8>> {
    let as_dds = Path::new(name).with_extension("dds");
    let data_files = folder.parent()?;
    for candidate in [
        folder.join("statics").join("textures").join(&as_dds),
        data_files.join("textures").join(&as_dds),
        folder.join("statics").join("textures").join(name),
        data_files.join("textures").join(name),
    ] {
        if let Ok(bytes) = std::fs::read(&candidate) {
            return Some(bytes);
        }
    }
    None
}

/// Reads the textures that are not loose files from the archives of the game, with the file
/// system layer of the generator: it knows the archives of `Morrowind.ini` and the rule for
/// a texture name with another extension.
fn archive_textures(folder: &Path, names: &[String], textures: &mut [Option<Vec<u8>>]) {
    if textures.iter().all(Option::is_some) {
        return;
    }
    // The folder of the game: two above the distant land, or MGE_XE_GAME_DIR for a replay
    // where the distant land is elsewhere.
    let game = match std::env::var_os("MGE_XE_GAME_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => match folder.parent().and_then(Path::parent) {
            Some(game) => game.to_path_buf(),
            None => return,
        },
    };
    let options = distantland::vfs::VfsLoadOptions { morrowind_ini: Some(game.join("Morrowind.ini")), ..Default::default() };
    let files = match distantland::vfs::Vfs::load(&options) {
        Ok(files) => files,
        Err(problem) => {
            println!("distant statics: the archives of the game are not read: {problem}");
            return;
        }
    };
    for (name, texture) in names.iter().zip(textures.iter_mut()).filter(|(_, texture)| texture.is_none()) {
        if let Some(asset) = files.resolve_texture(name) {
            *texture = files.read_asset_bytes(&asset).ok().map(|bytes| bytes.into_owned());
        }
    }
}

fn make_gpu(scene: &SceneView, loaded: &Loaded) -> Gpu {
    let device = scene.device;
    let catalog = &loaded.catalog;
    let make_uniforms = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let uniform_buffers = [make_uniforms(), make_uniforms(), make_uniforms(), make_uniforms()];
    let palettes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the palettes"),
        contents: bytemuck::cast_slice(&catalog.palettes),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
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
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let groups = uniform_buffers.each_ref().map(|uniforms| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: palettes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        })
    });

    // A texture that is not there, or that this program does not read, is white.
    let white = device
        .create_texture_with_data(
            scene.queue,
            &wgpu::TextureDescriptor {
                label: Some("distant statics, no texture"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            &[255, 255, 255, 255],
        )
        .create_view(&Default::default());
    let mut missing = Vec::new();
    let mut textures = Vec::with_capacity(catalog.textures.len());
    for (name, bytes) in catalog.textures.iter().zip(&loaded.textures) {
        let view = match bytes.as_ref().map(|bytes| dds_texture(scene, name, bytes)) {
            Some(Ok(view)) => view,
            Some(Err(problem)) => {
                missing.push(problem);
                white.clone()
            }
            None => {
                missing.push(format!("{name}: no file"));
                white.clone()
            }
        };
        textures.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &texture_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        }));
    }
    if !missing.is_empty() {
        println!("distant statics: {} textures are white: {}", missing.len(), missing.iter().take(6).cloned().collect::<Vec<_>>().join("; "));
    }

    let source = format!("{SHADER}{}", super::FOG_FUNCTION);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant statics"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout), Some(&texture_layout)], immediate_size: 0 });
    // A vertex of a statics file has 20 bytes: the position and the palette number as four
    // half floats, the normal, the colour, and the texture coordinate as two half floats.
    let vertex = wgpu::vertex_attr_array![0 => Float16x4, 1 => Unorm8x4, 2 => Unorm8x4, 3 => Float16x2];
    let place = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4];
    let make_pipeline = |cull: wgpu::Face, polygon_mode: wgpu::PolygonMode| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant statics"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout { array_stride: VERTEX as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex }),
                Some(wgpu::VertexBufferLayout { array_stride: 80, step_mode: wgpu::VertexStepMode::Instance, attributes: &place }),
            ],
        },
        // `CullMode = CW` of the pass of MGE XE: a triangle with its corners clockwise on
        // the screen is not drawn.
        primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: Some(cull), polygon_mode, ..Default::default() },
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
    let pipeline = make_pipeline(wgpu::Face::Back, wgpu::PolygonMode::Fill);
    let mirror_pipeline = make_pipeline(wgpu::Face::Front, wgpu::PolygonMode::Fill);
    // The same as lines, for a frame in which the game draws in wireframe (see
    // `SceneView::wireframe`); a device without the line mode draws them filled then.
    let lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
    let pipeline_lines = lines.then(|| make_pipeline(wgpu::Face::Back, wgpu::PolygonMode::Line));
    let mirror_pipeline_lines = lines.then(|| make_pipeline(wgpu::Face::Front, wgpu::PolygonMode::Line));
    // The casters of the shadow map: the same vertices and places, the depth from the sun.
    let shadow_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant statics shadow"), source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()) });
    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant statics shadow"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shadow_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout { array_stride: VERTEX as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex }),
                Some(wgpu::VertexBufferLayout { array_stride: 80, step_mode: wgpu::VertexStepMode::Instance, attributes: &place }),
            ],
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
    let make_instances = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics, the places of a frame"),
            size: 1024 * 80,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let [group_main, group_mirror, group_shadow0, group_shadow1] = groups;
    let [uniforms_main, uniforms_mirror, uniforms_shadow0, uniforms_shadow1] = uniform_buffers;
    let sets = [
        BufferSet { group: group_main, uniforms: uniforms_main, instances: make_instances(), instance_capacity: 1024 },
        BufferSet { group: group_mirror, uniforms: uniforms_mirror, instances: make_instances(), instance_capacity: 1024 },
        BufferSet { group: group_shadow0, uniforms: uniforms_shadow0, instances: make_instances(), instance_capacity: 1024 },
        BufferSet { group: group_shadow1, uniforms: uniforms_shadow1, instances: make_instances(), instance_capacity: 1024 },
    ];

    // The grass: its own uniforms and shader, and the textures of the statics.
    let grass_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grass"),
        size: size_of::<GrassUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let grass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    let grass_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &grass_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: grass_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let grass_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("grass"), source: wgpu::ShaderSource::Wgsl(GRASS_SHADER.into()) });
    let grass_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&grass_layout), Some(&texture_layout)], immediate_size: 0 });
    let grass_place = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
    let make_grass_pipeline = |polygon_mode: wgpu::PolygonMode| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("grass"),
        layout: Some(&grass_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &grass_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout { array_stride: VERTEX as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex }),
                Some(wgpu::VertexBufferLayout { array_stride: GRASS_RECORD as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &grass_place }),
            ],
        },
        // `CullMode = none` of the grass pass: the two sides of a blade.
        primitive: wgpu::PrimitiveState { cull_mode: None, polygon_mode, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: scene.depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &grass_module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let grass_pipeline = make_grass_pipeline(wgpu::PolygonMode::Fill);
    let grass_pipeline_lines = lines.then(|| make_grass_pipeline(wgpu::PolygonMode::Line));
    let grass_records = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grass, the places of a frame"),
        size: (GRASS_RECORDS * GRASS_RECORD) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Gpu { pipeline, mirror_pipeline, pipeline_lines, mirror_pipeline_lines, shadow_pipeline, sets, textures, grass_pipeline, grass_pipeline_lines, grass_group, grass_uniforms, grass_records }
}
