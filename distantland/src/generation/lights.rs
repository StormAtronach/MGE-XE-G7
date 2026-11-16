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
//! A block's key and a lamp's record id are keys of the format ([`key`]), made from the plugin's
//! own bytes of the name ([`key_of_plugin_name`]). A name that the game cannot have gives no key:
//! that cell or that lamp is left out with a warning, and the rest of the file is written.
//!
//! A smoke source is a placed object whose model path has "smoke" and not "nosmoke" (strength
//! 1), or a lamp of a fire whose smoke is seen from far ([`LightRecord::smokes`], strength 0.5).
//!
//! The file is an entry of the committed state ([`ArtifactKind::Lights`]): it is current when
//! the state lists it with the length and digest of the bytes made now, or when the lights are off
//! and the state lists none. It is written in place inside the writer's dirty session, with the
//! rest of a publication or alone ([`publish_alone`]); the pruner removes it when a state does not
//! list it.
//!
//! [`ArtifactKind::Lights`]: ArtifactKind::Lights
//!
//! [`key`]: crate::mge_xe::lights::key
//!
//! [`LightRecord::lights`]: crate::LightRecord::lights
//! [`LightRecord::smokes`]: crate::LightRecord::smokes
//! [`ObjectDefinition::script_disables`]: crate::ObjectDefinition::script_disables

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::storage::authority::WriterSession;
use super::storage::durable::SyncClass;
use super::storage::state::{self, ArtifactKind, CommittedState, RequiredArtifact};
use super::{GenerationSettings, OutputPaths, PublicationWrites};
use crate::UsageInfo;
use crate::mge_xe::lights::{EXTERIOR_KEY, Key, Lamp, LampRecord, LightBlock, Lights, SmokeSource, key};

/// The key of the exterior's references in [`UsageInfo::cells`].
const EXTERIOR_CELL: &str = "\0";
/// The strength of the smoke of a smoke effect, and of a fire.
const EFFECT_SMOKE: f32 = 1.0;
const FIRE_SMOKE: f32 = 0.5;
/// The file's path in the inventory, relative to `distantland`.
const RELATIVE_PATH: &str = "lights.data";

/// The bytes of the file, and what they hold.
pub(crate) struct LightsOutput {
    pub bytes: Vec<u8>,
    /// The BLAKE3 of `bytes`, as the inventory records it.
    pub digest: [u8; 32],
    pub block_count: usize,
    pub lamp_count: usize,
    pub smoke_count: usize,
}

/// A lamp before its block's records are numbered.
struct PlacedLamp {
    id: Key,
    lamp: Lamp,
}

/// The key of a name that the plugin reader gave. The reader decodes the bytes of a name as
/// WINDOWS-1252, whatever the code page of the install is. That decode loses nothing, so the
/// encode gives the plugin's own bytes back, which are the bytes that the game holds. `None` for a
/// text with a zero in it or a text that is not such a decode: the game cannot have that name.
///
/// One limit: the reader's `load_string` calls `Encoding::decode`, which looks for a byte order
/// mark first. So a name that starts with the bytes EF BB BF, FF FE or FE FF is not decoded as
/// WINDOWS-1252, and it gets another key or none. It costs only that name.
fn key_of_plugin_name(encoder: &bytes_io::Writer, name: &str) -> Option<Key> {
    if name.contains('\0') {
        return None;
    }
    encoder.encode(name).ok().map(|bytes| key(&bytes))
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
    fn finish(mut self, key: Key) -> LightBlock {
        let mut ids: Vec<&Key> = self.lamps.iter().map(|placed| &placed.id).collect();
        ids.sort_unstable();
        ids.dedup();
        let records: Vec<LampRecord> = ids
            .iter()
            .map(|id| LampRecord {
                id: (*id).clone(),
                glow_radius: 0.0,
            })
            .collect();

        let mut lamps: Vec<Lamp> = self
            .lamps
            .iter()
            .map(|placed| Lamp {
                record: ids.binary_search(&&placed.id).expect("the id of a lamp is a record") as u32,
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
    let encoder = bytes_io::Writer::default();
    let mut parts: BTreeMap<Key, BlockParts> = BTreeMap::new();
    let mut lamps_without_key = 0usize;
    for (cell, references) in &usage.cells {
        let key = if cell == EXTERIOR_CELL {
            EXTERIOR_KEY
        } else if !cell.is_empty() && include_behaves_like_exterior && usage.behaves_like_exterior(cell) {
            // Two spellings that differ in the case of A to Z give one key, and so one block.
            let Some(key) = key_of_plugin_name(&encoder, cell) else {
                crate::logging::warn!(
                    "Distant lights: the cell '{}' has a name that the game cannot have; its lamps are left out",
                    cell.escape_debug()
                );
                continue;
            };
            key
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
            let Some(id) = key_of_plugin_name(&encoder, id) else {
                lamps_without_key += 1;
                continue;
            };
            block.lamps.push(PlacedLamp {
                id,
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

    if lamps_without_key != 0 {
        crate::logging::warn!(
            "Distant lights: {lamps_without_key} lamps have a record id that the game cannot have; they are left out"
        );
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
    let bytes = lights.to_bytes();
    Some(LightsOutput {
        digest: *blake3::hash(&bytes).as_bytes(),
        bytes,
        block_count: lights.blocks.len(),
        lamp_count: lights.blocks.iter().map(|block| block.lamps.len()).sum(),
        smoke_count: lights.blocks.iter().map(|block| block.smoke.len()).sum(),
    })
}

/// True when `committed` has the lights of `output`: it lists the file with the length and
/// digest of its bytes, or the distant lights are off and it lists none.
pub(crate) fn is_current(committed: Option<&CommittedState>, output: Option<&LightsOutput>) -> bool {
    let entry = committed.and_then(|committed| committed.artifact_by_kind(ArtifactKind::Lights));
    match (output, entry) {
        (Some(output), Some(entry)) => {
            entry.byte_length == output.bytes.len() as u64 && entry.content_blake3 == output.digest
        }
        (None, None) => true,
        _ => false,
    }
}

/// The inventory entry of the lights: the base's own when it is current, else the file written in
/// place now. `None` when the distant lights are off. Call it only inside the dirty session.
pub(crate) fn entry(
    writes: &PublicationWrites,
    paths: &OutputPaths,
    base: Option<&CommittedState>,
    output: Option<&LightsOutput>,
) -> Result<Option<RequiredArtifact>> {
    let Some(output) = output else {
        return Ok(None);
    };
    if is_current(base, Some(output)) {
        return Ok(base.and_then(|base| base.artifact_by_kind(ArtifactKind::Lights)).cloned());
    }
    let written = writes
        .write_durable(&paths.lights_path, &output.bytes, SyncClass::SmallArtifact)
        .with_context(|| format!("Failed to write {}", paths.lights_path.display()))?;
    Ok(Some(state::artifact_from_written(
        ArtifactKind::Lights,
        RELATIVE_PATH,
        written.byte_length,
        written.content_blake3,
    )))
}

/// Publishes a change of the lights alone, on a valid base: the state not valid, the one file
/// written in place, the state valid again with its new entry. The bytes are made before, so the
/// window holds only the write, its sync and the state's write. A cut in that window leaves the
/// state not valid, and the next start makes the output again whole.
pub(crate) fn publish_alone(writer: &mut WriterSession, output: Option<&LightsOutput>) -> Result<()> {
    // `begin_dirty` drops the writer's base, so it is taken first.
    let base = writer
        .base_state()
        .cloned()
        .context("a publication of the lights alone needs a valid base state")?;
    let paths = writer.paths().clone();
    let writes = PublicationWrites::new();
    writer.begin_dirty(output.map_or(0, |output| output.bytes.len() as u64))?;

    let mut artifacts: Vec<RequiredArtifact> = base
        .artifacts
        .iter()
        .filter(|entry| entry.kind != ArtifactKind::Lights)
        .cloned()
        .collect();
    artifacts.extend(entry(&writes, &paths, None, output)?);
    let committed = CommittedState::new(base.state_bytes.clone(), artifacts)?;
    writer.finish_publish(committed, &writes)?;
    writer
        .validate_published()
        .context("the lights publication failed its validation")?;
    Ok(())
}

#[cfg(test)]
mod tests;
