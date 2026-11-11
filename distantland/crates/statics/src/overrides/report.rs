//! The log of the mesh overrides of a run: which keys of which source matched a mesh of the load
//! order, the type and the source that won for each used key, and the keys that matched nothing.
//!
//! A key of a plugin's metadata that matches nothing is a warning: the mod names a mesh that it
//! does not have. A key of a global list that matches nothing is a note: a global list names the
//! meshes of mods that may be absent.

use hashbrown::{HashMap, HashSet};
use tracing::{info, warn};

use crate::mge_xe::distant_statics::StaticType;
use crate::usage::{StaticOverride, StaticOverrides};

/// Where a source of overrides comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A list that the settings name, the `.ovr` files of the `mge3` folder, or override text.
    GlobalList,
    /// The `-metadata.toml` of an active plugin.
    PluginMetadata,
}

/// One source of overrides and the mesh keys it gave, in its order.
#[derive(Clone, Debug)]
pub struct ReportSource {
    pub label: String,
    pub kind: SourceKind,
    pub mesh_keys: Vec<String>,
}

/// The sources of a merge and the source that won for each mesh key.
#[derive(Clone, Debug, Default)]
pub struct OverrideReport {
    pub sources: Vec<ReportSource>,
    /// The index in `sources` of the source that won, for each mesh key.
    pub winners: HashMap<String, usize>,
}

/// The level of a line of the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportLevel {
    Info,
    Warning,
}

impl OverrideReport {
    /// The lines of the report against the meshes of the objects of this load order (the keys of
    /// both are normalized the same way). Pure, for the tests; [`OverrideReport::write_log`] writes
    /// them.
    pub fn lines(&self, overrides: &StaticOverrides, meshes: &HashSet<&str>) -> Vec<(ReportLevel, String)> {
        let mut lines = Vec::new();
        for source in &self.sources {
            if source.mesh_keys.is_empty() {
                continue;
            }
            let matched = source.mesh_keys.iter().filter(|key| meshes.contains(key.as_str())).count();
            lines.push((
                ReportLevel::Info,
                format!(
                    "Mesh overrides of {}: {} keys, {matched} match a mesh of this load order",
                    source.label,
                    source.mesh_keys.len()
                ),
            ));
        }

        // The used keys that set a tier or leave the mesh out, by key for a stable order.
        let mut used: Vec<(&String, &StaticOverride, usize)> = overrides
            .mesh_overrides
            .iter()
            .filter(|(key, _)| meshes.contains(key.as_str()))
            .filter_map(|(key, value)| self.winners.get(key).map(|&source| (key, value, source)))
            .filter(|(_, value, _)| value.ignore || value.static_type != StaticType::StaticAuto)
            .collect();
        used.sort_by(|a, b| a.0.cmp(b.0));
        let mut by_type: Vec<(&str, usize)> = Vec::new();
        for (key, value, source) in &used {
            let what = statement(value);
            lines.push((
                ReportLevel::Info,
                format!("Mesh override {key}: {what}, from {}", self.sources[*source].label),
            ));
            match by_type.iter_mut().find(|(name, _)| *name == what) {
                Some((_, count)) => *count += 1,
                None => by_type.push((what, 1)),
            }
        }
        if !by_type.is_empty() {
            by_type.sort();
            let counts: Vec<String> = by_type.iter().map(|(name, count)| format!("{name} {count}")).collect();
            lines.push((ReportLevel::Info, format!("Mesh overrides in use: {}", counts.join(", "))));
        }

        for source in &self.sources {
            for key in source.mesh_keys.iter().filter(|key| !meshes.contains(key.as_str())) {
                lines.push(match source.kind {
                    SourceKind::PluginMetadata => (
                        ReportLevel::Warning,
                        format!("Mesh override {key} of {} matches no mesh of this load order: the plugin names a mesh it does not use", source.label),
                    ),
                    SourceKind::GlobalList => (
                        ReportLevel::Info,
                        format!("Note: mesh override {key} of {} matches no mesh of this load order", source.label),
                    ),
                });
            }
        }
        lines
    }

    /// Writes the lines of [`OverrideReport::lines`] into the generator's log.
    pub fn write_log(lines: &[(ReportLevel, String)]) {
        for (level, line) in lines {
            match level {
                ReportLevel::Info => info!("{line}"),
                ReportLevel::Warning => warn!("{line}"),
            }
        }
    }
}

/// What an override states about its mesh: `ignore`, or the type as the lists write it.
fn statement(value: &StaticOverride) -> &'static str {
    if value.ignore {
        return "ignore";
    }
    match value.static_type {
        StaticType::StaticAuto => "auto",
        StaticType::StaticNear => "near",
        StaticType::StaticFar => "far",
        StaticType::StaticVeryFar => "very_far",
        StaticType::StaticGrass => "grass",
        StaticType::StaticTree => "tree",
        StaticType::StaticBuilding => "building",
    }
}
