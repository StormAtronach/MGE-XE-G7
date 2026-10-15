//! The light sources of the exterior for the distant lights of the renderer, and the light
//! that they give the land far away: `distantland\lights.data`.
//!
//! The sources are the placed references of the light records of the load order: their place,
//! colour, radius and flags. A light that is off by default, or a negative light, is left out.
//! The renderer draws a glow for each source with a mesh beyond the cells that the game has
//! loaded.
//!
//! With `bake_lights` the file also has the light of the sources on the land of each lit
//! exterior cell: one tile for each cell, a value for each of the 65 x 65 vertices of its land
//! (the grid of `TerrainCell::heights`, 128 units apart), so that the tiles of two cells meet at
//! the same values. The renderer adds it to the light of the distant land and statics.
//!
//! The file is written apart from the committed inventory of the generation, when its bytes
//! change: it is rebuilt at every generation run, which costs little.
//!
//! The layout, little endian:
//! - The header, 40 bytes: `XELIGHT1`; the version (2); the number of sources; the number of
//!   tiles; the side of a tile (65); the side of a cell (8192.0); the step of the grid (128.0);
//!   two zero words.
//! - The sources, 32 bytes each: x, y, z and the radius as f32; the colour as four bytes (red,
//!   green, blue, then 255 when the light has a mesh, else 0); the flags of the record (u32);
//!   two zero words.
//! - The cells of the tiles: x and y as i32 for each tile.
//! - The tiles: for each, 65 x 65 values, row after row from the south-west corner, each four
//!   bytes (red, green, blue, 255): half of the light, so that a byte holds a light up to 2.

use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result};

use super::{GenerationSettings, OutputPaths};
use crate::UsageInfo;

pub(crate) const LIGHTS_MAGIC: &[u8; 8] = b"XELIGHT1";
pub(crate) const LIGHTS_VERSION: u32 = 2;
/// The values on a side of a tile: the vertices on a side of the land of a cell.
pub(crate) const TILE: usize = 65;
const CELL: f32 = 8192.0;
const STEP: f32 = 128.0;
/// `tes3::esp::LightFlags`.
const NEGATIVE: u32 = 0x4;
const OFF_BY_DEFAULT: u32 = 0x20;
/// The bake reaches this many times the radius of a light: the radius of the game is where its
/// light is already weak, and far away a light should still make a pool on the ground.
const BAKE_REACH: f32 = 1.5;
/// The height difference counts this much against the distance across the ground. The values
/// are on the land, and the statics read them by their place across the ground: a lantern high
/// on the wall of a canton should light that wall, so the height counts little.
const HEIGHT_WEIGHT: f32 = 0.1;

/// A light source in the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightSource {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [u8; 3],
    pub flags: u32,
    pub has_mesh: bool,
}

/// The bytes of the file, and what they hold.
pub(crate) struct LightsOutput {
    pub bytes: Vec<u8>,
    pub source_count: usize,
    pub tile_count: usize,
}

/// The light sources of the exterior, sorted by their place.
pub(crate) fn light_sources(usage: &UsageInfo) -> Vec<LightSource> {
    let Some(references) = usage.exterior_references() else { return Vec::new() };
    let mut sources: Vec<LightSource> = references
        .values()
        .filter(|reference| !reference.deleted)
        .filter_map(|reference| {
            let record = usage.lights.get(reference.id.as_ref())?;
            if record.radius <= 0.0 || record.flags & (NEGATIVE | OFF_BY_DEFAULT) != 0 {
                return None;
            }
            Some(LightSource {
                position: reference.translation.to_array(),
                radius: record.radius,
                color: record.color,
                flags: record.flags,
                has_mesh: record.has_mesh,
            })
        })
        .collect();
    sources.sort_by(|a, b| {
        let key = |s: &LightSource| (s.position.map(f32::to_bits), s.radius.to_bits(), s.color, s.flags);
        key(a).cmp(&key(b))
    });
    sources
}

/// The light of `sources` at a point.
fn light_at(point: [f32; 3], sources: &[&LightSource]) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for source in sources {
        let reach = source.radius * BAKE_REACH;
        let dx = point[0] - source.position[0];
        let dy = point[1] - source.position[1];
        let dz = (point[2] - source.position[2]) * HEIGHT_WEIGHT;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();
        if distance >= reach {
            continue;
        }
        let falloff = 1.0 - distance / reach;
        let strength = falloff * falloff;
        for (channel, value) in sum.iter_mut().zip(source.color) {
            *channel += strength * value as f32 / 255.0;
        }
    }
    sum
}

/// The baked tiles: the cells with land that a source reaches, with their values.
fn bake(usage: &UsageInfo, sources: &[LightSource]) -> BTreeMap<(i32, i32), Vec<[f32; 3]>> {
    let mut by_cell: BTreeMap<(i32, i32), Vec<&LightSource>> = BTreeMap::new();
    for source in sources {
        let reach = source.radius * BAKE_REACH;
        let cell = |value: f32| (value / CELL).floor() as i32;
        for x in cell(source.position[0] - reach)..=cell(source.position[0] + reach) {
            for y in cell(source.position[1] - reach)..=cell(source.position[1] + reach) {
                by_cell.entry((x, y)).or_default().push(source);
            }
        }
    }
    let mut tiles = BTreeMap::new();
    for (grid, near) in by_cell {
        let Some(land) = usage.terrain_cells.get(&grid) else { continue };
        let origin = [grid.0 as f32 * CELL, grid.1 as f32 * CELL];
        let mut values = Vec::with_capacity(TILE * TILE);
        let mut lit = false;
        for y in 0..TILE {
            for x in 0..TILE {
                let point = [origin[0] + x as f32 * STEP, origin[1] + y as f32 * STEP, land.heights[y][x]];
                let value = light_at(point, &near);
                lit |= value.iter().any(|channel| *channel > 0.0);
                values.push(value);
            }
        }
        if lit {
            tiles.insert(grid, values);
        }
    }
    tiles
}

/// The bytes of `lights.data` for `sources` (from `light_sources`, before the references are
/// remapped to meshes) and the land of `usage`, or `None` when the distant lights are off.
pub(crate) fn build(sources: &[LightSource], usage: &UsageInfo, settings: &GenerationSettings) -> Option<LightsOutput> {
    if !settings.distant_lights {
        return None;
    }
    let tiles = if settings.bake_lights { bake(usage, sources) } else { BTreeMap::new() };

    let mut bytes = Vec::with_capacity(40 + sources.len() * 32 + tiles.len() * (8 + TILE * TILE * 4));
    bytes.extend_from_slice(LIGHTS_MAGIC);
    for word in [LIGHTS_VERSION, sources.len() as u32, tiles.len() as u32, TILE as u32] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&CELL.to_le_bytes());
    bytes.extend_from_slice(&STEP.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    for source in sources {
        for value in source.position.iter().chain([&source.radius]) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&source.color);
        bytes.push(if source.has_mesh { 255 } else { 0 });
        bytes.extend_from_slice(&source.flags.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
    }
    for grid in tiles.keys() {
        bytes.extend_from_slice(&grid.0.to_le_bytes());
        bytes.extend_from_slice(&grid.1.to_le_bytes());
    }
    for values in tiles.values() {
        for value in values {
            for channel in [value[0], value[1], value[2]] {
                bytes.push((channel * 0.5 * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            bytes.push(255);
        }
    }
    Some(LightsOutput { bytes, source_count: sources.len(), tile_count: tiles.len() })
}

/// True when the file on disk is the one that `output` would write (or there is none, and
/// `output` is `None`).
pub(crate) fn is_current(paths: &OutputPaths, output: Option<&LightsOutput>) -> bool {
    match (output, fs::read(&paths.lights_path)) {
        (Some(output), Ok(existing)) => existing == output.bytes,
        (None, Err(_)) => true,
        _ => false,
    }
}

/// Writes the file when its bytes change, or removes it when the distant lights are off. True
/// when the disk changed.
pub(crate) fn publish(paths: &OutputPaths, output: Option<&LightsOutput>) -> Result<bool> {
    if is_current(paths, output) {
        return Ok(false);
    }
    match output {
        Some(output) => {
            let temporary = paths.lights_path.with_extension("data.tmp");
            fs::write(&temporary, &output.bytes).with_context(|| format!("Failed to write {}", temporary.display()))?;
            fs::rename(&temporary, &paths.lights_path).with_context(|| format!("Failed to replace {}", paths.lights_path.display()))?;
        }
        None => {
            fs::remove_file(&paths.lights_path).with_context(|| format!("Failed to remove {}", paths.lights_path.display()))?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(position: [f32; 3], radius: f32, color: [u8; 3]) -> LightSource {
        LightSource { position, radius, color, flags: 0, has_mesh: true }
    }

    #[test]
    fn the_light_is_full_at_the_source_and_gone_at_its_reach() {
        let lamp = source([0.0, 0.0, 0.0], 100.0, [255, 128, 0]);
        let at = light_at([0.0, 0.0, 0.0], &[&lamp]);
        assert_eq!(at, [1.0, 128.0 / 255.0, 0.0]);
        assert_eq!(light_at([150.0, 0.0, 0.0], &[&lamp]), [0.0; 3]);
        let half_way = light_at([75.0, 0.0, 0.0], &[&lamp]);
        assert!((half_way[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn height_counts_a_tenth() {
        let lamp = source([0.0, 0.0, 100.0], 100.0, [255, 255, 255]);
        // 100 units up counts as 10 across the ground.
        let below = light_at([0.0, 0.0, 0.0], &[&lamp]);
        let across = light_at([10.0, 0.0, 100.0], &[&lamp]);
        assert!((below[0] - across[0]).abs() < 1e-6);
    }

    #[test]
    fn lights_add() {
        let a = source([0.0, 0.0, 0.0], 100.0, [100, 0, 0]);
        let b = source([0.0, 0.0, 0.0], 100.0, [100, 0, 0]);
        assert!((light_at([0.0; 3], &[&a, &b])[0] - 200.0 / 255.0).abs() < 1e-6);
    }
}
