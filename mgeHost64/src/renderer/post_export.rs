//! The export of an embedded port as a `.wgsl` file of a pack (W609; D640 piece 2, step 2; D641).
//!
//! A pack is a small mod `<Mod> - wgpu shaders`, placed after its mod, with
//! `shaders/XEshaders/<name>.wgsl`. The sources of the packs are in `ports/` of this repo; the
//! release does not ship them (`cargo xtask dist` packs the binaries, `assets/`, three readmes
//! and the plugin).
//!
//! The file of a port is the `//!` header of its Definition, then its source as it is. The
//! Definition is the one that the keyed path makes for the mod's `.fx` ([`keyed_definition`]):
//! the port with the file's numbers, variable defaults and priority. A `.wgsl` is read before
//! the `.fx` and skips that path, so the file must carry its result.
//!
//! - `each_port_gives_its_definition_back`: every embedded port, exported and parsed with
//!   [`Definition::of_file`], gives the same Definition.
//! - `each_pack_file_is_its_port`: each file under `ports/` parses, and it is the port of a
//!   `MOD_FILES` entry of its name, but for the numbers that a mod's file sets. A file whose
//!   name has no entry is a pack with no port, and is not compared. A name in `SAME_PORT` is
//!   compared with the port of its entry there. A file in a folder of `XEshaders` has the folder
//!   in its name (`InspectIt/Bokeh`). Our package's `assets/Data Files/shaders/XEshaders` is read
//!   as a pack too (the pack `MGE XE package` of the list): a name with no entry there is compared
//!   with the port of its name while that port is built in.
//! - `export_packs` (ignored): writes the files from the mods' `.fx` copies. The list file named
//!   by `W609_PACKS` has one line for each copy: `<pack folder>\t<chain name>\t<.fx path>`. The
//!   path `-` means no copy of the mod's file: the port with its own numbers.
//!   Without `W609_PACKS` it writes nothing and passes (the merge step runs all ignored tests). All
//!   copies of a name in a pack must give one Definition, else the test names them and fails.
//!   `cargo test -p mgeHost64 --release export_packs -- --ignored --nocapture`

use super::*;

/// The categories of the `category` key and their part of the priority (see `of_file`).
const CATEGORIES: &[(&str, i64)] = &[
    ("scene", 1_000_000),
    ("atmosphere", 2_000_000),
    ("lens", 3_000_000),
    ("sensor", 4_000_000),
    ("tone", 5_000_000),
    ("none", 6_000_000),
    ("final", 9_000_000),
];

/// The category and the adjust of a priority: the nearest category.
fn category_of(priority: i64) -> (&'static str, i64) {
    let (name, part) = CATEGORIES
        .iter()
        .min_by_key(|(_, part)| (priority - part).abs())
        .expect("the list of categories is not empty");
    (name, priority - part)
}

/// The `//!` item of a variable: `name type`, `name type[N]`, then ` = values` when it has values.
fn variable_item(variable: &VariableDef) -> String {
    let kind = match variable.per {
        1 => "f32",
        2 => "vec2",
        3 => "vec3",
        _ => "vec4",
    };
    let mut item = format!("{} {kind}", variable.name);
    if variable.slots != 1 {
        item.push_str(&format!("[{}]", variable.slots));
    }
    if !variable.default.is_empty() {
        let values: Vec<String> = variable.default.iter().map(|v| v.to_string()).collect();
        item.push_str(&format!(" = {}", values.join(" ")));
    }
    item
}

/// The text of the file of a Definition: the `//!` lines, then the source.
pub(super) fn file_text(definition: &Definition) -> String {
    let passes: Vec<String> = definition
        .passes
        .iter()
        .map(|(entry, scale, half_float)| {
            let mut item = entry.clone();
            if *scale != 1 {
                item.push_str(&format!(" /{scale}"));
            }
            if *half_float {
                item.push_str(" f16");
            }
            item
        })
        .collect();
    let (category, adjust) = category_of(definition.priority);
    let mut lines = vec![
        "interface: mge-xe-wgsl 1".to_string(),
        format!("passes: {}", passes.join(", ")),
        format!("category: {category}"),
    ];
    if adjust != 0 {
        lines.push(format!("priority_adjust: {adjust}"));
    }
    if definition.disable_flags != 0 {
        lines.push(format!("mgeflags: {}", definition.disable_flags));
    }
    if definition.requires_hdr {
        lines.push("requires_hdr: true".to_string());
    }
    if definition.no_sun_glare {
        lines.push("disable_sunglare: true".to_string());
    }
    if !definition.textures.is_empty() {
        lines.push(format!("texture: {}", definition.textures.join(", ")));
    }
    if !definition.variables.is_empty() {
        let items: Vec<String> = definition.variables.iter().map(variable_item).collect();
        lines.push(format!("variables: {}", items.join(", ")));
    }
    let mut text: String = lines.iter().map(|line| format!("//! {line}\n")).collect();
    text.push_str(&definition.source);
    text
}

/// A file's text without its `//!` lines, with LF line ends.
fn body(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut rest = text.as_str();
    while rest.starts_with("//!") {
        rest = rest.split_once('\n').map_or("", |(_, after)| after);
    }
    rest.to_string()
}

/// A file as [`Definition::of_file`] reads it, with the source after the header: the part that
/// a port's source is compared with.
fn of_pack_file(name: &str, text: &str) -> Result<Definition, String> {
    let mut definition = Definition::of_file(name, text)?;
    definition.source = body(&definition.source);
    Ok(definition)
}

/// The folder of the packs' sources: `ports/` at the root of the repo.
fn ports_folder() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("ports")
}

/// The pack name of a list line that writes into our package, not into `ports/`: the stock
/// shaders and the S_A shaders (D640 piece 2, step 3; D652).
const PACKAGE: &str = "MGE XE package";

/// The `Data Files` of our package: `assets/Data Files` of the repo.
pub(super) fn package_folder() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("assets")
        .join("Data Files")
}

/// The folder whose `shaders/XEshaders` a pack of the list writes into.
fn pack_root(pack: &str) -> std::path::PathBuf {
    if pack == PACKAGE {
        package_folder()
    } else {
        ports_folder().join(pack)
    }
}

/// The ports that a file of the chain name `name` may be: those of its `MOD_FILES` entries, or,
/// for a name with no entry, the port of that name (a stock or S_A shader). None: a shader with
/// no port, whose file is the shader (Eternal Shine).
fn port_names(name: &str) -> Vec<&'static str> {
    let keyed = keyed_name(name);
    let keyed_ports: Vec<&'static str> = MOD_FILES
        .iter()
        .filter(|(mod_name, _, _)| *mod_name == keyed)
        .map(|(_, _, port)| *port)
        .collect();
    if !keyed_ports.is_empty() {
        return keyed_ports;
    }
    ports().filter(|port| port.name == name).map(|port| port.name).collect()
}

#[test]
fn each_port_gives_its_definition_back() {
    let mut count = 0;
    for port in ports() {
        let definition = Definition::of_port(port);
        let text = file_text(&definition);
        let back = of_pack_file(port.name, &text).unwrap_or_else(|problem| panic!("{}: {problem}\n{text}", port.name));
        assert_eq!(back, definition, "the port {} does not come back from its file", port.name);
        count += 1;
    }
    assert!(count > 50, "only {count} ports");
}

/// The null of the test above: a change of one field shows.
#[test]
fn a_changed_field_shows() {
    let port = ports().find(|port| port.name == "ashfall").expect("the port ashfall");
    let definition = Definition::of_port(port);
    let text = file_text(&definition).replace("priority_adjust: 40", "priority_adjust: 41");
    assert_ne!(of_pack_file("ashfall", &text).unwrap(), definition);
    let text = file_text(&definition).replace("temperature f32 = 20", "temperature f32 = 21");
    assert_ne!(of_pack_file("ashfall", &text).unwrap(), definition);
}

/// The lines of a source with the values of its `// file` constants left out.
fn without_file_numbers(source: &str) -> Vec<String> {
    source
        .lines()
        .map(
            |line| match line.trim_end().strip_suffix("// file").and_then(|code| code.split_once('=')) {
                Some((left, _)) => format!("{left}= <file number>; // file"),
                None => line.to_string(),
            },
        )
        .collect()
}

/// Whether `file` is the port `port` with numbers that a mod's file may set: the `// file`
/// constants, the defaults of the variables of one value, and the priority.
fn is_port_with_file_numbers(file: &Definition, port: &Definition) -> bool {
    let shape = |d: &Definition| {
        let variables: Vec<(String, u32, u32, Option<Vec<f32>>)> = d
            .variables
            .iter()
            .map(|v| {
                let fitted = v.slots == 1 && v.per == 1;
                (v.name.clone(), v.slots, v.per, (!fitted).then(|| v.default.clone()))
            })
            .collect();
        (
            d.passes.clone(),
            d.disable_flags,
            d.no_sun_glare,
            d.textures.clone(),
            d.requires_hdr,
            variables,
        )
    };
    shape(file) == shape(port) && without_file_numbers(&file.source) == without_file_numbers(&port.source)
}

/// The names that a mod loads a shader by, with no `MOD_FILES` entry, and the `MOD_FILES` name
/// whose port its file is: Modern Lockpicking loads Inspect It's `Bokeh.fx` (the same file) by
/// its own name.
const SAME_PORT: &[(&str, &str)] = &[("modern-lockpicking/Bokeh", "InspectIt/Bokeh")];

/// The `MOD_FILES` name of the port of the shader that a mod loads as `name`.
fn keyed_name(name: &str) -> &str {
    SAME_PORT
        .iter()
        .find(|(load_name, _)| *load_name == name)
        .map_or(name, |(_, keyed)| *keyed)
}

/// The `.wgsl` files of a pack's `shaders/XEshaders`, with their names as a chain gives them:
/// a file in a folder of its own (`InspectIt/Bokeh.wgsl`) has the folder in its name.
pub(super) fn pack_post_files(pack: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let folder = pack.join("shaders").join("XEshaders");
    let mut found = Vec::new();
    let mut folders = vec![(String::new(), folder)];
    while let Some((prefix, folder)) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                folders.push((format!("{prefix}{name}/"), path));
            } else if path.extension().is_some_and(|extension| extension == "wgsl") {
                found.push((format!("{prefix}{stem}"), path));
            }
        }
    }
    found
}

#[test]
fn each_pack_file_is_its_port() {
    let packs = std::fs::read_dir(ports_folder()).unwrap_or_else(|e| panic!("{}: {e}", ports_folder().display()));
    // A pack with no post shader (only a core mod) has no XEshaders folder; the count shows that the walk found files.
    // Our package's XEshaders is read as a pack too.
    let mut roots: Vec<std::path::PathBuf> = packs
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    roots.push(package_folder());
    let mut checked = 0;
    for root in roots {
        for (name, path) in pack_post_files(&root) {
            let candidates = port_names(&name);
            // A pack with no port (Eternal Shine): its file is the shader. The test
            // `each_post_file_of_the_packs_is_valid` of post.rs reads it.
            if candidates.is_empty() {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let definition = of_pack_file(&name, &text).unwrap_or_else(|problem| panic!("{}: {problem}", path.display()));
            let matches: Vec<&str> = candidates
                .iter()
                .filter_map(|port_name| ports().find(|port| port.name == *port_name))
                .filter(|port| {
                    let mut of_port = Definition::of_port(port);
                    of_port.name = name.clone();
                    is_port_with_file_numbers(&definition, &of_port)
                })
                .map(|port| port.name)
                .collect();
            assert_eq!(matches.len(), 1, "{}: the ports that it is: {matches:?}", path.display());
            checked += 1;
        }
    }
    assert!(
        checked >= 1,
        "no .wgsl file in {}/*/shaders/XEshaders",
        ports_folder().display()
    );
}

/// The walk of a pack finds a file in a folder of `XEshaders`, by its name with the folder.
#[test]
fn a_file_in_a_folder_has_the_folder_in_its_name() {
    let pack = std::env::temp_dir().join(format!("w609-pack-walk-{}", std::process::id()));
    let folder = pack.join("shaders").join("XEshaders");
    std::fs::create_dir_all(folder.join("InspectIt")).unwrap();
    std::fs::write(folder.join("top.wgsl"), "").unwrap();
    std::fs::write(folder.join("InspectIt").join("Bokeh.wgsl"), "").unwrap();
    std::fs::write(folder.join("InspectIt").join("Bokeh.fx"), "").unwrap();
    let mut names: Vec<String> = pack_post_files(&pack).into_iter().map(|(name, _)| name).collect();
    names.sort();
    for file in [
        folder.join("top.wgsl"),
        folder.join("InspectIt").join("Bokeh.wgsl"),
        folder.join("InspectIt").join("Bokeh.fx"),
    ] {
        std::fs::remove_file(file).unwrap();
    }
    for empty in [folder.join("InspectIt"), folder.clone(), pack.join("shaders"), pack.clone()] {
        std::fs::remove_dir(empty).unwrap();
    }
    assert_eq!(names, ["InspectIt/Bokeh", "top"]);
    assert_eq!(keyed_name("modern-lockpicking/Bokeh"), "InspectIt/Bokeh");
    assert_eq!(keyed_name("InspectIt/Bokeh"), "InspectIt/Bokeh");
}

/// The stock "Bloom Soft" is a file of our package since W609 step 3, but its port stays: the
/// key of MGG's `Bloom Soft Sky.fx` finds it (until step 5), and MGG's pack file is that port.
#[test]
fn the_key_of_mgg_bloom_soft_sky_finds_the_bloom_soft_port() {
    let port = MOD_FILES
        .iter()
        .find(|(name, key, _)| *name == "Bloom Soft Sky" && *key == 0x1b83d108acc2ff62)
        .map(|(_, _, port)| *port);
    assert_eq!(port, Some("Bloom Soft"));
    let bloom_soft = ports().find(|port| port.name == "Bloom Soft").expect("the port Bloom Soft");
    let path = ports_folder()
        .join("Morrowind Graphics Guide Shaders - wgpu shaders")
        .join("shaders")
        .join("XEshaders")
        .join("Bloom Soft Sky.wgsl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file = of_pack_file("Bloom Soft Sky", &text).unwrap();
    let mut of_port = Definition::of_port(bloom_soft);
    of_port.name = "Bloom Soft Sky".to_string();
    assert!(is_port_with_file_numbers(&file, &of_port));
}

/// The null of `each_pack_file_is_its_port`: a changed line of code shows.
#[test]
fn a_changed_line_is_not_the_port() {
    let port = ports().find(|port| port.name == "ashfall").expect("the port ashfall");
    let definition = Definition::of_port(port);
    let mut changed = Definition::of_port(port);
    changed.source = changed.source.replacen("return", "return 0.5 *", 1);
    assert!(is_port_with_file_numbers(&definition, &definition));
    assert!(!is_port_with_file_numbers(&changed, &definition));
}

#[test]
#[ignore = "writes the packs from the mods' files: W609_PACKS names the list"]
fn export_packs() {
    // The merge step runs every ignored test: without the list, this is no export, and it passes.
    let Ok(list) = std::env::var("W609_PACKS") else {
        println!("export_packs: W609_PACKS is not set, so no pack is written");
        return;
    };
    let text = std::fs::read_to_string(&list).expect("the list file");
    // (pack, name) -> [(path, Definition)], in the order of the list.
    let mut groups: Vec<((String, String), Vec<(String, Definition)>)> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.split('\t');
        let (Some(pack), Some(name), Some(path), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            panic!("the line `{line}` is not `pack<TAB>name<TAB>path`");
        };
        let keyed = keyed_name(name);
        let has_entry = MOD_FILES.iter().any(|(mod_name, _, _)| *mod_name == keyed);
        let mut definition = if path == "-" {
            // No copy of the mod's file: the port with its own numbers.
            let port = *port_names(name)
                .first()
                .unwrap_or_else(|| panic!("{name}: no MOD_FILES entry of {keyed} and no port of that name"));
            println!("export: {pack} | {name} | port {port} | no file");
            keyed_definition(port, None).expect("the port of a MOD_FILES entry")
        } else {
            let effect =
                String::from_utf8_lossy(&std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))).into_owned();
            let key = code_key(&effect);
            // A name with no MOD_FILES entry is a stock shader: the port of its name takes the
            // file's numbers, as the keyed path does for the stock file.
            let port = if has_entry {
                MOD_FILES
                    .iter()
                    .find(|(mod_name, mod_key, _)| *mod_name == keyed && *mod_key == key)
                    .map(|(_, _, port)| *port)
                    .unwrap_or_else(|| panic!("{path}: no MOD_FILES entry of {keyed} with the code key {key:#018x}"))
            } else {
                *port_names(name)
                    .first()
                    .unwrap_or_else(|| panic!("{path}: no MOD_FILES entry of {keyed} and no port of that name"))
            };
            println!("export: {pack} | {name} | port {port} | key {key:#018x} | {path}");
            keyed_definition(keyed, Some(&effect)).expect("a port of a MOD_FILES entry")
        };
        definition.name = name.to_string();
        let at = (pack.to_string(), name.to_string());
        match groups.iter_mut().find(|(group, _)| *group == at) {
            Some((_, copies)) => copies.push((path.to_string(), definition)),
            None => groups.push((at, vec![(path.to_string(), definition)])),
        }
    }
    let mut differ = Vec::new();
    for ((pack, name), copies) in &groups {
        let first = &copies[0];
        let others: Vec<String> = copies[1..]
            .iter()
            .filter(|(_, definition)| *definition != first.1)
            .map(|(path, _)| format!("{pack} | {name}: {path} gives another Definition than {}", first.0))
            .collect();
        if !others.is_empty() {
            differ.extend(others);
            continue;
        }
        let text = file_text(&first.1);
        assert_eq!(
            of_pack_file(name, &text).unwrap(),
            first.1,
            "{name}: the file does not give its Definition back"
        );
        // A name with a folder (`InspectIt/Bokeh`) writes into that folder.
        let path = pack_root(pack).join("shaders").join("XEshaders").join(format!("{name}.wgsl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        println!(
            "export: wrote {} ({} copies, {} bytes)",
            path.display(),
            copies.len(),
            text.len()
        );
    }
    assert!(differ.is_empty(), "copies with another Definition:\n{}", differ.join("\n"));
}
