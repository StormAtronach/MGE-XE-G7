use std::borrow::Cow;

use glam::Vec3;

use super::*;
use crate::mge_xe::lights::parse_lights;
use crate::{DistantReference, InteriorMetadata, LightRecord, ObjectDefinition, ObjectKind, StableRefKey};

const FIRE: u32 = 0x10;
const NEGATIVE: u32 = 0x4;
const OFF_BY_DEFAULT: u32 = 0x20;

fn object(mesh: &'static str) -> ObjectDefinition<'static> {
    ObjectDefinition {
        mesh,
        ignore_by_default: false,
        force_mesh_generation: false,
        vis_index: 0,
        kind: ObjectKind::Misc,
        script_id: uncased::Uncased::new(""),
        script_disabled: false,
        disable_target: false,
    }
}

fn light(radius: f32, color: [u8; 3], flags: u32) -> LightRecord {
    LightRecord {
        radius,
        color,
        flags,
        has_mesh: true,
        smoky: true,
    }
}

fn reference(id: &'static str, x: f32) -> DistantReference<'static> {
    DistantReference {
        id: Cow::Borrowed(id),
        deleted: false,
        persistent: false,
        translation: Vec3::new(x, 2.0, 3.0),
        rotation: Vec3::ZERO,
        scale: 1.0,
        vis_index: 0,
    }
}

/// A light record and the object of its model, under one id.
fn add_light(usage: &mut UsageInfo<'static>, id: &str, record: LightRecord, mesh: &'static str) {
    usage.lights.insert(id.to_string(), record);
    usage.objects.insert(id.to_string(), object(mesh));
}

fn place(usage: &mut UsageInfo<'static>, cell: &str, key: u32, reference: DistantReference<'static>) {
    usage
        .cells
        .entry(cell.to_string())
        .or_default()
        .insert(StableRefKey::test(key), reference);
}

fn interior(usage: &mut UsageInfo<'static>, name: &str, behaves_like_exterior: bool) {
    usage.interior_metadata.insert(
        name.to_string().into(),
        InteriorMetadata {
            behaves_like_exterior,
            has_water: false,
            water_height: 0.0,
        },
    );
}

fn usage() -> UsageInfo<'static> {
    let mut usage = UsageInfo::default();
    usage.use_test_reference_source();
    usage
}

fn exterior(lights: &Lights) -> &LightBlock {
    lights.block(&EXTERIOR_KEY).expect("the exterior's block")
}

/// The text that the plugin reader makes of a name's bytes: a decode as WINDOWS-1252, by the
/// reader's own function.
fn plugin_text(bytes: &[u8]) -> String {
    bytes_io::Reader::new(bytes).load_string(bytes.len()).unwrap()
}

#[test]
fn a_lamp_is_a_placed_light_that_lights_and_that_no_script_disables() {
    let mut usage = usage();
    add_light(&mut usage, "lamp", light(450.0, [245, 140, 40], 0), "l\\lamp.nif");
    add_light(&mut usage, "off", light(450.0, [255, 255, 255], OFF_BY_DEFAULT), "l\\off.nif");
    add_light(
        &mut usage,
        "negative",
        light(450.0, [255, 255, 255], NEGATIVE),
        "l\\negative.nif",
    );
    add_light(&mut usage, "black", light(450.0, [0, 0, 0], 0), "l\\black.nif");
    add_light(&mut usage, "no_radius", light(0.0, [255, 255, 255], 0), "l\\none.nif");
    add_light(&mut usage, "own_script", light(450.0, [255, 255, 255], 0), "l\\script.nif");
    usage.objects.get_mut("own_script").unwrap().script_disabled = true;
    add_light(&mut usage, "target", light(450.0, [255, 255, 255], 0), "l\\target.nif");
    usage.objects.get_mut("target").unwrap().disable_target = true;

    let deleted = DistantReference {
        deleted: true,
        ..reference("lamp", 9.0)
    };
    let persistent_target = DistantReference {
        persistent: true,
        ..reference("target", 10.0)
    };
    let placed = [
        reference("lamp", 1.0),
        deleted,
        reference("off", 2.0),
        reference("negative", 3.0),
        reference("black", 4.0),
        reference("no_radius", 5.0),
        reference("own_script", 6.0),
        persistent_target,
        reference("target", 11.0),
        reference("not_a_light", 12.0),
    ];
    for (key, reference) in placed.into_iter().enumerate() {
        place(&mut usage, EXTERIOR_CELL, key as u32, reference);
    }

    let lights = light_blocks(&usage, true, true).lights;
    let block = exterior(&lights);
    let ids: Vec<String> = block.records.iter().map(|record| record.id.to_string()).collect();
    assert_eq!(ids, ["lamp", "target"]);
    let places: Vec<f32> = block.lamps.iter().map(|lamp| lamp.position[0]).collect();
    // A target that is not persistent is not reached by its script's call.
    assert_eq!(places, [1.0, 11.0]);

    // Without the exclusion of disable targets, the persistent target is a lamp too.
    let lights = light_blocks(&usage, true, false).lights;
    assert_eq!(exterior(&lights).lamps.len(), 3);
}

#[test]
fn an_interior_that_behaves_like_an_exterior_is_a_block_by_its_lower_case_name() {
    let mut usage = usage();
    add_light(&mut usage, "lamp", light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    place(&mut usage, EXTERIOR_CELL, 1, reference("lamp", 1.0));
    interior(&mut usage, "Mournhold, Plaza Brindisi Dorom", true);
    place(&mut usage, "Mournhold, Plaza Brindisi Dorom", 2, reference("lamp", 2.0));
    interior(&mut usage, "Arrille's Tradehouse", false);
    place(&mut usage, "Arrille's Tradehouse", 3, reference("lamp", 3.0));

    let lights = light_blocks(&usage, true, true).lights;
    let keys: Vec<String> = lights.blocks.iter().map(|block| block.key.to_string()).collect();
    assert_eq!(keys, ["", "mournhold, plaza brindisi dorom"]);

    let lights = light_blocks(&usage, false, true).lights;
    let keys: Vec<String> = lights.blocks.iter().map(|block| block.key.to_string()).collect();
    assert_eq!(keys, [""]);
}

/// A cell name and a record id with bytes over 127 (here an install in WINDOWS-1251, and an id
/// with a letter of WINDOWS-1252): the file holds the plugin's own bytes, the reader reads the
/// file, and the keys are those that the renderer makes from the game's bytes.
#[test]
fn a_name_with_bytes_over_127_keeps_the_plugins_bytes() {
    let cell_bytes = b"\xc1\xe0\xeb\xec\xee\xf0\xe0, \x80\x9f Hall";
    let id_bytes = b"Lampe_\xe9t\xe9";
    let cell = plugin_text(cell_bytes);
    let id = plugin_text(id_bytes);

    let mut usage = usage();
    add_light(&mut usage, &id, light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    interior(&mut usage, &cell, true);
    let placed = DistantReference {
        id: Cow::Owned(id),
        ..reference("", 2.0)
    };
    place(&mut usage, &cell, 1, placed);

    let lights = light_blocks(&usage, true, true).lights;
    let block = lights
        .block(&key(cell_bytes))
        .expect("the block of the cell, by the game's bytes");
    assert_eq!(block.key.as_bytes(), b"\xc1\xe0\xeb\xec\xee\xf0\xe0, \x80\x9f hall");
    assert_eq!(block.records[0].id, key(id_bytes));
    assert_eq!(block.lamps.len(), 1);

    let on = GenerationSettings {
        distant_lights: true,
        ..Default::default()
    };
    let output = build(&lights, &on).unwrap();
    assert_eq!(parse_lights(&output.bytes).unwrap(), lights);
}

/// Each byte from 1 to 255 comes back from the plugin reader's text as itself, so the key of a
/// plugin's name is the key of the game's bytes. That holds also for the five bytes that have no
/// letter in WINDOWS-1252 (0x81, 0x8D, 0x8F, 0x90, 0x9D), which are letters in WINDOWS-1251.
#[test]
fn each_byte_of_a_plugins_name_comes_back_as_the_games_byte() {
    let encoder = bytes_io::Writer::default();
    for byte in 1..=255u8 {
        let from_plugin = key_of_plugin_name(&encoder, &plugin_text(&[byte]));
        assert_eq!(from_plugin, Some(key(&[byte])), "the byte {byte:#04x}");
    }
}

/// One cell in two spellings that differ in the case of A to Z is one block, and so is one record
/// id in two spellings.
#[test]
fn a_name_in_two_cases_of_a_to_z_is_one_key() {
    let mut usage = usage();
    add_light(&mut usage, "lamp", light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    add_light(&mut usage, "LAMP", light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    interior(&mut usage, "Sotha Sil", true);
    place(&mut usage, "Sotha Sil", 1, reference("lamp", 1.0));
    place(&mut usage, "SOTHA SIL", 2, reference("LAMP", 2.0));

    let lights = light_blocks(&usage, true, true).lights;
    assert_eq!(lights.blocks.len(), 1);
    let block = lights.block(&key(b"sotha sil")).unwrap();
    assert_eq!(block.records.len(), 1);
    assert_eq!(block.lamps.len(), 2);
}

/// A name that the game cannot have (a letter that no single byte gives) costs its own lamp or
/// its own cell, and nothing else.
#[test]
fn a_name_that_the_game_cannot_have_costs_only_itself() {
    let mut usage = usage();
    add_light(&mut usage, "lamp", light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    add_light(&mut usage, "lamp_\u{4e2d}", light(300.0, [255, 200, 100], 0), "l\\lamp.nif");
    place(&mut usage, EXTERIOR_CELL, 1, reference("lamp", 1.0));
    place(&mut usage, EXTERIOR_CELL, 2, reference("lamp_\u{4e2d}", 2.0));
    interior(&mut usage, "Hall \u{4e2d}", true);
    place(&mut usage, "Hall \u{4e2d}", 3, reference("lamp", 3.0));

    let lights = light_blocks(&usage, true, true).lights;
    assert_eq!(lights.blocks.len(), 1);
    let block = exterior(&lights);
    assert_eq!(block.records.len(), 1);
    assert_eq!(block.lamps.len(), 1);
    assert_eq!(parse_lights(&lights.to_bytes()).unwrap(), lights);
}

#[test]
fn smoke_rises_from_a_smoke_model_and_from_a_large_fire() {
    let mut usage = usage();
    usage.objects.insert("chimney".to_string(), object("e\\smoke_chimney.nif"));
    usage.objects.insert("quiet".to_string(), object("e\\nosmoke_chimney.nif"));
    add_light(&mut usage, "campfire", light(300.0, [255, 160, 60], FIRE), "l\\campfire.nif");
    add_light(&mut usage, "candle", light(100.0, [255, 160, 60], FIRE), "l\\candle.nif");
    let still = LightRecord {
        smoky: false,
        ..light(300.0, [255, 160, 60], FIRE)
    };
    add_light(&mut usage, "brazier", still, "l\\nosmoke_brazier.nif");
    for (key, id) in ["chimney", "quiet", "campfire", "candle", "brazier"].into_iter().enumerate() {
        place(&mut usage, EXTERIOR_CELL, key as u32, reference(id, key as f32));
    }

    let lights = light_blocks(&usage, true, true).lights;
    let smoke: Vec<(f32, f32)> = exterior(&lights)
        .smoke
        .iter()
        .map(|source| (source.position[0], source.strength))
        .collect();
    assert_eq!(smoke, [(0.0, EFFECT_SMOKE), (2.0, FIRE_SMOKE)]);
    assert_eq!(exterior(&lights).lamps.len(), 3);
}

/// The block does not depend on the order of the references, and lamps of one record share it.
#[test]
fn the_order_of_the_references_does_not_change_the_file() {
    let make = |order: &[u32]| {
        let mut usage = usage();
        add_light(&mut usage, "b_lamp", light(300.0, [255, 0, 0], 0), "l\\b.nif");
        add_light(&mut usage, "a_lamp", light(200.0, [0, 0, 255], 0), "l\\a.nif");
        let placed = [("b_lamp", 5.0), ("a_lamp", -1.0), ("b_lamp", 2.0)];
        for &index in order {
            let (id, x) = placed[index as usize];
            place(&mut usage, EXTERIOR_CELL, index, reference(id, x));
        }
        light_blocks(&usage, true, true).lights
    };
    let forward = make(&[0, 1, 2]);
    assert_eq!(forward.to_bytes(), make(&[2, 1, 0]).to_bytes());

    let block = exterior(&forward);
    assert_eq!(block.records.len(), 2);
    let lamps: Vec<(f32, u32)> = block.lamps.iter().map(|lamp| (lamp.position[0], lamp.record)).collect();
    // The records by their ids: a_lamp 0, b_lamp 1. The lamps by their places.
    assert_eq!(lamps, [(-1.0, 0), (2.0, 1), (5.0, 1)]);
}

/// What this writer writes, the reader of the renderer reads (`parse_lights`).
#[test]
fn the_renderers_reader_reads_what_this_writes() {
    let mut usage = usage();
    add_light(&mut usage, "lamp", light(450.0, [245, 140, 40], FIRE), "l\\lamp.nif");
    place(&mut usage, EXTERIOR_CELL, 1, reference("lamp", 1.0));
    interior(&mut usage, "Sotha Sil", true);
    place(&mut usage, "Sotha Sil", 2, reference("lamp", 2.0));
    let lights = light_blocks(&usage, true, true).lights;

    let on = GenerationSettings {
        distant_lights: true,
        ..Default::default()
    };
    let output = build(&lights, &on).unwrap();
    assert_eq!((output.block_count, output.lamp_count, output.smoke_count), (2, 2, 2));
    assert_eq!(parse_lights(&output.bytes).unwrap(), lights);

    let off = GenerationSettings {
        distant_lights: false,
        ..Default::default()
    };
    assert!(build(&lights, &off).is_none());
}

/// A lamp model whose one shape glows: a right triangle with legs of `size`, with a texture and
/// an emissive of 0.8.
fn glowing_model(size: f32) -> Vec<u8> {
    use tes3::nif::*;

    let mut stream = NiStream::new();
    let texture = stream.insert(NiSourceTexture {
        source: TextureSource::External("l\\glass.dds".into()),
        ..NiSourceTexture::default()
    });
    let mut texture_map = Map::default();
    texture_map.texture = texture;
    let texturing = stream.insert(NiTexturingProperty {
        texture_maps: vec![Some(TextureMap::Map(texture_map))],
        ..NiTexturingProperty::default()
    });
    let material = stream.insert(NiMaterialProperty {
        emissive_color: Vec3::splat(0.8),
        ..NiMaterialProperty::default()
    });
    let mut data = NiTriShapeData::default();
    data.vertices = vec![Vec3::ZERO, Vec3::X * size, Vec3::Y * size];
    data.normals = vec![Vec3::Z; 3];
    data.uv_sets = vec![glam::Vec2::ZERO, glam::Vec2::X, glam::Vec2::Y];
    data.triangles = vec![[0, 1, 2]];
    data.update_center_radius();
    let data = stream.insert(data);
    let mut shape = NiTriShape::default();
    shape.geometry_data = data.cast();
    shape.properties.push(texturing.cast());
    shape.properties.push(material.cast());
    let shape = stream.insert(shape);
    stream.roots.push(shape.cast());
    stream.save_bytes().unwrap()
}

/// The glow radius of each record comes from its model, read one time for all its records, and
/// goes through the file to the renderer's reader.
#[test]
fn the_glow_radius_of_a_record_is_read_from_its_model() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("meshes").join("l")).unwrap();
    std::fs::write(temp.path().join("meshes").join("l").join("lamp.nif"), glowing_model(10.0)).unwrap();
    let vfs = Vfs {
        ini_path: temp.path().join("Morrowind.ini"),
        data_dirs: vec![temp.path().to_path_buf()],
        active_plugins: vec![],
        archives: vec![],
        maps: crate::vfs::directory_map::build_directory_map(&[temp.path().to_path_buf()]).unwrap(),
    };

    let mut usage = usage();
    add_light(&mut usage, "lamp", light(450.0, [245, 140, 40], 0), "l\\lamp.nif");
    add_light(&mut usage, "lamp_too", light(300.0, [245, 140, 40], 0), "l\\lamp.nif");
    add_light(&mut usage, "absent", light(450.0, [245, 140, 40], 0), "l\\absent.nif");
    let no_model = LightRecord {
        has_mesh: false,
        ..light(450.0, [245, 140, 40], 0)
    };
    add_light(&mut usage, "no_model", no_model, "l\\no_model.nif");
    for (key, id) in ["lamp", "lamp_too", "absent", "no_model"].into_iter().enumerate() {
        place(&mut usage, EXTERIOR_CELL, key as u32, reference(id, key as f32));
    }

    let mut parts = light_blocks(&usage, true, true);
    let models: Vec<(String, &str)> = parts
        .models
        .iter()
        .map(|(id, model)| (id.to_string(), model.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            ("absent".to_string(), "l\\absent.nif"),
            ("lamp".to_string(), "l\\lamp.nif"),
            ("lamp_too".to_string(), "l\\lamp.nif"),
        ]
    );
    assert_eq!(fill_glow_radii(&mut parts, &vfs), 2);

    let radii: Vec<(String, f32)> = exterior(&parts.lights)
        .records
        .iter()
        .map(|record| (record.id.to_string(), record.glow_radius))
        .collect();
    // The legs of 10 give a bound of half the long side.
    let lamp = 50.0f32.sqrt();
    assert_eq!(radii.len(), 4);
    assert_eq!(radii[0], ("absent".to_string(), 0.0));
    assert_eq!(radii[1].0, "lamp");
    assert!((radii[1].1 - lamp).abs() < 0.1, "{radii:?}");
    assert_eq!(radii[2].1, radii[1].1);
    assert_eq!(radii[3], ("no_model".to_string(), 0.0));
    assert_eq!(parse_lights(&parts.lights.to_bytes()).unwrap(), parts.lights);
}
