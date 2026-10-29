use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(|s| s.as_str()).unwrap_or("help");

    // --max-perf wins if both are given; it is the strictly stronger request.
    let mode = if args.iter().any(|a| a == "--max-perf") {
        BuildMode::MaxPerf
    } else if args.iter().any(|a| a == "--release") {
        BuildMode::Release
    } else {
        BuildMode::Dev
    };
    let morrowind_dir = args
        .iter()
        .position(|a| a == "--morrowind-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let root = workspace_root();

    match command {
        "build" => cmd_build(&root, mode),
        // The archive is a release build, as the builds of each day: it packs fast, and it
        // has the same binaries that the tests ran. `--max-perf` is for a final release.
        "dist" => cmd_dist(&root, if matches!(mode, BuildMode::MaxPerf) { BuildMode::MaxPerf } else { BuildMode::Release }),
        "deploy" => cmd_deploy(&root, mode, morrowind_dir),
        "compdb" => cmd_compdb(&root),
        _ => print_usage(),
    }
}

fn print_usage() {
    eprintln!(concat!(
        "Usage: cargo xtask <command> [--release | --max-perf] [--morrowind-dir <path>]\n\n",
        "Commands:\n",
        "  build     Build all artifacts and mirror assets/ into bin/MSVC-<config>/\n",
        "  dist      Build --release (or --max-perf) and pack a release archive into dist/\n",
        "  deploy    Copy binaries, PDBs, and assets into the Morrowind directory\n",
        "  compdb    Generate compile_commands.json for clangd/Serena\n\n",
        "Formatting and linting are plain cargo; the 32-bit crates carry their own\n",
        "forced-target, so no wrapper is needed:\n",
        "  cargo fmt --all\n",
        "  cargo clippy --workspace -- -D warnings\n\n",
        "Build modes:\n",
        "  (default)   dev profile\n",
        "  --release   release / release-dll profiles\n",
        "  --max-perf  max-perf / max-perf-dll profiles (codegen-units=1 + fat LTO; slow)\n",
        "`dist` builds --release into MGE-XE-G7-v<version>-test.7z; with --max-perf it\n",
        "builds a final release into MGE-XE-G7-v<version>.7z (slow)."
    ));
}

#[derive(Clone, Copy)]
enum BuildMode {
    Dev,
    Release,
    MaxPerf,
}

impl BuildMode {
    /// Cargo profile for the 32-bit DLLs, which keep their PDBs.
    fn dll_profile(self) -> &'static str {
        match self {
            BuildMode::Dev => "dev",
            BuildMode::Release => "release-dll",
            BuildMode::MaxPerf => "max-perf-dll",
        }
    }

    /// Cargo profile for the x64 executables.
    fn exe_profile(self) -> &'static str {
        match self {
            BuildMode::Dev => "dev",
            BuildMode::Release => "release",
            BuildMode::MaxPerf => "max-perf",
        }
    }

    /// Name used for the `bin\MSVC-*` output directory.
    fn config_name(self) -> &'static str {
        match self {
            BuildMode::Dev => "Debug",
            BuildMode::Release => "Release",
            BuildMode::MaxPerf => "MaxPerf",
        }
    }
}

/// Cargo writes `dev`-profile output to `target/<triple>/debug`; every other
/// profile's directory matches its name.
fn target_subdir(profile: &str) -> &str {
    if profile == "dev" { "debug" } else { profile }
}

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest.parent().unwrap().to_path_buf()
}

const WIN32_TARGET: &str = "i686-pc-windows-msvc";
const WINDOWS_HOST: &str = "x86_64-pc-windows-msvc";

fn cmd_compdb(root: &Path) {
    let d3d8_cpp = root.join("d3d8/cpp");
    let dinput8_cpp = root.join("dinput8/cpp");
    let dxsdk = resolve_dxsdk();
    let analysis_out = root.join("target/compdb");
    fs::create_dir_all(&analysis_out).unwrap_or_else(|e| {
        panic!(
            "failed to create compilation-database scratch directory {}: {e}",
            analysis_out.display()
        )
    });

    let d3d8_build = d3d8_analysis_build(&d3d8_cpp, &dxsdk, &analysis_out);
    let dinput8_build = dinput8_analysis_build(&analysis_out);

    let mut sources = Vec::new();
    collect_cpp_files(&d3d8_cpp, &mut sources);
    collect_cpp_files(&dinput8_cpp, &mut sources);
    sources.sort();

    let mut entries = Vec::with_capacity(sources.len());
    for source in sources {
        let build = if source.starts_with(&d3d8_cpp) {
            &d3d8_build
        } else {
            &dinput8_build
        };
        entries.push(serde_json::json!({
            "directory": root.to_string_lossy(),
            "file": source.to_string_lossy(),
            "arguments": analysis_arguments(build, &source),
        }));
    }

    let output = root.join("compile_commands.json");
    let json = serde_json::to_string_pretty(&entries).expect("compilation-database entries should always serialize");
    fs::write(&output, format!("{json}\n")).unwrap_or_else(|e| panic!("failed to write {}: {e}", output.display()));
    println!("Generated {} compilation commands at {}", entries.len(), output.display());
}

fn analysis_build(analysis_out: &Path) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .target(WIN32_TARGET)
        .host(WINDOWS_HOST)
        .out_dir(analysis_out)
        .opt_level(0)
        .debug(true)
        .cargo_metadata(false);
    build
}

fn d3d8_analysis_build(cpp_dir: &Path, dxsdk: &Path, analysis_out: &Path) -> cc::Build {
    let mut build = analysis_build(analysis_out);
    build
        .flag("/EHsc")
        .flag("/arch:SSE2")
        .flag("/fp:fast")
        .flag("/GR-")
        .flag("/Z7")
        .define("WIN32", None)
        .define("NOMINMAX", None)
        .define("_WINDOWS", None)
        .define("_DEBUG", None)
        .include(cpp_dir)
        .include(dxsdk.join("include"));
    build
}

fn dinput8_analysis_build(analysis_out: &Path) -> cc::Build {
    let mut build = analysis_build(analysis_out);
    build
        .flag("/EHsc")
        .flag("/GR-")
        .define("WIN32", None)
        .define("DINPUT8_EXPORTS", None)
        .define("_WINDOWS", None)
        .define("_USRDLL", None)
        .define("_DEBUG", None);
    build
}

fn analysis_arguments(build: &cc::Build, source: &Path) -> Vec<String> {
    let tool = build.get_compiler();
    let mut arguments = Vec::with_capacity(tool.args().len() + 4);
    arguments.push(tool.path().to_string_lossy().into_owned());
    arguments.extend(tool.args().iter().map(|arg| arg.to_string_lossy().into_owned()));

    // clangd does not execute this command, but it does use the explicit target
    // to model pointer widths and ABI-sensitive Win32 layouts correctly.
    arguments.push(format!("--target={WIN32_TARGET}"));
    arguments.push("/c".to_string());
    arguments.push(source.to_string_lossy().into_owned());
    arguments
}

fn collect_cpp_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to enumerate C++ sources under {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("failed to read an entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect_cpp_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "cpp") {
            out.push(path);
        }
    }
}

fn resolve_dxsdk() -> PathBuf {
    if let Ok(dir) = env::var("DXSDK_DIR") {
        let path = PathBuf::from(dir);
        if path.join("include/d3dx9.h").exists() {
            return path;
        }
    }

    let default = PathBuf::from(r"C:\Program Files (x86)\Microsoft DirectX SDK (June 2010)");
    if default.join("include/d3dx9.h").exists() {
        return default;
    }

    panic!(
        "DirectX SDK (June 2010) not found. Set DXSDK_DIR to its root or install it at {}",
        default.display()
    );
}

fn cargo() -> Command {
    Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("failed to spawn cargo");
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
}

/// A `--config` value for cargo that keeps the folders of this machine out of the binaries.
/// rustc writes the path of each source file into a binary, for the messages of a panic.
/// The registry of cargo and the toolchain are in the folder of the user, so each binary
/// had the Windows user name in it. The value adds two `--remap-path-prefix` flags to the
/// rustflags of `triple`: the folder of the user becomes `~`, and the workspace becomes
/// `mge-xe`. Cargo joins the list to the one of `.cargo/config.toml`; the environment
/// variable RUSTFLAGS would replace that list.
fn remap_config(root: &Path, triple: &str) -> String {
    let mut flags = Vec::new();
    if let Ok(home) = env::var("USERPROFILE") {
        flags.push(format!("'--remap-path-prefix={home}=~'"));
    }
    // After the folder of the user: rustc uses the last prefix that fits a path.
    flags.push(format!("'--remap-path-prefix={}=mge-xe'", root.display()));
    format!("target.{triple}.rustflags=[{}]", flags.join(", "))
}

fn cmd_build(root: &Path, mode: BuildMode) {
    let dll_profile = mode.dll_profile();
    let exe_profile = mode.exe_profile();

    let mut dll_cmd = cargo();
    dll_cmd
        .arg("build")
        .arg("--config")
        .arg(remap_config(root, "i686-pc-windows-msvc"))
        .arg("-p")
        .arg("d3d8")
        .arg("-p")
        .arg("dinput8")
        .arg("--target")
        .arg("i686-pc-windows-msvc")
        .arg("--profile")
        .arg(dll_profile);
    if matches!(mode, BuildMode::MaxPerf) {
        // Cargo's lto/codegen-units do not reach the C++; this is what turns on
        // /GL + /LTCG so `--max-perf` actually affects d3d8.dll.
        dll_cmd.arg("--features").arg("d3d8/max-perf");
    }
    dll_cmd.current_dir(root);
    run(&mut dll_cmd);

    let mut exe_cmd = cargo();
    exe_cmd
        .arg("build")
        .arg("--config")
        .arg(remap_config(root, "x86_64-pc-windows-msvc"))
        .arg("-p")
        .arg("MGEXEgui")
        .arg("-p")
        .arg("mgeHost64")
        .arg("--target")
        .arg("x86_64-pc-windows-msvc")
        .arg("--profile")
        .arg(exe_profile)
        .current_dir(root);
    run(&mut exe_cmd);

    let out_dir = root.join("bin").join(format!("MSVC-{}", mode.config_name()));
    fs::create_dir_all(&out_dir).unwrap();

    let dll_target_dir = root.join("target/i686-pc-windows-msvc").join(target_subdir(dll_profile));
    let exe_target_dir = root.join("target/x86_64-pc-windows-msvc").join(target_subdir(exe_profile));

    let artifacts: &[(&Path, &str)] = &[
        (&dll_target_dir, "d3d8.dll"),
        (&dll_target_dir, "d3d8.pdb"),
        (&dll_target_dir, "dinput8.dll"),
        (&dll_target_dir, "dinput8.pdb"),
        (&exe_target_dir, "numidium.exe"),
        (&exe_target_dir, "numidium.pdb"),
        (&exe_target_dir, "MGEXEgui.exe"),
    ];

    for (dir, name) in artifacts {
        let src = dir.join(name);
        if src.exists() {
            fs::copy(&src, out_dir.join(name)).unwrap();
        }
    }

    // bin\MSVC-* doubles as the shareable archive, so it carries the asset tree
    // next to the binaries. mirror_dir only rewrites files that actually changed.
    mirror_dir(&root.join("assets"), &out_dir);
    copy_readme(root, &out_dir);
    // The nirender plugin, when its release build is there (dist builds it).
    if collect_plugin(root, &out_dir, false) {
        println!("nirender plugin collected from {}", plugin_dir(root).display());
    }

    println!("Artifacts and assets collected into {}", out_dir.display());
}

/// The repository of the nirender plugin: the sibling folder `nirender-plugin`, or the one
/// that NIRENDER_PLUGIN_DIR names; a relative value is from the workspace root, as in
/// d3d8/build.rs, so that the two read one folder.
fn plugin_dir(root: &Path) -> PathBuf {
    env::var("NIRENDER_PLUGIN_DIR").map(|value| root.join(value)).unwrap_or_else(|_| root.parent().unwrap().join("nirender-plugin"))
}

/// The Lua files of the nirender mod that a player's install needs: `main.lua` and the files
/// that it loads with `dofile`. The other Lua files of the mod are its test harness.
const SHIPPED_LUA: [&str; 3] = ["main.lua", "failure.lua", "notifier.lua"];

/// The names of the mod's files that `main.lua` loads with `dofile`
/// (`"Data Files\\MWSE\\mods\\nirender\\<name>.lua"` in its source).
fn loaded_lua(main: &str) -> Vec<String> {
    let mark = r"mods\\nirender\\";
    main.match_indices(mark)
        .filter_map(|(at, _)| {
            let rest = &main[at + mark.len()..];
            rest.find(".lua").map(|end| format!("{}.lua", &rest[..end]))
        })
        .collect()
}

/// The nirender plugin into the output, as the MWSE mod that the wgpu renderer needs: the
/// DLL under `Data Files/MWSE/lib` and the files of [`SHIPPED_LUA`] under
/// `Data Files/MWSE/mods/nirender`. A file that `main.lua` loads and the list does not name
/// stops the build, so that an archive cannot miss it. With `build`, the release build of
/// the plugin runs first. Returns false when the plugin is not there.
fn collect_plugin(root: &Path, out_dir: &Path, build: bool) -> bool {
    let plugin = plugin_dir(root);
    if !plugin.join("Cargo.toml").exists() {
        return false;
    }
    if build {
        run(cargo().arg("build").arg("--release").current_dir(&plugin));
    }
    let dll = plugin.join("target/i686-pc-windows-msvc/release/akulakhan.dll");
    let source = plugin.join("mod/MWSE/mods/nirender");
    if !dll.exists() || !source.join("main.lua").exists() {
        return false;
    }
    let main = fs::read_to_string(source.join("main.lua")).unwrap();
    for name in loaded_lua(&main) {
        assert!(SHIPPED_LUA.contains(&name.as_str()), "main.lua loads {name}, which SHIPPED_LUA of xtask does not name");
    }
    let lib = out_dir.join("Data Files/MWSE/lib");
    let mods = out_dir.join("Data Files/MWSE/mods/nirender");
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(&mods).unwrap();
    fs::copy(&dll, lib.join("akulakhan.dll")).unwrap();
    for name in SHIPPED_LUA {
        fs::copy(source.join(name), mods.join(name)).unwrap();
    }
    true
}

/// 7-Zip: on the PATH, or at its usual place.
fn seven_zip() -> PathBuf {
    let usual = PathBuf::from(r"C:\Program Files\7-Zip\7z.exe");
    if usual.exists() { usual } else { PathBuf::from("7z") }
}

/// The release archive: the binaries of `mode` plus assets, packed flat so it
/// extracts straight into a Morrowind directory.
///
/// `mode` is `Release` or `MaxPerf`. `MaxPerf` (fat LTO, one codegen unit) takes some
/// minutes more for each archive and is for a final release.
fn cmd_dist(root: &Path, mode: BuildMode) {
    cmd_build(root, mode);

    // Stage into a directory rebuilt from scratch rather than packing bin\MSVC-*
    // directly. `mirror_dir` only ever adds, so that directory keeps files that
    // assets/ has since dropped; staging fresh is what keeps them out of a release.
    let staging = root.join("target/dist-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging).unwrap_or_else(|e| panic!("failed to clear {}: {e}", staging.display()));
    }
    fs::create_dir_all(&staging).unwrap();

    // PDBs are deliberately not shipped; crash reports come back as raw addresses.
    let built = root.join("bin").join(format!("MSVC-{}", mode.config_name()));
    for name in ["d3d8.dll", "dinput8.dll", "numidium.exe", "MGEXEgui.exe"] {
        let src = built.join(name);
        if !src.exists() {
            eprintln!("error: {} was not produced by the build", src.display());
            process::exit(1);
        }
        fs::copy(&src, staging.join(name)).unwrap();
    }
    mirror_dir(&root.join("assets"), &staging);
    // The d3d9.dll of the assets is the DXVK build for the Direct3D 9 path. The wgpu
    // renderer does not use it, and a file with this name in an archive replaces the
    // d3d9.dll that a user has. The Direct3D 9 path runs with the d3d9.dll of Windows.
    let dxvk = staging.join("d3d9.dll");
    if dxvk.exists() {
        fs::remove_file(&dxvk).unwrap_or_else(|e| panic!("failed to remove {}: {e}", dxvk.display()));
    }
    copy_readme(root, &staging);
    // The wgpu renderer needs the nirender plugin: the archive carries it.
    if !collect_plugin(root, &staging, true) {
        eprintln!("error: the nirender plugin was not found at {} (set NIRENDER_PLUGIN_DIR)", plugin_dir(root).display());
        process::exit(1);
    }

    let out_dir = root.join("dist");
    fs::create_dir_all(&out_dir).unwrap();
    // The name tells a release build from a max-perf build: nothing else does, when the
    // archive is not on this machine.
    let kind = if matches!(mode, BuildMode::MaxPerf) { "" } else { "-test" };
    let archive = out_dir.join(format!("MGE-XE-G7-v{}{kind}.7z", env!("CARGO_PKG_VERSION")));
    // 7-Zip refuses to overwrite entries in an existing archive the way `a` implies,
    // so a stale file would silently merge with the new one.
    if archive.exists() {
        fs::remove_file(&archive).unwrap_or_else(|e| panic!("failed to replace {}: {e}", archive.display()));
    }

    // Every top-level entry is named explicitly, from inside the staging directory,
    // so the archive holds no wrapper folder and no "./" prefixes.
    let mut pack = Command::new(seven_zip());
    pack.arg("a").arg("-t7z").arg("-mx=9").arg(&archive);
    for entry in fs::read_dir(&staging).unwrap() {
        pack.arg(entry.unwrap().file_name());
    }
    pack.current_dir(&staging);
    let status = pack.status().unwrap_or_else(|e| {
        eprintln!("error: could not run 7z: {e}\n7-Zip must be installed and on PATH.");
        process::exit(1);
    });
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }

    let size = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    println!("Packed {} ({:.1} MiB)", archive.display(), size as f64 / (1024.0 * 1024.0));
}

fn cmd_deploy(root: &Path, mode: BuildMode, morrowind_dir: Option<PathBuf>) {
    let morrowind = morrowind_dir
        .or_else(|| env::var("MGE_XE_MORROWIND_DIR").ok().map(PathBuf::from))
        .or_else(|| resolve_from_local_toml(root))
        .unwrap_or_else(|| {
            eprintln!(
                "error: Morrowind directory not configured.\n\
                 Pass --morrowind-dir <path>, set MGE_XE_MORROWIND_DIR, or create\n\
                 mge-xe-local.toml at the repo root with:\n\
                 \x20 morrowind_dir = \"C:/path/to/Morrowind\""
            );
            process::exit(1);
        });

    cmd_build(root, mode);

    let config = mode.config_name();
    let src_bin = root.join("bin").join(format!("MSVC-{config}"));

    let binaries = [
        "d3d8.dll",
        "d3d8.pdb",
        "dinput8.dll",
        "dinput8.pdb",
        "numidium.exe",
        "numidium.pdb",
        "MGEXEgui.exe",
    ];
    for name in &binaries {
        let src = src_bin.join(name);
        if src.exists() {
            fs::copy(&src, morrowind.join(name)).unwrap();
        }
    }

    mirror_dir(&root.join("assets"), &morrowind);
    copy_readme(root, &morrowind);

    println!("Deployed to {}", morrowind.display());
}

fn resolve_from_local_toml(root: &Path) -> Option<PathBuf> {
    let path = root.join("mge-xe-local.toml");
    let content = fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("morrowind_dir") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(PathBuf::from(val));
                }
            }
        }
    }
    None
}

/// Ship the repo root README next to the binaries, renamed: the destination is a
/// shared Morrowind directory where a bare README.md would collide with other mods.
fn copy_readme(root: &Path, dest: &Path) {
    let readmes = [("README.md", "MGE XE Readme.md"), ("RENDERER.md", "MGE XE Renderer Readme.md"), ("mgeHost64/POST_SHADERS.md", "MGE XE Post Shaders Readme.md")];
    for (name, shipped) in readmes {
        let src = root.join(name);
        if src.exists() {
            fs::copy(&src, dest.join(shipped)).unwrap();
        }
    }
}

/// Copy files from src into dest, creating subdirectories as needed.
/// Skips files that already exist with the same or newer modification time.
fn mirror_dir(src: &Path, dest: &Path) {
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if let Some(target) = link_pointing_outside(&src_path, &dest_path) {
            eprintln!(
                "warning: not updating {} - it links to {}, which is outside the source tree",
                dest_path.display(),
                target.display()
            );
            continue;
        }
        if src_path.is_dir() {
            fs::create_dir_all(&dest_path).unwrap();
            mirror_dir(&src_path, &dest_path);
        } else if should_copy(&src_path, &dest_path) {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&src_path, &dest_path).unwrap();
        }
    }
}

/// `Some(target)` when `dest` is a symlink or junction that does not resolve back
/// to `src`.
///
/// Copying onto such a path would follow the link and truncate whatever sits at the
/// far end, in a directory the user never named. Links that do resolve back to `src`
/// are the ordinary "edit the asset in place" convenience setup, so they stay on the
/// normal path, where the timestamp/size comparison skips them anyway.
///
/// Hard links are indistinguishable from regular files here and are still followed.
fn link_pointing_outside(src: &Path, dest: &Path) -> Option<PathBuf> {
    if !fs::symlink_metadata(dest).ok()?.file_type().is_symlink() {
        return None;
    }
    let resolves_to_src = match (fs::canonicalize(src), fs::canonicalize(dest)) {
        (Ok(src), Ok(dest)) => src == dest,
        // A link that cannot be resolved - broken, or a target we cannot open - is
        // precisely the case not to write through.
        _ => false,
    };
    if resolves_to_src {
        return None;
    }
    Some(fs::read_link(dest).unwrap_or_else(|_| dest.to_path_buf()))
}

fn should_copy(src: &Path, dest: &Path) -> bool {
    let src_meta = match fs::metadata(src) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let dest_meta = match fs::metadata(dest) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let src_time = src_meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let dest_time = dest_meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    src_time > dest_time || src_meta.len() != dest_meta.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_that_main_lua_loads_are_found_and_shipped() {
        let main = "dofile(\"Data Files\\\\MWSE\\\\mods\\\\nirender\\\\notifier.lua\")\n\
                    dofile(\"Data Files\\\\MWSE\\\\mods\\\\nirender\\\\failure.lua\")\n";
        assert_eq!(loaded_lua(main), vec!["notifier.lua", "failure.lua"]);
        assert!(loaded_lua("print(1)").is_empty());
    }

    #[test]
    fn the_plugins_main_lua_loads_only_shipped_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let main = plugin_dir(root).join("mod/MWSE/mods/nirender/main.lua");
        let Ok(text) = fs::read_to_string(&main) else { return };
        for name in loaded_lua(&text) {
            assert!(SHIPPED_LUA.contains(&name.as_str()), "main.lua loads {name}");
        }
    }
}
