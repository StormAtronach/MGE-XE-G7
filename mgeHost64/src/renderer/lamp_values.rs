//! The game's values of its lamps, from MGE XE's d3d8.dll: the engine's `[LightAttenuation]`
//! settings with the light limit (record 12, [`LightSettings`]), and the game's light records
//! (record 13, [`LightRecords`]).
//!
//! A distant lamp takes the three terms that the engine gives a near lamp of the same radius,
//! by the engine's rule ([`LightSettings::terms`]). The design: the Architect's page
//! `distant-lights-from-fork-2026-11-12.md`, section 2.2.

use super::distant_lights::FileLampRecord;
use distantland::mge_xe::lights::{Key, key};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

/// A little-endian word of `bytes` at `at`, if the bytes have it.
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at.checked_add(4)?)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// The engine's values of `[LightAttenuation]` in Morrowind.ini, and the most lights of a node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSettings {
    /// 1 UseConstant, 2 UseLinear, 4 UseQuadratic.
    pub flags: u32,
    pub linear_method: i32,
    pub quadratic_method: i32,
    pub constant: f32,
    pub linear: f32,
    pub quadratic: f32,
    pub linear_radius_multiplier: f32,
    pub quadratic_radius_multiplier: f32,
    /// The engine adds the quadratic term outdoors and the linear term indoors, whatever the
    /// flags say.
    pub out_quad_in_lin: bool,
    /// The most lights of a node that MGE XE asked of the engine: 0 when its setting
    /// `ExpandedLightLimit` is off.
    pub light_limit_asked: u32,
    /// The most lights of a node in force.
    pub light_limit: u32,
}

impl LightSettings {
    pub const VERSION: u32 = 1;
    /// The bytes of record 12: twelve words (`Deviceless::LightSettings`).
    pub const BYTES: usize = 48;

    const USE_CONSTANT: u32 = 1;
    const USE_LINEAR: u32 = 2;
    const USE_QUADRATIC: u32 = 4;
    /// A term whose radius times its multiplier is 0 (a radius of 0: the engine's parser turns a
    /// multiplier of 0 into 1).
    const TERM_OF_NO_RADIUS: f32 = 0.01;

    /// Reads record 12. [`None`] for another version or another length.
    pub fn read(bytes: &[u8]) -> Option<LightSettings> {
        if bytes.len() != Self::BYTES || word(bytes, 0)? != Self::VERSION {
            return None;
        }
        let at = |index: usize| word(bytes, index * 4).unwrap();
        let float = |index: usize| f32::from_bits(at(index));
        Some(LightSettings {
            flags: at(1),
            linear_method: at(2) as i32,
            quadratic_method: at(3) as i32,
            constant: float(4),
            linear: float(5),
            quadratic: float(6),
            linear_radius_multiplier: float(7),
            quadratic_radius_multiplier: float(8),
            out_quad_in_lin: at(9) != 0,
            light_limit_asked: at(10),
            light_limit: at(11),
        })
    }

    /// The constant, linear and quadratic terms that the engine gives a light of the record's
    /// `radius`, made `interior` or not: the rule of `EntityLight::createLightOnReference`
    /// (0x4D27E0). Inside or outside is the engine's current cell when it makes the light. The
    /// sums are in double precision, as the x87 code keeps them, and rounded to `f32` at the
    /// end, as the engine stores them.
    pub fn terms(&self, radius: u32, interior: bool) -> [f32; 3] {
        let radius = f64::from(radius);
        let constant = if self.flags & Self::USE_CONSTANT != 0 {
            self.constant
        } else {
            0.0
        };

        // OutQuadInLin adds a term; it does not take the other one away.
        let linear_made = self.flags & Self::USE_LINEAR != 0 || (interior && self.out_quad_in_lin);
        let linear = if !linear_made {
            0.0
        } else if self.linear_method == 0 {
            self.linear
        } else {
            let scaled = radius * f64::from(self.linear_radius_multiplier);
            if scaled == 0.0 {
                Self::TERM_OF_NO_RADIUS
            } else if self.linear_method == 2 {
                (f64::from(self.linear) / (scaled * scaled)) as f32
            } else {
                (1.0 / scaled * f64::from(self.linear)) as f32
            }
        };

        let quadratic_made = self.flags & Self::USE_QUADRATIC != 0 || (!interior && self.out_quad_in_lin);
        let quadratic = if !quadratic_made {
            0.0
        } else if self.quadratic_method == 0 {
            self.quadratic
        } else {
            let scaled = radius * f64::from(self.quadratic_radius_multiplier);
            if scaled == 0.0 {
                Self::TERM_OF_NO_RADIUS
            } else if self.quadratic_method == 1 {
                // Method 1 divides by the radius without its multiplier.
                (1.0 / radius * f64::from(self.quadratic)) as f32
            } else {
                (f64::from(self.quadratic) / (scaled * scaled)) as f32
            }
        };

        [constant, linear, quadratic]
    }

    /// The line of the log for the record: the values, the terms of an outside lamp of radius
    /// 450 (a street lamp, the value that a reading in the game gives), and the light limit.
    pub fn describe(&self) -> String {
        let [constant, linear, quadratic] = self.terms(450, false);
        let mut line = format!(
            "light settings: [LightAttenuation] flags {}, constant {}, linear method {} value {} radius x {}, \
             quadratic method {} value {} radius x {}, OutQuadInLin {}; an outside lamp of radius 450: \
             constant {constant}, linear {linear}, quadratic {quadratic}; ",
            self.flags,
            self.constant,
            self.linear_method,
            self.linear,
            self.linear_radius_multiplier,
            self.quadratic_method,
            self.quadratic,
            self.quadratic_radius_multiplier,
            u8::from(self.out_quad_in_lin),
        );
        if self.light_limit_asked == 0 {
            let _ = write!(
                line,
                "light limit {}: the setting ExpandedLightLimit is off",
                self.light_limit
            );
        } else if self.light_limit != self.light_limit_asked {
            let _ = write!(
                line,
                "warning: light limit {} asked and not taken: the engine's limit stays ({} unless another module changed it)",
                self.light_limit_asked, self.light_limit
            );
        } else {
            let _ = write!(line, "light limit {} (a node's own lights)", self.light_limit);
        }
        line
    }
}

/// What the game holds for one light record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightRecord {
    pub radius: i32,
    /// Red, green and blue, then the fourth byte as the game holds it.
    pub color: [u8; 4],
    /// The flags of the record, as the game holds them.
    pub flags: u32,
}

/// The game's light records, by the key of the id ([`key`]): the values that the running game
/// uses. A mod can change them after the game read its plugins, so they can differ from the
/// values of the lights file, which the generator takes from the plugins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightRecords {
    records: BTreeMap<Key, LightRecord>,
}

/// The difference between the game's light records and the records of the lights file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileDifference {
    /// The records of the file's lamps.
    pub records: usize,
    /// Those that the game holds with another radius or another colour.
    pub changed: usize,
    /// The lamps of the changed records.
    pub changed_lamps: u64,
    /// The records of the file's lamps that the game does not have.
    pub missing: usize,
}

impl LightRecords {
    /// Reads record 13: the version (1), the number of records, then for each record its
    /// radius, its colour (4 bytes), its flags, the length of its id, and the id padded to 4
    /// bytes. [`None`] for another version, a count or a length that passes the end, or bytes
    /// after the last record. An id is the game's own bytes, and every id is taken: no id makes
    /// the record refused.
    pub fn read(bytes: &[u8]) -> Option<LightRecords> {
        if word(bytes, 0)? != 1 {
            return None;
        }
        let count = word(bytes, 4)?;
        let mut records = BTreeMap::new();
        let mut at = 8usize;
        for _ in 0..count {
            let radius = word(bytes, at)? as i32;
            let color: [u8; 4] = bytes.get(at + 4..at + 8)?.try_into().unwrap();
            let flags = word(bytes, at + 8)?;
            let id_length = word(bytes, at + 12)? as usize;
            let id = bytes.get(at + 16..(at + 16).checked_add(id_length)?)?;
            records.insert(key(id), LightRecord { radius, color, flags });
            at = (at + 16).checked_add(id_length.checked_add(3)? & !3)?;
        }
        (at == bytes.len()).then_some(LightRecords { records })
    }

    /// The number of records.
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// The record of `id`.
    pub fn get(&self, id: &Key) -> Option<&LightRecord> {
        self.records.get(id)
    }

    /// The number of records that `previous` does not have with the same values, and of the
    /// records of `previous` that are gone.
    pub fn changed_since(&self, previous: &LightRecords) -> usize {
        let changed = self
            .records
            .iter()
            .filter(|(id, record)| previous.records.get(*id) != Some(record))
            .count();
        let gone = previous.records.keys().filter(|id| !self.records.contains_key(*id)).count();
        changed + gone
    }

    /// The records of the file's lamps against the game's: how many the game holds with
    /// another radius or colour, and how many it does not have.
    pub fn difference_from_file(&self, file: &[FileLampRecord]) -> FileDifference {
        let mut difference = FileDifference {
            records: file.len(),
            ..FileDifference::default()
        };
        for record in file {
            match self.get(&record.id) {
                None => difference.missing += 1,
                Some(game) => {
                    if game.radius as f32 != record.radius || game.color[..3] != record.color {
                        difference.changed += 1;
                        difference.changed_lamps += u64::from(record.lamps);
                    }
                }
            }
        }
        difference
    }

    /// The line of the log for a new record 13: the count, what changed since `previous`, the
    /// length of the record and the time that its read took.
    pub fn describe(&self, previous: Option<&LightRecords>, bytes: usize, read: Duration) -> String {
        let mut line = format!("light records: {} records of the game", self.count());
        if let Some(previous) = previous {
            let _ = write!(line, ", {} changed since the last record", self.changed_since(previous));
        }
        let _ = write!(line, "; {bytes} bytes, read in {:.3} ms", read.as_secs_f64() * 1000.0);
        line
    }

    /// The line of the log that compares the game's records with the lights file.
    pub fn describe_against_file(&self, file: &[FileLampRecord]) -> String {
        let difference = self.difference_from_file(file);
        let mut line = format!(
            "light records: of the {} records of the file's exterior lamps, the game holds {} with another radius or colour ({} lamps)",
            difference.records, difference.changed, difference.changed_lamps
        );
        if difference.missing != 0 {
            let _ = write!(line, "; warning: {} are not in the game's records", difference.missing);
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Record 13 with the records `(id, radius, colour, flags)`.
    fn light_records(records: &[(&str, i32, [u8; 4], u32)]) -> Vec<u8> {
        let records: Vec<(&[u8], i32, [u8; 4], u32)> = records
            .iter()
            .map(|&(id, radius, color, flags)| (id.as_bytes(), radius, color, flags))
            .collect();
        light_records_of_bytes(&records)
    }

    /// Record 13 with the records `(id, radius, colour, flags)`, the ids as the game's bytes.
    fn light_records_of_bytes(records: &[(&[u8], i32, [u8; 4], u32)]) -> Vec<u8> {
        let mut bytes = words(&[1, records.len() as u32]);
        for (id, radius, color, flags) in records {
            bytes.extend_from_slice(&radius.to_le_bytes());
            bytes.extend_from_slice(color);
            bytes.extend_from_slice(&flags.to_le_bytes());
            bytes.extend_from_slice(&(id.len() as u32).to_le_bytes());
            bytes.extend_from_slice(id);
            bytes.resize(bytes.len().next_multiple_of(4), 0);
        }
        bytes
    }

    /// An id is the game's own bytes. An id with bytes over 127 is a record as any other, and
    /// the record of the lights file with the same bytes finds it.
    #[test]
    fn an_id_with_bytes_over_127_is_a_record_as_any_other() {
        let bytes = light_records_of_bytes(&[
            (&b"Lampe_\xe9t\xe9"[..], 300, [1, 2, 3, 0], 0),
            (&b"lamp"[..], 450, [255, 255, 255, 0], 0),
        ]);
        let records = LightRecords::read(&bytes).expect("no id makes the record refused");
        assert_eq!(records.count(), 2);
        assert_eq!(records.get(&key(b"LAMPE_\xe9T\xe9")).map(|record| record.radius), Some(300));
        // 0xC9 is the letter of 0xE9 in the other case in WINDOWS-1252: another key.
        assert_eq!(records.get(&key(b"lampe_\xc9t\xc9")), None);

        let file = [FileLampRecord {
            id: key(b"Lampe_\xe9t\xe9"),
            radius: 300.0,
            color: [1, 2, 3],
            lamps: 7,
        }];
        let difference = records.difference_from_file(&file);
        assert_eq!((difference.changed, difference.missing), (0, 0));
    }

    /// Each byte from 0 to 255 as the id of a record 13 is a record, found by the key of that byte.
    #[test]
    fn an_id_of_any_one_byte_is_a_record() {
        let ids: Vec<[u8; 1]> = (0..=255u8).map(|byte| [byte]).collect();
        let records: Vec<(&[u8], i32, [u8; 4], u32)> = ids.iter().map(|id| (&id[..], i32::from(id[0]), [0; 4], 0)).collect();
        let read = LightRecords::read(&light_records_of_bytes(&records)).expect("no id makes the record refused");
        // A to Z and a to z are one key each: 26 of the 256 ids fall together.
        assert_eq!(read.count(), 256 - 26);
        for id in &ids {
            assert!(read.get(&key(id)).is_some(), "the byte {:#04x}", id[0]);
        }
    }

    #[test]
    fn the_light_records_read_whole_with_their_ids_in_lower_case() {
        let bytes = light_records(&[
            ("Light_Com_Lantern_02_128", 256, [240, 135, 50, 0], 0x13),
            ("lamp", 450, [255, 255, 255, 0], 0),
        ]);
        let records = LightRecords::read(&bytes).unwrap();
        assert_eq!(records.count(), 2);
        assert_eq!(
            records.get(&key(b"light_com_lantern_02_128")),
            Some(&LightRecord {
                radius: 256,
                color: [240, 135, 50, 0],
                flags: 0x13
            })
        );
        assert!(records.get(&key(b"Light_Com_Lantern_02_128")).is_some());
        assert_eq!(records.get(&key(b"light_com_lantern_02_129")), None);
        assert_eq!(LightRecords::read(&words(&[1, 0])).map(|records| records.count()), Some(0));
    }

    #[test]
    fn a_cut_record_a_longer_one_and_a_bad_count_are_refused() {
        let bytes = light_records(&[("lampa", 450, [1, 2, 3, 0], 0), ("lamp", 256, [4, 5, 6, 0], 1)]);
        for cut in 0..bytes.len() {
            assert_eq!(LightRecords::read(&bytes[..cut]), None, "a cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0; 4]);
        assert_eq!(LightRecords::read(&longer), None);
        let mut more = bytes.clone();
        more[4..8].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(LightRecords::read(&more), None);
        let mut other_version = bytes.clone();
        other_version[..4].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(LightRecords::read(&other_version), None);
        let mut id_past_the_end = words(&[1, 1, 0, 0, 0, u32::MAX]);
        id_past_the_end.extend_from_slice(&[0; 8]);
        assert_eq!(LightRecords::read(&id_past_the_end), None);
        // An id of any bytes is not a broken record: it is taken.
        let any_bytes = LightRecords::read(&light_records(&[("l\u{e4}mp", 1, [0; 4], 0)]));
        assert_eq!(any_bytes.map(|records| records.count()), Some(1));
    }

    #[test]
    fn the_changes_since_the_last_record_count_new_changed_and_gone_records() {
        let before = LightRecords::read(&light_records(&[
            ("a", 100, [1, 1, 1, 0], 0),
            ("b", 200, [2, 2, 2, 0], 0),
            ("c", 300, [3, 3, 3, 0], 0),
        ]))
        .unwrap();
        let after = LightRecords::read(&light_records(&[
            ("a", 100, [1, 1, 1, 0], 0),
            ("b", 250, [2, 2, 2, 0], 0),
            ("d", 400, [4, 4, 4, 0], 0),
        ]))
        .unwrap();
        // b changed, d is new, c is gone.
        assert_eq!(after.changed_since(&before), 3);
        assert_eq!(after.changed_since(&after), 0);
        let line = after.describe(Some(&before), 96, Duration::from_micros(250));
        assert!(line.starts_with("light records: 3 records of the game, 3 changed since the last record; 96 bytes"));
        assert!(!after.describe(None, 96, Duration::ZERO).contains("changed"));
    }

    #[test]
    fn the_difference_from_the_file_counts_radius_and_colour_not_flags() {
        let game = LightRecords::read(&light_records(&[
            ("same", 350, [240, 135, 50, 0], 0x13),
            ("radius", 256, [240, 135, 50, 0], 0x13),
            ("colour", 350, [10, 20, 30, 0], 0x13),
            ("flags", 350, [240, 135, 50, 9], 0x51),
        ]))
        .unwrap();
        let file = |id: &str, lamps| FileLampRecord {
            id: key(id.as_bytes()),
            radius: 350.0,
            color: [240, 135, 50],
            lamps,
        };
        let records = [
            file("same", 4),
            file("radius", 126),
            file("colour", 3),
            file("flags", 5),
            file("gone", 2),
        ];
        assert_eq!(
            game.difference_from_file(&records),
            FileDifference {
                records: 5,
                changed: 2,
                changed_lamps: 129,
                missing: 1
            }
        );
        let line = game.describe_against_file(&records);
        assert!(line.contains("of the 5 records of the file's exterior lamps, the game holds 2 with another radius or"));
        assert!(line.contains("(129 lamps)"));
        assert!(line.ends_with("warning: 1 are not in the game's records"));
        assert!(!game.describe_against_file(&records[..4]).contains("warning"));
    }

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    /// Outlander-test's Morrowind.ini: a constant of 0.36 and a quadratic term of 3.25 by
    /// method 2, no linear term.
    fn outlander_test() -> LightSettings {
        LightSettings {
            flags: 1 | 4,
            linear_method: 1,
            quadratic_method: 2,
            constant: 0.36,
            linear: 0.0,
            quadratic: 3.25,
            linear_radius_multiplier: 1.0,
            quadratic_radius_multiplier: 1.0,
            out_quad_in_lin: false,
            light_limit_asked: 32,
            light_limit: 32,
        }
    }

    /// Morrowind75testing's Morrowind.ini: the linear term only, 3 by method 1.
    fn morrowind75testing() -> LightSettings {
        LightSettings {
            flags: 2,
            linear_method: 1,
            quadratic_method: 2,
            constant: 0.0,
            linear: 3.0,
            quadratic: 0.0,
            ..outlander_test()
        }
    }

    fn settings_record(settings: &LightSettings) -> Vec<u8> {
        words(&[
            LightSettings::VERSION,
            settings.flags,
            settings.linear_method as u32,
            settings.quadratic_method as u32,
            settings.constant.to_bits(),
            settings.linear.to_bits(),
            settings.quadratic.to_bits(),
            settings.linear_radius_multiplier.to_bits(),
            settings.quadratic_radius_multiplier.to_bits(),
            u32::from(settings.out_quad_in_lin),
            settings.light_limit_asked,
            settings.light_limit,
        ])
    }

    #[test]
    fn the_street_lamp_of_outlander_test_has_the_terms_of_the_game() {
        // A placed street lamp of radius 450, read in the game: constant 0.36, linear 0,
        // quadratic 0.00001605 (Hob's reading).
        let [constant, linear, quadratic] = outlander_test().terms(450, false);
        assert_eq!(constant, 0.36);
        assert_eq!(linear, 0.0);
        assert!((quadratic - 0.000_016_05).abs() < 0.000_000_01, "{quadratic}");
        assert_eq!(quadratic, (3.25f64 / (450.0 * 450.0)) as f32);
    }

    #[test]
    fn the_linear_form_of_morrowind75testing_divides_by_the_radius() {
        assert_eq!(morrowind75testing().terms(300, false), [0.0, 0.01, 0.0]);
        assert_eq!(morrowind75testing().terms(300, true), [0.0, 0.01, 0.0]);
    }

    #[test]
    fn a_radius_of_0_gives_the_engines_term_of_no_radius() {
        assert_eq!(morrowind75testing().terms(0, false), [0.0, 0.01, 0.0]);
        assert_eq!(outlander_test().terms(0, false), [0.36, 0.0, 0.01]);
        let method_1 = LightSettings {
            quadratic_method: 1,
            ..outlander_test()
        };
        assert_eq!(method_1.terms(0, false)[2], 0.01);
        // Method 0 takes the value, with no radius in it.
        let method_0 = LightSettings {
            linear_method: 0,
            ..morrowind75testing()
        };
        assert_eq!(method_0.terms(0, false), [0.0, 3.0, 0.0]);
    }

    #[test]
    fn the_methods_follow_the_engines_branches() {
        let settings = LightSettings {
            flags: 2 | 4,
            linear: 8.0,
            quadratic: 10.0,
            linear_radius_multiplier: 2.0,
            quadratic_radius_multiplier: 2.0,
            ..outlander_test()
        };
        // Linear: method 2 squares the scaled radius; any other method but 0 divides by it.
        let linear = |method| {
            LightSettings {
                linear_method: method,
                ..settings
            }
            .terms(5, false)[1]
        };
        assert_eq!(linear(2), 8.0 / 100.0);
        assert_eq!(linear(1), 8.0 / 10.0);
        assert_eq!(linear(3), 8.0 / 10.0);
        // Quadratic: method 1 divides by the radius without its multiplier; any other method
        // but 0 by the scaled radius squared.
        let quadratic = |method| {
            LightSettings {
                quadratic_method: method,
                ..settings
            }
            .terms(5, false)[2]
        };
        assert_eq!(quadratic(1), 10.0 / 5.0);
        assert_eq!(quadratic(2), 10.0 / 100.0);
        assert_eq!(quadratic(3), 10.0 / 100.0);
        assert_eq!(quadratic(0), 10.0);
    }

    #[test]
    fn out_quad_in_lin_adds_a_term_and_takes_none_away() {
        let settings = LightSettings {
            out_quad_in_lin: true,
            quadratic: 3.25,
            ..morrowind75testing()
        };
        // Outside: the linear term of its flag, and the quadratic term of OutQuadInLin.
        let outside = settings.terms(100, false);
        assert_eq!(outside[1], 0.03);
        assert_eq!(outside[2], (3.25f64 / 10_000.0) as f32);
        // Inside: the linear term only.
        assert_eq!(settings.terms(100, true), [0.0, 0.03, 0.0]);
        // A quadratic flag only: inside, OutQuadInLin adds the linear term.
        let quadratic_only = LightSettings { flags: 4, ..settings };
        assert_eq!(quadratic_only.terms(100, true)[1], 0.03);
        assert_eq!(quadratic_only.terms(100, false)[1], 0.0);
    }

    #[test]
    fn the_settings_record_reads_whole_and_refuses_another_length_or_version() {
        let settings = outlander_test();
        let record = settings_record(&settings);
        assert_eq!(record.len(), LightSettings::BYTES);
        assert_eq!(LightSettings::read(&record), Some(settings));
        for cut in 0..record.len() {
            assert_eq!(LightSettings::read(&record[..cut]), None, "cut {cut}");
        }
        let mut longer = record.clone();
        longer.extend_from_slice(&[0; 4]);
        assert_eq!(LightSettings::read(&longer), None);
        let mut version_2 = record;
        version_2[0] = 2;
        assert_eq!(LightSettings::read(&version_2), None);
    }

    #[test]
    fn the_settings_line_warns_only_for_a_limit_asked_and_not_taken() {
        assert!(outlander_test().describe().ends_with("light limit 32 (a node's own lights)"));
        let refused = LightSettings {
            light_limit: 7,
            ..outlander_test()
        };
        assert!(
            refused
                .describe()
                .contains("warning: light limit 32 asked and not taken: the engine's limit stays (7 unless")
        );
        let off = LightSettings {
            light_limit_asked: 0,
            light_limit: 7,
            ..outlander_test()
        };
        assert!(
            off.describe()
                .ends_with("light limit 7: the setting ExpandedLightLimit is off")
        );
        assert!(!off.describe().contains("warning"));
    }
}
