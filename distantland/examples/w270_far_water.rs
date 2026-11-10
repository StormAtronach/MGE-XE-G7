//! Lists the distant water near a place, with the height of each water mesh (W270).
//!
//! The far water of the renderer is the water subsets among the distant statics (subset
//! `water` 1 or 2). It draws before the water plane, and both test LessEqual and write the
//! depth, so the far water shows only where it stands above the plane. This tool reads
//! `statics/usage.data`, `statics/static_meshes_*` and `generation_state.bin`, and prints for
//! each water subset within `--radius` (x and y) of `--point`:
//! - the record key of its static, its `water` value, its look line and its texture;
//! - the world z of its vertices (lowest, highest, mean) against `--level`;
//! - the x and y range of its vertices, and the distance of its centre from the point;
//! - whether its placement is in the loaded cells (`--loaded` cells around the point's cell),
//!   where the renderer cuts the far water nearer than the game's view range.
//!
//! Usage: `w270_far_water <distantland dir> --point x,y,z [--radius r] [--level z] [--loaded n]`

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use distantland_formats::distant_statics::{PackedDistantStatic, load_static_meshes};
use distantland_formats::usage_data::{UsageDataReference, load_usage_data};
use distantland_foundation::output::STATIC_MESH_SHARD_COUNT;
use distantland_foundation::record_key::StaticRecordKey;
use distantland_foundation::state_db::{self, LoadedGenerationState};
use distantland_foundation::storage::state as committed_state;
use glam::{Mat3, Vec3};

const CELL: f32 = 8192.0;

struct Options {
    folder: PathBuf,
    point: Vec3,
    radius: f32,
    level: f32,
    loaded: i32,
}

/// One placed static of `usage.data` with its world transform.
struct Placement<'a> {
    reference: &'a UsageDataReference,
    rotation: Mat3,
}

impl Placement<'_> {
    /// The generator's placement: scale, then roll, pitch and yaw, then the translation
    /// (the same product as w48_caster_lister).
    fn new(reference: &UsageDataReference) -> Placement<'_> {
        let [yaw, pitch, roll] = reference.rotation.to_array();
        let rotation = Mat3::from_rotation_x(-yaw) * Mat3::from_rotation_y(-pitch) * Mat3::from_rotation_z(-roll);
        Placement { reference, rotation }
    }

    fn to_world(&self, local: Vec3) -> Vec3 {
        self.rotation * (local * self.reference.scale) + self.reference.location
    }
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let statics_folder = options.folder.join("statics");
    let usage = load_usage_data(&statics_folder.join("usage.data")).context("usage.data")?;
    let statics = load_statics(&statics_folder)?;
    let keys = record_keys(&load_state(&options.folder)?);
    if keys.len() != statics.len() {
        bail!("generation_state.bin names {} records, the shards hold {} statics", keys.len(), statics.len());
    }

    let water_subsets: usize = statics.iter().map(|mesh| mesh.subsets.iter().filter(|s| s.water != 0).count()).sum();
    println!(
        "distant land {}: {} statics, {} with water subsets ({} water subsets), {} exterior placements",
        options.folder.display(),
        statics.len(),
        statics.iter().filter(|mesh| mesh.subsets.iter().any(|s| s.water != 0)).count(),
        water_subsets,
        usage.exterior.len()
    );
    let (cell_x, cell_y) = ((options.point.x / CELL).floor() as i32, (options.point.y / CELL).floor() as i32);
    println!(
        "point ({:.0}, {:.0}, {:.0}) in cell ({cell_x}, {cell_y}); radius {:.0}; plane level {:.1}; loaded cells x {}..{}, y {}..{}",
        options.point.x,
        options.point.y,
        options.point.z,
        options.radius,
        options.level,
        cell_x - options.loaded,
        cell_x + options.loaded,
        cell_y - options.loaded,
        cell_y + options.loaded
    );

    // The coverage of the data: the placements of any static near the point.
    let near = usage
        .exterior
        .iter()
        .filter(|reference| (reference.location - options.point).truncate().length() <= options.radius)
        .count();
    println!("{near} placements of any static within the radius");

    let mut found = 0;
    for (reference_index, reference) in usage.exterior.iter().enumerate() {
        let mesh = &statics[reference.id as usize];
        let placement = Placement::new(reference);
        for (subset_index, subset) in mesh.subsets.iter().enumerate() {
            if subset.water == 0 {
                continue;
            }
            let center = placement.to_world(subset.bounding_sphere.center);
            let sphere_radius = subset.bounding_sphere.radius * reference.scale;
            let flat = (center - options.point).truncate().length();
            if flat > options.radius + sphere_radius {
                continue;
            }
            let world: Vec<Vec3> = subset
                .vertices
                .iter()
                .map(|vertex| {
                    let [x, y, z, _] = vertex.position;
                    placement.to_world(Vec3::new(x.to_f32(), y.to_f32(), z.to_f32()))
                })
                .collect();
            if world.is_empty() {
                continue;
            }
            let low = world.iter().fold(Vec3::splat(f32::MAX), |a, &v| a.min(v));
            let high = world.iter().fold(Vec3::splat(f32::MIN), |a, &v| a.max(v));
            let mean_z = world.iter().map(|v| v.z).sum::<f32>() / world.len() as f32;
            let place = reference.location;
            let (px, py) = ((place.x / CELL).floor() as i32, (place.y / CELL).floor() as i32);
            let loaded = (px - cell_x).abs() <= options.loaded && (py - cell_y).abs() <= options.loaded;
            found += 1;
            println!(
                "{} subset {subset_index} water {} look '{}' texture {}: z {:.1} .. {:.1} (mean {:.1}; above the plane by {:.1} .. {:.1}); x {:.0} .. {:.0}, y {:.0} .. {:.0}; centre {:.0} from the point; ref {reference_index} at ({:.0}, {:.0}, {:.0}) cell ({px}, {py}) scale {:.2}; {}; {} vertices, {} triangles",
                keys[reference.id as usize].render(),
                subset.water,
                subset.water_look,
                subset.texture,
                low.z,
                high.z,
                mean_z,
                low.z - options.level,
                high.z - options.level,
                low.x,
                high.x,
                low.y,
                high.y,
                flat,
                place.x,
                place.y,
                place.z,
                reference.scale,
                if loaded { "in the loaded cells" } else { "outside the loaded cells" },
                world.len(),
                subset.triangles.len()
            );
        }
    }
    println!("{found} water subsets within the radius");
    Ok(())
}

fn parse_options() -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let mut folder = None;
    let mut point = None;
    let mut radius = 30000.0;
    let mut level = 0.0;
    let mut loaded = 1;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--point" => point = Some(parse_vec3(&args.next().context("--point needs x,y,z")?)?),
            "--radius" => radius = args.next().context("--radius needs a value")?.parse()?,
            "--level" => level = args.next().context("--level needs a value")?.parse()?,
            "--loaded" => loaded = args.next().context("--loaded needs a value")?.parse()?,
            _ if folder.is_none() => folder = Some(PathBuf::from(arg)),
            _ => bail!("unknown argument {arg}"),
        }
    }
    let usage = "usage: w270_far_water <distantland dir> --point x,y,z [--radius r] [--level z] [--loaded n]";
    Ok(Options {
        folder: folder.context(usage)?,
        point: point.context(usage)?,
        radius,
        level,
        loaded,
    })
}

fn parse_vec3(text: &str) -> Result<Vec3> {
    let parts: Vec<f32> = text.split(',').map(|part| part.trim().parse()).collect::<Result<_, _>>()?;
    match parts[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => bail!("{text} is not x,y,z"),
    }
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
