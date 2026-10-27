//! The commit of a build, for the build id that each binary logs on its first line and
//! names in a refused stream: `<program> v<version> <commit>[-dirty]`.
//!
//! A build script reads this file with a `#[path]` module, puts [`commit`] where its crate
//! reads it (an environment variable of the compiler, a C define) and calls [`watch_commit`].
//! `mgeHost64/build.rs` and `d3d8/build.rs` do. The file is the same as
//! `nirender-plugin/build_support/build_id.rs`: each repository has its own commit, and the
//! plugin's repository is not part of this workspace.

use std::path::Path;
use std::process::Command;

/// The output of `git` with `args` in the current folder, or [`None`] when git is not there
/// or the folder is not a repository.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The short hash of the commit of the current folder, with `-dirty` when the work tree has
/// changes that are not committed; "unknown" without git. The tags stay out: the version
/// comes from the manifest.
pub fn commit() -> String {
    git(&["describe", "--always", "--dirty", "--exclude", "*"]).unwrap_or_else(|| "unknown".to_string())
}

/// Names the files of the commit for `rerun-if-changed`: the build script runs again when
/// HEAD, the branch it names or the index changes, that is at a commit or a stage. An edit
/// alone does not run it: the `-dirty` mark is as of the last build.
pub fn watch_commit() {
    let Some(git_dir) = git(&["rev-parse", "--path-format=absolute", "--git-dir"]) else { return };
    for name in ["HEAD", "index"] {
        watch(Path::new(&git_dir).join(name));
    }
    // The branch file lives in the common folder of the repository, also for a work tree.
    let head = std::fs::read_to_string(Path::new(&git_dir).join("HEAD")).unwrap_or_default();
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    if let (Some(reference), Some(common)) = (head.strip_prefix("ref: "), common) {
        watch(Path::new(&common).join(reference.trim()));
    }
}

/// Names a file for `rerun-if-changed`. A file that is not there (a packed branch) is left
/// out: cargo would run the script at every build for it.
fn watch(path: std::path::PathBuf) {
    if path.is_file() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
