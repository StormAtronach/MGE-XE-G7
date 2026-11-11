//! A wrong `--debug-view` of a replay ends the helper with exit code 2, and its line is in the
//! log of `--log` (W335): the check runs after the log is set up, before any device.

use std::path::PathBuf;
use std::process::Command;

/// A folder of its own for one run of the test, under the temporary folder.
fn scratch(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("numidium-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

#[test]
fn a_wrong_debug_view_is_in_the_log_and_ends_with_exit_2() {
    let folder = scratch("debug-view-log");
    let log = folder.join("host.txt");
    let status = Command::new(env!("CARGO_BIN_EXE_numidium"))
        .current_dir(&folder)
        .args(["--replay", "missing.nircap", "picture.png", "--debug-view", "lightz", "--log"])
        .arg(&log)
        .status()
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&folder);

    assert_eq!(status.code(), Some(2), "the log: {text}");
    assert!(text.contains("debug view: "), "the problem is not in the log: {text}");
}
