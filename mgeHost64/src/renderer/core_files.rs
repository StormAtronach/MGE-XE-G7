//! The core shader files of the package, `Data Files/shaders/core/<name>.wgsl` (D652): the
//! renderer's own shaders, under the names of MGE XE's core `.fx` files.
//!
//! Each file is built in from `assets/Data Files/shaders/core` of the repo. The helper does not
//! read the files of the package yet.
//!
//! A file is one text, or a list of parts. A part starts after its marker line
//! `//! part: <name>` and ends at the next marker line or at the end of the file. No text may
//! stand before the first marker. Every text has LF line ends, whatever the checkout has.
//!
//! The composition stays in the modules that use the texts: they join the parts as they joined
//! the constants before, and the core mods put their hooks in.

use std::sync::OnceLock;

/// The marker line of a part, before its name.
const MARKER: &str = "//! part: ";

/// A core file, built in.
pub(crate) struct CoreFile {
    /// The name in `Data Files/shaders/core`.
    pub name: &'static str,
    /// The text as the build read it: CRLF where the checkout has CRLF.
    source: &'static str,
    /// The names of the parts, in their order. Empty for a file that is one text.
    parts: &'static [&'static str],
    /// The text with LF line ends, made at the first use.
    text: OnceLock<String>,
}

/// A core file of `assets/Data Files/shaders/core`, built in, with the names of its parts.
macro_rules! core_file {
    ($name:literal, $parts:expr) => {
        CoreFile {
            name: $name,
            source: include_str!(concat!("../../../assets/Data Files/shaders/core/", $name)),
            parts: $parts,
            text: OnceLock::new(),
        }
    };
}

/// `GrassInstVS` and `GrassPS` of `XE Mod Grass.fx`, with the looks of Remiros' Groundcover.
pub(crate) static GRASS: CoreFile = core_file!("XE Mod Grass.wgsl", &[]);

/// The built-ins of the shadow hooks `shadow_delta` and `shadow_esm` (`XE Mod Shadow.fx`).
pub(crate) static SHADOW: CoreFile = core_file!("XE Mod Shadow.wgsl", &["shadow_delta", "shadow_esm"]);

/// The shadow map (`XE Shadowmap.fx`): the blur, and the casters of the land and of the
/// distant statics.
pub(crate) static SHADOWMAP: CoreFile = core_file!("XE Shadowmap.wgsl", &["blur", "land", "statics"]);

/// The water (`XE Mod Water.fx` and `XE Water Shaders.fx`): the shading that each surface of
/// water shares, the caustics function, the built-ins of the water hooks, the plane, and a level
/// of the reflection.
pub(crate) static WATER: CoreFile = core_file!(
    "XE Mod Water.wgsl",
    &[
        "common",
        "caustics",
        "water_normal",
        "water_vertex",
        "water_color",
        "underwater_color",
        "plane",
        "level"
    ]
);

/// The passes of the dynamic waves (`XE Water Shader Passes.fx`): the ripples.
pub(crate) static WATER_PASSES: CoreFile = core_file!("XE Water Shader Passes.wgsl", &["ripples"]);

/// Every core file.
#[cfg(test)]
static FILES: [&CoreFile; 5] = [&GRASS, &SHADOW, &SHADOWMAP, &WATER, &WATER_PASSES];

impl CoreFile {
    /// The whole text, with LF line ends.
    pub fn text(&self) -> &str {
        self.text.get_or_init(|| lf(self.source))
    }

    /// The text of the part `name`. A name that the file does not give is a fault of the
    /// build, which the tests find.
    pub fn part(&self, name: &str) -> &str {
        debug_assert!(
            self.parts.contains(&name),
            "{}: the part {name} is not named in its list",
            self.name
        );
        match parts_of(self.text()) {
            Ok(parts) => match parts.into_iter().find(|(part, _)| *part == name) {
                Some((_, text)) => text,
                None => panic!("{}: no part {name}", self.name),
            },
            Err(fault) => panic!("{}: {fault}", self.name),
        }
    }
}

/// `text` with LF line ends.
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The parts of a text with LF line ends: each name with its text, in the order of the file.
/// Empty for a text with no marker line. A fault: text before the first marker, a marker with
/// no name, or a name given twice.
fn parts_of(text: &str) -> Result<Vec<(&str, &str)>, String> {
    let mut parts: Vec<(&str, &str)> = Vec::new();
    // The name of the open part and the place where its text starts.
    let mut open: Option<(&str, usize)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix(MARKER) {
            let name = name.trim();
            if name.is_empty() {
                return Err(format!("a marker with no name at byte {at}"));
            }
            match open {
                Some((previous, start)) => parts.push((previous, &text[start..at])),
                None if at > 0 => return Err("text before the first part".to_string()),
                None => {}
            }
            if parts.iter().any(|(part, _)| *part == name) {
                return Err(format!("the part {name} is given twice"));
            }
            open = Some((name, at + line.len()));
        }
        at += line.len();
    }
    if let Some((name, start)) = open {
        parts.push((name, &text[start..]));
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::{FILES, GRASS, SHADOW, SHADOWMAP, WATER, WATER_PASSES, lf, parts_of};

    /// FNV-1a, 64 bits.
    fn fnv(text: &str) -> u64 {
        text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    #[test]
    fn each_file_has_the_parts_of_its_list() {
        for file in FILES {
            let parts = parts_of(file.text()).unwrap_or_else(|fault| panic!("{}: {fault}", file.name));
            let names: Vec<&str> = parts.iter().map(|(name, _)| *name).collect();
            assert_eq!(names, file.parts, "{}", file.name);
            assert!(!file.text().contains('\r'), "{}", file.name);
        }
    }

    /// The texts at their move from the constants of the Rust code (W609 step 3b): the FNV-1a of
    /// each old constant's text with LF line ends. A change of a shader changes its number here.
    #[test]
    fn the_texts_are_those_of_the_old_constants() {
        assert_eq!(fnv(GRASS.text()), 0x1ce0_752e_f783_79e3);
        assert_eq!(fnv(SHADOW.part("shadow_delta")), 0xbeaa_f0be_0c95_1991);
        assert_eq!(fnv(SHADOW.part("shadow_esm")), 0x2b40_758d_3635_aadf);
        assert_eq!(fnv(SHADOWMAP.part("blur")), 0x43b7_4db6_2d00_892d);
        assert_eq!(fnv(SHADOWMAP.part("land")), 0x1b2f_cbb0_6159_7102);
        assert_eq!(fnv(SHADOWMAP.part("statics")), 0xbbf3_bfe5_8a90_495b);
        assert_eq!(fnv(WATER.part("common")), 0xb3a9_9136_d140_4fc6);
        assert_eq!(fnv(WATER.part("caustics")), 0x7c9f_798d_fb1b_82ac);
        assert_eq!(fnv(WATER.part("water_normal")), 0xc7fa_1ed7_c273_59f8);
        assert_eq!(fnv(WATER.part("water_vertex")), 0xa00b_e822_b195_6596);
        assert_eq!(fnv(WATER.part("water_color")), 0x503c_9872_6fa2_1c2a);
        assert_eq!(fnv(WATER.part("underwater_color")), 0xcc0f_7200_b15b_69aa);
        assert_eq!(fnv(WATER.part("plane")), 0xa3ad_fab8_e938_8210);
        assert_eq!(fnv(WATER.part("level")), 0xa891_3b38_8e63_f3aa);
        assert_eq!(fnv(WATER_PASSES.part("ripples")), 0xf92d_65d2_3f52_5c72);
    }

    #[test]
    fn a_part_is_the_text_between_its_marker_and_the_next() {
        let text = "//! part: a\n\nfn a() {}\n//! part: b\nfn b() {}\n";
        assert_eq!(parts_of(text).unwrap(), [("a", "\nfn a() {}\n"), ("b", "fn b() {}\n")]);
        assert_eq!(parts_of("fn whole() {}\n").unwrap(), []);
        assert_eq!(parts_of(&lf("//! part: a\r\nfn a() {}\r\n")).unwrap(), [("a", "fn a() {}\n")]);
    }

    #[test]
    fn a_faulty_list_of_parts_is_refused() {
        for (text, fault) in [
            ("fn x() {}\n//! part: a\n", "text before the first part"),
            ("//! part: a\n//! part: a\n", "the part a is given twice"),
            ("//! part: a\n//! part: b\n//! part: a\n", "the part a is given twice"),
            ("//! part: a\n//! part:  \n", "a marker with no name at byte 12"),
        ] {
            assert_eq!(parts_of(text), Err(fault.to_string()), "{text:?}");
        }
    }
}
