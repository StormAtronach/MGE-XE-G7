//! NIF geometry traversal and transform helpers.

use bytemuck::must_cast_slice;
use glam::Affine3A;
use hashbrown::HashSet;
use minsphere::{BoundingSphere, BoundingSphereScratch};
use str_utils::*;

use tes3::nif::*;

use crate::model::SubsetWater;
use crate::overrides::WaterNames;
use crate::vfs::normalize::make_normalized;

/// LOD distance threshold (in game units) used when selecting which LOD child to extract.
///
/// This matches the distant-static render distance used by MGE-XE so that the extracted
/// geometry corresponds to the level-of-detail the engine would display at that range.
const LOD_DIST: f32 = 8192.0;

#[derive(Clone, Copy, Default)]
struct Properties {
    alpha: NiLink<NiAlphaProperty>,
    material: NiLink<NiMaterialProperty>,
    texturing: NiLink<NiTexturingProperty>,
}

pub struct Geometry<'a> {
    pub shape: &'a NiTriShape,
    pub data: &'a NiTriShapeData,
    pub transform: Affine3A,
    properties: Properties,
}

impl<'a> Geometry<'a> {
    pub(crate) fn base_texture_path(&self, stream: &'a NiStream) -> Option<&'a str> {
        let texturing_property = stream.get(self.properties.texturing)?;
        let texture_map = texturing_property.texture_maps.first()?.as_ref()?;
        let TextureMap::Map(map) = texture_map else {
            return None;
        };

        let texture = stream.get(map.texture)?;
        let TextureSource::External(path) = &texture.source else {
            return None;
        };

        Some(path.as_str())
    }

    pub(crate) fn has_alpha(&self, stream: &'a NiStream) -> bool {
        if let Some(alpha_prop) = stream.get(self.properties.alpha) {
            alpha_prop.alpha_blending() || alpha_prop.alpha_testing()
        } else {
            false
        }
    }

    /// Returns whether this geometry has both a UV controller and the
    /// `mge.distant.scroll` opt-in extra-data tag.
    ///
    /// MGE-XE only enables distant UV animation when both are present. The
    /// controller detects animated UVs, while the string tag acts as an
    /// explicit author opt-in for the runtime's fixed scrolling approximation.
    pub(crate) fn has_uv_controller(&self, stream: &'a NiStream) -> bool {
        self.shape.controllers_of_type::<NiUVController>(stream).next().is_some()
            && self
                .shape
                .extra_datas_of_type::<NiStringExtraData>(stream)
                .any(|data| data.value.eq_ignore_ascii_case("mge.distant.scroll"))
    }

    pub(crate) fn material_property(&self, stream: &'a NiStream) -> Option<&'a NiMaterialProperty> {
        stream.get(self.properties.material)
    }

    /// Returns the stable identity of the shared shape data.
    pub(crate) fn data_id(&self) -> *const NiTriShapeData {
        std::ptr::from_ref(self.data)
    }

    /// Computes a minimum object-space bounding sphere, ignoring non-finite positions.
    pub(crate) fn object_space_bounding_sphere(&self, sphere_scratch: &mut BoundingSphereScratch) -> NiBound {
        let points: &[[f32; 3]] = must_cast_slice(&self.data.vertices);
        let bound = BoundingSphere::from_points_with_scratch(points, sphere_scratch);

        NiBound {
            center: bound.center.map(|v| v as f32).into(),
            radius: bound.radius as f32,
        }
    }

    pub(crate) fn place_bound(&self, object_space: NiBound) -> NiBound {
        object_space.transformed_by(&self.transform)
    }
}

/// What the water rules say about one mesh.
#[derive(Clone, Copy, Debug)]
pub struct MeshWater<'a> {
    /// Name prefixes of the objects that are left out, with everything under them.
    pub body_names: &'a [String],
    /// How the shapes that are left are drawn. `SubsetWater::None` for a mesh that keeps its
    /// own look.
    pub surface: SubsetWater,
}

fn name_starts_with_any(name: &str, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .any(|prefix| name.len() >= prefix.len() && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Returns whether lowercased `text` holds one of `words` with no letter on either side.
fn has_word(text: &str, words: &[String]) -> bool {
    words.iter().any(|word| {
        text.match_indices(word.as_str()).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphabetic()) && !after.is_some_and(|c| c.is_ascii_alphabetic())
        })
    })
}

/// Decides whether a mesh is distant water, and how its surface is drawn.
///
/// `registered` is the mesh override: `Some(false)` is never water, `Some(true)` is water
/// without any name. Otherwise the mesh is water when an object in it has a surface name or a
/// body name. The first surface name carries the words for the look.
///
/// The objects are taken in the order of the mod that makes the water near the player: from
/// the root, depth first, the children of a node in their order, and no farther than the first
/// surface name. An object that nothing under the root refers to does not count.
pub fn mesh_water<'a>(stream: &NiStream, names: &'a WaterNames, registered: Option<bool>) -> Option<MeshWater<'a>> {
    if registered == Some(false) {
        return None;
    }

    let mut tag: Option<String> = None;
    let mut has_body = false;
    let root = stream.roots.first().copied().unwrap_or_default();
    let mut stack = vec![root.key];
    // A malformed file can have a cycle. An object is looked at once.
    let mut seen = HashSet::new();
    while let Some(key) = stack.pop() {
        let Some(this) = stream.objects.get(key) else {
            continue;
        };
        let Ok(object) = <&NiAVObject>::try_from(this) else {
            continue;
        };
        if !seen.insert(key) {
            continue;
        }
        if name_starts_with_any(&object.name, &names.surface) {
            tag = Some(object.name.to_ascii_lowercase());
            break;
        }
        has_body |= name_starts_with_any(&object.name, &names.body);
        if let Ok(node) = <&NiNode>::try_from(this) {
            for child in node.children.iter().rev() {
                stack.push(child.key);
            }
        }
    }
    if registered != Some(true) && tag.is_none() && !has_body {
        return None;
    }

    let tag = tag.unwrap_or_default();
    let surface = if has_word(&tag, &names.plain_words) {
        SubsetWater::None
    } else if has_word(&tag, &names.sky_only_words) {
        SubsetWater::SkyOnly
    } else {
        SubsetWater::ReflectsScene
    };
    Some(MeshWater {
        body_names: &names.body,
        surface,
    })
}

/// Clears root-node transforms before traversal.
pub(crate) fn clear_root_node_transforms(stream: &mut NiStream) {
    for root in &stream.roots {
        if let Some(object) = stream.objects.get_mut(root.key)
            && let Ok(node) = <&mut NiNode>::try_from(object)
        {
            node.clear_transform();
        }
    }
}

/// Iterates visible triangle shapes with accumulated transforms and render properties.
///
/// DFS traversal skips dynamic effects, billboards, particles, collision nodes, culled/editor
/// markers, inactive LOD/switch children, missing geometry, and unsupported texture formats.
/// LOD selection uses the child covering the `LOD_DIST` sample.
///
/// Skinned shapes are included; callers are expected to have baked deformation first, via
/// [`NiStream::apply_skins`], which clears the skin instance.
///
/// In a water mesh the body of the water is left out: the game hides it when it runs. A shape
/// that is drawn as water is kept without a texture and without UVs: the water pass reads
/// neither.
pub(crate) fn visible_geometries<'a>(
    stream: &'a NiStream,
    water: Option<MeshWater<'a>>,
) -> impl Iterator<Item = Geometry<'a>> {
    let root = stream.roots.first().copied().unwrap_or_default();
    let drawn_as_water = water.is_some_and(|water| water.surface.is_water());

    // The engine only handles markers when the root has "mrk" string data.
    let has_markers = stream.root_has_string_data_starting_with("mrk");

    let mut stack = vec![(root.key, Affine3A::IDENTITY, Properties::default())];

    std::iter::from_fn(move || {
        while let Some((key, transform, properties)) = stack.pop() {
            let Some(this) = stream.objects.get(key) else {
                continue;
            };

            if this.is_instance_of::<NiDynamicEffect>()
                || this.is_instance_of::<NiBillboardNode>()
                || this.is_instance_of::<NiBSParticleNode>()
                || this.is_instance_of::<RootCollisionNode>()
            {
                continue;
            }

            let properties = if let Ok(object) = <&NiAVObject>::try_from(this) {
                if object.app_culled() || (has_markers && is_editor_marker(object)) {
                    continue;
                }
                if water.is_some_and(|water| name_starts_with_any(&object.name, water.body_names)) {
                    continue;
                }
                resolved_properties(stream, object, properties)
            } else {
                properties
            };

            if let Ok(node) = <&NiLODNode>::try_from(this) {
                let transform = transform * node.transform();
                for (child, &[min, max]) in node.children.iter().zip(&node.lod_levels) {
                    if LOD_DIST >= min && LOD_DIST < max {
                        stack.push((child.key, transform, properties));
                        break;
                    }
                }
                continue;
            }

            if let Ok(node) = <&NiSwitchNode>::try_from(this) {
                let transform = transform * node.transform();
                if let Some(child) = node.children.get(node.active_index) {
                    stack.push((child.key, transform, properties));
                }
                continue;
            }

            if let Ok(node) = <&NiNode>::try_from(this) {
                let transform = transform * node.transform();
                for child in node.children.iter().rev() {
                    stack.push((child.key, transform, properties));
                }
                continue;
            }

            let Ok(shape) = <&NiTriShape>::try_from(this) else {
                continue;
            };

            let Some(data) = stream.get_as::<_, NiTriShapeData>(shape.geometry_data) else {
                continue;
            };

            if data.vertices.is_empty() || data.triangles.is_empty() {
                continue;
            }
            if data.uv_sets.is_empty() && !drawn_as_water {
                continue;
            }

            let transform = transform * shape.transform();
            let geometry = Geometry {
                shape,
                data,
                transform,
                properties,
            };

            if !drawn_as_water && !geometry.base_texture_path(stream).is_some_and(is_valid_texture_format) {
                continue;
            }

            return Some(geometry);
        }
        None
    })
}

/// Normalizes all external texture source paths in-place for VFS key lookups.
///
/// Converts ASCII uppercase to lowercase and replaces forward slashes with backslashes,
/// matching the key format expected by `Vfs::resolve_texture`.
pub(crate) fn normalize_texture_paths(stream: &mut NiStream) {
    for texture in stream.objects_of_type_mut::<NiSourceTexture>() {
        if let TextureSource::External(path) = &mut texture.source {
            make_normalized(path);
        }
    }
}

fn resolved_properties(stream: &NiStream, object: &NiAVObject, mut properties: Properties) -> Properties {
    for property in &object.properties {
        apply_property(stream, *property, &mut properties);
    }

    properties
}

fn apply_property(stream: &NiStream, property: NiLink<NiProperty>, properties: &mut Properties) {
    match stream.objects.get(property.key) {
        Some(NiType::NiAlphaProperty(_)) => properties.alpha = property.cast(),
        Some(NiType::NiMaterialProperty(_)) => properties.material = property.cast(),
        Some(NiType::NiTexturingProperty(_)) => properties.texturing = property.cast(),
        _ => {}
    }
}

fn is_editor_marker(object: &NiAVObject) -> bool {
    object
        .name
        .starts_with_ignore_ascii_case_with_lowercase_multiple(&["editormarker", "tri editormarker"])
        .is_some()
}

fn is_valid_texture_format(path: &str) -> bool {
    path.ends_with_ignore_ascii_case_with_lowercase_multiple(&[".bmp", ".dds", ".tga"])
        .is_some()
}

#[cfg(test)]
mod tests {
    use tes3::nif::*;

    use super::{has_word, mesh_water, name_starts_with_any};
    use crate::model::SubsetWater;
    use crate::overrides::WaterNames;

    fn kit_names() -> WaterNames {
        WaterNames {
            surface: vec!["watervolume".to_owned()],
            body: vec!["waterbody".to_owned()],
            plain_words: vec!["plain".to_owned()],
            sky_only_words: vec!["skyonly".to_owned()],
        }
    }

    fn named_shape(name: &str) -> NiTriShape {
        let mut shape = NiTriShape::default();
        shape.name = name.into();
        shape
    }

    #[test]
    fn the_first_water_name_is_the_first_in_the_scene_graph_not_in_the_file() {
        // The file holds the objects in this order: the sky-only shape, the plain shape, the
        // rock, the group, the root. The scene graph is root -> [group -> [rock, plain],
        // sky-only]. The group is a switch node that shows the rock: all its children count.
        let mut stream = NiStream::new();
        let sky_only = stream.insert(named_shape("WaterVolume skyonly"));
        let plain = stream.insert(named_shape("WaterVolume plain"));
        let rock = stream.insert(named_shape("Rock"));
        let mut group = NiSwitchNode::default();
        group.children.push(rock.cast());
        group.children.push(plain.cast());
        let group = stream.insert(group);
        let mut root = NiNode::default();
        root.children.push(group.cast());
        root.children.push(sky_only.cast());
        let root = stream.insert(root);
        stream.roots.push(root.cast());

        // Depth first, the plain shape comes before the sky-only shape.
        let names = kit_names();
        let water = mesh_water(&stream, &names, None).expect("water mesh");
        assert_eq!(water.surface, SubsetWater::None);

        // With the children the other way round, the sky-only shape is first.
        let Some(NiType::NiNode(root)) = stream.objects.get_mut(root.key) else {
            panic!("root node");
        };
        root.children.reverse();
        let water = mesh_water(&stream, &names, None).expect("water mesh");
        assert_eq!(water.surface, SubsetWater::SkyOnly);
    }

    #[test]
    fn a_water_name_that_nothing_refers_to_does_not_count() {
        let names = kit_names();
        for unreachable in ["WaterVolume plain", "WaterBody"] {
            let mut stream = NiStream::new();
            stream.insert(named_shape(unreachable));
            let rock = stream.insert(named_shape("Rock"));
            let mut root = NiNode::default();
            root.children.push(rock.cast());
            let root = stream.insert(root);
            stream.roots.push(root.cast());

            // Nothing under the root is named as water.
            assert!(mesh_water(&stream, &names, None).is_none());

            // The surface under the root gives the look, not the object that is first in the file.
            let sky_only = stream.insert(named_shape("WaterVolume skyonly"));
            let Some(NiType::NiNode(root)) = stream.objects.get_mut(root.key) else {
                panic!("root node");
            };
            root.children.push(sky_only.cast());
            let water = mesh_water(&stream, &names, None).expect("water mesh");
            assert_eq!(water.surface, SubsetWater::SkyOnly);
        }
    }

    #[test]
    fn water_words_need_a_boundary_on_both_sides() {
        let words = ["plain".to_owned()];
        assert!(has_word("watervolume plain", &words));
        assert!(has_word("watervolume depth=300 plain.001", &words));
        assert!(!has_word("watervolume explained", &words));
        assert!(!has_word("watervolume plains", &words));
        assert!(!has_word("watervolume", &words));
    }

    #[test]
    fn water_names_match_by_prefix_without_case() {
        let names = ["waterbody".to_owned()];
        assert!(name_starts_with_any("WaterBody", &names));
        assert!(name_starts_with_any("WaterBody.001", &names));
        assert!(!name_starts_with_any("Water", &names));
        assert!(!name_starts_with_any("Tri WaterBody", &names));
    }
}
