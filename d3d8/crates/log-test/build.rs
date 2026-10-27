//! Compiles MGE XE's log and a C entry for the tests.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let d3d8_cpp_dir = manifest_dir.join("../../cpp");
    let log = d3d8_cpp_dir.join("support/log.cpp");
    let test = manifest_dir.join("cpp/log_test.cpp");

    println!("cargo:rerun-if-changed={}", log.display());
    println!("cargo:rerun-if-changed={}", test.display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .flag("/EHsc")
        .define("WIN32", None)
        .define("NOMINMAX", None)
        .include(&d3d8_cpp_dir)
        .include(d3d8_cpp_dir.join("support"))
        .file(&log)
        .file(&test)
        .compile("mge_log_test");
}
