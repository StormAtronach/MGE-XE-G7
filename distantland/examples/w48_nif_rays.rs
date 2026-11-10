//! Casts rays toward the sun through the full NIF of one placed reference (W48).
//!
//! The caster lister found the merged distant copy of the Vivec cantons on rays from the water
//! toward the sun, where the game's pick found nothing. The pick tests front faces only. This
//! tool reads the full mesh with the generator's NIF reader, places it as the game places a
//! reference, and casts the same rays through every shape, the hidden ones too. Both faces count.
//!
//! For each hit, nearest first, it prints the distance along s, the path of node names, the
//! shape's state (hidden by the AppCulled flag on it or a parent, under a RootCollisionNode, a
//! child of a switch or LOD node) and the side the ray meets: "back" when the shape's vertex
//! normals point along s, so a ray from below meets the back face, which a front-only pick
//! skips.
//!
//! Run: `cargo run --release -p distantland --example w48_nif_rays -- <mesh.nif> --at x,y,z
//! --rotation x,y,z [--scale s] [--sun x,y,z] [--point x,y,z]...`

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tes3::nif::glam::{Affine3A, Mat3, Vec3};
use tes3::nif::*;

/// The sun disc's vector at noon in the W48 runs.
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

/// The length of the rays: the reach of the pick in the sun-line run.
const MAX_ALONG: f32 = 8000.0;

/// The generator's LOD sample distance (statics nif.rs `LOD_DIST`, private there): of a node with
/// LOD levels, the generator keeps the child that covers it; the others are marked inactive.
const LOD_DIST: f32 = 8192.0;

struct Options {
    mesh: PathBuf,
    at: Vec3,
    rotation: Vec3,
    scale: f32,
    sun: Vec3,
    points: Vec<Vec3>,
}

/// What lies between the root and a shape, as the engine would treat it.
#[derive(Clone, Default)]
struct State {
    path: Vec<String>,
    hidden: bool,
    collision: bool,
    inactive: bool,
}

struct Shape<'a> {
    state: State,
    transform: Affine3A,
    data: &'a NiTriShapeData,
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let mut stream = NiStream::from_path(&options.mesh).with_context(|| options.mesh.display().to_string())?;

    // The engine puts the reference's transform in place of the root node's own.
    for root in stream.roots.clone() {
        if let Some(object) = stream.objects.get_mut(root.key)
            && let Ok(node) = <&mut NiNode>::try_from(object)
        {
            node.clear_transform();
        }
    }

    let [rx, ry, rz] = options.rotation.to_array();
    let rotation = Mat3::from_rotation_x(-rx) * Mat3::from_rotation_y(-ry) * Mat3::from_rotation_z(-rz);
    let placement = Affine3A::from_mat3_translation(rotation * options.scale, options.at);

    let shapes = all_shapes(&stream, placement);
    let sun = options.sun.normalize();
    println!(
        "{}: {} shapes, placed at {:?} rotation {:?} scale {}, sun {:?}",
        options.mesh.display(),
        shapes.len(),
        options.at.to_array(),
        options.rotation.to_array(),
        options.scale,
        sun.to_array()
    );
    for shape in &shapes {
        println!(
            "  shape {} triangles {}{}",
            shape.state.path.join("/"),
            shape.data.triangles.len(),
            flags(&shape.state)
        );
    }

    for (index, &point) in options.points.iter().enumerate() {
        let mut hits = Vec::new();
        for shape in &shapes {
            hits.extend(ray_hits(shape, point, sun));
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        println!();
        println!(
            "P{} ({:.0}, {:.0}, {:.0}): {} hits",
            index + 1,
            point.x,
            point.y,
            point.z,
            hits.len()
        );
        for (along, side, path, state) in hits {
            println!("  along {along:7.1}  {side}  {path}{}", flags(state));
        }
    }
    Ok(())
}

fn flags(state: &State) -> String {
    let mut text = String::new();
    if state.hidden {
        text.push_str("  HIDDEN");
    }
    if state.collision {
        text.push_str("  COLLISION");
    }
    if state.inactive {
        text.push_str("  INACTIVE-CHILD");
    }
    text
}

fn parse_options() -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let mut mesh = None;
    let mut at = None;
    let mut rotation = Vec3::ZERO;
    let mut scale = 1.0;
    let mut sun = SUN;
    let mut points = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--at" => at = Some(parse_vec3(&args.next().context("--at needs x,y,z")?)?),
            "--rotation" => rotation = parse_vec3(&args.next().context("--rotation needs x,y,z")?)?,
            "--scale" => scale = args.next().context("--scale needs a value")?.parse()?,
            "--sun" => sun = parse_vec3(&args.next().context("--sun needs x,y,z")?)?,
            "--point" => points.push(parse_vec3(&args.next().context("--point needs x,y,z")?)?),
            _ if mesh.is_none() => mesh = Some(PathBuf::from(arg)),
            _ => bail!("unknown argument {arg}"),
        }
    }
    if points.is_empty() {
        points = POINTS.to_vec();
    }
    Ok(Options {
        mesh: mesh.context(
            "usage: w48_nif_rays <mesh.nif> --at x,y,z [--rotation x,y,z] [--scale s] [--sun x,y,z] [--point x,y,z]...",
        )?,
        at: at.context("--at x,y,z is needed")?,
        rotation,
        scale,
        sun,
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

/// Every triangle shape of the stream with its world transform and the state of its path. Unlike
/// the generator's walk, nothing is left out: hidden, collision and inactive shapes are marked.
fn all_shapes(stream: &NiStream, placement: Affine3A) -> Vec<Shape<'_>> {
    let mut shapes = Vec::new();
    let mut stack: Vec<(NiKey, Affine3A, State)> = stream
        .roots
        .iter()
        .rev()
        .map(|root| (root.key, placement, State::default()))
        .collect();

    while let Some((key, transform, state)) = stack.pop() {
        let Some(this) = stream.objects.get(key) else {
            continue;
        };
        let Ok(object) = <&NiAVObject>::try_from(this) else {
            continue;
        };

        let mut state = state;
        state.path.push(object.name.to_string());
        state.hidden |= object.app_culled();
        state.collision |= this.is_instance_of::<RootCollisionNode>();
        let transform = transform * object.transform();

        if let Ok(shape) = <&NiTriShape>::try_from(this) {
            if let Some(data) = stream.get_as::<_, NiTriShapeData>(shape.geometry_data) {
                shapes.push(Shape { state, transform, data });
            }
            continue;
        }

        let Ok(node) = <&NiNode>::try_from(this) else {
            continue;
        };
        let active: Option<usize> = if let Ok(switch) = <&NiSwitchNode>::try_from(this) {
            Some(switch.active_index)
        } else if let Ok(lod) = <&NiLODNode>::try_from(this) {
            lod.lod_levels.iter().position(|&[min, max]| (min..max).contains(&LOD_DIST))
        } else {
            None
        };
        for (index, child) in node.children.iter().enumerate().rev() {
            let mut child_state = state.clone();
            if active.is_some_and(|active| active != index) {
                child_state.inactive = true;
            }
            stack.push((child.key, transform, child_state));
        }
    }
    shapes
}

/// The hits of the ray from `point` along `sun` with one shape, both faces: the distance along the
/// ray, the side met ("front" or "back" by the vertex normals), the path and the state.
fn ray_hits<'a>(shape: &'a Shape<'a>, point: Vec3, sun: Vec3) -> Vec<(f32, &'static str, String, &'a State)> {
    let world: Vec<Vec3> = shape
        .data
        .vertices
        .iter()
        .map(|&vertex| shape.transform.transform_point3(vertex))
        .collect();
    let normals: Vec<Vec3> = shape
        .data
        .normals
        .iter()
        .map(|&normal| shape.transform.transform_vector3(normal).normalize_or_zero())
        .collect();

    let mut hits = Vec::new();
    for corners in &shape.data.triangles {
        let [a, b, c] = corners.map(|corner| world[corner as usize]);
        let Some(along) = intersect(point, sun, a, b, c) else {
            continue;
        };
        if along <= 1.0 || along > MAX_ALONG {
            continue;
        }
        let facing = if normals.len() == world.len() {
            corners.iter().map(|&corner| normals[corner as usize]).sum::<Vec3>()
        } else {
            (b - a).cross(c - a)
        };
        let side = if facing.dot(sun) > 0.0 { "back " } else { "front" };
        hits.push((along, side, shape.state.path.join("/"), &shape.state));
    }
    hits
}

/// Moller-Trumbore, both faces: the distance along the ray to the triangle, if it meets it.
fn intersect(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge_ab = b - a;
    let edge_ac = c - a;
    let p = direction.cross(edge_ac);
    let determinant = edge_ab.dot(p);
    if determinant.abs() < 1e-8 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let offset = origin - a;
    let u = offset.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = offset.cross(edge_ab);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(edge_ac.dot(q) * inverse)
}
