//! The core shader files of the package, `Data Files/shaders/core/<name>.wgsl` (D652): the
//! renderer's own shaders, under the names of MGE XE's core `.fx` files.
//!
//! Each file is built in from `assets/Data Files/shaders/core` of the repo. The package has the
//! same files, each with a build line first, `//! core: <build id>` (xtask writes it). At the
//! start of a session, before the first pipeline, the helper reads the file of each name from
//! the places of `Data Files` ([`use_files`]) and chooses for each:
//!
//! - no file, no build line, or the build line of another build: the built-in;
//! - the file of this build with the text of the built-in: the file, which costs no check;
//! - the file of this build with another text (a user edited it): the parts of its list, each
//!   once, and then every shader that a core file is part of, checked as the device checks it,
//!   with the edited text. A fault gives the built-in, and the log names it.
//!
//! One log line names the choice of each file. A build id with `-dirty` is of the build of the
//! same id without it: the build line of a tree with changes is not the program's when the
//! program was built before the changes (xtask's release check). `MGE_XE_CORE_FILES=0` reads no
//! file: the built-ins only. The session keeps its choice: `mge.reloadDistantLand` does not
//! read the files again.
//!
//! A file is one text, or a list of parts. A part starts after its marker line
//! `//! part: <name>` and ends at the next marker line or at the end of the file. No text may
//! stand before the first marker. Every text has LF line ends, whatever the checkout has.
//!
//! The composition stays in the modules that use the texts: they join the parts as they joined
//! the constants before, and the core mods put their hooks in.

use std::cell::RefCell;
use std::sync::OnceLock;

/// The marker line of a part, before its name.
const MARKER: &str = "//! part: ";

/// The start of the build line, the first line of each core file of the package: the build id
/// follows (`stamp_core_files` of xtask).
const BUILD_LINE: &str = "//! core: ";

/// The folder of the core files under `Data Files`.
pub(crate) const FOLDER: &str = "shaders/core";

/// The switch that turns the files of the package off: the built-ins only.
const SWITCH: &str = "MGE_XE_CORE_FILES";

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
    /// The edited text of the package's file that the session uses ([`use_files`]).
    chosen: OnceLock<&'static str>,
}

/// A core file of `assets/Data Files/shaders/core`, built in, with the names of its parts.
macro_rules! core_file {
    ($name:literal, $parts:expr) => {
        CoreFile {
            name: $name,
            source: include_str!(concat!("../../../assets/Data Files/shaders/core/", $name)),
            parts: $parts,
            text: OnceLock::new(),
            chosen: OnceLock::new(),
        }
    };
}

/// `GrassInstVS` and `GrassPS` of `XE Mod Grass.fx`, with the looks of Remiros' Groundcover.
pub(crate) static GRASS: CoreFile = core_file!("XE Mod Grass.wgsl", &[]);

/// The built-in of the shadow hook `shadow_esm` (`XE Mod Shadow.fx`), and the shadow of the
/// cascades that each home of the shadow hooks gets (`cascades`, not a hook).
pub(crate) static SHADOW: CoreFile = core_file!("XE Mod Shadow.wgsl", &["shadow_esm", "cascades"]);

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

/// The surfaces of the water volumes (`XE Mod Water Volume.fx`): the entry points of a surface,
/// and the draws that mark the dry spaces.
pub(crate) static WATER_VOLUME: CoreFile = core_file!("XE Mod Water Volume.wgsl", &["volume", "dry"]);

/// The standard look of a water volume (`XE Mod Water Standard.fx`).
pub(crate) static WATER_STANDARD: CoreFile = core_file!("XE Mod Water Standard.wgsl", &["standard"]);

/// The caustics of the sea floor (`XE Mod Caustics.fx`).
pub(crate) static CAUSTICS: CoreFile = core_file!("XE Mod Caustics.wgsl", &["caustics"]);

/// The distant statics (`XE Mod Statics.fx`): the values of a set, and the statics in a view.
pub(crate) static STATICS: CoreFile = core_file!("XE Mod Statics.wgsl", &["set", "statics"]);

/// The distant land (`XE Mod Landscape.fx`).
pub(crate) static LANDSCAPE: CoreFile = core_file!("XE Mod Landscape.wgsl", &["landscape"]);

/// The sky (`XE Mod Sky.fx`): the sky dome, and the built-in clouds hook.
pub(crate) static SKY: CoreFile = core_file!("XE Mod Sky.wgsl", &["sky", "clouds_color"]);

/// The parts that the other core files share (`XE Common.fx`): the view group, and the fog.
pub(crate) static COMMON: CoreFile = core_file!("XE Common.wgsl", &["view", "fog"]);

/// Every core file.
static FILES: [&CoreFile; 12] = [
    &GRASS,
    &SHADOW,
    &SHADOWMAP,
    &WATER,
    &WATER_PASSES,
    &WATER_VOLUME,
    &WATER_STANDARD,
    &CAUSTICS,
    &STATICS,
    &LANDSCAPE,
    &SKY,
    &COMMON,
];

thread_local! {
    /// The edited text of a file under its check ([`with_candidate`]): the name of the file and
    /// the text. Only the thread of the check sees it.
    static CANDIDATE: RefCell<Option<(&'static str, &'static str)>> = const { RefCell::new(None) };
}

impl CoreFile {
    /// The whole text, with LF line ends: the candidate of a check, else the session's file,
    /// else the built-in.
    pub fn text(&self) -> &str {
        #[cfg(test)]
        note_read(self.name, "");
        let candidate = CANDIDATE.with(|candidate| *candidate.borrow());
        if let Some((_, text)) = candidate.filter(|(name, _)| *name == self.name) {
            return text;
        }
        if let Some(text) = self.chosen.get() {
            return text;
        }
        self.built_in()
    }

    /// The built-in text, with LF line ends.
    fn built_in(&self) -> &str {
        self.text.get_or_init(|| lf(self.source))
    }

    /// The text of the part `name`. A name that the file does not give is a fault of the
    /// build, which the tests find; an edited file gives every part of its list
    /// ([`parts_rule`]).
    pub fn part(&self, name: &str) -> &str {
        #[cfg(test)]
        note_read(self.name, name);
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

/// The file of a core file that a place of `Data Files` has: its place for the log, and its
/// bytes or why they cannot be read.
pub(crate) type Found = (String, Result<Vec<u8>, String>);

/// The check of an edited text of a core file in every shader that a core file is part of.
pub(crate) type Check<'a> = &'a dyn Fn(&CoreFile, &'static str) -> Result<(), String>;

/// What the session takes for a core file, and the log line that says why.
#[derive(Debug, PartialEq)]
pub(crate) struct Choice {
    /// The edited text of the package's file; `None` for the built-in, or for a file of this
    /// build with the built-in's text.
    pub edited: Option<&'static str>,
    pub line: String,
}

/// Chooses the text of each core file for the session, before the first pipeline, and prints
/// the line of each. `read` gives the file of a name in `Data Files/shaders/core`, `id` is the
/// build id of this helper, and `check` checks an edited text ([`with_candidate`]). The check of
/// a file has the edited files chosen before it. With `MGE_XE_CORE_FILES=0` no file is read.
pub(crate) fn use_files(read: &dyn Fn(&str) -> Option<Found>, id: &str, check: Check) {
    if nirender_wgpu::switches::on(SWITCH) == Some(false) {
        println!("core files: off ({SWITCH}=0): the built-ins are used");
        return;
    }
    for file in FILES {
        let choice = choose(file, read(file.name), id, check);
        println!("{}", choice.line);
        if let Some(text) = choice.edited {
            // The first choice stays: the session reads the files once.
            let _ = file.chosen.set(text);
        }
    }
}

/// The choice for `file` from `found`, its file in the package, for the helper of the build id
/// `id`. `check` runs only for a file of this build whose text is not the built-in's.
pub(crate) fn choose(file: &CoreFile, found: Option<Found>, id: &str, check: Check) -> Choice {
    let name = file.name;
    let built_in = |why: String| Choice {
        edited: None,
        line: format!("core file {name} {why}: the built-in is used"),
    };
    let Some((place, bytes)) = found else {
        return built_in("is missing".to_string());
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => return built_in(format!("({place}) cannot be read ({error})")),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return built_in(format!("({place}) is not UTF-8"));
    };
    let text = lf(&text);
    let (first, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let Some(file_id) = first.strip_prefix(BUILD_LINE).map(str::trim) else {
        return built_in(format!("({place}) has no build line"));
    };
    if !same_build(file_id, id) {
        return built_in(format!("({place}) is of build {file_id}, and this helper is {id}"));
    }
    if rest == file.built_in() {
        return Choice {
            edited: None,
            line: format!("core file {name} ({place}): the file, of this build"),
        };
    }
    if let Err(fault) = parts_rule(file, rest) {
        return built_in(format!("({place}) is edited, and {fault}"));
    }
    // The text must live as long as the session: a candidate that passes becomes its text.
    let rest: &'static str = Box::leak(rest.to_string().into_boxed_str());
    match check(file, rest) {
        Ok(()) => Choice {
            edited: Some(rest),
            line: format!("core file {name} ({place}): the file, of this build, edited: it is used"),
        },
        Err(error) => built_in(format!("({place}) is edited, and a shader fails with it: {error}")),
    }
}

/// Runs `run` with `text` as the text of `file` on this thread: the check of an edited file in
/// the shaders that it is part of. The other threads keep the session's texts.
pub(crate) fn with_candidate<T>(file: &CoreFile, text: &'static str, run: impl FnOnce() -> T) -> T {
    /// Clears the candidate, also when `run` panics.
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CANDIDATE.with(|candidate| candidate.borrow_mut().take());
        }
    }
    CANDIDATE.with(|candidate| *candidate.borrow_mut() = Some((file.name, text)));
    let _clear = Clear;
    run()
}

/// The two build ids are of one build: equal, without the mark `-dirty` of either.
fn same_build(a: &str, b: &str) -> bool {
    a.strip_suffix("-dirty").unwrap_or(a) == b.strip_suffix("-dirty").unwrap_or(b)
}

/// The rule of the parts of an edited file: the parts of the file's list, each once, and no
/// other part. A file that is one text has no marker line.
fn parts_rule(file: &CoreFile, text: &str) -> Result<(), String> {
    let parts = parts_of(text)?;
    let names: Vec<&str> = parts.iter().map(|(name, _)| *name).collect();
    if let Some(missing) = file.parts.iter().find(|part| !names.contains(part)) {
        return Err(format!("the part {missing} is missing"));
    }
    if let Some(other) = names.iter().find(|name| !file.parts.contains(name)) {
        return Err(format!("the part {other} is not one of the file's"));
    }
    Ok(())
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
thread_local! {
    /// The files and parts that this thread read while [`reads_during`] runs; "" names a whole
    /// text.
    static READS: RefCell<Option<Vec<(&'static str, String)>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn note_read(file: &'static str, part: &str) {
    READS.with(|reads| {
        if let Some(reads) = reads.borrow_mut().as_mut() {
            reads.push((file, part.to_string()));
        }
    });
}

/// The files and parts that `run` reads on this thread.
#[cfg(test)]
fn reads_during(run: impl FnOnce()) -> Vec<(&'static str, String)> {
    READS.with(|reads| *reads.borrow_mut() = Some(Vec::new()));
    run();
    READS.with(|reads| reads.borrow_mut().take().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{
        BUILD_LINE, CAUSTICS, COMMON, Choice, CoreFile, FILES, Found, GRASS, LANDSCAPE, SHADOW, SHADOWMAP, SKY, STATICS,
        WATER, WATER_PASSES, WATER_STANDARD, WATER_VOLUME, choose, lf, parts_of, reads_during, with_candidate,
    };
    use std::cell::Cell;

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
        assert_eq!(fnv(GRASS.text()), 0x7fd3_3a10_18c8_2c3a);
        assert_eq!(fnv(SHADOW.part("cascades")), 0xc7bd_ee3e_9854_62c5);
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
        assert_eq!(fnv(WATER_VOLUME.part("volume")), 0xd03b_f973_198b_6830);
        assert_eq!(fnv(WATER_VOLUME.part("dry")), 0x9952_e2ec_0483_7dec);
        assert_eq!(fnv(WATER_STANDARD.part("standard")), 0x383f_896b_efb8_a08b);
        assert_eq!(fnv(CAUSTICS.part("caustics")), 0x1d4b_2843_824d_eade);
        assert_eq!(fnv(STATICS.part("set")), 0xda48_a194_f44d_ea42);
        assert_eq!(fnv(STATICS.part("statics")), 0x69a9_edab_d438_69b5);
        assert_eq!(fnv(LANDSCAPE.part("landscape")), 0x0235_f973_da6d_5c95);
        assert_eq!(fnv(SKY.part("sky")), 0x2a1d_c750_6184_cb71);
        assert_eq!(fnv(SKY.part("clouds_color")), 0x39ba_a84c_0d98_ddff);
        assert_eq!(fnv(COMMON.part("view")), 0x3c1f_60c5_8e41_40d5);
        assert_eq!(fnv(COMMON.part("fog")), 0x8133_c9e5_a8f4_8522);
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

    /// Every part of every core file is in a shader of the check of an edited file
    /// (`core_file_compositions` of the renderer), and each of those shaders passes with the
    /// built-ins.
    #[test]
    fn each_part_is_in_a_checked_shader() {
        let reads = reads_during(|| {
            for (name, check) in crate::renderer::core_file_compositions() {
                check().unwrap_or_else(|error| panic!("{name}: {error}"));
            }
        });
        for file in FILES {
            let names: Vec<&str> = if file.parts.is_empty() {
                vec![""]
            } else {
                file.parts.to_vec()
            };
            for part in names {
                assert!(
                    reads.iter().any(|(read, name)| *read == file.name && name == part),
                    "{} {part:?} is in no checked shader",
                    file.name
                );
            }
        }
    }

    const ID: &str = "numidium v0.21.0 0123abc";

    /// The file of `file` in the package with the build line of `id` and `text`.
    fn package(id: &str, text: &str) -> Option<Found> {
        Some((
            "Data Files/shaders/core/x.wgsl".to_string(),
            Ok(format!("{BUILD_LINE}{id}\n{text}").into_bytes()),
        ))
    }

    /// The check of the renderer, and how many times it ran.
    fn checked(count: &Cell<u32>) -> impl Fn(&CoreFile, &'static str) -> Result<(), String> + '_ {
        move |file, text| {
            count.set(count.get() + 1);
            with_candidate(file, text, || {
                for (_, check) in crate::renderer::core_file_compositions() {
                    check()?;
                }
                Ok(())
            })
        }
    }

    /// The null: a stamped file of this build with the built-in's text runs no check, and the
    /// log says that the file is used. A CRLF copy and the id with `-dirty` are the same file.
    #[test]
    fn an_unedited_file_of_this_build_costs_no_check() {
        let count = Cell::new(0);
        for (id, text) in [
            (ID, LANDSCAPE.text().to_string()),
            ("numidium v0.21.0 0123abc-dirty", LANDSCAPE.text().replace('\n', "\r\n")),
        ] {
            let choice = choose(&LANDSCAPE, package(id, &text), ID, &checked(&count));
            assert_eq!(choice.edited, None);
            assert!(choice.line.ends_with("the file, of this build"), "{}", choice.line);
        }
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn a_valid_edited_file_is_used() {
        let count = Cell::new(0);
        let text = format!("{}\n// An edit.\nfn an_edit() -> f32 {{ return 1.0; }}\n", LANDSCAPE.text());
        let choice = choose(&LANDSCAPE, package(ID, &text), ID, &checked(&count));
        assert_eq!(choice.edited, Some(text.as_str()));
        assert!(choice.line.ends_with("edited: it is used"), "{}", choice.line);
        assert_eq!(count.get(), 1);
        // The edit is seen only on the thread of the check.
        assert_eq!(
            with_candidate(&LANDSCAPE, choice.edited.unwrap(), || LANDSCAPE.text().len()),
            text.len()
        );
        assert_eq!(LANDSCAPE.text(), LANDSCAPE.built_in());
    }

    #[test]
    fn an_edited_file_with_a_wgsl_fault_gives_the_built_in() {
        let count = Cell::new(0);
        let text = format!("{}\nfn broken( {{\n", LANDSCAPE.text());
        let choice = choose(&LANDSCAPE, package(ID, &text), ID, &checked(&count));
        assert_eq!(choice.edited, None);
        assert!(
            choice.line.contains("is edited, and a shader fails with it: distant land")
                && choice.line.ends_with("the built-in is used"),
            "{}",
            choice.line
        );
        assert_eq!(count.get(), 1);
    }

    /// A part that is missing, given twice or not of the list refuses the whole file, before
    /// any check.
    #[test]
    fn a_fault_of_the_parts_refuses_the_whole_file() {
        let count = Cell::new(0);
        let sky = SKY.text();
        let clouds = sky.find("//! part: clouds_color").unwrap();
        for (file, text, fault) in [
            (&SKY, sky[..clouds].to_string(), "the part clouds_color is missing"),
            (&SKY, format!("{sky}//! part: sky\n"), "the part sky is given twice"),
            (
                &SKY,
                format!("{sky}//! part: more\n"),
                "the part more is not one of the file's",
            ),
            (
                &GRASS,
                format!("//! part: grass\n{}", GRASS.text()),
                "the part grass is not one of the file's",
            ),
            (
                &GRASS,
                format!("{}//! part: grass\n", GRASS.text()),
                "text before the first part",
            ),
        ] {
            let choice = choose(file, package(ID, &text), ID, &checked(&count));
            assert_eq!(choice.edited, None);
            assert!(choice.line.contains(fault), "{}", choice.line);
        }
        assert_eq!(count.get(), 0);
    }

    /// A file that is missing, cannot be read, has no build line or is of another build gives the
    /// built-in, and the line says why; a file of another build names both ids.
    #[test]
    fn a_file_of_no_build_or_of_another_build_gives_the_built_in() {
        let count = Cell::new(0);
        let check = checked(&count);
        let text = LANDSCAPE.text();
        let choose = |found| choose(&LANDSCAPE, found, ID, &check);
        let built_in = |line: &str| Choice {
            edited: None,
            line: format!("core file XE Mod Landscape.wgsl {line}: the built-in is used"),
        };
        assert_eq!(choose(None), built_in("is missing"));
        assert_eq!(
            choose(Some(("p".to_string(), Err("denied".to_string())))),
            built_in("(p) cannot be read (denied)")
        );
        assert_eq!(
            choose(Some(("p".to_string(), Ok(text.as_bytes().to_vec())))),
            built_in("(p) has no build line")
        );
        assert_eq!(
            choose(package("numidium v0.20.0 0123abc", text)),
            built_in(
                "(Data Files/shaders/core/x.wgsl) is of build numidium v0.20.0 0123abc, and this helper is numidium v0.21.0 0123abc"
            )
        );
        assert_eq!(choose(package("numidium v0.21.0 4567def", "fn x() {}\n")).edited, None);
        assert_eq!(count.get(), 0);
    }
}
