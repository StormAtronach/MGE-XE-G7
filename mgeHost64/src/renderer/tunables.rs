//! The parameters of the renderer that a person can change while the game runs, to find a
//! good value by eye. Each has a name, a range and a step. The debug overlay shows them, and
//! the game side sends commands for them (`nirender.tune(command)` of the plugin; the Lua
//! mod of the plugin sends them from the number pad while the overlay is on).
//!
//! A value that was found goes into the code as the new start value: the parameters are not
//! settings, and nothing is written to a file.
//!
//! To add one: a constant for its place, and its line in `Tunables::new`. The code that
//! needs the value reads it with `get` in each frame.

/// The blur of the water reflection that is sharp at the waterline: the angle of the cone
/// that a point of the water mirrors, in radians, without wind.
pub const BLUR_ANGLE: usize = 0;
/// The same: how much the wind adds to the angle (times the wind factor of the water).
pub const BLUR_WIND: usize = 1;
/// The same: the most that the blur is taller than wide, for a flat look at the water.
pub const BLUR_STRETCH: usize = 2;

struct Tunable {
    name: &'static str,
    about: &'static str,
    value: f32,
    start: f32,
    least: f32,
    most: f32,
    step: f32,
}

pub struct Tunables {
    list: Vec<Tunable>,
    selected: usize,
}

impl Tunables {
    pub fn new() -> Tunables {
        let one = |name, about, start: f32, least, most, step| Tunable { name, about, value: start, start, least, most, step };
        Tunables {
            // In the order of the constants above.
            list: vec![
                one("water.blur.angle", "cone of the mirror without wind, radians", 0.004, 0.0, 0.05, 0.0005),
                one("water.blur.wind", "what the wind adds to the cone", 0.18, 0.0, 2.0, 0.02),
                one("water.blur.stretch", "most height of the blur for its width", 2.0, 1.0, 6.0, 0.1),
            ],
            selected: 0,
        }
    }

    pub fn get(&self, which: usize) -> f32 {
        self.list[which].value
    }

    /// The three numbers of the blur that is sharp at the waterline.
    pub fn blur(&self) -> [f32; 3] {
        [self.get(BLUR_ANGLE), self.get(BLUR_WIND), self.get(BLUR_STRETCH)]
    }

    /// One command: `next`, `previous`, `more [steps]`, `less [steps]`, `reset`,
    /// `reset all`, or `set <name> <value>`. Returns a line for the log when a value
    /// changed, and `None` for a move of the selection or a command that is not known.
    pub fn command(&mut self, text: &str) -> Option<String> {
        let mut words = text.split_whitespace();
        let count = self.list.len();
        let steps = |word: Option<&str>| word.and_then(|w| w.parse::<f32>().ok()).filter(|n| n.is_finite()).unwrap_or(1.0);
        let changed = match (words.next()?, words.next(), words.next()) {
            ("next", _, _) => {
                self.selected = (self.selected + 1) % count;
                return None;
            }
            ("previous", _, _) => {
                self.selected = (self.selected + count - 1) % count;
                return None;
            }
            ("more", n, _) => self.put(self.selected, self.list[self.selected].value + steps(n) * self.list[self.selected].step),
            ("less", n, _) => self.put(self.selected, self.list[self.selected].value - steps(n) * self.list[self.selected].step),
            ("reset", Some("all"), _) => {
                for tunable in &mut self.list {
                    tunable.value = tunable.start;
                }
                return Some("all parameters at their start values".to_string());
            }
            ("reset", _, _) => {
                self.list[self.selected].value = self.list[self.selected].start;
                self.selected
            }
            ("set", Some(name), Some(value)) => {
                let which = self.list.iter().position(|tunable| tunable.name == name)?;
                self.put(which, value.parse::<f32>().ok().filter(|v| v.is_finite())?)
            }
            _ => return None,
        };
        Some(format!("{} = {}", self.list[changed].name, shown(self.list[changed].value)))
    }

    /// Sets a value, kept in its range and on a whole number of steps from 0.
    fn put(&mut self, which: usize, value: f32) -> usize {
        let tunable = &mut self.list[which];
        let stepped = (value / tunable.step).round() * tunable.step;
        tunable.value = stepped.clamp(tunable.least, tunable.most);
        which
    }

    /// The lines of the debug overlay: the keys, then each parameter with its value, its
    /// start value when it differs, and what it is. A mark is before the selected one.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["parameters   numpad 8 2 select, 4 6 change (shift: 10 steps), 5 reset".to_string()];
        for (index, tunable) in self.list.iter().enumerate() {
            let mark = if index == self.selected { ">" } else { " " };
            let start = if tunable.value == tunable.start { String::new() } else { format!(" (start {})", shown(tunable.start)) };
            lines.push(format!("{mark} {:<20} {:<8}{start}  {}", tunable.name, shown(tunable.value), tunable.about));
        }
        lines
    }
}

/// A value as a text, without the last digits that a 32-bit float adds.
fn shown(value: f32) -> f64 {
    (value as f64 * 1e6).round() / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_select_and_change() {
        let mut tunables = Tunables::new();
        let start = tunables.get(BLUR_ANGLE);
        assert!(tunables.command("more").is_some_and(|line| line.starts_with("water.blur.angle = ")));
        assert!((tunables.get(BLUR_ANGLE) - (start + 0.0005)).abs() < 1e-6);
        assert!(tunables.command("less 3").is_some());
        assert!((tunables.get(BLUR_ANGLE) - (start - 0.001)).abs() < 1e-6);
        // The selection goes around; a move of it changes no value.
        assert_eq!(tunables.command("previous"), None);
        assert!(tunables.lines()[3].starts_with("> water.blur.stretch"));
        assert_eq!(tunables.command("next"), None);
        assert_eq!(tunables.command("next"), None);
        assert!(tunables.lines()[2].starts_with("> water.blur.wind"));
        assert_eq!(tunables.command("more 10"), Some("water.blur.wind = 0.38".to_string()));
        assert!((tunables.get(BLUR_WIND) - 0.38).abs() < 1e-6);
        assert!(tunables.command("reset").is_some());
        assert_eq!(tunables.get(BLUR_WIND), 0.18);
        // The first value is still changed: its line has the start value.
        assert!(tunables.lines()[1].contains("(start 0.004)"));
        assert!(tunables.command("reset all").is_some());
        assert_eq!(tunables.get(BLUR_ANGLE), start);
    }

    #[test]
    fn a_value_stays_in_its_range_and_a_bad_command_does_nothing() {
        let mut tunables = Tunables::new();
        assert!(tunables.command("less 1000").is_some());
        assert_eq!(tunables.get(BLUR_ANGLE), 0.0);
        assert!(tunables.command("set water.blur.stretch 99").is_some());
        assert_eq!(tunables.get(BLUR_STRETCH), 6.0);
        assert!(tunables.command("set water.blur.stretch 2.34").is_some());
        assert!((tunables.get(BLUR_STRETCH) - 2.3).abs() < 1e-6);
        let before = tunables.blur();
        for bad in ["", "louder", "set", "set no.such.name 1", "set water.blur.wind much", "set water.blur.wind NaN", "more NaN"] {
            let _ = tunables.command(bad);
        }
        // `more NaN` counts as one step of the selected parameter, the angle.
        assert_eq!(tunables.get(BLUR_WIND), before[1]);
        assert_eq!(tunables.get(BLUR_STRETCH), before[2]);
        assert!(tunables.get(BLUR_ANGLE).is_finite());
    }
}
