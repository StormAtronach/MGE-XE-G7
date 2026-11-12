//! The placed lights and the smoke sources of the distant land, by block:
//! `distantland\lights.data`. The layout is in `distantland_formats::lights`
//! (`crate::mge_xe::lights`), with its writer and the renderer's reader, `parse_lights`.
//!
//! The blocks are the exterior, and each interior that behaves like an exterior when the job
//! includes those. A lamp is a placed reference of a light record that lights
//! ([`LightRecord::lights`]): the reference is not deleted, and no script disables it
//! ([`ObjectDefinition::script_disables`], under the job's `exclude_script_disable_targets`,
//! as for the statics). Its radius, colour and flags are the plugins' values; the renderer takes
//! the game's own when the game gives them.
//!
//! A smoke source is a placed object whose model path has "smoke" and not "nosmoke" (strength
//! 1), or a lamp of a fire whose smoke is seen from far ([`LightRecord::smokes`], strength 0.5).
//!
//! The file is written apart from the committed inventory of the generation, when its bytes
//! change: it is made again at every generation run, which costs little.
//!
//! [`LightRecord::lights`]: crate::LightRecord::lights
//! [`LightRecord::smokes`]: crate::LightRecord::smokes
//! [`ObjectDefinition::script_disables`]: crate::ObjectDefinition::script_disables

use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result};

use super::{GenerationSettings, OutputPaths};
use crate::UsageInfo;
use crate::mge_xe::lights::{EXTERIOR_KEY, Lamp, LampRecord, LightBlock, Lights, SmokeSource};

/// The key of the exterior's references in [`UsageInfo::cells`].
const EXTERIOR_CELL: &str = "\0";
/// The strength of the smoke of a smoke effect, and of a fire.
const EFFECT_SMOKE: f32 = 1.0;
const FIRE_SMOKE: f32 = 0.5;

/// The bytes of the file, and what they hold.
pub(crate) struct LightsOutput {
    pub bytes: Vec<u8>,
    pub block_count: usize,
    pub lamp_count: usize,
    pub smoke_count: usize,
}

/// A lamp before its block's records are numbered.
struct PlacedLamp {
    id: String,
    lamp: Lamp,
}

/// The lamps and the smoke of one block, as the references give them.
#[derive(Default)]
struct BlockParts {
    lamps: Vec<PlacedLamp>,
    smoke: Vec<SmokeSource>,
}

impl BlockParts {
    /// The block, in an order that does not depend on the order of the references: the records
    /// by their ids, the lamps and the smoke by their places.
    fn finish(mut self, key: String) -> LightBlock {
        let mut ids: Vec<&str> = self.lamps.iter().map(|placed| placed.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        let records: Vec<LampRecord> = ids
            .iter()
            .map(|id| LampRecord {
                id: (*id).to_string(),
                glow_radius: 0.0,
            })
            .collect();

        let mut lamps: Vec<Lamp> = self
            .lamps
            .iter()
            .map(|placed| Lamp {
                record: ids.binary_search(&placed.id.as_str()).expect("the id of a lamp is a record") as u32,
                ..placed.lamp
            })
            .collect();
        // By place: x, then y, then z. f32::total_cmp is a total order, so the file does not
        // depend on the order of the references.
        let by_place =
            |a: &[f32; 3], b: &[f32; 3]| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2]));
        lamps.sort_by(|a, b| {
            by_place(&a.position, &b.position)
                .then(a.radius.total_cmp(&b.radius))
                .then((a.color, a.flags, a.record).cmp(&(b.color, b.flags, b.record)))
        });
        self.smoke
            .sort_by(|a, b| by_place(&a.position, &b.position).then(a.strength.total_cmp(&b.strength)));

        LightBlock {
            key,
            records,
            lamps,
            smoke: self.smoke,
        }
    }
}

/// Whether a model's path, normalised to lower case, is a smoke effect.
fn is_smoke_model(mesh: &str) -> bool {
    mesh.contains("smoke") && !mesh.contains("nosmoke")
}

/// The lamps and the smoke sources of each block, before the references are remapped to meshes
/// (the remap drops the lights without a mesh). Blocks with neither are left out.
pub(crate) fn light_blocks(usage: &UsageInfo, include_behaves_like_exterior: bool, exclude_disable_targets: bool) -> Lights {
    let mut parts: BTreeMap<String, BlockParts> = BTreeMap::new();
    for (cell, references) in &usage.cells {
        let key = if cell == EXTERIOR_CELL {
            EXTERIOR_KEY.to_string()
        } else if !cell.is_empty() && include_behaves_like_exterior && usage.behaves_like_exterior(cell) {
            // The game finds a cell by its name in any case: two spellings are one block.
            cell.to_ascii_lowercase()
        } else {
            continue;
        };
        let block = parts.entry(key).or_default();

        for reference in references.values().filter(|reference| !reference.deleted) {
            let id = reference.id.as_ref();
            let object = usage.objects.get(id);
            if object.is_some_and(|object| object.script_disables(reference, exclude_disable_targets)) {
                continue;
            }
            let position = reference.translation.to_array();
            if object.is_some_and(|object| is_smoke_model(object.mesh)) {
                block.smoke.push(SmokeSource {
                    position,
                    strength: EFFECT_SMOKE,
                });
            }

            let Some(record) = usage.lights.get(id).filter(|record| record.lights()) else {
                continue;
            };
            if record.smokes() {
                block.smoke.push(SmokeSource {
                    position,
                    strength: FIRE_SMOKE,
                });
            }
            block.lamps.push(PlacedLamp {
                id: id.to_ascii_lowercase(),
                lamp: Lamp {
                    position,
                    radius: record.radius,
                    color: record.color,
                    has_mesh: record.has_mesh,
                    flags: record.flags,
                    record: 0,
                },
            });
        }
    }

    let blocks = parts
        .into_iter()
        .filter(|(_, block)| !block.lamps.is_empty() || !block.smoke.is_empty())
        .map(|(key, block)| block.finish(key))
        .collect();
    Lights { blocks }
}

/// The bytes of `lights.data` for `lights` (from [`light_blocks`]), or `None` when the distant
/// lights are off.
pub(crate) fn build(lights: &Lights, settings: &GenerationSettings) -> Option<LightsOutput> {
    if !settings.distant_lights {
        return None;
    }
    Some(LightsOutput {
        bytes: lights.to_bytes(),
        block_count: lights.blocks.len(),
        lamp_count: lights.blocks.iter().map(|block| block.lamps.len()).sum(),
        smoke_count: lights.blocks.iter().map(|block| block.smoke.len()).sum(),
    })
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
            fs::rename(&temporary, &paths.lights_path)
                .with_context(|| format!("Failed to replace {}", paths.lights_path.display()))?;
        }
        None => {
            fs::remove_file(&paths.lights_path)
                .with_context(|| format!("Failed to remove {}", paths.lights_path.display()))?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
