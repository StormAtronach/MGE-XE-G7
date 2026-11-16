//! `distantland\lights.data`: the placed lights and the smoke sources of the distant land, by
//! block. The generator writes it with [`Lights::to_bytes`] (`distantland`
//! src/generation/lights.rs); the renderer reads it with [`parse_lights`].
//!
//! A block is a part of the world that has distant land: the exterior, with the empty key, or an
//! interior that behaves like an exterior, keyed by its cell name. The blocks are sorted by their
//! keys, with no key two times.
//!
//! A block's key and a record's id are a [`Key`]: the engine's own bytes of the name, with A to Z
//! lowered and nothing else changed. Two keys are equal when their bytes are equal. [`key`] is the
//! one function that makes a key, and each side uses it: the generator for the names of the
//! plugins, the renderer for the names that the running game gives. So the two sides agree
//! whatever the engine's own compare of names does. \[I\]: the engine's compare of names is not
//! read; it matters only where two cells or two records differ in the case of a letter over 127.
//! The names are in the code page of the install, so a key is never compared as text.
//!
//! The layout, little endian, every part a whole number of 4-byte words:
//! - The header, 16 bytes: `XELIGHT1`; the version (3); the number of blocks.
//! - Each block:
//!   - its key: the length in bytes (u32), then the bytes, with zeros to the next word;
//!   - the numbers of its records, lamps and smoke sources (u32 each);
//!   - the records, the light records that its lamps are: the length of the id (u32), the id
//!     with zeros to the next word, and the radius of the glowing parts of the record's model
//!     (f32, 0 when not known or when the glow is a billboard);
//!   - the lamps, [`LIGHT_LAMP_SIZE`] bytes each, the layout that the renderer's shaders read:
//!     x, y, z and the radius as f32; the colour as four bytes (red, green, blue, then 255 when
//!     the record has a model, else 0); the flags of the record (u32); the index of the record
//!     in the block (u32); one zero word. The radius, colour and flags are the plugins' values,
//!     which stand in until the game gives its own;
//!   - the smoke sources, [`LIGHT_SMOKE_SIZE`] bytes each: x, y, z and the strength as f32.

use std::{fmt, io};

/// The magic of the file.
pub const LIGHTS_MAGIC: &[u8; 8] = b"XELIGHT1";
/// The version of the layout.
pub const LIGHTS_VERSION: u32 = 3;
/// The bytes of the header.
pub const LIGHTS_HEADER_SIZE: usize = 16;
/// The bytes of one lamp.
pub const LIGHT_LAMP_SIZE: usize = 32;
/// The bytes of one smoke source.
pub const LIGHT_SMOKE_SIZE: usize = 16;
/// The key of the exterior's block.
pub const EXTERIOR_KEY: Key = Key(Vec::new());
/// The fewest bytes of a block: the length of its key and its three counts.
const BLOCK_MIN_SIZE: usize = 16;
/// The fewest bytes of a record: the length of its id and its glow radius.
const RECORD_MIN_SIZE: usize = 8;

/// The name of a cell or of a light record as the file holds it: the engine's own bytes, with
/// A to Z lowered and nothing else changed. Only [`key`] makes one. The order of two keys is
/// the order of their bytes.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(Vec<u8>);

/// The key of a name in the engine's own bytes, which are in the code page of the install. It
/// takes every name: no byte makes it fail.
pub fn key(name: &[u8]) -> Key {
    Key(name.to_ascii_lowercase())
}

impl Key {
    /// The bytes of the key.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// For a log line only, never for a compare: printable ASCII as it is, a backslash two times,
/// and any other byte as hex (`\xc1`), because its letter depends on the code page.
impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write as _;

        for &byte in &self.0 {
            match byte {
                b'\\' => formatter.write_str("\\\\")?,
                b' '..=b'~' => formatter.write_char(char::from(byte))?,
                _ => write!(formatter, "\\x{byte:02x}")?,
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Key(\"{self}\")")
    }
}

/// A `lights.data` file, checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lights {
    /// The blocks, sorted by their keys.
    pub blocks: Vec<LightBlock>,
}

/// The lights of one part of the world.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightBlock {
    /// [`EXTERIOR_KEY`], or the key of an interior's cell name.
    pub key: Key,
    /// The light records that the lamps are.
    pub records: Vec<LampRecord>,
    pub lamps: Vec<Lamp>,
    pub smoke: Vec<SmokeSource>,
}

/// A light record of a block's lamps.
#[derive(Clone, Debug, PartialEq)]
pub struct LampRecord {
    /// The key of the record's id.
    pub id: Key,
    /// The radius of the glowing parts of the record's model; 0 when not known.
    pub glow_radius: f32,
}

/// A placed light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamp {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [u8; 3],
    /// The record has a model.
    pub has_mesh: bool,
    /// The flags of the record (`tes3::esp::LightFlags`).
    pub flags: u32,
    /// The index of the record in [`LightBlock::records`].
    pub record: u32,
}

/// A place where smoke rises.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmokeSource {
    pub position: [f32; 3],
    /// 1 for a smoke effect, 0.5 for a fire.
    pub strength: f32,
}

impl Lights {
    /// The block of `key`.
    pub fn block(&self, key: &Key) -> Option<&LightBlock> {
        self.blocks
            .binary_search_by(|block| block.key.cmp(key))
            .ok()
            .map(|index| &self.blocks[index])
    }

    /// The bytes of the file. The blocks must be sorted by their keys, as [`parse_lights`] asks.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = LIGHTS_MAGIC.to_vec();
        put_word(&mut bytes, LIGHTS_VERSION);
        put_word(&mut bytes, self.blocks.len() as u32);
        for block in &self.blocks {
            put_key(&mut bytes, &block.key);
            for count in [block.records.len(), block.lamps.len(), block.smoke.len()] {
                put_word(&mut bytes, count as u32);
            }
            for record in &block.records {
                put_key(&mut bytes, &record.id);
                bytes.extend_from_slice(&record.glow_radius.to_le_bytes());
            }
            bytes.extend_from_slice(&block.lamp_bytes());
            for smoke in &block.smoke {
                for value in smoke.position.iter().chain([&smoke.strength]) {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        bytes
    }
}

impl LightBlock {
    /// The lamps in the file's layout, [`LIGHT_LAMP_SIZE`] bytes each: what the renderer's
    /// shaders read.
    pub fn lamp_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.lamps.len() * LIGHT_LAMP_SIZE);
        for lamp in &self.lamps {
            for value in lamp.position.iter().chain([&lamp.radius]) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&lamp.color);
            bytes.push(if lamp.has_mesh { 255 } else { 0 });
            put_word(&mut bytes, lamp.flags);
            put_word(&mut bytes, lamp.record);
            put_word(&mut bytes, 0);
        }
        bytes
    }
}

/// Reads a `lights.data` file. Each count and length is checked against the bytes that are left
/// before a part is read, so a cut or changed file is refused and never read past its end.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] with the reason when the file is truncated, has another
/// magic or version, has a key or an id with a capital A to Z, has keys out of order, has a lamp
/// whose record is not in its block, or has bytes after its last block. Each of these is a broken
/// structure: the writer makes none of them. No name of a cell or of a record makes a file refused,
/// whatever its bytes are.
pub fn parse_lights(bytes: &[u8]) -> io::Result<Lights> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(8)? != LIGHTS_MAGIC {
        return Err(invalid_data("lights.data magic mismatch: not an XELIGHT1 file"));
    }
    let version = reader.word()?;
    if version != LIGHTS_VERSION {
        return Err(invalid_data(format!(
            "lights.data version mismatch: expected {LIGHTS_VERSION}, found {version}"
        )));
    }
    let block_count = reader.count(BLOCK_MIN_SIZE, "blocks")?;
    let mut blocks: Vec<LightBlock> = Vec::with_capacity(block_count);
    for _ in 0..block_count {
        let key = reader.key("key")?;
        if blocks.last().is_some_and(|last| last.key >= key) {
            return Err(invalid_data(format!(
                "lights.data block keys are not sorted or not unique at \"{key}\""
            )));
        }
        let record_count = reader.word()? as usize;
        let lamp_count = reader.word()? as usize;
        let smoke_count = reader.word()? as usize;
        // The three parts must fit in what is left, at their fewest bytes.
        let needed = record_count as u64 * RECORD_MIN_SIZE as u64
            + lamp_count as u64 * LIGHT_LAMP_SIZE as u64
            + smoke_count as u64 * LIGHT_SMOKE_SIZE as u64;
        if needed > reader.left() as u64 {
            return Err(truncated(needed, reader.left()));
        }

        let mut records = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            let id = reader.key("record id")?;
            records.push(LampRecord {
                id,
                glow_radius: reader.float()?,
            });
        }
        let mut lamps = Vec::with_capacity(lamp_count);
        for _ in 0..lamp_count {
            let lamp = reader.take(LIGHT_LAMP_SIZE)?;
            let record = word(lamp, 24);
            if record as usize >= record_count {
                return Err(invalid_data(format!(
                    "lights.data lamp names record {record} of a block of {record_count} records"
                )));
            }
            lamps.push(Lamp {
                position: [float(lamp, 0), float(lamp, 4), float(lamp, 8)],
                radius: float(lamp, 12),
                color: [lamp[16], lamp[17], lamp[18]],
                has_mesh: lamp[19] != 0,
                flags: word(lamp, 20),
                record,
            });
        }
        let mut smoke = Vec::with_capacity(smoke_count);
        for _ in 0..smoke_count {
            let source = reader.take(LIGHT_SMOKE_SIZE)?;
            smoke.push(SmokeSource {
                position: [float(source, 0), float(source, 4), float(source, 8)],
                strength: float(source, 12),
            });
        }
        blocks.push(LightBlock {
            key,
            records,
            lamps,
            smoke,
        });
    }
    if reader.left() != 0 {
        return Err(invalid_data(format!(
            "lights.data has {} bytes after its last block",
            reader.left()
        )));
    }
    Ok(Lights { blocks })
}

/// The bytes of a file, read from the front.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if count > self.left() {
            return Err(truncated(count as u64, self.left()));
        }
        let part = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(part)
    }

    fn word(&mut self) -> io::Result<u32> {
        self.take(4).map(|part| word(part, 0))
    }

    fn float(&mut self) -> io::Result<f32> {
        self.word().map(f32::from_bits)
    }

    /// A count of parts of at least `size` bytes each, checked against the bytes that are left.
    fn count(&mut self, size: usize, what: &str) -> io::Result<usize> {
        let count = self.word()? as u64;
        if count * size as u64 > self.left() as u64 {
            return Err(invalid_data(format!(
                "lights.data is truncated: {count} {what} need at least {} bytes, {} are left",
                count * size as u64,
                self.left()
            )));
        }
        Ok(count as usize)
    }

    /// A key: its length, its bytes and the zeros to the next word. A capital A to Z is a broken
    /// structure, because [`key`] makes none; every other byte is a part of a name.
    fn key(&mut self, what: &str) -> io::Result<Key> {
        let length = self.word()? as usize;
        let padded = length.checked_next_multiple_of(4).unwrap_or(usize::MAX);
        let part = self.take(padded)?;
        let name = &part[..length];
        if name.iter().any(u8::is_ascii_uppercase) {
            return Err(invalid_data(format!(
                "lights.data {what} is not in lower case: it has a capital A to Z"
            )));
        }
        Ok(Key(name.to_vec()))
    }
}

fn put_word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// A key as [`Reader::key`] reads it.
fn put_key(bytes: &mut Vec<u8>, key: &Key) {
    put_word(bytes, key.0.len() as u32);
    bytes.extend_from_slice(&key.0);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn float(bytes: &[u8], at: usize) -> f32 {
    f32::from_bits(word(bytes, at))
}

fn truncated(needed: u64, left: usize) -> io::Error {
    invalid_data(format!("lights.data is truncated: {needed} bytes needed, {left} are left"))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lamp(x: f32, record: u32) -> Lamp {
        Lamp {
            position: [x, 2.0, 3.0],
            radius: 450.0,
            color: [245, 140, 40],
            has_mesh: true,
            flags: 0x10,
            record,
        }
    }

    /// Two blocks: the exterior with two records, three lamps and a smoke source, and an
    /// interior with a key of five bytes (padded) and nothing else.
    fn sample() -> Lights {
        Lights {
            blocks: vec![
                LightBlock {
                    key: EXTERIOR_KEY,
                    records: vec![
                        LampRecord {
                            id: key(b"light_de_streetlight_01_223"),
                            glow_radius: 12.5,
                        },
                        LampRecord {
                            id: key(b"torch"),
                            glow_radius: 0.0,
                        },
                    ],
                    lamps: vec![lamp(1.0, 0), lamp(-5.0, 1), lamp(7.0, 0)],
                    smoke: vec![SmokeSource {
                        position: [9.0, 8.0, 7.0],
                        strength: 0.5,
                    }],
                },
                LightBlock {
                    key: key(b"mournhold"),
                    ..LightBlock::default()
                },
            ],
        }
    }

    #[test]
    fn a_file_reads_back_what_was_written() {
        let lights = sample();
        let bytes = lights.to_bytes();
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(parse_lights(&bytes).unwrap(), lights);
        assert_eq!(parse_lights(&Lights::default().to_bytes()).unwrap(), Lights::default());
    }

    #[test]
    fn a_block_is_found_by_its_key() {
        let lights = sample();
        assert_eq!(lights.block(&EXTERIOR_KEY).unwrap().lamps.len(), 3);
        assert!(lights.block(&key(b"mournhold")).unwrap().lamps.is_empty());
        assert!(lights.block(&key(b"Mournhold")).unwrap().lamps.is_empty());
        assert!(lights.block(&key(b"balmora")).is_none());
    }

    /// A to Z are lowered, and no other byte changes: a letter over 127 keeps its case, because
    /// its case depends on the code page of the install.
    #[test]
    fn a_key_lowers_a_to_z_and_changes_nothing_else() {
        assert_eq!(key(b"Balmora, Guild of MAGES"), key(b"balmora, guild of mages"));
        assert_eq!(key(b"AZ[`az{").as_bytes(), b"az[`az{");
        // 0xC1 and 0xE1 are one letter in two cases in WINDOWS-1251 and in WINDOWS-1252.
        assert_ne!(key(b"\xc1"), key(b"\xe1"));
        let every_byte: Vec<u8> = (0..=255).collect();
        let lowered = key(&every_byte);
        for (byte, kept) in every_byte.iter().zip(lowered.as_bytes()) {
            let expected = if byte.is_ascii_uppercase() { byte + 32 } else { *byte };
            assert_eq!(*kept, expected);
        }
        assert_eq!(key(b"").as_bytes(), EXTERIOR_KEY.as_bytes());
        // For a log line: printable ASCII as it is, a backslash two times, any other byte as hex.
        assert_eq!(key(b"Caf\xe9's \\ \x00").to_string(), "caf\\xe9's \\\\ \\x00");
    }

    /// A key and an id with every byte from 0 to 255 in them go through the writer and the reader.
    #[test]
    fn a_key_with_every_byte_reads_back() {
        let every_byte: Vec<u8> = (0..=255).collect();
        let lights = Lights {
            blocks: vec![LightBlock {
                key: key(&every_byte),
                records: vec![LampRecord {
                    id: key(&every_byte),
                    glow_radius: 0.0,
                }],
                lamps: vec![lamp(1.0, 0)],
                smoke: Vec::new(),
            }],
        };
        let read = parse_lights(&lights.to_bytes()).unwrap();
        assert_eq!(read, lights);
        assert_eq!(read.blocks[0].key.as_bytes().len(), 256);
    }

    /// A cell name and a record id with bytes over 127 are in the file as the engine's own bytes,
    /// and the reader gives the same keys back. `\xc1\xe0\xeb\xec\xee\xf0\xe0` is "Balmora" in
    /// WINDOWS-1251.
    #[test]
    fn a_name_with_bytes_over_127_is_in_the_file_as_the_engines_bytes() {
        let cell = b"\xc1\xe0\xeb\xec\xee\xf0\xe0, Guild";
        let id = b"Lampe_\xe9t\xe9";
        let lights = Lights {
            blocks: vec![
                LightBlock {
                    key: EXTERIOR_KEY,
                    records: vec![LampRecord {
                        id: key(id),
                        glow_radius: 0.0,
                    }],
                    lamps: vec![lamp(1.0, 0)],
                    smoke: Vec::new(),
                },
                LightBlock {
                    key: key(cell),
                    ..LightBlock::default()
                },
            ],
        };
        let bytes = lights.to_bytes();
        let holds = |name: &[u8]| bytes.windows(name.len()).any(|part| part == name);
        assert!(holds(b"\xc1\xe0\xeb\xec\xee\xf0\xe0, guild"));
        assert!(holds(b"lampe_\xe9t\xe9"));

        let read = parse_lights(&bytes).unwrap();
        assert_eq!(read, lights);
        assert_eq!(
            read.block(&key(cell)).unwrap().key.as_bytes(),
            b"\xc1\xe0\xeb\xec\xee\xf0\xe0, guild"
        );
    }

    /// The lamps' bytes are the layout of the renderer's `Source`: the place and the radius, the
    /// colour with the model byte, the flags, the record and a zero word.
    #[test]
    fn the_lamp_bytes_are_the_shaders_layout() {
        let bytes = sample().blocks[0].lamp_bytes();
        assert_eq!(bytes.len(), 3 * LIGHT_LAMP_SIZE);
        let second = &bytes[LIGHT_LAMP_SIZE..2 * LIGHT_LAMP_SIZE];
        assert_eq!(float(second, 0), -5.0);
        assert_eq!(float(second, 12), 450.0);
        assert_eq!(second[16..20], [245, 140, 40, 255]);
        assert_eq!(word(second, 20), 0x10);
        assert_eq!(word(second, 24), 1);
        assert_eq!(word(second, 28), 0);
    }

    #[test]
    fn a_bad_file_is_refused_with_its_reason() {
        let original = sample().to_bytes();
        let refused = |bytes: &[u8], reason: &str| {
            let error = parse_lights(bytes).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(reason), "{error}");
        };
        refused(&original[..LIGHTS_HEADER_SIZE - 1], "truncated");
        // A file cut in the middle of its lamps.
        refused(&original[..original.len() / 2], "truncated");
        refused(&original[..original.len() - 4], "truncated");
        let mut bytes = original.clone();
        bytes.extend_from_slice(&[0; 4]);
        refused(&bytes, "after its last block");
        let mut bytes = original.clone();
        bytes[0] = b'Y';
        refused(&bytes, "magic");
        let mut bytes = original.clone();
        bytes[8] = 2;
        refused(&bytes, "version");
        let mut bytes = original.clone();
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        refused(&bytes, "blocks need");

        let mut unsorted = sample();
        unsorted.blocks.swap(0, 1);
        refused(&unsorted.to_bytes(), "not sorted");
        let mut upper = sample();
        upper.blocks[1].key = Key(b"Mournhold".to_vec());
        refused(&upper.to_bytes(), "lower case");
        let mut unknown = sample();
        unknown.blocks[0].lamps[0].record = 2;
        refused(&unknown.to_bytes(), "names record 2");
    }

    /// A corruption of a valid file gives Ok or Err, never a panic: each truncation, and 4000
    /// changes of one to four bytes at places from a fixed seed.
    #[test]
    fn no_corruption_makes_the_reader_panic() {
        let original = sample().to_bytes();
        for end in 0..original.len() {
            let _ = parse_lights(&original[..end]);
        }
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        for _ in 0..4000 {
            let mut bytes = original.clone();
            for _ in 0..1 + next() % 4 {
                // Half of the changes in the first 64 bytes, where the counts and lengths are.
                let at = if next() % 2 == 0 { next() % 64 } else { next() % bytes.len() };
                bytes[at] = next() as u8;
            }
            let _ = parse_lights(&bytes);
        }
    }
}
