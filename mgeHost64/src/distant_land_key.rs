//! `numidium.exe --distant-land-key --config <mgeXE.toml>`: the key of a distant land, for the
//! test tools (W521). It needs no game, no window and no GPU, and writes no log.
//!
//! Two lines on standard output:
//! - `format <8 hex> <text>`: the format of this build's distant land output
//!   ([`distantland::output_format`]); the 8 hex are the folder name's part.
//! - `settings <8 hex>`: the identity of the generation settings of that file, as the generator
//!   makes it for its own check ([`distantland::settings_identity`]).
//!
//! A command that does not have this form, or a file that does not parse: one line on standard
//! error and the exit code [`EXIT_PROBLEM`].

use std::path::Path;

/// The option that selects this mode.
pub const OPTION: &str = "--distant-land-key";
/// The exit code of a command or a file that gives no key.
pub const EXIT_PROBLEM: i32 = 2;

/// The two lines of the key for the arguments after [`OPTION`], or the problem.
pub fn lines(arguments: &[String]) -> Result<[String; 2], String> {
    let [flag, config] = arguments else {
        return Err(format!("usage: numidium.exe {OPTION} --config <mgeXE.toml>"));
    };
    if flag != "--config" {
        return Err(format!("usage: numidium.exe {OPTION} --config <mgeXE.toml>"));
    }

    let format = distantland::output_format();
    let settings = distantland::settings_identity(Path::new(config)).map_err(|error| format!("{error:#}"))?;
    Ok([
        format!("format {} {format}", distantland::short_hash(&format)),
        format!("settings {}", &settings[..8]),
    ])
}

/// Prints the key for the arguments after [`OPTION`] and returns the exit code.
pub fn run(arguments: &[String]) -> i32 {
    match lines(arguments) {
        Ok(key) => {
            for line in key {
                println!("{line}");
            }
            0
        }
        Err(problem) => {
            eprintln!("distant land key: {problem}");
            EXIT_PROBLEM
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A folder of the test under the temp folder, removed at the end.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("mgehost64_{name}_{}_{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // T-R2: the two lines of a planted file. The null: a generation setting changed gives
    // another settings line and the same format line.
    #[test]
    fn the_key_of_a_file() {
        let dir = TestDir::new("key");
        let base = "[generation]\nversion = 3\nplugins = [\"Morrowind.esm\"]\n[generation.settings]\ngrass_density = 0.5\n";
        let plain = dir.path().join("plain.toml");
        let grass = dir.path().join("grass.toml");
        fs::write(&plain, base).unwrap();
        fs::write(&grass, base.replace("0.5", "0.25")).unwrap();
        let arguments = |path: &Path| vec!["--config".to_string(), path.display().to_string()];

        let [format, settings] = lines(&arguments(&plain)).unwrap();
        assert!(
            format.starts_with("format ") && format.contains(" MGE_DL_VERSION="),
            "{format}"
        );
        assert_eq!(format.split(' ').nth(1).unwrap().len(), 8, "{format}");
        assert!(
            settings.starts_with("settings ") && settings.len() == "settings ".len() + 8,
            "{settings}"
        );

        let [format_grass, settings_grass] = lines(&arguments(&grass)).unwrap();
        assert_eq!(format_grass, format);
        assert_ne!(settings_grass, settings);
    }

    #[test]
    fn a_command_or_file_without_a_key_is_refused() {
        let dir = TestDir::new("no_key");
        let broken = dir.path().join("broken.toml");
        fs::write(&broken, "[generation\n").unwrap();
        for arguments in [
            vec![],
            vec!["--config".to_string()],
            vec!["--other".to_string(), "x".to_string()],
        ] {
            assert!(lines(&arguments).unwrap_err().starts_with("usage: "), "{arguments:?}");
        }
        assert!(lines(&["--config".to_string(), broken.display().to_string()]).is_err());
        assert_eq!(run(&[]), EXIT_PROBLEM);
    }
}
