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

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use glam::{Affine3A, Vec3};
use rayon::prelude::*;

use super::storage::authority::WriterSession;
use super::storage::durable::SyncClass;
use super::storage::state::{self, ArtifactKind, CommittedState, RequiredArtifact};
use super::{GenerationSettings, OutputPaths, PublicationWrites};
use crate::mge_xe::lights::{EXTERIOR_KEY, Key, Lamp, LampRecord, LightBlock, Lights, SmokeSource, key};
use crate::statics::{LampGlow, LampModel};
use crate::{UsageInfo, Vfs};

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
    /// The place, the turn and the scale of the reference: the light of a model is at a place
    /// in the model ([`read_lamp_models`]).
    frame: Affine3A,
}

/// The order of a block's lamps: by place (x, then y, then z), then by their values.
/// `f32::total_cmp` is a total order, so the file does not depend on the order of the
/// references.
fn lamp_order(a: &Lamp, b: &Lamp) -> Ordering {
    by_place(&a.position, &b.position)
        .then(a.radius.total_cmp(&b.radius))
        .then((a.color, a.flags, a.record).cmp(&(b.color, b.flags, b.record)))
}

fn by_place(a: &[f32; 3], b: &[f32; 3]) -> Ordering {
    a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2]))
}

/// The order of two references' transforms, for two lamps that are the same until their model
/// moves them.
fn frame_order(a: &Affine3A, b: &Affine3A) -> Ordering {
    let (a, b) = (a.to_cols_array(), b.to_cols_array());
    a.iter()
        .zip(&b)
        .map(|(a, b)| a.total_cmp(b))
        .find(|order| order.is_ne())
        .unwrap_or(Ordering::Equal)
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
    /// by their ids, the lamps and the smoke by their places. With the transform of each lamp's
    /// reference, in the order of the lamps.
    fn finish(mut self, key: Key) -> (LightBlock, Vec<Affine3A>) {
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

        let mut lamps: Vec<(Lamp, Affine3A)> = self
            .lamps
            .iter()
            .map(|placed| {
                let record = ids.binary_search(&&placed.id).expect("the id of a lamp is a record") as u32;
                (Lamp { record, ..placed.lamp }, placed.frame)
            })
            .collect();
        lamps.sort_by(|a, b| lamp_order(&a.0, &b.0).then_with(|| frame_order(&a.1, &b.1)));
        let (lamps, frames) = lamps.into_iter().unzip();
        self.smoke
            .sort_by(|a, b| by_place(&a.position, &b.position).then(a.strength.total_cmp(&b.strength)));

        let block = LightBlock {
            key,
            records,
            lamps,
            smoke: self.smoke,
        };
        (block, frames)
    }
}

/// Whether a model's path, normalised to lower case, is a smoke effect.
fn is_smoke_model(mesh: &str) -> bool {
    mesh.contains("smoke") && !mesh.contains("nosmoke")
}

/// The scales of the placed lamps with a model. The glow radius is of the model, so a lamp at
/// another scale than 1 has a glow radius of another size than its glowing parts.
#[derive(Default)]
struct LampScales {
    lamps: usize,
    scaled: usize,
    least: f32,
    most: f32,
}

impl LampScales {
    fn count(&mut self, scale: f32) {
        self.lamps += 1;
        if scale != 1.0 {
            if self.scaled == 0 {
                (self.least, self.most) = (scale, scale);
            }
            self.scaled += 1;
            self.least = self.least.min(scale);
            self.most = self.most.max(scale);
        }
    }

    fn log(&self) {
        if self.scaled == 0 {
            crate::logging::info!(
                "Distant lights: all {} lamps with a model are placed at the scale 1",
                self.lamps
            );
        } else {
            crate::logging::info!(
                "Distant lights: {} of {} lamps with a model are placed at another scale than 1, from {} to {}; their glow radius is that of the model",
                self.scaled,
                self.lamps,
                self.least,
                self.most
            );
        }
    }
}

/// The lamps and the smoke of the blocks, and the model of each light record that a lamp is.
#[derive(Default)]
pub(crate) struct LightParts {
    pub lights: Lights,
    /// The model of each lamp's record that has one, by the record's key. The glow radius of
    /// the record and the place of its light are read from it ([`read_lamp_models`]).
    pub models: BTreeMap<Key, String>,
    /// The transform of each lamp's reference: one list for each block of `lights`, in the
    /// order of the block's lamps.
    pub frames: Vec<Vec<Affine3A>>,
}

/// The lamps and the smoke sources of each block, before the references are remapped to meshes
/// (the remap drops the lights without a mesh). Blocks with neither are left out. A lamp is at
/// its reference's place, and the glow radius of each record is 0, until [`read_lamp_models`].
pub(crate) fn light_blocks(
    usage: &UsageInfo,
    include_behaves_like_exterior: bool,
    exclude_disable_targets: bool,
) -> LightParts {
    let encoder = bytes_io::Writer::default();
    let mut parts: BTreeMap<Key, BlockParts> = BTreeMap::new();
    let mut models: BTreeMap<Key, String> = BTreeMap::new();
    let mut scales = LampScales::default();
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
            if record.has_mesh
                && let Some(object) = object
            {
                models.entry(id.clone()).or_insert_with(|| object.mesh.to_owned());
                scales.count(reference.scale);
            }
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
                frame: reference.get_transform(),
            });
        }
    }

    scales.log();
    if lamps_without_key != 0 {
        crate::logging::warn!(
            "Distant lights: {lamps_without_key} lamps have a record id that the game cannot have; they are left out"
        );
    }

    let (blocks, frames) = parts
        .into_iter()
        .filter(|(_, block)| !block.lamps.is_empty() || !block.smoke.is_empty())
        .map(|(key, block)| block.finish(key))
        .unzip();
    LightParts {
        lights: Lights { blocks },
        models,
        frames,
    }
}

/// Reads the model of each record of `parts` that has one ([`crate::statics::lamp_model`]), one
/// time for each model. The record gets the radius of the model's drawn glowing parts. Each
/// lamp of a model with a light node moves to the place of its light: the node's place in the
/// model, through the transform of the lamp's reference, as the engine puts the light there.
/// The lamps of a block are then in the order of their places again. Logs the models by the
/// cause of a radius of 0, and the models with a light node. Answers the number of models read.
pub(crate) fn read_lamp_models(parts: &mut LightParts, vfs: &Vfs) -> usize {
    let unique: BTreeSet<&str> = parts.models.values().map(String::as_str).collect();
    let models: BTreeMap<&str, LampModel> = unique
        .par_iter()
        .map(|model| (*model, crate::statics::lamp_model(model, vfs)))
        .collect();
    let mut moved = 0usize;
    for (block, frames) in parts.lights.blocks.iter_mut().zip(&parts.frames) {
        let offsets: Vec<Option<Vec3>> = block
            .records
            .iter_mut()
            .map(|record| {
                let model = parts.models.get(&record.id).map(|model| models[model.as_str()]);
                if let Some(model) = model {
                    record.glow_radius = model.glow.radius();
                }
                model.and_then(|model| model.light_offset)
            })
            .collect();
        for (lamp, frame) in block.lamps.iter_mut().zip(frames) {
            if let Some(offset) = offsets[lamp.record as usize] {
                lamp.position = frame.transform_point3(offset).to_array();
                moved += 1;
            }
        }
        block.lamps.sort_by(lamp_order);
    }
    let glows: BTreeMap<&str, LampGlow> = models.iter().map(|(model, read)| (*model, read.glow)).collect();
    log_glow_causes(&glows);
    let with_node = models.values().filter(|read| read.light_offset.is_some()).count();
    crate::logging::info!(
        "Distant lights: {with_node} of {} lamp models have a light node; {moved} lamps are at their light node",
        models.len()
    );
    models.len()
}

/// The models by what their read found, so that a point of light over a lamp that shows as
/// itself can be explained. The models that were not read are named.
fn log_glow_causes(glows: &BTreeMap<&str, LampGlow>) {
    let count = |cause: fn(&LampGlow) -> bool| glows.values().filter(|glow| cause(glow)).count();
    let not_read: Vec<&str> = glows
        .iter()
        .filter(|(_, glow)| **glow == LampGlow::NotRead)
        .map(|(model, _)| *model)
        .collect();
    crate::logging::info!(
        "Distant lights: of {} lamp models, {} have a drawn glowing part, {} glow only under a billboard, {} have no glowing part and {} are not found or cannot be read{}",
        glows.len(),
        count(|glow| matches!(glow, LampGlow::Drawn(_))),
        count(|glow| *glow == LampGlow::UnderBillboard),
        count(|glow| *glow == LampGlow::NoGlowingPart),
        not_read.len(),
        if not_read.is_empty() {
            String::new()
        } else {
            format!(": {}", not_read.join(", "))
        }
    );
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
