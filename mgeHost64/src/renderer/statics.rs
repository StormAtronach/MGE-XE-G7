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
//!   most of the data. A reader thread gets the bytes of a merged subset when the camera
//!   comes within the draw distance and one more cell (`want_near`). Then
//!   `distant_land.statics_residency` decides: "auto" keeps it until the merged subsets pass
//!   a high mark and then takes off the ones wanted least recently (`trim_resident`); "near"
//!   drops it when the camera is farther than the draw distance and two more cells
//!   (`frame_work`, every 30 frames); "all" asks for all of a world space when they fit under
//!   the high mark. A subset of another world space (an interior cell with statics has its
//!   own coordinates) counts as the farthest (`distance_here`). The merged
//!   subsets on the GPU stay under a cap, as in the IPC host mode: what the memory budget of
//!   the GPU (`VK_EXT_memory_budget`) leaves after the memory in use and a margin, lowered
//!   when other programs take memory; without a budget, `distant_land.statics_fallback_cap_mb`.
//!   When the cap is full, a merged subset farther from the camera goes for a nearer one.
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
//! records, out to the view range of the game. The shader is that of `XE Mod Grass.fx`: the
//! wind, the grass that the player pushes away, the light of the sun from the two sides, and
//! the shadow map of the frame ([`GrassShadow`]), which ends on a blade as on the ground under
//! it (tunable `grass.shadow`).
//!
//! An interior cell with statics in `usage.data` is a world space of its own, with its
//! places and its grass; the renderer selects it by the name of the cell. In a plain
//! interior cell the statics have the linear fog of the game (`StaticInteriorVS`), given
//! by the flags of the frame.
//!
//! A subset with a water flag is not a static: it is the surface of a water volume beyond the
//! cells that the game has loaded (flag 0x04, 0x08 for one that reflects the sky only), or a
//! dry space (0x10). They have places of their own, and `far_water` gives those in view to
//! the water (`water/volume.rs`).
//!
//! A place can be in a vis group of `usage.data`: a journal entry, a global variable or a
//! unique object of the game shows or hides all the places of the group. MGE XE finds the
//! state of the groups when the player goes into another cell and sends it in a record
//! (`set_groups`); a place of a hidden group is not drawn, not in the shadow map, and not
//! given to the water. The generator never merges a place of a group.
//!
//! The places of a frame are chosen on the GPU when the device can draw from it (`cull.rs`):
//! a compute pass tests each place as the loop of the CPU does, and one indirect call for each
//! texture draws them from pools of geometry. Without the features of the device, or with
//! `MGE_XE_STATICS_CULL=cpu`, the CPU chooses them and draws each place from the same pools.
//!
//! No test against the land hides a place.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use ash::vk;
use distantland::mge_xe::usage_data::load_usage_data;
use mge_config::{ConfigDocument, StaticsResidency};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;
use nirender_wgpu::SceneView;

use super::water::{FarWater, Look};
use super::{CELL, dds_texture};

mod cull;

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
const GRASS_RECORD: usize = 64;
/// The work of one frame for subsets that arrive from the reader thread.
const ARRIVALS_IN_FRAME: usize = 24;
const ARRIVAL_BYTES_IN_FRAME: usize = 16 << 20;
/// The cap of the merged subsets from the memory budget, as `DistantLand` of the IPC host
/// mode makes it: the margin is an eighth of the budget, from 256 MB to 1 GB; a new sample
/// each half second; the cap goes down at the fourth lower sample in a row.
const HEADROOM_LEAST: u64 = 256 << 20;
const HEADROOM_MOST: u64 = 1 << 30;
const BUDGET_SAMPLE: Duration = Duration::from_millis(500);
const LOWER_SAMPLES: u32 = 4;
/// The cap goes up again at the twentieth higher sample in a row (10 seconds), so a short dip
/// of another program's memory does not raise it.
const HIGHER_SAMPLES: u32 = 20;
/// The least dead band of the cap ([`cap_move`]).
const CAP_BAND_LEAST: u64 = 64 << 20;
/// With `distant_land.statics_residency` "auto" or "all": a merged subset that the camera
/// wanted in the last minute stays, and the trim goes down to 85 % of the high mark, so the
/// next trim is not at the next arrival.
const KEEP_SECONDS: f32 = 60.0;
const LOW_MARK_TWENTIETHS: u64 = 17;

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn long(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(core::array::from_fn(|i| bytes[at + i]))
}

fn float(bytes: &[u8], at: usize) -> f32 {
    f32::from_bits(word(bytes, at))
}

/// An IEEE half float (the positions of a statics vertex) as f32.
fn half(bytes: &[u8], at: usize) -> f32 {
    let h = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32;
    let (sign, exponent, mantissa) = (h >> 15, (h >> 10) & 0x1F, h & 0x3FF);
    let magnitude = match exponent {
        0 => mantissa as f32 * (1.0 / 16_777_216.0),
        31 => f32::INFINITY,
        e => f32::from_bits(((e + 112) << 23) | (mantissa << 13)),
    };
    if sign == 1 { -magnitude } else { magnitude }
}

/// A grass subset has bent normals when at least this share of the corners of its triangles
/// have a normal that points against the face of the triangle (by its winding). A normal of
/// the geometry never does; an author's bent normal (up, or away from the clump) does on one
/// triangle of each pair: the grass of Fantasia Grass Mod has each blade twice, with the two
/// windings and the same normal. The scan of its 402 models (2026-10-20): about 0.5 in the 342
/// bent ones, at most 0.25 in the ordinary ones, 0.27 to 0.39 in four partly bent ones.
const BENT_SHARE: f32 = 0.3;

/// The share of the corners whose normal points against the face of their triangle, for the
/// vertices ([`VERTEX`] bytes each: the position as half floats, then the normal as bytes) and
/// the triangles (three u16 each) of a subset; [`None`] without a triangle of some area.
fn normal_alignment(vertices: &[u8], indexes: &[u8]) -> Option<f32> {
    let count = vertices.len() / VERTEX;
    let position = |v: usize| [half(vertices, v * VERTEX), half(vertices, v * VERTEX + 2), half(vertices, v * VERTEX + 4)];
    let normal = |v: usize| -> [f32; 3] { core::array::from_fn(|i| vertices[v * VERTEX + 8 + i] as f32 / 127.5 - 1.0) };
    let (mut corners, mut against) = (0u32, 0u32);
    for triangle in indexes.chunks_exact(TRIANGLE) {
        let corner: [usize; 3] = core::array::from_fn(|i| u16::from_le_bytes([triangle[2 * i], triangle[2 * i + 1]]) as usize);
        if corner.iter().any(|&v| v >= count) {
            continue;
        }
        let [a, b, c] = corner.map(position);
        let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let face = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let area = (face[0] * face[0] + face[1] * face[1] + face[2] * face[2]).sqrt();
        if area < 1e-6 {
            continue;
        }
        for v in corner {
            let n = normal(v);
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if length > 1e-3 {
                corners += 1;
                against += ((n[0] * face[0] + n[1] * face[1] + n[2] * face[2]) / (length * area) < 0.0) as u32;
            }
        }
    }
    (corners > 0).then(|| against as f32 / corners as f32)
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
    /// 0: a static. 1: water that reflects the scene, 2: water that reflects the sky only,
    /// 3: a dry space.
    water: u8,
    /// The look of water: its look line, or the standard look of its kind.
    look: Option<Look>,
    /// The sphere of the subset in the space of its model: the centre and the radius.
    sphere: [f32; 4],
    texture: u32,
    /// The place of its first palette entry in the palette buffer.
    palette: u32,
    components: Vec<Component>,
    /// A subset of a grass static: drawn by the grass, from buffers of its own.
    grass: bool,
}

impl Subset {
    /// True for a subset that the draws of the statics use: in the pools of the textures.
    fn pooled(&self) -> bool {
        self.water == 0 && !self.grass
    }
    /// The bytes of its vertices and triangles in the file.
    fn bytes(&self) -> u64 {
        self.vertex_count as u64 * VERTEX as u64 + self.triangle_count as u64 * TRIANGLE as u64
    }
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
    /// The vis group of the place: 0 for none.
    group: u16,
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
    /// The vis group of the place: 0 for none.
    group: u16,
}

/// The places of one world space: the exterior, or one interior cell.
#[derive(Default)]
struct World {
    /// The places of the water subsets, by cell: as `cells`.
    water_cells: HashMap<(i32, i32), Vec<u32>>,
    /// The instances by the cell of their centre.
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// The grass places by the cell of their position.
    grass_cells: HashMap<(i32, i32), Vec<u32>>,
    /// The places of merged subsets in `cells`.
    merged: Vec<u32>,
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
    /// The vis groups of `usage.data` and the group 0 (no group).
    groups: usize,
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

/// The subset flags that the generator writes and this build knows: alpha (0x01), a UV
/// controller (0x02), water (0x04), water that reflects only the sky (0x08) and a dry space
/// (0x10).
/// Direct3D 9 checks the same mask (`KnownFlagsMask`).
const KNOWN_SUBSET_FLAGS: u32 = 0x1F;

/// Why the catalog of the statics did not load.
enum CatalogError {
    /// A file is not there or cannot be read: no distant statics.
    Missing(String),
    /// The data has what this build does not know: distant land is off, and it must be
    /// generated again.
    Rejected(String),
}

impl From<String> for CatalogError {
    fn from(problem: String) -> Self {
        CatalogError::Missing(problem)
    }
}

/// The problem with the flags of a subset, if there is one.
fn flags_problem(flags: u32, file: &Path) -> Option<String> {
    let unknown = flags & !KNOWN_SUBSET_FLAGS;
    (unknown != 0).then(|| {
        format!("a subset in {} has the flags {unknown:#x}, which this MGE XE does not know: generate distant land again in the MGE XE GUI", file.display())
    })
}

/// The problem with the subset flags of the statics files in `folder` (the distant land
/// folder), if there is one. It reads only the headers and the subset tables: for the loader
/// of the land, which must not draw from data that the statics reject. A file that cannot be
/// read is the problem of the loader of the statics, not of this check.
pub(super) fn flags_problem_in(folder: &Path) -> Option<String> {
    for shard in 0..SHARDS {
        let path = folder.join("statics").join(format!("static_meshes_{shard:03}"));
        let Ok(file) = File::open(&path) else { continue };
        let mut header = [0u8; 160];
        if file.seek_read(&mut header, 0).ok() != Some(160) || &header[0..8] != b"XESTAT06" {
            continue;
        }
        let (subset_count, subset_table) = (word(&header, 36) as usize, long(&header, 56));
        let mut table = vec![0u8; subset_count * 168];
        if file.seek_read(&mut table, subset_table).ok() != Some(table.len()) {
            continue;
        }
        if let Some(problem) = table.chunks_exact(168).find_map(|r| flags_problem(word(r, 72), &path)) {
            return Some(problem);
        }
    }
    None
}

/// Reads the tables of the 128 files and the places of `usage.data`.
fn read_catalog(folder: &Path, settings: &Settings) -> Result<Catalog, CatalogError> {
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
            return Err(format!("{} is not a statics file of version 6", path.display()).into());
        }
        let geometry = long(&header, 88) as usize;
        let mut tables = vec![0u8; geometry];
        file.seek_read(&mut tables, 0).map_err(|error| format!("{}: {error}", path.display()))?;
        let (static_count, subset_count) = (word(&header, 32) as usize, word(&header, 36) as usize);
        let (static_table, subset_table) = (long(&header, 40) as usize, long(&header, 56) as usize);
        let (component_table, palette_table) = (long(&header, 112) as usize, long(&header, 136) as usize);
        if subset_table + subset_count * 168 > geometry || static_table + static_count * 52 > geometry {
            return Err(format!("{} has tables past its geometry", path.display()).into());
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
            if let Some(problem) = flags_problem(flags, &path) {
                return Err(CatalogError::Rejected(problem));
            }
            let water = if flags & 0x10 != 0 {
                3
            } else if flags & 0x04 != 0 {
                if flags & 0x08 != 0 { 2 } else { 1 }
            } else {
                0
            };
            // The look line of a water subset, in the texture blob.
            let (look_at, look_length) = (long(r, 152) as usize, word(r, 160) as usize);
            let line = (water != 0 && water != 3 && look_length != 0).then(|| tables.get(look_at..look_at + look_length)).flatten().map(|bytes| String::from_utf8_lossy(bytes).into_owned());
            let look = matches!(water, 1 | 2).then(|| Look::of_distant(water, line.as_deref()));
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
                water,
                look,
                sphere: [float(r, 4), float(r, 8), float(r, 12), float(r, 0)],
                texture,
                palette,
                components,
                grass: false,
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

    for model in statics.iter().filter(|model| model.kind == 4) {
        for subset in subsets.iter_mut().skip(model.first_subset as usize).take(model.subset_count as usize) {
            subset.grass = true;
        }
    }

    // The places. `usage.data` has the static of a place as its number in all the files, in
    // their order.
    let usage = load_usage_data(&statics_folder.join("usage.data")).map_err(|error| format!("usage.data: {error}"))?;
    if usage.num_meshes as usize != statics.len() {
        return Err(format!("usage.data is for {} statics and the files have {}", usage.num_meshes, statics.len()).into());
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
                        group: place.vis_index,
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
                if s.vertex_count == 0 || s.triangle_count == 0 {
                    continue;
                }
                let (center, radius) = if model.kind == 6 {
                    (transform(&world, model.center), model.radius * place.scale)
                } else {
                    (transform(&world, [s.sphere[0], s.sphere[1], s.sphere[2]]), s.sphere[3] * place.scale)
                };
                let cell = ((center[0] / CELL).floor() as i32, (center[1] / CELL).floor() as i32);
                let list = if s.water != 0 { &mut world_space.water_cells } else { &mut world_space.cells };
                list.entry(cell).or_default().push(instances.len() as u32);
                if !s.components.is_empty() {
                    merged_place.insert(subset, instances.len() as u32);
                    if s.water == 0 {
                        world_space.merged.push(instances.len() as u32);
                    }
                }
                instances.push(Instance { subset, world, center, radius, tier, group: place.vis_index });
            }
        }
    }
    let groups = usage.vis_groups.len() + 1;
    Ok(Catalog { folder: folder.to_path_buf(), subsets, instances, worlds, textures, palettes, merged_place, grass, groups, ordinary_bytes, merged_bytes })
}

/// The work of the draws of one set (the main view, the mirror, the two layers of the shadow
/// map) since the last report (`statics report` of `nirender.tune`).
#[derive(Default, Clone, Copy)]
struct SetStats {
    calls: u64,
    seconds: f64,
    /// The places in the cells within reach.
    visited: u64,
    /// Out of the band of their tier, or before the handoff.
    distance: u64,
    /// Out of the view.
    frustum: u64,
    /// The subset is not on the GPU (yet, or not near enough for a merged one).
    waiting: u64,
    drawn: u64,
    triangles: u64,
    /// Runs of draws of one subset after the sort: the draws that one draw for a range of
    /// places would make.
    runs: u64,
}

/// The work of a wait on the loading screen (a teleport or a game load), from its first
/// frame with no world to the end of the wait ([`Statics::take_wait`]).
#[derive(Default, Clone, Copy)]
struct WaitStats {
    /// The frames with no world, and the time of the arrivals in them: all and the longest.
    frames: u32,
    upload_seconds: f64,
    longest_upload: f64,
    /// The subsets asked for from the reader thread, and their bytes.
    asked: u32,
    asked_bytes: u64,
    /// The subsets that arrived, their bytes, and those of them that were on the GPU before
    /// in the session (a return to a place reads them again).
    arrived: u32,
    arrived_bytes: u64,
    again: u32,
}

const SET_NAMES: [&str; 4] = ["view", "mirror", "shadow 1", "shadow 2"];

/// True when the place of a group is hidden: `shown` has a byte for each group, 0 for a hidden
/// one. A place without a group (0) is always shown, and so is each place before the first
/// record.
fn hidden(shown: &[u8], group: u16) -> bool {
    group != 0 && shown.get(group as usize) == Some(&0)
}

/// A name of `usage.data` or of the environment without the zeros that pad it.
pub fn unpadded(name: &[u8]) -> &[u8] {
    &name[..name.iter().rposition(|&byte| byte != 0).map_or(0, |last| last + 1)]
}

/// The bytes of a subset, from the reader thread.
struct Arrival {
    subset: u32,
    /// The reader could not open the shard of the subset or read its bytes: the rest is
    /// empty, and the subset is not asked for again.
    unreadable: bool,
    vertices: Vec<u8>,
    indexes: Vec<u8>,
    /// The triangles of the three levels of detail: all, far, very far.
    faces: [u32; 3],
    /// A grass subset with bent normals ([`BENT_SHARE`]): lit by its normal as the author
    /// made it, not turned to the side that the camera sees.
    bent: bool,
}

impl Arrival {
    fn unreadable(subset: u32) -> Arrival {
        Arrival { subset, unreadable: true, vertices: Vec::new(), indexes: Vec::new(), faces: [0; 3], bent: false }
    }
}

/// Reads the vertices and the indexes of the subsets that the renderer asks for. The indexes
/// of a merged subset come out with the triangles of the very far tier first, then those of
/// the far tier, then the rest. A subset that cannot be read gets an answer too
/// ([`Arrival::unreadable`]): every request is answered.
fn reader(catalog: Arc<Catalog>, requests: Receiver<u32>, arrivals: Sender<Arrival>) {
    let mut files: HashMap<u16, File> = HashMap::new();
    for subset in requests {
        let Some(arrival) = read_subset(&catalog, &mut files, subset) else {
            if arrivals.send(Arrival::unreadable(subset)).is_err() {
                return;
            }
            continue;
        };
        if arrivals.send(arrival).is_err() {
            return;
        }
    }
}

/// The bytes of one subset from its shard; [`None`] when the shard cannot be opened or read.
fn read_subset(catalog: &Catalog, files: &mut HashMap<u16, File>, subset: u32) -> Option<Arrival> {
    let s = &catalog.subsets[subset as usize];
    let file = match files.entry(s.shard) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            let path = catalog.folder.join("statics").join(format!("static_meshes_{:03}", s.shard));
            entry.insert(File::open(path).ok()?)
        }
    };
    let mut vertices = vec![0u8; s.vertex_count as usize * VERTEX];
    let mut source = vec![0u8; s.triangle_count as usize * TRIANGLE];
    file.seek_read(&mut vertices, s.vertex_offset).ok()?;
    file.seek_read(&mut source, s.index_offset).ok()?;
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
    let bent = s.grass && normal_alignment(&vertices, &indexes).is_some_and(|share| share >= BENT_SHARE);
    Some(Arrival { subset, unreadable: false, vertices, indexes, faces, bent })
}

/// The distance from the camera to the centre of a place, across the ground.
fn flat_distance(instance: &Instance, eye: [f32; 3]) -> f32 {
    ((instance.center[0] - eye[0]).powi(2) + (instance.center[1] - eye[1]).powi(2)).sqrt()
}

/// The budget and the use of this process of the largest memory heap of the GPU, from
/// `VK_EXT_memory_budget`, which wgpu turns on when the driver has it. [`None`] on another
/// backend than Vulkan, or without the extension.
fn memory_budget(device: &wgpu::Device) -> Option<(u64, u64)> {
    // SAFETY: the guard keeps the device alive, and the calls only read the properties of its
    // physical device.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }?;
    if !hal.enabled_device_extensions().contains(&ash::ext::memory_budget::NAME) {
        return None;
    }
    let instance = hal.shared_instance();
    // `vkGetPhysicalDeviceMemoryProperties2` is core in Vulkan 1.1.
    if instance.instance_api_version() < vk::API_VERSION_1_1 {
        return None;
    }
    let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
    unsafe { instance.raw_instance().get_physical_device_memory_properties2(hal.raw_physical_device(), &mut properties) };
    let memory = properties.memory_properties;
    let heaps = &memory.memory_heaps[..(memory.memory_heap_count as usize).min(vk::MAX_MEMORY_HEAPS)];
    let largest = (0..heaps.len()).filter(|&heap| heaps[heap].flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL)).max_by_key(|&heap| heaps[heap].size)?;
    (budget.heap_budget[largest] > 0).then_some((budget.heap_budget[largest], budget.heap_usage[largest]))
}

/// The cap of the merged subsets from a memory budget, as `mergedCapCandidate` of the IPC
/// host mode: what the budget leaves after the memory in use and a margin, and the merged
/// bytes on the GPU, which the memory in use counts already. Without them the merged subsets
/// would make their own cap smaller.
fn cap_from_budget(budget: u64, used: u64, merged: u64) -> u64 {
    let headroom = (budget / 8).clamp(HEADROOM_LEAST, HEADROOM_MOST);
    merged.saturating_add(budget.saturating_sub(used).saturating_sub(headroom))
}

/// Whether a new value of the cap from the memory budget is lower (-1) or higher (1) than the
/// cap, or the same (0): a move within a dead band of the larger of 64 MB and 1 % of the
/// budget is the same. The budget's samples move by a few megabytes from frame to frame; a
/// merged subset is a few megabytes, and the margin of the cap (an eighth of the budget, at
/// least 256 MB) holds the band.
fn cap_move(cap: u64, candidate: u64, budget: u64) -> i32 {
    let band = CAP_BAND_LEAST.max(budget / 100);
    if candidate > cap.saturating_add(band) {
        1
    } else if candidate.saturating_add(band) < cap {
        -1
    } else {
        0
    }
}

/// The merged subsets that the trim takes off: `candidates` are (the time the camera last
/// wanted it, the distance from the camera, the subset, its bytes). The least recently wanted
/// go first, the farthest first among equals, until the bytes on the GPU are at most `low`.
fn pick_trim(mut candidates: Vec<(f32, f32, u32, u64)>, mut bytes: u64, low: u64) -> Vec<u32> {
    candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut gone = Vec::new();
    for (_, _, subset, size) in candidates {
        if bytes <= low {
            break;
        }
        gone.push(subset);
        bytes = bytes.saturating_sub(size);
    }
    gone
}

/// The choice of the cap of the merged subsets. `wanted` and `on_gpu` are (distance from the
/// camera, subset, bytes); `wanted` is the nearest first, and `used` is the bytes on the GPU
/// and asked for. Gives the subsets to ask for, the subsets to take off the GPU for them, and
/// whether a wanted subset must wait. A subset on the GPU goes only for a nearer one, and only
/// when that makes enough room.
fn plan_merged(wanted: &[(f32, u32, u64)], mut on_gpu: Vec<(f32, u32, u64)>, mut used: u64, cap: u64) -> (Vec<u32>, Vec<u32>, bool) {
    on_gpu.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let (mut admit, mut evict) = (Vec::new(), Vec::new());
    let mut end = on_gpu.len();
    for &(near, subset, bytes) in wanted {
        let (mut cut, mut free) = (end, cap.saturating_sub(used));
        while free < bytes && cut > 0 && on_gpu[cut - 1].0 > near {
            cut -= 1;
            free += on_gpu[cut].2;
        }
        if free < bytes {
            return (admit, evict, true);
        }
        for &(_, gone, gone_bytes) in &on_gpu[cut..end] {
            evict.push(gone);
            used -= gone_bytes;
        }
        end = cut;
        admit.push(subset);
        used += bytes;
    }
    (admit, evict, false)
}

/// The geometry of a subset on the GPU: buffers of its own (the grass and the water), or a slot
/// in the pool of its texture (the statics).
enum Geometry {
    Own { vertices: wgpu::Buffer, indexes: wgpu::Buffer },
    Pool(cull::Slot),
}

struct Resident {
    geometry: Geometry,
    faces: [u32; 3],
    bytes: u64,
    /// The time of the arrival on the clock of [`Statics`], in seconds: the subset comes in
    /// over the fade time after it.
    arrived: f32,
    /// The last time on that clock that the camera was within the draw distance and one cell
    /// of a merged subset (`want_near`): the trim takes the least recent first.
    wanted_at: f32,
    /// A grass subset with bent normals ([`Arrival::bent`]).
    bent: bool,
}

/// What the loader thread gives to the renderer: the catalog, and the bytes of the texture
/// files in the order of the catalog ([`None`] for a file that is not there).
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
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    /// The same with the other side culled: for the reflection, where the mirror turns the
    /// winding of each triangle.
    mirror_pipeline: wgpu::RenderPipeline,
    /// The two as lines, when the device has the line mode: for a frame in which the game
    /// draws in wireframe ([`SceneView::wireframe`]).
    pipeline_lines: Option<wgpu::RenderPipeline>,
    mirror_pipeline_lines: Option<wgpu::RenderPipeline>,
    /// The depth from the sun into the shadow map.
    shadow_pipeline: wgpu::RenderPipeline,
    /// The main view, the reflection, and the two layers of the shadow map.
    sets: [BufferSet; 4],
    /// The values of all places for the draws: the world matrix and the values of the subset.
    instances: wgpu::Buffer,
    /// The time of the arrival of each subset, in seconds of the clock of [`Statics`], for the
    /// fade-in of the shaders (`arrivals` of group 1). A subset that never arrived has a time
    /// long ago.
    arrivals: wgpu::Buffer,
    /// The geometry of the statics, by texture.
    pools: cull::Pools,
    /// The choice of the places on the GPU, when the device can do it.
    culling: Option<cull::Culling>,
    textures: Vec<wgpu::BindGroup>,
    /// The same textures, for a water shader of a mod on far water.
    texture_views: Vec<wgpu::TextureView>,
    grass_pipeline: wgpu::RenderPipeline,
    grass_pipeline_lines: Option<wgpu::RenderPipeline>,
    grass_group: wgpu::BindGroup,
    grass_layout: wgpu::BindGroupLayout,
    grass_sampler: wgpu::Sampler,
    grass_shadow_sampler: wgpu::Sampler,
    /// The shadow map in `grass_group`: a texture of one texel until the first frame with a
    /// shadow map.
    grass_atlas: std::rc::Rc<wgpu::TextureView>,
    grass_uniforms: wgpu::Buffer,
    grass_records: wgpu::Buffer,
}

/// The values of a frame for the grass.
#[derive(Clone)]
pub struct GrassFrame {
    /// The position of the camera, and the view range of the game: the end of the grass.
    pub eye: [f32; 4],
    /// The fog colour of the near cells (the game view has the start and the end of their
    /// linear fog).
    pub fog_color: [f32; 4],
    pub sun_color: [f32; 4],
    pub ambient: [f32; 4],
    /// The smooth wind, and the time of the game in z.
    pub wind: [f32; 4],
    /// The position of the player, and 1 in w when it is known.
    pub foot: [f32; 4],
    pub planes: [[f32; 4]; 6],
    /// The shadow map of the frame, when the frame has one.
    pub shadow: Option<GrassShadow>,
    /// The looks of Remiros' Groundcover (`[distant_land.grass]`): 1 on, for the soft end,
    /// the vertex colour sway and the wrap lighting.
    pub look: [f32; 4],
}

/// The shadow map for the grass of a frame (`renderGrassInst` of the Direct3D 9 path).
#[derive(Clone)]
pub struct GrassShadow {
    pub atlas: std::rc::Rc<wgpu::TextureView>,
    /// The matrices from the world to the two layers, with the layout of Direct3D.
    pub layers: [[f32; 16]; 2],
    /// 1 over the resolution of a layer.
    pub rcp: f32,
    /// The visibility of the sun, for the light that `shadowSunEstimate` gives.
    pub sun_vis: f32,
    /// The camera is above the sea: the fog weakens the shadow less below it.
    pub above_sea: bool,
    /// The shadow of a blade ends as the shadow of the ground under it: the fade at the edge
    /// of the outer layer and in the fog (tunable `grass.shadow`, 2). With 1, as the
    /// Direct3D 9 path: no fade.
    pub edge: bool,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GrassUniforms {
    /// x: the end of the grass.
    range: [f32; 4],
    fog_color: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    wind: [f32; 4],
    foot: [f32; 4],
    /// The matrices from the world to the two layers of the shadow map.
    shadow_layers: [[f32; 16]; 2],
    /// `ESM_c`, `ESM_bias`, `ESM_scale`, 1 over the resolution of a layer.
    esm: [f32; 4],
    /// `shadecolor`, and `shade` of `shadowSunEstimate`.
    shade: [f32; 4],
    /// x: 1 with a shadow map. y: the light of the sun for `shadowSunEstimate` (the luminance
    /// of its colour, times 0.25 + 0.75 of its visibility). z: 1 above the sea. w: 1 for the
    /// fade of the ground at the edge and in the fog.
    shadow: [f32; 4],
    /// The looks of Remiros' Groundcover: x soft end, y vertex colour sway, z wrap lighting.
    look: [f32; 4],
}

/// The grass shader (`XE Mod Grass.fx`, with the looks of Remiros' Groundcover as options).
const GRASS_SHADER: &str = include_str!("statics/grass.wgsl");

/// The values of a set (group 1 of the statics shaders), and the share of a subset that is in.
const SET_WGSL: &str = "
struct Set {
    // x: the view range of the game in this set (0 for a set without the handoff).
    range: vec4f,
    // x: the time. y: the distance where a static is complete. z: the width of the band
    // before it. w: the distance before the band where a vertex goes away.
    handoff: vec4f,
    // x: the time now on the clock of the arrivals. y: the time a subset takes to come in
    // after its arrival (0: no fade-in).
    fade: vec4f,
}
@group(1) @binding(0) var<uniform> set_values: Set;
@group(1) @binding(1) var<storage, read> palettes: array<vec4f>;
@group(1) @binding(2) var base_sampler: sampler;
@group(1) @binding(3) var<storage, read> arrivals: array<f32>;
@group(2) @binding(0) var base_texture: texture_2d<f32>;

// The share of a subset that is in: it comes in over set_values.fade.y seconds after its
// geometry arrived (`Statics::fade_values`).
fn arrival_fade(subset: u32) -> f32 {
    if set_values.fade.y <= 0.0 {
        return 1.0;
    }
    return clamp((set_values.fade.x - arrivals[subset]) / set_values.fade.y, 0.0, 1.0);
}

// The view range of the game, for the fog (`fog_of`).
fn fog_range() -> f32 {
    return set_values.range.x;
}
";

/// The casters of the shadow map: `StaticShadowVS` and `StaticShadowPS` of `XE Shadowmap.fx`.
/// After the view group and [`SET_WGSL`] (the matrix of the layer in `view_projection`).
const SHADOW_SHADER: &str = "

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
    @location(4) @interpolate(flat) fade: f32,
}

// The order of the pixels of a block of 4 x 4 for the fade-in (Bayer), as the statics have it.
const FADE_ORDER = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);

@vertex fn vs(in: In) -> Out {
    var out: Out;
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    var clip = clip_of(world);
    // What is nearer to the sun than the near plane stays, at the near plane.
    clip.z = max(0.0, clip.z);
    out.position = clip;
    out.depth = clip.z / clip.w;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * set_values.handoff.x);
    }
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    out.fade = arrival_fade(u32(in.subset.w));
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let cell = vec2u(in.position.xy) % 4u;
    if in.fade < (FADE_ORDER[cell.y * 4u + cell.x] + 0.5) / 16.0 {
        discard;
    }
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

/// The values of a set of the statics in a frame for its draws and its choice of places. Those
/// of its view (the matrix, the fog, the sun) are in its view group (`view::View`).
#[derive(Clone, Copy)]
pub struct Frame {
    /// The position of the camera, and the view range of the game (0 for a set without the
    /// handoff).
    pub eye: [f32; 4],
    pub planes: [[f32; 4]; 6],
    /// The end of the fog of the frame: nothing is drawn past it.
    pub fog_end: f32,
    /// Seconds, for a texture that moves.
    pub time: f32,
}

/// Group 1 of the statics shaders: `Set` of [`SET_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SetUniforms {
    range: [f32; 4],
    handoff: [f32; 4],
    fade: [f32; 4],
}

/// The statics in a view: after the view group and [`SET_WGSL`].
const SHADER: &str = "
// The order of the pixels of a block of 4 x 4 for the band of the handoff (Bayer).
const HANDOFF_ORDER = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);

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
    // that moves. w: the number of the subset.
    @location(8) subset: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
    @location(1) coordinate: vec2f,
    @location(2) world: vec3f,
    @location(3) @interpolate(flat) bounds: vec4f,
    @location(4) @interpolate(flat) alpha_test: f32,
    @location(5) @interpolate(flat) fade: f32,
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    // A position is a row vector in Direct3D: position * world.
    let world = in.position.x * in.world0.xyz + in.position.y * in.world1.xyz + in.position.z * in.world2.xyz + in.world3.xyz;
    out.position = clip_of(world);
    // Near the camera the game draws the real static. A vertex before the band goes away.
    if length(world - u.eye.xyz) < set_values.handoff.y - set_values.handoff.z - set_values.handoff.w {
        out.position = vec4f(0.0, 0.0, -10000.0, 0.0);
    }
    // lightStaticVert: the normal goes through the world matrix and is not made of length 1
    // again, as in the shader of MGE XE.
    let n = normalize(in.normal.xyz * 2.0 - 1.0);
    let normal = n.x * in.world0.xyz + n.y * in.world1.xyz + n.z * in.world2.xyz;
    let light = u.sun_color.rgb * clamp(dot(normal, -u.sun_direction.xyz), 0.0, 1.0) + u.ambient.rgb + in.normal.w + baked_light(world);
    // The colour of a vertex has the bytes blue, green, red.
    out.color = in.color.bgr * light;
    out.coordinate = in.coordinate;
    if in.subset.z > 0.5 {
        out.coordinate.y += fract(0.08 * set_values.handoff.x);
    }
    out.world = world;
    out.bounds = palettes[u32(in.subset.x) + u32(in.position.w)];
    out.alpha_test = in.subset.y;
    out.fade = arrival_fade(u32(in.subset.w));
    return out;
}

// True for the pipelines of the frame with multisampling and alpha to coverage (a pipeline
// constant); false for the mirror of the water and with one sample.
override ALPHA_TO_COVERAGE: bool = false;

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
    // The handoff to the game, and the fade-in after the arrival of the subset.
    let dissolve = clamp((range - (set_values.handoff.y - set_values.handoff.z)) / set_values.handoff.z, 0.0, 1.0) * in.fade;
    // Across the band a static comes in by its share of covered pixels: Direct3D 9 gives the
    // dissolve to alpha to coverage, which needs the samples of multisampling; here an
    // ordered threshold of 4 x 4 pixels, the same with one sample and with more (the alpha
    // to coverage of the frame's pipeline takes the alpha of the texel only).
    let cell = vec2u(in.position.xy) % 4u;
    let threshold = (HANDOFF_ORDER[cell.y * 4u + cell.x] + 0.5) / 16.0;
    if dissolve < max(threshold, 1.0 / 255.0) {
        discard;
    }
    // The alpha test of StaticPS (AlphaRef 133). With alpha to coverage the alpha test is
    // off, as DXVK turns it off under the vendor switch of alpha to coverage, and the alpha
    // is calc_coverage(a, 133 / 255, 2) of `XE Common.fx`: the samples of the pixel that
    // the static covers.
    var alpha = 1.0;
    if in.alpha_test > 0.5 {
        if ALPHA_TO_COVERAGE {
            alpha = clamp(2.0 * (texel.a - 133.0 / 255.0) + 133.0 / 255.0, 0.0, 1.0);
        } else if texel.a < 133.0 / 255.0 {
            discard;
        }
    }
    return vec4f(fog_of(texel.rgb * in.color, in.world), alpha);
}
";

pub struct Statics {
    settings: Settings,
    /// The alpha of a static with an alpha test and of a blade of grass becomes the coverage
    /// of the samples of its pixel (`graphics.transparency_antialiasing`, with more than one
    /// sample): the `beginAlphaToCoverage` of the Direct3D 9 path around the statics and the
    /// grass.
    alpha_to_coverage: bool,
    loading: Option<Receiver<Result<Loaded, CatalogError>>>,
    catalog: Option<Arc<Catalog>>,
    gpu: Option<Gpu>,
    requests: Option<Sender<u32>>,
    arrivals: Option<Receiver<Arrival>>,
    resident: HashMap<u32, Resident>,
    requested: HashSet<u32>,
    resident_bytes: u64,
    /// The merged subsets that the draws want and that are not on the GPU or asked for.
    wanted: Vec<u32>,
    /// The bytes of the merged subsets on the GPU, and the most that may be.
    merged_bytes: u64,
    merged_cap: u64,
    /// `distant_land.statics_fallback_cap_mb` in bytes, [`u64::MAX`] for no cap.
    fallback_cap: u64,
    /// `distant_land.statics_residency`, and `distant_land.statics_resident_mb` in bytes (0:
    /// half of the first cap). See [`Statics::high_mark`].
    residency: StaticsResidency,
    resident_mark: u64,
    /// The first cap of the session: half of it is the automatic high mark. The cap that
    /// follows the budget falls as the statics' own pools grow, so half of it would chase
    /// the data down.
    first_cap: u64,
    /// With "all": the world space whose merged subsets were all asked for.
    all_world: Option<Vec<u8>>,
    /// The merged subsets of the world space of the frame. An interior cell with statics is a
    /// world space of its own, with its own coordinates: a distance from the camera to a
    /// subset of another world space means nothing, and such a subset counts as the farthest
    /// (`distance_here`).
    here: HashSet<u32>,
    /// The merged subsets that the trim took off.
    trimmed: u64,
    /// The time of the next sample of the memory budget, when the cap follows it, and the
    /// lower and higher samples in a row.
    next_budget_sample: Option<Instant>,
    lower_samples: u32,
    higher_samples: u32,
    lowered: u64,
    raised: u64,
    most_merged: u64,
    evicted: u64,
    capped_frames: u64,
    /// The last admission left wanted subsets waiting for the cap and asked for none.
    cap_waiting: bool,
    /// The draws of the frame: texture, subset, instance, triangles.
    draws: Vec<(u32, u32, u32, u32)>,
    /// The device and its queue, for the pools and the culling outside a draw.
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    /// The main view drew since the slots that went were last made free: a new frame.
    frame_ended: bool,
    /// A pool could not grow for a subset: said once.
    pool_full: bool,
    /// The subsets the reader thread could not read (a shard missing or unreadable): not asked
    /// for again. The first is said; the count is in the summary.
    unreadable: HashSet<u32>,
    /// The subsets that arrived and found no room in the pool of their texture (it cannot grow
    /// past the largest buffer of the device): not asked for again until a slot of that pool
    /// goes free.
    refused: HashSet<u32>,
    refused_count: u64,
    /// The sets that `prepare` chose for this frame.
    prepared: [bool; 4],
    /// The view groups of the frame, one for each set, and their layout (group 0 of the
    /// pipelines of the statics). The catalog loads when the layout is there.
    view_layout: Option<wgpu::BindGroupLayout>,
    view_groups: Vec<wgpu::BindGroup>,
    prepare_calls: u64,
    prepare_seconds: f64,
    /// The grass of the frame: texture, subset, record, the fade-in, 1 for bent normals.
    grass_draws: Vec<(u32, u32, [f32; 12], f32, f32)>,
    /// The clock of the arrivals, and the time a subset takes to come in (0: no fade-in).
    clock: Instant,
    fade_seconds: f32,
    /// The arrivals are those of a wait on the loading screen: they show complete when the
    /// wait ends, with no fade-in.
    prefetching: bool,
    grass_frames: u64,
    grass_drawn: u64,
    grass_cut: u64,
    /// The grass subsets that arrived, and those of them with bent normals.
    grass_arrived: u64,
    grass_bent: u64,
    frames: u64,
    drawn: u64,
    most_bytes: u64,
    failed: bool,
    /// The data has what this build does not know ([`CatalogError::Rejected`]).
    rejected: Option<String>,
    /// The loads complete before a frame, and all arrivals go to the GPU in the frame.
    pub synchronous: bool,
    /// The name of the world space of the frame: empty for the exterior.
    world: Vec<u8>,
    /// The state of the vis groups from MGE XE: a byte for each group, 0 for a hidden one.
    /// Empty while the record is not there or is not for the groups of the catalog.
    shown: Vec<u8>,
    /// The record of the groups as it came, for a catalog that is not read yet.
    groups_record: Vec<u8>,
    /// The places of hidden groups that a frame did not draw, for the report.
    hidden_skipped: u64,
    /// The work of each set since the last `report`, and of the grass.
    set_stats: [SetStats; 4],
    grass_stats: SetStats,
    /// The view range of the game in the last main view ([`CullSet::eye`] w).
    game_range: f32,
    /// The merged subsets that arrived in play since the last `report`: all of them, those
    /// with their place inside the draw distance, those of them past the handoff (a late
    /// arrival that the view shows), and the nearest distance of a late one.
    play_arrivals: u64,
    late_arrivals: u64,
    late_shown: u64,
    late_nearest: f32,
    /// The merged subsets that arrived in play and were on the GPU before in the session.
    play_again: u64,
    /// The subsets that were on the GPU at some time in the session.
    ever_arrived: HashSet<u32>,
    /// The work of the wait on the loading screen, from its first frame to `take_wait`.
    wait: Option<WaitStats>,
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
            alpha_to_coverage: document.settings().graphics.transparency_antialiasing,
            loading: Some(receiver),
            catalog: None,
            gpu: None,
            requests: None,
            arrivals: None,
            resident: HashMap::new(),
            requested: HashSet::new(),
            resident_bytes: 0,
            wanted: Vec::new(),
            merged_bytes: 0,
            merged_cap: u64::MAX,
            fallback_cap: match land.statics_fallback_cap_mb {
                0 => u64::MAX,
                mb => u64::from(mb) << 20,
            },
            residency: land.statics_residency,
            resident_mark: u64::from(land.statics_resident_mb) << 20,
            first_cap: u64::MAX,
            all_world: None,
            here: HashSet::new(),
            trimmed: 0,
            next_budget_sample: None,
            lower_samples: 0,
            higher_samples: 0,
            lowered: 0,
            raised: 0,
            most_merged: 0,
            evicted: 0,
            capped_frames: 0,
            cap_waiting: false,
            draws: Vec::new(),
            device: None,
            queue: None,
            frame_ended: false,
            pool_full: false,
            unreadable: HashSet::new(),
            refused: HashSet::new(),
            refused_count: 0,
            prepared: [false; 4],
            view_layout: None,
            view_groups: Vec::new(),
            prepare_calls: 0,
            prepare_seconds: 0.0,
            grass_draws: Vec::new(),
            clock: Instant::now(),
            fade_seconds: 1.0,
            prefetching: false,
            grass_frames: 0,
            grass_drawn: 0,
            grass_cut: 0,
            grass_arrived: 0,
            grass_bent: 0,
            frames: 0,
            drawn: 0,
            most_bytes: 0,
            failed: false,
            rejected: None,
            synchronous: false,
            world: Vec::new(),
            shown: Vec::new(),
            groups_record: Vec::new(),
            hidden_skipped: 0,
            set_stats: [SetStats::default(); 4],
            grass_stats: SetStats::default(),
            game_range: 0.0,
            play_arrivals: 0,
            late_arrivals: 0,
            late_shown: 0,
            late_nearest: f32::MAX,
            play_again: 0,
            ever_arrived: HashSet::new(),
            wait: None,
        }
    }

    /// Prints the grass places of the world space of the frame within `radius` of (x, y),
    /// by model: the number of places, their vis groups and how many of them are hidden, and
    /// each subset of the model with its texture, its size, and whether it is on the GPU.
    /// The command `grass report <x> <y> <radius>` of `nirender.tune`, for a test.
    pub fn grass_report(&self, x: f32, y: f32, radius: f32) {
        let Some(catalog) = self.catalog.as_ref() else {
            println!("grass report: no catalog");
            return;
        };
        let Some(world) = catalog.worlds.get(&self.world) else {
            println!("grass report: no world space {}", String::from_utf8_lossy(&self.world));
            return;
        };
        // (first subset, subset count) -> (places, hidden places, groups)
        let mut models: std::collections::BTreeMap<(u32, u32), (u32, u32, std::collections::BTreeSet<u16>)> = Default::default();
        let cells = (radius / CELL).ceil() as i32 + 1;
        let (cx, cy) = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        for gy in cy - cells..=cy + cells {
            for gx in cx - cells..=cx + cells {
                for index in world.grass_cells.get(&(gx, gy)).into_iter().flatten() {
                    let place = &catalog.grass[*index as usize];
                    let (dx, dy) = (place.position[0] - x, place.position[1] - y);
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    let entry = models.entry((place.first_subset, place.subset_count)).or_default();
                    entry.0 += 1;
                    entry.1 += hidden(&self.shown, place.group) as u32;
                    entry.2.insert(place.group);
                }
            }
        }
        println!("grass report ({x}, {y}) within {radius}: {} models, grass on: {}", models.len(), self.settings.grass);
        for ((first, count), (places, hidden_places, groups)) in &models {
            println!("  model at subset {first} ({count} subsets): {places} places, {hidden_places} hidden, groups {groups:?}");
            for subset in *first..first + count {
                let Some(s) = catalog.subsets.get(subset as usize) else { continue };
                let texture = catalog.textures.get(s.texture as usize).map_or("?", |name| name.as_str());
                println!(
                    "    subset {subset}: texture {} '{texture}', {} vertices, {} triangles, alpha {}, grass {}, on the GPU {}",
                    s.texture,
                    s.vertex_count,
                    s.triangle_count,
                    s.has_alpha,
                    s.grass,
                    self.resident.contains_key(&subset)
                );
            }
        }
    }

    /// Prints the work of the draws for each call of each set since the last report, and
    /// starts again: the command `statics report <label>` of `nirender.tune`.
    pub fn report(&mut self, label: &str) {
        for (name, stats) in SET_NAMES.iter().zip(self.set_stats.iter()).chain(core::iter::once((&"grass", &self.grass_stats))) {
            if stats.calls == 0 {
                continue;
            }
            let per = |n: u64| n as f64 / stats.calls as f64;
            println!(
                "statics report {label}: {name}: {} calls, {:.3} ms a call; places {:.0} visited, {:.0} out of their band, {:.0} out of the view, {:.0} not on the GPU, {:.0} drawn ({:.0} runs of one subset), {:.0} triangles",
                stats.calls,
                stats.seconds * 1000.0 / stats.calls as f64,
                per(stats.visited),
                per(stats.distance),
                per(stats.frustum),
                per(stats.waiting),
                per(stats.drawn),
                per(stats.runs),
                per(stats.triangles)
            );
        }
        if self.prepare_calls > 0 {
            println!(
                "statics report {label}: prepare: {} calls, {:.3} ms a call (the work of the frame and the culling of its sets)",
                self.prepare_calls,
                self.prepare_seconds * 1000.0 / self.prepare_calls as f64
            );
        }
        self.prepare_calls = 0;
        self.prepare_seconds = 0.0;
        if let (Some(gpu), Some(device), Some(queue)) = (self.gpu.as_ref(), self.device.as_ref(), self.queue.as_ref()) {
            if let Some(culling) = gpu.culling.as_ref() {
                for ((name, stats), counts) in SET_NAMES.iter().zip(self.set_stats.iter()).zip(culling.take_stats(device, queue)) {
                    if stats.calls == 0 {
                        continue;
                    }
                    let per = |n: u32| n as f64 / stats.calls as f64;
                    println!(
                        "statics report {label}: {name} (GPU): places {:.0} visited, {:.0} out of their band, {:.0} out of the view, {:.0} not on the GPU, {:.0} drawn, {:.0} triangles, {:.0} of hidden groups; pools {:.0} MB",
                        per(counts[0]),
                        per(counts[1]),
                        per(counts[2]),
                        per(counts[3]),
                        per(counts[4]),
                        (counts[5] as u64 | (counts[7] as u64) << 32) as f64 / stats.calls as f64,
                        per(counts[6]),
                        gpu.pools.bytes as f64 / 1e6
                    );
                }
            }
        }
        if self.play_arrivals > 0 {
            println!(
                "statics report {label}: arrivals: {} merged subsets in play ({} of them on the GPU before), {} inside the draw distance of {:.0}, {} of them past the handoff (shown late), the nearest at {:.0}",
                self.play_arrivals,
                self.play_again,
                self.late_arrivals,
                self.settings.draw_distance,
                self.late_shown,
                if self.late_arrivals > 0 { self.late_nearest } else { 0.0 }
            );
        }
        self.play_arrivals = 0;
        self.play_again = 0;
        self.late_arrivals = 0;
        self.late_shown = 0;
        self.late_nearest = f32::MAX;
        self.set_stats = [SetStats::default(); 4];
        self.grass_stats = SetStats::default();
    }

    /// The catalog is loaded.
    pub fn loaded(&self) -> bool {
        self.loading.is_none() && self.catalog.is_some()
    }

    /// The subsets near the camera are on the GPU: none is asked for from the reader thread,
    /// and none is wanted that the cap would let in. Subsets that wait for the cap count as
    /// there: they come when the camera comes nearer.
    pub fn settled(&self) -> bool {
        self.loaded() && self.requested.is_empty() && (self.wanted.is_empty() || self.cap_waiting)
    }

    /// Why the data was rejected, when it was: distant land must then be off.
    pub fn rejected(&self) -> Option<&str> {
        self.rejected.as_deref()
    }

    /// The state of the vis groups, from the record of MGE XE: a byte for each group, with
    /// the group 0 first; 0 is a hidden group. A record for another number of groups than
    /// the catalog has is from other files (the generator made them again after MGE XE read
    /// its groups): it is not used.
    pub fn set_groups(&mut self, shown: &[u8]) {
        self.groups_record = shown.to_vec();
        self.apply_groups();
    }

    fn apply_groups(&mut self) {
        let Some(catalog) = self.catalog.as_ref() else { return };
        if self.groups_record.len() == catalog.groups {
            self.shown = self.groups_record.clone();
            if let (Some(gpu), Some(device)) = (self.gpu.as_mut(), self.device.as_ref()) {
                if let Some(culling) = gpu.culling.as_mut() {
                    culling.set_groups(device, &self.shown);
                }
            }
            let hidden = self.shown.iter().skip(1).filter(|shown| **shown == 0).count();
            println!("distant statics: vis groups: {hidden} of {} hidden", catalog.groups - 1);
        } else if !self.groups_record.is_empty() {
            self.shown.clear();
            println!("distant statics: a record of {} vis groups for a catalog of {}; all are shown", self.groups_record.len(), catalog.groups);
        }
    }

    /// Sets the world space of the frame: the exterior with an empty name, or an interior
    /// cell with its name as the game has it. Returns true when the catalog has it; false
    /// before the catalog is read.
    pub fn set_world(&mut self, name: &[u8]) -> bool {
        if self.world != name {
            self.world = name.to_vec();
            self.request_world();
            self.find_here();
        }
        self.catalog.as_ref().is_some_and(|catalog| catalog.worlds.contains_key(name))
    }

    /// The merged subsets of the world space of the frame (`here`).
    fn find_here(&mut self) {
        self.here.clear();
        let Some(catalog) = self.catalog.as_ref() else { return };
        if let Some(world) = catalog.worlds.get(&self.world) {
            self.here.extend(world.merged.iter().map(|&index| catalog.instances[index as usize].subset));
        }
    }

    /// The distance from the camera to the place of a merged subset across the ground; the
    /// largest for a subset of another world space.
    fn distance_here(&self, catalog: &Catalog, subset: u32, eye: [f32; 3]) -> f32 {
        match (self.here.contains(&subset), catalog.merged_place.get(&subset)) {
            (true, Some(&index)) => flat_distance(&catalog.instances[index as usize], eye),
            _ => f32::INFINITY,
        }
    }

    /// Asks the reader thread for the ordinary subsets of the places of the world space of
    /// the frame that are not on the GPU. A merged subset comes when the camera is near it.
    fn request_world(&mut self) {
        let Some(catalog) = self.catalog.clone() else { return };
        let Some(requests) = self.requests.as_ref() else { return };
        let Some(world) = catalog.worlds.get(&self.world) else { return };
        let mut wanted: Vec<u32> = world.cells.values().chain(world.water_cells.values()).flatten().map(|i| catalog.instances[*i as usize].subset).filter(|s| catalog.subsets[*s as usize].components.is_empty()).collect();
        // The subsets of the grass too: they are never merged.
        wanted.extend(world.grass_cells.values().flatten().flat_map(|i| {
            let place = &catalog.grass[*i as usize];
            place.first_subset..place.first_subset + place.subset_count
        }));
        wanted.sort_unstable();
        wanted.dedup();
        for subset in wanted {
            if self.may_ask(subset) && self.requested.insert(subset) {
                let _ = requests.send(subset);
                if let Some(wait) = self.wait.as_mut() {
                    wait.asked += 1;
                    wait.asked_bytes += catalog.subsets[subset as usize].bytes();
                }
            }
        }
    }

    /// The first cap of the merged subsets: from the memory budget of the GPU, else
    /// `distant_land.statics_fallback_cap_mb`. `MGE_XE_STATICS_CAP_MB` gives a fixed cap,
    /// for a test.
    fn select_cap(&mut self, device: &wgpu::Device) {
        if let Some(mb) = std::env::var("MGE_XE_STATICS_CAP_MB").ok().and_then(|mb| mb.parse::<u64>().ok()) {
            self.merged_cap = mb << 20;
            println!("distant statics: the cap of the merged subsets is {mb} MB, from MGE_XE_STATICS_CAP_MB");
        } else if let Some((budget, used)) = memory_budget(device) {
            self.merged_cap = cap_from_budget(budget, used, self.merged_bytes);
            self.next_budget_sample = Some(Instant::now() + BUDGET_SAMPLE);
            println!(
                "distant statics: the cap of the merged subsets is {:.0} MB, from the memory budget of the GPU ({:.0} MB, {:.0} MB in use)",
                self.merged_cap as f64 / 1e6,
                budget as f64 / 1e6,
                used as f64 / 1e6
            );
        } else {
            self.merged_cap = self.fallback_cap;
            if self.fallback_cap == u64::MAX {
                println!("distant statics: the GPU gives no memory budget; no cap of the merged subsets (distant_land.statics_fallback_cap_mb = 0)");
            } else {
                println!("distant statics: the GPU gives no memory budget; the cap of the merged subsets is {} MB (distant_land.statics_fallback_cap_mb)", self.fallback_cap >> 20);
            }
        }
        self.first_cap = self.merged_cap;
        let high = self.high_mark();
        println!(
            "distant statics: residency {:?}, the high mark of the merged subsets {}",
            self.residency,
            if high >= u64::MAX / 2 { "none".to_string() } else { format!("{:.0} MB ({})", high as f64 / 1e6, if self.resident_mark > 0 { "distant_land.statics_resident_mb" } else { "half of the first cap" }) }
        );
    }

    /// A new sample of the memory budget each half second, when the cap follows it. The cap
    /// goes down to the fourth lower value in a row, not to the least of them, so that a
    /// short peak of another program does not hold it low; it goes up again to the twentieth
    /// higher value in a row, when the other program gave its memory back. A value counts as
    /// lower or higher only beyond the dead band of [`cap_move`].
    fn sample_budget(&mut self, device: &wgpu::Device) {
        let Some(next) = self.next_budget_sample else { return };
        let now = Instant::now();
        if now < next {
            return;
        }
        self.next_budget_sample = Some(now + BUDGET_SAMPLE);
        let Some((budget, used)) = memory_budget(device) else {
            self.lower_samples = 0;
            self.higher_samples = 0;
            return;
        };
        let candidate = cap_from_budget(budget, used, self.merged_bytes);
        let way = cap_move(self.merged_cap, candidate, budget);
        if way > 0 {
            self.lower_samples = 0;
            self.higher_samples += 1;
            if self.higher_samples >= HIGHER_SAMPLES {
                println!(
                    "distant statics: the cap of the merged subsets goes up from {:.0} MB to {:.0} MB (budget {:.0} MB, {:.0} MB in use)",
                    self.merged_cap as f64 / 1e6,
                    candidate as f64 / 1e6,
                    budget as f64 / 1e6,
                    used as f64 / 1e6
                );
                self.merged_cap = candidate;
                self.higher_samples = 0;
                self.raised += 1;
            }
            return;
        }
        self.higher_samples = 0;
        if way == 0 {
            self.lower_samples = 0;
            return;
        }
        self.lower_samples += 1;
        if self.lower_samples < LOWER_SAMPLES {
            return;
        }
        println!(
            "distant statics: the cap of the merged subsets goes down from {:.0} MB to {:.0} MB (budget {:.0} MB, {:.0} MB in use)",
            self.merged_cap as f64 / 1e6,
            candidate as f64 / 1e6,
            budget as f64 / 1e6,
            used as f64 / 1e6
        );
        self.merged_cap = candidate;
        self.lower_samples = 0;
        self.lowered += 1;
    }

    /// Takes a subset off the GPU: its buffers go, or its slot in a pool is free from the next
    /// frame on, and the culling no longer draws it.
    fn take_off(&mut self, subset: u32) -> Option<Resident> {
        let resident = self.resident.remove(&subset)?;
        self.resident_bytes -= resident.bytes;
        if let Geometry::Pool(slot) = resident.geometry {
            // The pool has room again: the subsets it refused may be asked for.
            if let Some(catalog) = self.catalog.as_ref() {
                self.refused.retain(|refused| catalog.subsets[*refused as usize].texture != slot.texture);
            }
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.pools.remove(slot);
                if let (Some(culling), Some(queue)) = (gpu.culling.as_ref(), self.queue.as_ref()) {
                    queue.write_buffer(&culling.subsets, subset as u64 * size_of::<cull::Subset>() as u64, bytemuck::bytes_of(&cull::Subset::default()));
                }
            }
        }
        Some(resident)
    }

    /// True when `subset` may be asked for from the reader thread: it is not on the GPU, not
    /// asked for already, and neither unreadable nor refused by a full pool.
    fn may_ask(&self, subset: u32) -> bool {
        !self.resident.contains_key(&subset) && !self.requested.contains(&subset) && !self.unreadable.contains(&subset) && !self.refused.contains(&subset)
    }

    /// Takes the farthest merged subsets off the GPU while their bytes are over the cap: after
    /// the cap went down.
    fn pay_cap_debt(&mut self, catalog: &Catalog, eye: [f32; 3]) {
        if self.merged_bytes <= self.merged_cap {
            return;
        }
        let mut on_gpu: Vec<(f32, u32)> = self.resident.keys().filter(|subset| catalog.merged_place.contains_key(subset)).map(|subset| (self.distance_here(catalog, *subset, eye), *subset)).collect();
        on_gpu.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, subset) in on_gpu {
            if self.merged_bytes <= self.merged_cap {
                break;
            }
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.evicted += 1;
            }
        }
    }

    /// Asks the reader thread for the merged subsets that the draws want, the nearest first,
    /// while the merged bytes on the GPU and asked for stay under the cap. A merged subset on
    /// the GPU that is further from the camera than the one asked for goes to make room; when
    /// there is none, the rest wait until the camera comes nearer.
    fn admit_merged(&mut self, catalog: &Catalog, eye: [f32; 3]) {
        self.pay_cap_debt(catalog, eye);
        if self.wanted.is_empty() {
            return;
        }
        let Some(requests) = self.requests.clone() else {
            self.wanted.clear();
            return;
        };
        let distance = |subset: u32| flat_distance(&catalog.instances[catalog.merged_place[&subset] as usize], eye);
        let mut pending = std::mem::take(&mut self.wanted);
        let mut wanted: Vec<(f32, u32, u64)> = pending.drain(..).filter(|subset| self.may_ask(*subset)).map(|subset| (distance(subset), subset, catalog.subsets[subset as usize].bytes())).collect();
        self.wanted = pending;
        wanted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        wanted.dedup_by_key(|w| w.1);
        let asked: u64 = self.requested.iter().filter(|subset| catalog.merged_place.contains_key(subset)).map(|subset| catalog.subsets[*subset as usize].bytes()).sum();
        let used = self.merged_bytes + asked;
        let (admit, evict, waiting) = if used + wanted.iter().map(|w| w.2).sum::<u64>() <= self.merged_cap {
            (wanted.iter().map(|w| w.1).collect(), Vec::new(), false)
        } else {
            let on_gpu = self.resident.iter().filter(|(subset, _)| catalog.merged_place.contains_key(subset)).map(|(subset, resident)| (self.distance_here(catalog, *subset, eye), *subset, resident.bytes)).collect();
            plan_merged(&wanted, on_gpu, used, self.merged_cap)
        };
        for subset in evict {
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.evicted += 1;
            }
        }
        self.cap_waiting = waiting && admit.is_empty();
        for subset in admit {
            self.requested.insert(subset);
            let _ = requests.send(subset);
            if let Some(wait) = self.wait.as_mut() {
                wait.asked += 1;
                wait.asked_bytes += catalog.subsets[subset as usize].bytes();
            }
        }
        self.capped_frames += waiting as u64;
    }

    fn finish_load(&mut self, scene: &SceneView) {
        let Some(view_layout) = self.view_layout.clone() else { return };
        let Some(receiver) = self.loading.as_ref() else { return };
        let result = if self.synchronous { receiver.recv().ok() } else { receiver.try_recv().ok() };
        let Some(result) = result else { return };
        self.loading = None;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(CatalogError::Missing(problem)) => {
                println!("distant statics: not there: {problem}");
                self.failed = true;
                return;
            }
            Err(CatalogError::Rejected(problem)) => {
                println!("distant statics: rejected: {problem}");
                self.failed = true;
                self.rejected = Some(problem);
                return;
            }
        };
        let catalog = loaded.catalog.clone();
        self.select_cap(scene.device);
        println!(
            "distant statics: {} subsets ({} of water, {} dry spaces), {} places in the exterior and {} interior cells, {} grass places, {} textures; {:.0} MB ordinary, {:.0} MB merged",
            catalog.subsets.len(),
            catalog.subsets.iter().filter(|s| matches!(s.water, 1 | 2)).count(),
            catalog.subsets.iter().filter(|s| s.water == 3).count(),
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
        self.device = Some(scene.device.clone());
        self.queue = Some(scene.queue.clone());
        self.gpu = Some(make_gpu(scene, &loaded, &view_layout, self.alpha_to_coverage));
        let (request_sender, request_receiver) = channel();
        let (arrival_sender, arrival_receiver) = channel();
        let for_reader = catalog.clone();
        thread::spawn(move || reader(for_reader, request_receiver, arrival_sender));
        self.requests = Some(request_sender);
        self.arrivals = Some(arrival_receiver);
        self.catalog = Some(catalog);
        // A record of the groups that came before the catalog.
        self.apply_groups();
        // All ordinary subsets that a place of the world space uses go to the GPU now.
        self.request_world();
        self.find_here();
    }

    /// Makes the buffers of the subsets that the reader thread has ready, for a part of a frame.
    fn take_arrivals(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, eye: [f32; 3]) {
        let Some(arrivals) = self.arrivals.as_ref() else { return };
        let (mut count, mut bytes) = (0, 0);
        // The places of the merged subsets that arrived in play, counted after the loop.
        let mut in_play = Vec::new();
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
            if !self.requested.remove(&arrival.subset) {
                continue;
            }
            let Some(catalog) = self.catalog.as_ref() else { continue };
            let s = &catalog.subsets[arrival.subset as usize];
            if arrival.unreadable {
                if self.unreadable.is_empty() {
                    println!("distant statics: subset {} cannot be read from statics/static_meshes_{:03}; an unreadable subset is not drawn and not asked for again (the count is in the summary)", arrival.subset, s.shard);
                }
                self.unreadable.insert(arrival.subset);
                continue;
            }
            if arrival.indexes.is_empty() {
                continue;
            }
            let size = (arrival.vertices.len() + arrival.indexes.len()) as u64;
            let geometry = if s.pooled() {
                let Some(gpu) = self.gpu.as_mut() else { continue };
                let Some(slot) = gpu.pools.put(device, queue, s.texture, &arrival.vertices, &arrival.indexes) else {
                    if !self.pool_full {
                        self.pool_full = true;
                        println!("distant statics: the pool of a texture cannot grow for a subset of {size} bytes; it is not drawn until a slot of the pool goes free");
                    }
                    self.refused.insert(arrival.subset);
                    self.refused_count += 1;
                    continue;
                };
                if let Some(culling) = gpu.culling.as_ref() {
                    let entry = cull::Subset { first_index: slot.first_index, base_vertex: slot.base_vertex, faces: arrival.faces, resident: 1, _pad: [0; 2] };
                    queue.write_buffer(&culling.subsets, arrival.subset as u64 * size_of::<cull::Subset>() as u64, bytemuck::bytes_of(&entry));
                }
                Geometry::Pool(slot)
            } else {
                let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("distant static, vertices"), contents: &arrival.vertices, usage: wgpu::BufferUsages::VERTEX });
                let indexes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("distant static, indexes"), contents: &arrival.indexes, usage: wgpu::BufferUsages::INDEX });
                Geometry::Own { vertices, indexes }
            };
            let arrived = if self.prefetching { -1.0e9 } else { self.clock.elapsed().as_secs_f32() };
            if let Some(gpu) = self.gpu.as_ref() {
                queue.write_buffer(&gpu.arrivals, arrival.subset as u64 * 4, bytemuck::bytes_of(&arrived));
            }
            if s.grass {
                self.grass_arrived += 1;
                self.grass_bent += arrival.bent as u64;
            }
            let wanted_at = self.clock.elapsed().as_secs_f32();
            self.resident.insert(arrival.subset, Resident { geometry, faces: arrival.faces, bytes: size, arrived, wanted_at, bent: arrival.bent });
            self.resident_bytes += size;
            self.most_bytes = self.most_bytes.max(self.resident_bytes);
            let again = !self.ever_arrived.insert(arrival.subset);
            if let Some(wait) = self.wait.as_mut() {
                wait.arrived += 1;
                wait.arrived_bytes += size;
                wait.again += again as u32;
            }
            if let Some(&index) = self.catalog.as_ref().and_then(|catalog| catalog.merged_place.get(&arrival.subset)) {
                self.merged_bytes += size;
                self.most_merged = self.most_merged.max(self.merged_bytes);
                if !self.prefetching {
                    in_play.push(index);
                    self.play_again += again as u64;
                }
            }
            count += 1;
            bytes += size as usize;
        }
        for index in in_play {
            self.count_arrival(index, eye);
        }
    }

    /// Counts a merged subset that arrived in play by the distance of its place: inside the
    /// draw distance it came late, and past the handoff the view shows it late.
    fn count_arrival(&mut self, index: u32, eye: [f32; 3]) {
        let Some(catalog) = self.catalog.as_ref() else { return };
        let instance = &catalog.instances[index as usize];
        let near = flat_distance(instance, eye) - instance.radius;
        self.play_arrivals += 1;
        if near >= self.settings.draw_distance {
            return;
        }
        self.late_arrivals += 1;
        self.late_nearest = self.late_nearest.min(near.max(0.0));
        let gone_before = self.game_range - HANDOFF - HANDOFF_BAND - HANDOFF_CULL;
        if flat_distance(instance, eye) + instance.radius >= gone_before {
            self.late_shown += 1;
        }
    }

    /// Draws the grass of the frame into the pass: the places within the view range of the
    /// game that are in the view, one draw for each subset.
    pub fn draw_grass(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &GrassFrame) {
        if self.failed || !self.settings.grass {
            return;
        }
        let started = Instant::now();
        self.draw_grass_timed(scene, pass, frame);
        self.grass_stats.calls += 1;
        self.grass_stats.seconds += started.elapsed().as_secs_f64();
        self.grass_stats.drawn += self.grass_draws.len() as u64;
    }

    fn draw_grass_timed(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &GrassFrame) {
        // The shadow map is made once in a session: the group takes it at its first frame.
        if let (Some(shadow), Some(gpu)) = (frame.shadow.as_ref(), self.gpu.as_mut()) {
            if !std::rc::Rc::ptr_eq(&gpu.grass_atlas, &shadow.atlas) {
                gpu.grass_group = make_grass_group(scene.device, &gpu.grass_layout, &gpu.grass_uniforms, &gpu.grass_sampler, &shadow.atlas, &gpu.grass_shadow_sampler);
                gpu.grass_atlas = shadow.atlas.clone();
            }
        }
        let (Some(catalog), Some(gpu)) = (self.catalog.clone(), self.gpu.as_ref()) else { return };
        let Some(view) = self.view_groups.get(super::view::GAME) else { return };
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
                    if hidden(&self.shown, place.group) {
                        continue;
                    }
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
                        let Some(bent) = self.resident.get(&subset).map(|r| r.bent) else {
                            continue;
                        };
                        if self.grass_draws.len() == GRASS_RECORDS {
                            cut = true;
                            break 'cells;
                        }
                        let fade = self.fade_of(subset);
                        self.grass_draws.push((catalog.subsets[subset as usize].texture, subset, record, fade, bent as u32 as f32));
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
        let records: Vec<f32> = self.grass_draws.iter().flat_map(|draw| draw.2.into_iter().chain([draw.3, draw.4, 0.0, 0.0])).collect();
        scene.queue.write_buffer(&gpu.grass_records, 0, bytemuck::cast_slice(&records));
        let uniforms = GrassUniforms {
            range: [view_range, 0.0, 0.0, 0.0],
            fog_color: frame.fog_color,
            sun_color: frame.sun_color,
            ambient: frame.ambient,
            wind: frame.wind,
            foot: frame.foot,
            shadow_layers: frame.shadow.as_ref().map_or([[0.0; 16]; 2], |shadow| shadow.layers),
            esm: [super::shadow::ESM_C, super::shadow::ESM_BIAS, super::shadow::ESM_SCALE, frame.shadow.as_ref().map_or(0.0, |shadow| shadow.rcp)],
            shade: [super::shadow::SHADE_COLOR[0], super::shadow::SHADE_COLOR[1], super::shadow::SHADE_COLOR[2], super::shadow::SHADE],
            shadow: frame.shadow.as_ref().map_or([0.0; 4], |shadow| {
                let c = frame.sun_color;
                let luminance = c[0] * 0.36 + c[1] * 0.53 + c[2] * 0.11;
                [1.0, luminance * (0.25 + 0.75 * shadow.sun_vis), shadow.above_sea as u32 as f32, shadow.edge as u32 as f32]
            }),
            look: frame.look,
        };
        scene.queue.write_buffer(&gpu.grass_uniforms, 0, bytemuck::bytes_of(&uniforms));

        pass.set_pipeline(if scene.wireframe { gpu.grass_pipeline_lines.as_ref().unwrap_or(&gpu.grass_pipeline) } else { &gpu.grass_pipeline });
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, &gpu.grass_group, &[]);
        pass.set_vertex_buffer(1, gpu.grass_records.slice(..));
        // One draw for each run of places with the same subset.
        let mut start = 0;
        while start < self.grass_draws.len() {
            let (texture, subset, ..) = self.grass_draws[start];
            let mut end = start + 1;
            while end < self.grass_draws.len() && self.grass_draws[end].1 == subset {
                end += 1;
            }
            if let Some(Resident { geometry: Geometry::Own { vertices, indexes }, faces, .. }) = self.resident.get(&subset) {
                pass.set_bind_group(2, &gpu.textures[texture as usize], &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..faces[0] * 3, 0, start as u32..end as u32);
            }
            start = end;
        }
        self.grass_drawn += self.grass_draws.len() as u64;
    }

    /// The time that a subset takes to come in after its arrival, in seconds (the tunable
    /// `statics.fade`); 0 for none.
    pub fn set_fade(&mut self, seconds: f32) {
        self.fade_seconds = seconds.max(0.0);
    }

    /// The fade of the shaders: the time now on the clock of the arrivals, and the fade
    /// time. A replay has no fade: its pictures must not depend on the time of the machine.
    fn fade_values(&self) -> [f32; 4] {
        [self.clock.elapsed().as_secs_f32(), if self.synchronous { 0.0 } else { self.fade_seconds }, 0.0, 0.0]
    }

    /// The share of a subset that is in, as the shaders have it.
    fn fade_of(&self, subset: u32) -> f32 {
        let [now, seconds, _, _] = self.fade_values();
        match self.resident.get(&subset) {
            Some(resident) if seconds > 0.0 => ((now - resident.arrived) / seconds).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    /// The statics and the grass on or off, from the settings record of MGE XE. A kind
    /// that the settings had off at the start is not in the catalog, and stays off. Statics
    /// that are off are not in the shadow map either.
    pub fn set_enabled(&mut self, statics: bool, grass: bool) {
        self.settings.statics = statics;
        self.settings.grass = grass;
    }

    /// The draw distance and the ends of the three tiers, in units, from the settings
    /// record of MGE XE; [`None`] keeps an end. The draw of a frame reads them, as the
    /// Direct3D 9 path reads them in each frame. The tier of a place stays the one of the
    /// load (the sizes of the tiers are for the load only).
    pub fn set_distances(&mut self, draw_distance: f32, band_ends: [Option<f32>; 3]) {
        self.settings.draw_distance = draw_distance;
        for (end, new) in self.settings.band_ends.iter_mut().zip(band_ends) {
            if let Some(new) = new {
                *end = new;
            }
        }
    }

    /// The water among the distant statics of the frame: the water subsets within the
    /// draw distance that are in the view (`planes`), with their buffers. A merged one comes
    /// to the GPU as a merged static does. `loaded` is the range of x and y of the cells that
    /// the game has loaded: the game draws the meshes of their places near the camera.
    pub fn far_water(&mut self, eye: [f32; 3], planes: &[[f32; 4]; 6], loaded: [f32; 4]) -> Vec<FarWater> {
        let mut found = Vec::new();
        if self.failed || !self.settings.statics {
            return found;
        }
        let Some(catalog) = self.catalog.clone() else { return found };
        let Some(world) = catalog.worlds.get(&self.world) else { return found };
        let reach = self.settings.draw_distance;
        let admit = reach + CELL;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = ((reach + CELL) / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = world.water_cells.get(&(x, y)) else { continue };
                for index in list {
                    let instance = &catalog.instances[*index as usize];
                    if hidden(&self.shown, instance.group) {
                        continue;
                    }
                    let subset = &catalog.subsets[instance.subset as usize];
                    let Some(look) = subset.look else { continue };
                    let to = [instance.center[0] - eye[0], instance.center[1] - eye[1], instance.center[2] - eye[2]];
                    if !subset.components.is_empty() && (to[0] * to[0] + to[1] * to[1]).sqrt() <= admit + instance.radius && self.may_ask(instance.subset) {
                        self.wanted.push(instance.subset);
                    }
                    let Some(Resident { geometry: Geometry::Own { vertices, indexes }, faces, .. }) = self.resident.get(&instance.subset) else { continue };
                    let range = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                    if range > reach + instance.radius {
                        continue;
                    }
                    if !planes.iter().all(|p| p[0] * instance.center[0] + p[1] * instance.center[1] + p[2] * instance.center[2] + p[3] >= -instance.radius) {
                        continue;
                    }
                    // The place of the reference is the translation of its world matrix.
                    let (px, py) = (instance.world[12], instance.world[13]);
                    found.push(FarWater {
                        vertices: vertices.clone(),
                        indexes: indexes.clone(),
                        triangles: faces[0],
                        world: instance.world,
                        look,
                        loaded: px >= loaded[0] && py >= loaded[1] && px < loaded[2] && py < loaded[3],
                        texture: self.gpu.as_ref().and_then(|gpu| gpu.texture_views.get(subset.texture as usize).cloned()),
                    });
                }
            }
        }
        found
    }

    /// The view groups for the draws of this frame, one for each set (`view::MAIN` and the
    /// others have the numbers of the sets).
    pub fn set_views(&mut self, views: &super::view::ViewBinding) {
        if self.view_layout.is_none() {
            self.view_layout = Some(views.layout.clone());
        }
        self.view_groups.clear();
        self.view_groups.extend_from_slice(views.groups());
    }

    /// Chooses the places of the sets of a frame on the GPU, in one command buffer before the
    /// passes that draw them, and does the work of the frame for the statics once: the main
    /// view first. A set chosen here is only drawn by `draw`; a set that is not here is chosen
    /// in its `draw`, as without this call.
    pub fn prepare(&mut self, scene: &SceneView, sets: &[CullSet]) {
        self.prepared = [false; 4];
        let Some(first) = sets.first() else { return };
        if self.failed || !self.settings.statics {
            return;
        }
        let started = Instant::now();
        self.game_range = first.eye[3];
        if self.frame_work(scene, [first.eye[0], first.eye[1], first.eye[2]], true) {
            let catalog = self.catalog.clone();
            if let (Some(catalog), Some(gpu)) = (catalog, self.gpu.as_mut()) {
                if let (Some(culling), Some(world)) = (gpu.culling.as_mut(), catalog.worlds.get(&self.world)) {
                    if !culling.has_world(&self.world) {
                        culling.set_world(scene.device, &self.world, &world_places(&catalog, world));
                    }
                    let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("distant statics, cull") });
                    let at = scene.commands.stamps().begin(&mut encoder, "distant statics: the GPU cull of all sets", nirender_wgpu::stamps::StampKind::Detail);
                    for cull_set in sets {
                        let values = cull_values(&self.settings, cull_set.planes, cull_set.eye, cull_set.fog_end);
                        culling.encode(scene.queue, &mut encoder, cull_set.set, &values);
                        if let Some(prepared) = self.prepared.get_mut(cull_set.set) {
                            *prepared = true;
                        }
                    }
                    scene.commands.stamps().end(&mut encoder, at);
                    scene.commands.defer(encoder.finish());
                }
            }
        }
        self.prepare_calls += 1;
        self.prepare_seconds += started.elapsed().as_secs_f64();
    }

    /// Draws the statics of the frame into the pass. Returns the number of draws.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &Frame, set: usize) -> u32 {
        let started = Instant::now();
        let draws = self.draw_timed(scene, pass, frame, set);
        if let Some(stats) = self.set_stats.get_mut(set) {
            stats.calls += 1;
            stats.seconds += started.elapsed().as_secs_f64();
        }
        draws
    }

    /// The work of a frame before its draws: the load, the slots that are free again, the
    /// memory budget, the merged subsets that come and go, the arrivals. False when there is
    /// nothing to draw in the world space of the frame.
    /// The merged subsets near enough to `eye` come to the GPU: the ones that are not there
    /// and not asked for go into `wanted`.
    /// A merged subset on the GPU that is near enough is wanted now (`Resident::wanted_at`).
    /// With "all", every merged subset of the world space is wanted once, when they fit
    /// under the high mark.
    fn want_near(&mut self, catalog: &Catalog, eye: [f32; 3]) {
        let Some(world) = catalog.worlds.get(&self.world) else { return };
        let admit = self.settings.draw_distance + CELL;
        let now = self.clock.elapsed().as_secs_f32();
        let all = self.residency == StaticsResidency::All && self.all_world.as_deref() != Some(self.world.as_slice());
        if all {
            self.all_world = Some(self.world.clone());
            let bytes: u64 = world.merged.iter().map(|&index| catalog.subsets[catalog.instances[index as usize].subset as usize].bytes()).sum();
            let high = self.high_mark();
            if bytes <= high {
                println!("distant statics: residency all: the {:.0} MB of merged subsets of the world space fit under the high mark of {:.0} MB; all are asked for", bytes as f64 / 1e6, high as f64 / 1e6);
            } else {
                println!("distant statics: residency all: the {:.0} MB of merged subsets of the world space do not fit under the high mark of {:.0} MB; as auto", bytes as f64 / 1e6, high as f64 / 1e6);
            }
            if bytes <= high {
                // The pools grow once to hold them all, not in doublings (with doublings
                // 3175 MB of subsets took 4720 MB of pools, records entry 164).
                let mut room: HashMap<u32, (u32, u32)> = HashMap::new();
                for &index in &world.merged {
                    let instance = &catalog.instances[index as usize];
                    if !hidden(&self.shown, instance.group) && self.may_ask(instance.subset) {
                        self.wanted.push(instance.subset);
                        let s = &catalog.subsets[instance.subset as usize];
                        if s.pooled() {
                            let entry = room.entry(s.texture).or_default();
                            entry.0 += s.vertex_count;
                            entry.1 += (s.triangle_count * 3 + 1) & !1;
                        }
                    }
                }
                // The subsets asked for and not yet there take room in the same pools: the
                // ordinary subsets of the world space come in the same seconds.
                for &subset in &self.requested {
                    let s = &catalog.subsets[subset as usize];
                    if s.pooled() {
                        let entry = room.entry(s.texture).or_default();
                        entry.0 += s.vertex_count;
                        entry.1 += (s.triangle_count * 3 + 1) & !1;
                    }
                }
                if let (Some(gpu), Some(device), Some(queue)) = (self.gpu.as_mut(), self.device.as_ref(), self.queue.as_ref()) {
                    for (texture, (vertices, indexes)) in room {
                        gpu.pools.reserve(device, queue, texture, vertices, indexes);
                    }
                }
            }
        }
        for &index in &world.merged {
            let instance = &catalog.instances[index as usize];
            if hidden(&self.shown, instance.group) || flat_distance(instance, eye) > admit + instance.radius {
                continue;
            }
            if let Some(resident) = self.resident.get_mut(&instance.subset) {
                resident.wanted_at = now;
            } else if self.may_ask(instance.subset) {
                self.wanted.push(instance.subset);
            }
        }
    }

    /// The most bytes of merged subsets that stay on the GPU before the trim takes some off:
    /// `distant_land.statics_resident_mb`, or half of the first cap of the session; never over
    /// the cap as it is now (another program that takes memory lowers it).
    fn high_mark(&self) -> u64 {
        let mark = if self.resident_mark > 0 { self.resident_mark } else { self.first_cap / 2 };
        mark.min(self.merged_cap)
    }

    /// With "auto" and "all": when the merged subsets on the GPU pass the high mark, the ones
    /// wanted least recently go (the farthest first among equals) down to the low mark. A
    /// subset wanted in the last minute or within the draw distance and one cell stays.
    fn trim_resident(&mut self, catalog: &Catalog, eye: [f32; 3]) {
        let high = self.high_mark();
        if self.merged_bytes <= high {
            return;
        }
        let low = high / 20 * LOW_MARK_TWENTIETHS;
        let now = self.clock.elapsed().as_secs_f32();
        let admit = self.settings.draw_distance + CELL;
        let candidates: Vec<(f32, f32, u32, u64)> = self
            .resident
            .iter()
            .filter_map(|(subset, resident)| {
                let instance = &catalog.instances[*catalog.merged_place.get(subset)? as usize];
                let distance = self.distance_here(catalog, *subset, eye);
                (now - resident.wanted_at > KEEP_SECONDS && distance > admit + instance.radius).then_some((resident.wanted_at, distance, *subset, resident.bytes))
            })
            .collect();
        for subset in pick_trim(candidates, self.merged_bytes, low) {
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.trimmed += 1;
            }
        }
    }

    /// The work of a frame with no world (a loading screen) for the place where the camera
    /// will be: the world space by its name (empty for the exterior) and the eye. The
    /// subsets near it are asked for and taken as in a frame, so a wait on the loading screen
    /// ends when they are there (`settled`).
    pub fn prefetch(&mut self, scene: &SceneView, world: &[u8], eye: [f32; 3]) {
        // At the first load of a session the catalog comes in a frame with no world.
        self.finish_load(scene);
        if !self.loaded() {
            return;
        }
        let wait = self.wait.get_or_insert_default();
        wait.frames += 1;
        self.set_world(world);
        let Some(catalog) = self.catalog.clone() else { return };
        self.admit_merged(&catalog, eye);
        self.prefetching = true;
        let started = Instant::now();
        self.take_arrivals(scene.device, scene.queue, eye);
        let seconds = started.elapsed().as_secs_f64();
        if let Some(wait) = self.wait.as_mut() {
            wait.upload_seconds += seconds;
            wait.longest_upload = wait.longest_upload.max(seconds);
        }
        self.prefetching = false;
        self.want_near(&catalog, eye);
    }

    /// The work of the wait on the loading screen that ends, as a text for the log, and the
    /// start of the next one. None when no frame with no world came.
    pub fn take_wait(&mut self) -> Option<String> {
        let wait = self.wait.take()?;
        Some(format!(
            "{} frames with no world, the arrivals in them {:.1} ms (the longest frame {:.1} ms); {} subsets asked for ({:.1} MB), {} arrived ({:.1} MB), {} of them on the GPU before",
            wait.frames,
            wait.upload_seconds * 1000.0,
            wait.longest_upload * 1000.0,
            wait.asked,
            wait.asked_bytes as f64 / 1e6,
            wait.arrived,
            wait.arrived_bytes as f64 / 1e6,
            wait.again
        ))
    }

    fn frame_work(&mut self, scene: &SceneView, eye: [f32; 3], main: bool) -> bool {
        self.finish_load(scene);
        // The slots that went in the frame before are free: the draws that read them are done.
        if self.frame_ended {
            self.frame_ended = false;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.pools.next_frame();
            }
        }
        // The merged subsets that the draws before wanted are asked for before the arrivals
        // are taken: a subset that the shadow map wants comes in time for the main view of the
        // same frame. The budget is sampled once in a frame, with the main view.
        if let Some(catalog) = self.catalog.clone() {
            if main {
                self.sample_budget(scene.device);
            }
            self.admit_merged(&catalog, eye);
        }
        self.take_arrivals(scene.device, scene.queue, eye);
        let Some(catalog) = self.catalog.clone() else { return false };
        if !catalog.worlds.contains_key(&self.world) {
            return false;
        }
        self.want_near(&catalog, eye);
        let retain = self.settings.draw_distance + CELL + CELL;
        // With "auto" and "all" the merged subsets stay until the high mark (`trim_resident`).
        if self.frames % 30 == 0 && self.residency != StaticsResidency::Near {
            self.trim_resident(&catalog, eye);
        }
        // With "near" the merged subsets that are too far go away. The check is for all of
        // them, because the camera can go a long way in one frame.
        if self.frames % 30 == 0 && self.residency == StaticsResidency::Near {
            let far: Vec<u32> = self
                .resident
                .keys()
                .copied()
                .filter(|subset| {
                    catalog.merged_place.get(subset).is_some_and(|index| {
                        let instance = &catalog.instances[*index as usize];
                        flat_distance(instance, eye) > retain + instance.radius
                    })
                })
                .collect();
            for subset in far {
                if let Some(resident) = self.take_off(subset) {
                    self.merged_bytes -= resident.bytes;
                }
            }
        }
        self.frames += 1;
        if main {
            self.frame_ended = true;
        }
        true
    }

    /// Binds what the draws of a set read: its pipeline, its uniforms and the places.
    fn bind_set(gpu: &Gpu, view_group: Option<&wgpu::BindGroup>, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &Frame, set: usize, fade: [f32; 4]) {
        let buffers = &gpu.sets[set];
        if let Some(view_group) = view_group {
            pass.set_bind_group(0, view_group, &[]);
        }
        scene.queue.write_buffer(&buffers.uniforms, 0, bytemuck::bytes_of(&uniforms_of(frame, fade)));
        // Lines when the game draws in wireframe (see `SceneView::wireframe`), as MGE XE on
        // Direct3D 9 draws with the fill mode the engine left; the shadow map stays filled.
        let (fill, lines) = if set == 1 { (&gpu.mirror_pipeline, &gpu.mirror_pipeline_lines) } else { (&gpu.pipeline, &gpu.pipeline_lines) };
        pass.set_pipeline(if set >= 2 { &gpu.shadow_pipeline } else if scene.wireframe { lines.as_ref().unwrap_or(fill) } else { fill });
        pass.set_bind_group(1, &buffers.group, &[]);
        pass.set_vertex_buffer(1, gpu.instances.slice(..));
    }

    /// The indirect draws of a set that the GPU chose.
    fn draw_chosen(gpu: &Gpu, view_group: Option<&wgpu::BindGroup>, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &Frame, set: usize, fade: [f32; 4]) -> u32 {
        let Some(culling) = gpu.culling.as_ref() else { return 0 };
        Self::bind_set(gpu, view_group, scene, pass, frame, set, fade);
        let (pools, textures) = (&gpu.pools, &gpu.textures);
        culling.draw(set, pass, |pass, texture| {
            let Some((vertices, indexes)) = pools.buffers(texture) else { return false };
            pass.set_bind_group(2, &textures[texture as usize], &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
            true
        })
    }

    fn draw_timed(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame: &Frame, set: usize) -> u32 {
        if self.failed || !self.settings.statics {
            return 0;
        }
        let fade = self.fade_values();
        // Chosen in `prepare`: only the draws.
        if self.prepared.get(set).copied().unwrap_or(false) {
            self.prepared[set] = false;
            let Some(gpu) = self.gpu.as_ref() else { return 0 };
            return Self::draw_chosen(gpu, self.view_groups.get(set), scene, pass, frame, set, fade);
        }
        let eye = [frame.eye[0], frame.eye[1], frame.eye[2]];
        if !self.frame_work(scene, eye, set == 0) {
            return 0;
        }
        let Some(catalog) = self.catalog.clone() else { return 0 };
        let Some(world) = catalog.worlds.get(&self.world) else { return 0 };
        let settings = self.settings;

        // The places chosen on the GPU, in a command buffer of this set.
        if let Some(gpu) = self.gpu.as_mut() {
            if let Some(culling) = gpu.culling.as_mut() {
                if !culling.has_world(&self.world) {
                    culling.set_world(scene.device, &self.world, &world_places(&catalog, world));
                }
                culling.run(scene, set, &cull_values(&settings, frame.planes, frame.eye, frame.fog_end));
                return Self::draw_chosen(gpu, self.view_groups.get(set), scene, pass, frame, set, fade);
            }
        }

        // The places chosen on the CPU.
        let gone_before = frame.eye[3] - HANDOFF - HANDOFF_BAND - HANDOFF_CULL;
        let admit = settings.draw_distance + CELL;
        self.draws.clear();
        let mut stats = SetStats::default();
        let reach = settings.band_ends[2].max(admit) + CELL;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = (reach / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = world.cells.get(&(x, y)) else { continue };
                for index in list {
                    let instance = &catalog.instances[*index as usize];
                    if hidden(&self.shown, instance.group) {
                        self.hidden_skipped += 1;
                        continue;
                    }
                    stats.visited += 1;
                    let subset = &catalog.subsets[instance.subset as usize];
                    let to = [instance.center[0] - eye[0], instance.center[1] - eye[1], instance.center[2] - eye[2]];
                    let range = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                    let Some(resident) = self.resident.get(&instance.subset) else {
                        stats.waiting += 1;
                        continue;
                    };
                    // The band of the tier of the place, and the end of the fog.
                    let band_end = settings.band_ends[instance.tier as usize].min(frame.fog_end);
                    if range > band_end + instance.radius || range + instance.radius < gone_before {
                        stats.distance += 1;
                        continue;
                    }
                    if !frame.planes.iter().all(|p| p[0] * instance.center[0] + p[1] * instance.center[1] + p[2] * instance.center[2] + p[3] >= -instance.radius) {
                        stats.frustum += 1;
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
                        stats.triangles += faces as u64;
                    }
                }
            }
        }
        self.draws.sort_unstable();
        stats.drawn = self.draws.len() as u64;
        stats.runs = self.draws.iter().zip(self.draws.iter().skip(1)).filter(|(a, b)| (a.0, a.1) != (b.0, b.1)).count() as u64 + (!self.draws.is_empty()) as u64;
        if let Some(total) = self.set_stats.get_mut(set) {
            for (sum, add) in [
                (&mut total.visited, stats.visited),
                (&mut total.distance, stats.distance),
                (&mut total.frustum, stats.frustum),
                (&mut total.waiting, stats.waiting),
                (&mut total.drawn, stats.drawn),
                (&mut total.triangles, stats.triangles),
                (&mut total.runs, stats.runs),
            ] {
                *sum += add;
            }
        }
        if self.draws.is_empty() {
            return 0;
        }
        let Some(gpu) = self.gpu.as_ref() else { return 0 };
        Self::bind_set(gpu, self.view_groups.get(set), scene, pass, frame, set, fade);
        let mut texture = u32::MAX;
        for &(draw_texture, subset, instance, faces) in &self.draws {
            let Some(Resident { geometry: Geometry::Pool(slot), .. }) = self.resident.get(&subset) else { continue };
            if draw_texture != texture {
                let Some((vertices, indexes)) = gpu.pools.buffers(draw_texture) else { continue };
                texture = draw_texture;
                pass.set_bind_group(2, &gpu.textures[texture as usize], &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
            }
            pass.draw_indexed(slot.first_index..slot.first_index + faces * 3, slot.base_vertex as i32, instance..instance + 1);
        }
        self.drawn += self.draws.len() as u64;
        self.draws.len() as u32
    }
}

/// The places of the statics of a world space for the culling on the GPU: texture and place,
/// sorted by texture, subset and place (the order of the draws of the CPU).
fn world_places(catalog: &Catalog, world: &World) -> Vec<(u32, u32)> {
    let mut places: Vec<(u32, u32, u32)> = world
        .cells
        .values()
        .flatten()
        .map(|&index| {
            let subset = catalog.instances[index as usize].subset;
            (catalog.subsets[subset as usize].texture, subset, index)
        })
        .collect();
    places.sort_unstable();
    places.iter().map(|&(texture, _, index)| (texture, index)).collect()
}

/// The values of the culling of a set: the planes, the eye with the view range of the game in
/// w, the ends of the tiers and of the fog, and the handoff.
fn cull_values(settings: &Settings, planes: [[f32; 4]; 6], eye: [f32; 4], fog_end: f32) -> cull::CullUniforms {
    cull::CullUniforms {
        planes,
        eye: [eye[0], eye[1], eye[2], 0.0],
        bands: [settings.band_ends[0], settings.band_ends[1], settings.band_ends[2], fog_end],
        limits: [eye[3] - HANDOFF - HANDOFF_BAND - HANDOFF_CULL, 0.0, 0.0, 0.0],
        counts: [0; 4],
    }
}

/// The values of the draws of a set (group 1).
fn uniforms_of(frame: &Frame, fade: [f32; 4]) -> SetUniforms {
    SetUniforms { range: [frame.eye[3], 0.0, 0.0, 0.0], handoff: [frame.time, frame.eye[3] - HANDOFF, HANDOFF_BAND, HANDOFF_CULL], fade }
}

/// A set for [`Statics::prepare`]: its number (0 the main view, 1 the mirror, 2 and 3 the
/// layers of the shadow map), the planes of its view, the eye with the view range of the game
/// in w (0 for a set without the handoff), and the end of its fog.
pub struct CullSet {
    pub set: usize,
    pub planes: [[f32; 4]; 6],
    pub eye: [f32; 4],
    pub fog_end: f32,
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
            "distant statics: at most {:.0} MB of merged subsets on the GPU, the cap at the end {}; the cap went down {} times and up {} times, {} subsets taken off for nearer ones, {} frames in which some waited; residency {:?}, the high mark {}, {} subsets trimmed",
            self.most_merged as f64 / 1e6,
            if self.merged_cap == u64::MAX { "none".to_string() } else { format!("{:.0} MB", self.merged_cap as f64 / 1e6) },
            self.lowered,
            self.raised,
            self.evicted,
            self.capped_frames,
            self.residency,
            if self.high_mark() >= u64::MAX / 2 { "none".to_string() } else { format!("{:.0} MB", self.high_mark() as f64 / 1e6) },
            self.trimmed
        );
        if !self.unreadable.is_empty() || self.refused_count > 0 {
            println!("distant statics: {} subsets could not be read, {} arrivals found no room in a full pool", self.unreadable.len(), self.refused_count);
        }
        println!(
            "grass: {} frames, {:.0} records in a frame as a mean, {} frames with more than {GRASS_RECORDS}; {} subsets arrived, {} of them with bent normals",
            self.grass_frames,
            self.grass_drawn as f64 / self.grass_frames.max(1) as f64,
            self.grass_cut,
            self.grass_arrived,
            self.grass_bent
        );
        println!("distant statics: {} places of hidden vis groups not drawn", self.hidden_skipped);
        self.report("end");
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

fn make_gpu(scene: &SceneView, loaded: &Loaded, view_layout: &wgpu::BindGroupLayout, alpha_to_coverage: bool) -> Gpu {
    let device = scene.device;
    let catalog = &loaded.catalog;
    let make_uniforms = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics"),
            size: size_of::<SetUniforms>() as u64,
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
    // Long ago: a subset that is there from the start of a replay is in.
    let arrivals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the arrivals"),
        contents: bytemuck::cast_slice(&vec![-1.0e9f32; catalog.subsets.len().max(1)]),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
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
        label: Some("distant statics"),
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
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant statics, textures"),
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
            label: Some("distant statics"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: palettes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: arrivals.as_entire_binding() },
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
    let mut texture_views = Vec::with_capacity(catalog.textures.len());
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
            label: Some("distant statics, textures"),
            layout: &texture_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        }));
        texture_views.push(view);
    }
    if !missing.is_empty() {
        println!("distant statics: {} textures are white: {}", missing.len(), missing.iter().take(6).cloned().collect::<Vec<_>>().join("; "));
    }

    let source = format!("{}{SET_WGSL}{SHADER}{}", super::view::wgsl(), super::FOG_FUNCTION);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant statics"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    // Group 0: the view group of the set (`view::ViewBinding`); group 1: the values of the
    // set; group 2: the textures of a draw.
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("distant statics"),
        bind_group_layouts: &[Some(view_layout), Some(&layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    // A vertex of a statics file has 20 bytes: the position and the palette number as four
    // half floats, the normal, the colour, and the texture coordinate as two half floats.
    let vertex = wgpu::vertex_attr_array![0 => Float16x4, 1 => Unorm8x4, 2 => Unorm8x4, 3 => Float16x2];
    let place = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4];
    // The statics of the frame go into the pass of the game (its samples, the alpha as the
    // coverage with `alpha_to_coverage`; the alpha of the back buffer stays 1, as a target
    // of the game without alpha gives). The mirror of the water is a texture of this
    // program, with one sample.
    let make_pipeline = |cull: wgpu::Face, polygon_mode: wgpu::PolygonMode, multisample: wgpu::MultisampleState, write_mask: wgpu::ColorWrites| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
            depth_compare: Some(nirender_wgpu::depth::compare(wgpu::CompareFunction::LessEqual)),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("ALPHA_TO_COVERAGE", multisample.alpha_to_coverage_enabled as u32 as f64)],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let pipeline = make_pipeline(wgpu::Face::Back, wgpu::PolygonMode::Fill, scene.multisample(alpha_to_coverage), wgpu::ColorWrites::COLOR);
    let mirror_pipeline = make_pipeline(wgpu::Face::Front, wgpu::PolygonMode::Fill, Default::default(), wgpu::ColorWrites::ALL);
    // The same as lines, for a frame in which the game draws in wireframe (see
    // `SceneView::wireframe`); a device without the line mode draws them filled then.
    let lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
    let pipeline_lines = lines.then(|| make_pipeline(wgpu::Face::Back, wgpu::PolygonMode::Line, scene.multisample(alpha_to_coverage), wgpu::ColorWrites::COLOR));
    let mirror_pipeline_lines = lines.then(|| make_pipeline(wgpu::Face::Front, wgpu::PolygonMode::Line, Default::default(), wgpu::ColorWrites::ALL));
    // The casters of the shadow map: the same vertices and places, the depth from the sun.
    let shadow_source = format!("{}{SET_WGSL}{SHADOW_SHADER}", super::view::wgsl());
    let shadow_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant statics shadow"), source: wgpu::ShaderSource::Wgsl(shadow_source.into()) });
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
    let [group_main, group_mirror, group_shadow0, group_shadow1] = groups;
    let [uniforms_main, uniforms_mirror, uniforms_shadow0, uniforms_shadow1] = uniform_buffers;
    let sets = [
        BufferSet { group: group_main, uniforms: uniforms_main },
        BufferSet { group: group_mirror, uniforms: uniforms_mirror },
        BufferSet { group: group_shadow0, uniforms: uniforms_shadow0 },
        BufferSet { group: group_shadow1, uniforms: uniforms_shadow1 },
    ];

    // The values of every place for the draws, once: the world matrix and the values of the
    // subset. A draw names its place by its first instance.
    let mut instance_data: Vec<f32> = Vec::with_capacity(catalog.instances.len().max(1) * 20);
    for instance in &catalog.instances {
        let s = &catalog.subsets[instance.subset as usize];
        instance_data.extend_from_slice(&instance.world);
        instance_data.extend_from_slice(&[s.palette as f32, s.has_alpha as u32 as f32, s.animate_uv as u32 as f32, instance.subset as f32]);
    }
    if instance_data.is_empty() {
        instance_data.resize(20, 0.0);
    }
    let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the places"),
        contents: bytemuck::cast_slice(&instance_data),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let pools = cull::Pools::new(catalog.textures.len(), device);
    let forced_cpu = std::env::var("MGE_XE_STATICS_CULL").is_ok_and(|value| value == "cpu");
    let culling = if cull::supported(device) && !forced_cpu {
        let places: Vec<cull::Place> = catalog
            .instances
            .iter()
            .map(|instance| cull::Place { center: instance.center, radius: instance.radius, subset: instance.subset, tier: instance.tier as u32, group: instance.group as u32, _pad: 0 })
            .collect();
        println!("distant statics: the places are chosen on the GPU");
        Some(cull::Culling::new(device, &places, catalog.subsets.len(), catalog.textures.len()))
    } else {
        println!(
            "distant statics: the places are chosen on the CPU ({})",
            if forced_cpu { "MGE_XE_STATICS_CULL=cpu" } else { "the device has no first instance or count for indirect draws" }
        );
        None
    };

    // The grass: its own uniforms and shader, and the textures of the statics.
    let grass_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grass"),
        size: size_of::<GrassUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let grass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grass"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    // The shadow map is made at the first frame with shadows; until then the group has a
    // texture of one texel in its place (the shader reads no shadow without a map).
    let grass_shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("grass, the shadow map"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let no_shadow = std::rc::Rc::new(
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("grass, no shadow map"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default()),
    );
    let grass_group = make_grass_group(device, &grass_layout, &grass_uniforms, &sampler, &no_shadow, &grass_shadow_sampler);
    let grass_source = format!("{}{GRASS_SHADER}", super::view::wgsl());
    let grass_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("grass"), source: wgpu::ShaderSource::Wgsl(grass_source.into()) });
    let grass_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grass"),
        bind_group_layouts: &[Some(view_layout), Some(&grass_layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    let grass_place = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
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
            depth_compare: Some(nirender_wgpu::depth::compare(wgpu::CompareFunction::LessEqual)),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        // The alpha of the blade is the coverage of its pixel with `alpha_to_coverage`; the
        // alpha of the back buffer stays 1.
        multisample: scene.multisample(alpha_to_coverage),
        fragment: Some(wgpu::FragmentState {
            module: &grass_module,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("ALPHA_TO_COVERAGE", scene.multisample(alpha_to_coverage).alpha_to_coverage_enabled as u32 as f64)],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::COLOR })],
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
    Gpu {
        pipeline,
        mirror_pipeline,
        pipeline_lines,
        mirror_pipeline_lines,
        shadow_pipeline,
        sets,
        textures,
        texture_views,
        instances,
        arrivals,
        pools,
        culling,
        grass_pipeline,
        grass_pipeline_lines,
        grass_group,
        grass_layout,
        grass_sampler: sampler,
        grass_shadow_sampler,
        grass_atlas: no_shadow,
        grass_uniforms,
        grass_records,
    }
}

/// The frame group of the grass: its uniforms, the sampler of its textures, and the shadow
/// map.
fn make_grass_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, sampler: &wgpu::Sampler, atlas: &wgpu::TextureView, atlas_sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grass"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(atlas) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(atlas_sampler) },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::{cap_from_budget, cap_move, flags_problem, half, hidden, normal_alignment, pick_trim, plan_merged, BENT_SHARE, VERTEX};

    const MB: u64 = 1 << 20;

    /// A value that a half float holds exactly (0 or a normal number), as its bits.
    fn to_half(value: f32) -> u16 {
        if value == 0.0 {
            return 0;
        }
        let bits = value.to_bits();
        let sign = (bits >> 16) & 0x8000;
        let exponent = ((bits >> 23) & 0xFF) - 112;
        (sign | (exponent << 10) | ((bits >> 13) & 0x3FF)) as u16
    }

    /// A statics vertex: the position as half floats (and the palette number 0), the normal
    /// as bytes, a white colour, the texture coordinate 0.
    fn vertex(position: [f32; 3], normal: [f32; 3]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(VERTEX);
        for p in position {
            bytes.extend_from_slice(&to_half(p).to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend(normal.map(|n| ((n + 1.0) * 127.5).round() as u8));
        bytes.push(0);
        bytes.extend_from_slice(&[255; 4]);
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    #[test]
    fn a_half_float_decodes() {
        for value in [0.0f32, 1.0, -2.5, 65504.0, 123.375, 0.5] {
            assert_eq!(half(&to_half(value).to_le_bytes(), 0), value);
        }
        // The smallest subnormal and infinity.
        assert_eq!(half(&1u16.to_le_bytes(), 0), 1.0 / 16_777_216.0);
        assert_eq!(half(&0x7C00u16.to_le_bytes(), 0), f32::INFINITY);
    }

    /// An upright blade drawn on both sides, as two windings of its two triangles (the face of
    /// the first winding faces -y). Ordinary: each winding has vertices of its own with the
    /// normal of its face. Bent, as Fantasia Grass Mod has it: both windings share vertices with
    /// a normal that leans up and out, so it points against the faces of one winding.
    #[test]
    fn bent_normals_are_told_from_the_normals_of_the_faces() {
        let corners = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 30.0], [10.0, 0.0, 30.0]];
        let triangles = |list: &[u16]| -> Vec<u8> { list.iter().flat_map(|i| i.to_le_bytes()).collect() };
        // Ordinary: vertices 0 to 3 face -y, 4 to 7 (the same places) face +y.
        let mut ordinary: Vec<u8> = corners.iter().flat_map(|&p| vertex(p, [0.0, -1.0, 0.0])).collect();
        ordinary.extend(corners.iter().flat_map(|&p| vertex(p, [0.0, 1.0, 0.0])));
        let ordinary_triangles = triangles(&[0, 1, 2, 1, 3, 2, 4, 6, 5, 5, 6, 7]);
        assert_eq!(normal_alignment(&ordinary, &ordinary_triangles), Some(0.0));
        // Bent: four shared vertices, both windings.
        let bent: Vec<u8> = corners.iter().flat_map(|&p| vertex(p, [0.0, -0.5, 0.866])).collect();
        let share = normal_alignment(&bent, &triangles(&[0, 1, 2, 1, 3, 2, 0, 2, 1, 1, 2, 3])).unwrap();
        assert!((share - 0.5).abs() < 1e-6 && share >= BENT_SHARE, "{share}");
        assert_eq!(normal_alignment(&ordinary, &[]), None);
    }

    #[test]
    fn a_subset_flag_that_is_not_known_rejects_the_data() {
        let file = std::path::Path::new("static_meshes_000");
        assert_eq!(flags_problem(0x1F, file), None);
        assert_eq!(flags_problem(0, file), None);
        let problem = flags_problem(0x41, file).unwrap();
        assert!(problem.contains("0x40") && problem.contains("generate distant land again"), "{problem}");
    }

    #[test]
    fn the_cap_leaves_a_margin_of_an_eighth_of_the_budget() {
        // 8 GB: the margin is 1 GB; 4 GB in use, 100 MB of it merged.
        assert_eq!(cap_from_budget(8192 * MB, 4096 * MB, 100 * MB), 100 * MB + 3072 * MB);
        // 4 GB: the margin is 512 MB.
        assert_eq!(cap_from_budget(4096 * MB, 1024 * MB, 0), 2560 * MB);
    }

    #[test]
    fn the_margin_is_256_mb_at_least_and_1_gb_at_most() {
        assert_eq!(cap_from_budget(1024 * MB, 0, 0), 768 * MB);
        assert_eq!(cap_from_budget(32768 * MB, 0, 0), 31744 * MB);
    }

    #[test]
    fn a_full_budget_leaves_the_merged_bytes_on_the_gpu() {
        assert_eq!(cap_from_budget(4096 * MB, 5000 * MB, 300 * MB), 300 * MB);
    }

    #[test]
    fn the_cap_moves_only_beyond_its_dead_band() {
        // A budget of 11718 MB: the band is 1 % of it, 117 MB.
        let budget = 11718 * MB;
        assert_eq!(cap_move(7882 * MB, 7891 * MB, budget), 0);
        assert_eq!(cap_move(7882 * MB, 7882 * MB - 100 * MB, budget), 0);
        assert_eq!(cap_move(7882 * MB, 7882 * MB + 200 * MB, budget), 1);
        assert_eq!(cap_move(7882 * MB, 6472 * MB, budget), -1);
        // A budget of 4096 MB: the band is the least, 64 MB.
        assert_eq!(cap_move(2000 * MB, 2050 * MB, 4096 * MB), 0);
        assert_eq!(cap_move(2000 * MB, 2070 * MB, 4096 * MB), 1);
    }

    #[test]
    fn the_trim_takes_the_least_recently_wanted_first_down_to_the_low_mark() {
        // (wanted at, distance, subset, bytes): 3 was wanted longest ago, then 1 and 2 at the
        // same time, 2 farther.
        let candidates = vec![(50.0, 9000.0, 1, 30), (50.0, 20000.0, 2, 30), (10.0, 5000.0, 3, 30), (90.0, 30000.0, 4, 30)];
        assert_eq!(pick_trim(candidates.clone(), 200, 140), vec![3, 2]);
        assert_eq!(pick_trim(candidates.clone(), 200, 200), Vec::<u32>::new());
        assert_eq!(pick_trim(candidates, 200, 0), vec![3, 2, 1, 4]);
    }

    #[test]
    fn all_wanted_fit_under_the_cap() {
        let (admit, evict, waiting) = plan_merged(&[(1.0, 7, 30), (2.0, 8, 30)], vec![(5.0, 1, 30)], 30, 100);
        assert_eq!((admit, evict, waiting), (vec![7, 8], vec![], false));
    }

    #[test]
    fn a_further_subset_goes_for_a_nearer_one() {
        let (admit, evict, waiting) = plan_merged(&[(1.0, 7, 40)], vec![(3.0, 1, 30), (9.0, 2, 30), (8.0, 3, 30)], 90, 100);
        assert_eq!((admit, evict, waiting), (vec![7], vec![2], false));
    }

    #[test]
    fn a_nearer_subset_stays_and_the_wanted_one_waits() {
        let (admit, evict, waiting) = plan_merged(&[(1.0, 7, 30), (6.0, 8, 30)], vec![(3.0, 1, 30), (5.0, 2, 30)], 60, 100);
        assert_eq!((admit, evict, waiting), (vec![7], vec![], true));
    }

    #[test]
    fn nothing_goes_when_it_does_not_make_enough_room() {
        let (admit, evict, waiting) = plan_merged(&[(1.0, 7, 80)], vec![(0.5, 1, 50), (9.0, 2, 30)], 80, 100);
        assert_eq!((admit, evict, waiting), (vec![], vec![], true));
    }

    #[test]
    fn a_subset_larger_than_the_cap_waits() {
        let (admit, evict, waiting) = plan_merged(&[(1.0, 7, 200)], vec![(9.0, 2, 30)], 30, 100);
        assert_eq!((admit, evict, waiting), (vec![], vec![], true));
    }
    #[test]
    fn a_place_of_a_hidden_group_is_hidden() {
        let shown = [1, 1, 0, 1];
        assert!(hidden(&shown, 2));
        assert!(!hidden(&shown, 1));
        assert!(!hidden(&shown, 3));
    }

    #[test]
    fn a_place_without_a_group_or_record_is_shown() {
        assert!(!hidden(&[0, 0], 0));
        assert!(!hidden(&[], 2));
        assert!(!hidden(&[1, 0], 9));
    }
}

#[cfg(test)]
mod reader_tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    use super::{Catalog, Subset, reader};

    /// A catalog of one subset in shard 7 of a folder that has no shards.
    fn catalog_of_a_missing_shard() -> Catalog {
        let subset = Subset {
            shard: 7,
            vertex_offset: 0,
            index_offset: 0,
            vertex_count: 3,
            triangle_count: 1,
            has_alpha: false,
            animate_uv: false,
            water: 0,
            look: None,
            sphere: [0.0; 4],
            texture: 0,
            palette: 0,
            components: Vec::new(),
            grass: false,
        };
        Catalog {
            folder: std::env::temp_dir().join("mge-xe-reader-test-no-shards"),
            subsets: vec![subset],
            instances: Vec::new(),
            worlds: HashMap::new(),
            textures: Vec::new(),
            palettes: Vec::new(),
            merged_place: HashMap::new(),
            grass: Vec::new(),
            groups: 1,
            ordinary_bytes: 0,
            merged_bytes: 0,
        }
    }

    /// A request the reader cannot serve is answered, as unreadable: the renderer takes the
    /// subset out of the ones asked for, and asks for it no more.
    #[test]
    fn a_subset_of_a_missing_shard_is_answered_as_unreadable() {
        let (requests, request_receiver) = channel();
        let (arrival_sender, arrivals) = channel();
        let catalog = Arc::new(catalog_of_a_missing_shard());
        let thread = std::thread::spawn(move || reader(catalog, request_receiver, arrival_sender));
        requests.send(0).unwrap();
        drop(requests);
        let arrival = arrivals.recv_timeout(std::time::Duration::from_secs(5)).expect("an answer for the unreadable subset");
        assert_eq!(arrival.subset, 0);
        assert!(arrival.unreadable);
        assert!(arrival.vertices.is_empty() && arrival.indexes.is_empty());
        thread.join().unwrap();
    }
}
