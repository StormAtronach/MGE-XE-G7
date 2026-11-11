use std::sync::{Arc, Mutex};

use super::parse::{parse_ranges, parse_static_keywords, strip_comment, unescape};
use super::{OverridesBuilder, ReportLevel, SourceKind, WaterNames};
use crate::mge_xe::distant_statics::StaticType;

#[test]
fn test_parse_static_keywords() {
    let ov = parse_static_keywords(b"far no_script");
    assert!(matches!(ov.static_type, StaticType::StaticFar));
    assert!(ov.no_script);
    assert!(!ov.ignore);
}

#[test]
fn override_file_identity_matches_parsed_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.ovr");
    let bytes = b"far no_script = meshes\\fixture.nif\n";
    std::fs::write(&path, bytes).unwrap();

    let mut builder = OverridesBuilder::new();
    let identity = builder.add_override_file_with_identity(&path).unwrap();

    assert_eq!(identity.path, path);
    assert_eq!(
        identity.content,
        distantland_foundation::identity::ContentIdentity::from_bytes(bytes)
    );
}

#[test]
fn test_parse_static_keywords_grass_density() {
    let ov = parse_static_keywords(b"grass_50");
    assert!(matches!(ov.static_type, StaticType::StaticGrass));
    assert!((ov.density - 0.5).abs() < f32::EPSILON);
}

#[test]
fn test_parse_static_keywords_reduction() {
    let ov = parse_static_keywords(b"very_far reduction_50");
    assert!(matches!(ov.static_type, StaticType::StaticVeryFar));
    assert_eq!(ov.simplify, Some(0.5));
}

#[test]
fn test_parse_static_keywords_building() {
    let ov = parse_static_keywords(b"building far");
    assert!(matches!(ov.static_type, StaticType::StaticFar));
}

#[test]
fn test_parse_static_keywords_building_last_token_wins() {
    let ov = parse_static_keywords(b"far building");
    assert!(matches!(ov.static_type, StaticType::StaticBuilding));
}

#[test]
fn test_strip_comment_simple() {
    assert_eq!(strip_comment(b"foo=bar : comment", false), b"foo=bar ");
    assert_eq!(strip_comment(b": full comment", false), b"");
    assert_eq!(strip_comment(b"no comment here", false), b"no comment here");
}

#[test]
fn test_strip_comment_escaped() {
    assert_eq!(strip_comment(b"foo\\:bar : comment", true), b"foo\\:bar ");
    assert_eq!(strip_comment(b"foo\\:bar\\:baz", true), b"foo\\:bar\\:baz");
}

#[test]
fn test_unescape() {
    assert_eq!(unescape(b"foo\\:bar"), "foo:bar");
    assert_eq!(unescape(b"foo\\\\bar"), "foo\\bar");
    assert_eq!(unescape(b"plain"), "plain");
}

#[test]
fn test_parse_ranges() {
    let tokens: Vec<&[u8]> = vec![b"0-20", b"25", b"50-100"];
    let ranges = parse_ranges(&tokens);
    assert_eq!(ranges.as_slice(), &[(0, 21), (25, 26), (50, 101)]);
}

#[test]
fn test_parse_ranges_max_8() {
    let tokens: Vec<&[u8]> = vec![b"1", b"2", b"3", b"4", b"5", b"6", b"7", b"8", b"9", b"10"];
    let ranges = parse_ranges(&tokens);
    assert_eq!(ranges.len(), 8);
}

/// Collects the messages of the warnings that are logged while it is the subscriber.
struct WarningLog(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for WarningLog {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event() && *metadata.level() == tracing::Level::WARN
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Message<'a>(&'a mut Vec<String>);
        impl tracing::field::Visit for Message<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0.push(format!("{value:?}"));
                }
            }
        }
        event.record(&mut Message(&mut self.0.lock().unwrap()));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

#[test]
fn later_water_names_replace_the_table_and_warn_when_they_differ() {
    let names = |surface: &str| WaterNames {
        surface: vec![surface.to_owned()],
        body: vec!["waterbody".to_owned()],
        mask: Vec::new(),
        ..WaterNames::default()
    };
    let warnings = Arc::new(Mutex::new(Vec::new()));

    let overrides = tracing::subscriber::with_default(WarningLog(warnings.clone()), || {
        let mut builder = OverridesBuilder::new();
        builder.begin_source("default.toml");
        builder.set_water_names(names("watervolume"));
        // The same names from another source are not a conflict.
        builder.begin_source("Same-metadata.toml");
        builder.set_water_names(names("watervolume"));
        assert!(warnings.lock().unwrap().is_empty());

        builder.begin_source("Other-metadata.toml");
        builder.set_water_names(names("pond"));
        builder.finish()
    });

    // The later table replaces the earlier one whole.
    assert_eq!(overrides.water_names, names("pond"));
    assert_eq!(
        *warnings.lock().unwrap(),
        ["Water names from Other-metadata.toml replace conflicting names from Same-metadata.toml"]
    );
}

/// A builder with a global list (a matched very far key, a matched key of `reduction` only, an
/// unmatched key) and a plugin's metadata (a matched near key that replaces the list's type of
/// the same mesh, an unmatched key).
fn report_builder() -> OverridesBuilder {
    let mut builder = OverridesBuilder::new();
    builder.begin_source("default.toml");
    builder.insert_mesh_override("pc\\x\\bridge_bot.nif".to_owned(), parse_static_keywords(b"very_far"));
    builder.insert_mesh_override("x\\rock.nif".to_owned(), parse_static_keywords(b"reduction_50"));
    builder.insert_mesh_override("x\\absent_in_list.nif".to_owned(), parse_static_keywords(b"far"));
    builder.insert_mesh_override("x\\tower.nif".to_owned(), parse_static_keywords(b"far"));
    builder.begin_plugin_source("Mod-metadata.toml");
    builder.insert_mesh_override("x\\tower.nif".to_owned(), parse_static_keywords(b"near"));
    builder.insert_mesh_override("x\\absent_in_mod.nif".to_owned(), parse_static_keywords(b"ignore"));
    builder
}

fn report_meshes() -> hashbrown::HashSet<&'static str> {
    ["pc\\x\\bridge_bot.nif", "x\\rock.nif", "x\\tower.nif", "x\\other.nif"].into_iter().collect()
}

/// Each source has its count of keys and of keys that match; a used key that states a tier
/// names its type and the source that won; `reduction` alone is no statement.
#[test]
fn the_report_names_each_used_key_with_its_type_and_its_source() {
    let (overrides, report) = report_builder().finish_with_report();
    let lines = report.lines(&overrides, &report_meshes());
    let infos: Vec<&str> = lines.iter().filter(|(level, _)| *level == ReportLevel::Info).map(|(_, line)| line.as_str()).collect();
    assert!(infos.contains(&"Mesh overrides of default.toml: 4 keys, 3 match a mesh of this load order"), "{infos:?}");
    assert!(infos.contains(&"Mesh overrides of Mod-metadata.toml: 2 keys, 1 match a mesh of this load order"), "{infos:?}");
    assert!(infos.contains(&"Mesh override pc\\x\\bridge_bot.nif: very_far, from default.toml"), "{infos:?}");
    assert!(infos.contains(&"Mesh override x\\tower.nif: near, from Mod-metadata.toml"), "{infos:?}");
    assert!(!infos.iter().any(|line| line.starts_with("Mesh override x\\rock.nif")), "{infos:?}");
    assert!(infos.contains(&"Mesh overrides in use: near 1, very_far 1"), "{infos:?}");
}

/// A key that matches nothing: a note for a global list, a warning for a plugin's metadata.
#[test]
fn an_unmatched_key_is_a_note_in_a_list_and_a_warning_in_a_plugin() {
    let (overrides, report) = report_builder().finish_with_report();
    let lines = report.lines(&overrides, &report_meshes());
    assert!(lines.contains(&(
        ReportLevel::Info,
        "Note: mesh override x\\absent_in_list.nif of default.toml matches no mesh of this load order".to_owned()
    )));
    assert!(lines.contains(&(
        ReportLevel::Warning,
        "Mesh override x\\absent_in_mod.nif of Mod-metadata.toml matches no mesh of this load order: the plugin names a mesh it does not use".to_owned()
    )));
    assert_eq!(lines.iter().filter(|(level, _)| *level == ReportLevel::Warning).count(), 1, "{lines:?}");
}

/// A key that a source gives twice counts once, and the later value wins.
#[test]
fn the_report_counts_a_repeated_key_once() {
    let mut builder = OverridesBuilder::new();
    builder.begin_source("list.toml");
    builder.insert_mesh_override("x\\a.nif".to_owned(), parse_static_keywords(b"far"));
    builder.insert_mesh_override("x\\a.nif".to_owned(), parse_static_keywords(b"very_far"));
    let (overrides, report) = builder.finish_with_report();
    assert_eq!(report.sources.len(), 1);
    assert_eq!(report.sources[0].mesh_keys, ["x\\a.nif"]);
    assert_eq!(report.sources[0].kind, SourceKind::GlobalList);
    assert_eq!(overrides.mesh_overrides["x\\a.nif"].static_type, StaticType::StaticVeryFar);
}
