//! Lists the distant statics that can shade a water point through the sun's shadow map (W48).
//!
//! The tool reads the distant land of one generation with the generator's own readers:
//! `statics/usage.data`, `statics/static_meshes_*` and `generation_state.bin`. For each probe
//! point P it tests every placed subset against a prism. The prism starts at P and runs along
//! the sun vector s up to the depth of the shadow layer. Its cross-section is a disc around the
//! ray, so that it holds the filter kernel of the shadow map for any orientation of the map's
//! texels. A triangle is a hit when its projection on the plane across s reaches the disc and
//! its nearest point lies toward the sun from P.
//!
//! For each hit, nearest along s first, it prints the static's record key (a mesh id, or a
//! merged group with the member meshes of its cell), the subset's water flag and texture, the
//! component of a merged subset, the placement and the distance along s. It also prints the
//! water flags of the True Water tier meshes (`vp_tier`).
//!
//! The shadow map keeps the surface nearest the sun, which is the farthest hit along s from P.
//! So each subset's line also gives the farthest exact hit of a grid of parallel rays in the
//! disc, and each point ends with the farthest caster hit (water left out) in the disc and on
//! the ray through P itself.
//!
//! Run: `cargo run --release -p distantland --example w48_caster_lister -- <distantland dir>`
//! with optional `--sun x,y,z`, `--radius r` and one or more `--point x,y,z`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use distantland_formats::distant_statics::{PackedDistantStatic, load_static_meshes};
use distantland_formats::usage_data::{UsageDataReference, load_usage_data};
use distantland_foundation::output::STATIC_MESH_SHARD_COUNT;
use distantland_foundation::record_key::StaticRecordKey;
use distantland_foundation::state_db::{self, LoadedGenerationState};
use distantland_foundation::storage::state as committed_state;
use glam::{Mat3, Vec2, Vec3};

/// The sun disc's vector at noon in the W48 runs (NUMBERS-w48.txt, the sun line).
const SUN: Vec3 = Vec3::new(0.161, -0.211, 0.964);

/// The 7 probed water points of W48 step 3 (the tier-3 water of Vivec, z 1382).
const POINTS: [Vec3; 7] = [
    Vec3::new(32864.0, -107552.0, 1382.0),
    Vec3::new(32864.0, -107232.0, 1382.0),
    Vec3::new(32864.0, -106432.0, 1382.0),
    Vec3::new(32864.0, -104932.0, 1382.0),
    Vec3::new(33864.0, -106432.0, 1382.0),
    Vec3::new(31864.0, -106432.0, 1382.0),
    Vec3::new(32864.0, -108232.0, 1382.0),
];

/// The depth range of a shadow layer: twice the cell size (rendershadow.cpp, zrange).
const LAYER_DEPTH: f32 = 16384.0;

/// The far layer's 4-texel kernel: 4 * 2 * 4000 / 2048 is about 15.6 units. The disc holds
/// that square in any orientation.
const DEFAULT_RADIUS: f32 = 16.0 * std::f32::consts::SQRT_2;

/// A hit nearer than this along s is the water surface itself or below it.
const MIN_ALONG: f32 = 1.0;

/// The spacing of the parallel rays in the disc. The shadow map keeps the surface nearest the sun
/// in each texel, which is the farthest hit along s from P, so the farthest hit of these rays is
/// what the map can hold over the kernel.
const GRID_STEP: f32 = 2.0;

struct Options {
    folder: PathBuf,
    sun: Vec3,
    radius: f32,
    points: Vec<Vec3>,
}

/// One placed static of `usage.data` with its world transform.
struct Placement<'a> {
    reference: &'a UsageDataReference,
    rotation: Mat3,
}

impl Placement<'_> {
    /// The generator's placement: scale, then roll, pitch and yaw, then the translation
    /// (mgeHost64 state/loading.rs, the D3DX row-vector product S * Rz(-roll) * Ry(-pitch) *
    /// Rx(-yaw) * T).
    fn new(reference: &UsageDataReference) -> Placement<'_> {
        let [yaw, pitch, roll] = reference.rotation.to_array();
        let rotation = Mat3::from_rotation_x(-yaw) * Mat3::from_rotation_y(-pitch) * Mat3::from_rotation_z(-roll);
        Placement { reference, rotation }
    }

    fn to_world(&self, local: Vec3) -> Vec3 {
        self.rotation * (local * self.reference.scale) + self.reference.location
    }
}

/// The ray of one probe point and the plane across it.
struct Probe {
    origin: Vec3,
    along: Vec3,
    across_u: Vec3,
    across_v: Vec3,
}

impl Probe {
    fn new(origin: Vec3, sun: Vec3) -> Probe {
        let along = sun.normalize();
        let across_u = along.any_orthonormal_vector();
        let across_v = along.cross(across_u);
        Probe {
            origin,
            along,
            across_u,
            across_v,
        }
    }

    /// The point in the plane across the ray, and its distance along the ray.
    fn project(&self, world: Vec3) -> (Vec2, f32) {
        let offset = world - self.origin;
        (
            Vec2::new(offset.dot(self.across_u), offset.dot(self.across_v)),
            offset.dot(self.along),
        )
    }

    /// The distance of a sphere from the ray segment, less its radius.
    fn sphere_gap(&self, center: Vec3, radius: f32) -> f32 {
        let offset = center - self.origin;
        let t = offset.dot(self.along).clamp(0.0, LAYER_DEPTH);
        (offset - self.along * t).length() - radius
    }
}

struct Hit {
    point: usize,
    along: f32,
    lateral: f32,
    /// The farthest exact hit along s of the grid rays that pass through the triangle, with the
    /// count of those rays; None when no grid ray passes through it.
    farthest: Option<f32>,
    rays: usize,
    /// The exact hit along s of the ray through P itself, when it passes through the triangle.
    centre: Option<f32>,
    static_index: usize,
    subset_index: usize,
    triangle: usize,
    reference_index: usize,
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let statics_folder = options.folder.join("statics");

    let usage = load_usage_data(&statics_folder.join("usage.data")).context("usage.data")?;
    let statics = load_statics(&statics_folder)?;
    let state = load_state(&options.folder)?;
    let keys = record_keys(&state);
    let merge_members = merge_members(&state);
    if keys.len() != statics.len() {
        bail!(
            "generation_state.bin names {} records, the shards hold {} statics",
            keys.len(),
            statics.len()
        );
    }
    println!(
        "distant land {}: {} statics, {} exterior placements, sun {:?}, disc radius {:.1}",
        options.folder.display(),
        statics.len(),
        usage.exterior.len(),
        options.sun.normalize().to_array(),
        options.radius
    );

    print_tier_meshes(&statics, &keys);

    let probes: Vec<Probe> = options.points.iter().map(|&point| Probe::new(point, options.sun)).collect();
    let (hits, sphere_misses) = find_hits(&probes, &usage.exterior, &statics, options.radius);
    println!("check: {sphere_misses} transformed vertices outside their subset's bounding sphere (expect 0)");

    for (point_index, probe) in probes.iter().enumerate() {
        let mut point_hits: Vec<&Hit> = hits.iter().filter(|hit| hit.point == point_index).collect();
        point_hits.sort_by(|a, b| a.along.total_cmp(&b.along));
        println!();
        println!(
            "P{} ({:.0}, {:.0}, {:.0}): {} triangles in the prism",
            point_index + 1,
            probe.origin.x,
            probe.origin.y,
            probe.origin.z,
            point_hits.len()
        );
        print_point_hits(&point_hits, &usage.exterior, &statics, &keys, &merge_members);
        print_point_farthest(&point_hits, &statics, &keys);
    }
    Ok(())
}

/// The farthest exact hit along s of the non-water subsets: over all grid rays in the disc, and
/// on the ray through P alone.
fn print_point_farthest(hits: &[&Hit], statics: &[PackedDistantStatic], keys: &[StaticRecordKey]) {
    let casters = || {
        hits.iter()
            .filter(|hit| statics[hit.static_index].subsets[hit.subset_index].water == 0)
    };
    let describe = |found: Option<(f32, &&Hit)>| match found {
        Some((along, hit)) => format!(
            "{along:.1} ({}, ref {})",
            keys[hit.static_index].render(),
            hit.reference_index
        ),
        None => "none".to_string(),
    };
    let disc = casters()
        .filter_map(|hit| hit.farthest.map(|along| (along, hit)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let centre = casters()
        .filter_map(|hit| hit.centre.map(|along| (along, hit)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "  farthest caster hit along s: in the disc {}; on the ray through P {}",
        describe(disc),
        describe(centre)
    );
}

fn parse_options() -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let mut folder = None;
    let mut sun = SUN;
    let mut radius = DEFAULT_RADIUS;
    let mut points = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sun" => sun = parse_vec3(&args.next().context("--sun needs x,y,z")?)?,
            "--radius" => radius = args.next().context("--radius needs a value")?.parse()?,
            "--point" => points.push(parse_vec3(&args.next().context("--point needs x,y,z")?)?),
            _ if folder.is_none() => folder = Some(PathBuf::from(arg)),
            _ => bail!("unknown argument {arg}"),
        }
    }
    if points.is_empty() {
        points = POINTS.to_vec();
    }
    Ok(Options {
        folder: folder
            .context("usage: w48_caster_lister <distantland dir> [--sun x,y,z] [--radius r] [--point x,y,z]...")?,
        sun,
        radius,
        points,
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

/// The member meshes of each merge cell, from the reverse index mesh to merge cell.
fn merge_members(state: &state_db::GenerationState) -> HashMap<(i32, i32), Vec<String>> {
    let mut members: HashMap<(i32, i32), Vec<String>> = HashMap::new();
    for (mesh, cells) in &state.reverse.mesh_to_merge {
        for cell in cells {
            members.entry((cell.x, cell.y)).or_default().push(mesh.clone());
        }
    }
    for list in members.values_mut() {
        list.sort();
    }
    members
}

fn print_tier_meshes(statics: &[PackedDistantStatic], keys: &[StaticRecordKey]) {
    println!("True Water tier meshes (record keys with vp_tier):");
    for (key, mesh) in keys.iter().zip(statics) {
        let name = key.render();
        if !name.to_ascii_lowercase().contains("vp_tier") {
            continue;
        }
        let subsets: Vec<String> = mesh
            .subsets
            .iter()
            .map(|subset| format!("water {} {}", subset.water, subset.texture))
            .collect();
        println!("  {name}: {:?}, subsets [{}]", mesh.static_type, subsets.join("; "));
    }
}

/// Tests every placed subset against every probe's prism. Returns the hits and the number of
/// transformed vertices that lie outside their subset's transformed bounding sphere, which
/// checks the sphere pre-filter.
fn find_hits(
    probes: &[Probe],
    placements: &[UsageDataReference],
    statics: &[PackedDistantStatic],
    radius: f32,
) -> (Vec<Hit>, usize) {
    let grid = disc_grid(radius);
    let mut hits = Vec::new();
    let mut sphere_misses = 0;
    for (reference_index, reference) in placements.iter().enumerate() {
        let static_index = reference.id as usize;
        let mesh = &statics[static_index];
        let placement = Placement::new(reference);

        for (subset_index, subset) in mesh.subsets.iter().enumerate() {
            let center = placement.to_world(subset.bounding_sphere.center);
            let sphere_radius = subset.bounding_sphere.radius * reference.scale;
            let near_probes: Vec<usize> = (0..probes.len())
                .filter(|&point| probes[point].sphere_gap(center, sphere_radius) <= radius)
                .collect();
            if near_probes.is_empty() {
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
            sphere_misses += world
                .iter()
                .filter(|vertex| vertex.distance(center) > sphere_radius * 1.01 + 1.0)
                .count();

            for point in near_probes {
                let probe = &probes[point];
                let projected: Vec<(Vec2, f32)> = world.iter().map(|&vertex| probe.project(vertex)).collect();
                for (triangle, corners) in subset.triangles.iter().enumerate() {
                    let [a, b, c] = corners.map(|corner| projected[corner as usize]);
                    let corners = [a.0, b.0, c.0];
                    let depths = [a.1, b.1, c.1];
                    let Some((lateral, along)) = nearest_to_ray(corners, depths) else {
                        continue;
                    };
                    if lateral <= radius && along > MIN_ALONG && along <= LAYER_DEPTH {
                        let exact_hit = |offset: Vec2| {
                            depth_at(corners, depths, offset).filter(|&depth| depth > MIN_ALONG && depth <= LAYER_DEPTH)
                        };
                        let grid_hits: Vec<f32> = grid.iter().filter_map(|&offset| exact_hit(offset)).collect();
                        hits.push(Hit {
                            point,
                            along,
                            lateral,
                            farthest: grid_hits.iter().copied().reduce(f32::max),
                            rays: grid_hits.len(),
                            centre: exact_hit(Vec2::ZERO),
                            static_index,
                            subset_index,
                            triangle,
                            reference_index,
                        });
                    }
                }
            }
        }
    }
    (hits, sphere_misses)
}

/// The offsets of the parallel rays in the plane across s: a square grid of [`GRID_STEP`] cut to
/// the disc, the centre included.
fn disc_grid(radius: f32) -> Vec<Vec2> {
    let steps = (radius / GRID_STEP).floor() as i32;
    let mut grid = Vec::new();
    for row in -steps..=steps {
        for column in -steps..=steps {
            let offset = Vec2::new(column as f32, row as f32) * GRID_STEP;
            if offset.length() <= radius {
                grid.push(offset);
            }
        }
    }
    grid
}

/// The exact distance along s at which the ray at `offset` passes through the projected
/// triangle. The projection is along s, so the depth is linear in the barycentric weights. None
/// when the ray misses, or for a triangle seen edge-on.
fn depth_at(corners: [Vec2; 3], along: [f32; 3], offset: Vec2) -> Option<f32> {
    let [a, b, c] = corners.map(|corner| corner - offset);
    let area = (b - a).perp_dot(c - a);
    if area.abs() < 1e-6 {
        return None;
    }
    let weight_a = b.perp_dot(c) / area;
    let weight_b = c.perp_dot(a) / area;
    let weight_c = a.perp_dot(b) / area;
    (weight_a >= 0.0 && weight_b >= 0.0 && weight_c >= 0.0)
        .then(|| weight_a * along[0] + weight_b * along[1] + weight_c * along[2])
}

/// The distance of a projected triangle from the ray (0 when the ray passes through it) and
/// the distance along the ray of the triangle's point nearest to the ray. None for a triangle
/// seen edge-on.
fn nearest_to_ray(corners: [Vec2; 3], along: [f32; 3]) -> Option<(f32, f32)> {
    let [a, b, c] = corners;
    if (b - a).perp_dot(c - a).abs() < 1e-6 {
        return None;
    }
    if let Some(depth) = depth_at(corners, along, Vec2::ZERO) {
        return Some((0.0, depth));
    }

    let edges = [(0, 1), (1, 2), (2, 0)];
    edges
        .iter()
        .map(|&(from, to)| {
            let start = corners[from];
            let edge = corners[to] - start;
            let share = (-start).dot(edge) / edge.length_squared().max(1e-12);
            let share = share.clamp(0.0, 1.0);
            let lateral = (start + edge * share).length();
            let depth = along[from] + (along[to] - along[from]) * share;
            (lateral, depth)
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
}

fn print_point_hits(
    hits: &[&Hit],
    placements: &[UsageDataReference],
    statics: &[PackedDistantStatic],
    keys: &[StaticRecordKey],
    merge_members: &HashMap<(i32, i32), Vec<String>>,
) {
    // One line per placed subset: its nearest triangle along s, its farthest exact hit of the grid
    // rays, and the count of its triangles.
    let mut seen: Vec<(usize, usize)> = Vec::new();
    for hit in hits {
        let place = (hit.reference_index, hit.subset_index);
        if seen.contains(&place) {
            continue;
        }
        seen.push(place);
        let same_place: Vec<&&Hit> = hits
            .iter()
            .filter(|other| (other.reference_index, other.subset_index) == place)
            .collect();
        let count = same_place.len();
        let farthest = same_place.iter().filter_map(|other| other.farthest).reduce(f32::max);
        let rays: usize = same_place.iter().map(|other| other.rays).sum();
        let farthest_text = farthest.map_or("none".to_string(), |along| format!("{along:.1}"));

        let reference = &placements[hit.reference_index];
        let mesh = &statics[hit.static_index];
        let subset = &mesh.subsets[hit.subset_index];
        let key = &keys[hit.static_index];
        let component = subset.components.iter().position(|component| {
            let first = component.first_triangle as usize;
            (first..first + component.triangle_count as usize).contains(&hit.triangle)
        });

        println!(
            "  along {:7.1}  lateral {:5.1}  farthest {} ({} rays)  {}  {:?}  subset {} water {} alpha {} texture {}  triangles {}  ref {} at ({:.0}, {:.0}, {:.0}) scale {:.2} vis {}",
            hit.along,
            hit.lateral,
            farthest_text,
            rays,
            key.render(),
            mesh.static_type,
            hit.subset_index,
            subset.water,
            subset.has_alpha,
            subset.texture,
            count,
            hit.reference_index,
            reference.location.x,
            reference.location.y,
            reference.location.z,
            reference.scale,
            reference.vis_index
        );
        if let Some(index) = component {
            let record = &subset.components[index];
            println!(
                "      component {} of {}: radius {:.0}, class {}, triangles {}",
                index,
                subset.components.len(),
                record.radius,
                record.classification,
                record.triangle_count
            );
        }
        if let StaticRecordKey::Merged { cell_x, cell_y, .. } = key {
            let members = merge_members.get(&(*cell_x, *cell_y)).map(Vec::as_slice).unwrap_or_default();
            println!("      merge cell ({cell_x}, {cell_y}) members: {}", members.join(", "));
        }
    }
}
