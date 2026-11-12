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
//! texture draws them from pools of geometry. Without the features of the device, the CPU
//! chooses them and draws each place from the same pools.
//!
//! No test against the land hides a place.

use nirender_wgpu::objects::{Create, CreateView};
use std::collections::{HashMap, HashSet};
use std::fs::File;

use nirender_wgpu::pass_report::Skip;

/// The skips of the statics that more than one call answers.
const FAILED: Skip = "the distant statics failed";
const NO_CATALOG: Skip = "no catalog of the distant statics";
const NO_WORLD: Skip = "no world space of the cell among the distant statics";
const OFF: Skip = "the distant statics are off";
const NO_GPU: Skip = "no GPU state of the statics";
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use ash::vk;
use distantland::mge_xe::distant_statics::{StaticTables, load_static_tables};
use distantland::mge_xe::usage_data::load_usage_data;
use mge_config::{ConfigDocument, StaticsResidency};
use nirender_wgpu::SceneView;
use nirender_wgpu::wgpu;

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
/// The bands of the near distance of the arrivals in the view, one cell each; the last band
/// holds that distance and farther.
const VIEW_BANDS: usize = 10;
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
    let position = |v: usize| {
        [
            half(vertices, v * VERTEX),
            half(vertices, v * VERTEX + 2),
            half(vertices, v * VERTEX + 4),
        ]
    };
    let normal = |v: usize| -> [f32; 3] { core::array::from_fn(|i| vertices[v * VERTEX + 8 + i] as f32 / 127.5 - 1.0) };
    let (mut corners, mut against) = (0u32, 0u32);
    for triangle in indexes.chunks_exact(TRIANGLE) {
        let corner: [usize; 3] =
            core::array::from_fn(|i| u16::from_le_bytes([triangle[2 * i], triangle[2 * i + 1]]) as usize);
        if corner.iter().any(|&v| v >= count) {
            continue;
        }
        let [a, b, c] = corner.map(position);
        let (e1, e2) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let face = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
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

/// The edge of a tile of a grass cell, in units: a cell has 8 by 8 tiles.
const GRASS_TILE: f32 = 1024.0;

/// Why a place is not in the view.
#[derive(Debug, PartialEq)]
enum GrassMiss {
    TooFar,
    OutOfView,
}

impl GrassPlace {
    /// The tile of the place: its row and its column, in tiles of [`GRASS_TILE`] units.
    fn tile(&self) -> (i32, i32) {
        (
            (self.position[1] / GRASS_TILE).floor() as i32,
            (self.position[0] / GRASS_TILE).floor() as i32,
        )
    }

    /// Why the place is not in the view of the eye with the view range and the planes of the
    /// view: [`None`] when it is taken.
    fn miss(&self, eye: [f32; 3], range: f32, planes: &[[f32; 4]; 6]) -> Option<GrassMiss> {
        let to = [
            self.position[0] - eye[0],
            self.position[1] - eye[1],
            self.position[2] - eye[2],
        ];
        let reach = range + self.radius;
        if to[0] * to[0] + to[1] * to[1] + to[2] * to[2] > reach * reach {
            return Some(GrassMiss::TooFar);
        }
        let p = self.position;
        if !planes
            .iter()
            .all(|plane| plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3] >= -self.radius)
        {
            return Some(GrassMiss::OutOfView);
        }
        None
    }
}

/// A run of the places of a grass cell that lie in one tile, with the box of their positions and
/// the largest radius among them.
struct GrassTile {
    first: u32,
    count: u32,
    min: [f32; 3],
    max: [f32; 3],
    radius: f32,
}

impl GrassTile {
    /// The tile of the places `members` (not empty), starting at `first` in the list of the cell.
    fn of(first: u32, members: &[&GrassPlace]) -> GrassTile {
        let mut tile = GrassTile {
            first,
            count: members.len() as u32,
            min: [f32::MAX; 3],
            max: [f32::MIN; 3],
            radius: 0.0,
        };
        for place in members {
            for axis in 0..3 {
                tile.min[axis] = tile.min[axis].min(place.position[axis]);
                tile.max[axis] = tile.max[axis].max(place.position[axis]);
            }
            tile.radius = tile.radius.max(place.radius);
        }
        tile
    }

    fn range(&self) -> std::ops::Range<usize> {
        self.first as usize..(self.first + self.count) as usize
    }

    /// True when [`GrassPlace::miss`] is `Some` for every place of the tile. The range test takes
    /// the nearest point of the box in place of the position, and each plane test takes the
    /// corner of the box that lies farthest along the normal of the plane. Both use the largest
    /// radius in place of the radius. The operations are those of the place test and each is
    /// monotonic in the position and in the radius, so a place inside the box cannot pass where
    /// the box fails, in the numbers of the machine too.
    fn misses(&self, eye: [f32; 3], range: f32, planes: &[[f32; 4]; 6]) -> bool {
        let to: [f32; 3] =
            core::array::from_fn(|axis| (self.min[axis] - eye[axis]).max(0.0).max(eye[axis] - self.max[axis]));
        let reach = range + self.radius;
        if to[0] * to[0] + to[1] * to[1] + to[2] * to[2] > reach * reach {
            return true;
        }
        planes.iter().any(|plane| {
            let corner: [f32; 3] =
                core::array::from_fn(|axis| if plane[axis] >= 0.0 { self.max[axis] } else { self.min[axis] });
            plane[0] * corner[0] + plane[1] * corner[1] + plane[2] * corner[2] + plane[3] < -self.radius
        })
    }
}

/// The grass places of one cell of the world, in tile order, with the tiles. The list holds
/// indexes into [`Catalog::grass`].
#[derive(Default)]
struct GrassCell {
    places: Vec<u32>,
    tiles: Vec<GrassTile>,
}

impl GrassCell {
    /// Orders the places by tile (row by row, and in the order of the catalog within a tile) and
    /// makes the tiles. Called once, when the catalog is made.
    fn make_tiles(&mut self, grass: &[GrassPlace]) {
        self.places.sort_by_key(|index| (grass[*index as usize].tile(), *index));
        self.tiles.clear();
        let mut first = 0;
        while first < self.places.len() {
            let tile = grass[self.places[first] as usize].tile();
            let end = first
                + self.places[first..]
                    .iter()
                    .take_while(|index| grass[**index as usize].tile() == tile)
                    .count();
            let members: Vec<&GrassPlace> = self.places[first..end].iter().map(|index| &grass[*index as usize]).collect();
            self.tiles.push(GrassTile::of(first as u32, &members));
            first = end;
        }
    }

    /// The places of the tiles that can have a place for the view, in the order of the cell, each
    /// with its position in the list of the cell.
    fn reachable<'a>(
        &'a self,
        eye: [f32; 3],
        range: f32,
        planes: &'a [[f32; 4]; 6],
    ) -> impl Iterator<Item = (usize, u32)> + 'a {
        self.tiles
            .iter()
            .filter(move |tile| !tile.misses(eye, range, planes))
            .flat_map(move |tile| tile.range().map(move |at| (at, self.places[at])))
    }
}

/// The number of calls after which a cell that no call walks loses its records. A cell at the
/// edge of the reach keeps them while the eye moves to and fro across the edge of the cell.
const GRASS_KEEP_CALLS: u64 = 8;

/// The records of the places of one grass cell, each made when its place is first taken.
struct CellRecords {
    /// One for each place of the cell, in the order of [`GrassCell::places`].
    records: Vec<Option<[f32; 12]>>,
    /// The number of the last call that walked the cell.
    walked: u64,
}

impl CellRecords {
    /// The record of the place at `at` in the list of the cell. A record that is made adds to `made`.
    fn record(&mut self, at: usize, place: &GrassPlace, made: &mut u64) -> [f32; 12] {
        *self.records[at].get_or_insert_with(|| {
            *made += 1;
            grass_record(&world_matrix(place.position, place.rotation, place.scale))
        })
    }
}

/// The records of the grass places that the calls keep, by world space and cell. A record only
/// depends on its place, so the records stay valid until the catalog changes.
#[derive(Default)]
struct GrassKept {
    worlds: HashMap<Vec<u8>, HashMap<(i32, i32), CellRecords>>,
    /// The number of the current call.
    call: u64,
}

/// The records of one world space during one call. The world space enters the map with its
/// first cell, so a call that makes no records (an interior with no grass) allocates nothing.
struct GrassKeep<'a> {
    worlds: &'a mut HashMap<Vec<u8>, HashMap<(i32, i32), CellRecords>>,
    world: &'a [u8],
    call: u64,
}

impl GrassKept {
    /// Starts a call in the world space `world`.
    fn begin<'a>(&'a mut self, world: &'a [u8]) -> GrassKeep<'a> {
        self.call += 1;
        GrassKeep {
            worlds: &mut self.worlds,
            world,
            call: self.call,
        }
    }

    /// Ends a call: the cells that no call walked in the last [`GRASS_KEEP_CALLS`] calls go, and
    /// the world spaces that have no cell left.
    fn end(&mut self) {
        let call = self.call;
        self.worlds.retain(|_, cells| {
            cells.retain(|_, cell| call - cell.walked <= GRASS_KEEP_CALLS);
            !cells.is_empty()
        });
    }

    /// The number of cells with records, and the bytes of their records.
    fn size(&self) -> (usize, usize) {
        let cells = self.worlds.values().flat_map(|cells| cells.values());
        cells.fold((0, 0), |(count, bytes), cell| {
            (
                count + 1,
                bytes + cell.records.len() * std::mem::size_of::<Option<[f32; 12]>>(),
            )
        })
    }
}

impl GrassKeep<'_> {
    /// Marks the cell as walked by this call, when it has records.
    fn walk(&mut self, key: (i32, i32)) {
        if let Some(cell) = self.worlds.get_mut(self.world).and_then(|cells| cells.get_mut(&key)) {
            cell.walked = self.call;
        }
    }

    /// The records of the cell, empty when it has none yet, for a cell of `places` places.
    fn cell(&mut self, key: (i32, i32), places: usize) -> &mut CellRecords {
        let call = self.call;
        if !self.worlds.contains_key(self.world) {
            self.worlds.insert(self.world.to_vec(), HashMap::new());
        }
        let cells = self.worlds.get_mut(self.world).expect("the world space is inserted above");
        cells.entry(key).or_insert_with(|| CellRecords {
            records: vec![None; places],
            walked: call,
        })
    }
}

/// What a grass call sees: the eye, the view range of the game, the planes of the view, the
/// time on the clock of the arrivals with the fade time (read once for the call), and the most
/// draws that the call may make.
struct GrassSight {
    eye: [f32; 3],
    range: f32,
    planes: [[f32; 4]; 6],
    now: f32,
    fade_seconds: f32,
    limit: usize,
}

/// What a grass call needs of a subset that is on the GPU.
struct GrassSubset {
    texture: u32,
    /// The time of the arrival of the subset ([`Resident::arrived`]).
    arrived: f32,
    bent: f32,
}

/// One draw of a grass call: a subset of a place, with its record.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GrassDraw {
    texture: u32,
    subset: u32,
    record: [f32; 12],
    fade: f32,
    bent: f32,
}

/// The places of one world space: the exterior, or one interior cell.
#[derive(Default)]
struct World {
    /// The places of the water subsets, by cell: as `cells`.
    water_cells: HashMap<(i32, i32), Vec<u32>>,
    /// The instances by the cell of their centre.
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// The grass places by the cell of their position.
    grass_cells: HashMap<(i32, i32), GrassCell>,
    /// The places of merged subsets in `cells`.
    merged: Vec<u32>,
}

impl World {
    /// Orders the places of each grass cell by tile and makes the tiles. Called once, when the
    /// catalog is made.
    fn make_grass_tiles(&mut self, grass: &[GrassPlace]) {
        for cell in self.grass_cells.values_mut() {
            cell.make_tiles(grass);
        }
    }

    /// Chooses the grass draws for a view: each place in the cells within the range of the eye
    /// that is not hidden, is within the range and the planes, and has a subset on the GPU. The
    /// draws go to `draws`, at most `sight.limit` of them. Returns true when the limit cut the choice.
    #[allow(clippy::too_many_arguments)]
    fn choose_grass(
        &self,
        grass: &[GrassPlace],
        sight: &GrassSight,
        hidden: impl Fn(u16) -> bool,
        subset: impl Fn(u32) -> Option<GrassSubset>,
        draws: &mut Vec<GrassDraw>,
        counts: &mut GrassParts,
        mut keep: GrassKeep<'_>,
    ) -> bool {
        let eye = sight.eye;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = (sight.range / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(cell) = self.grass_cells.get(&(x, y)) else { continue };
                keep.walk((x, y));
                let mut places = cell.reachable(eye, sight.range, &sight.planes).peekable();
                if places.peek().is_none() {
                    counts.skipped += cell.places.len() as u64;
                    continue;
                }
                let kept = keep.cell((x, y), cell.places.len());
                let visited_before = counts.visited;
                for (at, index) in places {
                    let place = &grass[index as usize];
                    counts.visited += 1;
                    if hidden(place.group) {
                        counts.hidden += 1;
                        continue;
                    }
                    match place.miss(eye, sight.range, &sight.planes) {
                        Some(GrassMiss::TooFar) => {
                            counts.too_far += 1;
                            continue;
                        }
                        Some(GrassMiss::OutOfView) => {
                            counts.out_of_view += 1;
                            continue;
                        }
                        None => {}
                    }
                    let draws_before = draws.len();
                    let mut record = None;
                    for id in place.first_subset..place.first_subset + place.subset_count {
                        let Some(on_gpu) = subset(id) else {
                            counts.subset_misses += 1;
                            continue;
                        };
                        if draws.len() == sight.limit {
                            return true;
                        }
                        let record = *record.get_or_insert_with(|| kept.record(at, place, &mut counts.records_made));
                        let fade = Resident::fade_after(sight.now - on_gpu.arrived, sight.fade_seconds);
                        draws.push(GrassDraw {
                            texture: on_gpu.texture,
                            subset: id,
                            record,
                            fade,
                            bent: on_gpu.bent,
                        });
                    }
                    counts.without_resident += (draws.len() == draws_before) as u64;
                }
                counts.skipped += cell.places.len() as u64 - (counts.visited - visited_before);
            }
        }
        false
    }
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
    let scaling = [
        scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
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

/// The tables of one statics file, through the formats crate: the generator's own reader, with
/// every check of the file's layout and ranges (risk 9: a bad index in a table panicked the
/// helper's own reader). A file that cannot be read leaves the statics off (`Missing`); a file
/// that the crate refuses is data that this build cannot use (`Rejected`), with its reason.
fn read_tables(path: &Path) -> Result<StaticTables, CatalogError> {
    load_static_tables(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::InvalidData => CatalogError::Rejected(format!(
            "{}: {error}: generate distant land again in the MGE XE GUI",
            path.display()
        )),
        _ => CatalogError::Missing(format!("{}: {error}", path.display())),
    })
}

/// The problem with the statics files in `folder` (the distant land folder), if the formats
/// crate refuses one: for the loader of the land, which must not draw from data that the statics
/// reject. It reads the tables only, not the geometry. A file that cannot be read is the problem
/// of the loader of the statics, not of this check.
pub(super) fn tables_problem_in(folder: &Path) -> Option<String> {
    (0..SHARDS).find_map(
        |shard| match read_tables(&folder.join("statics").join(format!("static_meshes_{shard:03}"))) {
            Err(CatalogError::Rejected(problem)) => Some(problem),
            _ => None,
        },
    )
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
        let tables = read_tables(&path)?;
        let subset_base = subsets.len() as u32;

        for subset in tables.statics.iter().flat_map(|model| &model.subsets) {
            let record = &subset.record;
            let name = subset.texture.to_string();
            let texture = *texture_ids.entry(name.clone()).or_insert_with(|| {
                textures.push(name);
                textures.len() as u32 - 1
            });
            let water = subset.water();
            // The look line of a water subset: empty for one without a line of its own.
            let line = (!subset.water_look.is_empty()).then_some(&*subset.water_look);
            let look = matches!(water, 1 | 2).then(|| Look::of_distant(water, line));
            let palette = if subset.palette.is_empty() {
                0
            } else {
                let at = palettes.len() as u32;
                palettes.extend(subset.palette.iter().map(|entry| entry.bound));
                at
            };
            let components: Vec<Component> = subset
                .components
                .iter()
                .map(|component| Component {
                    first_triangle: component.first_triangle,
                    triangle_count: component.triangle_count,
                    tier: tier_of(u32::from(component.classification), component.radius, settings).unwrap_or(0),
                })
                .collect();
            let bytes = record.vertex_count as u64 * VERTEX as u64 + record.triangle_count as u64 * TRIANGLE as u64;
            if components.is_empty() {
                ordinary_bytes += bytes;
            } else {
                merged_bytes += bytes;
            }
            let sphere = record.sphere;
            subsets.push(Subset {
                shard: shard as u16,
                vertex_offset: record.vertex_offset,
                index_offset: record.index_offset,
                vertex_count: record.vertex_count,
                triangle_count: record.triangle_count,
                has_alpha: record.flags & 1 != 0,
                animate_uv: record.flags & 2 != 0,
                water,
                look,
                sphere: [sphere.center.x, sphere.center.y, sphere.center.z, sphere.radius],
                texture,
                palette,
                components,
                grass: false,
            });
        }
        for model in &tables.statics {
            let sphere = model.record.sphere;
            statics.push(Static {
                kind: model.record.static_type,
                radius: sphere.radius,
                center: [sphere.center.x, sphere.center.y, sphere.center.z],
                first_subset: subset_base + model.record.first_subset_index,
                subset_count: model.record.subset_count,
            });
        }
    }

    for model in statics.iter().filter(|model| model.kind == 4) {
        for subset in subsets
            .iter_mut()
            .skip(model.first_subset as usize)
            .take(model.subset_count as usize)
        {
            subset.grass = true;
        }
    }

    // The places. `usage.data` has the static of a place as its number in all the files, in
    // their order.
    let usage = load_usage_data(&statics_folder.join("usage.data")).map_err(|error| format!("usage.data: {error}"))?;
    if usage.num_meshes as usize != statics.len() {
        return Err(format!(
            "usage.data is for {} statics and the files have {}",
            usage.num_meshes,
            statics.len()
        )
        .into());
    }
    let mut instances: Vec<Instance> = Vec::new();
    let mut worlds: HashMap<Vec<u8>, World> = HashMap::new();
    let mut merged_place: HashMap<u32, u32> = HashMap::new();
    let mut grass: Vec<GrassPlace> = Vec::new();
    // The places of the exterior, then those of each interior cell that has statics.
    let exterior = (&[][..], &usage.exterior);
    let interiors = usage
        .interiors
        .iter()
        .map(|interior| (unpadded(&interior.name), &interior.references));
    for (name, places) in core::iter::once(exterior).chain(interiors) {
        let world_space = worlds.entry(name.to_vec()).or_default();
        for place in places {
            let Some(model) = statics.get(place.id as usize) else {
                continue;
            };
            if model.kind == 4 {
                // Grass: one record for the place. Its world matrix is made when it is drawn.
                if settings.grass {
                    let center = (model.center[0].powi(2) + model.center[1].powi(2) + model.center[2].powi(2)).sqrt();
                    let cell = (
                        (place.location.x / CELL).floor() as i32,
                        (place.location.y / CELL).floor() as i32,
                    );
                    world_space
                        .grass_cells
                        .entry(cell)
                        .or_default()
                        .places
                        .push(grass.len() as u32);
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
            let Some(tier) = tier_of(model.kind, model.radius * place.scale, settings) else {
                continue;
            };
            let world = world_matrix(
                [place.location.x, place.location.y, place.location.z],
                [place.rotation.x, place.rotation.y, place.rotation.z],
                place.scale,
            );
            for subset in model.first_subset..model.first_subset + model.subset_count {
                let Some(s) = subsets.get(subset as usize) else { continue };
                if s.vertex_count == 0 || s.triangle_count == 0 {
                    continue;
                }
                let (center, radius) = if model.kind == 6 {
                    (transform(&world, model.center), model.radius * place.scale)
                } else {
                    (
                        transform(&world, [s.sphere[0], s.sphere[1], s.sphere[2]]),
                        s.sphere[3] * place.scale,
                    )
                };
                let cell = ((center[0] / CELL).floor() as i32, (center[1] / CELL).floor() as i32);
                let list = if s.water != 0 {
                    &mut world_space.water_cells
                } else {
                    &mut world_space.cells
                };
                list.entry(cell).or_default().push(instances.len() as u32);
                if !s.components.is_empty() {
                    merged_place.insert(subset, instances.len() as u32);
                    if s.water == 0 {
                        world_space.merged.push(instances.len() as u32);
                    }
                }
                instances.push(Instance {
                    subset,
                    world,
                    center,
                    radius,
                    tier,
                    group: place.vis_index,
                });
            }
        }
    }
    for world in worlds.values_mut() {
        world.make_grass_tiles(&grass);
    }
    let groups = usage.vis_groups.len() + 1;
    Ok(Catalog {
        folder: folder.to_path_buf(),
        subsets,
        instances,
        worlds,
        textures,
        palettes,
        merged_place,
        grass,
        groups,
        ordinary_bytes,
        merged_bytes,
    })
}

/// The work of the grass calls of the main view since the last report, by what the loop does:
/// the places it meets and drops, the draws it makes, and the time of its parts.
#[derive(Default, Clone, Copy)]
struct GrassParts {
    calls: u64,
    /// The places walked, in the tiles that can have a place for the view.
    visited: u64,
    /// The places of the cells within reach that sit in a tile skipped as a whole.
    skipped: u64,
    /// The matrices made: the places taken that had no record kept.
    records_made: u64,
    /// Of the places walked: hidden by a vis group, beyond the view range, out of the view.
    hidden: u64,
    too_far: u64,
    out_of_view: u64,
    /// Of the places in the view: with no subset on the GPU, and the subsets missing.
    without_resident: u64,
    subset_misses: u64,
    /// The draws (a place with one subset) and the runs of one subset after the sort.
    draws: u64,
    runs: u64,
    triangles: u64,
    /// The seconds of the choice of the places (with the matrix and the fade of each), the
    /// sort, the records and their write, the uniforms and the pipeline, and the draw loop.
    choose_seconds: f64,
    sort_seconds: f64,
    records_seconds: f64,
    setup_seconds: f64,
    draw_seconds: f64,
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

/// The sets of a frame that exist from the start: the main view, the mirror and the two layers
/// of the shadow map. The sets of the views of their own ([`tier_set`]) follow, a list that
/// grows with the places and the spares (D82 caps, design B).
pub const FIXED_SETS: usize = 4;

/// The set of the view of its own in the set `k` of the views of their own (a place, or a
/// spare: [`super::view::TierViews`]).
pub const fn tier_set(k: usize) -> usize {
    FIXED_SETS + k
}

/// The names of the fixed sets, for the report.
const SET_NAMES: [&str; FIXED_SETS] = ["view", "mirror", "shadow 1", "shadow 2"];

/// The view group of the set `set` of the statics, when it is made.
fn group_of(view_groups: &[Option<wgpu::BindGroup>], set: usize) -> Option<&wgpu::BindGroup> {
    view_groups.get(super::view::group_of_set(set)).and_then(Option::as_ref)
}

/// Marks the set `set` as chosen by `prepare` for the frame.
fn mark_prepared(prepared: &mut Vec<bool>, set: usize) {
    if prepared.len() <= set {
        prepared.resize(set + 1, false);
    }
    prepared[set] = true;
}

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
        Arrival {
            subset,
            unreadable: true,
            vertices: Vec::new(),
            indexes: Vec::new(),
            faces: [0; 3],
            bent: false,
        }
    }
}

/// Reads the vertices and the indexes of the subsets that the renderer asks for. The indexes
/// of a merged subset come out with the triangles of the very far tier first, then those of
/// the far tier, then the rest. A subset that cannot be read gets an answer too
/// ([`Arrival::unreadable`]): every request is answered.
fn reader(catalog: Arc<Catalog>, requests: Receiver<u32>, arrivals: Sender<Arrival>) {
    // A shard that could not be opened is remembered as None: its subsets are answered
    // without another try.
    let mut files: HashMap<u16, Option<File>> = HashMap::new();
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

/// The bytes of one subset from its shard; [`None`] when the shard cannot be opened, or the
/// read fails or ends short (a truncated shard would arrive zero-filled otherwise).
fn read_subset(catalog: &Catalog, files: &mut HashMap<u16, Option<File>>, subset: u32) -> Option<Arrival> {
    let s = &catalog.subsets[subset as usize];
    let file = files
        .entry(s.shard)
        .or_insert_with(|| File::open(catalog.folder.join("statics").join(format!("static_meshes_{:03}", s.shard))).ok())
        .as_ref()?;
    let mut vertices = vec![0u8; s.vertex_count as usize * VERTEX];
    let mut source = vec![0u8; s.triangle_count as usize * TRIANGLE];
    read_whole(file, &mut vertices, s.vertex_offset)?;
    read_whole(file, &mut source, s.index_offset)?;
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
    Some(Arrival {
        subset,
        unreadable: false,
        vertices,
        indexes,
        faces,
        bent,
    })
}

/// Fills `buffer` from `file` at `offset`; [`None`] on an error or when the file ends first.
fn read_whole(file: &File, buffer: &mut [u8], offset: u64) -> Option<()> {
    let mut done = 0;
    while done < buffer.len() {
        let count = file.seek_read(&mut buffer[done..], offset + done as u64).ok()?;
        if count == 0 {
            return None;
        }
        done += count;
    }
    Some(())
}

/// The distance from the camera to the centre of a place, across the ground.
fn flat_distance(instance: &Instance, eye: [f32; 3]) -> f32 {
    ((instance.center[0] - eye[0]).powi(2) + (instance.center[1] - eye[1]).powi(2)).sqrt()
}

/// The budget and the use of this process of the largest memory heap of the GPU, from
/// `VK_EXT_memory_budget`, which wgpu turns on when the driver has it. [`None`] on another
/// backend than Vulkan, or without the extension.
pub(super) fn memory_budget(device: &wgpu::Device) -> Option<(u64, u64)> {
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
    unsafe {
        instance
            .raw_instance()
            .get_physical_device_memory_properties2(hal.raw_physical_device(), &mut properties)
    };
    let memory = properties.memory_properties;
    let heaps = &memory.memory_heaps[..(memory.memory_heap_count as usize).min(vk::MAX_MEMORY_HEAPS)];
    let largest = (0..heaps.len())
        .filter(|&heap| heaps[heap].flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .max_by_key(|&heap| heaps[heap].size)?;
    (budget.heap_budget[largest] > 0).then_some((budget.heap_budget[largest], budget.heap_usage[largest]))
}

/// The cap of the merged subsets from a memory budget, as `mergedCapCandidate` of the IPC
/// host mode: what the budget leaves after the memory in use and a margin, and the merged
/// bytes on the GPU, which the memory in use counts already. Without them the merged subsets
/// would make their own cap smaller.
pub(super) fn cap_from_budget(budget: u64, used: u64, merged: u64) -> u64 {
    let headroom = (budget / 8).clamp(HEADROOM_LEAST, HEADROOM_MOST);
    merged.saturating_add(budget.saturating_sub(used).saturating_sub(headroom))
}

/// The log line of a memory decision of the merged subsets in a frame: what was done, to how
/// many, their bytes on the GPU before and after, and the limit that decided it. One line for
/// each decision of a frame, not one for each subset, so the log stays quiet in a walk.
fn memory_event(stamp: &str, what: &str, count: u32, before: u64, after: u64, limit: &str) -> String {
    format!(
        "distant statics: {stamp}: {count} merged subsets {what}, {:.0} MB to {:.0} MB on the GPU ({limit})",
        before as f64 / 1e6,
        after as f64 / 1e6
    )
}

/// When a memory decision happened, for its log line: the renderer's frame and the performance
/// counter of Windows, the clock of PresentMon's `--qpc_time`, so that a summary finds the
/// frames that the player saw after it.
fn event_stamp(frame: u64, qpc: i64) -> String {
    format!("frame {frame}, qpc {qpc}")
}

/// The performance counter of Windows now.
fn qpc() -> i64 {
    let mut count = 0;
    // SAFETY: the call writes one number into `count`, a valid i64.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut count) };
    count
}

/// The limit of a [`memory_event`] in bytes, with its name: "none" for no limit.
fn bytes_limit(name: &str, bytes: u64) -> String {
    if bytes >= u64::MAX / 2 {
        format!("{name} none")
    } else {
        format!("{name} {:.0} MB", bytes as f64 / 1e6)
    }
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
fn plan_merged(
    wanted: &[(f32, u32, u64)],
    mut on_gpu: Vec<(f32, u32, u64)>,
    mut used: u64,
    cap: u64,
) -> (Vec<u32>, Vec<u32>, bool) {
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

impl Resident {
    /// The share of a subset that is in `since` seconds after its arrival, as the shaders have it,
    /// with the fade time `seconds` (0 for no fade).
    fn fade_after(since: f32, seconds: f32) -> f32 {
        if seconds > 0.0 {
            (since / seconds).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// The draws of a grass call in the order of the draw: by texture, then by subset, then by the
/// place in which the call found them. The key is a total order, so equal draws keep the order
/// of the choice and the same view gives the same records.
#[derive(Default)]
struct GrassOrder {
    keys: Vec<(u32, u32, u32)>,
}

impl GrassOrder {
    /// Sorts the draws given as (texture, subset) in the order of their choice.
    fn sort(&mut self, draws: impl Iterator<Item = (u32, u32)>) {
        self.keys.clear();
        self.keys.extend(
            draws
                .enumerate()
                .map(|(index, (texture, subset))| (texture, subset, index as u32)),
        );
        // A stable sort on the texture and the subset: the draws start in the order of their choice, so equal ones keep it, and the key is a total order of (texture, subset, index).
        self.keys.sort_by_key(|key| (key.0, key.1));
    }
}

/// The hash of the record list of a grass call, as two numbers: `ordered` hashes the records in
/// the order of the draw, so a change of the order changes it; `unordered` adds the hashes of the
/// records, so it holds the set of the records and not their order. A record is the 16 floats that
/// go to the GPU, hashed by their bits. It is a proof that two builds draw the same grass: a
/// picture does not show the order of the draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GrassHash {
    ordered: u64,
    unordered: u64,
}

impl GrassHash {
    /// The floats of one record.
    const RECORD_FLOATS: usize = 16;
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x100_0000_01b3;

    /// The hash of `records`, the floats of whole records one after the other.
    fn of(records: &[f32]) -> GrassHash {
        let (whole, rest) = records.as_chunks::<{ Self::RECORD_FLOATS }>();
        debug_assert!(rest.is_empty());
        let mut hash = GrassHash {
            ordered: Self::FNV_OFFSET,
            unordered: 0,
        };
        for record in whole {
            let one = record.iter().fold(Self::FNV_OFFSET, |one, value| {
                (one ^ value.to_bits() as u64).wrapping_mul(Self::FNV_PRIME)
            });
            hash.ordered = (hash.ordered ^ one).wrapping_mul(Self::FNV_PRIME);
            hash.unordered = hash.unordered.wrapping_add(one);
        }
        hash
    }

    /// The report line of a call: the frame of the main view counted so far, the view it is for,
    /// the records drawn and whether the limit of records cut the choice.
    fn line(&self, frame: u64, main: bool, record_floats: usize, cut: bool) -> String {
        format!(
            "grass hash frame {} {}: {} records, cut {}, ordered {:016x}, unordered {:016x}",
            frame,
            if main { "main" } else { "view" },
            record_floats / Self::RECORD_FLOATS,
            cut,
            self.ordered,
            self.unordered
        )
    }
}

/// What the loader thread gives to the renderer: the catalog, and the bytes of the texture
/// files in the order of the catalog ([`None`] for a file that is not there).
struct Loaded {
    catalog: Arc<Catalog>,
    textures: Vec<Option<Vec<u8>>>,
}

/// The answer of a loader thread that a test sends when it wants: [`Statics::with_late_catalog`].
#[cfg(test)]
pub struct LateCatalog {
    sender: Sender<Result<Loaded, CatalogError>>,
    loaded: Loaded,
}

#[cfg(test)]
impl LateCatalog {
    /// Sends the catalog, as the loader thread does when it is done.
    pub fn arrive(self) {
        self.sender.send(Ok(self.loaded)).expect("the statics take the answer");
    }
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
    /// The main view, the reflection, the two layers of the shadow map ([`FIXED_SETS`]), and the
    /// views of their own, by the number of the set; `None` for a set not made yet or freed.
    sets: Vec<Option<BufferSet>>,
    /// What a set of the views of their own needs to make its uniforms and its group
    /// ([`Gpu::make_buffer_set`]): the layout of group 1, the palettes, the sampler and the
    /// arrivals.
    set_layout: wgpu::BindGroupLayout,
    palettes: wgpu::Buffer,
    set_sampler: wgpu::Sampler,
    /// The samples of `pipeline`: a draw into a pass with another count (a rendered texture
    /// has one) takes `picture_pipeline`.
    samples: u32,
    /// The same as `pipeline` for a pass with one sample: a view of its own into a texture.
    picture_pipeline: wgpu::RenderPipeline,
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
    /// The grass into a pass with one sample (a view of its own into a rendered texture): no
    /// alpha to coverage.
    grass_picture_pipeline: wgpu::RenderPipeline,
    /// The values of the grass of the views of their own, one set for each call at the end of
    /// the world in a frame (by the number of the set of the views of their own, as
    /// [`tier_set`] less [`FIXED_SETS`]), made when a call first needs it; `None` while it is
    /// not made, or after it was freed.
    grass_sets: Vec<Option<GrassSet>>,
    /// The texture of one texel that stands for the shadow map in the group of a view of its own.
    grass_no_shadow: std::rc::Rc<wgpu::TextureView>,
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

/// The places, the uniforms and the group of the grass for one call of a view of its own. The
/// writes of a call land before all commands of the frame (the one-submit rule), so each call
/// of a frame has a set of its own.
struct GrassSet {
    records: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// The bytes of the buffers of one grass set: the places of a call and the uniforms.
const GRASS_SET_BYTES: u64 = (GRASS_RECORDS * GRASS_RECORD) as u64 + size_of::<GrassUniforms>() as u64;

impl Gpu {
    /// The uniforms and the group of one set of the statics (group 1 of its pipelines).
    fn make_buffer_set(&self, device: &wgpu::Device) -> BufferSet {
        let uniforms = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics"),
            size: size_of::<SetUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distant statics"),
            layout: &self.set_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.palettes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.set_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.arrivals.as_entire_binding(),
                },
            ],
        });
        BufferSet { group, uniforms }
    }
}

/// Where the grass of a call goes: the main view at the end of the world of the game, or a
/// view of its own (the `k`th call at the end of the world in the frame, D82 section 3.4f).
#[derive(Clone, Copy)]
pub enum GrassTarget {
    Main,
    View(usize),
}

/// What a call of the grass of a view of its own did.
#[derive(Default, Clone, Copy)]
pub struct GrassView {
    /// The places drawn.
    pub drawn: usize,
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
    /// The values of the shadow: the built-in ones or those of a core mod.
    pub shadow_values: super::core_mods::ShadowValues,
    /// The looks of Remiros' Groundcover (`[distant_land.grass]`): 1 on, for the soft end,
    /// the vertex colour sway and the wrap lighting.
    pub look: [f32; 4],
}

/// The shadow map for the grass of a frame (`renderGrassInst` of the Direct3D 9 path).
#[derive(Clone)]
pub struct GrassShadow {
    pub atlas: std::rc::Rc<wgpu::TextureView>,
    /// The matrices from a position less the eye to the two layers, with the layout of Direct3D
    /// (made as `view::relative_of_product` makes them): the grass takes them with `clip_with`.
    pub layers: [[f32; 16]; 2],
    /// 1 over the resolution of a layer.
    pub rcp: f32,
    /// The visibility of the sun, for the light that `shadowSunEstimate` gives.
    pub sun_vis: f32,
    /// The day of the hour (1 less the night): the shadow fades out through dusk and in
    /// through dawn, as the shadow of the ground under the grass does.
    pub day: f32,
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
    /// of its colour, times 0.25 + 0.75 of its visibility, times the day of the hour as the
    /// ground gets it). z: 1 above the sea. w: 1 for the fade of the ground at the edge and in
    /// the fog.
    shadow: [f32; 4],
    /// The looks of Remiros' Groundcover: x soft end, y vertex colour sway, z wrap lighting.
    look: [f32; 4],
}

/// The grass shader (`XE Mod Grass.fx`, with the looks of Remiros' Groundcover as options).
const GRASS_SHADER: &str = include_str!("statics/grass.wgsl");

/// The grass shader with the shadow hooks of the session (`core_mods.rs`).
pub(super) fn grass_source(shadow_hooks: &str) -> String {
    format!("{}{GRASS_SHADER}{shadow_hooks}", super::view::wgsl())
}

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
    [
        world[0], world[4], world[8], world[12], world[1], world[5], world[9], world[13], world[2], world[6], world[10],
        world[14],
    ]
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
    // In the mirror under water (flags.y is -1) the statics have no scattering: the interior
    // pass of `renderWaterReflection` fogs them with the linear fog of the game.
    return vec4f(fog_with(texel.rgb * in.color, in.world, u.flags.y >= 0.0), alpha);
}
";

/// How many times each part of a view's set was freed: for the test that a free of a set frees
/// every part ([`Statics::releases`]). Test builds only.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReleaseCounts {
    pub view_groups: u32,
    pub view_set: u32,
    pub grass_set: u32,
}

pub struct Statics {
    settings: Settings,
    /// The alpha of a static with an alpha test and of a blade of grass becomes the coverage
    /// of the samples of its pixel (`graphics.transparency_antialiasing`, with more than one
    /// sample): the `beginAlphaToCoverage` of the Direct3D 9 path around the statics and the
    /// grass.
    alpha_to_coverage: bool,
    /// The shadow hooks of the session for the grass shader (`core_mods.rs`).
    shadow_hooks: String,
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
    /// The number of the frame of this call (`SceneView::frame`), for the log lines of the
    /// memory decisions ([`memory_event`]).
    frame: u64,
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
    /// The sets that `prepare` chose for this frame, by the number of the set (a set past the
    /// end is not chosen).
    prepared: Vec<bool>,
    /// The view groups of the frame by their numbers (`None` for a group not made), and their
    /// layout (group 0 of the pipelines of the statics). The catalog loads when the layout is
    /// there.
    view_layout: Option<wgpu::BindGroupLayout>,
    view_groups: Vec<Option<wgpu::BindGroup>>,
    /// The places that the player allows for the views of their own: the sets of the views of
    /// their own from this number on are spares, for the names of the report.
    tier_places: usize,
    prepare_calls: u64,
    prepare_seconds: f64,
    /// The grass of the frame: texture, subset, record, the fade-in, 1 for bent normals.
    grass_draws: Vec<GrassDraw>,
    /// The draws of the call in the order of the draw, and their records, kept from call to call.
    grass_order: GrassOrder,
    grass_records: Vec<f32>,
    /// Each grass call reports the hash of its records (`MGE_XE_GRASS_HASH=1`), read once.
    grass_hash: bool,
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
    /// The work of each set since the last `report` (by the number of the set; it grows to the
    /// last set that drew), and of the grass.
    set_stats: Vec<SetStats>,
    grass_stats: SetStats,
    grass_parts: GrassParts,
    grass_kept: GrassKept,
    /// The view range of the game in the last main view ([`CullSet::eye`] w).
    game_range: f32,
    /// The merged subsets that arrived in play (no wait open) since the last `report`: all
    /// of them, those with their place inside the draw distance, those of them past the
    /// handoff (a late arrival that the view shows), and the nearest distance of a late one.
    play_arrivals: u64,
    late_arrivals: u64,
    late_shown: u64,
    late_nearest: f32,
    /// The merged subsets that arrived while a wait for the statics was open, and those in
    /// play whose place was in the main view, by the near distance of the place in cells.
    wait_arrivals: u64,
    view_arrivals: [u64; VIEW_BANDS],
    /// A wait for the statics is open (`set_waiting`).
    waiting: bool,
    /// The planes of the last main view.
    view_planes: Option<[[f32; 4]; 6]>,
    /// The merged subsets that arrived in play and were on the GPU before in the session.
    play_again: u64,
    /// The subsets that were on the GPU at some time in the session.
    ever_arrived: HashSet<u32>,
    /// The work of the wait on the loading screen, from its first frame to `take_wait`.
    wait: Option<WaitStats>,
    #[cfg(test)]
    releases: ReleaseCounts,
}

impl Statics {
    /// Reads the settings and starts the load of the catalog and of the textures.
    /// `shadow_hooks`: the shadow hooks of the session, for the grass shader.
    pub fn start(document: &ConfigDocument, folder: PathBuf, shadow_hooks: &str) -> Statics {
        let (mut statics, sender) = Statics::waiting(document);
        statics.shadow_hooks = shadow_hooks.to_string();
        let settings = statics.settings;
        thread::spawn(move || {
            let result = read_catalog(&folder, &settings).map(|catalog| {
                let mut textures: Vec<Option<Vec<u8>>> =
                    catalog.textures.iter().map(|name| read_texture(&folder, name)).collect();
                archive_textures(&folder, &catalog.textures, &mut textures);
                Loaded {
                    catalog: Arc::new(catalog),
                    textures,
                }
            });
            let _ = sender.send(result);
        });
        statics
    }

    /// The statics with the settings of `document`, before the catalog: it comes through the
    /// sender.
    fn waiting(document: &ConfigDocument) -> (Statics, Sender<Result<Loaded, CatalogError>>) {
        let land = &document.settings().distant_land;
        let settings = Settings {
            statics: land.statics,
            grass: land.render_grass,
            band_ends: [
                land.near_static_end * CELL,
                land.far_static_end * CELL,
                land.very_far_static_end * CELL,
            ],
            far_min_size: land.far_static_min_size,
            very_far_min_size: land.very_far_static_min_size,
            draw_distance: land.draw_distance * CELL,
        };
        let (sender, receiver) = channel();
        let statics = Statics {
            settings,
            alpha_to_coverage: document.settings().graphics.transparency_antialiasing,
            shadow_hooks: super::core_mods::CoreMods::built_in().shadow_hooks().to_string(),
            loading: Some(receiver),
            catalog: None,
            gpu: None,
            #[cfg(test)]
            releases: ReleaseCounts::default(),
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
            frame: 0,
            cap_waiting: false,
            draws: Vec::new(),
            device: None,
            queue: None,
            frame_ended: false,
            pool_full: false,
            unreadable: HashSet::new(),
            refused: HashSet::new(),
            refused_count: 0,
            prepared: Vec::new(),
            view_layout: None,
            view_groups: Vec::new(),
            tier_places: 0,
            prepare_calls: 0,
            prepare_seconds: 0.0,
            grass_draws: Vec::new(),
            grass_order: GrassOrder::default(),
            grass_records: Vec::new(),
            grass_hash: nirender_wgpu::switches::on("MGE_XE_GRASS_HASH") == Some(true),
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
            set_stats: Vec::new(),
            grass_stats: SetStats::default(),
            grass_parts: GrassParts::default(),
            grass_kept: GrassKept::default(),
            game_range: 0.0,
            play_arrivals: 0,
            late_arrivals: 0,
            late_shown: 0,
            late_nearest: f32::MAX,
            wait_arrivals: 0,
            view_arrivals: [0; VIEW_BANDS],
            waiting: false,
            view_planes: None,
            play_again: 0,
            ever_arrived: HashSet::new(),
            wait: None,
        };
        (statics, sender)
    }

    /// The statics with a catalog that comes when the test calls [`LateCatalog::arrive`]: one
    /// place of one subset in the interior cell `interior`, and the exterior with no place.
    #[cfg(test)]
    pub fn with_late_catalog(document: &ConfigDocument, interior: &[u8]) -> (Statics, LateCatalog) {
        let (statics, sender) = Statics::waiting(document);
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
            sphere: [0.0, 0.0, 0.0, 64.0],
            texture: 0,
            palette: 0,
            components: Vec::new(),
            grass: false,
        };
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let place = Instance {
            subset: 0,
            world: identity,
            center: [0.0; 3],
            radius: 64.0,
            tier: 0,
            group: 0,
        };
        let mut cell = World::default();
        cell.cells.insert((0, 0), vec![0]);
        let worlds = HashMap::from([(Vec::new(), World::default()), (interior.to_vec(), cell)]);
        let catalog = Catalog {
            folder: std::env::temp_dir().join("mge-xe-late-catalog-no-shards"),
            subsets: vec![subset],
            instances: vec![place],
            worlds,
            textures: vec!["late-catalog-missing.dds".to_string()],
            palettes: vec![[1.0; 4]],
            merged_place: HashMap::new(),
            grass: Vec::new(),
            groups: 1,
            ordinary_bytes: 0,
            merged_bytes: 0,
        };
        let loaded = Loaded {
            catalog: Arc::new(catalog),
            textures: vec![None],
        };
        (statics, LateCatalog { sender, loaded })
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
        let mut models: std::collections::BTreeMap<(u32, u32), (u32, u32, std::collections::BTreeSet<u16>)> =
            Default::default();
        let cells = (radius / CELL).ceil() as i32 + 1;
        let (cx, cy) = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        for gy in cy - cells..=cy + cells {
            for gx in cx - cells..=cx + cells {
                for index in world.grass_cells.get(&(gx, gy)).into_iter().flat_map(|cell| &cell.places) {
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
        println!(
            "grass report ({x}, {y}) within {radius}: {} models, grass on: {}",
            models.len(),
            self.settings.grass
        );
        for ((first, count), (places, hidden_places, groups)) in &models {
            println!(
                "  model at subset {first} ({count} subsets): {places} places, {hidden_places} hidden, groups {groups:?}"
            );
            for subset in *first..first + count {
                let Some(s) = catalog.subsets.get(subset as usize) else {
                    continue;
                };
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
        let names: Vec<String> = (0..self.set_stats.len()).map(|set| self.set_name(set)).collect();
        for (name, stats) in names
            .iter()
            .map(String::as_str)
            .zip(self.set_stats.iter())
            .chain(core::iter::once(("grass", &self.grass_stats)))
        {
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
        let parts = &self.grass_parts;
        if parts.calls > 0 {
            let per = |n: u64| n as f64 / parts.calls as f64;
            let ms = |seconds: f64| seconds * 1000.0 / parts.calls as f64;
            println!(
                "statics report {label}: grass parts: {} calls; for each call {:.0} places visited and {:.0} skipped with their tile ({:.0} hidden, {:.0} beyond the range, {:.0} out of the view, {:.0} in the view with no subset on the GPU), {:.0} subsets missed, {:.0} draws in {:.0} runs of one subset, {:.0} triangles; ms for each call: choice of the places {:.3}, sort {:.3}, records and their write {:.3}, uniforms and pipeline {:.3}, draw loop {:.3}",
                parts.calls,
                per(parts.visited),
                per(parts.skipped),
                per(parts.hidden),
                per(parts.too_far),
                per(parts.out_of_view),
                per(parts.without_resident),
                per(parts.subset_misses),
                per(parts.draws),
                per(parts.runs),
                per(parts.triangles),
                ms(parts.choose_seconds),
                ms(parts.sort_seconds),
                ms(parts.records_seconds),
                ms(parts.setup_seconds),
                ms(parts.draw_seconds)
            );
            let (cells, bytes) = self.grass_kept.size();
            println!(
                "statics report {label}: grass kept: {cells} cells with records, {:.0} KiB; {:.2} matrices made for each call",
                bytes as f64 / 1024.0,
                per(parts.records_made)
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
                for ((name, stats), counts) in names.iter().zip(self.set_stats.iter()).zip(culling.take_stats(device, queue))
                {
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
        if self.play_arrivals > 0 || self.wait_arrivals > 0 {
            println!(
                "statics report {label}: arrivals: {} merged subsets in play ({} of them on the GPU before), {} inside the draw distance of {:.0}, {} of them past the handoff (shown late), the nearest at {:.0}; {} more during a wait; in play and in the main view, by the near distance in cells (the last: {} and farther): {:?}",
                self.play_arrivals,
                self.play_again,
                self.late_arrivals,
                self.settings.draw_distance,
                self.late_shown,
                if self.late_arrivals > 0 { self.late_nearest } else { 0.0 },
                self.wait_arrivals,
                VIEW_BANDS - 1,
                self.view_arrivals
            );
        }
        self.wait_arrivals = 0;
        self.view_arrivals = [0; VIEW_BANDS];
        self.play_arrivals = 0;
        self.play_again = 0;
        self.late_arrivals = 0;
        self.late_shown = 0;
        self.late_nearest = f32::MAX;
        self.set_stats.fill(SetStats::default());
        self.grass_stats = SetStats::default();
        self.grass_parts = GrassParts::default();
    }

    /// The catalog and its textures are on their way, and a frame will take them: their GPU
    /// objects ([`Statics::finish_load`]) are then one long step of the frame thread, which the
    /// watchdog's allowance must cover (`Extension::loading`). Not with the statics off: then
    /// only a wait on a load takes them, and the allowance stays as it was.
    pub fn loading(&self) -> bool {
        self.loading.is_some() && self.settings.statics
    }

    /// The catalog is loaded.
    pub fn loaded(&self) -> bool {
        self.loading.is_none() && self.catalog.is_some()
    }

    /// Nothing more comes of the load: the catalog is there, or it failed. A wait on a load does
    /// not wait for more.
    pub fn load_ended(&self) -> bool {
        self.failed || self.loaded()
    }

    /// The subsets near the camera are on the GPU: none is asked for from the reader thread,
    /// and none is wanted that the cap would let in. Subsets that wait for the cap count as
    /// there: they come when the camera comes nearer.
    pub fn settled(&self) -> bool {
        self.failed || (self.loaded() && self.requested.is_empty() && (self.wanted.is_empty() || self.cap_waiting))
    }

    /// The reader thread ended (a panic): what it was asked for never comes. The statics are
    /// off, and a wait on a load does not wait for them.
    fn reader_ended(&mut self) {
        println!(
            "distant statics: off: the reader thread ended; {} subsets did not come",
            self.requested.len()
        );
        self.arrivals = None;
        self.requested.clear();
        self.failed = true;
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
            println!(
                "distant statics: a record of {} vis groups for a catalog of {}; all are shown",
                self.groups_record.len(),
                catalog.groups
            );
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
            self.here
                .extend(world.merged.iter().map(|&index| catalog.instances[index as usize].subset));
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
        let Some(world) = catalog.worlds.get(&self.world) else {
            return;
        };
        let mut wanted: Vec<u32> = world
            .cells
            .values()
            .chain(world.water_cells.values())
            .flatten()
            .map(|i| catalog.instances[*i as usize].subset)
            .filter(|s| catalog.subsets[*s as usize].components.is_empty())
            .collect();
        // The subsets of the grass too: they are never merged.
        wanted.extend(world.grass_cells.values().flat_map(|cell| &cell.places).flat_map(|i| {
            let place = &catalog.grass[*i as usize];
            place.first_subset..place.first_subset + place.subset_count
        }));
        wanted.sort_unstable();
        wanted.dedup();
        for subset in wanted {
            if self.may_ask(subset) {
                self.requested.insert(subset);
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
        if let Some(mb) =
            nirender_wgpu::switches::parsed("MGE_XE_STATICS_CAP_MB", "a number of MB", |mb| mb.parse::<u64>().ok())
        {
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
                println!(
                    "distant statics: the GPU gives no memory budget; no cap of the merged subsets (distant_land.statics_fallback_cap_mb = 0)"
                );
            } else {
                println!(
                    "distant statics: the GPU gives no memory budget; the cap of the merged subsets is {} MB (distant_land.statics_fallback_cap_mb)",
                    self.fallback_cap >> 20
                );
            }
        }
        self.first_cap = self.merged_cap;
        let high = self.high_mark();
        println!(
            "distant statics: residency {:?}, the high mark of the merged subsets {}",
            self.residency,
            if high >= u64::MAX / 2 {
                "none".to_string()
            } else {
                format!(
                    "{:.0} MB ({})",
                    high as f64 / 1e6,
                    if self.resident_mark > 0 {
                        "distant_land.statics_resident_mb"
                    } else {
                        "half of the first cap"
                    }
                )
            }
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
                    "distant statics: {}: the cap of the merged subsets goes up from {:.0} MB to {:.0} MB (budget {:.0} MB, {:.0} MB in use)",
                    self.stamp(),
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
            "distant statics: {}: the cap of the merged subsets goes down from {:.0} MB to {:.0} MB (budget {:.0} MB, {:.0} MB in use)",
            self.stamp(),
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
        if let Geometry::Pool(slot) = resident.geometry
            && let Some(gpu) = self.gpu.as_mut()
        {
            gpu.pools.remove(slot);
            if let (Some(culling), Some(queue)) = (gpu.culling.as_ref(), self.queue.as_ref()) {
                queue.write_buffer(
                    &culling.subsets,
                    subset as u64 * size_of::<cull::Subset>() as u64,
                    bytemuck::bytes_of(&cull::Subset::default()),
                );
            }
        }
        Some(resident)
    }

    /// True when `subset` may be asked for from the reader thread: it is not on the GPU, not
    /// asked for already, and neither unreadable nor refused by a full pool.
    fn may_ask(&self, subset: u32) -> bool {
        !self.resident.contains_key(&subset)
            && !self.requested.contains(&subset)
            && !self.unreadable.contains(&subset)
            && !self.refused.contains(&subset)
    }

    /// Takes the farthest merged subsets off the GPU while their bytes are over the cap: after
    /// the cap went down.
    fn pay_cap_debt(&mut self, catalog: &Catalog, eye: [f32; 3]) {
        if self.merged_bytes <= self.merged_cap {
            return;
        }
        let mut on_gpu: Vec<(f32, u32)> = self
            .resident
            .keys()
            .filter(|subset| catalog.merged_place.contains_key(subset))
            .map(|subset| (self.distance_here(catalog, *subset, eye), *subset))
            .collect();
        on_gpu.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let (before, mut count) = (self.merged_bytes, 0);
        for (_, subset) in on_gpu {
            if self.merged_bytes <= self.merged_cap {
                break;
            }
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.evicted += 1;
                count += 1;
            }
        }
        if count > 0 {
            println!(
                "{}",
                memory_event(
                    &self.stamp(),
                    "taken off for the lower cap",
                    count,
                    before,
                    self.merged_bytes,
                    &bytes_limit("the cap", self.merged_cap)
                )
            );
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
        let mut wanted: Vec<(f32, u32, u64)> = pending
            .drain(..)
            .filter(|subset| self.may_ask(*subset))
            .map(|subset| (distance(subset), subset, catalog.subsets[subset as usize].bytes()))
            .collect();
        self.wanted = pending;
        wanted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        wanted.dedup_by_key(|w| w.1);
        let asked: u64 = self
            .requested
            .iter()
            .filter(|subset| catalog.merged_place.contains_key(subset))
            .map(|subset| catalog.subsets[*subset as usize].bytes())
            .sum();
        let used = self.merged_bytes + asked;
        let (admit, evict, waiting) = if used + wanted.iter().map(|w| w.2).sum::<u64>() <= self.merged_cap {
            (wanted.iter().map(|w| w.1).collect(), Vec::new(), false)
        } else {
            let on_gpu = self
                .resident
                .iter()
                .filter(|(subset, _)| catalog.merged_place.contains_key(subset))
                .map(|(subset, resident)| (self.distance_here(catalog, *subset, eye), *subset, resident.bytes))
                .collect();
            plan_merged(&wanted, on_gpu, used, self.merged_cap)
        };
        let (before, mut count) = (self.merged_bytes, 0);
        for subset in evict {
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.evicted += 1;
                count += 1;
            }
        }
        if count > 0 {
            println!(
                "{}",
                memory_event(
                    &self.stamp(),
                    "taken off for nearer ones",
                    count,
                    before,
                    self.merged_bytes,
                    &bytes_limit("the cap", self.merged_cap)
                )
            );
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

    /// The answer of the loader thread, once it is there. A loader thread that ended without
    /// an answer (a panic) is a failed load, not a load still to come: the statics are off,
    /// and their load no longer counts for the watchdog ([`Statics::loading`]).
    fn take_catalog(&mut self) -> Option<Result<Loaded, CatalogError>> {
        let receiver = self.loading.as_ref()?;
        let answer = if self.synchronous {
            receiver.recv().map_err(|_| TryRecvError::Disconnected)
        } else {
            receiver.try_recv()
        };
        match answer {
            Ok(result) => {
                self.loading = None;
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                println!("distant statics: off: the loader thread ended without an answer");
                self.loading = None;
                self.failed = true;
                None
            }
        }
    }

    /// Takes the catalog when the loader thread is done, and makes its GPU objects. Once a
    /// frame in any cell ([`DistantLand::frame`](super::DistantLand)), and in the statics' own
    /// frame work and wait; a call after the catalog is taken does nothing. Needs the view
    /// layout of [`Statics::set_views`] first.
    pub fn finish_load(&mut self, scene: &SceneView) {
        let Some(view_layout) = self.view_layout.clone() else { return };
        let Some(result) = self.take_catalog() else { return };
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
        let mut interiors: Vec<&[u8]> = catalog
            .worlds
            .keys()
            .map(|name| name.as_slice())
            .filter(|name| !name.is_empty())
            .collect();
        interiors.sort();
        if !interiors.is_empty() {
            let shown = if nirender_wgpu::switches::on("MGE_XE_LIST_WORLDS") == Some(true) {
                interiors.len()
            } else {
                12
            };
            let names: Vec<String> = interiors
                .iter()
                .take(shown)
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect();
            println!(
                "distant statics: interior cells with statics: {}{}",
                names.join("; "),
                if interiors.len() > shown { "; ..." } else { "" }
            );
        }
        self.device = Some(scene.device.clone());
        self.queue = Some(scene.queue.clone());
        self.gpu = Some(super::timed("the distant statics' GPU objects", || {
            make_gpu(scene, &loaded, &view_layout, self.alpha_to_coverage, &self.shadow_hooks)
        }));
        let (request_sender, request_receiver) = channel();
        let (arrival_sender, arrival_receiver) = channel();
        let for_reader = catalog.clone();
        thread::spawn(move || reader(for_reader, request_receiver, arrival_sender));
        self.requests = Some(request_sender);
        self.arrivals = Some(arrival_receiver);
        self.catalog = Some(catalog);
        self.grass_kept = GrassKept::default();
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
        let mut reader_ended = false;
        while self.synchronous || (count < ARRIVALS_IN_FRAME && bytes < ARRIVAL_BYTES_IN_FRAME) {
            // Synchronous: all that was asked for comes now; every request is answered, so
            // the limit of time covers a dead reader thread only.
            let arrival = if self.synchronous {
                if self.requested.is_empty() {
                    break;
                }
                arrivals.recv_timeout(std::time::Duration::from_secs(5)).ok()
            } else {
                match arrivals.try_recv() {
                    Ok(arrival) => Some(arrival),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        reader_ended = true;
                        None
                    }
                }
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
            // A subset with no triangles would be asked for again each frame: it goes with
            // the unreadable ones.
            if arrival.unreadable || arrival.indexes.is_empty() {
                if self.unreadable.is_empty() {
                    println!(
                        "distant statics: subset {} {} statics/static_meshes_{:03}; such a subset is not drawn and not asked for again (the count is in the summary)",
                        arrival.subset,
                        if arrival.unreadable {
                            "cannot be read from"
                        } else {
                            "has no triangles in"
                        },
                        s.shard
                    );
                }
                self.unreadable.insert(arrival.subset);
                continue;
            }
            let size = (arrival.vertices.len() + arrival.indexes.len()) as u64;
            let geometry = if s.pooled() {
                let Some(gpu) = self.gpu.as_mut() else { continue };
                let Some(slot) = gpu.pools.put(device, queue, s.texture, &arrival.vertices, &arrival.indexes) else {
                    if !self.pool_full {
                        self.pool_full = true;
                        println!(
                            "distant statics: {}: the pool of a texture cannot grow for a subset of {size} bytes; it is not drawn until a slot of the pool goes free",
                            self.stamp()
                        );
                    }
                    self.refused.insert(arrival.subset);
                    self.refused_count += 1;
                    continue;
                };
                if let Some(culling) = gpu.culling.as_ref() {
                    let entry = cull::Subset {
                        first_index: slot.first_index,
                        base_vertex: slot.base_vertex,
                        faces: arrival.faces,
                        resident: 1,
                        _pad: [0; 2],
                    };
                    queue.write_buffer(
                        &culling.subsets,
                        arrival.subset as u64 * size_of::<cull::Subset>() as u64,
                        bytemuck::bytes_of(&entry),
                    );
                }
                Geometry::Pool(slot)
            } else {
                let vertices = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("distant static, vertices"),
                    contents: &arrival.vertices,
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let indexes = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("distant static, indexes"),
                    contents: &arrival.indexes,
                    usage: wgpu::BufferUsages::INDEX,
                });
                Geometry::Own { vertices, indexes }
            };
            let arrived = if self.prefetching {
                -1.0e9
            } else {
                self.clock.elapsed().as_secs_f32()
            };
            if let Some(gpu) = self.gpu.as_ref() {
                queue.write_buffer(&gpu.arrivals, arrival.subset as u64 * 4, bytemuck::bytes_of(&arrived));
            }
            if s.grass {
                self.grass_arrived += 1;
                self.grass_bent += arrival.bent as u64;
            }
            let wanted_at = self.clock.elapsed().as_secs_f32();
            self.resident.insert(
                arrival.subset,
                Resident {
                    geometry,
                    faces: arrival.faces,
                    bytes: size,
                    arrived,
                    wanted_at,
                    bent: arrival.bent,
                },
            );
            self.resident_bytes += size;
            self.most_bytes = self.most_bytes.max(self.resident_bytes);
            let again = !self.ever_arrived.insert(arrival.subset);
            if let Some(wait) = self.wait.as_mut() {
                wait.arrived += 1;
                wait.arrived_bytes += size;
                wait.again += again as u32;
            }
            if let Some(&index) = self
                .catalog
                .as_ref()
                .and_then(|catalog| catalog.merged_place.get(&arrival.subset))
            {
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
        if reader_ended {
            self.reader_ended();
        }
    }

    /// Counts a merged subset that arrived in play by the distance of its place: inside the
    /// draw distance it came late, and past the handoff the view shows it late.
    fn count_arrival(&mut self, index: u32, eye: [f32; 3]) {
        let Some(catalog) = self.catalog.as_ref() else { return };
        let instance = &catalog.instances[index as usize];
        if self.waiting {
            self.wait_arrivals += 1;
            return;
        }
        let near = flat_distance(instance, eye) - instance.radius;
        self.play_arrivals += 1;
        let c = instance.center;
        let in_view = self.view_planes.is_some_and(|planes| {
            planes
                .iter()
                .all(|p| p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] >= -instance.radius)
        });
        if in_view {
            self.view_arrivals[((near.max(0.0) / CELL) as usize).min(VIEW_BANDS - 1)] += 1;
        }
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
        if self.failed {
            nirender_wgpu::pass_report::note_skipped(super::part::GRASS, FAILED);
            return;
        }
        if !self.settings.grass {
            nirender_wgpu::pass_report::note_skipped(super::part::GRASS, "the grass is off");
            return;
        }
        let started = Instant::now();
        let drawn = self.draw_grass_timed(scene, pass, frame, GrassTarget::Main);
        nirender_wgpu::pass_report::note_result(super::part::GRASS, drawn);
        self.grass_stats.calls += 1;
        self.grass_stats.seconds += started.elapsed().as_secs_f64();
        self.grass_stats.drawn += self.grass_draws.len() as u64;
    }

    /// Draws the grass of a view of its own, the `k`th call at the end of the world in the
    /// frame: the view group `view::tier_game(k)` has the projection of the game, and the call
    /// has places, uniforms and a group of its own. The view has no shadow on its grass. The
    /// main view's grass counters stay as they are.
    pub fn draw_grass_view(
        &mut self,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &GrassFrame,
        k: usize,
    ) -> GrassView {
        if self.failed {
            nirender_wgpu::pass_report::note_skipped(super::part::GRASS, FAILED);
            return GrassView::default();
        }
        if !self.settings.grass {
            nirender_wgpu::pass_report::note_skipped(super::part::GRASS, "the grass is off");
            return GrassView::default();
        }
        let drawn = self.draw_grass_timed(scene, pass, frame, GrassTarget::View(k));
        GrassView {
            drawn: nirender_wgpu::pass_report::note_result(super::part::GRASS, drawn).unwrap_or(0),
        }
    }

    /// Makes the grass set `k` of the views of their own when it is not made; returns the bytes
    /// of the set made, 0 when it was there or the statics have no GPU objects yet.
    pub fn ensure_grass_set(&mut self, device: &wgpu::Device, k: usize) -> u64 {
        let Some(gpu) = self.gpu.as_mut() else { return 0 };
        if gpu.grass_sets.len() <= k {
            gpu.grass_sets.resize_with(k + 1, || None);
        }
        let mut made = 0;
        if gpu.grass_sets[k].is_none() {
            let uniforms = device.counted_buffer(&wgpu::BufferDescriptor {
                label: Some("grass, a view of its own"),
                size: size_of::<GrassUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let records = device.counted_buffer(&wgpu::BufferDescriptor {
                label: Some("grass, the places of a view of its own"),
                size: (GRASS_RECORDS * GRASS_RECORD) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = make_grass_group(
                device,
                &gpu.grass_layout,
                &uniforms,
                &gpu.grass_sampler,
                &gpu.grass_no_shadow,
                &gpu.grass_shadow_sampler,
            );
            made += uniforms.size() + records.size();
            gpu.grass_sets[k] = Some(GrassSet {
                records,
                uniforms,
                group,
            });
        }
        made
    }

    /// Draws the grass for `target`; returns the places drawn, or why it drew none: no catalog or
    /// GPU objects yet, no view group or set, or no grass in the world space.
    fn draw_grass_timed(
        &mut self,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &GrassFrame,
        target: GrassTarget,
    ) -> Result<usize, nirender_wgpu::pass_report::Skip> {
        let main = matches!(target, GrassTarget::Main);
        // The shadow map is made once in a session: the group takes it at its first frame.
        if let (Some(shadow), Some(gpu), true) = (frame.shadow.as_ref(), self.gpu.as_mut(), main) {
            if !std::rc::Rc::ptr_eq(&gpu.grass_atlas, &shadow.atlas) {
                gpu.grass_group = make_grass_group(
                    scene.device,
                    &gpu.grass_layout,
                    &gpu.grass_uniforms,
                    &gpu.grass_sampler,
                    &shadow.atlas,
                    &gpu.grass_shadow_sampler,
                );
                gpu.grass_atlas = shadow.atlas.clone();
            }
        }
        let (Some(catalog), Some(gpu)) = (self.catalog.clone(), self.gpu.as_ref()) else {
            return Err("no catalog or GPU objects of the statics yet");
        };
        let view_index = match target {
            GrassTarget::Main => super::view::GAME,
            GrassTarget::View(k) => super::view::tier_game(k),
        };
        let Some(view) = self.view_groups.get(view_index).and_then(Option::as_ref) else {
            return Err(super::part::NOT_MADE);
        };
        let (records_buffer, uniforms_buffer, group) = match target {
            GrassTarget::Main => (&gpu.grass_records, &gpu.grass_uniforms, &gpu.grass_group),
            GrassTarget::View(k) => match gpu.grass_sets.get(k).and_then(Option::as_ref) {
                Some(set) => (&set.records, &set.uniforms, &set.group),
                None => return Err("the grass set of the view is not made"),
            },
        };
        let Some(world) = catalog.worlds.get(&self.world) else {
            return Err("the world space has no grass");
        };
        let eye = [frame.eye[0], frame.eye[1], frame.eye[2]];
        let view_range = frame.eye[3];
        self.grass_draws.clear();
        let [now, fade_seconds, _, _] = self.fade_values();
        let mut counts = GrassParts::default();
        let choose_started = Instant::now();
        let sight = GrassSight {
            eye,
            range: view_range,
            planes: frame.planes,
            now,
            fade_seconds,
            limit: GRASS_RECORDS,
        };
        let keep = self.grass_kept.begin(&self.world);
        let (shown, resident) = (&self.shown, &self.resident);
        let cut = world.choose_grass(
            &catalog.grass,
            &sight,
            |group| hidden(shown, group),
            |subset| {
                let r = resident.get(&subset)?;
                Some(GrassSubset {
                    texture: catalog.subsets[subset as usize].texture,
                    arrived: r.arrived,
                    bent: r.bent as u32 as f32,
                })
            },
            &mut self.grass_draws,
            &mut counts,
            keep,
        );
        self.grass_kept.end();
        if main {
            self.grass_frames += 1;
            self.grass_cut += cut as u64;
        }
        counts.choose_seconds = choose_started.elapsed().as_secs_f64();
        counts.draws = self.grass_draws.len() as u64;
        if self.grass_draws.is_empty() {
            self.note_grass(main, &counts);
            return Ok(0);
        }
        let sort_started = Instant::now();
        self.grass_order
            .sort(self.grass_draws.iter().map(|draw| (draw.texture, draw.subset)));
        counts.sort_seconds = sort_started.elapsed().as_secs_f64();
        let records_started = Instant::now();
        let draws = &self.grass_draws;
        self.grass_records.clear();
        self.grass_records
            .extend(self.grass_order.keys.iter().flat_map(|&(_, _, index)| {
                let draw = &draws[index as usize];
                draw.record.into_iter().chain([draw.fade, draw.bent, 0.0, 0.0])
            }));
        if self.grass_hash {
            println!(
                "{}",
                GrassHash::of(&self.grass_records).line(self.grass_frames, main, self.grass_records.len(), cut)
            );
        }
        scene
            .queue
            .write_buffer(records_buffer, 0, bytemuck::cast_slice(&self.grass_records));
        counts.records_seconds = records_started.elapsed().as_secs_f64();
        let setup_started = Instant::now();
        let uniforms = GrassUniforms {
            range: [view_range, 0.0, 0.0, 0.0],
            fog_color: frame.fog_color,
            sun_color: frame.sun_color,
            ambient: frame.ambient,
            wind: frame.wind,
            foot: frame.foot,
            shadow_layers: frame.shadow.as_ref().map_or([[0.0; 16]; 2], |shadow| shadow.layers),
            esm: frame
                .shadow_values
                .esm(frame.shadow.as_ref().map_or(0.0, |shadow| shadow.rcp)),
            shade: frame.shadow_values.shade(),
            shadow: frame.shadow.as_ref().map_or([0.0; 4], |shadow| {
                let c = frame.sun_color;
                [
                    1.0,
                    super::shadow::sun_light([c[0], c[1], c[2]], shadow.sun_vis, shadow.day),
                    shadow.above_sea as u32 as f32,
                    shadow.edge as u32 as f32,
                ]
            }),
            look: frame.look,
        };
        scene.queue.write_buffer(uniforms_buffer, 0, bytemuck::bytes_of(&uniforms));

        // A pass with another count of samples than the main view's (a rendered texture) has the
        // pipeline for one sample, filled.
        let pipeline = if scene.samples != gpu.samples {
            &gpu.grass_picture_pipeline
        } else if scene.wireframe {
            gpu.grass_pipeline_lines.as_ref().unwrap_or(&gpu.grass_pipeline)
        } else {
            &gpu.grass_pipeline
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, records_buffer.slice(..));
        counts.setup_seconds = setup_started.elapsed().as_secs_f64();
        let draw_started = Instant::now();
        // One draw for each run of places with the same subset.
        let mut start = 0;
        while start < self.grass_order.keys.len() {
            let (texture, subset, _) = self.grass_order.keys[start];
            let mut end = start + 1;
            while end < self.grass_order.keys.len() && self.grass_order.keys[end].1 == subset {
                end += 1;
            }
            if let Some(Resident {
                geometry: Geometry::Own { vertices, indexes },
                faces,
                ..
            }) = self.resident.get(&subset)
            {
                pass.set_bind_group(2, &gpu.textures[texture as usize], &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..faces[0] * 3, 0, start as u32..end as u32);
                counts.triangles += faces[0] as u64 * (end - start) as u64;
            }
            counts.runs += 1;
            start = end;
        }
        counts.draw_seconds = draw_started.elapsed().as_secs_f64();
        self.note_grass(main, &counts);
        if main {
            self.grass_drawn += self.grass_draws.len() as u64;
        }
        Ok(self.grass_draws.len())
    }

    /// A wait for the statics is open: the merged subsets that arrive in it are counted apart
    /// from the arrivals in play.
    pub fn set_waiting(&mut self, waiting: bool) {
        self.waiting = waiting;
    }

    /// The time that a subset takes to come in after its arrival, in seconds (the tunable
    /// `statics.fade`); 0 for none.
    pub fn set_fade(&mut self, seconds: f32) {
        self.fade_seconds = seconds.max(0.0);
    }

    /// The fade of the shaders: the time now on the clock of the arrivals, and the fade
    /// time. A replay has no fade: its pictures must not depend on the time of the machine.
    fn fade_values(&self) -> [f32; 4] {
        [
            self.clock.elapsed().as_secs_f32(),
            if self.synchronous { 0.0 } else { self.fade_seconds },
            0.0,
            0.0,
        ]
    }

    /// Adds the counts of a grass call of the main view to the report; the calls of the views
    /// of their own stay out of it.
    fn note_grass(&mut self, main: bool, counts: &GrassParts) {
        if !main {
            return;
        }
        let parts = &mut self.grass_parts;
        parts.calls += 1;
        parts.visited += counts.visited;
        parts.skipped += counts.skipped;
        parts.records_made += counts.records_made;
        parts.hidden += counts.hidden;
        parts.too_far += counts.too_far;
        parts.out_of_view += counts.out_of_view;
        parts.without_resident += counts.without_resident;
        parts.subset_misses += counts.subset_misses;
        parts.draws += counts.draws;
        parts.runs += counts.runs;
        parts.triangles += counts.triangles;
        parts.choose_seconds += counts.choose_seconds;
        parts.sort_seconds += counts.sort_seconds;
        parts.records_seconds += counts.records_seconds;
        parts.setup_seconds += counts.setup_seconds;
        parts.draw_seconds += counts.draw_seconds;
        // The set line of the grass has the same counts under the names of the other sets.
        let stats = &mut self.grass_stats;
        stats.visited += counts.visited;
        stats.distance += counts.hidden + counts.too_far;
        stats.frustum += counts.out_of_view;
        stats.waiting += counts.without_resident;
        stats.runs += counts.runs;
        stats.triangles += counts.triangles;
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
        let Some(world) = catalog.worlds.get(&self.world) else {
            return found;
        };
        let reach = self.settings.draw_distance;
        let admit = reach + CELL;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = ((reach + CELL) / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = world.water_cells.get(&(x, y)) else {
                    continue;
                };
                for index in list {
                    let instance = &catalog.instances[*index as usize];
                    if hidden(&self.shown, instance.group) {
                        continue;
                    }
                    let subset = &catalog.subsets[instance.subset as usize];
                    let Some(look) = subset.look else { continue };
                    let to = [
                        instance.center[0] - eye[0],
                        instance.center[1] - eye[1],
                        instance.center[2] - eye[2],
                    ];
                    if !subset.components.is_empty()
                        && (to[0] * to[0] + to[1] * to[1]).sqrt() <= admit + instance.radius
                        && self.may_ask(instance.subset)
                    {
                        self.wanted.push(instance.subset);
                    }
                    let Some(Resident {
                        geometry: Geometry::Own { vertices, indexes },
                        faces,
                        ..
                    }) = self.resident.get(&instance.subset)
                    else {
                        continue;
                    };
                    let range = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                    if range > reach + instance.radius {
                        continue;
                    }
                    if !planes.iter().all(|p| {
                        p[0] * instance.center[0] + p[1] * instance.center[1] + p[2] * instance.center[2] + p[3]
                            >= -instance.radius
                    }) {
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
                        texture: self
                            .gpu
                            .as_ref()
                            .and_then(|gpu| gpu.texture_views.get(subset.texture as usize).cloned()),
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

    /// Takes the view groups of the set `set` of the views of their own from `views`, after
    /// [`ViewBinding::ensure_set`](super::view::ViewBinding::ensure_set) made them. The groups
    /// of the frame come with [`set_views`](Self::set_views); a set made later in the frame
    /// comes here.
    pub fn set_view_set(&mut self, views: &super::view::ViewBinding, set: usize) {
        for number in [super::view::tier(set), super::view::tier_game(set)] {
            if self.view_groups.len() <= number {
                self.view_groups.resize_with(number + 1, || None);
            }
            self.view_groups[number] = views.group(number).cloned();
        }
    }

    /// Tells the statics how many of the sets of the views of their own are places (the rest
    /// are spares), for the names of the report.
    pub fn set_places(&mut self, places: usize) {
        self.tier_places = places;
    }

    /// The name of the set `set` in the report.
    fn set_name(&self, set: usize) -> String {
        match set.checked_sub(FIXED_SETS) {
            None => SET_NAMES[set].to_string(),
            Some(k) if k < self.tier_places => format!("view of its own {}", k + 1),
            Some(k) => format!("spare {}", k + 1 - self.tier_places),
        }
    }

    /// The bytes that the cull buffers of the set `set` of the views of their own would take
    /// now, 0 when they exist or the statics cannot draw (no GPU culling, no world space).
    pub fn view_set_bytes_needed(&self, set: usize) -> u64 {
        let (Some(gpu), Some(catalog)) = (self.gpu.as_ref(), self.catalog.as_ref()) else {
            return 0;
        };
        let Some(culling) = gpu.culling.as_ref() else { return 0 };
        match (culling.has_world(&self.world), culling.has_set(tier_set(set))) {
            (true, true) => 0,
            (true, false) => culling.next_set_bytes(),
            // The list of the world space is not made yet: all the places of the catalog are
            // an upper bound of its length.
            (false, _) => cull::set_bytes(catalog.instances.len() as u32, catalog.textures.len()),
        }
    }

    /// The bytes of the statics sets of the views of their own that exist: the cull buffers and
    /// the grass sets.
    pub fn views_bytes(&self) -> u64 {
        let Some(gpu) = self.gpu.as_ref() else { return 0 };
        gpu.culling.as_ref().map_or(0, cull::Culling::views_bytes)
            + gpu.grass_sets.iter().flatten().count() as u64 * GRASS_SET_BYTES
    }

    /// The bytes that the grass set `k` would take now, 0 when it exists.
    pub fn grass_set_bytes_needed(&self, k: usize) -> u64 {
        match self.gpu.as_ref() {
            Some(gpu) if gpu.grass_sets.get(k).is_none_or(Option::is_none) => GRASS_SET_BYTES,
            _ => 0,
        }
    }

    /// The frees of the parts of a set so far, counted by the freeing calls themselves.
    #[cfg(test)]
    pub fn releases(&self) -> ReleaseCounts {
        self.releases
    }

    /// Frees the statics set `k` of the views of their own: the uniforms and the group of group 1,
    /// and the cull buffers.
    pub fn release_view_set(&mut self, k: usize) {
        #[cfg(test)]
        {
            self.releases.view_set += 1;
        }
        let statics_set = tier_set(k);
        if let Some(gpu) = self.gpu.as_mut() {
            if let Some(buffers) = gpu.sets.get_mut(statics_set) {
                *buffers = None;
            }
            if let Some(culling) = gpu.culling.as_mut() {
                culling.release_set(statics_set);
            }
        }
        if let Some(prepared) = self.prepared.get_mut(statics_set) {
            *prepared = false;
        }
    }

    /// Frees the grass set `k` of the views of their own.
    pub fn release_grass_set(&mut self, k: usize) {
        #[cfg(test)]
        {
            self.releases.grass_set += 1;
        }
        if let Some(grass) = self.gpu.as_mut().and_then(|gpu| gpu.grass_sets.get_mut(k)) {
            *grass = None;
        }
    }

    /// Forgets the two view groups of the set `k` of the views of their own, which the view
    /// binding freed ([`ViewBinding::release_set`](super::view::ViewBinding::release_set)).
    pub fn release_view_groups(&mut self, k: usize) {
        #[cfg(test)]
        {
            self.releases.view_groups += 1;
        }
        for number in [super::view::tier(k), super::view::tier_game(k)] {
            if let Some(group) = self.view_groups.get_mut(number) {
                *group = None;
            }
        }
    }

    /// Chooses the places of the sets of a frame on the GPU, in one command buffer before the
    /// passes that draw them, and does the work of the frame for the statics once: the main
    /// view first. A set chosen here is only drawn by `draw`; a set that is not here is chosen
    /// in its `draw`, as without this call.
    ///
    /// Answers why the cull did not run.
    pub fn prepare(&mut self, scene: &SceneView, sets: &[CullSet]) -> Result<(), Skip> {
        self.frame = scene.frame;
        self.prepared.fill(false);
        let first = sets.first().ok_or("no set to cull")?;
        if self.failed {
            return Err(FAILED);
        }
        if !self.settings.statics {
            return Err(OFF);
        }
        if first.set == 0 {
            self.view_planes = Some(first.planes);
        }
        let started = Instant::now();
        self.game_range = first.eye[3];
        let culled = self
            .frame_work(scene, [first.eye[0], first.eye[1], first.eye[2]], true)
            .and_then(|()| {
                let catalog = self.catalog.clone();
                match (catalog, self.gpu.as_mut()) {
                    (None, _) => Err(NO_CATALOG),
                    (_, None) => Err(NO_GPU),
                    (Some(catalog), Some(gpu)) => match (gpu.culling.as_mut(), catalog.worlds.get(&self.world)) {
                        (None, _) => Err("no GPU cull"),
                        (_, None) => Err(NO_WORLD),
                        (Some(culling), Some(world)) => {
                            if !culling.has_world(&self.world) {
                                culling.set_world(scene.device, &self.world, &world_places(&catalog, world));
                            }
                            let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("distant statics, cull"),
                            });
                            let at = scene.commands.stamps().begin(
                                &mut encoder,
                                "distant statics: the GPU cull of all sets",
                                nirender_wgpu::stamps::StampKind::Detail,
                            );
                            for cull_set in sets {
                                let values = cull_values(&self.settings, cull_set.planes, cull_set.eye, cull_set.fog_end);
                                culling.encode(scene.queue, &mut encoder, cull_set.set, &values);
                                mark_prepared(&mut self.prepared, cull_set.set);
                            }
                            scene.commands.stamps().end(&mut encoder, at);
                            scene.commands.defer(encoder.finish());
                            Ok(())
                        }
                    },
                }
            });
        self.prepare_calls += 1;
        self.prepare_seconds += started.elapsed().as_secs_f64();
        culled
    }

    /// Chooses the places of one view of its own on the GPU (`cull_set.set` from [`tier_set`]),
    /// in a command buffer of its own before the commands of the frame. It does no work of the
    /// frame (the arrivals, the residency, the budget): that is the main view's, and keyed to
    /// its eye, so a view far from the main eye draws only the subsets that are resident for
    /// that eye. The report line of the set counts these places as "not on the GPU". The buffers
    /// of the set are made by [`ensure_view_set`](Self::ensure_view_set) first. Answers why the
    /// statics cannot draw the set (off, failed, not a set of a view, no view group, no GPU
    /// state, no buffers of the set, no GPU culling of it).
    pub fn prepare_view(&mut self, scene: &SceneView, cull_set: &CullSet) -> Result<(), Skip> {
        if self.failed {
            return Err(FAILED);
        }
        if !self.settings.statics {
            return Err(OFF);
        }
        if cull_set.set < tier_set(0) {
            return Err("not a set of a view of its own");
        }
        // The view groups come with the main view's call, and the group of a set with its first
        // call (`set_view_set`); before them the draw has no group 0.
        group_of(&self.view_groups, cull_set.set).ok_or("no view group of the set")?;
        let gpu = self.gpu.as_mut().ok_or(NO_GPU)?;
        gpu.sets
            .get(cull_set.set)
            .and_then(Option::as_ref)
            .ok_or("no buffers of the set")?;
        let culling = gpu.culling.as_mut().ok_or("no GPU cull")?;
        if !culling.has_set(cull_set.set) {
            return Err("no GPU cull of the set");
        }
        let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("distant statics, cull of a view of its own"),
        });
        let at = scene.commands.stamps().begin(
            &mut encoder,
            "distant statics: the GPU cull of a view of its own",
            nirender_wgpu::stamps::StampKind::Detail,
        );
        culling.encode(
            scene.queue,
            &mut encoder,
            cull_set.set,
            &cull_values(&self.settings, cull_set.planes, cull_set.eye, cull_set.fog_end),
        );
        scene.commands.stamps().end(&mut encoder, at);
        scene.commands.defer(encoder.finish());
        mark_prepared(&mut self.prepared, cull_set.set);
        Ok(())
    }

    /// Makes what the statics need for the set `k` of the views of their own (its statics set is
    /// [`tier_set`]`(k)`): the list of the places of the world space when it is not there, the
    /// cull buffers, and the uniforms and the group of group 1. Returns the bytes of the cull
    /// buffers that this call made, or `None` when the statics cannot draw the set (off, failed,
    /// no catalog, no world space, no GPU culling). The caller makes it inside an out-of-memory
    /// scope and after the size rule (see [`view_set_bytes_needed`](Self::view_set_bytes_needed)).
    pub fn ensure_view_set(&mut self, device: &wgpu::Device, k: usize) -> Option<u64> {
        if self.failed || !self.settings.statics {
            return None;
        }
        let set = tier_set(k);
        let catalog = self.catalog.clone()?;
        let world = catalog.worlds.get(&self.world)?;
        let gpu = self.gpu.as_mut()?;
        let culling = gpu.culling.as_mut()?;
        if !culling.has_world(&self.world) {
            culling.set_world(device, &self.world, &world_places(&catalog, world));
        }
        let made = culling.ensure_set(device, set);
        if gpu.sets.len() <= set {
            gpu.sets.resize_with(set + 1, || None);
        }
        if gpu.sets[set].is_none() {
            gpu.sets[set] = Some(gpu.make_buffer_set(device));
        }
        Some(made)
    }

    /// Draws the statics of the frame into the pass. Returns the number of draws (0: nothing in
    /// view), or why the set was not drawn.
    pub fn draw(
        &mut self,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &Frame,
        set: usize,
    ) -> Result<u32, Skip> {
        let started = Instant::now();
        let draws = self.draw_timed(scene, pass, frame, set);
        if self.set_stats.len() <= set {
            self.set_stats.resize(set + 1, SetStats::default());
        }
        let stats = &mut self.set_stats[set];
        stats.calls += 1;
        stats.seconds += started.elapsed().as_secs_f64();
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
        let Some(world) = catalog.worlds.get(&self.world) else {
            return;
        };
        let admit = self.settings.draw_distance + CELL;
        let now = self.clock.elapsed().as_secs_f32();
        let all = self.residency == StaticsResidency::All && self.all_world.as_deref() != Some(self.world.as_slice());
        if all {
            self.all_world = Some(self.world.clone());
            let bytes: u64 = world
                .merged
                .iter()
                .map(|&index| catalog.subsets[catalog.instances[index as usize].subset as usize].bytes())
                .sum();
            let high = self.high_mark();
            if bytes <= high {
                println!(
                    "distant statics: residency all: the {:.0} MB of merged subsets of the world space fit under the high mark of {:.0} MB; all are asked for",
                    bytes as f64 / 1e6,
                    high as f64 / 1e6
                );
            } else {
                println!(
                    "distant statics: residency all: the {:.0} MB of merged subsets of the world space do not fit under the high mark of {:.0} MB; as auto",
                    bytes as f64 / 1e6,
                    high as f64 / 1e6
                );
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
                if let (Some(gpu), Some(device), Some(queue)) =
                    (self.gpu.as_mut(), self.device.as_ref(), self.queue.as_ref())
                {
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

    /// The frame and the time of a memory decision now ([`event_stamp`]).
    fn stamp(&self) -> String {
        event_stamp(self.frame, qpc())
    }

    /// The most bytes of merged subsets that stay on the GPU before the trim takes some off:
    /// `distant_land.statics_resident_mb`, or half of the first cap of the session; never over
    /// the cap as it is now (another program that takes memory lowers it).
    fn high_mark(&self) -> u64 {
        let mark = if self.resident_mark > 0 {
            self.resident_mark
        } else {
            self.first_cap / 2
        };
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
                (now - resident.wanted_at > KEEP_SECONDS && distance > admit + instance.radius).then_some((
                    resident.wanted_at,
                    distance,
                    *subset,
                    resident.bytes,
                ))
            })
            .collect();
        let (before, mut count) = (self.merged_bytes, 0);
        for subset in pick_trim(candidates, self.merged_bytes, low) {
            if let Some(resident) = self.take_off(subset) {
                self.merged_bytes -= resident.bytes;
                self.trimmed += 1;
                count += 1;
            }
        }
        if count > 0 {
            println!(
                "{}",
                memory_event(
                    &self.stamp(),
                    "trimmed (wanted least recently)",
                    count,
                    before,
                    self.merged_bytes,
                    &bytes_limit("the high mark", high)
                )
            );
        }
    }

    /// The work of a frame with no world (a loading screen) for the place where the camera
    /// will be: the world space by its name (empty for the exterior) and the eye. The
    /// subsets near it are asked for and taken as in a frame, so a wait on the loading screen
    /// ends when they are there (`settled`).
    pub fn prefetch(&mut self, scene: &SceneView, world: &[u8], eye: [f32; 3]) {
        self.frame = scene.frame;
        // At the first load of a session the catalog comes in a frame with no world.
        self.finish_load(scene);
        if self.failed || !self.loaded() {
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

    /// The work of the frame for the statics. Answers why the statics cannot draw: no catalog, or
    /// no world space of the cell in it.
    fn frame_work(&mut self, scene: &SceneView, eye: [f32; 3], main: bool) -> Result<(), Skip> {
        self.frame = scene.frame;
        self.finish_load(scene);
        // The slots that went in the frame before are free: the draws that read them are done.
        if self.frame_ended {
            self.frame_ended = false;
            if let Some(gpu) = self.gpu.as_mut() {
                let freed = gpu.pools.next_frame();
                // Those pools have room again: the subsets they refused may be asked for.
                if let (false, Some(catalog)) = (freed.is_empty(), self.catalog.as_ref()) {
                    self.refused
                        .retain(|refused| !freed.contains(&catalog.subsets[*refused as usize].texture));
                }
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
        let catalog = self.catalog.clone().ok_or(NO_CATALOG)?;
        if !catalog.worlds.contains_key(&self.world) {
            return Err(NO_WORLD);
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
            let (before, mut count) = (self.merged_bytes, 0);
            for subset in far {
                if let Some(resident) = self.take_off(subset) {
                    self.merged_bytes -= resident.bytes;
                    count += 1;
                }
            }
            if count > 0 {
                let limit = format!("farther than {retain:.0} units and the subset's radius");
                println!(
                    "{}",
                    memory_event(&self.stamp(), "taken off, too far", count, before, self.merged_bytes, &limit)
                );
            }
        }
        self.frames += 1;
        if main {
            self.frame_ended = true;
        }
        Ok(())
    }

    /// Binds what the draws of a set read: its pipeline, its uniforms and the places. Gives false
    /// when the set has no uniforms (a set of the views of their own that is not made): nothing
    /// is bound then.
    fn bind_set(
        gpu: &Gpu,
        view_group: Option<&wgpu::BindGroup>,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &Frame,
        set: usize,
        fade: [f32; 4],
    ) -> bool {
        let Some(buffers) = gpu.sets.get(set).and_then(Option::as_ref) else {
            return false;
        };
        if let Some(view_group) = view_group {
            pass.set_bind_group(0, view_group, &[]);
        }
        scene
            .queue
            .write_buffer(&buffers.uniforms, 0, bytemuck::bytes_of(&uniforms_of(frame, fade)));
        // Lines when the game draws in wireframe (see `SceneView::wireframe`), as MGE XE on
        // Direct3D 9 draws with the fill mode the engine left; the shadow map stays filled.
        // A pass with another count of samples than the main view's (a rendered texture) has the
        // pipeline for one sample, filled: the lines pipeline has the samples of the frame.
        let own_samples = scene.samples != gpu.samples;
        let (fill, lines) = match set {
            1 => (&gpu.mirror_pipeline, &gpu.mirror_pipeline_lines),
            _ if own_samples => (&gpu.picture_pipeline, &None),
            _ => (&gpu.pipeline, &gpu.pipeline_lines),
        };
        let shadow = matches!(set, 2 | 3);
        pass.set_pipeline(if shadow {
            &gpu.shadow_pipeline
        } else if scene.wireframe {
            lines.as_ref().unwrap_or(fill)
        } else {
            fill
        });
        pass.set_bind_group(1, &buffers.group, &[]);
        pass.set_vertex_buffer(1, gpu.instances.slice(..));
        true
    }

    /// The indirect draws of a set that the GPU chose.
    fn draw_chosen(
        gpu: &Gpu,
        view_group: Option<&wgpu::BindGroup>,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &Frame,
        set: usize,
        fade: [f32; 4],
    ) -> u32 {
        let Some(culling) = gpu.culling.as_ref() else { return 0 };
        if !Self::bind_set(gpu, view_group, scene, pass, frame, set, fade) {
            return 0;
        }
        let (pools, textures) = (&gpu.pools, &gpu.textures);
        culling.draw(set, pass, |pass, texture| {
            let Some((vertices, indexes)) = pools.buffers(texture) else {
                return false;
            };
            pass.set_bind_group(2, &textures[texture as usize], &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
            true
        })
    }

    fn draw_timed(
        &mut self,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        frame: &Frame,
        set: usize,
    ) -> Result<u32, Skip> {
        if self.failed {
            return Err(FAILED);
        }
        if !self.settings.statics {
            return Err(OFF);
        }
        let fade = self.fade_values();
        // Chosen in `prepare`: only the draws.
        if self.prepared.get(set).copied().unwrap_or(false) {
            self.prepared[set] = false;
            let gpu = self.gpu.as_ref().ok_or(NO_GPU)?;
            return Ok(Self::draw_chosen(
                gpu,
                group_of(&self.view_groups, set),
                scene,
                pass,
                frame,
                set,
                fade,
            ));
        }
        let eye = [frame.eye[0], frame.eye[1], frame.eye[2]];
        if set == 0 {
            self.view_planes = Some(frame.planes);
        }
        self.frame_work(scene, eye, set == 0)?;
        let catalog = self.catalog.clone().ok_or(NO_CATALOG)?;
        let world = catalog.worlds.get(&self.world).ok_or(NO_WORLD)?;
        let settings = self.settings;

        // The places chosen on the GPU, in a command buffer of this set.
        if let Some(gpu) = self.gpu.as_mut() {
            if let Some(culling) = gpu.culling.as_mut() {
                if !culling.has_world(&self.world) {
                    culling.set_world(scene.device, &self.world, &world_places(&catalog, world));
                }
                culling.run(scene, set, &cull_values(&settings, frame.planes, frame.eye, frame.fog_end));
                return Ok(Self::draw_chosen(
                    gpu,
                    group_of(&self.view_groups, set),
                    scene,
                    pass,
                    frame,
                    set,
                    fade,
                ));
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
                    let to = [
                        instance.center[0] - eye[0],
                        instance.center[1] - eye[1],
                        instance.center[2] - eye[2],
                    ];
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
                    if !frame.planes.iter().all(|p| {
                        p[0] * instance.center[0] + p[1] * instance.center[1] + p[2] * instance.center[2] + p[3]
                            >= -instance.radius
                    }) {
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
        stats.runs = self
            .draws
            .iter()
            .zip(self.draws.iter().skip(1))
            .filter(|(a, b)| (a.0, a.1) != (b.0, b.1))
            .count() as u64
            + (!self.draws.is_empty()) as u64;
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
            return Ok(0);
        }
        let gpu = self.gpu.as_ref().ok_or(NO_GPU)?;
        if !Self::bind_set(gpu, group_of(&self.view_groups, set), scene, pass, frame, set, fade) {
            return Err("no uniforms of the set (a set of a view that is not made)");
        }
        let mut texture = u32::MAX;
        for &(draw_texture, subset, instance, faces) in &self.draws {
            let Some(Resident {
                geometry: Geometry::Pool(slot),
                ..
            }) = self.resident.get(&subset)
            else {
                continue;
            };
            if draw_texture != texture {
                let Some((vertices, indexes)) = gpu.pools.buffers(draw_texture) else {
                    continue;
                };
                texture = draw_texture;
                pass.set_bind_group(2, &gpu.textures[texture as usize], &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indexes.slice(..), wgpu::IndexFormat::Uint16);
            }
            pass.draw_indexed(
                slot.first_index..slot.first_index + faces * 3,
                slot.base_vertex as i32,
                instance..instance + 1,
            );
        }
        self.drawn += self.draws.len() as u64;
        Ok(self.draws.len() as u32)
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
    SetUniforms {
        range: [frame.eye[3], 0.0, 0.0, 0.0],
        handoff: [frame.time, frame.eye[3] - HANDOFF, HANDOFF_BAND, HANDOFF_CULL],
        fade,
    }
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
            if self.merged_cap == u64::MAX {
                "none".to_string()
            } else {
                format!("{:.0} MB", self.merged_cap as f64 / 1e6)
            },
            self.lowered,
            self.raised,
            self.evicted,
            self.capped_frames,
            self.residency,
            if self.high_mark() >= u64::MAX / 2 {
                "none".to_string()
            } else {
                format!("{:.0} MB", self.high_mark() as f64 / 1e6)
            },
            self.trimmed
        );
        if !self.unreadable.is_empty() || self.refused_count > 0 {
            println!(
                "distant statics: {} subsets could not be read, {} arrivals found no room in a full pool",
                self.unreadable.len(),
                self.refused_count
            );
        }
        println!(
            "grass: {} frames, {:.0} records in a frame as a mean, {} frames with more than {GRASS_RECORDS}; {} subsets arrived, {} of them with bent normals",
            self.grass_frames,
            self.grass_drawn as f64 / self.grass_frames.max(1) as f64,
            self.grass_cut,
            self.grass_arrived,
            self.grass_bent
        );
        println!(
            "distant statics: {} places of hidden vis groups not drawn",
            self.hidden_skipped
        );
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
    let game = match nirender_wgpu::switches::path("MGE_XE_GAME_DIR") {
        Some(dir) => dir,
        None => match folder.parent().and_then(Path::parent) {
            Some(game) => game.to_path_buf(),
            None => return,
        },
    };
    let options = distantland::vfs::VfsLoadOptions {
        morrowind_ini: Some(game.join("Morrowind.ini")),
        ..Default::default()
    };
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

fn make_gpu(
    scene: &SceneView,
    loaded: &Loaded,
    view_layout: &wgpu::BindGroupLayout,
    alpha_to_coverage: bool,
    shadow_hooks: &str,
) -> Gpu {
    let device = scene.device;
    let catalog = &loaded.catalog;
    let make_uniforms = || {
        device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics"),
            size: size_of::<SetUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let uniform_buffers: [wgpu::Buffer; FIXED_SETS] = core::array::from_fn(|_| make_uniforms());
    let palettes = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the palettes"),
        contents: bytemuck::cast_slice(&catalog.palettes),
        usage: wgpu::BufferUsages::STORAGE,
    });
    // Long ago: a subset that is there from the start of a replay is in.
    let arrivals = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the arrivals"),
        contents: bytemuck::cast_slice(&vec![-1.0e9f32; catalog.subsets.len().max(1)]),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let sampler = device.counted_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    let layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant statics"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let texture_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distant statics"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: palettes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: arrivals.as_entire_binding(),
                },
            ],
        })
    });

    // A texture that is not there, or that this program does not read, is white.
    let white = device
        .counted_texture_with_data(
            scene.queue,
            &wgpu::TextureDescriptor {
                label: Some("distant statics, no texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
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
        .counted_view(&Default::default());
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
        textures.push(device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distant statics, textures"),
            layout: &texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        }));
        texture_views.push(view);
    }
    if !missing.is_empty() {
        println!(
            "distant statics: {} textures are white: {}",
            missing.len(),
            missing.iter().take(6).cloned().collect::<Vec<_>>().join("; ")
        );
    }

    let source = format!("{}{SET_WGSL}{SHADER}{}", super::view::wgsl(), super::FOG_FUNCTION);
    let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("distant statics"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    // Group 0: the view group of the set (`view::ViewBinding`); group 1: the values of the
    // set; group 2: the textures of a draw.
    let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
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
    let make_pipeline = |cull: wgpu::Face,
                         polygon_mode: wgpu::PolygonMode,
                         multisample: wgpu::MultisampleState,
                         write_mask: wgpu::ColorWrites| {
        device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("distant statics"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 80,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &place,
                    }),
                ],
            },
            // `CullMode = CW` of the pass of MGE XE: a triangle with its corners clockwise on
            // the screen is not drawn.
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(cull),
                polygon_mode,
                ..Default::default()
            },
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene.target_format,
                    blend: None,
                    write_mask,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let pipeline = make_pipeline(
        wgpu::Face::Back,
        wgpu::PolygonMode::Fill,
        scene.multisample(alpha_to_coverage),
        wgpu::ColorWrites::COLOR,
    );
    let mirror_pipeline = make_pipeline(
        wgpu::Face::Front,
        wgpu::PolygonMode::Fill,
        Default::default(),
        wgpu::ColorWrites::ALL,
    );
    // A view of its own into a rendered texture: one sample, the winding of the game.
    let picture_pipeline = make_pipeline(
        wgpu::Face::Back,
        wgpu::PolygonMode::Fill,
        Default::default(),
        wgpu::ColorWrites::COLOR,
    );
    // The same as lines, for a frame in which the game draws in wireframe (see
    // `SceneView::wireframe`); a device without the line mode draws them filled then.
    let lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
    let pipeline_lines = lines.then(|| {
        make_pipeline(
            wgpu::Face::Back,
            wgpu::PolygonMode::Line,
            scene.multisample(alpha_to_coverage),
            wgpu::ColorWrites::COLOR,
        )
    });
    let mirror_pipeline_lines = lines.then(|| {
        make_pipeline(
            wgpu::Face::Front,
            wgpu::PolygonMode::Line,
            Default::default(),
            wgpu::ColorWrites::ALL,
        )
    });
    // The casters of the shadow map: the same vertices and places, the depth from the sun.
    let shadow_source = format!("{}{SET_WGSL}{SHADOW_SHADER}", super::view::wgsl());
    let shadow_module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("distant statics shadow"),
        source: wgpu::ShaderSource::Wgsl(shadow_source.into()),
    });
    let shadow_pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant statics shadow"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shadow_module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vertex,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: 80,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &place,
                }),
            ],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: super::shadow::Shadow::depth_format(scene.depth_format),
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
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sets: Vec<Option<BufferSet>> = groups
        .into_iter()
        .zip(uniform_buffers)
        .map(|(group, uniforms)| Some(BufferSet { group, uniforms }))
        .collect();

    // The values of every place for the draws, once: the world matrix and the values of the
    // subset. A draw names its place by its first instance.
    let mut instance_data: Vec<f32> = Vec::with_capacity(catalog.instances.len().max(1) * 20);
    for instance in &catalog.instances {
        let s = &catalog.subsets[instance.subset as usize];
        instance_data.extend_from_slice(&instance.world);
        instance_data.extend_from_slice(&[
            s.palette as f32,
            s.has_alpha as u32 as f32,
            s.animate_uv as u32 as f32,
            instance.subset as f32,
        ]);
    }
    if instance_data.is_empty() {
        instance_data.resize(20, 0.0);
    }
    let instances = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant statics, the places"),
        contents: bytemuck::cast_slice(&instance_data),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let pools = cull::Pools::new(catalog.textures.len(), device);
    let culling = if cull::supported(device) {
        let places: Vec<cull::Place> = catalog
            .instances
            .iter()
            .map(|instance| cull::Place {
                center: instance.center,
                radius: instance.radius,
                subset: instance.subset,
                tier: instance.tier as u32,
                group: instance.group as u32,
                _pad: 0,
            })
            .collect();
        println!("distant statics: the places are chosen on the GPU");
        Some(cull::Culling::new(
            device,
            &places,
            catalog.subsets.len(),
            catalog.textures.len(),
        ))
    } else {
        println!(
            "distant statics: the places are chosen on the CPU (the device has no first instance or count for indirect draws)"
        );
        None
    };

    // The grass: its own uniforms and shader, and the textures of the statics.
    let grass_uniforms = device.counted_buffer(&wgpu::BufferDescriptor {
        label: Some("grass"),
        size: size_of::<GrassUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let grass_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grass"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    // The shadow map is made at the first frame with shadows; until then the group has a
    // texture of one texel in its place (the shader reads no shadow without a map).
    let grass_shadow_sampler = device.counted_sampler(&wgpu::SamplerDescriptor {
        label: Some("grass, the shadow map"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let no_shadow = std::rc::Rc::new(
        device
            .counted_texture(&wgpu::TextureDescriptor {
                label: Some("grass, no shadow map"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .counted_view(&Default::default()),
    );
    let grass_group = make_grass_group(
        device,
        &grass_layout,
        &grass_uniforms,
        &sampler,
        &no_shadow,
        &grass_shadow_sampler,
    );
    let grass_source = grass_source(shadow_hooks);
    let grass_module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("grass"),
        source: wgpu::ShaderSource::Wgsl(grass_source.into()),
    });
    let grass_pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grass"),
        bind_group_layouts: &[Some(view_layout), Some(&grass_layout), Some(&texture_layout)],
        immediate_size: 0,
    });
    let grass_place = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
    let make_grass_pipeline = |polygon_mode: wgpu::PolygonMode, multisample: wgpu::MultisampleState| {
        device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grass"),
            layout: Some(&grass_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &grass_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: GRASS_RECORD as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &grass_place,
                    }),
                ],
            },
            // `CullMode = none` of the grass pass: the two sides of a blade.
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                polygon_mode,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: scene.depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(nirender_wgpu::depth::compare(wgpu::CompareFunction::LessEqual)),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            // The alpha of the blade is the coverage of its pixel with `alpha_to_coverage`; the
            // alpha of the back buffer stays 1.
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &grass_module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("ALPHA_TO_COVERAGE", multisample.alpha_to_coverage_enabled as u32 as f64)],
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene.target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let grass_pipeline = make_grass_pipeline(wgpu::PolygonMode::Fill, scene.multisample(alpha_to_coverage));
    let grass_pipeline_lines =
        lines.then(|| make_grass_pipeline(wgpu::PolygonMode::Line, scene.multisample(alpha_to_coverage)));
    let grass_picture_pipeline = make_grass_pipeline(wgpu::PolygonMode::Fill, Default::default());
    let grass_records = device.counted_buffer(&wgpu::BufferDescriptor {
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
        set_layout: layout,
        palettes,
        set_sampler: sampler.clone(),
        samples: scene.samples,
        picture_pipeline,
        textures,
        texture_views,
        instances,
        arrivals,
        pools,
        culling,
        grass_pipeline,
        grass_pipeline_lines,
        grass_picture_pipeline,
        grass_sets: Vec::new(),
        grass_no_shadow: no_shadow.clone(),
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
fn make_grass_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    atlas: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.counted_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grass"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(atlas),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(atlas_sampler),
            },
        ],
    })
}

#[cfg(test)]
mod shader_tests {
    use super::{SET_WGSL, SHADER, SHADOW_SHADER};

    /// The shaders of the distant statics compile as the device checks them: the statics of
    /// a set, their caster of the shadow map, and the grass.
    #[test]
    fn the_shaders_of_the_statics_are_valid() {
        let check = crate::renderer::shader_check::check;
        let view = crate::renderer::view::wgsl();
        check(
            "distant statics",
            &format!("{view}{SET_WGSL}{SHADER}{}", crate::renderer::FOG_FUNCTION),
        )
        .unwrap();
        check("distant statics shadow", &format!("{view}{SET_WGSL}{SHADOW_SHADER}")).unwrap();
        check(
            "grass",
            &super::grass_source(crate::renderer::core_mods::CoreMods::built_in().shadow_hooks()),
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CELL, GRASS_KEEP_CALLS, GRASS_RECORDS, GrassDraw, GrassHash, GrassKept, GrassMiss, GrassOrder, GrassParts,
        GrassPlace, GrassSight, GrassSubset, GrassTile, Resident, World, grass_record, world_matrix,
    };

    /// A record of 16 floats that starts at `first` and rises by one.
    fn rising_record(first: f32) -> Vec<f32> {
        (0..16).map(|index| first + index as f32).collect()
    }

    /// The hash of known records. The numbers come from a second implementation of the hash (FNV-1a
    /// on the bits of the floats, written in Python), not from this one.
    #[test]
    fn the_record_hash_has_known_values() {
        assert_eq!(
            GrassHash::of(&[]),
            GrassHash {
                ordered: 0xcbf29ce484222325,
                unordered: 0
            }
        );
        assert_eq!(
            GrassHash::of(&[0.0; 16]),
            GrassHash {
                ordered: 0x1bfc788be40011c0,
                unordered: 0x88201fb960ff6465
            }
        );
        assert_eq!(
            GrassHash::of(&rising_record(0.0)),
            GrassHash {
                ordered: 0xebffe88ca04011c0,
                unordered: 0x9b3cfa2d13bf6465
            }
        );
        let two: Vec<f32> = [rising_record(0.0), rising_record(0.5)].concat();
        assert_eq!(
            GrassHash::of(&two),
            GrassHash {
                ordered: 0xd97f375d8634e75f,
                unordered: 0xd9548e063fdec8ca
            }
        );
    }

    /// Two records in the other order change the ordered hash and keep the unordered one: the
    /// hash shows a fault of the order that a picture does not show. A changed value, or a zero of
    /// the other sign, changes both.
    #[test]
    fn the_ordered_hash_sees_the_order_and_the_unordered_hash_does_not() {
        let forward: Vec<f32> = [rising_record(0.0), rising_record(0.5)].concat();
        let backward: Vec<f32> = [rising_record(0.5), rising_record(0.0)].concat();
        let (a, b) = (GrassHash::of(&forward), GrassHash::of(&backward));
        assert_ne!(a.ordered, b.ordered);
        assert_eq!(a.unordered, b.unordered);
        assert_eq!(
            b,
            GrassHash {
                ordered: 0x5e661f88f7f4e75f,
                unordered: 0xd9548e063fdec8ca
            }
        );

        let mut changed = forward.clone();
        changed[31] += 0.25;
        let c = GrassHash::of(&changed);
        assert_ne!(c.ordered, a.ordered);
        assert_ne!(c.unordered, a.unordered);

        let (zero, negative_zero) = (GrassHash::of(&[0.0; 16]), GrassHash::of(&[-0.0; 16]));
        assert_ne!(zero.ordered, negative_zero.ordered);
    }

    /// The report line keeps the form of the first measurement (the proof of the order of 2026-11-08),
    /// so that old and new lines compare.
    #[test]
    fn the_hash_line_has_the_form_of_the_first_measurement() {
        let hash = GrassHash::of(&rising_record(0.0));
        assert_eq!(
            hash.line(7, true, 16 * 3, true),
            "grass hash frame 7 main: 3 records, cut true, ordered ebffe88ca04011c0, unordered 9b3cfa2d13bf6465"
        );
        assert!(
            hash.line(1, false, 0, false)
                .starts_with("grass hash frame 1 view: 0 records, cut false, ordered ")
        );
    }

    /// A small generator of numbers for the tests: the same sequence on each run.
    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*state >> 40) as f32) / (1u64 << 24) as f32
    }

    /// The grass places of the cells, as lists of indexes into the catalog.
    type CellLists = std::collections::HashMap<(i32, i32), Vec<u32>>;

    /// Random grass places in the cells around the cell (0, 0), with the places of each cell in
    /// the order of the catalog.
    fn random_catalog(state: &mut u64, places: usize) -> (CellLists, Vec<GrassPlace>) {
        let mut lists = CellLists::new();
        let mut grass = Vec::new();
        for _ in 0..places {
            let position = [
                lcg(state) * 3.0 * CELL - CELL,
                lcg(state) * 3.0 * CELL - CELL,
                lcg(state) * 2000.0 - 500.0,
            ];
            let cell = ((position[0] / CELL).floor() as i32, (position[1] / CELL).floor() as i32);
            lists.entry(cell).or_default().push(grass.len() as u32);
            grass.push(GrassPlace {
                first_subset: (lcg(state) * 7.0) as u32,
                subset_count: 1 + (lcg(state) * 2.0) as u32,
                position,
                rotation: [lcg(state), lcg(state), lcg(state) * 6.0],
                scale: 0.5 + lcg(state),
                radius: lcg(state) * 300.0,
                group: (lcg(state) * 5.0) as u16,
            });
        }
        (lists, grass)
    }

    /// The world with the cells of `lists`, made as the catalog makes it: the places of each cell
    /// in tile order, with the tiles.
    fn world_of(lists: &CellLists, grass: &[GrassPlace]) -> World {
        let mut world = World::default();
        for (cell, list) in lists {
            world.grass_cells.entry(*cell).or_default().places = list.clone();
        }
        world.make_grass_tiles(grass);
        world
    }

    /// A world of random grass places, made as the catalog makes it.
    fn random_world(state: &mut u64, places: usize) -> (World, Vec<GrassPlace>) {
        let (lists, grass) = random_catalog(state, places);
        (world_of(&lists, &grass), grass)
    }

    /// The lists of the places of the cells of a world, in the order of the cells.
    fn lists_of(world: &World) -> CellLists {
        world
            .grass_cells
            .iter()
            .map(|(cell, grass_cell)| (*cell, grass_cell.places.clone()))
            .collect()
    }

    /// A view from a random eye with random planes.
    fn random_sight(state: &mut u64) -> GrassSight {
        let planes = core::array::from_fn(|_| {
            let n = [lcg(state) - 0.5, lcg(state) - 0.5, lcg(state) - 0.5];
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-3);
            [n[0] / length, n[1] / length, n[2] / length, lcg(state) * 6000.0 - 1500.0]
        });
        GrassSight {
            eye: [
                lcg(state) * 3.0 * CELL - CELL,
                lcg(state) * 3.0 * CELL - CELL,
                lcg(state) * 3000.0,
            ],
            range: 500.0 + lcg(state) * 7000.0,
            planes,
            now: 10.0,
            fade_seconds: 2.0,
            limit: GRASS_RECORDS,
        }
    }

    /// The subsets on the GPU in the tests: the ones whose number is not a multiple of 4.
    fn on_gpu(subset: u32) -> Option<GrassSubset> {
        (subset % 4 != 0).then(|| GrassSubset {
            texture: subset % 3,
            arrived: 8.0 + subset as f32 * 0.3,
            bent: (subset % 2) as f32,
        })
    }

    /// The group 3 is hidden in the tests.
    fn hidden_group(group: u16) -> bool {
        group == 3
    }

    /// The choice of the grass of 58fe62e: each place of the cells within reach, in the order of
    /// the lists, tested one by one, with its matrix made when it is taken.
    fn reference_choice(lists: &CellLists, grass: &[GrassPlace], sight: &GrassSight) -> (Vec<GrassDraw>, bool) {
        let mut draws: Vec<GrassDraw> = Vec::new();
        let eye = sight.eye;
        let (cell_x, cell_y) = ((eye[0] / CELL).floor() as i32, (eye[1] / CELL).floor() as i32);
        let cells = (sight.range / CELL).ceil() as i32;
        for y in cell_y - cells..=cell_y + cells {
            for x in cell_x - cells..=cell_x + cells {
                let Some(list) = lists.get(&(x, y)) else { continue };
                for index in list {
                    let place = &grass[*index as usize];
                    if hidden_group(place.group) {
                        continue;
                    }
                    let d = [
                        place.position[0] - eye[0],
                        place.position[1] - eye[1],
                        place.position[2] - eye[2],
                    ];
                    let reach = sight.range + place.radius;
                    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > reach * reach {
                        continue;
                    }
                    let p = place.position;
                    if !sight
                        .planes
                        .iter()
                        .all(|plane| plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3] >= -place.radius)
                    {
                        continue;
                    }
                    for id in place.first_subset..place.first_subset + place.subset_count {
                        let Some(on_gpu) = on_gpu(id) else { continue };
                        if draws.len() == sight.limit {
                            return (draws, true);
                        }
                        let record = grass_record(&world_matrix(place.position, place.rotation, place.scale));
                        let fade = Resident::fade_after(sight.now - on_gpu.arrived, sight.fade_seconds);
                        draws.push(GrassDraw {
                            texture: on_gpu.texture,
                            subset: id,
                            record,
                            fade,
                            bent: on_gpu.bent,
                        });
                    }
                }
            }
        }
        (draws, false)
    }

    /// World::choose_grass takes the places that the reference takes, with the same records, over
    /// a random world and many views, with the cap of the call and with a small one. The reference
    /// walks the lists of the cells in tile order, so the cap is compared against tile order; the
    /// test of the tile order below compares with the order of the catalog, below the cap.
    #[test]
    fn the_grass_choice_equals_the_reference() {
        let mut state = 11;
        let (world, grass) = random_world(&mut state, 4000);
        let tile_lists = lists_of(&world);
        let mut kept = GrassKept::default();
        let (mut taken, mut cuts) = (0, 0);
        for round in 0..120 {
            let sight = GrassSight {
                limit: if round % 3 == 0 { 40 } else { GRASS_RECORDS },
                ..random_sight(&mut state)
            };
            let (expected, expected_cut) = reference_choice(&tile_lists, &grass, &sight);
            let mut draws = Vec::new();
            let mut counts = GrassParts::default();
            let cut = world.choose_grass(&grass, &sight, hidden_group, on_gpu, &mut draws, &mut counts, kept.begin(b""));
            kept.end();
            assert_eq!(draws, expected, "round {round}");
            assert_eq!(cut, expected_cut, "round {round}");
            taken += draws.len();
            cuts += cut as usize;
        }
        assert!(
            taken > 1000 && cuts > 5 && kept.size().0 > 0,
            "{taken} draws, {cuts} cuts, {} cells kept",
            kept.size().0
        );
    }

    /// The clock is read once for a call, and the fade is still one value for each subset: every
    /// draw of a subset has the fade of its arrival at that time, and subsets that arrived at
    /// different times have different fades.
    #[test]
    fn each_subset_has_its_own_fade_in_a_call() {
        let mut state = 5;
        let (world, grass) = random_world(&mut state, 2000);
        let sight = GrassSight {
            range: 9000.0,
            eye: [4000.0, 4000.0, 500.0],
            planes: [[0.0, 0.0, 0.0, 1.0]; 6],
            ..random_sight(&mut state)
        };
        let mut draws = Vec::new();
        world.choose_grass(
            &grass,
            &sight,
            hidden_group,
            on_gpu,
            &mut draws,
            &mut GrassParts::default(),
            GrassKept::default().begin(b""),
        );
        let mut fades = std::collections::HashMap::new();
        for draw in &draws {
            let arrived = on_gpu(draw.subset).expect("a draw has a subset on the GPU").arrived;
            assert_eq!(
                draw.fade,
                Resident::fade_after(sight.now - arrived, sight.fade_seconds),
                "subset {}",
                draw.subset
            );
            assert_eq!(
                *fades.entry(draw.subset).or_insert(draw.fade),
                draw.fade,
                "subset {}",
                draw.subset
            );
        }
        let distinct: std::collections::HashSet<u32> = fades.values().map(|fade| fade.to_bits()).collect();
        assert!(
            distinct.len() > 1 && draws.len() > 100,
            "{} fades in {} draws",
            distinct.len(),
            draws.len()
        );
    }

    /// The draws of a call as a sorted list of their bits: the order of the records is left out.
    fn sorted_bits(draws: &[GrassDraw]) -> Vec<Vec<u32>> {
        let mut bits: Vec<Vec<u32>> = draws
            .iter()
            .map(|draw| {
                let mut one = vec![draw.texture, draw.subset, draw.fade.to_bits(), draw.bent.to_bits()];
                one.extend(draw.record.iter().map(|value| value.to_bits()));
                one
            })
            .collect();
        bits.sort();
        bits
    }

    /// Restores the test of c5ba423 that 5efb335 dropped. A world made as the catalog makes it, with
    /// the cells in tile order and the tiles skipped, gives the draws of the walk over the cells in
    /// the order of the catalog, as a sorted list. No call is cut by the cap: at the cap the new
    /// order keeps other places, by design. Some calls come in another order, so that the test
    /// can fail.
    #[test]
    fn the_tile_order_keeps_the_draws_of_a_call_below_the_cap() {
        let mut state = 11;
        let (catalog_lists, grass) = random_catalog(&mut state, 4000);
        let world = world_of(&catalog_lists, &grass);
        let (mut taken, mut reordered) = (0, 0);
        for round in 0..120 {
            let sight = random_sight(&mut state);
            let (expected, expected_cut) = reference_choice(&catalog_lists, &grass, &sight);
            let mut draws = Vec::new();
            let cut = world.choose_grass(
                &grass,
                &sight,
                hidden_group,
                on_gpu,
                &mut draws,
                &mut GrassParts::default(),
                GrassKept::default().begin(b""),
            );
            assert!(!expected_cut && !cut, "round {round} is cut by the cap");
            assert_eq!(sorted_bits(&draws), sorted_bits(&expected), "round {round}");
            taken += draws.len();
            reordered += (draws != expected) as usize;
        }
        assert!(
            taken > 1000 && reordered > 20,
            "{taken} draws, {reordered} calls in another order"
        );
    }

    /// The cells of a world made as the catalog makes it hold the same places as the lists of the
    /// catalog, once each, and the tiles cover each list once: every place of a tile is in the tile
    /// (its box and its radius) and in one tile of the grid.
    #[test]
    fn make_tiles_keeps_every_place_of_the_catalog() {
        let mut state = 31;
        let (catalog_lists, grass) = random_catalog(&mut state, 4000);
        let world = world_of(&catalog_lists, &grass);
        assert_eq!(world.grass_cells.len(), catalog_lists.len());
        let mut biggest = 0;
        for (key, list) in &catalog_lists {
            let cell = &world.grass_cells[key];
            let (mut kept, mut expected) = (cell.places.clone(), list.clone());
            kept.sort_unstable();
            expected.sort_unstable();
            assert_eq!(kept, expected, "cell {key:?}");
            let mut next = 0;
            for tile in &cell.tiles {
                assert_eq!((tile.first as usize, tile.count > 0), (next, true), "cell {key:?}");
                let first = grass[cell.places[next] as usize].tile();
                for index in &cell.places[tile.range()] {
                    let place = &grass[*index as usize];
                    assert_eq!(place.tile(), first);
                    assert!(place.radius <= tile.radius);
                    assert!(
                        (0..3).all(|axis| tile.min[axis] <= place.position[axis] && place.position[axis] <= tile.max[axis])
                    );
                }
                next += tile.count as usize;
            }
            assert_eq!(next, cell.places.len(), "cell {key:?}");
            biggest = biggest.max(list.len());
        }
        assert!(biggest > 100, "the biggest cell has {biggest} places");
    }

    /// The tile skip changes nothing: the draws are those of the walk of every place of the cell,
    /// in the same order. Some rounds use a small cap, so that the cut falls in the middle of a walk.
    #[test]
    fn the_tile_skip_keeps_the_draws_and_their_order() {
        let mut state = 23;
        let (world, grass) = random_world(&mut state, 4000);
        let tile_lists = lists_of(&world);
        let mut kept = GrassKept::default();
        let (mut skipped, mut cuts, mut taken) = (0, 0, 0);
        for round in 0..200 {
            let sight = GrassSight {
                limit: if round % 2 == 0 { 25 } else { GRASS_RECORDS },
                ..random_sight(&mut state)
            };
            let (expected, expected_cut) = reference_choice(&tile_lists, &grass, &sight);
            let mut draws = Vec::new();
            let mut counts = GrassParts::default();
            let cut = world.choose_grass(&grass, &sight, hidden_group, on_gpu, &mut draws, &mut counts, kept.begin(b""));
            kept.end();
            assert_eq!(draws, expected, "round {round}");
            assert_eq!(cut, expected_cut, "round {round}");
            skipped += counts.skipped;
            cuts += cut as usize;
            taken += draws.len();
        }
        assert!(
            skipped > 5000 && cuts > 5 && taken > 1000,
            "{skipped} skipped, {cuts} cuts, {taken} draws"
        );
    }

    /// A tile is skipped only when no place of it is taken, in cells of random places, eyes, ranges
    /// and planes. The test counts that some tiles are skipped and some are not, so that it can fail.
    #[test]
    fn a_skipped_tile_has_no_place_that_is_taken() {
        let mut state = 7;
        let (mut skipped, mut kept) = (0, 0);
        for _ in 0..300 {
            let (world, grass) = random_world(&mut state, 200);
            let sight = random_sight(&mut state);
            for cell in world.grass_cells.values() {
                assert_eq!(
                    cell.tiles.iter().map(|tile| tile.count as usize).sum::<usize>(),
                    cell.places.len()
                );
                for tile in &cell.tiles {
                    let taken = cell.places[tile.range()]
                        .iter()
                        .any(|index| grass[*index as usize].miss(sight.eye, sight.range, &sight.planes).is_none());
                    if tile.misses(sight.eye, sight.range, &sight.planes) {
                        assert!(!taken);
                        skipped += 1;
                    } else {
                        kept += 1;
                    }
                }
            }
        }
        assert!(skipped > 100 && kept > 100, "{skipped} skipped, {kept} kept");
    }

    /// A plane that cuts the box of a tile diagonally keeps the tile when the corner of the box on
    /// the side of the normal is inside, even if the corner nearest to the plane is outside.
    #[test]
    fn a_plane_that_cuts_a_tile_diagonally_keeps_it() {
        let at = |x: f32, y: f32, radius: f32| GrassPlace {
            first_subset: 0,
            subset_count: 1,
            position: [x, y, 0.0],
            rotation: [0.0; 3],
            scale: 1.0,
            radius,
            group: 0,
        };
        let grass = [at(10.0, 10.0, 0.0), at(1000.0, 1000.0, 0.0)];
        let tile = GrassTile::of(0, &[&grass[0], &grass[1]]);
        let side = std::f32::consts::FRAC_1_SQRT_2;
        let eye = [500.0, 500.0, 0.0];
        // The far corner (1000, 1000) is 414 units inside; the near corner (10, 10) is 986 outside.
        let cutting = [[side, side, 0.0, -1000.0]; 6];
        assert_eq!(grass[0].miss(eye, 5000.0, &cutting), Some(GrassMiss::OutOfView));
        assert_eq!(grass[1].miss(eye, 5000.0, &cutting), None);
        assert!(!tile.misses(eye, 5000.0, &cutting));
        // The plane moved out by 600 units: every place of the tile is outside it.
        let outside = [[side, side, 0.0, -1600.0]; 6];
        assert_eq!(grass[1].miss(eye, 5000.0, &outside), Some(GrassMiss::OutOfView));
        assert!(tile.misses(eye, 5000.0, &outside));
        // The same plane with a radius of 200 on a place: it is taken, so the tile stays.
        let round = [at(10.0, 10.0, 0.0), at(1000.0, 1000.0, 200.0)];
        let tile = GrassTile::of(0, &[&round[0], &round[1]]);
        assert_eq!(round[1].miss(eye, 5000.0, &outside), None);
        assert!(!tile.misses(eye, 5000.0, &outside));
    }

    /// The tiles of a cell follow the tile grid, row by row, and keep the order of the catalog
    /// inside a tile.
    #[test]
    fn a_cell_orders_its_places_by_tile() {
        let at = |x: f32, y: f32, z: f32, radius: f32| GrassPlace {
            first_subset: 0,
            subset_count: 1,
            position: [x, y, z],
            rotation: [0.0; 3],
            scale: 1.0,
            radius,
            group: 0,
        };
        let grass = vec![
            at(100.0, 1100.0, 0.0, 2.0),
            at(1100.0, 100.0, 0.0, 5.0),
            at(100.0, 100.0, 0.0, 9.0),
            at(1200.0, 200.0, 4.0, 1.0),
        ];
        let mut world = World::default();
        world.grass_cells.entry((0, 0)).or_default().places = vec![0, 1, 2, 3];
        world.make_grass_tiles(&grass);
        let cell = &world.grass_cells[&(0, 0)];
        assert_eq!(cell.places, vec![2, 1, 3, 0]);
        let spans: Vec<(u32, u32)> = cell.tiles.iter().map(|tile| (tile.first, tile.count)).collect();
        assert_eq!(spans, vec![(0, 1), (1, 2), (3, 1)]);
        assert_eq!(cell.tiles[1].radius, 5.0);
        assert_eq!(cell.tiles[1].max, [1200.0, 200.0, 4.0]);
    }

    /// One call of the kept-records tests: the draws, and the matrices that the call made.
    fn kept_call(world: &World, grass: &[GrassPlace], sight: &GrassSight, kept: &mut GrassKept) -> (usize, u64) {
        let (mut draws, mut counts) = (Vec::new(), GrassParts::default());
        world.choose_grass(grass, sight, hidden_group, on_gpu, &mut draws, &mut counts, kept.begin(b""));
        kept.end();
        (draws.len(), counts.records_made)
    }

    /// A sight in which every place passes the planes.
    fn open_sight(state: &mut u64, eye: [f32; 3]) -> GrassSight {
        GrassSight {
            eye,
            range: 1500.0,
            planes: [[0.0, 0.0, 0.0, 1.0]; 6],
            ..random_sight(state)
        }
    }

    /// A cell keeps its records over the next GRASS_KEEP_CALLS calls that do not walk it, loses
    /// them at the call after, and a second call over the same view makes no matrix.
    #[test]
    fn a_cell_keeps_its_records_for_a_while() {
        let mut state = 3;
        let (world, grass) = random_world(&mut state, 8000);
        let near = open_sight(&mut state, [4000.0, 4000.0, 500.0]);
        let far = GrassSight {
            eye: [4000.0 + 40.0 * CELL, 4000.0, 500.0],
            ..open_sight(&mut state, [0.0; 3])
        };
        let mut kept = GrassKept::default();
        let (drawn, made) = kept_call(&world, &grass, &near, &mut kept);
        assert!(drawn > 5 && made > 0, "{drawn} draws, {made} matrices");
        assert_eq!(kept_call(&world, &grass, &near, &mut kept), (drawn, 0));
        let size = kept.size();
        assert!(size.0 > 0 && size.1 > 0);
        for _ in 0..GRASS_KEEP_CALLS {
            assert_eq!(kept_call(&world, &grass, &far, &mut kept).0, 0);
            assert_eq!(kept.size(), size);
        }
        kept_call(&world, &grass, &far, &mut kept);
        assert_eq!(kept.size(), (0, 0));
        assert_eq!(kept_call(&world, &grass, &near, &mut kept), (drawn, made));
    }

    /// A call in a world space with no grass in reach (an interior) keeps nothing, and allocates
    /// no entry for its world space. A call with grass makes the entry with its first cell.
    #[test]
    fn a_call_with_no_grass_allocates_no_world_space() {
        let mut state = 5;
        let (interior, no_grass) = random_world(&mut state, 0);
        let sight = open_sight(&mut state, [4000.0, 4000.0, 500.0]);
        let mut kept = GrassKept::default();
        for _ in 0..3 {
            let (mut draws, mut counts) = (Vec::new(), GrassParts::default());
            interior.choose_grass(
                &no_grass,
                &sight,
                hidden_group,
                on_gpu,
                &mut draws,
                &mut counts,
                kept.begin(b"Balmora, Eve's House"),
            );
            kept.end();
            assert!(draws.is_empty());
        }
        assert_eq!(kept.worlds.capacity(), 0, "the map never held a world space");

        let (world, grass) = random_world(&mut state, 8000);
        let (drawn, _) = kept_call(&world, &grass, &sight, &mut kept);
        assert!(drawn > 0);
        assert_eq!(kept.worlds.len(), 1);
    }

    /// An eye that moves to and fro across the edge of a cell makes no matrix after its first two
    /// calls: each cell is walked at least once in two calls.
    #[test]
    fn a_walk_along_a_cell_edge_makes_no_matrix_again() {
        let mut state = 9;
        let (world, grass) = random_world(&mut state, 8000);
        let sights = [
            open_sight(&mut state, [CELL - 100.0, 4000.0, 500.0]),
            open_sight(&mut state, [CELL + 100.0, 4000.0, 500.0]),
        ];
        let mut kept = GrassKept::default();
        let mut made_late = 0;
        for call in 0..40 {
            let (drawn, made) = kept_call(&world, &grass, &sights[call % 2], &mut kept);
            assert!(drawn > 5, "call {call}: {drawn} draws");
            made_late += if call >= 2 { made } else { 0 };
        }
        assert_eq!(made_late, 0);
    }

    /// The fade of a subset runs from 0 at its arrival to 1 after the fade time, and a fade time
    /// of 0 means no fade (a replay).
    #[test]
    fn a_subset_fades_in_over_the_fade_time() {
        assert_eq!(Resident::fade_after(5.0, 0.0), 1.0);
        assert_eq!(Resident::fade_after(0.0, 2.0), 0.0);
        assert_eq!(Resident::fade_after(1.0, 2.0), 0.5);
        assert_eq!(Resident::fade_after(9.0, 2.0), 1.0);
        assert_eq!(Resident::fade_after(-1.0, 2.0), 0.0);
    }

    /// The draws of a grass call sort by texture, then by subset, then by the order of their
    /// choice, so that the order has no equal keys and the same call gives the same records.
    #[test]
    fn the_grass_draws_sort_by_texture_subset_and_choice() {
        let mut order = GrassOrder::default();
        order.sort([(2, 7), (1, 9), (2, 7), (1, 4), (1, 9), (2, 3)].into_iter());
        assert_eq!(
            order.keys,
            vec![(1, 4, 3), (1, 9, 1), (1, 9, 4), (2, 3, 5), (2, 7, 0), (2, 7, 2)]
        );
        // The keys are made again for the next call, not added to.
        order.sort([(5, 5)].into_iter());
        assert_eq!(order.keys, vec![(5, 5, 0)]);
    }

    use super::{
        BENT_SHARE, CatalogError, VERTEX, bytes_limit, cap_from_budget, cap_move, event_stamp, half, hidden, memory_event,
        normal_alignment, pick_trim, plan_merged, qpc, read_tables,
    };

    const MB: u64 = 1 << 20;

    /// The statics' load counts for the watchdog's allowance while its catalog is on the way,
    /// only with the statics on, and no longer once a frame took it.
    #[test]
    fn the_statics_load_counts_while_the_catalog_is_on_its_way() {
        let folder = std::env::temp_dir().join(format!("statics-loading-{}", std::process::id()));
        let document = mge_config::ConfigDocument::open(folder.join("absent.toml"));
        let mut statics = super::Statics::start(
            &document,
            folder,
            crate::renderer::core_mods::CoreMods::built_in().shadow_hooks(),
        );
        statics.settings.statics = true;
        assert!(statics.loading());
        statics.settings.statics = false;
        assert!(!statics.loading());
        statics.settings.statics = true;
        statics.loading = None;
        assert!(!statics.loading());
    }

    /// A loader thread that ends without an answer turns the statics off, and their load no
    /// longer counts; one that has not answered yet still counts.
    #[test]
    fn a_loader_thread_that_ends_without_an_answer_is_a_failed_load() {
        let folder = std::env::temp_dir().join(format!("statics-disconnected-{}", std::process::id()));
        let document = mge_config::ConfigDocument::open(folder.join("absent.toml"));
        let mut statics = super::Statics::start(
            &document,
            folder,
            crate::renderer::core_mods::CoreMods::built_in().shadow_hooks(),
        );
        statics.settings.statics = true;
        let (sender, receiver) = std::sync::mpsc::channel();
        statics.loading = Some(receiver);
        assert!(statics.take_catalog().is_none());
        assert!(statics.loading() && !statics.failed);
        drop(sender);
        assert!(statics.take_catalog().is_none());
        assert!(!statics.loading() && statics.failed);
    }

    /// A failed catalog (a missing statics file, with the terrain there) ends the wait of a load:
    /// before, the wait's clock started again at each frame, and the wait had no end. While the
    /// catalog is on its way, the wait goes on.
    #[test]
    fn a_failed_catalog_ends_the_wait_of_a_load() {
        use nirender_wgpu::Extension;
        let root = std::env::temp_dir().join(format!("statics-wait-{}", std::process::id()));
        let document = mge_config::ConfigDocument::open(root.join("absent.toml"));
        let mut land = super::super::DistantLand::start(&root, false);
        land.loading = None;
        land.generation = None;
        land.statics = Some(super::Statics::start(
            &document,
            root.join("distantland"),
            crate::renderer::core_mods::CoreMods::built_in().shadow_hooks(),
        ));
        land.arrival = Some((std::time::Instant::now(), 2));
        assert!(land.messages().is_empty());
        assert!(land.arrival.is_some(), "the catalog is on its way");
        let statics = land.statics.as_mut().unwrap();
        statics.loading = None;
        statics.failed = true;
        let messages = land.messages();
        assert!(land.arrival.is_none(), "the wait ended");
        let (kind, words) = &messages[0];
        assert_eq!(*kind, super::super::MESSAGE_DISTANT_LAND);
        assert_eq!(u32::from_le_bytes(words[4..8].try_into().unwrap()), super::super::ARRIVED);
    }

    /// A reader thread that ended: what it was asked for never comes, so a wait on a load
    /// must not wait for it, and the arrivals are not looked at again.
    #[test]
    fn a_reader_thread_that_ends_turns_the_statics_off_and_ends_the_wait() {
        let folder = std::env::temp_dir().join(format!("statics-reader-{}", std::process::id()));
        let document = mge_config::ConfigDocument::open(folder.join("absent.toml"));
        let mut statics = super::Statics::start(
            &document,
            folder,
            crate::renderer::core_mods::CoreMods::built_in().shadow_hooks(),
        );
        let (_sender, receiver) = std::sync::mpsc::channel();
        statics.arrivals = Some(receiver);
        statics.requested.insert(7);
        assert!(!statics.settled());
        statics.reader_ended();
        assert!(statics.settled() && statics.failed && statics.requested.is_empty() && statics.arrivals.is_none());
    }

    #[test]
    fn a_memory_event_line_names_its_frame_its_time_its_count_and_its_bytes() {
        let line = memory_event(
            &event_stamp(1234, 56_789),
            "taken off for nearer ones",
            3,
            3_200_000_000,
            3_150_000_000,
            &bytes_limit("the cap", 3_160_000_000),
        );
        assert_eq!(
            line,
            "distant statics: frame 1234, qpc 56789: 3 merged subsets taken off for nearer ones, 3200 MB to 3150 MB on the GPU (the cap 3160 MB)"
        );
        let none = memory_event(
            &event_stamp(7, 1),
            "trimmed (wanted least recently)",
            1,
            2_000_000,
            1_000_000,
            &bytes_limit("the high mark", u64::MAX),
        );
        assert!(none.ends_with("(the high mark none)"), "{none}");
        let far = memory_event(
            &event_stamp(9, 1),
            "taken off, too far",
            2,
            5_000_000,
            3_000_000,
            "farther than 26624 units and the subset's radius",
        );
        assert!(
            far.ends_with("5 MB to 3 MB on the GPU (farther than 26624 units and the subset's radius)"),
            "{far}"
        );
    }

    /// The time of an event is the performance counter: it runs, and it does not go back.
    #[test]
    fn the_time_of_a_memory_event_is_the_running_performance_counter() {
        let first = qpc();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = qpc();
        assert!(first > 0 && second > first, "{first} then {second}");
    }

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

    /// A statics file that the formats crate refuses is rejected with its reason, never a
    /// panic (risk 9: a palette range past the table panicked the helper's own reader); a file
    /// that is not there is missing.
    #[test]
    fn a_bad_statics_file_is_rejected_with_its_reason() {
        use distantland::mge_xe::distant_statics::{
            HEADER_SIZE, PackedDistantStatic, PackedSubset, PackedVertex, SUBSET_RECORD_SIZE, StaticMeshesFileHeader,
            StaticType, SubsetRecord, UvBoundRecord,
        };
        let subset = PackedSubset {
            texture: "a.dds".into(),
            vertices: vec![PackedVertex::default(); 3],
            triangles: vec![[0, 1, 2]],
            palette: vec![UvBoundRecord {
                bound: [0.0, 1.0, 0.0, 1.0],
            }],
            ..Default::default()
        };
        let mut statics = distantland::mge_xe::PackedDistantStatics::default();
        statics.insert(
            "a.nif".into(),
            PackedDistantStatic {
                static_type: StaticType::StaticNear,
                subsets: vec![subset],
                ..Default::default()
            },
        );
        let mut bytes = distantland::mge_xe::serialize_static_meshes(&statics).unwrap();

        let folder = std::env::temp_dir().join(format!("mgehost64-statics-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("static_meshes_000");
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_tables(&path).is_ok());

        let header: StaticMeshesFileHeader = bytemuck::pod_read_unaligned(&bytes[..HEADER_SIZE]);
        let at = header.subset_table_offset as usize;
        let mut record: SubsetRecord = bytemuck::pod_read_unaligned(&bytes[at..at + SUBSET_RECORD_SIZE]);
        record.palette_count = 1000;
        bytes[at..at + SUBSET_RECORD_SIZE].copy_from_slice(bytemuck::bytes_of(&record));
        std::fs::write(&path, &bytes).unwrap();
        match read_tables(&path) {
            Err(CatalogError::Rejected(problem)) => assert!(
                problem.contains("palette range") && problem.contains("generate distant land again"),
                "{problem}"
            ),
            _ => panic!("a palette range past the table was not rejected"),
        }

        assert!(matches!(
            read_tables(&folder.join("static_meshes_001")),
            Err(CatalogError::Missing(_))
        ));
        std::fs::remove_dir_all(&folder).unwrap();
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
        let candidates = vec![
            (50.0, 9000.0, 1, 30),
            (50.0, 20000.0, 2, 30),
            (10.0, 5000.0, 3, 30),
            (90.0, 30000.0, 4, 30),
        ];
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

    use super::{Catalog, Subset, read_whole, reader};

    /// A read past the end of the shard is a failure, not a zero-filled buffer.
    #[test]
    fn a_short_read_is_a_failure() {
        let path = std::env::temp_dir().join("mge-xe-reader-test-short.bin");
        std::fs::write(&path, [7u8; 10]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut whole = [0u8; 4];
        assert_eq!(read_whole(&file, &mut whole, 6), Some(()));
        assert_eq!(whole, [7; 4]);
        let mut short = [0u8; 16];
        assert_eq!(read_whole(&file, &mut short, 0), None);
        assert_eq!(read_whole(&file, &mut whole, 10), None);
        drop(file);
        let _ = std::fs::remove_file(path);
    }

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
        let arrival = arrivals
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("an answer for the unreadable subset");
        assert_eq!(arrival.subset, 0);
        assert!(arrival.unreadable);
        assert!(arrival.vertices.is_empty() && arrival.indexes.is_empty());
        thread.join().unwrap();
    }
}
