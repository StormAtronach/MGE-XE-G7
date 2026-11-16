//! The lights file as an entry of the committed state (W449 step 1): an output from before the
//! Lights kind stays valid and gains the entry with no rewrite of the other files; the lights off
//! leave no entry and no file; a cut write of the lights alone leaves the state not valid, and
//! the next run makes the output right.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use distantland::output_index::{OutputValidation, open_output_snapshot};
use distantland::{NullProgressReporter, OutputPaths, generate};
use distantland_foundation::storage::state::{self, ArtifactKind, ArtifactValidation, CommittedState};
use distantland_test_support::{
    BASELINE_WORLD_V1, build_hermetic_fixture, hermetic_generation_job, write_generation_job_file,
};
use tempfile::TempDir;

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn committed(paths: &OutputPaths) -> CommittedState {
    state::load_and_validate(
        &paths.distantland_dir,
        &paths.generation_state_path,
        ArtifactValidation::Routine,
    )
    .unwrap()
}

/// Makes the output look like one of a build from before the Lights kind: the state without
/// the entry, and no file.
fn drop_the_lights_entry(paths: &OutputPaths) {
    let current = committed(paths);
    let artifacts = current
        .artifacts
        .iter()
        .filter(|entry| entry.kind != ArtifactKind::Lights)
        .cloned()
        .collect();
    let old = CommittedState::new(current.state_bytes.clone(), artifacts).unwrap();
    state::write_durable(&paths.generation_state_path, &old).unwrap();
    fs::remove_file(&paths.lights_path).unwrap();
}

/// The files of the output other than the state and the lights, with their times.
fn other_files(paths: &OutputPaths) -> Vec<(PathBuf, SystemTime)> {
    let mut files: Vec<PathBuf> = paths.static_mesh_shard_paths.to_vec();
    files.extend([
        paths.version_path.clone(),
        paths.usage_data_path.clone(),
        paths.terrain_path.clone(),
        paths.terrain_occlusion_path.clone(),
    ]);
    files
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| {
            let time = mtime(&path);
            (path, time)
        })
        .collect()
}

#[test]
fn an_output_without_a_lights_entry_stays_valid_and_gains_one_alone() {
    let root = TempDir::new().unwrap();
    let inputs = build_hermetic_fixture(BASELINE_WORLD_V1, &root.path().join("inputs")).unwrap();
    let output_root = root.path().join("output");
    let job = hermetic_generation_job(&inputs, &output_root);
    generate(&job, &mut NullProgressReporter).unwrap();
    let paths = OutputPaths::new(&output_root);
    assert!(committed(&paths).artifact_by_kind(ArtifactKind::Lights).is_some());

    drop_the_lights_entry(&paths);
    // The old form is served as it is: no rebuild is needed to read it.
    open_output_snapshot(&output_root, Duration::from_secs(5), OutputValidation::Routine).unwrap();
    let before = other_files(&paths);
    assert!(before.len() > paths.static_mesh_shard_paths.len());

    // Sleep briefly so mtime resolution cannot hide a rewrite.
    std::thread::sleep(Duration::from_millis(20));
    generate(&job, &mut NullProgressReporter).unwrap();

    let entry = committed(&paths)
        .artifact_by_kind(ArtifactKind::Lights)
        .cloned()
        .expect("the run adds the lights entry");
    assert_eq!(fs::read(&paths.lights_path).unwrap().len() as u64, entry.byte_length);
    for (path, time) in before {
        assert_eq!(mtime(&path), time, "{} was written again", path.display());
    }
}

#[test]
fn the_lights_off_leave_no_entry_and_no_file() {
    let root = TempDir::new().unwrap();
    let inputs = build_hermetic_fixture(BASELINE_WORLD_V1, &root.path().join("inputs")).unwrap();
    let output_root = root.path().join("output");
    let mut job = hermetic_generation_job(&inputs, &output_root);
    generate(&job, &mut NullProgressReporter).unwrap();
    let paths = OutputPaths::new(&output_root);
    assert!(paths.lights_path.exists());

    job.settings.distant_lights = false;
    generate(&job, &mut NullProgressReporter).unwrap();
    assert!(committed(&paths).artifact_by_kind(ArtifactKind::Lights).is_none());
    assert!(
        !paths.lights_path.exists(),
        "the pruner removes a lights file that the state does not list"
    );
}

#[test]
fn a_cut_write_of_the_lights_alone_is_not_served_and_the_next_run_makes_it_right() {
    let root = TempDir::new().unwrap();
    let inputs = build_hermetic_fixture(BASELINE_WORLD_V1, &root.path().join("inputs")).unwrap();
    let output_root = root.path().join("output");
    let job = hermetic_generation_job(&inputs, &output_root);
    generate(&job, &mut NullProgressReporter).unwrap();
    let paths = OutputPaths::new(&output_root);
    drop_the_lights_entry(&paths);

    let job_path = root.path().join("job.json");
    write_generation_job_file(&job_path, &job).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_distantland"))
        .arg("--job")
        .arg(&job_path)
        .env("RUST_LOG", "off")
        .env("TES3_DL_FAULT_EXIT", "after_state_invalidation")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(42));
    let refused = open_output_snapshot(&output_root, Duration::from_secs(5), OutputValidation::Routine);
    assert!(refused.is_err(), "an invalidated state must not serve");

    generate(&job, &mut NullProgressReporter).unwrap();
    open_output_snapshot(&output_root, Duration::from_secs(5), OutputValidation::Routine).unwrap();
    assert!(committed(&paths).artifact_by_kind(ArtifactKind::Lights).is_some());
}
