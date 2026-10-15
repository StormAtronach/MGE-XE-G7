//! The parameters of the renderer that a person can change while the game runs, to find a
//! good value by eye. Each has a name, a range, a step and a start value.
//!
//! The list is one file, `tunables.toml` of the MGE XE options mod. This program has it
//! built in; the window "renderer parameters" of that mod reads the same file for its
//! sliders, and sends a command for each change (`nirender.tune(command)` of the plugin,
//! data of an extension). The debug overlay shows the values.
//!
//! A value that was found goes into the file as the new start value: the parameters are not
//! settings, and nothing is written to a file.
//!
//! To add one: its entry in the file, and a constant with its name here. The code that
//! needs the value reads it with `get` in each frame.

/// The file of the parameters, as the MGE XE options mod has it.
const FILE: &str = include_str!("../../../assets/Data Files/mwse/mods/MGE XE Options/tunables.toml");

/// The blur of the water reflection that is sharp at the waterline: the angle of the cone
/// that a point of the water mirrors, in radians, without wind.
pub const BLUR_ANGLE: &str = "water.blur.angle";
/// The same: how much the wind adds to the angle (times the wind factor of the water).
pub const BLUR_WIND: &str = "water.blur.wind";
/// The same: the most that the blur is taller than wide, for a flat look at the water.
pub const BLUR_STRETCH: &str = "water.blur.stretch";
/// The light of a glow under water: how much faster it loses the colours that the water has
/// less of (the power of the ratio of the colours of the fog).
pub const WATER_LIGHT_COLOUR: &str = "water.light.colour";
/// The distant lights: the size of a glow as a part of the radius of its light.
pub const GLOW_SIZE: &str = "lights.glow.size";
/// The same: how bright a glow is at night.
pub const GLOW_STRENGTH: &str = "lights.glow.strength";
/// The same: the least size of a glow on the screen, in pixels.
pub const GLOW_PIXELS: &str = "lights.glow.pixels";
/// The same: how strong the baked light is on the land and the statics at night.
pub const BAKED_STRENGTH: &str = "lights.baked.strength";

#[derive(Default)]
struct Tunable {
    name: String,
    label: String,
    value: f32,
    start: f32,
    least: f32,
    most: f32,
    step: f32,
}

pub struct Tunables {
    list: Vec<Tunable>,
}

/// Reads the file: a line `[[parameter]]` starts a parameter, and each line `key = value`
/// after it is one of its keys, with a text between quotation marks or a number.
fn parse(text: &str) -> Vec<Tunable> {
    let mut list: Vec<Tunable> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line == "[[parameter]]" {
            list.push(Tunable::default());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (Some(tunable), Some((key, value))) = (list.last_mut(), line.split_once('=')) else { continue };
        let value = value.trim();
        let text = value.trim_matches('"').to_string();
        let number = value.parse::<f32>().unwrap_or(0.0);
        match key.trim() {
            "name" => tunable.name = text,
            "label" => tunable.label = text,
            "start" => (tunable.start, tunable.value) = (number, number),
            "least" => tunable.least = number,
            "most" => tunable.most = number,
            "step" => tunable.step = number,
            _ => {}
        }
    }
    list
}

impl Tunables {
    pub fn new() -> Tunables {
        Tunables { list: parse(FILE) }
    }

    /// The value of a parameter; 0 for a name that the file does not have (a test checks
    /// that each name of this file is there).
    pub fn get(&self, name: &str) -> f32 {
        self.list.iter().find(|tunable| tunable.name == name).map_or(0.0, |tunable| tunable.value)
    }

    /// The three numbers of the blur that is sharp at the waterline.
    pub fn blur(&self) -> [f32; 3] {
        [self.get(BLUR_ANGLE), self.get(BLUR_WIND), self.get(BLUR_STRETCH)]
    }

    /// One command: `set <name> <value>`, `reset <name>`, or `reset all`. Returns a line
    /// for the log when a value changed, and `None` for a command that is not known.
    pub fn command(&mut self, text: &str) -> Option<String> {
        let mut words = text.split_whitespace();
        match (words.next()?, words.next()?, words.next()) {
            ("reset", "all", _) => {
                for tunable in &mut self.list {
                    tunable.value = tunable.start;
                }
                Some("all parameters at their start values".to_string())
            }
            ("reset", name, _) => {
                let tunable = self.list.iter_mut().find(|tunable| tunable.name == name)?;
                tunable.value = tunable.start;
                Some(format!("{} = {}", tunable.name, shown(tunable.value)))
            }
            ("set", name, Some(value)) => {
                let value = value.parse::<f32>().ok().filter(|v| v.is_finite())?;
                let tunable = self.list.iter_mut().find(|tunable| tunable.name == name)?;
                // In its range, and on a whole number of steps from the least value.
                let steps = if tunable.step > 0.0 { ((value - tunable.least) / tunable.step).round() * tunable.step } else { value - tunable.least };
                tunable.value = (tunable.least + steps).clamp(tunable.least, tunable.most);
                Some(format!("{} = {}", tunable.name, shown(tunable.value)))
            }
            _ => None,
        }
    }

    /// The lines of the debug overlay: each parameter with its value, and its start value
    /// when it differs.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["parameters   (the window of the MGE XE options changes them)".to_string()];
        for tunable in &self.list {
            let start = if tunable.value == tunable.start { String::new() } else { format!("  (start {})", shown(tunable.start)) };
            lines.push(format!("  {:<20} {:<8}{start}", tunable.name, shown(tunable.value)));
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
    fn the_file_has_each_parameter_of_the_code_with_a_range() {
        let tunables = Tunables::new();
        for name in [BLUR_ANGLE, BLUR_WIND, BLUR_STRETCH, WATER_LIGHT_COLOUR, GLOW_SIZE, GLOW_STRENGTH, GLOW_PIXELS, BAKED_STRENGTH] {
            let tunable = tunables.list.iter().find(|tunable| tunable.name == name).unwrap_or_else(|| panic!("no parameter {name} in the file"));
            assert!(!tunable.label.is_empty(), "{name} has no label");
            assert!(tunable.step > 0.0 && tunable.least < tunable.most, "{name} has no range");
            assert!((tunable.least..=tunable.most).contains(&tunable.start), "{name} starts out of its range");
            assert_eq!(tunable.value, tunable.start);
        }
        // Each entry of the file is complete, and no name comes two times.
        for (index, tunable) in tunables.list.iter().enumerate() {
            assert!(!tunable.name.is_empty() && tunable.step > 0.0, "entry {index} is not complete");
            assert_eq!(tunables.list.iter().filter(|other| other.name == tunable.name).count(), 1, "{} comes two times", tunable.name);
        }
        assert_eq!(tunables.blur(), [0.004, 0.18, 2.0]);
    }

    #[test]
    fn the_commands_set_and_reset() {
        let mut tunables = Tunables::new();
        assert_eq!(tunables.command("set water.blur.wind 0.38"), Some("water.blur.wind = 0.38".to_string()));
        assert!((tunables.get(BLUR_WIND) - 0.38).abs() < 1e-6);
        // A changed value shows its start value in the overlay.
        assert!(tunables.lines().iter().any(|line| line.contains("water.blur.wind") && line.contains("(start 0.18)")));
        assert!(tunables.command("set water.blur.angle 0.0101").is_some());
        assert!((tunables.get(BLUR_ANGLE) - 0.01).abs() < 1e-6);
        assert!(tunables.command("reset water.blur.wind").is_some());
        assert_eq!(tunables.get(BLUR_WIND), 0.18);
        assert!((tunables.get(BLUR_ANGLE) - 0.01).abs() < 1e-6);
        assert!(tunables.command("reset all").is_some());
        assert_eq!(tunables.blur(), [0.004, 0.18, 2.0]);
        assert_eq!(tunables.lines().len(), 1 + tunables.list.len());
    }

    #[test]
    fn a_value_stays_in_its_range_and_a_bad_command_does_nothing() {
        let mut tunables = Tunables::new();
        assert!(tunables.command("set water.blur.stretch 99").is_some());
        assert_eq!(tunables.get(BLUR_STRETCH), 6.0);
        assert!(tunables.command("set water.blur.angle -3").is_some());
        assert_eq!(tunables.get(BLUR_ANGLE), 0.0);
        let before = tunables.blur();
        for bad in ["", "louder", "set", "reset", "set no.such.name 1", "reset no.such.name", "set water.blur.wind much", "set water.blur.wind NaN", "set water.blur.wind"] {
            assert_eq!(tunables.command(bad), None, "{bad}");
        }
        assert_eq!(tunables.blur(), before);
    }

    #[test]
    fn the_file_format_is_read() {
        let list = parse("# a comment = with a sign\nname = \"before the first\"\n[[parameter]]\nname = \"a.b\"\nlabel = \"A label: with = signs\"\nabout = \"ignored\"\nstart = 1.5\nleast = 0\nmost = 3\nstep = 0.5\n\n[[parameter]]\nname = \"c\"\n");
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].label.as_str(), list[0].start, list[0].value, list[0].least, list[0].most, list[0].step), ("a.b", "A label: with = signs", 1.5, 1.5, 0.0, 3.0, 0.5));
        assert_eq!(list[1].name, "c");
    }
}
