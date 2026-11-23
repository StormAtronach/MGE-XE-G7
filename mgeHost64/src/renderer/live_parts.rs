//! The parts of the renderer that a test leaves out, and the alpha to coverage of the grass and
//! the distant statics, changed while the game runs. A tool for a test, never a setting of a
//! player: the commands come only from `nirender.tune(command)`, they are not in
//! `tunables.toml` (whose entries the MGE XE options show as sliders), and nothing is saved.
//!
//! - `skip <part>,<part>,...` leaves those parts out ([`PARTS`]); `skip none` leaves none out;
//!   `reset skip` gives back the set of the switch `MGE_XE_SKIP`.
//! - `a2c on`, `a2c off`; `reset a2c` gives back the user's `transparency_antialiasing`.
//! - `reset all` resets both, beside the parameters of `tunables.toml`.
//!
//! A command is held until the end of the frame in which it came ([`LiveParts::end_frame`]): the
//! next frame draws all its views with one set. Without a command, the set is that of
//! `MGE_XE_SKIP`, as before.

/// The parts that a test can leave out, by the word of `MGE_XE_SKIP` and of the command `skip`.
pub const PARTS: [&str; 7] = ["water", "terrain", "statics", "grass", "blend", "glows", "lamps"];

/// The name of the change of a frame that a command of this module made
/// ([`super::Extension::frame_changed`]).
pub const CHANGE: &str = "a tune of the parts left out for a test";

/// The bit of a part in a set, or [`None`] for a word that is not a part.
pub fn bit(part: &str) -> Option<u32> {
    PARTS.iter().position(|name| *name == part).map(|index| 1 << index)
}

/// The set of `MGE_XE_SKIP`'s text: its words split at commas; a word that is not a part is left
/// out, as the switch did before.
pub fn set_of_switch(text: &str) -> u32 {
    text.split(',')
        .filter_map(|word| bit(word.trim()))
        .fold(0, |set, bit| set | bit)
}

/// The parts of a set as a text, in the order of [`PARTS`]; "none" for the empty set.
pub fn text_of(set: u32) -> String {
    let names: Vec<&str> = PARTS
        .iter()
        .enumerate()
        .filter(|(index, _)| set & (1 << index) != 0)
        .map(|(_, name)| *name)
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(",")
    }
}

/// The value of the alpha to coverage that a command asks for: `None` is the user's setting.
pub type Coverage = Option<bool>;

/// The commands' state. The renderer takes the set that comes in force at the end of a frame
/// ([`LiveParts::end_frame`]), and the alpha to coverage that is due at the start of a frame
/// ([`LiveParts::take_coverage`]).
pub struct LiveParts {
    /// The set of `MGE_XE_SKIP`.
    start: u32,
    /// The set in force: the frames draw without these parts.
    now: u32,
    /// The set of a command of this frame, for the end of the frame.
    held: Option<u32>,
    /// The alpha to coverage of a command of this frame, for the end of the frame.
    held_coverage: Option<Coverage>,
    /// The alpha to coverage that the next frame takes, at its start.
    due_coverage: Option<Coverage>,
    /// The alpha to coverage in force: `None` while it is the user's setting.
    coverage: Coverage,
}

impl LiveParts {
    pub fn new(start: u32) -> LiveParts {
        LiveParts {
            start,
            now: start,
            held: None,
            held_coverage: None,
            due_coverage: None,
            coverage: None,
        }
    }

    /// One command. Returns the line for the log, or [`None`] for a command of another kind (the
    /// parameters of `tunables.toml`, which also take `reset all`).
    pub fn command(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text == "reset all" {
            self.held = Some(self.start);
            self.held_coverage = Some(None);
            return Some(format!(
                "skip {} and a2c as the setting, from the next frame",
                text_of(self.start)
            ));
        }
        if text == "reset skip" {
            self.held = Some(self.start);
            return Some(format!("skip {}, from the next frame", text_of(self.start)));
        }
        if text == "reset a2c" {
            self.held_coverage = Some(None);
            return Some("a2c as the setting, from the next frame".to_string());
        }
        if let Some(rest) = text.strip_prefix("a2c ") {
            return Some(match rest.trim() {
                "on" => {
                    self.held_coverage = Some(Some(true));
                    "a2c on, from the next frame".to_string()
                }
                "off" => {
                    self.held_coverage = Some(Some(false));
                    "a2c off, from the next frame".to_string()
                }
                other => format!("a2c {other}: refused, not on or off"),
            });
        }
        let rest = text.strip_prefix("skip")?;
        if !(rest.is_empty() || rest.starts_with(' ')) {
            return None;
        }
        let rest = rest.trim();
        let words: Vec<&str> = rest.split([',', ' ']).filter(|word| !word.is_empty()).collect();
        if words == ["none"] {
            self.held = Some(0);
            return Some("skip none, from the next frame".to_string());
        }
        let mut set = 0;
        for word in &words {
            match bit(word) {
                Some(bit) => set |= bit,
                None => {
                    return Some(format!(
                        "skip {rest}: refused, '{word}' is not a part ({} or none); the set stays {}",
                        PARTS.join(", "),
                        text_of(self.held.unwrap_or(self.now))
                    ));
                }
            }
        }
        if words.is_empty() {
            return Some(format!(
                "skip: refused, no part; the set stays {}",
                text_of(self.held.unwrap_or(self.now))
            ));
        }
        self.held = Some(set);
        Some(format!("skip {}, from the next frame", text_of(set)))
    }

    /// The end of a frame: a held set comes in force, and a held alpha to coverage becomes due for
    /// the next frame's start. Returns the new set when it differs from the set before, else
    /// [`None`]; a new alpha to coverage counts in [`LiveParts::take_coverage`].
    pub fn end_frame(&mut self) -> Option<u32> {
        if let Some(coverage) = self.held_coverage.take() {
            self.due_coverage = Some(coverage);
        }
        let set = self.held.take()?;
        let before = std::mem::replace(&mut self.now, set);
        (set != before).then_some(set)
    }

    /// At the start of a frame: the alpha to coverage that a command made due, when it differs
    /// from the one in force. `Some(None)` is the user's setting.
    pub fn take_coverage(&mut self) -> Option<Coverage> {
        let coverage = self.due_coverage.take()?;
        let before = std::mem::replace(&mut self.coverage, coverage);
        (coverage != before).then_some(coverage)
    }

    /// The lines of the debug overlay: the set and the alpha to coverage, when a command changed
    /// either from its start.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.now != self.start {
            lines.push(format!("skip: {}   (start {})", text_of(self.now), text_of(self.start)));
        }
        if let Some(on) = self.coverage {
            lines.push(format!("a2c: {}   (start: the setting)", if on { "on" } else { "off" }));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water_grass() -> u32 {
        bit("water").unwrap() | bit("grass").unwrap()
    }

    #[test]
    fn the_switch_gives_its_parts_and_drops_other_words() {
        assert_eq!(set_of_switch(""), 0);
        assert_eq!(set_of_switch("water, grass,nothing"), water_grass());
        assert_eq!(text_of(set_of_switch("lamps,water")), "water,lamps");
        assert_eq!(text_of(0), "none");
    }

    /// Without a command the set stays that of the switch: no picture changes.
    #[test]
    fn without_a_command_the_set_is_the_switchs() {
        let mut parts = LiveParts::new(water_grass());
        assert_eq!(parts.end_frame(), None);
        assert_eq!(parts.now(), water_grass());
        assert_eq!(parts.take_coverage(), None);
        assert!(parts.lines().is_empty());
        let mut none = LiveParts::new(0);
        assert_eq!(none.end_frame(), None);
        assert_eq!(none.now(), 0);
    }

    /// What the hold rule needs of a set: a command, the set in force, the end of a frame.
    trait Frames {
        fn command(&mut self, text: &str);
        fn now(&self) -> u32;
        fn end_frame(&mut self);
    }

    impl Frames for LiveParts {
        fn command(&mut self, text: &str) {
            LiveParts::command(self, text);
        }
        fn now(&self) -> u32 {
            self.now
        }
        fn end_frame(&mut self) {
            LiveParts::end_frame(self);
        }
    }

    /// The rule: a command leaves the set of its frame as it was, and the set changes at the end
    /// of that frame; a frame with no command changes nothing.
    fn holds_until_the_end_of_the_frame(parts: &mut impl Frames) -> bool {
        parts.command("skip water, grass");
        let held = parts.now() == 0;
        parts.end_frame();
        let in_force = parts.now() == water_grass();
        parts.end_frame();
        let stays = parts.now() == water_grass();
        parts.command("skip none");
        let held_again = parts.now() == water_grass();
        parts.end_frame();
        held && in_force && stays && held_again && parts.now() == 0
    }

    #[test]
    fn a_set_comes_in_force_at_the_end_of_the_frame() {
        assert!(holds_until_the_end_of_the_frame(&mut LiveParts::new(0)));
    }

    /// The null of the test above: a plant that puts the set in force at the command fails it.
    #[test]
    fn a_set_in_force_at_the_command_fails_the_rule() {
        struct AtOnce(LiveParts);
        impl Frames for AtOnce {
            fn command(&mut self, text: &str) {
                self.0.command(text);
                self.0.end_frame();
            }
            fn now(&self) -> u32 {
                self.0.now
            }
            fn end_frame(&mut self) {}
        }
        assert!(!holds_until_the_end_of_the_frame(&mut AtOnce(LiveParts::new(0))));
    }

    /// The last command of a frame wins; a same set reports no change.
    #[test]
    fn the_last_command_of_a_frame_wins() {
        let mut parts = LiveParts::new(0);
        parts.command("skip water");
        parts.command("skip grass");
        assert_eq!(parts.end_frame(), bit("grass"));
        parts.command("skip grass");
        assert_eq!(parts.end_frame(), None);
    }

    /// A word that is not a part refuses the whole command, and the set stays.
    #[test]
    fn an_unknown_part_is_refused_and_the_set_stays() {
        let mut parts = LiveParts::new(water_grass());
        let line = parts.command("skip water,sky").unwrap();
        assert!(line.contains("refused") && line.contains("'sky'"), "{line}");
        assert!(parts.command("skip ").unwrap().contains("refused"));
        assert_eq!(parts.end_frame(), None);
        assert_eq!(parts.now(), water_grass());
        // The null: a parser that kept the known words would have changed the set.
        assert_ne!(set_of_switch("water,sky"), water_grass());
    }

    /// The resets give back the switch's set and the setting.
    #[test]
    fn the_resets_give_back_the_start() {
        let mut parts = LiveParts::new(bit("blend").unwrap());
        parts.command("skip none");
        parts.command("a2c off");
        parts.end_frame();
        assert_eq!(parts.take_coverage(), Some(Some(false)));
        assert_eq!(parts.lines().len(), 2);
        parts.command("reset skip");
        assert_eq!(parts.end_frame(), bit("blend"));
        parts.command("reset a2c");
        parts.end_frame();
        assert_eq!(parts.take_coverage(), Some(None));
        assert!(parts.lines().is_empty());
        parts.command("skip water");
        parts.command("a2c on");
        parts.end_frame();
        parts.take_coverage();
        parts.command("reset all");
        assert_eq!(parts.end_frame(), bit("blend"));
        assert_eq!(parts.take_coverage(), Some(None));
    }

    /// The alpha to coverage: held to the end of the frame, due at the next start, once.
    #[test]
    fn the_alpha_to_coverage_is_due_at_the_next_start_once() {
        let mut parts = LiveParts::new(0);
        assert_eq!(parts.command("a2c maybe").unwrap(), "a2c maybe: refused, not on or off");
        parts.command("a2c off");
        assert_eq!(parts.take_coverage(), None, "not before the end of the frame");
        parts.end_frame();
        assert_eq!(parts.take_coverage(), Some(Some(false)));
        assert_eq!(parts.take_coverage(), None);
        parts.command("a2c off");
        parts.end_frame();
        assert_eq!(parts.take_coverage(), None, "the same value builds nothing");
    }

    /// Other commands go to the parameters.
    #[test]
    fn other_commands_are_not_taken() {
        let mut parts = LiveParts::new(0);
        assert_eq!(parts.command("set water.blur.angle 0.01"), None);
        assert_eq!(parts.command("reset water.blur.angle"), None);
        assert_eq!(parts.command("skipper water"), None);
        assert!(parts.command("reset all").is_some());
    }
}
