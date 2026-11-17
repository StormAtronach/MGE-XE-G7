//! The key of a distant land output: its format and the identity of its generation settings.
//!
//! Two builds whose format texts are equal read and keep each other's output. A test tool keeps
//! one distant land for each key (W521), so a build of another format never rebuilds the
//! distant land of another build. [`output_format`] is the one home of the format text.

use std::path::Path;

use anyhow::Result;

use crate::generation::job::load_generation_job_file;
use crate::generation::unit_fingerprint::fingerprint_settings;
use distantland_formats::distant_statics::STATIC_MESHES_VERSION;
use distantland_formats::distant_terrain::TERRAIN_FILE_VERSION;
use distantland_formats::lights::LIGHTS_VERSION;
use distantland_formats::terrain_occlusion::TERRAIN_OCCLUSION_VERSION;
use distantland_foundation::output::MGE_DL_VERSION;
use distantland_foundation::state_db::state_format_version;
use distantland_foundation::storage::state::{ArtifactKind, COMMITTED_STATE_VERSION};
use distantland_foundation::units::{STATICS_RECIPE_VERSION, TERRAIN_RECIPE_VERSION};
use distantland_statics::atlas::atlas_cache_version;

/// The numbers that decide whether a build reads another build's output, by name. A constant
/// whose name ends in `VERSION` is in this list or in [`NOT_FORMAT`] (a test scans the sources).
fn format_numbers() -> [(&'static str, u32); 10] {
    [
        ("MGE_DL_VERSION", u32::from(MGE_DL_VERSION)),
        ("STATE_FORMAT_VERSION", state_format_version()),
        ("COMMITTED_STATE_VERSION", COMMITTED_STATE_VERSION),
        ("LIGHTS_VERSION", LIGHTS_VERSION),
        ("STATIC_MESHES_VERSION", STATIC_MESHES_VERSION),
        ("TERRAIN_FILE_VERSION", TERRAIN_FILE_VERSION),
        ("TERRAIN_OCCLUSION_VERSION", TERRAIN_OCCLUSION_VERSION),
        ("STATICS_RECIPE_VERSION", STATICS_RECIPE_VERSION),
        ("TERRAIN_RECIPE_VERSION", TERRAIN_RECIPE_VERSION),
        ("ATLAS_CACHE_VERSION", atlas_cache_version()),
    ]
}

/// The constants named `...VERSION` that are not a part of the format, each with its reason.
/// Only the test of the sources reads it.
#[cfg(test)]
const NOT_FORMAT: [(&str, &str); 3] = [
    (
        "GENERATION_REPORT_VERSION",
        "generation_report.toml is advisory: the output stays valid when it does not decode (output/manifest.rs)",
    ),
    (
        "GENERATION_JOB_FILE_VERSION",
        "the job file (mgeXE.toml) is an input; another version only gives a warning (job/src/document.rs)",
    ),
    (
        "WATER_RULES_VERSION",
        "the highest version of a mod's water rules that the generator reads: an input, not the output",
    ),
];

/// The format text: "name=value" for each number of [`format_numbers`] and each kind of the
/// committed state by name and number, in a fixed order, joined by ";".
pub fn output_format() -> String {
    let numbers = format_numbers().into_iter().map(|(name, value)| format!("{name}={value}"));
    let kinds = ArtifactKind::ALL
        .into_iter()
        .map(|kind| format!("ArtifactKind.{}={}", kind.name(), kind as u8));
    numbers.chain(kinds).collect::<Vec<_>>().join(";")
}

/// The identity of the generation settings of the job file at `path` (an `mgeXE.toml`), as the
/// generator makes it for its own check, in hex. It reads only that file: no game, no plugin list.
///
/// # Errors
///
/// Returns an error if the file cannot be read or is not valid TOML.
pub fn settings_identity(path: &Path) -> Result<String> {
    let job = load_generation_job_file(path)?;
    Ok(blake3::Hash::from(fingerprint_settings(&job.settings)).to_hex().to_string())
}

/// The first 8 hex digits of the blake3 of `text`: the short name of a key's part.
pub fn short_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    /// The `const NAME...VERSION:` names of a source text.
    fn version_constants(source: &str) -> Vec<String> {
        source
            .lines()
            .filter_map(|line| {
                let after = &line[line.find("const ")? + "const ".len()..];
                let name = after[..after.find(':')?].trim();
                (name.ends_with("VERSION") && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                    .then(|| name.to_string())
            })
            .collect()
    }

    /// The constants of `names` that are neither in the format nor in [`NOT_FORMAT`].
    fn unlisted(names: &[String]) -> Vec<String> {
        let listed: Vec<&str> = format_numbers()
            .iter()
            .map(|(name, _)| *name)
            .chain(NOT_FORMAT.iter().map(|(name, _)| *name))
            .collect();
        names
            .iter()
            .filter(|name| !listed.contains(&name.as_str()))
            .cloned()
            .collect()
    }

    /// The `.rs` files of the crate's sources and of its crates' sources, without test files.
    fn source_files() -> Vec<PathBuf> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut folders = vec![root.join("src")];
        for entry in fs::read_dir(root.join("crates")).unwrap() {
            folders.push(entry.unwrap().path().join("src"));
        }
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            let Ok(entries) = fs::read_dir(&folder) else { continue };
            for entry in entries {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "tests" {
                        folders.push(path);
                    }
                } else if name.ends_with(".rs") && name != "tests.rs" {
                    files.push(path);
                }
            }
        }
        files
    }

    // T-R1a: the text holds each number by its name and value, and each kind by its name and
    // number. The null: the same check on a text with one number taken out fails.
    #[test]
    fn the_format_text_names_every_number_and_every_kind() {
        let text = output_format();
        let parts: Vec<&str> = text.split(';').collect();
        for (name, value) in format_numbers() {
            assert!(parts.contains(&format!("{name}={value}").as_str()), "{name} is not in {text}");
        }
        for kind in ArtifactKind::ALL {
            assert!(
                parts.contains(&format!("ArtifactKind.{}={}", kind.name(), kind as u8).as_str()),
                "{kind:?} is not in {text}"
            );
        }
        assert_eq!(parts.len(), format_numbers().len() + ArtifactKind::ALL.len());
        assert!(
            parts.contains(&"MGE_DL_VERSION=22") && parts.contains(&"ArtifactKind.Lights=8"),
            "{text}"
        );

        let short: Vec<&str> = parts
            .iter()
            .copied()
            .filter(|part| !part.starts_with("LIGHTS_VERSION="))
            .collect();
        assert!(
            !short.contains(&format!("LIGHTS_VERSION={LIGHTS_VERSION}").as_str()),
            "the null must miss the number"
        );
        assert_ne!(
            short_hash(&short.join(";")),
            short_hash(&text),
            "a text without a number must have another name"
        );
    }

    // T-R1b: a key outside the generation settings leaves the identity as it is; a generation
    // setting changes it. The second case is the null: it shows that the test sees a change.
    #[test]
    fn the_settings_identity_follows_the_generation_settings_only() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.path().join(name);
            fs::write(&path, text).unwrap();
            path
        };
        let base = "[generation]\nversion = 3\nplugins = [\"Morrowind.esm\"]\n[generation.settings]\ngrass_density = 0.5\n";
        let plain = write("plain.toml", base);
        let views = write("views.toml", &format!("[views]\nfull_tier_max = 4\n\n{base}"));
        let grass = write("grass.toml", &base.replace("grass_density = 0.5", "grass_density = 0.25"));

        let identity = settings_identity(&plain).unwrap();
        assert_eq!(identity.len(), 64);
        assert_eq!(settings_identity(&views).unwrap(), identity);
        assert_ne!(settings_identity(&grass).unwrap(), identity);
        assert!(settings_identity(&dir.path().join("missing.toml")).is_err());
    }

    // T-R1c: every constant of the sources whose name ends in VERSION is in the format or in
    // NOT_FORMAT. The null: a planted constant that is in neither list is found.
    #[test]
    fn every_version_constant_is_in_a_list() {
        let files = source_files();
        assert!(files.len() > 50, "the scan found {} files", files.len());
        let mut found = Vec::new();
        for file in &files {
            found.extend(version_constants(&fs::read_to_string(file).unwrap()));
        }
        for (name, _) in format_numbers() {
            assert!(found.iter().any(|seen| seen == name), "the scan did not see {name}");
        }
        assert_eq!(unlisted(&found), Vec::<String>::new());

        // The planted line is made at run time, so that the scan of this file does not find it.
        let planted = version_constants(&format!(
            "pub const {}: u32 = 1;\nconst LIGHTS_VERSION: u32 = 3;\n",
            "PLANTED_VERSION"
        ));
        assert_eq!(unlisted(&planted), vec!["PLANTED_VERSION".to_string()]);
    }

    // The kinds of ArtifactKind::ALL are the kinds of the enum's source, by name and number.
    // The null: a planted enum with one more kind differs.
    #[test]
    fn the_kind_list_is_the_enum() {
        fn kinds_of(source: &str) -> Vec<(String, u8)> {
            let body = &source[source.find("pub enum ArtifactKind {").unwrap()..];
            let body = &body[..body.find("\n}").unwrap()];
            body.lines()
                .filter_map(|line| {
                    let (name, number) = line.trim().trim_end_matches(',').split_once(" = ")?;
                    Some((name.to_string(), number.parse().ok()?))
                })
                .collect()
        }
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/foundation/src/storage/state.rs");
        let listed: Vec<(String, u8)> = ArtifactKind::ALL
            .iter()
            .map(|kind| (kind.name().to_string(), *kind as u8))
            .collect();
        assert_eq!(kinds_of(&fs::read_to_string(path).unwrap().replace("\r\n", "\n")), listed);

        let planted = "pub enum ArtifactKind {\n    Version = 1,\n    Usage = 2,\n}\n";
        assert_ne!(kinds_of(planted), listed);
    }
}
