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
    lights.block(EXTERIOR_KEY).expect("the exterior's block")
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

    let lights = light_blocks(&usage, true, true);
    let block = exterior(&lights);
    let ids: Vec<&str> = block.records.iter().map(|record| record.id.as_str()).collect();
    assert_eq!(ids, ["lamp", "target"]);
    let places: Vec<f32> = block.lamps.iter().map(|lamp| lamp.position[0]).collect();
    // A target that is not persistent is not reached by its script's call.
    assert_eq!(places, [1.0, 11.0]);

    // Without the exclusion of disable targets, the persistent target is a lamp too.
    let lights = light_blocks(&usage, true, false);
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

    let lights = light_blocks(&usage, true, true);
    let keys: Vec<&str> = lights.blocks.iter().map(|block| block.key.as_str()).collect();
    assert_eq!(keys, ["", "mournhold, plaza brindisi dorom"]);

    let lights = light_blocks(&usage, false, true);
    let keys: Vec<&str> = lights.blocks.iter().map(|block| block.key.as_str()).collect();
    assert_eq!(keys, [""]);
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

    let lights = light_blocks(&usage, true, true);
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
        light_blocks(&usage, true, true)
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
    let lights = light_blocks(&usage, true, true);

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
