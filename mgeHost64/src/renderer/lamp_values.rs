//! The game's values of its lamps, from MGE XE's d3d8.dll: the engine's `[LightAttenuation]`
//! settings with the light limit (record 12, [`LightSettings`]).
//!
//! A distant lamp takes the three terms that the engine gives a near lamp of the same radius,
//! by the engine's rule ([`LightSettings::terms`]). The design: the Architect's page
//! `distant-lights-from-fork-2026-11-12.md`, section 2.2.

use std::fmt::Write as _;

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
                "warning: light limit {} asked, {} in force (a node's own lights)",
                self.light_limit_asked, self.light_limit
            );
        } else {
            let _ = write!(line, "light limit {} (a node's own lights)", self.light_limit);
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(refused.describe().contains("warning: light limit 32 asked, 7 in force"));
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
