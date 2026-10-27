//! Puts the commit of this build into the environment of the compiler as MGE_COMMIT, for the
//! build id of numidium (`DistantLand::name` in `src/renderer.rs`).

#[path = "../build_support/build_id.rs"]
mod build_id;

fn main() {
    println!("cargo:rustc-env=MGE_COMMIT={}", build_id::commit());
    build_id::watch_commit();
}
