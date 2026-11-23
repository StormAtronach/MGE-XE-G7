//! The settings of the post shader ports that a player adjusts, and the file that keeps their
//! values between game sessions (W576 part 4; the owner, 2026-11-23: "values should be saved
//! between sessions, in the same folder as the shader is").
//!
//! A port of a mod's post shader can have settings that the mod's effect file does not have:
//! on Direct3D 9 the mod's Lua computed them and gave the shader the result. [`PORTS`] lists
//! them, with their defaults. This list is their one home:
//! - the helper's port reads each one as a variable of the shader;
//! - d3d8.dll lists them to the API of MGE XE, so a mod or a player sets them from Lua
//!   (`mge.shaders.find({ name = "Eternal Shine" }).radmult = 0.4`), and the value goes to
//!   the helper in the shader-state record.
//!
//! d3d8.dll keeps the values in the file `<shader>.values.toml` beside the effect file
//! ([`values_path`]): one line `name = number` for each setting. It reads the file when the
//! shader loads ([`read_values`]). After a set from Lua it writes the file whole ([`Saver`]),
//! about 1 s after the last change and on a thread of its own, never on the frame thread. A
//! write goes to a temporary file first, then a rename replaces the file, so a crash in the
//! write cannot leave a broken file. A change in the last second before the game ends can be
//! lost: d3d8.dll has no safe point at the game's end to write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A setting of a port: one value.
pub struct Setting {
    pub name: &'static str,
    /// The default, as one value; a slice, so that a port can use it as the default of a
    /// variable of the shader.
    pub default: &'static [f32],
}

/// The settings of the port of one shader, by the name of the shader in the chain.
pub struct PortSettings {
    pub shader: &'static str,
    pub settings: &'static [Setting],
}

/// The settings of Eternal Shine, with the defaults of the mod's Lua (`mwse.loadConfig` of its
/// main.lua, by its names). `near_eye` is not the Lua's: a light nearer to the eye gives no
/// glow (the owner's choice, 2026-11-23).
pub const ETERNAL_SHINE: &[Setting] = &[
    // The multiplier of a light's radius for the glow.
    Setting {
        name: "radmult",
        default: &[0.3],
    },
    // The strength of the air in an interior.
    Setting {
        name: "int",
        default: &[0.25],
    },
    // The strength of the air outside.
    Setting {
        name: "ext",
        default: &[0.4],
    },
    // The strength of the air in rain, thunder and snow.
    Setting {
        name: "rain",
        default: &[0.5],
    },
    // The distance from the eye, in units of the game, within which a light gives no glow.
    Setting {
        name: "near_eye",
        default: &[100.0],
    },
];

/// The ports that have settings.
pub const PORTS: &[PortSettings] = &[PortSettings {
    shader: "Eternal Shine",
    settings: ETERNAL_SHINE,
}];

/// The settings of the port of `shader`; none for a shader whose port has none.
pub fn settings_of(shader: &str) -> &'static [Setting] {
    PORTS
        .iter()
        .find(|port| port.shader == shader)
        .map_or(&[], |port| port.settings)
}

/// The end of the name of a values file.
pub const VALUES_SUFFIX: &str = ".values.toml";

/// The values file of `shader` in the folder of its effect file.
pub fn values_path(folder: &Path, shader: &str) -> PathBuf {
    folder.join(format!("{shader}{VALUES_SUFFIX}"))
}

/// What a read of a values file gave.
#[derive(Debug, Default, PartialEq)]
pub struct ReadValues {
    /// The value of each setting, in the order of the settings; `None` where the file has no
    /// valid value.
    pub values: Vec<Option<f32>>,
    /// A line for the log for each part of the file that was not used.
    pub warnings: Vec<String>,
}

/// Reads the values file at `path` for `settings`. A missing file gives no values and no
/// warning. A file that cannot be read, or that is not TOML, gives no values and a warning.
/// A name that is not a setting, and a value that is not a finite number, give a warning;
/// the other names of the file still apply.
pub fn read_values(path: &Path, settings: &[Setting]) -> ReadValues {
    let mut read = ReadValues {
        values: vec![None; settings.len()],
        warnings: Vec::new(),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return read,
        Err(error) => {
            read.warnings
                .push(format!("{}: not read ({error}); the defaults apply", path.display()));
            return read;
        }
    };
    let document = match text.parse::<toml_edit::DocumentMut>() {
        Ok(document) => document,
        Err(error) => {
            let reason = error.to_string().replace('\n', " ");
            read.warnings
                .push(format!("{}: not TOML ({reason}); the defaults apply", path.display()));
            return read;
        }
    };
    for (name, item) in document.iter() {
        let Some(index) = settings.iter().position(|setting| setting.name == name) else {
            read.warnings.push(format!(
                "{}: `{name}` is not a setting of the shader; not used",
                path.display()
            ));
            continue;
        };
        let number = item
            .as_float()
            .or_else(|| item.as_integer().map(|value| value as f64))
            .map(|value| value as f32)
            .filter(|value| value.is_finite());
        match number {
            Some(value) => read.values[index] = Some(value),
            None => read.warnings.push(format!(
                "{}: `{name}` is not a finite number; the default applies",
                path.display()
            )),
        }
    }
    read
}

/// The text of a values file: a comment line, then one line `name = value` for each setting
/// with a finite value. The value has the shortest form that reads back as the same f32 (0.3,
/// not 0.30000001192092896), always with a decimal point, so that TOML reads a float.
pub fn values_text(shader: &str, settings: &[Setting], values: &[f32]) -> String {
    let mut text = format!("# The values of the post shader {shader}, kept by MGE XE between game sessions.\n");
    for (setting, value) in settings.iter().zip(values) {
        if !value.is_finite() {
            continue;
        }
        let mut number = value.to_string();
        if !number.contains('.') {
            number.push_str(".0");
        }
        text.push_str(&format!("{} = {number}\n", setting.name));
    }
    text
}

/// Writes `text` into the file at `path` whole: into a temporary file first, then a rename
/// replaces the file.
pub fn write_values(path: &Path, text: &str) -> Result<(), String> {
    crate::document::atomic_replace(path, text.as_bytes(), true).map_err(|error| error.to_string())
}

/// The writes that wait, by the path of their file: the text and the time of the last change.
#[derive(Default)]
struct Pending {
    files: BTreeMap<PathBuf, (String, Instant)>,
}

/// A thread that writes each values file `delay` after the last change of it. A slider that
/// changes a value in each frame gives one write.
pub struct Saver {
    shared: Arc<(Mutex<Pending>, Condvar)>,
}

impl Saver {
    /// Starts the thread. `log` gets a line for each failed write.
    pub fn new(delay: Duration, log: impl Fn(&str) + Send + 'static) -> Saver {
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("post shader values".into())
            .spawn(move || Saver::run(&worker, delay, &log))
            .expect("the thread of the values files");
        Saver { shared }
    }

    /// Queues the text of the file at `path`. A newer text of the same file replaces a text
    /// that waits.
    pub fn queue(&self, path: PathBuf, text: String) {
        let (pending, wake) = &*self.shared;
        let mut pending = pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.files.insert(path, (text, Instant::now()));
        wake.notify_one();
    }

    fn run(shared: &(Mutex<Pending>, Condvar), delay: Duration, log: &dyn Fn(&str)) {
        let (pending, wake) = shared;
        let mut guard = pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let now = Instant::now();
            let due: Vec<PathBuf> = guard
                .files
                .iter()
                .filter(|(_, (_, changed))| now.duration_since(*changed) >= delay)
                .map(|(path, _)| path.clone())
                .collect();
            if !due.is_empty() {
                let writes: Vec<(PathBuf, String)> = due
                    .into_iter()
                    .filter_map(|path| guard.files.remove(&path).map(|(text, _)| (path, text)))
                    .collect();
                // The writes go without the lock: a set from Lua never waits for the disk.
                drop(guard);
                for (path, text) in writes {
                    if let Err(error) = write_values(&path, &text) {
                        log(&format!("{}: not written ({error})", path.display()));
                    }
                }
                guard = pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                continue;
            }
            let next = guard.files.values().map(|(_, changed)| *changed + delay).min();
            guard = match next {
                Some(at) => {
                    wake.wait_timeout(guard, at.saturating_duration_since(now))
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0
                }
                None => wake.wait(guard).unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root(label: &str) -> PathBuf {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mge-post-values-{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// The defaults of Eternal Shine are those of the mod's Lua, and the owner's 100 units.
    #[test]
    fn the_settings_of_eternal_shine_have_the_defaults_of_the_lua() {
        let have: Vec<(&str, &[f32])> = settings_of("Eternal Shine")
            .iter()
            .map(|setting| (setting.name, setting.default))
            .collect();
        let want: Vec<(&str, &[f32])> = vec![
            ("radmult", &[0.3]),
            ("int", &[0.25]),
            ("ext", &[0.4]),
            ("rain", &[0.5]),
            ("near_eye", &[100.0]),
        ];
        assert_eq!(have, want);
        assert!(settings_of("Ultra Bloom").is_empty());
    }

    #[test]
    fn a_missing_file_gives_no_values_and_no_warning() {
        let root = test_root("missing");
        let read = read_values(&values_path(&root, "Eternal Shine"), ETERNAL_SHINE);
        assert_eq!(read.values, vec![None; 5]);
        assert!(read.warnings.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// A good line applies; a name that is not a setting and a value that is not a finite
    /// number are skipped with a warning each, and the other lines still apply.
    #[test]
    fn the_good_lines_apply_and_each_bad_line_gives_a_warning() {
        let root = test_root("lines");
        let path = values_path(&root, "Eternal Shine");
        fs::write(&path, "radmult = 0.6\nrain = 1\nglow = 2.0\next = \"thick\"\nint = nan\n").unwrap();
        let read = read_values(&path, ETERNAL_SHINE);
        assert_eq!(read.values, vec![Some(0.6), None, None, Some(1.0), None]);
        assert_eq!(read.warnings.len(), 3, "{:?}", read.warnings);
        assert!(read.warnings.iter().any(|warning| warning.contains("`glow`")));
        assert!(read.warnings.iter().any(|warning| warning.contains("`ext`")));
        assert!(read.warnings.iter().any(|warning| warning.contains("`int`")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_that_is_not_toml_gives_the_defaults_and_a_warning() {
        let root = test_root("broken");
        let path = values_path(&root, "Eternal Shine");
        fs::write(&path, "radmult = 0.6\nrain = [\n").unwrap();
        let read = read_values(&path, ETERNAL_SHINE);
        assert_eq!(read.values, vec![None; 5]);
        assert_eq!(read.warnings.len(), 1);
        assert!(read.warnings[0].contains("not TOML"));
        fs::remove_dir_all(root).unwrap();
    }

    /// A written file reads back the same values, replaces the file before it, and leaves no
    /// temporary file.
    #[test]
    fn a_written_file_reads_back_and_leaves_no_temporary_file() {
        let root = test_root("write");
        let path = values_path(&root, "Eternal Shine");
        fs::write(&path, "radmult = 9.0\n").unwrap();
        let values = [0.3, 0.25, 0.45, 0.5, 120.0];
        let text = values_text("Eternal Shine", ETERNAL_SHINE, &values);
        assert!(text.contains("radmult = 0.3\n"), "{text}");
        write_values(&path, &text).unwrap();
        let read = read_values(&path, ETERNAL_SHINE);
        assert_eq!(read.values, values.map(Some).to_vec());
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
        let names: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Eternal Shine.values.toml"]);
        fs::remove_dir_all(root).unwrap();
    }

    /// Changes that come within the delay give one write of the last text, after the delay.
    #[test]
    fn the_saver_writes_once_after_the_last_change() {
        let root = test_root("saver");
        let path = values_path(&root, "Eternal Shine");
        let saver = Saver::new(Duration::from_millis(300), |line| panic!("{line}"));
        for value in [0.4f32, 0.5, 0.6] {
            let mut values: Vec<f32> = ETERNAL_SHINE.iter().map(|setting| setting.default[0]).collect();
            values[0] = value;
            saver.queue(path.clone(), values_text("Eternal Shine", ETERNAL_SHINE, &values));
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!path.exists(), "a write before the delay");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(read_values(&path, ETERNAL_SHINE).values[0], Some(0.6));
        fs::remove_dir_all(root).unwrap();
    }
}
