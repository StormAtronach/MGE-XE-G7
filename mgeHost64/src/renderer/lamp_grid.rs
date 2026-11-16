//! The distant lamps for the light of each pixel of the distant land and statics: the lamps of
//! the lights file in a list, each with the three terms that the engine gives a near lamp of
//! its radius ([`LightSettings::terms`]) and the distance where its light ends, and a grid
//! across the ground where each small cell names the lamps that can reach it.
//!
//! The grid has two levels, because most of a large world has no lamp: one entry for each
//! exterior cell of the game (8192 units) between the first and the last lit cell, and a
//! second level of small cells only for a cell that a lamp reaches. A small cell has the list
//! of all its lamps up to the bound (the light limit of record 12, the most lights of a near
//! node). The size of a small cell is the largest of 1024, 512 and 256 units at which no list
//! is over the bound. At 256 units a list over the bound keeps its strongest lamps, and the
//! report counts the cells and the lamps that were left out.
//!
//! The list and the grid are made on the CPU one time for each lights file, record 12 and
//! setting of the light fade, and go to the GPU as two storage buffers of the view group
//! ([`LampBinding`]). The design: the Architect's page `distant-lights-from-fork-2026-11-12.md`,
//! sections 2.1 and 2.2.

use std::collections::{BTreeMap, HashMap};

use nirender_wgpu::objects::Create;
use nirender_wgpu::wgpu;

use super::lamp_values::LightSettings;

/// The side of an exterior cell of the game.
const CELL: f32 = 8192.0;
/// The sizes of a small cell that the grid tries, the largest first. Each divides [`CELL`].
const SIZES: [f32; 3] = [1024.0, 512.0, 256.0];
/// An exterior cell without lamps in the first level.
const NO_CELL: u32 = u32::MAX;
/// Without the light fade, a lamp ends where its falloff is this part of its light, at most
/// [`MOST_RADII`] times its radius: the end of the Worldspaces fork.
const LEAST_FALLOFF: f32 = 0.04;
const MOST_RADII: f32 = 4.0;
/// The end of a light, at most: as the near lights of `PerPixel::shade_lights`.
const MOST_END: f32 = 8192.0;
/// The radius of a light that the end takes at least: as `PerPixel::shade_lights`.
const LEAST_RADIUS: f32 = 16.0;

/// A lamp of the lights file, with the values that its light takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceLamp {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [u8; 3],
    /// The flags of the light record.
    pub flags: u32,
}

/// A lamp as the shaders read it: `Lamp` of [`LAMPS_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLamp {
    pub position: [f32; 3],
    /// The distance where the light ends.
    pub end: f32,
    /// The colour of the light, from 0 to 1.
    pub color: [f32; 3],
    /// The flags of the light record.
    pub flags: u32,
    /// The constant, linear and quadratic terms of the falloff.
    pub terms: [f32; 3],
    /// The part of the light that does not depend on the facing (only with the original
    /// lights).
    pub wrap: f32,
    /// The ambient colour of the light.
    pub ambient: [f32; 3],
    /// The distance that the light moves toward the camera (only with the original lights).
    pub bias: f32,
}

/// The values of the grid: `LampValues` of [`LAMPS_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LampValues {
    /// The first exterior cell of the first level, and the number of cells in x and in y.
    pub grid: [i32; 4],
    /// The small cells on a side of an exterior cell, the first word of the second level and of
    /// the lists in the grid's words, and the number of lamps.
    pub counts: [u32; 4],
    /// x: the size of a small cell. y: the strength of the lamps' light in the frame (0: no
    /// light; written each frame). z, w: not used.
    pub values: [f32; 4],
}

/// What a made grid has, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridReport {
    pub lamps: usize,
    /// The size of a small cell, 0 without lamps.
    pub size: f32,
    /// The exterior cells that have a second level.
    pub lit_cells: usize,
    /// The longest list of a small cell.
    pub fullest: usize,
    /// The bound of a list.
    pub bound: usize,
    /// The small cells whose lamps were over the bound at 256 units, and the lamps that their
    /// lists left out.
    pub cells_over: usize,
    pub left_out: usize,
    /// The bytes of the two storage buffers.
    pub lamp_bytes: usize,
    pub grid_bytes: usize,
}

impl GridReport {
    /// The line of the log.
    pub fn describe(&self) -> String {
        if self.lamps == 0 {
            return "distant lamps: none".to_string();
        }
        let mut line = format!(
            "distant lamps: {} lamps, cells of {} units in {} exterior cells, the fullest cell {} lamps (the bound {}); \
             the lamps {} bytes, the grid {} bytes",
            self.lamps, self.size, self.lit_cells, self.fullest, self.bound, self.lamp_bytes, self.grid_bytes
        );
        if self.cells_over != 0 {
            line += &format!(
                "; warning: {} cells over the bound left out {} of their lamps",
                self.cells_over, self.left_out
            );
        }
        line
    }
}

/// The list of the lamps and the grid's words, ready for the GPU.
#[derive(Clone, Debug, PartialEq)]
pub struct LampGrid {
    pub lamps: Vec<GpuLamp>,
    /// The first level (an index of a lit cell, or [`NO_CELL`]), then for each lit cell the
    /// first entry and the count of each of its small cells, then the entries (lamp indices).
    pub words: Vec<u32>,
    pub values: LampValues,
    pub report: GridReport,
}

/// The distance where the light of a lamp ends: with the light fade (`fade` the number of
/// radii, more than 0), as a near light ends; without it, where its falloff is
/// [`LEAST_FALLOFF`], at most [`MOST_RADII`] radii.
pub fn end_of(radius: f32, terms: [f32; 3], fade: f32) -> f32 {
    if fade > 0.0 {
        return (fade * radius.max(LEAST_RADIUS)).min(MOST_END);
    }
    let most = MOST_RADII * radius.max(0.0);
    let [constant, linear, quadratic] = terms.map(f64::from);
    // constant + linear d + quadratic d^2 = 1 / LEAST_FALLOFF.
    let wanted = 1.0 / f64::from(LEAST_FALLOFF) - constant;
    if wanted <= 0.0 {
        return 0.0;
    }
    let distance = if quadratic > 0.0 {
        (-linear + (linear * linear + 4.0 * quadratic * wanted).sqrt()) / (2.0 * quadratic)
    } else if linear > 0.0 {
        wanted / linear
    } else {
        f64::INFINITY
    };
    (distance as f32).min(most)
}

/// The lamps of a list, with their terms and ends. A lamp whose light ends at 0 has none.
pub fn gpu_lamps(source: &[SourceLamp], settings: &LightSettings, fade: f32) -> Vec<GpuLamp> {
    source
        .iter()
        .filter_map(|lamp| {
            let radius = lamp.radius.max(0.0);
            // The record's radius is a whole number (0x4D2A4D).
            let terms = settings.terms(radius as u32, false);
            let end = end_of(radius, terms, fade);
            (end > 0.0).then(|| GpuLamp {
                position: lamp.position,
                end,
                color: lamp.color.map(|c| f32::from(c) / 255.0),
                flags: lamp.flags,
                terms,
                ..GpuLamp::default()
            })
        })
        .collect()
}

/// The light of a lamp at a distance, for the choice of the lamps that a full list keeps.
fn strength_at(lamp: &GpuLamp, distance: f32) -> f32 {
    let [constant, linear, quadratic] = lamp.terms;
    let brightness = (lamp.color[0] + lamp.color[1] + lamp.color[2]) / 3.0;
    brightness / (constant + linear * distance + quadratic * distance * distance).max(0.0001)
}

/// The small cells of `size` that the circle of a lamp's end touches, with their lamps.
fn lists_of(lamps: &[GpuLamp], size: f32) -> HashMap<(i32, i32), Vec<u32>> {
    let mut lists: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (index, lamp) in lamps.iter().enumerate() {
        let [x, y, _] = lamp.position;
        let end = lamp.end;
        let first = (((x - end) / size).floor() as i32, ((y - end) / size).floor() as i32);
        let last = (((x + end) / size).floor() as i32, ((y + end) / size).floor() as i32);
        for cy in first.1..=last.1 {
            for cx in first.0..=last.0 {
                // The nearest point of the cell to the lamp.
                let near_x = x.clamp(cx as f32 * size, (cx + 1) as f32 * size);
                let near_y = y.clamp(cy as f32 * size, (cy + 1) as f32 * size);
                let (dx, dy) = (x - near_x, y - near_y);
                if dx * dx + dy * dy < end * end {
                    lists.entry((cx, cy)).or_default().push(index as u32);
                }
            }
        }
    }
    lists
}

/// The list and the grid of `lamps`, each small cell with at most `bound` lamps.
pub fn build(lamps: Vec<GpuLamp>, bound: usize) -> LampGrid {
    let mut report = GridReport {
        lamps: lamps.len(),
        bound,
        ..GridReport::default()
    };
    if lamps.is_empty() || bound == 0 {
        return finish(lamps, 0, BTreeMap::new(), report);
    }

    // The largest size whose lists are all within the bound; else the smallest.
    let (size, mut lists) = SIZES
        .iter()
        .map(|&size| (size, lists_of(&lamps, size)))
        .find(|(size, lists)| *size == SIZES[SIZES.len() - 1] || lists.values().all(|list| list.len() <= bound))
        .expect("the last size is always taken");
    report.size = size;
    report.fullest = lists.values().map(Vec::len).max().unwrap_or(0);

    // A list over the bound keeps the lamps with the most light at the cell's centre.
    for ((cx, cy), list) in lists.iter_mut() {
        if list.len() <= bound {
            continue;
        }
        let centre = [(*cx as f32 + 0.5) * size, (*cy as f32 + 0.5) * size];
        let light = |index: &u32| {
            let lamp = &lamps[*index as usize];
            let (dx, dy) = (lamp.position[0] - centre[0], lamp.position[1] - centre[1]);
            strength_at(lamp, (dx * dx + dy * dy).sqrt())
        };
        list.sort_by(|a, b| light(b).total_cmp(&light(a)).then(a.cmp(b)));
        report.cells_over += 1;
        report.left_out += list.len() - bound;
        list.truncate(bound);
    }

    // The small cells by their exterior cell, in a fixed order.
    let side = (CELL / size) as i32;
    let mut cells: BTreeMap<(i32, i32), Vec<((i32, i32), Vec<u32>)>> = BTreeMap::new();
    for ((cx, cy), mut list) in lists {
        list.sort_unstable();
        let exterior = (cx.div_euclid(side), cy.div_euclid(side));
        cells
            .entry(exterior)
            .or_default()
            .push(((cx.rem_euclid(side), cy.rem_euclid(side)), list));
    }
    finish(lamps, side as u32, cells, report)
}

/// The words of the grid from the lists of the lit cells, and the values and the report.
fn finish(
    lamps: Vec<GpuLamp>,
    side: u32,
    cells: BTreeMap<(i32, i32), Vec<((i32, i32), Vec<u32>)>>,
    mut report: GridReport,
) -> LampGrid {
    let first = cells
        .keys()
        .fold((i32::MAX, i32::MAX), |(x, y), cell| (x.min(cell.0), y.min(cell.1)));
    let last = cells
        .keys()
        .fold((i32::MIN, i32::MIN), |(x, y), cell| (x.max(cell.0), y.max(cell.1)));
    let (width, height) = if cells.is_empty() {
        (0, 0)
    } else {
        (last.0 - first.0 + 1, last.1 - first.1 + 1)
    };

    let small = (side * side) as usize;
    let level2 = (width * height) as usize;
    let lists_at = level2 + 2 * small * cells.len();
    let mut words = vec![NO_CELL; lists_at];
    for (lit, (exterior, small_cells)) in cells.iter().enumerate() {
        words[((exterior.1 - first.1) * width + exterior.0 - first.0) as usize] = lit as u32;
        let block = level2 + 2 * small * lit;
        // A small cell without lamps has no entries.
        words[block..block + 2 * small].fill(0);
        for ((x, y), list) in small_cells {
            let pair = block + 2 * (*y as usize * side as usize + *x as usize);
            words[pair] = (words.len() - lists_at) as u32;
            words[pair + 1] = list.len() as u32;
            words.extend_from_slice(list);
        }
    }
    report.lit_cells = cells.len();
    // A storage buffer has at least one element.
    if words.is_empty() {
        words.push(NO_CELL);
    }
    let lamp_count = lamps.len();
    let mut lamps = lamps;
    if lamps.is_empty() {
        lamps.push(GpuLamp::default());
    }
    report.lamp_bytes = lamps.len() * size_of::<GpuLamp>();
    report.grid_bytes = words.len() * 4;
    let size = if side == 0 { 0.0 } else { CELL / side as f32 };
    LampGrid {
        values: LampValues {
            grid: [first.0, first.1, width, height].map(|v| if cells.is_empty() { 0 } else { v }),
            counts: [side, level2 as u32, lists_at as u32, lamp_count as u32],
            values: [size, 0.0, 0.0, 0.0],
        },
        lamps,
        words,
        report,
    }
}

/// The lamps of the view group: the bindings `first` to `first + 2` of a shader that starts
/// with the view group (`view::wgsl`).
const LAMPS_WGSL: &str = "
struct Lamp {
    position: vec3f,
    end: f32,
    color: vec3f,
    flags: u32,
    terms: vec3f,
    wrap: f32,
    ambient: vec3f,
    bias: f32,
}
struct LampValues {
    // The first exterior cell of the grid, and its cells in x and y.
    grid: vec4i,
    // The small cells on a side of a cell, the first word of the second level and of the
    // lists, and the number of lamps.
    counts: vec4u,
    // x: the size of a small cell. y: the strength of the lamps' light in the frame.
    values: vec4f,
}
@group(0) @binding(B0) var<uniform> lamp_values: LampValues;
@group(0) @binding(B1) var<storage, read> lamps: array<Lamp>;
@group(0) @binding(B2) var<storage, read> lamp_grid: array<u32>;
";

/// [`LAMPS_WGSL`] with the bindings from `first`.
pub fn wgsl_at(first: u32) -> String {
    (0..3).fold(LAMPS_WGSL.to_string(), |code, i| {
        code.replace(&format!("B{i}"), &(first + i).to_string())
    })
}

/// The buffers of the lamps that the view group binds: the values, the lamps and the grid.
pub struct LampBinding {
    values: wgpu::Buffer,
    lamps: wgpu::Buffer,
    grid: wgpu::Buffer,
    /// One more each time the buffers are made again, so that a group with these bindings
    /// (`entries`) is made again.
    pub generation: u64,
}

impl LampBinding {
    /// The layout of the three bindings of the lamps, from the binding `first`. Only the pixel
    /// shaders read them.
    pub fn layout_entries(first: u32) -> [wgpu::BindGroupLayoutEntry; 3] {
        let buffer = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        [
            buffer(first, wgpu::BufferBindingType::Uniform),
            buffer(first + 1, storage),
            buffer(first + 2, storage),
        ]
    }

    /// The three bindings of the lamps, from the binding `first`.
    pub fn entries(&self, first: u32) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            wgpu::BindGroupEntry {
                binding: first,
                resource: self.values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 1,
                resource: self.lamps.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 2,
                resource: self.grid.as_entire_binding(),
            },
        ]
    }

    /// The binding with no lamps.
    pub fn new(device: &wgpu::Device) -> LampBinding {
        let empty = build(Vec::new(), 0);
        let (values, lamps, grid) = Self::buffers(device, &empty);
        LampBinding {
            values,
            lamps,
            grid,
            generation: 0,
        }
    }

    fn buffers(device: &wgpu::Device, grid: &LampGrid) -> (wgpu::Buffer, wgpu::Buffer, wgpu::Buffer) {
        let make = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        (
            make(
                "distant lamps, values",
                bytemuck::bytes_of(&grid.values),
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            make(
                "distant lamps, the list",
                bytemuck::cast_slice(&grid.lamps),
                wgpu::BufferUsages::STORAGE,
            ),
            make(
                "distant lamps, the grid",
                bytemuck::cast_slice(&grid.words),
                wgpu::BufferUsages::STORAGE,
            ),
        )
    }

    /// The lamps and the grid of `grid` in new buffers.
    pub fn set(&mut self, device: &wgpu::Device, grid: &LampGrid) {
        (self.values, self.lamps, self.grid) = Self::buffers(device, grid);
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outlander-test's settings: a constant of 0.36 and a quadratic term of 3.25 by method 2.
    fn outlander_test() -> LightSettings {
        LightSettings {
            flags: 1 | 4,
            linear_method: 1,
            quadratic_method: 2,
            constant: 0.36,
            linear: 0.0,
            quadratic: 3.25,
            linear_radius_multiplier: 1.0,
            quadratic_radius_multiplier: 1.0,
            out_quad_in_lin: false,
            light_limit_asked: 48,
            light_limit: 48,
        }
    }

    /// Morrowind75testing's settings: the linear term only, 3 by method 1.
    fn morrowind75testing() -> LightSettings {
        LightSettings {
            flags: 2,
            constant: 0.0,
            linear: 3.0,
            quadratic: 0.0,
            ..outlander_test()
        }
    }

    fn lamp(x: f32, y: f32, radius: f32) -> SourceLamp {
        SourceLamp {
            position: [x, y, 0.0],
            radius,
            color: [255, 200, 100],
            flags: 0,
        }
    }

    #[test]
    fn the_end_with_the_fade_is_the_near_lights_end() {
        assert_eq!(end_of(400.0, [0.0, 0.0075, 0.0], 1.75), 700.0);
        // As shade_lights: a radius of at least 16, an end of at most 8192.
        assert_eq!(end_of(4.0, [0.0, 0.75, 0.0], 1.75), 28.0);
        assert_eq!(end_of(9000.0, [0.0, 0.0, 0.0], 1.75), 8192.0);
    }

    #[test]
    fn the_end_without_the_fade_is_where_the_falloff_is_a_25th() {
        // Outlander-test: 1 / (0.36 + 3.25 (d/r)^2) = 0.04 at d = 2.75 r (the fork's reach).
        let terms = outlander_test().terms(450, false);
        let end = end_of(450.0, terms, 0.0);
        assert!((end / 450.0 - 2.7535).abs() < 0.001, "{end}");
        // Morrowind75testing: r / (3 d) = 0.04 at d = 8.3 r, so the most, 4 radii.
        let terms = morrowind75testing().terms(300, false);
        assert_eq!(end_of(300.0, terms, 0.0), 1200.0);
        // A constant that is never under 25: no light that ends.
        assert_eq!(end_of(300.0, [30.0, 0.0, 0.0], 0.0), 0.0);
        // No falloff at all: the most.
        assert_eq!(end_of(300.0, [1.0, 0.0, 0.0], 0.0), 1200.0);
    }

    #[test]
    fn a_lamp_takes_the_terms_of_a_near_lamp_of_its_radius() {
        let settings = outlander_test();
        let lamps = gpu_lamps(&[lamp(100.0, 200.0, 450.0)], &settings, 1.75);
        assert_eq!(lamps.len(), 1);
        assert_eq!(lamps[0].terms, settings.terms(450, false));
        assert_eq!(lamps[0].end, 787.5);
        assert_eq!(lamps[0].color, [1.0, 200.0 / 255.0, 100.0 / 255.0]);
        // A radius of 0 ends at once without the fade, and at 16 radii x 1.75 with it.
        assert!(gpu_lamps(&[lamp(0.0, 0.0, 0.0)], &settings, 0.0).is_empty());
        assert_eq!(gpu_lamps(&[lamp(0.0, 0.0, 0.0)], &settings, 1.75)[0].end, 28.0);
    }

    /// The lamps that a world position finds through the grid, as the shader walks it.
    fn lamps_at(grid: &LampGrid, x: f32, y: f32) -> Vec<u32> {
        let values = grid.values;
        let [x0, y0, width, height] = values.grid;
        let [side, level2, lists_at, _] = values.counts;
        let size = values.values[0];
        let cell = ((x / CELL).floor() as i32 - x0, (y / CELL).floor() as i32 - y0);
        if cell.0 < 0 || cell.1 < 0 || cell.0 >= width || cell.1 >= height {
            return Vec::new();
        }
        let lit = grid.words[(cell.1 * width + cell.0) as usize];
        if lit == NO_CELL {
            return Vec::new();
        }
        let local_x = ((x - (cell.0 + x0) as f32 * CELL) / size).floor() as u32;
        let local_y = ((y - (cell.1 + y0) as f32 * CELL) / size).floor() as u32;
        let pair = (level2 + 2 * (lit * side * side + local_y * side + local_x)) as usize;
        let (start, count) = (grid.words[pair], grid.words[pair + 1]);
        let from = (lists_at + start) as usize;
        grid.words[from..from + count as usize].to_vec()
    }

    #[test]
    fn a_point_finds_every_lamp_whose_end_covers_it() {
        let settings = outlander_test();
        // Lamps in three exterior cells, two of them far apart, one on a border of cells.
        let source = [
            lamp(100.0, 100.0, 200.0),
            lamp(500.0, 300.0, 300.0),
            lamp(8192.0, 50.0, 256.0),
            lamp(-40000.0, 90000.0, 128.0),
        ];
        let lamps = gpu_lamps(&source, &settings, 1.75);
        let grid = build(lamps.clone(), 48);
        assert_eq!(grid.report.size, 1024.0);
        assert_eq!(grid.report.cells_over, 0);
        // At points around each lamp, the grid's list has every lamp that reaches the point.
        for x in (-41000..10000).step_by(97) {
            for y in [-500, 0, 99, 300, 700, 89600, 90000, 90400] {
                let (x, y) = (x as f32, y as f32);
                let found = lamps_at(&grid, x, y);
                for (index, lamp) in lamps.iter().enumerate() {
                    let (dx, dy) = (lamp.position[0] - x, lamp.position[1] - y);
                    if dx * dx + dy * dy < lamp.end * lamp.end {
                        assert!(found.contains(&(index as u32)), "lamp {index} at {x}, {y}");
                    }
                }
            }
        }
        // The far lamp makes a grid of many empty cells. Only the 8 cells that a lamp reaches
        // have a second level.
        assert_eq!(grid.report.lit_cells, 8, "{:?}", grid.report);
        assert!(grid.report.describe().contains("4 lamps, cells of 1024 units"));
    }

    #[test]
    fn a_full_place_takes_a_smaller_cell_and_then_counts_what_it_leaves_out() {
        let settings = outlander_test();
        // 60 lamps over 1000 units: no list of 1024 units is within 48.
        let spread: Vec<SourceLamp> = (0..60).map(|i| lamp((i * 16) as f32, 300.0, 64.0)).collect();
        let grid = build(gpu_lamps(&spread, &settings, 1.75), 48);
        assert!(grid.report.size < 1024.0, "{:?}", grid.report);
        assert_eq!(grid.report.cells_over, 0);
        assert!(grid.report.fullest <= 48);
        // 60 lamps on one point: no size helps; each list keeps 48 and the report says so.
        let piled: Vec<SourceLamp> = (0..60)
            .map(|i| SourceLamp {
                color: [i as u8 * 4, 0, 0],
                ..lamp(300.0, 300.0, 64.0)
            })
            .collect();
        let grid = build(gpu_lamps(&piled, &settings, 1.75), 48);
        assert_eq!(grid.report.size, 256.0);
        assert!(grid.report.cells_over > 0);
        assert_eq!(grid.report.left_out, 12 * grid.report.cells_over);
        // The brightest lamps stay: the 12 darkest (0 to 11) are left out.
        let kept = lamps_at(&grid, 300.0, 300.0);
        assert_eq!(kept.len(), 48);
        assert!(kept.iter().all(|&index| index >= 12));
        assert!(grid.report.describe().contains("warning:"));
    }

    #[test]
    fn the_bound_is_the_light_limit_in_force() {
        let settings = outlander_test();
        let ten: Vec<SourceLamp> = (0..10).map(|i| lamp(300.0 + i as f32, 300.0, 64.0)).collect();
        let grid = build(gpu_lamps(&ten, &settings, 1.75), 7);
        assert_eq!(grid.report.bound, 7);
        assert_eq!(lamps_at(&grid, 300.0, 300.0).len(), 7);
    }

    #[test]
    fn no_lamps_make_buffers_of_one_element() {
        let grid = build(Vec::new(), 48);
        assert_eq!(grid.lamps.len(), 1);
        assert_eq!(grid.words, vec![NO_CELL]);
        assert_eq!(grid.values.counts[3], 0);
        assert_eq!(grid.values.grid, [0; 4]);
        assert_eq!(grid.report.describe(), "distant lamps: none");
    }

    #[test]
    fn the_lamp_is_64_bytes_and_the_wgsl_names_its_bindings() {
        assert_eq!(size_of::<GpuLamp>(), 64);
        assert_eq!(size_of::<LampValues>(), 48);
        let code = wgsl_at(1);
        assert!(code.contains("@group(0) @binding(1) var<uniform> lamp_values"));
        assert!(code.contains("@group(0) @binding(3) var<storage, read> lamp_grid"));
        assert!(!code.contains("@binding(B"));
    }
}
