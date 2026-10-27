//! `distantland\lights.data`: the light sources of the exterior for the distant lights of the
//! renderer, and the light that they give the land of each lit exterior cell. The generator
//! writes it (`distantland` src/generation/lights.rs, with these constants); the renderer reads
//! it with [`parse_lights`].
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

use std::io;

/// The magic of the file.
pub const LIGHTS_MAGIC: &[u8; 8] = b"XELIGHT1";
/// The version of the layout.
pub const LIGHTS_VERSION: u32 = 2;
/// The bytes of the header.
pub const LIGHTS_HEADER_SIZE: usize = 40;
/// The bytes of one light source.
pub const LIGHT_SOURCE_SIZE: usize = 32;
/// The values on a side of a tile: the vertices on a side of the land of a cell.
pub const LIGHT_TILE: usize = 65;
/// The side of a cell in the units of the game.
pub const LIGHT_CELL: f32 = 8192.0;
/// The distance between two values of a tile.
pub const LIGHT_STEP: f32 = 128.0;
/// The bytes of one value of a tile.
pub const LIGHT_TEXEL_SIZE: usize = 4;
/// The bytes of the cell of one tile: x and y.
const TILE_CELL_SIZE: usize = 8;

/// A `lights.data` file, checked: its parts as the renderer takes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Lights {
    /// The sources, [`LIGHT_SOURCE_SIZE`] bytes each, as in the file.
    pub sources: Vec<u8>,
    /// The number of sources.
    pub source_count: u32,
    /// The cell of each tile.
    pub tiles: Vec<(i32, i32)>,
    /// The values of the tiles, in the order of `tiles`, [`LIGHT_TEXEL_SIZE`] bytes each.
    pub texels: Vec<u8>,
}

/// Reads a `lights.data` file. Every count is checked against the length of `bytes` before a
/// part is copied.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] with the reason when the file is truncated, has another
/// magic, version or grid, or its length is not that of its counts.
pub fn parse_lights(bytes: &[u8]) -> io::Result<Lights> {
    if bytes.len() < LIGHTS_HEADER_SIZE {
        return Err(invalid_data(format!(
            "lights.data is truncated: expected at least {LIGHTS_HEADER_SIZE} header bytes, found {}",
            bytes.len()
        )));
    }
    if &bytes[..8] != LIGHTS_MAGIC {
        return Err(invalid_data("lights.data magic mismatch: not an XELIGHT1 file"));
    }
    let version = word(bytes, 8);
    if version != LIGHTS_VERSION {
        return Err(invalid_data(format!(
            "lights.data version mismatch: expected {LIGHTS_VERSION}, found {version}"
        )));
    }
    let (source_count, tile_count, tile) = (word(bytes, 12), word(bytes, 16), word(bytes, 20));
    if tile as usize != LIGHT_TILE {
        return Err(invalid_data(format!("lights.data tile side mismatch: expected {LIGHT_TILE}, found {tile}")));
    }
    let (cell, step) = (f32::from_bits(word(bytes, 24)), f32::from_bits(word(bytes, 28)));
    if cell != LIGHT_CELL || step != LIGHT_STEP {
        return Err(invalid_data(format!(
            "lights.data grid mismatch: expected a cell of {LIGHT_CELL} and a step of {LIGHT_STEP}, found {cell} and {step}"
        )));
    }

    // The sizes in u64, so that no count can overflow them.
    let tile_bytes = (LIGHT_TILE * LIGHT_TILE * LIGHT_TEXEL_SIZE) as u64;
    let sources_end = LIGHTS_HEADER_SIZE as u64 + u64::from(source_count) * LIGHT_SOURCE_SIZE as u64;
    let cells_end = sources_end + u64::from(tile_count) * TILE_CELL_SIZE as u64;
    let texels_end = cells_end + u64::from(tile_count) * tile_bytes;
    if bytes.len() as u64 != texels_end {
        return Err(invalid_data(format!(
            "lights.data has {} bytes, and its {source_count} sources and {tile_count} tiles need {texels_end}",
            bytes.len()
        )));
    }

    // All the ends are inside `bytes` now.
    let (sources_end, cells_end) = (sources_end as usize, cells_end as usize);
    let tiles = bytes[sources_end..cells_end]
        .as_chunks::<TILE_CELL_SIZE>()
        .0
        .iter()
        .map(|cell| (word(cell, 0) as i32, word(cell, 4) as i32))
        .collect();
    Ok(Lights {
        sources: bytes[LIGHTS_HEADER_SIZE..sources_end].to_vec(),
        source_count,
        tiles,
        texels: bytes[cells_end..].to_vec(),
    })
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file of `sources` sources and `tiles` tiles, each byte of a part its own value.
    fn file(sources: u32, tiles: u32) -> Vec<u8> {
        let mut bytes = LIGHTS_MAGIC.to_vec();
        for value in [LIGHTS_VERSION, sources, tiles, LIGHT_TILE as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&LIGHT_CELL.to_le_bytes());
        bytes.extend_from_slice(&LIGHT_STEP.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend((0..sources as usize * LIGHT_SOURCE_SIZE).map(|i| i as u8));
        for tile in 0..tiles as i32 {
            bytes.extend_from_slice(&(tile - 3).to_le_bytes());
            bytes.extend_from_slice(&(7 - tile).to_le_bytes());
        }
        bytes.extend((0..tiles as usize * LIGHT_TILE * LIGHT_TILE * LIGHT_TEXEL_SIZE).map(|i| (i * 7) as u8));
        bytes
    }

    #[test]
    fn a_file_reads_back_its_parts() {
        let bytes = file(3, 2);
        let lights = parse_lights(&bytes).unwrap();
        assert_eq!(lights.source_count, 3);
        assert_eq!(lights.sources, bytes[LIGHTS_HEADER_SIZE..LIGHTS_HEADER_SIZE + 3 * LIGHT_SOURCE_SIZE]);
        assert_eq!(lights.tiles, vec![(-3, 7), (-2, 6)]);
        assert_eq!(lights.texels.len(), 2 * LIGHT_TILE * LIGHT_TILE * LIGHT_TEXEL_SIZE);
        assert_eq!(lights.texels[..], bytes[bytes.len() - lights.texels.len()..]);

        let empty = parse_lights(&file(0, 0)).unwrap();
        assert_eq!((empty.source_count, empty.tiles.len(), empty.texels.len()), (0, 0, 0));
    }

    #[test]
    fn a_bad_file_is_refused_with_its_reason() {
        let original = file(2, 1);
        let refused = |bytes: &[u8], reason: &str| {
            let error = parse_lights(bytes).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(reason), "{error}");
        };
        refused(&original[..LIGHTS_HEADER_SIZE - 1], "truncated");
        refused(&original[..original.len() - 1], "need");
        let mut bytes = original.clone();
        bytes[0] = b'Y';
        refused(&bytes, "magic");
        let mut bytes = original.clone();
        bytes[8] = 3;
        refused(&bytes, "version");
        let mut bytes = original.clone();
        bytes[20] = 64;
        refused(&bytes, "tile side");
        let mut bytes = original.clone();
        bytes[24..28].copy_from_slice(&4096.0f32.to_le_bytes());
        refused(&bytes, "grid");
        // Counts that would overflow 32 bits of bytes are a length that does not match.
        let mut bytes = original.clone();
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        refused(&bytes, "need");
    }

    /// A corruption of a valid file gives Ok or Err, never a panic: each truncation, and 4000
    /// changes of one to four bytes at places from a fixed seed.
    #[test]
    fn no_corruption_makes_the_reader_panic() {
        let original = file(3, 1);
        for end in 0..original.len() {
            let _ = parse_lights(&original[..end]);
        }
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        for _ in 0..4000 {
            let mut bytes = original.clone();
            for _ in 0..1 + next() % 4 {
                // Half of the changes in the header, where the counts are.
                let at = if next() % 2 == 0 { next() % LIGHTS_HEADER_SIZE } else { next() % bytes.len() };
                bytes[at] = next() as u8;
            }
            let _ = parse_lights(&bytes);
        }
    }
}
