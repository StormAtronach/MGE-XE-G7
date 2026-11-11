//! Lists the tier that the helper gives to the distant statics whose record key holds a text.
//!
//! The tool reads the distant land of one generation with the generator's own readers:
//! `statics/usage.data`, `statics/static_meshes_*` and `generation_state.bin`. For each static
//! whose record key holds one of the texts (no case), it prints the type, the bounding radius,
//! the placements with their scales, and the tiers of the placements. The tier follows the
//! helper's rule (mgeHost64 renderer/statics.rs, `tier_of`): near, far and very far are fixed
//! tiers; another type goes by the radius times the scale against the two limits, and a
//! building's radius counts two times.
//!
//! A mesh can also be part of the merged statics of a cell. For each merge cell that holds a
//! mesh with one of the texts, the tool prints the cell, the members that hold the text, and the
//! count of the cell's components in each tier. A component record has no mesh name.
//!
//! With `--mesh`, the tool builds each named mesh with the generator's own extraction from the
//! data folders and the archives of a `Morrowind.ini`, without the overrides, and prints its
//! type, its radius and its tier at scale 1. The radius is the one that a merge component holds,
//! before the scale of the placement.
//!
//! Run: `cargo run --release -p distantland --example static_tier_lister -- <distantland dir>
//! --match <text> [--match <text>]... [--far 600] [--very-far 800]`, or
//! `static_tier_lister --ini <Morrowind.ini> --data <dir> [--data <dir>]... --mesh <path>...`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use distantland_formats::distant_statics::{PackedDistantStatic, load_static_meshes};
use distantland_formats::usage_data::load_usage_data;
use distantland_foundation::output::STATIC_MESH_SHARD_COUNT;
use distantland_foundation::record_key::StaticRecordKey;
use distantland_foundation::state_db::{self, LoadedGenerationState};
use distantland_foundation::storage::state as committed_state;
use distantland_statics::{DistantStatic, OverridesBuilder};
use distantland_vfs::{Vfs, VfsLoadOptions};

/// The defaults of `far_static_min_size` and `very_far_static_min_size`.
const FAR_MIN_SIZE: f32 = 600.0;
const VERY_FAR_MIN_SIZE: f32 = 800.0;

const TIER_NAMES: [&str; 3] = ["near", "far", "very far"];

struct Options {
    folder: Option<PathBuf>,
    texts: Vec<String>,
    ini: Option<PathBuf>,
    data: Vec<PathBuf>,
    meshes: Vec<String>,
    far_min_size: f32,
    very_far_min_size: f32,
}

impl Options {
    fn matches(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        self.texts.iter().any(|text| name.contains(text.as_str()))
    }

    /// The helper's tier of a model of this type and this radius; None for grass.
    fn tier_of(&self, kind: u32, radius: f32) -> Option<usize> {
        match kind {
            1 => Some(0),
            2 => Some(1),
            3 => Some(2),
            4 => None,
            _ => {
                let radius = if kind == 6 { radius * 2.0 } else { radius };
                Some(if radius <= self.far_min_size {
                    0
                } else if radius <= self.very_far_min_size {
                    1
                } else {
                    2
                })
            }
        }
    }
}

fn main() -> Result<()> {
    let options = parse_options()?;
    if !options.meshes.is_empty() {
        return print_meshes(&options);
    }
    let folder = options.folder.as_ref().context("give the distant land folder")?;
    let statics_folder = folder.join("statics");

    let usage = load_usage_data(&statics_folder.join("usage.data")).context("usage.data")?;
    let statics = load_statics(&statics_folder)?;
    let state = load_state(folder)?;
    let keys = record_keys(&state);
    if keys.len() != statics.len() {
        bail!(
            "generation_state.bin names {} records, the shards hold {} statics",
            keys.len(),
            statics.len()
        );
    }
    println!(
        "distant land {}: {} statics, {} exterior placements, far limit {}, very far limit {}",
        folder.display(),
        statics.len(),
        usage.exterior.len(),
        options.far_min_size,
        options.very_far_min_size
    );

    // The scales of the exterior placements of each static.
    let mut scales: HashMap<usize, Vec<f32>> = HashMap::new();
    for reference in &usage.exterior {
        scales.entry(reference.id as usize).or_default().push(reference.scale);
    }

    println!();
    println!("Statics whose record key holds a text:");
    let mut found: Vec<(String, usize)> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| (key.render(), index))
        .filter(|(name, _)| options.matches(name))
        .collect();
    found.sort();
    for (name, index) in &found {
        print_static(
            &options,
            name,
            &statics[*index],
            scales.get(index).map(Vec::as_slice).unwrap_or_default(),
        );
    }
    if found.is_empty() {
        println!("  none");
    }

    println!();
    println!("Merge cells that hold a mesh with a text:");
    print_merge_cells(&options, &state, &statics, &keys);
    Ok(())
}

/// Builds each mesh of `--mesh` with the generator's extraction and prints its tier at scale 1.
fn print_meshes(options: &Options) -> Result<()> {
    let ini = options.ini.clone().context("--mesh needs --ini")?;
    let vfs = Vfs::load(&VfsLoadOptions {
        morrowind_ini: Some(ini),
        data_dirs: Some(options.data.clone()),
        plugins: Some(Vec::new()),
    })?;
    let overrides = OverridesBuilder::new().finish();
    println!(
        "Meshes built by the generator's extraction, no overrides, far limit {}, very far limit {}:",
        options.far_min_size, options.very_far_min_size
    );
    for mesh in &options.meshes {
        let key = mesh.to_ascii_lowercase().replace('/', "\\");
        let extraction = DistantStatic::from_nif_with_identity(&key, &vfs, 1.0, 0.0, false, 1.0, true, &overrides);
        match extraction.distant_static {
            Some(distant_static) => {
                let kind = distant_static.static_type as u32;
                let radius = distant_static.bounding_sphere.radius;
                let tier = options.tier_of(kind, radius).map_or("grass", |tier| TIER_NAMES[tier]);
                println!(
                    "  {key}: {:?} (type {kind}), radius {radius:.1}, tier at scale 1 {tier}",
                    distant_static.static_type
                );
            }
            None => println!("  {key}: not built (not found, or no visible shape)"),
        }
    }
    Ok(())
}

fn print_static(options: &Options, name: &str, mesh: &PackedDistantStatic, scales: &[f32]) {
    let kind = mesh.static_type as u32;
    let radius = mesh.bounding_sphere.radius;
    let mut tiers = [0usize; 3];
    let mut grass = 0;
    for &scale in scales {
        match options.tier_of(kind, radius * scale) {
            Some(tier) => tiers[tier] += 1,
            None => grass += 1,
        }
    }
    let (low, high) = scales
        .iter()
        .fold((f32::MAX, f32::MIN), |(low, high), &scale| (low.min(scale), high.max(scale)));
    let scale_text = if scales.is_empty() {
        "none".to_owned()
    } else {
        format!("{low:.2} to {high:.2}")
    };
    let tier_text = match options.tier_of(kind, radius) {
        Some(tier) => TIER_NAMES[tier],
        None => "grass",
    };
    println!(
        "  {name}: {:?} (type {kind}), radius {radius:.1}, tier at scale 1 {tier_text}; {} placements, scale {scale_text}; placements near {}, far {}, very far {}, grass {grass}",
        mesh.static_type,
        scales.len(),
        tiers[0],
        tiers[1],
        tiers[2]
    );
}

fn print_merge_cells(
    options: &Options,
    state: &state_db::GenerationState,
    statics: &[PackedDistantStatic],
    keys: &[StaticRecordKey],
) {
    // The merge cells of the meshes that hold a text, with those meshes.
    let mut cells: BTreeMap<(i32, i32), Vec<String>> = BTreeMap::new();
    for (mesh, mesh_cells) in &state.reverse.mesh_to_merge {
        if !options.matches(mesh) {
            continue;
        }
        for cell in mesh_cells {
            cells.entry((cell.x, cell.y)).or_default().push(mesh.clone());
        }
    }
    if cells.is_empty() {
        println!("  none");
        return;
    }

    // The merged statics of each cell, by the cell of the record key.
    let mut merged: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (index, key) in keys.iter().enumerate() {
        if let StaticRecordKey::Merged { cell_x, cell_y, .. } = key {
            merged.entry((*cell_x, *cell_y)).or_default().push(index);
        }
    }

    for ((cell_x, cell_y), mut members) in cells {
        members.sort();
        let mut tiers = [0usize; 3];
        for &index in merged.get(&(cell_x, cell_y)).map(Vec::as_slice).unwrap_or_default() {
            for subset in &statics[index].subsets {
                for component in &subset.components {
                    if let Some(tier) = options.tier_of(u32::from(component.classification), component.radius) {
                        tiers[tier] += 1;
                    }
                }
            }
        }
        println!(
            "  cell ({cell_x}, {cell_y}): {}; components of the cell near {}, far {}, very far {}",
            members.join(", "),
            tiers[0],
            tiers[1],
            tiers[2]
        );
    }
}

fn parse_options() -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let mut folder = None;
    let mut texts = Vec::new();
    let mut ini = None;
    let mut data = Vec::new();
    let mut meshes = Vec::new();
    let mut far_min_size = FAR_MIN_SIZE;
    let mut very_far_min_size = VERY_FAR_MIN_SIZE;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--match" => texts.push(args.next().context("--match needs a text")?.to_ascii_lowercase()),
            "--far" => far_min_size = args.next().context("--far needs a value")?.parse()?,
            "--very-far" => very_far_min_size = args.next().context("--very-far needs a value")?.parse()?,
            "--ini" => ini = Some(PathBuf::from(args.next().context("--ini needs a path")?)),
            "--data" => data.push(PathBuf::from(args.next().context("--data needs a folder")?)),
            "--mesh" => meshes.push(args.next().context("--mesh needs a mesh path")?),
            _ if folder.is_none() => folder = Some(PathBuf::from(arg)),
            _ => bail!("unknown argument {arg}"),
        }
    }
    if texts.is_empty() && meshes.is_empty() {
        bail!(
            "usage: static_tier_lister <distantland dir> --match <text>... [--far r] [--very-far r], or --ini <path> --data <dir>... --mesh <path>..."
        );
    }
    Ok(Options {
        folder,
        texts,
        ini,
        data,
        meshes,
        far_min_size,
        very_far_min_size,
    })
}

/// The statics of all shards in shard order: the order that `usage.data` ids index.
fn load_statics(statics_folder: &Path) -> Result<Vec<PackedDistantStatic>> {
    let mut statics = Vec::new();
    for shard in 0..STATIC_MESH_SHARD_COUNT {
        let path = statics_folder.join(format!("static_meshes_{shard:03}"));
        statics.extend(load_static_meshes(&path).with_context(|| path.display().to_string())?);
    }
    Ok(statics)
}

/// The generation state. The file is a committed state (`TES3GCS1`) that holds the state bytes.
fn load_state(folder: &Path) -> Result<state_db::GenerationState> {
    let bytes = std::fs::read(folder.join("generation_state.bin")).context("generation_state.bin")?;
    let committed = committed_state::decode(&bytes).map_err(|error| anyhow!("generation_state.bin: {error:?}"))?;
    match state_db::decode(&committed.state_bytes) {
        LoadedGenerationState::Loaded(state) => Ok(state),
        other => bail!("generation_state.bin: {:?}", other.status()),
    }
}

/// The record key of every static, in the same shard-major order as the statics.
fn record_keys(state: &state_db::GenerationState) -> Vec<StaticRecordKey> {
    state
        .static_mesh_shards
        .iter()
        .flat_map(|shard| shard.records.iter().cloned())
        .collect()
}
