//! The settings identity (W459): the distant lights and the overrides that the generator finds in
//! the mge3 folder are outside it. A change of either is no "settings changed": the lights have
//! a file of their own (`lights.rs`), and the found overrides reach the units through their
//! identities. A key inside the identity still makes the output stale.

use distantland::{GenerationJob, NullProgressReporter, OutputStatusKind, check_output_status, generate};
use distantland_test_support::{BASELINE_WORLD_V1, build_hermetic_fixture, hermetic_generation_job};
use tempfile::TempDir;

/// The kind and the issue lines of the start's status check for `job`.
fn status_of(job: &GenerationJob) -> (OutputStatusKind, String) {
    let status = check_output_status(job).unwrap();
    (status.kind, status.format_issues())
}

#[test]
fn a_change_of_the_lights_or_of_the_found_overrides_is_no_settings_change() {
    let root = TempDir::new().unwrap();
    let inputs = build_hermetic_fixture(BASELINE_WORLD_V1, &root.path().join("inputs")).unwrap();
    let output_root = root.path().join("output");
    let job = hermetic_generation_job(&inputs, &output_root);
    generate(&job, &mut NullProgressReporter).unwrap();
    assert_eq!(status_of(&job).0, OutputStatusKind::Valid);

    // The fixture has no mge3 folder: the search finds nothing, and the output stays current.
    let mut found = job.clone();
    found.settings.discover_override_files = !job.settings.discover_override_files;
    let (kind, issues) = status_of(&found);
    assert_eq!(kind, OutputStatusKind::Valid, "{issues}");

    // The lights alone are stale: their own cause, and no settings change.
    let mut lights = job.clone();
    lights.settings.distant_lights = !job.settings.distant_lights;
    let (kind, issues) = status_of(&lights);
    assert_eq!(kind, OutputStatusKind::Stale, "{issues}");
    assert!(issues.contains("distant lights"), "{issues}");
    assert!(!issues.contains("settings changed"), "{issues}");

    // The null: a key inside the identity is a settings change.
    let mut grass = job.clone();
    grass.settings.grass_density = job.settings.grass_density * 0.5;
    let (kind, issues) = status_of(&grass);
    assert_eq!(kind, OutputStatusKind::Stale, "{issues}");
    assert!(issues.contains("settings changed"), "{issues}");
}
