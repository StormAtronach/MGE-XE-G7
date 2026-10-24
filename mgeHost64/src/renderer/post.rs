//! The post-processing chain of MGE XE: the shaders of `[shaders] chain`, ported by hand to
//! WGSL, run on the picture of the frame after the world views and before the user
//! interface of the game. The port specification is in
//! `moreFPS/docs/plans/mge-xe-post-process-spec.md`.
//!
//! A shader is a WGSL module with one fragment function for each pass; each pass draws a
//! full-screen triangle into a texture of its own: `rgba8unorm` of the size of the picture
//! without a word, or a fraction of the size and 16-bit floats when the pass asks for them
//! (a pool of textures by size and format serves all the shaders of the chain).
//! `lastshader` is the input of the shader (the picture, or the output of the shader before),
//! `lastpass` the output of the pass before, and `pass0` to `pass7` the outputs of the first
//! eight passes of the shader. The variables of MGE XE are one uniform.

use nirender_wgpu::{SceneView, wgpu};

/// The variables that MGE XE gives a shader (section 2 of the specification), as one
/// uniform. The matrices are the rows of Direct3D, so that `mview[row][col]` of HLSL is
/// `u.mview[row][col]` here.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Variables {
    pub mview: [[f32; 4]; 4],
    pub mproj: [[f32; 4]; 4],
    /// xyz, and `sunvis` in w.
    pub eyevec: [f32; 4],
    /// xyz, and `fov` (degrees) in w.
    pub eyepos: [f32; 4],
    /// xyz, and `time` in w.
    pub sunvec: [f32; 4],
    /// xyz, and `waterlevel` in w.
    pub suncol: [f32; 4],
    /// xyz, and `isInterior` in w.
    pub sunamb: [f32; 4],
    /// xyz, and `isUnderwater` in w.
    pub sunpos: [f32; 4],
    pub hdr: [f32; 4],
    /// xyz, and `fogstart` in w.
    pub fogcol: [f32; 4],
    /// xyz, and `fogrange` in w.
    pub fognearcol: [f32; 4],
    /// `fognearstart`, `fognearrange`, and `rcpres`.
    pub fog_near: [f32; 4],
    /// x: 1 with the exponential fog (the macro `USE_EXPFOG` of an effect file).
    pub switches: [f32; 4],
}

/// The part of each shader module that every shader has: the variables, the textures, the
/// samplers, the quad, and the helpers of HLSL that WGSL does not have.
const COMMON: &str = "
struct Variables {
    mview: mat4x4f,
    mproj: mat4x4f,
    eyevec: vec4f,
    eyepos: vec4f,
    sunvec: vec4f,
    suncol: vec4f,
    sunamb: vec4f,
    sunpos: vec4f,
    hdr: vec4f,
    fogcol: vec4f,
    fognearcol: vec4f,
    fog_near: vec4f,
    switches: vec4f,
}
// Group 0, the frame: the same for all shaders and passes of the frame.
@group(0) @binding(0) var<uniform> u: Variables;
@group(0) @binding(1) var depthframe: texture_2d<f32>;
@group(0) @binding(2) var watertexture: texture_3d<f32>;
@group(0) @binding(3) var point_clamp: sampler;
@group(0) @binding(4) var linear_clamp: sampler;
@group(0) @binding(5) var linear_wrap: sampler;
@group(0) @binding(6) var linear_mirror: sampler;
@group(0) @binding(7) var point_wrap: sampler;
@group(0) @binding(8) var point_mirror: sampler;

// Group 1, the shader: made when the shader is loaded.
// The variables of the shader that a mod sets from Lua. The renderer makes a function for
// each one: `name()`, or `name(i)` for an array.
struct ModValues {
    values: array<vec4f, 256>,
}
@group(1) @binding(0) var<uniform> modv: ModValues;
// The textures of the shader (the `src` annotations of its file), white without one.
@group(1) @binding(1) var tex1: texture_2d<f32>;
@group(1) @binding(2) var tex2: texture_2d<f32>;
@group(1) @binding(3) var tex3: texture_2d<f32>;
@group(1) @binding(4) var tex4: texture_2d<f32>;
@group(1) @binding(5) var tex5: texture_2d<f32>;
@group(1) @binding(6) var tex6: texture_2d<f32>;
@group(1) @binding(7) var tex7: texture_2d<f32>;
@group(1) @binding(8) var tex8: texture_2d<f32>;
@group(1) @binding(9) var tex9: texture_2d<f32>;
@group(1) @binding(10) var tex10: texture_2d<f32>;

// Group 2, the pass.
// The size of the target of the pass in xy, and 1 over it in zw.
@group(2) @binding(0) var<uniform> pass_info: vec4f;
@group(2) @binding(1) var lastshader: texture_2d<f32>;
@group(2) @binding(2) var lastpass: texture_2d<f32>;
// The outputs of the first eight passes of the shader; white before they are drawn.
@group(2) @binding(3) var pass0: texture_2d<f32>;
@group(2) @binding(4) var pass1: texture_2d<f32>;
@group(2) @binding(5) var pass2: texture_2d<f32>;
@group(2) @binding(6) var pass3: texture_2d<f32>;
@group(2) @binding(7) var pass4: texture_2d<f32>;
@group(2) @binding(8) var pass5: texture_2d<f32>;
@group(2) @binding(9) var pass6: texture_2d<f32>;
@group(2) @binding(10) var pass7: texture_2d<f32>;

// The rows of the matrices of Direct3D: mview[row][col] of HLSL.

fn mview(row: u32, col: u32) -> f32 {
    return u.mview[row][col];
}
fn mproj(row: u32, col: u32) -> f32 {
    return u.mproj[row][col];
}
// 1 over the size of the picture.
// The macro USE_EXPFOG of an effect file: the distant land has the exponential fog.
fn use_expfog() -> bool {
    return u.switches.x > 0.5;
}

fn rcpres() -> vec2f {
    return u.fog_near.zw;
}
// The size of the target of this pass, and 1 over it.
fn target_size() -> vec2f {
    return pass_info.xy;
}
fn target_rcpres() -> vec2f {
    return pass_info.zw;
}
fn saturate(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}
fn saturate3(x: vec3f) -> vec3f {
    return clamp(x, vec3f(0.0), vec3f(1.0));
}

struct Quad {
    @builtin(position) position: vec4f,
    @location(0) tex: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.tex = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}
";

/// A pass of a shader: its fragment function, and its target: the picture size over `scale`
/// (1, 2, 4, ...), with 8-bit colours or 16-bit floats.
#[derive(Clone)]
pub struct Pass {
    pub entry: &'static str,
    pub scale: u32,
    pub half_float: bool,
}

/// A pass into a texture of the size of the picture with 8-bit colours.
pub(super) const fn full(entry: &'static str) -> Pass {
    Pass { entry, scale: 1, half_float: false }
}

/// A pass into a texture of the picture size over `scale`, with 16-bit floats.
const fn scaled(entry: &'static str, scale: u32) -> Pass {
    Pass { entry, scale, half_float: true }
}

/// The textures that a shader can name: `tex1` to `tex10`.
const TEXTURES: usize = 10;

/// The places of the variables of a shader in the uniform `modv`: 256 vectors of 4 floats.
const MOD_SLOTS: usize = 256;

/// A variable of a port that a mod sets from Lua (`shader.name = value` of MWSE). It has
/// `slots` places of `modv`, with `per` floats in each place: one value (1, 1), a vector
/// (1, 2 to 4), an array of values (N, 1), an array `float name[N][3]` (N, 3). The WGSL of
/// the port reads it with a function that the renderer makes: `name()`, or `name(i)` for
/// an array.
pub struct Variable {
    pub name: &'static str,
    pub slots: u32,
    pub per: u32,
    /// The values before a mod sets the variable, as the effect file has them.
    pub default: &'static [f32],
}

/// A variable of a shader as this program compiles it.
#[derive(Clone)]
struct VariableDef {
    name: String,
    slots: u32,
    per: u32,
    default: Vec<f32>,
}

/// The name of the file with the result of each shader, in the folder of the game.
pub const STATUS_FILE: &str = "mgeXE-shaders.txt";

/// The result of the last load of each post shader: it compiles, or the error. The game
/// side has no other way to know (the stream goes from the game to this program only), so
/// the results go into a file in the folder of the game. The options of MGE XE read it
/// after a reload of a shader, and show the error.
///
/// The file: a line `sequence N`, which changes with each write; then for each shader a
/// line `shader <name>`, a line `loads N` (the number of its loads: a reload is complete
/// when it changes), a line `ok` or `error`, and the lines of the error with `| ` before
/// each.
#[derive(Default)]
pub struct ShaderStatus {
    list: Vec<(String, Option<String>, u32)>,
    sequence: u32,
    written: u32,
}

impl ShaderStatus {
    /// The result of a load: [`None`] for a shader that compiles.
    pub fn note(&mut self, name: &str, error: Option<String>) {
        let loads = self.list.iter().find(|(known, _, _)| known == name).map_or(0, |(_, _, loads)| *loads) + 1;
        self.list.retain(|(known, _, _)| known != name);
        self.list.push((name.to_string(), error, loads));
        self.sequence += 1;
    }

    pub fn text(&self) -> String {
        let mut text = format!("sequence {}\n", self.sequence);
        for (name, error, loads) in &self.list {
            text.push_str(&format!("shader {name}\nloads {loads}\n{}\n", if error.is_some() { "error" } else { "ok" }));
            for line in error.iter().flat_map(|error| error.lines()) {
                text.push_str(&format!("| {line}\n"));
            }
        }
        text
    }

    /// Writes the file when a result came since the last write.
    pub fn write(&mut self, root: &std::path::Path) {
        if self.written == self.sequence {
            return;
        }
        self.written = self.sequence;
        if let Err(problem) = std::fs::write(root.join(STATUS_FILE), self.text()) {
            println!("post: the file {STATUS_FILE} was not written: {problem}");
        }
    }
}

/// The part of an error of wgpu that the compiler wrote: wgpu puts lines of its own before
/// it ("Validation Error", "Caused by", the call). The whole text without such a line.
fn compiler_part(error: &str) -> &str {
    error.find("Shader '").map_or(error, |at| &error[at..])
}

/// An error message of the WGSL compiler with the line numbers of the file of the shader.
/// The compiler counts the common part and the functions of the variables first: `offset`
/// lines. The numbers are after `wgsl:` and at the start of the lines that show the code.
fn own_lines(message: &str, offset: usize) -> String {
    let own = |number: &str| number.parse::<usize>().ok().filter(|n| *n > offset).map(|n| (n - offset).to_string());
    let digits = |text: &str| text.chars().take_while(|c| c.is_ascii_digit()).count();
    message
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let count = digits(trimmed);
            if count > 0 && trimmed[count..].starts_with(" \u{2502}") {
                if let Some(number) = own(&trimmed[..count]) {
                    return format!("{number}{}", &trimmed[count..]);
                }
            }
            if let Some(at) = line.find("wgsl:") {
                let rest = &line[at + 5..];
                let count = digits(rest);
                if let Some(number) = own(&rest[..count]) {
                    return format!("{}wgsl:{number}{}", &line[..at], &rest[count..]);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The state of a shader that a mod loaded: the record 2 of the data of MGE XE
/// (`PostShaders::sendShaderStates`).
pub struct ShaderState {
    pub name: String,
    pub enabled: bool,
    /// The priority, when a mod set one.
    pub priority: Option<i64>,
    /// The number of times that a mod reloaded the shader (`shader:reload()` of MWSE).
    pub reloads: u32,
    /// The variables that the mod set: the name, the type letter of the API of MGE XE
    /// (`V` is an array of vectors with 4 floats each), the values.
    pub values: Vec<(String, u8, Vec<f32>)>,
}

impl ShaderState {
    /// Reads the record: the head (version, flags with the number of reloads from the bit
    /// 8, priority, number of variables, name),
    /// then for each variable its name, its type, the number of its values, the values.
    pub fn read(bytes: &[u8]) -> Option<ShaderState> {
        let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let name = |at: usize| bytes.get(at..at + 64).map(|b| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(64)]).into_owned());
        if word(0)? != 1 {
            return None;
        }
        let flags = word(4)?;
        let mut state = ShaderState { name: name(16)?, enabled: flags & 1 != 0, priority: (flags & 2 != 0).then_some(word(8)? as i32 as i64), reloads: flags >> 8, values: Vec::new() };
        let mut at = 80;
        for _ in 0..word(12)? {
            let (variable, kind, count) = (name(at)?, word(at + 64)?, word(at + 68)? as usize);
            let floats = bytes.get(at + 72..at + 72 + count * 4)?;
            state.values.push((variable, kind as u8, floats.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()));
            at += 72 + count * 4;
        }
        Some(state)
    }
}

/// The floats of `modv` for a shader: the default of each variable, or what a mod set.
fn values_of(variables: &[VariableDef], set: &[(String, u8, Vec<f32>)]) -> Vec<f32> {
    let mut values = vec![0.0f32; MOD_SLOTS * 4];
    let mut slot = 0usize;
    for variable in variables {
        let per = variable.per as usize;
        let given = set.iter().find(|(name, _, _)| *name == variable.name);
        // An array of vectors of the API has 4 floats for each vector.
        let (source, stride) = match given {
            Some((_, kind, floats)) => (floats.as_slice(), if *kind == b'V' { 4 } else { per }),
            None => (variable.default.as_slice(), per),
        };
        for (i, value) in source.iter().enumerate() {
            let (element, part) = (i / stride, i % stride);
            if element < variable.slots as usize && part < per && slot + element < MOD_SLOTS {
                values[(slot + element) * 4 + part] = *value;
            }
        }
        slot += variable.slots as usize;
    }
    values
}

/// The name of the WGSL function that reads a variable: the name of the variable, with `_`
/// after it when WGSL has a function or a word with that name (`distance` is `distance_()`).
fn function_name(variable: &str) -> String {
    const TAKEN: &[&str] = &[
        "abs", "acos", "all", "any", "asin", "atan", "ceil", "clamp", "cos", "cross", "degrees", "distance", "dot", "exp", "floor", "fract", "length", "log", "max", "min", "mix",
        "modf", "normalize", "pow", "radians", "reflect", "round", "saturate", "sign", "sin", "smoothstep", "sqrt", "step", "tan", "trunc", "select", "sample", "filter", "target",
        "texture", "loop", "switch", "default", "const", "var", "let", "fn", "struct", "return", "if", "else", "for", "while", "break", "continue", "discard", "true", "false",
        "u", "vs", "modv", "rcpres", "mview", "mproj",
    ];
    if TAKEN.contains(&variable) { format!("{variable}_") } else { variable.to_string() }
}

/// The WGSL functions that read the variables of a shader from `modv`.
fn variables_wgsl(variables: &[VariableDef]) -> String {
    let mut out = String::new();
    let mut slot = 0u32;
    for variable in variables {
        let name = function_name(&variable.name);
        let (kind, parts) = match variable.per {
            1 => ("f32", ".x"),
            2 => ("vec2f", ".xy"),
            3 => ("vec3f", ".xyz"),
            _ => ("vec4f", ""),
        };
        if variable.slots == 1 {
            out.push_str(&format!("fn {name}() -> {kind} {{ return modv.values[{slot}]{parts}; }}\n"));
        } else {
            out.push_str(&format!("fn {name}(i: i32) -> {kind} {{ return modv.values[{slot} + clamp(i, 0, {})]{parts}; }}\n", variable.slots - 1));
        }
        slot += variable.slots;
    }
    out
}

/// A variable of the `variables` line of a file: `name type`, `name type[N]`, with
/// `= number number ...` after it for the values before a mod sets it. The types are f32,
/// vec2, vec3 and vec4.
fn parse_variable(item: &str) -> Result<VariableDef, String> {
    let (left, default) = item.split_once('=').unwrap_or((item, ""));
    let mut words = left.split_whitespace();
    let (Some(name), Some(kind), None) = (words.next(), words.next(), words.next()) else { return Err(format!("the variable `{item}` is not `name type`")) };
    let (kind, slots) = match kind.split_once('[') {
        Some((kind, count)) => (kind, count.trim_end_matches(']').parse::<u32>().ok().filter(|n| *n >= 1).ok_or_else(|| format!("the size of the variable {name} is not [N]"))?),
        None => (kind, 1),
    };
    let per = match kind {
        "f32" => 1,
        "vec2" => 2,
        "vec3" => 3,
        "vec4" => 4,
        other => return Err(format!("the type `{other}` of the variable {name} is not f32, vec2, vec3 or vec4")),
    };
    let default = default.split_whitespace().map(|v| v.parse::<f32>().map_err(|_| format!("the value `{v}` of the variable {name} is not a number"))).collect::<Result<Vec<_>, _>>()?;
    Ok(VariableDef { name: name.to_string(), slots, per, default })
}

/// A shader of the chain: its name, its passes, and the environment flags that disable it
/// (`mgeflags`).
pub struct Shader {
    pub name: &'static str,
    pub source: &'static str,
    pub passes: &'static [Pass],
    pub disable_flags: u32,
    /// The shader has `disableSunglare`.
    pub no_sun_glare: bool,
    /// The priority of the order: the category (scene 1M, atmosphere 2M, lens 3M, sensor
    /// 4M, tone 5M, none 6M, final 9M) plus `priorityAdjust`.
    pub priority: i64,
    /// The textures of the shader under `Data Files/textures`, from the `src` annotations
    /// of its file: `tex1` to `tex10`.
    pub textures: &'static [&'static str],
    /// The shader has `requiresHDR`: it reads the adaptation point `u.hdr`.
    pub requires_hdr: bool,
    /// The variables that a mod sets from Lua, in the order of their places in `modv`.
    pub variables: &'static [Variable],
}

/// A shader of the chain as this program compiles it: from a WGSL file of a mod, or from
/// the table of the ports.
struct Definition {
    name: String,
    source: String,
    passes: Vec<(String, u32, bool)>,
    disable_flags: u32,
    no_sun_glare: bool,
    priority: i64,
    textures: Vec<String>,
    requires_hdr: bool,
    variables: Vec<VariableDef>,
}

/// The effect files of MGE XE that the stock ports were made from. A chain name resolves to
/// one file, of MGE XE or of a mod that ships a file with the same name: `port_for_file`
/// compares that file with the one here.
const STOCK_FILES: &[(&str, &str)] = &[
    ("Bloom Fine", include_str!("../../../assets/Data Files/shaders/XEshaders/Bloom Fine.fx")),
    ("Bloom Soft", include_str!("../../../assets/Data Files/shaders/XEshaders/Bloom Soft.fx")),
    ("Depth of Field", include_str!("../../../assets/Data Files/shaders/XEshaders/Depth of Field.fx")),
    ("Eye Adaptation (HDR)", include_str!("../../../assets/Data Files/shaders/XEshaders/Eye Adaptation (HDR).fx")),
    ("SSAO Fast", include_str!("../../../assets/Data Files/shaders/XEshaders/SSAO Fast.fx")),
    ("SSAO HQ", include_str!("../../../assets/Data Files/shaders/XEshaders/SSAO HQ.fx")),
    ("Sunshafts", include_str!("../../../assets/Data Files/shaders/XEshaders/Sunshafts.fx")),
    ("Underwater Effects", include_str!("../../../assets/Data Files/shaders/XEshaders/Underwater Effects.fx")),
    ("Underwater Interior Effects", include_str!("../../../assets/Data Files/shaders/XEshaders/Underwater Interior Effects.fx")),
];

/// The effect files of mods that have a port here: the name of the file, the key of its
/// code (`code_key`; the file of a mod is not a part of this program), and the name of its
/// port in the table. A mod can ship a file with the name of a stock shader and other
/// code: its port has another name in the table, and the file finds it by its key.
const MOD_FILES: &[(&str, u64, &str)] = &[
    // Morrowind Graphics Guide Shaders.
    ("MGG SSAO", 0x38f5e7deb7bb6c1d, "MGG SSAO"),
    ("MGG darker interiors", 0xdceef7333b83188a, "MGG darker interiors"),
    // STEP Shaders Compilation.
    ("ColorMood", 0xc75101063b24e812, "ColorMood"),
    // Heat Haze.
    ("heathaze", 0x1fbb2d0629a058db, "heathaze"),
    // Outlander Configuration.
    ("Special Process", 0x575527aede8d99ae, "Special Process"),
    // Magicka Expanded.
    ("oj_me_fog_box", 0xf74c17372038ae1e, "oj_me_fog_box"),
    // Vapourmist.
    ("tew_fogbox", 0x15d340262f176b99, "tew_fogbox"),
    // Skoomaesthesia.
    ("skoomaesthesia", 0xe55ab2ed9ba3d056, "skoomaesthesia"),
    // Kogoruhn Expanded.
    ("Disorient", 0xc9ff4e369536581f, "Disorient"),
    // MM - Enhanced Invisibility.
    ("Invisibility", 0x0c6d7e3f7e8cc48c, "Invisibility"),
    // Inspect It.
    ("InspectIt/Bokeh", 0x3850cc8f1d975ae9, "InspectIt/Bokeh"),
    // Ashfall.
    ("ashfall", 0xbe5ea7106c7913f6, "ashfall"),
    // The test harness of this project.
    ("harness_fill_opaque", 0x32a5e4f7392f4267, "harness_fill_opaque"),
    ("harness_fill_scene", 0x91682bbb28cc1bf5, "harness_fill_scene"),
    // Outlander Configuration.
    ("Lens Effects", 0x88f4ac8b06a9e54d, "Lens Effects"),
    // Morrowind Graphics Guide Shaders.
    ("film", 0xde752eb9fa3c514d, "film"),
    ("deband_fogaware", 0xfaf3aa00971077d4, "deband_fogaware"),
    ("Bloom Soft Sky", 0x1b83d108acc2ff62, "Bloom Soft"),
    // STEP Shaders Compilation: files with the names of stock shaders, and a Bloom Soft
    // with the sky and the hands in the bloom.
    ("Eye Adaptation (HDR)", 0x273066355e8e3439, "Eye Adaptation (HDR) of STEP"),
    ("SSAO HQ", 0x9c67d15c0cf12f4e, "SSAO HQ of STEP"),
    ("alternatebloom", 0xf1901492cdee5229, "Bloom Soft without the depth pass"),
    // Files with the name of a stock shader, of the STEP Shaders Compilation, of Watch the
    // Skies and of Krokantor's Enhanced Water Shader.
    ("Sunshafts", 0x661c38e7ca34478b, "Sunshafts of STEP"),
    ("Sunshafts", 0xc73cf55cee70bb5b, "Sunshafts of Watch the Skies"),
    ("Underwater Effects", 0x3a4ff8693245fd68, "Underwater Effects of Krokantor"),
    ("gammavtdt", 0xcb1302506316701c, "gammavtdt"),
    // Outlander Configuration.
    ("Cinematic Bokeh DoF", 0xe568928ca1eb5856, "Cinematic Bokeh DoF"),
    // Joy of Painting (the ports are in post_jop.rs). The key is that of the file itself:
    // the files `jop_common.fx` and `jop_gaussian.fx` that it includes are not in it.
    ("jop_adjuster", 0x58853825f229f1c8, "jop_adjuster"),
    ("jop_blackwhite", 0xec9e7d3f244acfe3, "jop_blackwhite"),
    ("jop_charcoal", 0x2f4f71267a5e92d1, "jop_charcoal"),
    ("jop_composite", 0xaaac8bb51755deca, "jop_composite"),
    ("jop_distort", 0x28f37e82f0f1eefb, "jop_distort"),
    ("jop_dof", 0x464e7bf4876da5d9, "jop_dof"),
    ("jop_fog_bw", 0xbb80908aef76b08a, "jop_fog_bw"),
    ("jop_fog_color", 0x55d5768dbfdc54be, "jop_fog_color"),
    ("jop_greyscale", 0x542b8cca03325275, "jop_greyscale"),
    ("jop_hatch", 0xdf358f1dab38c79e, "jop_hatch"),
    ("jop_ink", 0xeb46ebd60cd2f3bd, "jop_ink"),
    ("jop_kuwahara", 0xc0c589bded46ba86, "jop_kuwahara"),
    ("jop_lut", 0x9aa16b0e5ad0e065, "jop_lut"),
    ("jop_mottle", 0xa55d88578ff77866, "jop_mottle"),
    ("jop_oil", 0xf7e15b9eb0627cc5, "jop_oil"),
    ("jop_outline", 0xed650dc23c1c3ddf, "jop_outline"),
    ("jop_pastel", 0xf88ef0fcfb32813a, "jop_pastel"),
    ("jop_quantize", 0x64d332f6e1204381, "jop_quantize"),
    ("jop_sharpen", 0x6c0aabd717a7c391, "jop_sharpen"),
    ("jop_splash", 0x9e3614172c5c2be5, "jop_splash"),
    ("jop_vignette", 0x20dfac92c2828d91, "jop_vignette"),
    ("jop_watercolor", 0x48a80762115e64eb, "jop_watercolor"),
    ("jop_window", 0xfd2c875d2b5577e4, "jop_window"),
];

/// A line of an effect file that gives a name one number: `#define NAME value`, or
/// `static const float NAME = value;` (with `static`, `const` or `extern` before the type
/// `float`, `int` or `bool`). A mod that ships a shader with other numbers changes such
/// lines only.
fn number_line(line: &str) -> Option<(&str, f64)> {
    let line = line.split("//").next().unwrap_or("").trim();
    let (name, value) = if let Some(rest) = line.strip_prefix("#define ") {
        let mut words = rest.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some(name), Some(value), None) => (name, value),
            _ => return None,
        }
    } else {
        let mut rest = line;
        while let Some(after) = ["static ", "const ", "extern "].iter().find_map(|word| rest.strip_prefix(word)) {
            rest = after.trim_start();
        }
        if rest.len() == line.len() {
            return None;
        }
        let (left, right) = rest.split_once('=')?;
        let mut left = left.split_whitespace();
        match (left.next(), left.next(), left.next()) {
            (Some("float" | "int" | "bool"), Some(name), None) => (name, right.trim().trim_end_matches(';').trim()),
            _ => return None,
        }
    };
    let value = match value {
        "true" => 1.0,
        "false" => 0.0,
        other => other.trim_end_matches(['f', 'F']).parse::<f64>().ok()?,
    };
    Some((name, value))
}

/// The numbers of an effect file (see `number_line`).
fn file_numbers(text: &str) -> Vec<(String, f64)> {
    text.lines().filter_map(number_line).map(|(name, value)| (name.to_string(), value)).collect()
}

/// The key of the code of an effect file: FNV-1a 64 of its lines without the comments, the
/// space and the values of its numbers. Two files with the same key are the same shader,
/// with the same numbers or not.
fn code_key(text: &str) -> u64 {
    let mut key = 0xcbf2_9ce4_8422_2325u64;
    let mut add = |bytes: &[u8]| {
        for &byte in bytes {
            key ^= byte as u64;
            key = key.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for line in text.lines() {
        // The line of the technique has the annotations (the category and the like), which
        // are not code: see `file_priority`.
        if line.trim_start().starts_with("technique") {
            add(b"technique\n");
            continue;
        }
        match number_line(line) {
            Some((name, _)) => add(name.as_bytes()),
            None => {
                let code = line.split("//").next().unwrap_or("");
                for word in code.split_whitespace() {
                    add(word.as_bytes());
                    add(b" ");
                }
            }
        }
        add(b"\n");
    }
    key
}

/// The place of an effect file in the order of the chain, from the annotations of its
/// technique: the category (`getShaderPriority` of the Direct3D 9 path) plus
/// `priorityAdjust`. A file without a category has the default, 6000000.
fn file_priority(text: &str) -> i64 {
    let Some(line) = text.lines().find(|line| line.trim_start().starts_with("technique")) else { return 6_000_000 };
    let value_of = |key: &str| line.split_once(key).and_then(|(_, rest)| rest.split_once('=')).map(|(_, rest)| rest.trim_start());
    let category = match value_of("category").and_then(|rest| rest.strip_prefix('"')).and_then(|rest| rest.split('"').next()) {
        Some("scene") => 1_000_000,
        Some("atmosphere") => 2_000_000,
        Some("lens") => 3_000_000,
        Some("sensor") => 4_000_000,
        Some("tone") => 5_000_000,
        Some("final") => 9_000_000,
        _ => 6_000_000,
    };
    let adjust = value_of("priorityAdjust").and_then(|rest| rest.split([';', ' ', '>']).next()).and_then(|number| number.trim().parse::<i64>().ok()).unwrap_or(0);
    category + adjust
}

/// How the effect file of a chain name stands to the port of that name.
enum FileOfPort {
    /// The file the port was made from, with the same numbers or with other ones.
    Same,
    /// Other code: the port of the name is not a port of this file.
    Other,
}

/// Puts numbers of an effect file into the WGSL of a port. A line of the port
/// `const NAME: TYPE = VALUE; // file` takes the value of NAME when `numbers` has it.
/// Returns the source and the names that it took.
fn with_file_numbers(source: &str, numbers: &[(String, f64)]) -> (String, Vec<String>) {
    let mut taken = Vec::new();
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let mut written = false;
        if let Some((left, _)) = line.trim_end().strip_suffix("// file").and_then(|code| code.trim_start().strip_prefix("const ")).and_then(|rest| rest.split_once('=')) {
            let (name, kind) = left.split_once(':').map_or((left.trim(), "f32"), |(name, kind)| (name.trim(), kind.trim()));
            if let Some((_, value)) = numbers.iter().find(|(n, _)| n == name) {
                let text = match kind {
                    "i32" | "u32" => format!("{}", *value as i64),
                    _ => {
                        let text = format!("{value}");
                        if text.contains(['.', 'e']) { text } else { format!("{text}.0") }
                    }
                };
                let indent = &line[..line.len() - line.trim_start().len()];
                out.push_str(&format!("{indent}const {name}: {kind} = {text}; // file\n"));
                taken.push(name.to_string());
                written = true;
            }
        }
        if !written {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, taken)
}

/// Gives each variable of a port with one value the number of the file of that name, as its
/// value before a mod sets it. Returns the names that took a number.
fn fit_variable_defaults(variables: &mut [VariableDef], numbers: &[(String, f64)]) -> Vec<String> {
    let mut taken = Vec::new();
    for variable in variables.iter_mut().filter(|v| v.slots == 1 && v.per == 1) {
        if let Some((name, value)) = numbers.iter().find(|(n, _)| *n == variable.name) {
            variable.default = vec![*value as f32];
            taken.push(name.clone());
        }
    }
    taken
}

/// Fits the port of a chain name to the effect file that the name resolves to, and tells
/// the log what it did. A copy of the file of the port with other numbers: the port takes
/// the numbers that it can (a number that a table of the port goes with, for example a
/// sample count, stays). A file with other code: the port runs as it is, and the log says
/// that the shader of the mod needs a port of its own.
fn port_for_file(definition: &mut Definition, file: &str) {
    let name = definition.name.clone();
    let name = name.as_str();
    let source = definition.source.clone();
    let source = source.as_str();
    let key = code_key(file);
    // The stock file of the name is the file of the port, unless the port is that of a file
    // of a mod with that name.
    let of_mod = MOD_FILES.iter().any(|(mod_name, mod_key, _)| *mod_name == name && *mod_key == key);
    let stock = STOCK_FILES.iter().find(|(stock_name, _)| *stock_name == name && !of_mod).map(|(_, text)| *text);
    let relation = match stock {
        Some(text) if code_key(text) == key => FileOfPort::Same,
        _ if MOD_FILES.iter().any(|(mod_name, mod_key, _)| *mod_name == name && *mod_key == key) => FileOfPort::Same,
        _ => FileOfPort::Other,
    };
    match relation {
        FileOfPort::Same => {
            // The same code has the same number lines in the same order: the numbers that
            // differ from those of the file of the port. A port of a file of a mod has no
            // file to compare with, and takes each number of its marked constants.
            let numbers = file_numbers(file);
            let other: Vec<(String, f64)> = match stock {
                Some(text) => numbers.into_iter().zip(file_numbers(text)).filter(|((_, a), (_, b))| (a - b).abs() > 1e-9 * b.abs().max(1.0)).map(|(number, _)| number).collect(),
                None => numbers,
            };
            let (fitted, mut taken) = with_file_numbers(source, &other);
            definition.source = fitted;
            taken.extend(fit_variable_defaults(&mut definition.variables, &other));
            // An older copy of a stock file has no category: its place in the chain is
            // that of the file, as on Direct3D 9.
            if let Some(text) = stock.filter(|text| file_priority(text) != file_priority(file)) {
                println!("post: {name}: the file {name}.fx has another category than the stock file (place {} and not {}); the port takes it", file_priority(file), file_priority(text));
                definition.priority = file_priority(file);
            }
            if stock.is_none() {
                definition.priority = file_priority(file);
            }
            if stock.is_some() {
                let say = |numbers: Vec<&(String, f64)>| numbers.iter().map(|(n, v)| format!("{n} = {v}")).collect::<Vec<_>>().join(", ");
                let (took, kept): (Vec<_>, Vec<_>) = other.iter().partition(|(n, _)| taken.contains(n));
                if !took.is_empty() {
                    println!("post: {name}: the file {name}.fx is the stock shader with other numbers; the port takes them: {}", say(took));
                }
                if !kept.is_empty() {
                    println!("post: {name}: numbers of the file {name}.fx that the port cannot take: {}", say(kept));
                }
            }
        }
        FileOfPort::Other => {
            println!("post: {name}: the file {name}.fx is another shader than the port of this name (code key {key:#018x}); the port runs as it is, and the file needs a port of its own");
        }
    }
}

/// A pass of the `passes` line of a file: `name`, `name /N` for a target of the picture size
/// over N, `name f16` for 16-bit floats, or both.
fn parse_pass(item: &str) -> Result<(String, u32, bool), String> {
    let mut words = item.split_whitespace();
    let Some(name) = words.next() else { return Err("a pass without a name".to_string()) };
    let (mut scale, mut half_float) = (1u32, false);
    for word in words {
        if let Some(number) = word.strip_prefix('/') {
            scale = number.parse().ok().filter(|n: &u32| *n >= 1).ok_or_else(|| format!("the scale `{word}` of the pass {name} is not /N with N 1 or more"))?;
        } else if word == "f16" {
            half_float = true;
        } else {
            return Err(format!("the word `{word}` of the pass {name} is not known (/N or f16)"));
        }
    }
    Ok((name.to_string(), scale, half_float))
}

impl Definition {
    fn of_port(shader: &Shader) -> Definition {
        Definition {
            name: shader.name.to_string(),
            source: shader.source.to_string(),
            passes: shader.passes.iter().map(|pass| (pass.entry.to_string(), pass.scale, pass.half_float)).collect(),
            disable_flags: shader.disable_flags,
            no_sun_glare: shader.no_sun_glare,
            priority: shader.priority,
            textures: shader.textures.iter().map(|name| name.to_string()).collect(),
            requires_hdr: shader.requires_hdr,
            variables: shader.variables.iter().map(|v| VariableDef { name: v.name.to_string(), slots: v.slots, per: v.per, default: v.default.to_vec() }).collect(),
        }
    }

    /// A shader file of a mod: `Data Files/shaders/XEshaders/<name>.wgsl`. The file starts
    /// with lines `//! key: value`, which have what the technique of an effect file of
    /// MGE XE has in its annotations; the rest is WGSL, compiled after the common part
    /// ([`COMMON`]: the variables `u`, the textures, the samplers, the vertex function `vs`
    /// and the struct `Quad`). The keys:
    ///
    /// - `interface: mge-xe-wgsl 1` (needed)
    /// - `passes: a, b /2 f16, c` (needed): the fragment functions, one for each pass, in
    ///   their order; `/N` draws the pass into a target of the picture size over N, `f16`
    ///   into 16-bit floats (8-bit colours without it)
    /// - `category: scene | atmosphere | lens | sensor | tone | none | final` (`none`)
    /// - `priority_adjust: N` (0)
    /// - `mgeflags: N` (0): the shader does not run when a bit is set in the frame: 1
    ///   interior, 2 exterior, 4 interior that behaves as an exterior, 8 under the water,
    ///   16 above the water, 32 the sun is visible, 64 the sun is not visible
    /// - `requires_hdr: true | false` (false)
    /// - `disable_sunglare: true | false` (false)
    /// - `texture: path under Data Files/textures` (none): bound as `tex1`
    /// - `variables: a f32 = 1, b vec3, c f32[8]` (none): the variables that a mod sets from
    ///   Lua, with their values before that; the WGSL reads them as `a()`, `b()`, `c(i)`
    fn of_file(name: &str, text: &str) -> Result<Definition, String> {
        let mut definition = Definition { name: name.to_string(), source: text.to_string(), passes: Vec::new(), disable_flags: 0, no_sun_glare: false, priority: 0, textures: Vec::new(), requires_hdr: false, variables: Vec::new() };
        let (mut category, mut adjust, mut interface) = (6_000_000i64, 0i64, false);
        let flag = |value: &str| match value {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("`{other}` is not true or false")),
        };
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("//!") else { break };
            let Some((key, value)) = rest.split_once(':') else { return Err(format!("the line `{line}` has no `key: value`")) };
            let value = value.trim();
            match key.trim() {
                "interface" => interface = value == "mge-xe-wgsl 1",
                "passes" => definition.passes = value.split(',').map(str::trim).filter(|pass| !pass.is_empty()).map(parse_pass).collect::<Result<_, _>>()?,
                "category" => {
                    category = match value {
                        "scene" => 1_000_000,
                        "atmosphere" => 2_000_000,
                        "lens" => 3_000_000,
                        "sensor" => 4_000_000,
                        "tone" => 5_000_000,
                        "none" => 6_000_000,
                        "final" => 9_000_000,
                        other => return Err(format!("the category `{other}` is not known")),
                    }
                }
                "priority_adjust" => adjust = value.parse().map_err(|_| format!("the priority adjust `{value}` is not a number"))?,
                "mgeflags" => definition.disable_flags = value.parse().map_err(|_| format!("the flags `{value}` are not a number"))?,
                "requires_hdr" => definition.requires_hdr = flag(value)?,
                "disable_sunglare" => definition.no_sun_glare = flag(value)?,
                "texture" => {
                    definition.textures = value.split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect();
                    if definition.textures.len() > TEXTURES {
                        return Err(format!("more than {TEXTURES} textures"));
                    }
                }
                "variables" => definition.variables = value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(parse_variable).collect::<Result<_, _>>()?,
                other => return Err(format!("the key `{other}` is not known")),
            }
        }
        if !interface {
            return Err("no line `//! interface: mge-xe-wgsl 1` at its start".to_string());
        }
        if definition.passes.is_empty() {
            return Err("no line `//! passes: ...` with the fragment functions".to_string());
        }
        definition.priority = category + adjust;
        Ok(definition)
    }
}

/// The HDR measure of MGE XE (`PostShaders::evalAdaptHDR`): the picture before the chain
/// goes into 512 x 512 pixels and then by halves to 1 pixel, which is read back some frames
/// later; its luminance moves the adaptation point of each frame, the `HDR` of a shader.
struct Hdr {
    texture: wgpu::Texture,
    /// The levels of the texture, 512 x 512 to 1 x 1.
    levels: Vec<wgpu::TextureView>,
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    state: HdrState,
    /// The last luminance read, 0.5 before the first one.
    luminance: f32,
    point: [f32; 4],
    last: Option<std::time::Instant>,
    /// `render.hdr_reaction_time`, seconds.
    reaction: f32,
}

/// The copy of the pixel into the buffer is in the commands of a frame; the map of the
/// buffer is asked for in the frame after, when those commands are with the GPU.
enum HdrState {
    Idle,
    Copied,
    Mapping(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

impl Hdr {
    fn new(device: &wgpu::Device, blit_layout: &wgpu::BindGroupLayout, reaction: f32) -> Hdr {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post hdr"),
            size: wgpu::Extent3d { width: 512, height: 512, depth_or_array_layers: 1 },
            mip_level_count: 10,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let levels = (0..10).map(|level| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() })).collect();
        let pipeline = full_screen_pipeline(device, "post hdr", BLIT_SHADER, blit_layout, wgpu::TextureFormat::Rgba8Unorm);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post hdr"), size: 4, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Hdr { texture, levels, pipeline, buffer, state: HdrState::Idle, luminance: 0.5, point: [1.0; 4], last: None, reaction: reaction.clamp(0.01, 30.0) }
    }

    /// One frame of the measure. Returns the adaptation point.
    fn frame(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, picture: &wgpu::TextureView, blit_layout: &wgpu::BindGroupLayout, linear: &wgpu::Sampler, environment_flags: u32) -> [f32; 4] {
        use std::sync::atomic::{AtomicBool, Ordering};
        match &self.state {
            HdrState::Idle => {
                // A linear copy into each level is the linear `StretchRect` of Direct3D 9:
                // the mean of 2 x 2 pixels from the second level on.
                let mut source = picture;
                for level in &self.levels {
                    let group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
                        label: Some("post hdr"),
                        layout: blit_layout,
                        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(linear) }],
                    });
                    let mut pass = full_screen_pass(encoder, level, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &group, &[]);
                    pass.draw(0..3, 0..1);
                    drop(pass);
                    source = level;
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 9, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyBufferInfo { buffer: &self.buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None } },
                    wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                );
                self.state = HdrState::Copied;
            }
            HdrState::Copied => {
                let ready = std::sync::Arc::new(AtomicBool::new(false));
                let flag = ready.clone();
                self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_ok() {
                        flag.store(true, Ordering::Relaxed);
                    }
                });
                self.state = HdrState::Mapping(ready);
            }
            HdrState::Mapping(ready) => {
                if ready.load(Ordering::Relaxed) {
                    if let Ok(pixel) = self.buffer.slice(..).get_mapped_range() {
                        let (r, g, b) = (pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[2] as f32 / 255.0);
                        self.luminance = 0.27 * r + 0.67 * g + 0.06 * b;
                    }
                    self.buffer.unmap();
                    self.state = HdrState::Idle;
                }
            }
        }
        // Each frame: the point follows the luminance with the reaction time. The scale is
        // for an exterior and an interior that behaves as one (the flags 2 and 4).
        let now = std::time::Instant::now();
        let dt = self.last.map_or(0.0, |last| (now - last).as_secs_f32());
        self.last = Some(now);
        let scale = if environment_flags & 6 != 0 { 2.5 } else { 1.0 };
        let lambda = (-dt / self.reaction).exp();
        let p = &mut self.point;
        p[3] = self.luminance;
        p[2] = p[3] * scale + (p[2] - p[3] * scale) * lambda;
        p[1] = p[3] + (p[1] - p[3]) * lambda;
        p[0] = p[2] / scale;
        *p
    }
}

/// Sunshafts by phal and Hrnchamd, MGE XE 0.11.0, as `Sunshafts.fx` has it: a mask of the
/// sky at a quarter of the size, a radial blur, the rays toward the sun, a tangential blur,
/// and the sum with a sun disc.
// The Sunshafts of the STEP Shaders Compilation: an older file than that of MGE XE, with
// the same name. The disc of the sun has another edge, another colour and another mix.
const SUNSHAFTS_STEP: &str = "
const N: i32 = 40; // file
const raysunradius: f32 = 0.25; // file
const raystrength: f32 = 1.7; // file
const rayfalloff: f32 = 1.10; // file
const rayfalloffconst: f32 = 0.125; // file
const raysunfalloff: f32 = 1.5; // file
const centervis: f32 = 0.3; // file
const sunrayocclude: f32 = 0.75; // file
const brightnessadd: f32 = 1.25; // file
const offscreenrange: f32 = 0.5; // file
const sundiscradius: f32 = 0.025; // file
const sundiscbrightness: f32 = 1.4; // file
const sundiscdesaturate: f32 = 0.4; // file
const sundiscocclude: f32 = 0.75; // file
const rscale: f32 = 0.5; // file
const scale: f32 = 2.0; // file
const threshold: f32 = 1e7; // file

// The values that the effect computes from the variables (its static consts).
struct Statics {
    raspect: f32,
    sundir: vec3f,
    forward: f32,
    sunview: vec2f,
    sunviewhalf: vec2f,
    light: f32,
    sharpness: f32,
    strength: f32,
    suncoldisc: vec3f,
    azi_horizon: f32,
}

fn statics() -> Statics {
    var s: Statics;
    let rcp = rcpres();
    s.raspect = rcp.x / rcp.y;
    s.sundir = -normalize(u.sunpos.xyz);
    s.forward = dot(s.sundir, u.eyevec.xyz);
    let fov = u.eyepos.w;
    let texproj = 0.5 * vec2f(1.0, -rcp.y / rcp.x) / tan(radians(fov * 0.5));
    // mul(float3, mview): a row vector by the rotation of the view.
    let v = u.sunpos.xyz / dot(u.eyevec.xyz, u.sunpos.xyz);
    let sunview_v = v.x * u.mview[0].xyz + v.y * u.mview[1].xyz + v.z * u.mview[2].xyz;
    s.sunview = vec2f(0.5) + sunview_v.xy * texproj;
    s.sunviewhalf = 0.5 * s.sunview;
    let sunvis = u.eyevec.w;
    s.light = 1.0 - pow(1.0 - sunvis, 2.0);
    let fogstart = u.fogcol.w;
    s.sharpness = mix(60.0, 450.0 + 100.0 * s.sundir.z, saturate(fogstart / 480.0));
    s.strength = raystrength * s.light * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.x - 0.5)) * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.y - 0.5));
    let suncol = u.suncol.xyz;
    s.suncoldisc = vec3f(1.0, 0.76 + 0.24 * u.sunpos.z, 0.37 + 0.63 * u.sunpos.z) * saturate3(suncol / max(suncol.r, max(suncol.g, suncol.b)) * (1.0 - sundiscdesaturate) + vec3f(sundiscdesaturate));
    let fogrange = u.fognearcol.w;
    let waterlevel = u.suncol.w;
    s.azi_horizon = normalize(vec2f(4.0 * fogrange, waterlevel - u.eyepos.z)).y;
    return s;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn stretch(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    var depth = 0.0;
    if s.forward < 0.0 {
        let src = scale * in.tex;
        let rcp = rcpres();
        depth = step(threshold, depth_at(src));
        depth += step(threshold, depth_at(src + vec2f(rcp.x, 0.0)));
        depth += step(threshold, depth_at(src + vec2f(0.0, rcp.y)));
        depth += step(threshold, depth_at(src + rcp));
        depth *= 0.25;
    }
    return vec4f(0.0, 0.0, 0.0, depth);
}

@fragment fn blur_r_half(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let radial = normalize(in.tex - s.sunviewhalf) * rcpres().yx;
    var alpha = 0.3333 * pass_at(in.tex).a;
    alpha += 0.2222 * pass_at(in.tex + radial).a;
    alpha += 0.2222 * pass_at(in.tex - radial).a;
    alpha += 0.1111 * pass_at(in.tex + 2.0 * radial).a;
    alpha += 0.1111 * pass_at(in.tex - 2.0 * radial).a;
    return vec4f(0.0, 0.0, 0.0, alpha);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    var screendir = in.tex - s.sunview;
    let screendist = length(screendir * vec2f(1.0, s.raspect));
    screendir /= screendist;
    let sunr = min(raysunradius, screendist);
    var l = 0.0;
    for (var i = 1; i <= N; i++) {
        let sundist = f32(i) / f32(N) * sunr;
        let at = clamp(s.sunview + sundist * screendir, vec2f(0.0), vec2f(1.0)) * rscale;
        l += pass_at(at).a * exp(-((screendist - sundist) / (rayfalloffconst + sundist)) * rayfalloff) * pow(1.0 - saturate(sundist / raysunradius), raysunfalloff);
    }
    l *= s.strength / f32(N) * (screendist / raysunradius * (1.0 - centervis) + centervis);
    var col = vec4f(u.suncol.r, 0.8 * u.suncol.g, 0.8 * u.suncol.b, l);
    col = vec4f(col.rgb * (1.0 + brightnessadd * pow(col.a, 3.0)), col.a);
    return col;
}

@fragment fn blur_t(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let tangent = normalize(in.tex - s.sunview).yx * vec2f(rcpres().y, -rcpres().x);
    var col = 0.3333 * pass_at(in.tex);
    col += 0.2222 * pass_at(in.tex + tangent);
    col += 0.2222 * pass_at(in.tex - tangent);
    col += 0.1111 * pass_at(in.tex + 2.0 * tangent);
    col += 0.1111 * pass_at(in.tex - 2.0 * tangent);
    return col;
}

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let s = statics();
    let ray = pass_at(in.tex);
    var col = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    col *= saturate(1.0 - sunrayocclude * ray.a);
    col = saturate3(col + ray.rgb * ray.a);
    if s.forward < 0.0 {
        var screendir = in.tex - s.sunview;
        screendir.y *= s.raspect;
        var occl = s.light * step(threshold, depth_at(in.tex));
        occl *= pow(saturate(sundiscradius / (length(screendir) + 0.0001)), 3.0);
        if occl > 0.004 {
            var scol = s.suncoldisc * sundiscbrightness;
            let azi = normalize(to_world(in.tex)).z;
            occl *= smoothstep(-0.005, 0.010, azi - s.azi_horizon);
            let gb = smoothstep(-0.04, 0.05, azi - s.azi_horizon);
            scol = vec3f(scol.r, scol.g * gb, scol.b * gb);
            col = max(col, scol) * occl + col * (1.0 - sundiscocclude * occl);
        }
    }
    return vec4f(col, 1.0);
}
";

// The Underwater Effects of Krokantor's Enhanced Water Shader: the file of MGE XE with a
// green tint, more wobble, other rays at night, and a second pass that splits the colours.
// The file reads `skycol`, which MGE XE does not give a post shader: it is black there.
const UNDERWATER_KROKANTOR: &str = "
const lightcolour: vec4f = vec4f(0.06, 0.14, 0.08, 0.0);
const chromaRC: f32 = 0.0575; // file
const chromaBY: f32 = 0.0375; // file

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn water_at(t: vec3f) -> vec4f {
    return textureSampleLevel(watertexture, linear_wrap, t, 0.0);
}

fn fetch(tex: vec3f, uw: f32) -> f32 {
    let time = u.sunvec.w;
    let c = water_at(vec3f(tex.xy / 3308.0, 0.22 * time)).b;
    return pow(c, 0.25) * saturate(exp(tex.z / 90.0)) * saturate(1.0 + tex.z / uw);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let sunvis = u.eyevec.w;
    let sunpos = u.sunpos.xyz;
    let eyepos = u.eyepos.xyz;
    // `lf` of the file is one number from a sum of three: HLSL takes the first part. The
    // sky colour of the file is black (see above).
    let lf = (0.2 + 0.8 * sunvis) * (u.sunamb.x * 1.4);

    var wobble = 0.02 * (2.0 * water_at(vec3f(0.25 * in.tex, 0.2 * time)).rg - 1.0);
    // The power 32 of the file, by squares: the base can be negative.
    let x = 2.0 * in.tex - 1.0;
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    wobble *= 1.0 - x16 * x16;
    let tex = in.tex + wobble;

    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    var d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    var v = toWorld(tex);
    let e = vec3f(0.1, 0.1, 1.0) * eyepos;
    let green = vec4f(0.24, 0.52, 0.44, 1.0);

    d = min(d, -eyepos.z / max(1e-5, v.z));
    v = v / length(v.xy);
    let r = -sunpos.xy - v.xy * dot(-sunpos.xy, v.xy);

    var shellpos = e + 800.0 * v;
    shellpos = vec3f(shellpos.xy + 0.8 * r * shellpos.z, shellpos.z);
    var rayz = fetch(shellpos, min(eyepos.z, 400.0)) * saturate(d / 800.0) * 0.294;

    var shellpos2 = e + 1400.0 * v;
    shellpos2 = vec3f(shellpos2.xy + 1.0 * r * shellpos2.z, shellpos2.z);
    rayz += fetch(shellpos2, 2.0 * eyepos.z) * saturate(d / 800.0 - 1.0) * 0.132;

    var shellpos3 = e + 2000.0 * v;
    shellpos3 = vec3f(shellpos3.xy + 1.2 * r * shellpos3.z, shellpos3.z);
    rayz += fetch(shellpos3, 3.0 * eyepos.z) * saturate(d / 800.0 - 2.0) * 0.065;

    var result = c + rayz * lf * lightcolour;
    if sunvis <= 0.5 {
        result += 0.2 * green;
    } else {
        result += rayz * 0.3;
        result += 0.3 * green;
    }
    return result;
}

@fragment fn chroma(in: Quad) -> @location(0) vec4f {
    let dispa = vec3f(1.0 + chromaRC * 0.07, 1.0 + chromaRC * 0.05, 1.0 + chromaRC * 0.03);
    let dispb = vec3f(1.0 + chromaBY * 0.08, 1.0 + chromaBY * 0.04, 1.0 + chromaBY * 0.02);

    let rCoords = (dispa.r * dispb.r) * in.tex;
    let gCoords = (dispa.g * dispb.g) * in.tex;
    let bCoords = (dispa.b * dispb.b) * in.tex;

    let scene = vec3f(
        textureSampleLevel(lastpass, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastpass, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastpass, linear_clamp, bCoords, 0.0).b);
    return vec4f(scene, 1.0);
}
";

// The Sunshafts of Watch the Skies: the file of MGE XE with rays that are summed along the
// line to the sun in another way, a dither against bands, and other numbers.
const SUNSHAFTS_WTS: &str = "
// Volumetric sun shafts - banding removed
// Compatible with MGE XE, screen-space radial method
const N: i32 = 50; // file
const raysunradius: f32 = 0.36; // file
const raystrength: f32 = 1.85; // file
const rayfalloff: f32 = 1.7; // file
const rayfalloffconst: f32 = 0.21; // file
const raysunfalloff: f32 = 2.2; // file
const centervis: f32 = 0.25; // file
const sunrayocclude: f32 = 0.8; // file
const brightnessadd: f32 = 1.15; // file
const offscreenrange: f32 = 0.8; // file
const sundiscradius: f32 = 0.027; // file
const sundiscbrightness: f32 = 1.3; // file
const sundiscdesaturate: f32 = 0.7; // file
const sundiscocclude: f32 = 0.73; // file
const rscale: f32 = 0.5; // file
const scale: f32 = 2.0; // file
const threshold: f32 = 1e7; // file

// The values that the effect computes from the variables (its static consts).
struct Statics {
    raspect: f32,
    sundir: vec3f,
    forward: f32,
    sunview: vec2f,
    sunviewhalf: vec2f,
    light: f32,
    sharpness: f32,
    strength: f32,
    suncoldisc: vec3f,
    azi_horizon: f32,
}

fn statics() -> Statics {
    var s: Statics;
    let rcp = rcpres();
    s.raspect = rcp.x / rcp.y;
    s.sundir = -normalize(u.sunpos.xyz);
    s.forward = dot(s.sundir, u.eyevec.xyz);
    let fov = u.eyepos.w;
    let texproj = 0.5 * vec2f(1.0, -rcp.y / rcp.x) / tan(radians(fov * 0.5));
    // mul(float3, mview): a row vector by the rotation of the view.
    let v = u.sunpos.xyz / dot(u.eyevec.xyz, u.sunpos.xyz);
    let sunview_v = v.x * u.mview[0].xyz + v.y * u.mview[1].xyz + v.z * u.mview[2].xyz;
    s.sunview = vec2f(0.5) + sunview_v.xy * texproj;
    s.sunviewhalf = 0.5 * s.sunview;
    let sunvis = u.eyevec.w;
    s.light = 1.0 - pow(1.0 - sunvis, 2.0);
    let fogstart = u.fogcol.w;
    s.sharpness = mix(60.0, 660.0 + 360.0 * s.sundir.z, saturate(fogstart / 480.0));
    s.strength = raystrength * s.light * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.x - 0.5)) * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.y - 0.5));
    let suncol = u.suncol.xyz;
    s.suncoldisc = vec3f(1.0, 0.76 + 0.24 * u.sunpos.z, 0.54 + 0.46 * u.sunpos.z) * saturate3(suncol / max(suncol.r, max(suncol.g, suncol.b)) * (1.0 - sundiscdesaturate) + vec3f(sundiscdesaturate));
    let fogrange = u.fognearcol.w;
    let waterlevel = u.suncol.w;
    s.azi_horizon = normalize(vec2f(4.0 * fogrange, waterlevel - u.eyepos.z)).y;
    return s;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn stretch(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    var depth = 0.0;
    if s.forward < 0.0 {
        let src = scale * in.tex;
        let rcp = rcpres();
        depth = step(threshold, depth_at(src));
        depth += step(threshold, depth_at(src + vec2f(rcp.x, 0.0)));
        depth += step(threshold, depth_at(src + vec2f(0.0, rcp.y)));
        depth += step(threshold, depth_at(src + rcp));
        depth *= 0.25;
    }
    return vec4f(0.0, 0.0, 0.0, depth);
}

@fragment fn blur_r_half(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let radial = normalize(in.tex - s.sunviewhalf) * rcpres().yx;
    var alpha = 0.3333 * pass_at(in.tex).a;
    alpha += 0.2222 * pass_at(in.tex + radial).a;
    alpha += 0.2222 * pass_at(in.tex - radial).a;
    alpha += 0.1111 * pass_at(in.tex + 2.0 * radial).a;
    alpha += 0.1111 * pass_at(in.tex - 2.0 * radial).a;
    return vec4f(0.0, 0.0, 0.0, alpha);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    var dir = in.tex - s.sunview;
    let dist = length(dir * vec2f(1.0, s.raspect));
    dir /= max(dist, 1e-5);

    var l = 0.0;
    for (var i = 1; i <= N; i++) {
        let t = f32(i) / f32(N);
        let offset = t * min(raysunradius, dist);
        let sampleA = pass_at(clamp(in.tex - offset * dir, vec2f(0.0), vec2f(1.0)) * rscale).a;
        let fade = exp(-((dist - offset) / (rayfalloffconst + offset)) * rayfalloff);
        let taper = pow(1.0 - offset / raysunradius, 3.0);
        let bias = 0.25 * (1.0 - t);
        l += sampleA * fade * (taper + bias) / f32(N);
    }

    l *= s.strength * (dist / raysunradius * (1.0 - centervis) + centervis);
    l = saturate(l);

    var col = vec4f(u.suncol.r, 0.8 * u.suncol.g, 0.8 * u.suncol.b, l);
    col = vec4f(col.rgb * (1.0 + brightnessadd * pow(col.a, 3.0)), col.a);

    // subtle dithering to remove banding
    let dither = (fract(sin(dot(in.tex, vec2f(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.001;
    col = vec4f(saturate3(col.rgb + dither), col.a);
    return col;
}

@fragment fn blur_t(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let tangent = normalize(in.tex - s.sunview).yx * vec2f(rcpres().y, -rcpres().x);
    var col = 0.3333 * pass_at(in.tex);
    col += 0.2222 * pass_at(in.tex + tangent);
    col += 0.2222 * pass_at(in.tex - tangent);
    col += 0.1111 * pass_at(in.tex + 2.0 * tangent);
    col += 0.1111 * pass_at(in.tex - 2.0 * tangent);
    return col;
}

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let s = statics();
    let ray = pass_at(in.tex);
    var col = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    col *= saturate(1.0 - sunrayocclude * ray.a);
    col = saturate3(col + ray.rgb * ray.a);
    if s.forward < 0.0 {
        var screendir = in.tex - s.sunview;
        screendir.y *= s.raspect;
        var occl = s.light * step(threshold, depth_at(in.tex));
        occl *= smoothstep(0.0, 1.0, exp2(s.sharpness * (sundiscradius - length(screendir))));
        if occl > 0.004 {
            var scol = s.suncoldisc * sundiscbrightness;
            let azi = normalize(to_world(in.tex)).z;
            occl *= smoothstep(-0.005, 0.010, azi - s.azi_horizon);
            let gb = smoothstep(-0.04, 0.09, azi - s.azi_horizon);
            scol = vec3f(scol.r, scol.g * gb, scol.b * gb);
            col = mix(col, scol, sundiscocclude * occl);
        }
    }
    return vec4f(col, 1.0);
}
";

const SUNSHAFTS: &str = "
const N: i32 = 20; // file
const raysunradius: f32 = 0.25; // file
const raystrength: f32 = 1.7; // file
const rayfalloff: f32 = 1.10; // file
const rayfalloffconst: f32 = 0.125; // file
const raysunfalloff: f32 = 1.5; // file
const centervis: f32 = 0.3; // file
const sunrayocclude: f32 = 0.75; // file
const brightnessadd: f32 = 1.25; // file
const offscreenrange: f32 = 0.5; // file
const sundiscradius: f32 = 0.025; // file
const sundiscbrightness: f32 = 1.9; // file
const sundiscdesaturate: f32 = 0.5; // file
const sundiscocclude: f32 = 0.75; // file
const rscale: f32 = 0.5; // file
const scale: f32 = 2.0; // file
const threshold: f32 = 1e7; // file

// The values that the effect computes from the variables (its static consts).
struct Statics {
    raspect: f32,
    sundir: vec3f,
    forward: f32,
    sunview: vec2f,
    sunviewhalf: vec2f,
    light: f32,
    sharpness: f32,
    strength: f32,
    suncoldisc: vec3f,
    azi_horizon: f32,
}

fn statics() -> Statics {
    var s: Statics;
    let rcp = rcpres();
    s.raspect = rcp.x / rcp.y;
    s.sundir = -normalize(u.sunpos.xyz);
    s.forward = dot(s.sundir, u.eyevec.xyz);
    let fov = u.eyepos.w;
    let texproj = 0.5 * vec2f(1.0, -rcp.y / rcp.x) / tan(radians(fov * 0.5));
    // mul(float3, mview): a row vector by the rotation of the view.
    let v = u.sunpos.xyz / dot(u.eyevec.xyz, u.sunpos.xyz);
    let sunview_v = v.x * u.mview[0].xyz + v.y * u.mview[1].xyz + v.z * u.mview[2].xyz;
    s.sunview = vec2f(0.5) + sunview_v.xy * texproj;
    s.sunviewhalf = 0.5 * s.sunview;
    let sunvis = u.eyevec.w;
    s.light = 1.0 - pow(1.0 - sunvis, 2.0);
    let fogstart = u.fogcol.w;
    s.sharpness = mix(60.0, 450.0 + 100.0 * s.sundir.z, saturate(fogstart / 480.0));
    s.strength = raystrength * s.light * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.x - 0.5)) * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.y - 0.5));
    let suncol = u.suncol.xyz;
    s.suncoldisc = vec3f(1.0, 0.76 + 0.24 * u.sunpos.z, 0.54 + 0.46 * u.sunpos.z) * saturate3(suncol / max(suncol.r, max(suncol.g, suncol.b)) * (1.0 - sundiscdesaturate) + vec3f(sundiscdesaturate));
    let fogrange = u.fognearcol.w;
    let waterlevel = u.suncol.w;
    s.azi_horizon = normalize(vec2f(4.0 * fogrange, waterlevel - u.eyepos.z)).y;
    return s;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn stretch(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    var depth = 0.0;
    if s.forward < 0.0 {
        let src = scale * in.tex;
        let rcp = rcpres();
        depth = step(threshold, depth_at(src));
        depth += step(threshold, depth_at(src + vec2f(rcp.x, 0.0)));
        depth += step(threshold, depth_at(src + vec2f(0.0, rcp.y)));
        depth += step(threshold, depth_at(src + rcp));
        depth *= 0.25;
    }
    return vec4f(0.0, 0.0, 0.0, depth);
}

@fragment fn blur_r_half(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let radial = normalize(in.tex - s.sunviewhalf) * rcpres().yx;
    var alpha = 0.3333 * pass_at(in.tex).a;
    alpha += 0.2222 * pass_at(in.tex + radial).a;
    alpha += 0.2222 * pass_at(in.tex - radial).a;
    alpha += 0.1111 * pass_at(in.tex + 2.0 * radial).a;
    alpha += 0.1111 * pass_at(in.tex - 2.0 * radial).a;
    return vec4f(0.0, 0.0, 0.0, alpha);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    var screendir = in.tex - s.sunview;
    let screendist = length(screendir * vec2f(1.0, s.raspect));
    screendir /= screendist;
    let sunr = min(raysunradius, screendist);
    var l = 0.0;
    for (var i = 1; i <= N; i++) {
        let sundist = f32(i) / f32(N) * sunr;
        let at = clamp(s.sunview + sundist * screendir, vec2f(0.0), vec2f(1.0)) * rscale;
        l += pass_at(at).a * exp(-((screendist - sundist) / (rayfalloffconst + sundist)) * rayfalloff) * pow(1.0 - saturate(sundist / raysunradius), raysunfalloff);
    }
    l *= s.strength / f32(N) * (screendist / raysunradius * (1.0 - centervis) + centervis);
    var col = vec4f(u.suncol.r, 0.8 * u.suncol.g, 0.8 * u.suncol.b, l);
    col = vec4f(col.rgb * (1.0 + brightnessadd * pow(col.a, 3.0)), col.a);
    return col;
}

@fragment fn blur_t(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let tangent = normalize(in.tex - s.sunview).yx * vec2f(rcpres().y, -rcpres().x);
    var col = 0.3333 * pass_at(in.tex);
    col += 0.2222 * pass_at(in.tex + tangent);
    col += 0.2222 * pass_at(in.tex - tangent);
    col += 0.1111 * pass_at(in.tex + 2.0 * tangent);
    col += 0.1111 * pass_at(in.tex - 2.0 * tangent);
    return col;
}

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let s = statics();
    let ray = pass_at(in.tex);
    var col = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    col *= saturate(1.0 - sunrayocclude * ray.a);
    col = saturate3(col + ray.rgb * ray.a);
    if s.forward < 0.0 {
        var screendir = in.tex - s.sunview;
        screendir.y *= s.raspect;
        var occl = s.light * step(threshold, depth_at(in.tex));
        occl *= saturate(exp2(s.sharpness * (sundiscradius - length(screendir))));
        if occl > 0.004 {
            var scol = s.suncoldisc * sundiscbrightness;
            let azi = normalize(to_world(in.tex)).z;
            occl *= smoothstep(-0.005, 0.010, azi - s.azi_horizon);
            let gb = smoothstep(-0.03, 0.090, azi - s.azi_horizon);
            scol = vec3f(scol.r, scol.g * gb, scol.b * gb);
            col = mix(col, scol, sundiscocclude * occl);
        }
    }
    return vec4f(col, 1.0);
}
";

/// Bloom, fine, by Hrnchamd, v2, as `Bloom Fine.fx` has it: the energy over a threshold,
/// a vertical and a horizontal blur, the sum in a linear space.
const BLOOM_FINE: &str = "
const bloomThreshold: f32 = 0.48; // file
const bloomLevel: f32 = 0.23; // file
const bloomColourSens: f32 = 0.39; // file
const bloomGamma: f32 = 2.45; // file

fn shader_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, point_clamp, t, 0.0);
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn energy_levels(in: Quad) -> @location(0) vec4f {
    var radiance = shader_at(in.tex);
    let depth = textureSampleLevel(depthframe, point_clamp, in.tex, 0.0).r;
    let lum = dot(vec3f(0.27, 0.54, 0.19), radiance.rgb);
    // Mix with luminance to avoid oversaturated single channels.
    radiance = mix(vec4f(lum), radiance, bloomColourSens);
    // Cut sky/distant illumination down.
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    radiance *= 1.0 - 0.45 * saturate(2.0 * (depth - fogstart) / (fogrange - fogstart));
    return clamp(radiance - bloomThreshold, vec4f(0.0), vec4f(1.0));
}

@fragment fn blur_vert(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(0.0, rcpres().y * taps[i]));
    }
    return radiance;
}

@fragment fn blur_horz_combine(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var base = shader_at(in.tex);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(rcpres().x * taps[i], 0.0));
    }
    // Mix in linear space.
    base = pow(max(base, vec4f(0.0)), vec4f(bloomGamma));
    return pow(max(base + bloomLevel * radiance, vec4f(0.0)), vec4f(1.0 / bloomGamma));
}
";

/// Screen space ambient occlusion, the fast version with 8 taps, from ssao v09 by Knu, as
/// `SSAO Fast.fx` has it with BLUR and ROTATE: the occlusion, two smart blurs, the sum.
const SSAO_FAST: &str = "
const N: i32 = 8;
const R: f32 = 15.0; // file
const multiplier: f32 = 1.8; // file
const occlusion_falloff: f32 = 20.0; // file
const blur_falloff: f32 = 0.1; // file
const blur_radius: f32 = 5.0; // file
const depth_scale: f32 = 10000.0;
const eps: f32 = 1e-6;
const sky: f32 = 1e6;
const M: i32 = 8;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let xy = depth * (tex - 0.5) * invproj();
    return vec3f(xy, depth);
}

fn from_view(view: vec3f) -> vec2f {
    return (view.xy / view.z) / invproj() + vec2f(0.5, 0.5);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 8>(
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    let rnd = textureSampleLevel(tex1, point_wrap, tex / rcp / 8.0, 0.0).xyz * 2.0 - 1.0;
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = reflect(dirs[j] * R, rnd);
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray));
        // Bias occ.z to avoid self-occlusion errors from point sampling depth.
        let diff = (pos.z + ray.z) - 1.00025 * occ.z;
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 8>(
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = blur_radius * (2.0 * data.a - 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) / depth / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let final_ao = fog * multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let result = mix(textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb, u.fognearcol.xyz * (1.0 - fog), final_ao);
    return vec4f(result, 1.0);
}
";

/// The part that the two underwater shaders share: the ray of a pixel in the world for a
/// depth of 1 (`toWorld`), the samplers as the effect declares them (linear or point, with
/// the wrap addressing that Direct3D 9 has without a declaration), and the wobble of the
/// picture from the wave texture. `pow(x, 32)` of the effect is repeated squaring: its base
/// is negative on a half of the screen.
macro_rules! underwater_common {
    () => {
        "
fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}
fn water_at(t: vec3f) -> vec4f {
    return textureSampleLevel(watertexture, linear_wrap, t, 0.0);
}
fn wobbled(tex: vec2f) -> vec2f {
    let time = u.sunvec.w;
    var wobble = 0.01 * (2.0 * water_at(vec3f(0.25 * tex, 0.2 * time)).rg - vec2f(1.0));
    let x = 2.0 * tex - vec2f(1.0);
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    wobble *= vec2f(1.0) - x16 * x16;
    return tex + wobble;
}
"
    };
}

/// Underwater Interior Effects by Hrnchamd, v1, as `Underwater Interior Effects.fx` has it:
/// the wobble of the picture, and caustics near the surface of the water.
const UNDERWATER_INTERIOR: &str = concat!(underwater_common!(), "
@fragment fn wobble(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let waterlevel = u.suncol.w;
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    let d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let v = u.eyepos.xyz + d * to_world(tex);
    let k = 1.0 - water_at(vec3f(v.xy / 1783.0, 0.4 * time)).b;
    return c * (1.0 - 0.2 * step(v.z, waterlevel - 1.0) * saturate(exp((v.z - waterlevel) / 100.0)) * k);
}
");

/// Underwater Effects by Hrnchamd, v2, as `Underwater Effects.fx` has it: the wobble of the
/// picture, and the rays of the light from three shells of the wave texture. The effect
/// takes the water of an exterior at the height 0.
const UNDERWATER: &str = concat!(underwater_common!(), "
const lightcolour: vec4f = vec4f(0.6, 0.91, 1.0, 0.0);

fn fetch(tex: vec3f, uw: f32) -> f32 {
    let time = u.sunvec.w;
    let c = water_at(vec3f(tex.xy / 3308.0, 0.22 * time)).b;
    return pow(c, 0.25) * saturate(exp(tex.z / 90.0)) * saturate(1.0 + tex.z / uw);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let sunvis = u.eyevec.w;
    let suncol = u.suncol.xyz;
    let sunpos = u.sunpos.xyz;
    let eyepos = u.eyepos.xyz;
    let lf = (0.2 + 0.8 * sunvis) * (0.32 * suncol.r + 0.47 * suncol.g + 0.21 * suncol.b);
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    var d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    var v = to_world(tex);
    let e = vec3f(0.1, 0.1, 1.0) * eyepos;

    d = min(d, -eyepos.z / max(1e-5, v.z));
    v = v / length(v.xy);
    let r = -sunpos.xy - v.xy * dot(-sunpos.xy, v.xy);

    var shellpos = e + 800.0 * v;
    shellpos = vec3f(shellpos.xy + 0.8 * r * shellpos.z, shellpos.z);
    var rayz = fetch(shellpos, min(eyepos.z, 400.0)) * saturate(d / 800.0) * 0.294;

    var shellpos2 = e + 1400.0 * v;
    shellpos2 = vec3f(shellpos2.xy + 1.0 * r * shellpos2.z, shellpos2.z);
    rayz += fetch(shellpos2, 2.0 * eyepos.z) * saturate(d / 800.0 - 1.0) * 0.132;

    var shellpos3 = e + 2000.0 * v;
    shellpos3 = vec3f(shellpos3.xy + 1.2 * r * shellpos3.z, shellpos3.z);
    rayz += fetch(shellpos3, 3.0 * eyepos.z) * saturate(d / 800.0 - 2.0) * 0.065;

    return c + rayz * lf * lightcolour;
}
");

/// Eye Adaptation (HDR) by Hrnchamd, v4, as `Eye Adaptation (HDR).fx` has it: the exposure
/// from the adaptation point, a film response curve, and less colour in low light.
const EYE_ADAPTATION: &str = "
const neutral_point: f32 = 0.335; // file
const sensitivity: f32 = 0.11; // file
const max_exposure: f32 = 3.2; // file

@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let scotopic = 0.5 * saturate(exposure / max_exposure - 0.66666);
    let c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;
    // More dynamic range in low light.
    var g = exposure * c + (1.0 - exposure) * (c * c * c * c);
    // The toe and the knee of a film response.
    g = (((0.727408 * g + vec3f(-2.05477)) * g + vec3f(1.63747)) * g + vec3f(0.712032)) * g;
    // Less colour when the exposure is high.
    g = mix(g, vec3f(dot(g, vec3f(0.33333))), scotopic);
    return vec4f(saturate3(g), 1.0);
}
";

/// Screen space ambient occlusion, the high-quality version with 16 taps at full resolution,
/// from ssao v09 by Knu, as `SSAO HQ.fx` has it with BLUR and ROTATE: the occlusion, two
/// smart blurs, the sum.
const SSAO_HQ: &str = "
const N: i32 = 16;
const R: f32 = 15.0; // file
const multiplier: f32 = 1.8; // file
const occlusion_falloff: f32 = 15.0; // file
const blur_falloff: f32 = 0.06; // file
const blur_radius: f32 = 4.0; // file
const depth_scale: f32 = 10000.0;
const sky: f32 = 1e6;
const M: i32 = 12;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

// The depth with the linear filter of the sampler of the effect: the depth texture takes
// no filter here (32-bit float), so the four texels are mixed by hand.
fn depth_linear(t: vec2f) -> f32 {
    let size = vec2f(textureDimensions(depthframe));
    let p = t * size - 0.5;
    let i = vec2i(floor(p));
    let f = fract(p);
    let limit = vec2i(size) - 1;
    let a = textureLoad(depthframe, clamp(i, vec2i(0), limit), 0).r;
    let b = textureLoad(depthframe, clamp(i + vec2i(1, 0), vec2i(0), limit), 0).r;
    let c = textureLoad(depthframe, clamp(i + vec2i(0, 1), vec2i(0), limit), 0).r;
    let d = textureLoad(depthframe, clamp(i + vec2i(1, 1), vec2i(0), limit), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = depth_linear(tex);
    let xy = depth * (tex - 0.5) * invproj();
    return vec3f(xy, depth);
}

fn from_view(view: vec3f) -> vec2f {
    return (view.xy / view.z) / invproj() + vec2f(0.5, 0.5);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 16>(
        vec3f(-0.00941, -0.00326, -0.05597), vec3f(0.11686, 0.00831, 0.04915),
        vec3f(-0.08125, -0.24638, 0.30141), vec3f(0.35193, 0.29639, 0.47544),
        vec3f(0.32063, -0.70203, -0.40622), vec3f(-0.37344, -0.18112, 0.37140),
        vec3f(-0.73605, -0.39320, 0.04992), vec3f(0.02274, 0.21583, 0.19429),
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    let rnd = textureSampleLevel(tex1, point_wrap, tex / rcp / 8.0, 0.0).xyz * 2.0 - 1.0;
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = reflect(dirs[j] * R, rnd);
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray));
        let diff = pos.z + ray.z - occ.z;
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = blur_radius * (2.0 * data.a - 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) / depth / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let final_ao = fog * multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let result = mix(textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb, u.fognearcol.xyz * (1.0 - fog), final_ao);
    return vec4f(result, 1.0);
}
";

/// Depth of Field v12 by Knu, as `Depth of Field.fx` has it with ROTATE: the blur radius of
/// each pixel from the eye focused on the centre of the picture (into the alpha of the
/// first pass), then a blur with 12 taps turned by the noise texture.
const DEPTH_OF_FIELD: &str = "
const fr: f32 = 60.0; // file
const fp: f32 = 60.0; // file
const fpa: f32 = 10.0; // file
const pupil: f32 = 0.006; // file
const blur_radius: f32 = 0.273; // file
const blur_falloff: f32 = 2.0; // file
const R: f32 = 6.0; // file
const k: f32 = 0.00001; // file
const unit2m: f32 = 0.0142;
const M: i32 = 12;

// The blur radius for the width of the picture, the same at each resolution.
fn rfixed() -> f32 {
    return R / (1280.0 * rcpres().x);
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

@fragment fn dof(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let t = 2.0 * tan(radians(0.5 * u.eyepos.w));
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    let fogoffset = saturate(-fogstart / (fogrange - fogstart));
    let s = depth_at(vec2f(0.5, 0.5)) * unit2m;
    let z_corr = length(vec3f((tex.x - 0.5) * t, (tex.y - 0.5) * t / rcpres().y * rcpres().x, 1.0));
    let z = z_corr * unit2m * depth_at(tex);
    let savemyhands = smoothstep(0.568, 0.781, z);
    let fpf = clamp(1.0 / s + fr, fp, fp + fpa);
    var c = pupil * (fr - fpf + 1.0 / z) / fr / k * blur_radius;
    let fog = fogoffset * saturate(z / (4.0 * fogrange * unit2m));
    c = min(abs(c / rfixed()) + fog, 1.0) * savemyhands;
    return vec4f(textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb, c);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let tex = in.tex;
    let rcp = rcpres();
    var color = textureSampleLevel(lastpass, linear_mirror, tex, 0.0);
    let c = color.a * rfixed();
    let rnd = normalize(textureSampleLevel(tex1, point_wrap, tex / rcp / 64.0, 0.0).xy * 2.0 + 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let dir = reflect(taps[i], rnd);
        let s_tex = tex + rcp * dir * c;
        let s_color = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0);
        let s_c = s_color.a * rfixed();
        let weight = exp2(-abs(c - s_c) / blur_falloff);
        color += s_color * weight;
        amount += weight;
    }
    return vec4f(color.rgb / amount, 1.0);
}
";

/// trueBloom g6.6 by peachykeen, as `Bloom Soft.fx` has it with its switches as shipped
/// (quality 3, no sky, no hands, dark): the depth cut, a blur of the squares, the bright
/// pass, a blur that spreads with the brightness, the second combination and blur, and the
/// sum. The quirk of `blurHor3` with `rcpres.y` for its two outer taps is kept.
const BLOOM_SOFT: &str = "
const bloom_mult_global: f32 = 0.22; // file
const bloom_mult_uwater: f32 = 1.2; // file
const bloom_mult_inside: f32 = 1.1; // file
const bloom_mult_outside: f32 = 0.8; // file
const bloom_cutoff: f32 = 0.72; // file
const bloom_power: f32 = 1.1; // file

// The switches of the file: 1 blooms the sky, 1 darkens the scene (the divisors of the
// blurs), 1 blooms the hands of the player. The quality of the file is 3.
const BLOOM_FX_SKY: i32 = 0; // file
const BLOOM_FX_DARK: i32 = 1; // file
const BLOOM_FX_HANDS: i32 = 0; // file

// The divisors of the blurs with 6 taps on each side, and with 4.
fn div6() -> f32 {
    if BLOOM_FX_DARK == 1 {
        return 5.65;
    }
    return 5.2;
}
fn div4() -> f32 {
    if BLOOM_FX_DARK == 1 {
        return 5.2;
    }
    return 4.7;
}

fn shader_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, linear_mirror, t, 0.0);
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_mirror, t, 0.0);
}
fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

// Without the sky: the picture goes dark past the fog. Without the hands: it goes dark
// under 50 units.
fn depth_fixed(tex: vec2f) -> vec4f {
    let d = depth_at(tex);
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    var color = shader_at(tex);
    var keep = 1.0;
    if BLOOM_FX_SKY != 1 {
        keep *= saturate((fogrange - d) / (fogrange - fogstart));
    }
    if BLOOM_FX_HANDS != 1 {
        keep *= smoothstep(35.0, 50.0, d);
    }
    color = vec4f(color.rgb * keep, color.a);
    return color;
}

// The six taps on each side of a blur along `step`, with the divisor of the quality 3.
fn blur6(tex: vec2f, step: vec2f) -> vec4f {
    var color = pass_at(tex);
    color += pass_at(tex - step * 1.0) * 0.8;
    color += pass_at(tex + step * 1.0) * 0.8;
    color += pass_at(tex - step * 2.0) * 0.65;
    color += pass_at(tex + step * 2.0) * 0.65;
    color += pass_at(tex - step * 3.0) * 0.4;
    color += pass_at(tex + step * 3.0) * 0.4;
    color += pass_at(tex - step * 4.0) * 0.25;
    color += pass_at(tex + step * 4.0) * 0.25;
    return color;
}

// The spread of the second blur: wider where the picture is bright.
fn spread_of(color: vec4f) -> f32 {
    let spread = 1.0 - (color.r * 0.3) + (color.g * 0.6) + (color.b * 0.1);
    return (smoothstep(0.0, 0.7, spread) * 4.0) + 1.0;
}

@fragment fn fix_sky(in: Quad) -> @location(0) vec4f {
    return depth_fixed(in.tex);
}

@fragment fn blur_hor_i(in: Quad) -> @location(0) vec4f {
    let step = vec2f(rcpres().x, 0.0);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    color /= div6();
    return color * color;
}

@fragment fn blur_vert_i(in: Quad) -> @location(0) vec4f {
    let step = vec2f(0.0, rcpres().y);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    return color / div6();
}

@fragment fn bright_pass(in: Quad) -> @location(0) vec4f {
    let c_b = pass_at(in.tex);
    var c = smoothstep(vec4f(0.0), vec4f(bloom_cutoff), c_b);
    c = pow(c, vec4f(bloom_power));
    c += smoothstep(vec4f(bloom_cutoff), vec4f(0.5 + bloom_cutoff), c_b);
    return vec4f(c.rgb, 1.0);
}

@fragment fn blur_hor_2(in: Quad) -> @location(0) vec4f {
    let spread = spread_of(pass_at(in.tex));
    return blur6(in.tex, vec2f(rcpres().x * spread, 0.0)) / div4();
}

@fragment fn blur_vert_2(in: Quad) -> @location(0) vec4f {
    let spread = spread_of(pass_at(in.tex));
    return blur6(in.tex, vec2f(0.0, rcpres().y * spread)) / div4();
}

@fragment fn recombine_a(in: Quad) -> @location(0) vec4f {
    let c_b = depth_fixed(in.tex);
    var c = smoothstep(vec4f(0.0), vec4f(bloom_cutoff), pass_at(in.tex));
    c += smoothstep(vec4f(bloom_cutoff - 0.5), vec4f(bloom_cutoff + 0.5), c_b);
    return clamp(c, vec4f(0.0), vec4f(1.0));
}

@fragment fn blur_hor_3(in: Quad) -> @location(0) vec4f {
    let rcp = rcpres();
    var color = blur6(in.tex, vec2f(rcp.x, 0.0));
    let outer = vec2f(rcp.y, 0.0);
    color += pass_at(in.tex - outer * 5.0) * 0.15;
    color += pass_at(in.tex + outer * 5.0) * 0.15;
    color += pass_at(in.tex - outer * 6.0) * 0.10;
    color += pass_at(in.tex + outer * 6.0) * 0.10;
    return color / div6();
}

@fragment fn blur_vert_3(in: Quad) -> @location(0) vec4f {
    let step = vec2f(0.0, rcpres().y);
    var color = blur6(in.tex, step);
    color += pass_at(in.tex - step * 5.0) * 0.15;
    color += pass_at(in.tex + step * 5.0) * 0.15;
    color += pass_at(in.tex - step * 6.0) * 0.10;
    color += pass_at(in.tex + step * 6.0) * 0.10;
    return color / div6();
}

@fragment fn bloom_b(in: Quad) -> @location(0) vec4f {
    let scene = shader_at(in.tex);
    var blur = pass_at(in.tex) * bloom_mult_global;
    if u.sunamb.w > 0.5 {
        blur *= bloom_mult_inside;
    } else {
        blur *= bloom_mult_outside;
    }
    if u.sunpos.w > 0.5 {
        blur *= bloom_mult_uwater;
    }
    return scene + blur;
}
";

/// A bloom of this program, not a port: the method of Jimenez (SIGGRAPH 2014, "Next
/// Generation Post Processing in Call of Duty: Advanced Warfare"). A threshold with a soft
/// knee, a chain of six half-size levels with a 13-tap downsample that has no ringing, an
/// upsample with a tent filter that adds each level back, and one add into the picture.
/// The levels are 16-bit floats, so the sum of the levels keeps its range.
const BLOOM_S_A: &str = "
// The brightness where the bloom starts, the width of its soft start, and its strength.
// The picture has 8-bit colours: a flame or a lamp is near 1, a lit wall near 0.5.
const threshold: f32 = 0.5;
const knee: f32 = 0.25;
const intensity: f32 = 0.4;

fn at(t: vec2f) -> vec3f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0).rgb;
}

// The 13 taps of a downsample by two: the centre, the four corners of the texel, and the
// four corners of the four texels around (the source is twice the size of the target).
fn down13(tex: vec2f, rcp: vec2f) -> vec3f {
    let a = at(tex + rcp * vec2f(-2.0, -2.0));
    let b = at(tex + rcp * vec2f(0.0, -2.0));
    let c = at(tex + rcp * vec2f(2.0, -2.0));
    let d = at(tex + rcp * vec2f(-2.0, 0.0));
    let e = at(tex);
    let f = at(tex + rcp * vec2f(2.0, 0.0));
    let g = at(tex + rcp * vec2f(-2.0, 2.0));
    let h = at(tex + rcp * vec2f(0.0, 2.0));
    let i = at(tex + rcp * vec2f(2.0, 2.0));
    let j = at(tex + rcp * vec2f(-1.0, -1.0));
    let k = at(tex + rcp * vec2f(1.0, -1.0));
    let l = at(tex + rcp * vec2f(-1.0, 1.0));
    let m = at(tex + rcp * vec2f(1.0, 1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

// A 3 x 3 tent over the source, which is half the size of the target.
fn tent9(tex: vec2f, rcp: vec2f) -> vec3f {
    var sum = at(tex) * 4.0;
    sum += (at(tex + rcp * vec2f(-1.0, 0.0)) + at(tex + rcp * vec2f(1.0, 0.0)) + at(tex + rcp * vec2f(0.0, -1.0)) + at(tex + rcp * vec2f(0.0, 1.0))) * 2.0;
    sum += at(tex + rcp * vec2f(-1.0, -1.0)) + at(tex + rcp * vec2f(1.0, -1.0)) + at(tex + rcp * vec2f(-1.0, 1.0)) + at(tex + rcp * vec2f(1.0, 1.0));
    return sum / 16.0;
}

// The part of a colour above the threshold, with a soft knee below it.
fn bright(color: vec3f) -> vec3f {
    let brightness = max(color.r, max(color.g, color.b));
    var soft = brightness - threshold + knee;
    soft = clamp(soft, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    let contribution = max(soft, brightness - threshold) / max(brightness, 1e-4);
    return color * contribution;
}

// The first level: the picture through the threshold, at half size. The threshold is on
// each tap, so that one bright pixel does not spread as a square.
@fragment fn prefilter(in: Quad) -> @location(0) vec4f {
    let rcp = rcpres();
    let tex = in.tex;
    let a = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, -2.0), 0.0).rgb);
    let b = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(0.0, -2.0), 0.0).rgb);
    let c = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, -2.0), 0.0).rgb);
    let d = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, 0.0), 0.0).rgb);
    let e = bright(textureSampleLevel(lastshader, linear_clamp, tex, 0.0).rgb);
    let f = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, 0.0), 0.0).rgb);
    let g = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-2.0, 2.0), 0.0).rgb);
    let h = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(0.0, 2.0), 0.0).rgb);
    let i = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(2.0, 2.0), 0.0).rgb);
    let j = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-1.0, -1.0), 0.0).rgb);
    let k = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(1.0, -1.0), 0.0).rgb);
    let l = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(-1.0, 1.0), 0.0).rgb);
    let m = bright(textureSampleLevel(lastshader, linear_clamp, tex + rcp * vec2f(1.0, 1.0), 0.0).rgb);
    return vec4f(e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125, 1.0);
}

// The next level: half the size of the pass before.
@fragment fn down(in: Quad) -> @location(0) vec4f {
    return vec4f(down13(in.tex, target_rcpres() * 0.5), 1.0);
}

// Up the chain: the level below, spread by the tent, plus the level of this size.
@fragment fn up4(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass4, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up3(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass3, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up2(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass2, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up1(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass1, linear_clamp, in.tex, 0.0).rgb, 1.0);
}
@fragment fn up0(in: Quad) -> @location(0) vec4f {
    return vec4f(tent9(in.tex, target_rcpres() * 2.0) + textureSampleLevel(pass0, linear_clamp, in.tex, 0.0).rgb, 1.0);
}

// The picture plus the bloom. The six levels add up to a sum of about 6 at a flat bright
// area; the intensity is for that sum.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let bloom = tent9(in.tex, rcpres() * 2.0) * (intensity / 6.0);
    return vec4f(scene.rgb + bloom, scene.a);
}
";

/// An ambient occlusion of this program, not a port: ground-truth ambient occlusion
/// (Jimenez and others, SIGGRAPH 2016; the form of XeGTAO), at half size. For each pixel,
/// four slices through the view direction; in each, the two horizons found with eight steps
/// each way, and the visible part of the hemisphere of the normal integrated with the cosine
/// weight. A blur that follows the depth, then the picture darkened by the result.
const SSAO_S_A: &str = "
// The radius in units of the game, the strength, the slices and the steps of each side.
const R: f32 = 48.0;
const strength: f32 = 1.4;
const SLICES: i32 = 4;
const STEPS: i32 = 8;
const PI: f32 = 3.14159265;
const depth_scale: f32 = 10000.0;
const sky: f32 = 1e6;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

// The position in the view, with y down as the picture has it.
fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    return vec3f(depth * (tex - 0.5) * invproj(), depth);
}

// A noise without a texture: interleaved gradient noise over the pixels of the target.
fn noise(pixel: vec2f, seed: f32) -> f32 {
    let p = pixel + seed * vec2f(47.0, 17.0);
    return fract(52.9829189 * fract(0.06711056 * p.x + 0.00583715 * p.y));
}

@fragment fn ao(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let pos = to_view(tex);
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - u.suncol.w) < 0.0 {
        return vec4f(1.0, pos.z / depth_scale, 0.0, 1.0);
    }
    // The normal from the depth: the smaller difference on each axis, so that an edge does
    // not give a wrong one.
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    let dx = select(right, left, length(left) < length(right));
    let dy = select(down, up, length(up) < length(down));
    let view_dir = normalize(-pos);
    var normal = normalize(cross(dy, dx));
    if dot(normal, view_dir) < 0.0 {
        normal = -normal;
    }
    // The radius in the picture: in the units of the texture coordinates, by axis.
    let radius = R / (pos.z * invproj());
    let pixel = in.position.xy;
    let turn = noise(pixel, 0.0);
    let jitter = noise(pixel, 1.0);
    var visibility = 0.0;
    for (var s = 0; s < SLICES; s++) {
        let phi = (f32(s) + turn) * PI / f32(SLICES);
        let slice = vec2f(cos(phi), sin(phi));
        // The plane of the slice holds the view direction; the normal projected into it.
        let direction = vec3f(slice, 0.0);
        let ortho = direction - dot(direction, view_dir) * view_dir;
        let axis = normalize(cross(ortho, view_dir));
        let projected = normal - axis * dot(normal, axis);
        let projected_length = length(projected);
        let cos_n = clamp(dot(projected, view_dir) / max(projected_length, 1e-5), -1.0, 1.0);
        let n = sign(dot(ortho, projected)) * acos(cos_n);
        // The horizons: the highest sample on each side, as the cosine of its angle to the
        // view direction; a sample past the radius counts as the floor of the slice. The
        // steps are denser near the pixel, and the first is one texel of the target at
        // least: a sample on the texel of the pixel itself has no direction.
        var horizon0 = cos(n + 0.5 * PI);
        var horizon1 = cos(n - 0.5 * PI);
        let texel = min(target_rcpres().x / radius.x, target_rcpres().y / radius.y);
        for (var i = 0; i < STEPS; i++) {
            let t = (f32(i) + jitter) / f32(STEPS);
            let offset = slice * radius * max(t * t, texel);
            let p0 = to_view(tex + offset) - pos;
            let p1 = to_view(tex - offset) - pos;
            let d0 = length(p0);
            let d1 = length(p1);
            let w0 = select(saturate(2.0 - 2.0 * d0 / R), 0.0, d0 < 1e-3);
            let w1 = select(saturate(2.0 - 2.0 * d1 / R), 0.0, d1 < 1e-3);
            horizon0 = max(horizon0, mix(cos(n + 0.5 * PI), dot(p0, view_dir) / max(d0, 1e-5), w0));
            horizon1 = max(horizon1, mix(cos(n - 0.5 * PI), dot(p1, view_dir) / max(d1, 1e-5), w1));
        }
        var h0 = -acos(clamp(horizon1, -1.0, 1.0));
        var h1 = acos(clamp(horizon0, -1.0, 1.0));
        h0 = n + max(h0 - n, -0.5 * PI);
        h1 = n + min(h1 - n, 0.5 * PI);
        let arc0 = (cos_n + 2.0 * h0 * sin(n) - cos(2.0 * h0 - n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin(n) - cos(2.0 * h1 - n)) * 0.25;
        visibility += projected_length * (arc0 + arc1);
    }
    visibility = saturate(visibility / f32(SLICES));
    return vec4f(pow(visibility, strength), pos.z / depth_scale, 0.0, 1.0);
}

// A blur along one axis that keeps the edges: a tap counts less the more its depth differs.
fn blur(tex: vec2f, step: vec2f) -> vec4f {
    let centre = textureSampleLevel(lastpass, point_clamp, tex, 0.0);
    let depth = centre.g;
    var weights = array<f32, 5>(0.2270, 0.1945, 0.1216, 0.0540, 0.0162);
    var total = centre.r * weights[0];
    var amount = weights[0];
    for (var i = 1; i < 5; i++) {
        for (var side = -1.0; side <= 1.0; side += 2.0) {
            let s = textureSampleLevel(lastpass, point_clamp, tex + step * f32(i) * side, 0.0);
            let w = weights[i] * exp2(-abs(s.g - depth) / max(depth, 1e-4) / 0.03);
            total += s.r * w;
            amount += w;
        }
    }
    return vec4f(total / amount, depth, 0.0, 1.0);
}

@fragment fn blur_x(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(target_rcpres().x, 0.0));
}

@fragment fn blur_y(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(0.0, target_rcpres().y));
}

// The picture darkened by the occlusion; the fog of the near cells takes it away with the
// distance, as the fog takes the light away.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let occlusion = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0).r;
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(scene.rgb * mix(1.0, occlusion, fog), scene.a);
}
";

/// A depth of field of this program, not a port: a thin lens focused where the picture is
/// looked at (its centre), the circle of confusion of each pixel from its depth, a gather
/// over a disk at half size that lets a near object spread over what is behind it and keeps
/// a far object behind what is in front (the near and the far field apart, as Unity's post
/// processing does), a small filter against the noise of the disk, and the blend into the
/// picture by the circle of confusion.
const DOF_S_A: &str = "
// The lens, in units of the game (1 unit is 1.42 cm): the focal length of a 50 mm lens, the
// diameter of its aperture at f/2, and the width of a full-frame sensor. The largest circle
// in pixels of the half-size target; the taps of the disk; and the depth below which
// nothing blurs (the hands of the player).
const focal: f32 = 3.5;
const aperture: f32 = 1.76;
const sensor: f32 = 2.53;
const max_radius: f32 = 8.0;
const TAPS: i32 = 48;
const hands_near: f32 = 40.0;
const hands_far: f32 = 55.0;

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

fn noise(pixel: vec2f) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * pixel.x + 0.00583715 * pixel.y));
}

// The circle of confusion of a depth as the thin lens gives it, over the largest circle:
// negative in front of the focus, positive behind it. A far focus gives small circles, a
// near one (a table, a face) large ones, as a camera does.
fn circle(z: f32) -> f32 {
    let focus = depth_at(vec2f(0.5, 0.5));
    let pixels_per_unit = target_size().x / sensor;
    let scale = pixels_per_unit * aperture * focal / max(focus - focal, 1.0);
    let c = clamp(scale * (z - focus) / max(z, 1e-3) / max_radius, -1.0, 1.0);
    return c * smoothstep(hands_near, hands_far, z);
}

// The picture at half size with its circle in alpha.
@fragment fn coc(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, linear_clamp, in.tex, 0.0).rgb;
    return vec4f(color, circle(depth_at(in.tex)));
}

// The gather over the disk. A tap of the near field counts where its own circle reaches
// this pixel; a tap of the far field counts where its circle and the circle of this pixel
// both reach it, so that a sharp near object stays sharp against a blurred far one.
@fragment fn gather(in: Quad) -> @location(0) vec4f {
    let rcp = target_rcpres();
    let centre = textureSampleLevel(lastpass, point_clamp, in.tex, 0.0);
    let centre_circle = centre.a * max_radius;
    let turn = noise(in.position.xy) * 6.2831853;
    var far_sum = vec3f(0.0);
    var far_weight = 0.0;
    var near_sum = vec3f(0.0);
    var near_weight = 0.0;
    for (var i = 0; i < TAPS; i++) {
        // A Vogel disk: the taps spread evenly, turned by the noise of the pixel.
        let r = sqrt((f32(i) + 0.5) / f32(TAPS)) * max_radius;
        let theta = f32(i) * 2.39996323 + turn;
        let offset = vec2f(cos(theta), sin(theta)) * r;
        let s = textureSampleLevel(lastpass, linear_clamp, in.tex + offset * rcp, 0.0);
        let s_circle = s.a * max_radius;
        let far_reach = max(min(s_circle, centre_circle), 0.0);
        let far_w = saturate((far_reach - r + 2.0) / 2.0);
        let near_w = saturate((-s_circle - r + 2.0) / 2.0);
        far_sum += s.rgb * far_w;
        far_weight += far_w;
        near_sum += s.rgb * near_w;
        near_weight += near_w;
    }
    let far = far_sum / max(far_weight, 1e-4);
    let near = near_sum / max(near_weight, 1e-4);
    // The near field covers the pixel in the measure of the taps that reach it.
    let cover = saturate(near_weight / f32(TAPS) * 2.0);
    return vec4f(mix(far, near, cover), cover);
}

// A 3 x 3 tent against the noise of the disk.
@fragment fn postfilter(in: Quad) -> @location(0) vec4f {
    let rcp = target_rcpres();
    var sum = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0) * 4.0;
    sum += (textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, 0.0), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, 0.0), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(0.0, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(0.0, rcp.y), 0.0)) * 2.0;
    sum += textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, -rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(-rcp.x, rcp.y), 0.0) + textureSampleLevel(lastpass, linear_clamp, in.tex + vec2f(rcp.x, rcp.y), 0.0);
    return sum / 16.0;
}

// The picture and the blurred picture, by the far circle of the pixel and the cover of the
// near field.
@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let scene = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let blurred = textureSampleLevel(lastpass, linear_clamp, in.tex, 0.0);
    let far_circle = max(circle(depth_at(in.tex)), 0.0) * max_radius;
    let far_w = smoothstep(0.0, 2.0, far_circle);
    let near_w = blurred.a;
    let w = far_w + near_w - far_w * near_w;
    return vec4f(mix(scene.rgb, blurred.rgb, w), scene.a);
}
";

/// Screen space ambient occlusion, ssao v09 by Knu, as `MGG SSAO.fx` of the Morrowind
/// Graphics Guide has it with BLUR and without ROTATE: the occlusion, one smart blur, the
/// sum. The file reads the depth with a point filter, and has no texture.
// The SSAO HQ of the STEP Shaders Compilation: the code of MGG SSAO with 32 samples, the
// rotation of the rays (two blur passes) and another fog power. Its file has the name of the
// SSAO HQ of MGE XE.
const SSAO_HQ_STEP: &str = "
const N: i32 = 32;
const R: f32 = 15.0; // Max ray radius, world units // file
const multiplier: f32 = 1.4; // Overall strength. 1.0 is the correct physical value // file
const occlusion_falloff: f32 = 15.0; // More means less precision, and more strength // file
const blur_falloff: f32 = 5.0; // Same for blur // file
const blur_radius: f32 = 4.0; // In pixels // file
const depth_scale: f32 = 10000.0;
const eps: f32 = 1e-6;
const sky: f32 = 1e6;
const M: i32 = 12;

// The `t` of the effect: a value from the field of view.
fn t() -> f32 {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov));
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let x = (tex.x - 0.5) * depth * t();
    let y = (tex.y - 0.5) * depth * t() / rcpres().y * rcpres().x;
    return vec3f(x, y, depth);
}

fn from_view(view: vec3f) -> vec2f {
    let x = (view.x / t() / view.z) + 0.5;
    let y = (view.y / t() / view.z * rcpres().y / rcpres().x) + 0.5;
    return vec2f(x, y);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 32>(
        vec3f(-0.48710, 0.29981, -0.25607), vec3f(0.03163, 0.02985, 0.15875),
        vec3f(-0.18035, -0.06732, 0.58081), vec3f(0.17450, -0.02599, -0.33065),
        vec3f(0.06349, -0.03883, 0.03387), vec3f(0.10394, 0.69917, -0.16286),
        vec3f(0.04695, 0.36486, -0.33281), vec3f(0.31748, -0.27097, -0.03810),
        vec3f(-0.07172, 0.09571, -0.00005), vec3f(-0.06029, 0.38936, 0.07889),
        vec3f(0.13681, -0.30447, 0.04381), vec3f(-0.09649, -0.92119, 0.34473),
        vec3f(-0.56596, -0.03144, 0.18844), vec3f(-0.10715, 0.25182, -0.13061),
        vec3f(0.59962, 0.21747, -0.07792), vec3f(-0.03793, -0.03653, -0.06986),
        vec3f(-0.00941, -0.00326, -0.05597), vec3f(0.11686, 0.00831, 0.04915),
        vec3f(-0.08125, -0.24638, 0.30141), vec3f(0.35193, 0.29639, 0.47544),
        vec3f(0.32063, -0.70203, -0.40622), vec3f(-0.37344, -0.18112, 0.37140),
        vec3f(-0.73605, -0.39320, 0.04992), vec3f(0.02274, 0.21583, 0.19429),
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * sqrt(1.0 - u.eyevec.z * u.eyevec.z) + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    // ROTATE of the file: a turn of the rays by a noise texture.
    let rnd = normalize(textureSampleLevel(tex1, linear_wrap, tex / rcp / 8.0, 0.0).xyz * 2.0 - vec3f(1.0, 1.0, 1.0));
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = reflect(dirs[j] * R, rnd);
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray)) - pos;
        let diff = ray.z - occ.z - 1.0; // -1 avoid self-occlusion
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = data.a * 2.0 - 1.0;
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * blur_radius * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) * depth_scale / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let final_ao = multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    var fog = saturate((fogrange - dist) / (fogrange - fogstart));
    if use_expfog() {
        fog = saturate(exp(-(dist - fogstart) / (fogrange - fogstart)));
    }
    let result = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb * (1.0 - final_ao * pow(fog, 3.0));
    return vec4f(result, 1.0);
}
";

const MGG_SSAO: &str = "
const N: i32 = 16;
const R: f32 = 15.0; // Max ray radius, world units // file
const multiplier: f32 = 1.8; // Overall strength. 1.0 is the correct physical value // file
const occlusion_falloff: f32 = 15.0; // More means less precision, and more strength // file
const blur_falloff: f32 = 5.0; // Same for blur // file
const blur_radius: f32 = 4.0; // In pixels // file
const depth_scale: f32 = 10000.0;
const eps: f32 = 1e-6;
const sky: f32 = 1e6;
const M: i32 = 12;

// The `t` of the effect: a value from the field of view.
fn t() -> f32 {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov));
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let x = (tex.x - 0.5) * depth * t();
    let y = (tex.y - 0.5) * depth * t() / rcpres().y * rcpres().x;
    return vec3f(x, y, depth);
}

fn from_view(view: vec3f) -> vec2f {
    let x = (view.x / t() / view.z) + 0.5;
    let y = (view.y / t() / view.z * rcpres().y / rcpres().x) + 0.5;
    return vec2f(x, y);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 16>(
        vec3f(-0.00941, -0.00326, -0.05597), vec3f(0.11686, 0.00831, 0.04915),
        vec3f(-0.08125, -0.24638, 0.30141), vec3f(0.35193, 0.29639, 0.47544),
        vec3f(0.32063, -0.70203, -0.40622), vec3f(-0.37344, -0.18112, 0.37140),
        vec3f(-0.73605, -0.39320, 0.04992), vec3f(0.02274, 0.21583, 0.19429),
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * sqrt(1.0 - u.eyevec.z * u.eyevec.z) + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = dirs[j] * R;
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray)) - pos;
        let diff = ray.z - occ.z - 1.0; // -1 avoid self-occlusion
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = data.a * 2.0 - 1.0;
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * blur_radius * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) * depth_scale / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let final_ao = multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    var fog = saturate((fogrange - dist) / (fogrange - fogstart));
    if use_expfog() {
        fog = saturate(exp(-(dist - fogstart) / (fogrange - fogstart)));
    }
    let result = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb * (1.0 - final_ao * pow(fog, 7.0));
    return vec4f(result, 1.0);
}
";

/// Heat haze, as `heathaze.fx` (warp fx 0.1) of the mod "Heat Haze" has it: the picture is
/// warped with a normal map that scrolls on a cylinder around the camera, where the pixel is
/// far enough (between `dmaskint1` and `dmaskint2` units) and near the horizon. The depth
/// of the warped pixel keeps a near object in front of the haze. The depth texture takes a
/// point sampler here: the file names a linear one, but 32-bit floats take no filter.
const HEATHAZE: &str = "
// warp fx 0.1

// warpint, dmaskint1 and dmaskint2 are variables: the mod Heat Haze sets them from Lua.
const wspeed: f32 = -0.06; // file

const PI: f32 = 3.1415926535897932384626433832795;

const hscale: f32 = 0.19; // file

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn cylindrical(tex: vec2f) -> vec2f {
    let worldpos = toWorld(tex);
    let u = -atan2(worldpos.y, worldpos.x) / PI;
    let v = -worldpos.z / length(worldpos.xy);
    return vec2f(0.5 * u + 0.5, hscale * v);
}

@fragment fn warpfx(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let time = u.sunvec.w;
    var depth = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let dmask = smoothstep(dmaskint1(), dmaskint2(), depth);
    depth -= dmaskint1();
    //return float4(abs(depth.xxx), 1);
    let timetick = time * wspeed;
    let uv = cylindrical(tex);

    //float middle = abs(saturate(abs(uv.g)) - 1);
    var middle = 1.0 - min(1.0, abs(uv.g));
    middle = pow(middle, 14.0);

    var warpn = textureSampleLevel(tex1, linear_wrap, uv * vec2f(4.0, -4.0) + vec2f(0.0, timetick), 0.0).xy;

    //return float4(warpn, 0, 1);
    //warpn = lerp(0.0, lerp(float2(0.0, 0.0), warpn * dmask, warpint), dmask * middle);
    warpn = warpn * 2.0 - 1.0;
    warpn = warpn * warpint();

    //return dmask * middle;

    warpn *= saturate(depth);

    var color = vec4f(0.0);

    var wtex = tex + warpn * saturate(dmask * middle);
    let wtexd = tex + 10.0 * warpn * saturate(dmask * middle);
    depth = textureSampleLevel(depthframe, point_wrap, wtexd, 0.0).r;
    depth -= dmaskint1();

    let mask = min(dpdx(depth), dpdy(depth)) * saturate(depth);
    depth = max(depth, mask);
    //return depth.xxxx;
    if (depth < 0.001) {
        wtex = tex;
    }
    //return float4(10000 * (wtex-tex),0,1);

        //return depthm2.rrrr;
        color = textureSampleLevel(lastshader, linear_mirror, wtex, 0.0);

    //color = lerp(tex2D(s0, tex), color, dmask * middle);

    //return float4(warpn.rg * 100, 0, 1.0);
    //return float4(wtex.rg,0, 1.0);
    return vec4f(color.rgb, 1.0);
}
";

/// Special Process by vtastek, v1 ("Special Contrast"), as `Special Process.fx` has it: a
/// bright pass with a threshold that follows the sun colour, a horizontal and a vertical
/// blur with a radius that follows the exposure, a screen combination with a red halo, and a
/// contrast and saturation step.
const SPECIAL_PROCESS: &str = "
// Special Contrast
// vtastek
// v1

// Compatibility: MGE XE 0, fully working

//////////////
// Settings //
//////////////

const neutral_point: f32 = 0.25; // file
const sensitivity: f32 = 0.33; // file
const max_exposure: f32 = 1.6; // file

// The file computes `exposure` and `radius` as static globals from `HDR`. WGSL has no such
// globals: `blurx` and `blury` compute them at their start.
//float radius = 1.66;

const contrast: f32 = 0.3; // file
const saturation: f32 = 1.0; // file

// `s0` of the file is `lastshader` with `point_clamp`; `s1` is `lastpass` with `linear_clamp`.
fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

@fragment fn brightpass(in: Quad) -> @location(0) vec4f {
    let col = sample0(lastshader, point_clamp, in.tex).rgb;

    var lum = dot(col, vec3f(0.33, 0.33, 0.33));

    //return float4(col, 1);
    let sun = mix(0.75, 0.5, u.suncol.x);
    lum = smoothstep(sun - 0.05, sun, lum);

    //float3 fringe = abs(fwidth(final));
    //fringe = saturate(fringe - 0.1);

    //fringe = final +  final * float3(0.2,0.04, 0.0);
    let halcol = smoothstep(0.9, 1.0, pow(length(col), 1.0 / 4.5));
    //final += fringe.rgb;
    return vec4f(pow(col * lum, vec3f(1.5)), halcol);
}

@fragment fn blurx(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let radius = mix(0.1, 1.6, saturate(exposure / max_exposure) - 0.5);

    var rcprcp = 1.0 / rcpres();
    rcprcp *= radius;

    var blr = vec4f(0.0);
    blr += pow(0.026109 * sample0(lastpass, linear_clamp, in.tex + vec2f(-15.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.034202 * sample0(lastpass, linear_clamp, in.tex + vec2f(-13.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.043219 * sample0(lastpass, linear_clamp, in.tex + vec2f(-11.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.052683 * sample0(lastpass, linear_clamp, in.tex + vec2f(-9.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.061948 * sample0(lastpass, linear_clamp, in.tex + vec2f(-7.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.070266 * sample0(lastpass, linear_clamp, in.tex + vec2f(-5.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.076883 * sample0(lastpass, linear_clamp, in.tex + vec2f(-3.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.081149 * sample0(lastpass, linear_clamp, in.tex + vec2f(-1.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.041312 * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.081149 * sample0(lastpass, linear_clamp, in.tex + vec2f(1.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.076883 * sample0(lastpass, linear_clamp, in.tex + vec2f(3.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.070266 * sample0(lastpass, linear_clamp, in.tex + vec2f(5.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.061948 * sample0(lastpass, linear_clamp, in.tex + vec2f(7.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.052683 * sample0(lastpass, linear_clamp, in.tex + vec2f(9.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.043219 * sample0(lastpass, linear_clamp, in.tex + vec2f(11.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.034202 * sample0(lastpass, linear_clamp, in.tex + vec2f(13.5, 0.0) / rcprcp), vec4f(2.0));
    blr += pow(0.026109 * sample0(lastpass, linear_clamp, in.tex + vec2f(15.5, 0.0) / rcprcp), vec4f(2.0));
    //blr += 0.019227*samplePremul( s0, (TexD+float2( 17.5,0.0)/rcprcp.xy) ).xyz;
    //blr += 0.013658*samplePremul( s0, (TexD+float2( 19.5,0.0)/rcprcp.xy) ).xyz;

    blr /= pow(0.93423, 2.0); // renormalize to compensate for the 4 taps I skipped

    return blr * 16.0;
}

@fragment fn blury(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let radius = mix(0.1, 1.6, saturate(exposure / max_exposure) - 0.5);

    var rcprcp = 1.0 / rcpres();

    rcprcp *= radius;
    var blr = vec4f(0.0);
    //blr += 0.013658*sample0( s1, (TexD+float2(0.0,-19.5)/rcprcp.xy) ).xyz;
    //blr += 0.019227*sample0( s1, (TexD+float2(0.0,-17.5)/rcprcp.xy) ).xyz;
    blr += pow(0.026109, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -15.5) / rcprcp);
    blr += pow(0.034202, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -13.5) / rcprcp);
    blr += pow(0.043219, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -11.5) / rcprcp);
    blr += pow(0.052683, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -9.5) / rcprcp);
    blr += pow(0.061948, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -7.5) / rcprcp);
    blr += pow(0.070266, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -5.5) / rcprcp);
    blr += pow(0.076883, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -3.5) / rcprcp);
    blr += pow(0.081149, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, -1.5) / rcprcp);
    blr += pow(0.041312, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 0.0) / rcprcp);
    blr += pow(0.081149, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 1.5) / rcprcp);
    blr += pow(0.076883, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 3.5) / rcprcp);
    blr += pow(0.070266, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 5.5) / rcprcp);
    blr += pow(0.061948, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 7.5) / rcprcp);
    blr += pow(0.052683, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 9.5) / rcprcp);
    blr += pow(0.043219, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 11.5) / rcprcp);
    blr += pow(0.034202, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 13.5) / rcprcp);
    blr += pow(0.026109, 2.0) * sample0(lastpass, linear_clamp, in.tex + vec2f(0.0, 15.5) / rcprcp);
    //blr += 0.019227*sample0( s1, (TexD+float2(0.0, 17.5)/rcprcp.xy) ).xyz;
    //blr += 0.013658*sample0( s1, (TexD+float2(0.0, 19.5)/rcprcp.xy) ).xyz;

    blr /= pow(0.93423, 2.0); // renormalize to compensate for the 4 taps I skipped
    blr *= 16.0;
    let org = sample0(lastshader, point_clamp, in.tex).rgb;

    var col = pow(org, vec3f(2.0));
    blr *= mix(0.0, 1.0, saturate(exposure / max_exposure) - 0.5);
    blr = clamp(blr, vec4f(0.0), vec4f(1.0));
    // The file adds a float4 to a float3: HLSL drops the alpha part.
    col = col + blr.rgb - col * blr.rgb;

    let halcol = blr.rgb * blr.a * 0.1 * vec3f(0.6, 0.0091, 0.0);
    col += saturate3(halcol * 30.0 - col);
    //col += halcol;

    //col = col / (1.0 + col);
    col = pow(col, vec3f(1.0 / 2.0));

    return vec4f(col, 1.0);
}

@fragment fn specialcontrast(in: Quad) -> @location(0) vec4f {
    /*float3 mag = sample0(s1, Tex * 0.1);
    return float4(mag, 1);
    */

    var x = sample0(lastpass, linear_clamp, in.tex);
    let contrasted = (x * x * x * (x * (x * 6.0 - 15.0) + 10.0)).rgb;
    // WGSL does not assign to `x.rgb`: the whole vector is built again.
    x = vec4f(mix(x.rgb, contrasted, contrast), x.a);
    let sat = mix(vec3f(dot(vec3f(0.2125, 0.7154, 0.0721), x.rgb)), x.rgb, saturation);

    var col = x.rgb;

    col = mix(x.rgb, sat, 0.33 * (x.r + x.g + x.b));

    return vec4f(col, 1.0);
}
";

/// Color Mood by shadeMe, v0.2, as `ColorMood.fx` has it: a tint that goes from black to
/// white with the luminance of the pixel, mixed into the picture by the luminance.
const COLOR_MOOD: &str = "
// color mood settings
const fRatio: f32 = 0.4; // 0..1 // file
const moodR: f32 = 0.35; // file
const moodG: f32 = 0.30; // file
const moodB: f32 = 0.25; // file

@fragment fn colorMood(in: Quad) -> @location(0) vec4f {
    let colInput = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    var colMood = vec4f(moodR, moodG, moodB, 1.0);

    // The sum of the four parts of the pixel, the alpha too, over 3, as the file has it.
    let fLum = dot(colInput, vec4f(1.0)) / 3.0;

    colMood = mix(vec4f(0.0), colMood, saturate(fLum * 2.0));
    colMood = mix(colMood, vec4f(1.0), saturate(fLum - 0.5) * 2.0);

    let colOutput = mix(colInput, colMood, saturate(fLum * fRatio));

    return vec4f(colOutput.rgb, 1.0);
}
";

/// Apel's gamma correction shader, as `MGG darker interiors.fx` has it: more saturation, a
/// gamma, and less luminance. Its `mgeflags` 6 keeps it to interiors.
const MGG_DARKER_INTERIORS: &str = "
//tweakables
const gamma: f32 = 1.1; // file
const saturation: f32 = 1.1; // file
const luminance: f32 = 0.9; // file
//tweakables

fn grayscale(col: vec3f) -> vec3f {
    return vec3f(dot(col.rgb, vec3f(0.3, 0.59, 0.11)));
}

@fragment fn gammatoo(in: Quad) -> @location(0) vec4f {
    var c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;
    c = mix(grayscale(c), c, saturation); // Apply saturation
    // The saturation can make a part of `c` negative. `pow` of a negative base has no
    // defined result in WGSL: the base is 0 there.
    c = pow(max(c, vec3f(0.0)), vec3f(gamma)); // Apply gamma

    return vec4f(saturate3(c * luminance), 1.0);
}
";

// The fog boxes of Magicka Expanded: `oj_me_fog_box.fx`. The mod sets the boxes from Lua.
const OJ_ME_FOG_BOX: &str = "
const NUM_FOG_VOLUMES: i32 = 2;

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

// -------------------------------------------------------------- //
//
// https://www.shadertoy.com/view/Ml3GR8
//

fn boxDensity(wpos: vec3f, wdir: vec3f, p: vec3f, r: vec3f, dbuffer: f32) -> f32 {
    let d = wdir;
    var o = wpos - p;

    // ray-box intersection in box space
    let m = 1.0 / d;
    let n = m * o;
    let k = abs(m) * r;
    let ta = -n - k;
    let tb = -n + k;
    var tN = max(max(ta.x, ta.y), ta.z);
    var tF = min(min(tb.x, tb.y), tb.z);
    if tN > tF || tF < 0.0 {
        return 0.0;
    }

    // not visible (behind camera or behind dbuffer)
    if tF < 0.0 || tN > dbuffer {
        return 0.0;
    }

    // clip integration segment from camera to dbuffer
    tN = max(tN, 0.0);
    tF = min(tF, dbuffer);

    // move ray to the intersection point
    o += tN * d;
    tF = tF - tN;
    tN = 0.0;

    // density calculation. density is of the form
    //
    // d(x,y,z) = [1-(x/rx)^2] * [1-(y/ry)^2] * [1-(z/rz)^2];
    //
    // this can be analytically integrable (it's a degree 6 polynomial):

    let a = 1.0 - (o * o) / (r * r);
    let b = -2.0 * (o * d) / (r * r);
    let c = -(d * d) / (r * r);

    let t1 = tF;
    let t2 = t1 * t1;
    let t3 = t2 * t1;
    let t4 = t2 * t2;
    let t5 = t2 * t3;
    let t6 = t3 * t3;
    let t7 = t3 * t4;

    let f = (t1 / 1.0) * (a.x * a.y * a.z)
        + (t2 / 2.0) * (a.x * a.y * b.z + a.x * b.y * a.z + b.x * a.y * a.z)
        + (t3 / 3.0) * (a.x * a.y * c.z + a.x * b.y * b.z + a.x * c.y * a.z + b.x * a.y * b.z + b.x * b.y * a.z + c.x * a.y * a.z)
        + (t4 / 4.0) * (a.x * b.y * c.z + a.x * c.y * b.z + b.x * a.y * c.z + b.x * b.y * b.z + b.x * c.y * a.z + c.x * a.y * b.z + c.x * b.y * a.z)
        + (t5 / 5.0) * (a.x * c.y * c.z + b.x * b.y * c.z + b.x * c.y * b.z + c.x * a.y * c.z + c.x * b.y * b.z + c.x * c.y * a.z)
        + (t6 / 6.0) * (b.x * c.y * c.z + c.x * b.y * c.z + c.x * c.y * b.z)
        + (t7 / 7.0) * (c.x * c.y * c.z);

    return f;
}

// -------------------------------------------------------------- //

// Ordered dithering matrix
const DITHERING = array<f32, 16>(
    0.001176, 0.001961, -0.001176, -0.001699,
    -0.000654, -0.000915, 0.000392, 0.000131,
    -0.000131, -0.001961, 0.000654, 0.000915,
    0.001699, 0.001438, -0.000392, -0.001438,
);

@fragment fn draw(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    var color = textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb;
    // The file reads the depth with a linear filter at the centre of a pixel: the value of
    // that pixel.
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;

    let pos = u.eyepos.xyz;
    let dir = toWorld(tex);

    // gamma -> linear
    color = pow(color, vec3f(2.2));

    // draw fog volumes
    for (var i = 0; i < NUM_FOG_VOLUMES; i++) {
        let center = fogCenters(i);
        let radius = fogRadi(i);
        let density = boxDensity(pos, dir, center, radius, depth);
        if density > 0.0 {
            // apply lua config
            let fogScalar = 1.0 / sqrt(dot(radius, radius));
            let amount = density * fogScalar * fogDensities(i);

            // do the fog stuff
            let fogColor = fogColors(i);
            color = mix(fogColor * fogColor, color, exp(-0.5 * amount));
        }
    }

    // linear -> gamma
    color = pow(color, vec3f(1.0 / 2.2));

    // dithering!
    // VPOS of the file is the pixel: DITHERING[vpos.x % 4][vpos.y % 4].
    let dithering = DITHERING[(i32(in.position.x) % 4) * 4 + i32(in.position.y) % 4];
    color += dithering;

    return vec4f(color, 1.0);
}
";

// The fog boxes of Vapourmist: `tew_fogbox.fx`, the shader of Magicka Expanded with boxes
// that move with the time.
const TEW_FOGBOX: &str = "
// Adapted to MGEXE by G7 and Safebox, modified by tewlwolow

const NUM_FOG_VOLUMES: i32 = 3;

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

// -------------------------------------------------------------- //
//
// https://www.shadertoy.com/view/Ml3GR8
//

fn boxDensity(wpos: vec3f, wdir: vec3f, p: vec3f, r: vec3f, dbuffer: f32) -> f32 {
    let d = wdir;
    var o = wpos - p;

    // ray-box intersection in box space
    let m = 1.0 / d;
    let n = m * o;
    let k = abs(m) * r;
    let ta = -n - k;
    let tb = -n + k;
    var tN = max(max(ta.x, ta.y), ta.z);
    var tF = min(min(tb.x, tb.y), tb.z);
    if tN > tF || tF < 0.0 {
        return 0.0;
    }

    // not visible (behind camera or behind dbuffer)
    if tF < 0.0 || tN > dbuffer {
        return 0.0;
    }

    // clip integration segment from camera to dbuffer
    tN = max(tN, 0.0);
    tF = min(tF, dbuffer);

    // move ray to the intersection point
    o += tN * d;
    tF = tF - tN;
    tN = 0.0;

    // density calculation. density is of the form
    //
    // d(x,y,z) = [1-(x/rx)^2] * [1-(y/ry)^2] * [1-(z/rz)^2];
    //
    // this can be analytically integrable (it's a degree 6 polynomial):

    let a = 1.0 - (o * o) / (r * r);
    let b = -2.0 * (o * d) / (r * r);
    let c = -(d * d) / (r * r);

    let t1 = tF;
    let t2 = t1 * t1;
    let t3 = t2 * t1;
    let t4 = t2 * t2;
    let t5 = t2 * t3;
    let t6 = t3 * t3;
    let t7 = t3 * t4;

    let f = (t1 / 1.0) * (a.x * a.y * a.z)
        + (t2 / 2.0) * (a.x * a.y * b.z + a.x * b.y * a.z + b.x * a.y * a.z)
        + (t3 / 3.0) * (a.x * a.y * c.z + a.x * b.y * b.z + a.x * c.y * a.z + b.x * a.y * b.z + b.x * b.y * a.z + c.x * a.y * a.z)
        + (t4 / 4.0) * (a.x * b.y * c.z + a.x * c.y * b.z + b.x * a.y * c.z + b.x * b.y * b.z + b.x * c.y * a.z + c.x * a.y * b.z + c.x * b.y * a.z)
        + (t5 / 5.0) * (a.x * c.y * c.z + b.x * b.y * c.z + b.x * c.y * b.z + c.x * a.y * c.z + c.x * b.y * b.z + c.x * c.y * a.z)
        + (t6 / 6.0) * (b.x * c.y * c.z + c.x * b.y * c.z + c.x * c.y * b.z)
        + (t7 / 7.0) * (c.x * c.y * c.z);

    return f - (f * 0.41 * sin(u.sunvec.w / 45.0));
}

// -------------------------------------------------------------- //

// Ordered dithering matrix
const DITHERING = array<f32, 16>(
    0.001176, 0.001961, -0.001176, -0.001699,
    -0.000654, -0.000915, 0.000392, 0.000131,
    -0.000131, -0.001961, 0.000654, 0.000915,
    0.001699, 0.001438, -0.000392, -0.001438,
);

@fragment fn draw(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    var color = textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb;
    // The file reads the depth with a linear filter at the centre of a pixel: the value of
    // that pixel.
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;

    let pos = u.eyepos.xyz;
    let dir = toWorld(tex);

    // gamma -> linear
    color = pow(color, vec3f(2.2));

    // draw fog volumes
    for (var i = 0; i < NUM_FOG_VOLUMES; i++) {
        var center = fogCenters(i);
        center.z -= ((f32(i + 2) * 320.0) - 250.0) * cos(u.sunvec.w / 60.0);

        var radius = fogRadi(i);
        radius.z += ((f32(i + 2) * 150.0) - 60.0) * sin(u.sunvec.w / 25.0);
        let density = boxDensity(pos, dir, center, radius, depth);
        if density > 0.0 {
            // apply lua config
            let fogScalar = 1.0 / sqrt(dot(radius, radius));
            let amount = density * fogScalar * fogDensities(i);

            // do the fog stuff
            let fogColor = fogColors(i);
            color = mix(fogColor * fogColor, color, exp(-0.5 * amount));
        }
    }

    // linear -> gamma
    color = pow(color, vec3f(1.0 / 2.2));

    // dithering!
    // VPOS of the file is the pixel: DITHERING[vpos.x % 4][vpos.y % 4].
    let dithering = DITHERING[(i32(in.position.x) % 4) * 4 + i32(in.position.y) % 4];
    color += dithering;

    return vec4f(color, 1.0);
}
";


// The picture under skooma: `skoomaesthesia.fx` of the mod Skoomaesthesia. The mod sets the
// strength, the colour cycle and the blur of the edges from Lua.
const SKOOMAESTHESIA: &str = "
// Skooma Effects

const blurinner: f32 = 0.5; // file

fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

// The file reads the table with tex2Dlod and a level from the coordinate: the table has one
// level, and its sampler has no filter between levels.
fn ClutFunc(colorIN_in: vec3f) -> vec3f {
    var colorIN = colorIN_in;
    let color_offset = vec3f(0.2, 0.4, 0.3);
    colorIN += color_offset * cycle();
    let CLut_pSize = vec2f(0.00390625, 0.0625); // 1 / float2(256, 16);
    var CLut_UV: vec4f;
    colorIN = saturate3(colorIN) * 15.0;
    CLut_UV.w = floor(colorIN.b);
    CLut_UV = vec4f((colorIN.rg + 0.5) * CLut_pSize, CLut_UV.z, CLut_UV.w);
    CLut_UV.x += CLut_UV.w * CLut_pSize.y;
    CLut_UV.z = CLut_UV.x + CLut_pSize.y;
    return mix(sample0(tex1, linear_wrap, CLut_UV.xy).rgb, sample0(tex1, linear_wrap, CLut_UV.zy).rgb, colorIN.b - CLut_UV.w);
}

@fragment fn lut(in: Quad) -> @location(0) vec4f {
    let scene = sample0(lastshader, point_clamp, in.tex);
    let orig = scene.rgb;
    return vec4f(mix(orig, ClutFunc(scene.rgb), intensity()), scene.a);
}

// The 17 taps of the two blurs, along `axis`. The file has them one by one.
fn taps(TexD: vec2f, axis: vec2f, rcprcp: vec2f) -> vec4f {
    var blr = vec4f(0.0);
    blr += 0.026109 * sample0(lastpass, linear_clamp, TexD + axis * -15.5 / rcprcp);
    blr += 0.034202 * sample0(lastpass, linear_clamp, TexD + axis * -13.5 / rcprcp);
    blr += 0.043219 * sample0(lastpass, linear_clamp, TexD + axis * -11.5 / rcprcp);
    blr += 0.052683 * sample0(lastpass, linear_clamp, TexD + axis * -9.5 / rcprcp);
    blr += 0.061948 * sample0(lastpass, linear_clamp, TexD + axis * -7.5 / rcprcp);
    blr += 0.070266 * sample0(lastpass, linear_clamp, TexD + axis * -5.5 / rcprcp);
    blr += 0.076883 * sample0(lastpass, linear_clamp, TexD + axis * -3.5 / rcprcp);
    blr += 0.081149 * sample0(lastpass, linear_clamp, TexD + axis * -1.5 / rcprcp);
    blr += 0.041312 * sample0(lastpass, linear_clamp, TexD);
    blr += 0.081149 * sample0(lastpass, linear_clamp, TexD + axis * 1.5 / rcprcp);
    blr += 0.076883 * sample0(lastpass, linear_clamp, TexD + axis * 3.5 / rcprcp);
    blr += 0.070266 * sample0(lastpass, linear_clamp, TexD + axis * 5.5 / rcprcp);
    blr += 0.061948 * sample0(lastpass, linear_clamp, TexD + axis * 7.5 / rcprcp);
    blr += 0.052683 * sample0(lastpass, linear_clamp, TexD + axis * 9.5 / rcprcp);
    blr += 0.043219 * sample0(lastpass, linear_clamp, TexD + axis * 11.5 / rcprcp);
    blr += 0.034202 * sample0(lastpass, linear_clamp, TexD + axis * 13.5 / rcprcp);
    blr += 0.026109 * sample0(lastpass, linear_clamp, TexD + axis * 15.5 / rcprcp);
    return blr / 0.93423; // renormalize to compensate for the 4 taps I skipped
}

// One blur: `inner` is where the blur of the edges starts.
fn blur(TexD: vec2f, axis: vec2f, inner: f32) -> vec4f {
    let p = TexD * (1.0 - TexD);
    var vig = saturate(p.x * p.y * 15.0);
    vig = smoothstep(inner, 1.0, 1.0 - vig);
    vig = radius() * vig;
    var blr = vec4f(0.0);
    // OPTI of the file: no taps where there is no blur.
    if vig > 0.0 {
        blr = taps(TexD, axis, 1.0 / rcpres() / vig);
    }
    let pcol = sample0(lastpass, linear_clamp, TexD);
    let col = mix(blr, pcol, 1.0 - vig);
    return vec4f(col.rgb, 1.0);
}

@fragment fn blurx(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(1.0, 0.0), 0.70);
}

@fragment fn blury(in: Quad) -> @location(0) vec4f {
    return blur(in.tex, vec2f(0.0, 1.0), blurinner);
}
";

// The shake of the picture: `Disorient.fx` of Kogoruhn Expanded. The mod sets it from Lua.
const DISORIENT: &str = "
// Made by Vtastek
// Want to learn shaders ---> https://thebookofshaders.com/

fn rand(co: vec2f) -> f32 {
    return fract(sin(dot(co, vec2f(12.9898, 78.233))) * 43758.5453);
}

@fragment fn shakervib(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let i = floor(time * sspeed() * srange()); // integer
    let f = fract(time * sspeed() * srange()); // fraction
    let iy = floor(time * sspeed() * srange() + seed()); // integer
    let fy = fract(time * sspeed() * srange()); // fraction
    // The file gives `rand` one number: HLSL makes a vector of two from it.
    let randy = mix(rand(vec2f(i)), rand(vec2f(i + 1.0)), smoothstep(0.0, 1.0, f));
    let randx = mix(rand(vec2f(iy)), rand(vec2f(iy + 1.0)), smoothstep(0.0, 1.0, fy));

    let scaleCenter = vec2f(0.5, 0.5);
    let tex = (in.tex - scaleCenter) * zoom() + scaleCenter;
    let stex = tex + (vec2f(randx, randy) - 0.5) * srange() * 0.0015;
    var final_color = textureSampleLevel(lastshader, point_mirror, stex, 0.0);
    let finalleft = textureSampleLevel(lastshader, point_mirror, stex - vec2f(offset(), 0.0), 0.0);
    let finalright = textureSampleLevel(lastshader, point_mirror, stex + vec2f(offset(), 0.0), 0.0);
    final_color = (1.0 + offset() * vibright()) * mix(final_color, mix(finalleft, finalright, 0.5), min(offset() * intensity(), 0.5));
    return final_color;
}
";


// The view of an invisible player: `Invisibility.fx` of MM - Enhanced Invisibility. The mod
// loads it and sets its strength from Lua.
const INVISIBILITY: &str = "
// invis fx 0.1

const wspeed: f32 = -0.3; // file
const thres: f32 = 0.3; // file
const thresm: f32 = 1.3; // file

const PI: f32 = 3.1415926535897932384626433832795;
const TAU: f32 = 6.28318530718;
const MAX_ITER: i32 = 6;

const hscale: f32 = 0.19; // file

fn sample0(t: texture_2d<f32>, s: sampler, at: vec2f) -> vec4f {
    return textureSampleLevel(t, s, at, 0.0);
}

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

fn cylindrical(tex: vec2f) -> vec2f {
    let worldpos = toWorld(tex);
    let cu = -atan2(worldpos.y, worldpos.x) / PI;
    let cv = -worldpos.z / length(worldpos.xy);
    return vec2f(0.5 * cu + 0.5, hscale * cv);
}

fn waterHighlight(p: vec2f, time: f32) -> f32 {
    var i = p;
    var c = 0.0;
    let foaminess_factor = 1.81;
    let inten = 0.005 * foaminess_factor;

    for (var n = 0; n < MAX_ITER; n++) {
        let t = time * (1.0 - (3.5 / f32(n + 1)));
        i = p + vec2f(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
        c += 1.0 / length(vec2f(p.x / (sin(i.x + t)), p.y / (cos(i.y + t))));
    }
    c = 0.2 + c / (inten * f32(MAX_ITER));
    c = 1.17 - pow(c, 1.4);
    c = pow(abs(c), 8.0);
    return c / sqrt(foaminess_factor);
}

@fragment fn brightpass(in: Quad) -> @location(0) vec4f {
    var col = sample0(lastshader, linear_mirror, in.tex) * 1.4;
    col = max(col - thres, vec4f(0.0));
    col /= 1.0 + col;
    col *= thresm;
    // The file takes the dot of four parts with three: HLSL uses the first three.
    col = mix(col, vec4f(dot(col.rgb, vec3f(0.299, 0.587, 0.114))), 0.7);
    //col.rgb = lerp(col.rgb, float3(0.8,0.6,0.7), saturate(col.rgb * 10));
    return col;
}

@fragment fn warpfx(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let time = u.sunvec.w;
    let isInterior = u.sunamb.w;
    // The file reads the depth with a linear filter at the centre of a pixel: the value of
    // that pixel.
    var dmask = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let inoff = mix(1.0, 0.5, isInterior);
    let fmask = smoothstep(500.0 * inoff, 3000.0 * inoff, dmask);
    let maskint = 1.0 - smoothstep(0.0, 5500.0, dmask);
    dmask = smoothstep(55.0, 60.0, dmask);
    let timetick = time * wspeed;
    let uv = cylindrical(tex);

    let ftime = timetick * 0.4 + 23.0;
    // fmod of HLSL has the sign of its first value, as % of WGSL.
    let p = (uv * TAU) % vec2f(TAU) - 250.0;

    //float middle = abs(saturate(abs(uv.g)) - 1);
    var middle = 1.0 - min(1.0, abs(uv.g));
    middle = pow(middle, 4.0);

    var warpn = sample0(tex1, linear_wrap, uv * vec2f(2.0, -2.0) + vec2f(0.0, timetick)).xy + 0.5;

    warpn = mix(vec2f(1.0), mix(vec2f(1.0, 1.0), warpn * dmask, warpint()), dmask * middle);
    warpn = mix(warpn, vec2f(1.0, 1.0), mix(vec2f(1.0), vec2f(0.95, 0.7), maskint));
    let warpnless = mix(warpn, vec2f(1.0, 1.0), mix(vec2f(1.0), vec2f(0.99, 0.8), maskint));
    let orgorg = sample0(lastshader, linear_mirror, tex);
    let org = sample0(lastshader, linear_mirror, tex * warpnless);
    let px = sample0(lastpass, linear_mirror, tex * warpn);

    let c = waterHighlight(p, ftime);

    let water_color = vec3f(0.0, 0.35, 0.5);

    var colorfog = saturate3(c + water_color);
    colorfog = mix(water_color, colorfog, middle);

    var comb = org.rgb + px.rgb;
    // dot(1.0, warpn) of the file: the sum of the two parts.
    comb = mix(comb, inoff * colorfog, saturate3(fmask + colorfog * 0.0005 + (warpn.x + warpn.y) * 0.0005));

    comb = mix(comb, vec3f(dot(vec3f(0.299, 0.587, 0.114), comb)), 0.6);

    let contrasted = comb * comb * comb * (comb * (comb * 6.0 - 15.0) + 10.0);
    comb = mix(comb, contrasted, 0.5);

    comb = mix(comb, comb * vec3f(0.83, 0.95, 0.87), 3.0);

    comb = mix(orgorg.rgb, comb * 1.2, invisint());
    return vec4f(comb, 1.0);
}
";

// The depth of field of Inspect It: `InspectIt/Bokeh.fx`, the Depth of Field of MGE XE with a
// focus distance and a blur strength that the mod sets from Lua.
const BOKEH: &str = "
const fr: f32 = 60.0; // file
const fp: f32 = 60.0; // file
const fpa: f32 = 10.0; // file
const pupil: f32 = 0.006; // file
const blur_radius: f32 = 0.273; // file
const blur_falloff: f32 = 2.0; // file
const R: f32 = 6.0; // file
const k: f32 = 0.00001; // file
const unit2m: f32 = 0.0142;
const M: i32 = 12;

// The blur radius for the width of the picture, the same at each resolution.
fn rfixed() -> f32 {
    return R / (1280.0 * rcpres().x);
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

@fragment fn dof(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let t = 2.0 * tan(radians(0.5 * u.eyepos.w));
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    let fogoffset = saturate(-fogstart / (fogrange - fogstart));
    let s = focus_distance(); //sample0(s1, float2(0.5, 0.5)).r * unit2m;
    let z_corr = length(vec3f((tex.x - 0.5) * t, (tex.y - 0.5) * t / rcpres().y * rcpres().x, 1.0));
    let z = z_corr * unit2m * depth_at(tex);
    let savemyhands = smoothstep(0.568, 0.781, z);
    let fpf = clamp(1.0 / s + fr, fp, fp + fpa);
    var c = pupil * (fr - fpf + 1.0 / z) / fr / k * blur_radius * focal_length();
    let fog = fogoffset * saturate(z / (4.0 * fogrange * unit2m));
    c = min(abs(c / rfixed()) + fog, 1.0) * savemyhands;
    return vec4f(textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb, c);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 12>(
        vec2f(-0.326212, -0.40581), vec2f(-0.840144, -0.07358),
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.519456, 0.767022), vec2f(0.185461, -0.893124),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let tex = in.tex;
    let rcp = rcpres();
    var color = textureSampleLevel(lastpass, linear_mirror, tex, 0.0);
    let c = color.a * rfixed();
    let rnd = normalize(textureSampleLevel(tex1, point_wrap, tex / rcp / 64.0, 0.0).xy * 2.0 + 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let dir = reflect(taps[i], rnd);
        let s_tex = tex + rcp * dir * c;
        let s_color = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0);
        let s_c = s_color.a * rfixed();
        let weight = exp2(-abs(c - s_c) / blur_falloff);
        color += s_color * weight;
        amount += weight;
    }
    return vec4f(color.rgb / amount, 1.0);
}
";


// The heat and the cold of Ashfall on the picture: `ashfall.fx`. The file has a library of
// functions that its pass does not use; the port has the pass and its one function.
const ASHFALL: &str = "
fn temperatureEffect(uv: vec2f, color: vec3f, temperature: f32) -> vec3f {
    let coldColor = vec3f(0.0, 0.50, 1.0);
    let hotColor = vec3f(1.0, 0.5, 0.0);

    let colorTemperature = mix(coldColor, hotColor, f32(temperature > 0.0));

    let luminosity = dot(color, vec3f(0.299, 0.587, 0.114));
    let distanceFromCenter = distance(uv, vec2f(0.5, 0.5));
    let absTemp = abs(temperature);
    let distanceEffect = saturate(distanceFromCenter * (absTemp / 100.0) - 0.5);
    //Limit the color values to cold/hot depending on temperature
    let fadeAmount = saturate(abs(temperature) / 100.0) * luminosity * 2.0;
    return mix(color, colorTemperature, fadeAmount * distanceEffect);
}

@fragment fn main(in: Quad) -> @location(0) vec4f {
    let color = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    return vec4f(temperatureEffect(in.tex, color.rgb, temperature()), color.a);
}
";

// The two test shaders of the test harness: a fill of the frame with one colour.
const HARNESS_FILL_OPAQUE: &str = "
@fragment fn fill(in: Quad) -> @location(0) vec4f {
    return vec4f(1.0, 0.0, 1.0, 1.0);
}
";

const HARNESS_FILL_SCENE: &str = "
@fragment fn fill(in: Quad) -> @location(0) vec4f {
    return vec4f(0.0, 1.0, 1.0, 1.0);
}
";

// The lens of the Outlander configuration: `Lens Effects.fx`. The file computes a vignette
// and a gradient that it does not use; they are not here.
const LENS_EFFECTS: &str = "
// Special Contrast
// vtastek
// v1

//////////////
// Settings //
//////////////

const dscale: f32 = 1.08; // file
const chromaticDispersionRC: f32 = 0.045; // file
const chromaticDispersionBY: f32 = 0.045; // file
const distortion: f32 = 0.01; // file
const cubicDistortion: f32 = 0.031; // file

@fragment fn barrelvig(in: Quad) -> @location(0) vec4f {
    let ratio = rcpres().x / rcpres().y;
    let scaleFactor = 1.0 / dscale;

    //index of refraction of each color channel, causing chromatic dispersion
    let eta = vec3f(1.0 + chromaticDispersionRC * 0.09, 1.0 + chromaticDispersionRC * 0.06, 1.0 + chromaticDispersionRC * 0.03);
    let eta2 = vec3f(1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.03);

    let tex = 2.0 * (in.tex - 0.5);

    // lens distortion coefficient
    let k = distortion;
    // cubic distortion value
    let kcube = cubicDistortion;

    //APPLY DISTORTION
    let r2 = ratio * ratio * tex.x * tex.x + tex.y * tex.y;
    //compute the cubic distortion
    let f = 1.0 + r2 * (k + kcube * sqrt(r2));

    //Apply CA
    let rCoords = (f * eta.r * eta2.r) * scaleFactor * (tex * 0.5) + 0.5;
    let gCoords = (f * eta.g * eta2.g) * scaleFactor * (tex * 0.5) + 0.5;
    let bCoords = (f * eta.b * eta2.b) * scaleFactor * (tex * 0.5) + 0.5;

    let scene = vec3f(
        textureSampleLevel(lastshader, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastshader, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastshader, linear_clamp, bCoords, 0.0).b);
    return vec4f(scene, 1.0);
}
";

// The Eye Adaptation of the STEP Shaders Compilation: an older file than that of MGE XE,
// with the same name and other code.
const EYE_ADAPTATION_STEP: &str = "
// HDR Eye Adaptation, tuned for Morrowind lighting
// Simulates exposure adjustment
// Hrnchamd
// v3

const neutral_point: f32 = 0.31; // file
const sensitivity: f32 = 1.4; // file

@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let exposure = (1.0 + sensitivity * (neutral_point - u.hdr.x)) * sqrt(u.hdr.y / u.hdr.x);
    let c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;

    // Increase dynamic range in low light
    var g = exposure * c + (1.0 - exposure) * (c * c * c * c);

    // Film response toe/knee contrast
    // 0.727408x^4 - 2.05477x^3 + 1.63747x^2 + 0.712032x
    g = (((0.727408 * g + -2.05477) * g + 1.63747) * g + 0.712032) * g;

    return vec4f(saturate3(g), 1.0);
}
";


// The film look of the Morrowind Graphics Guide: `film.fx`.
const FILM: &str = "
// ---------------------------------------------------------------------------------------
//
//     Film shader (Softlightx2 Version) for video to be used in Avery Lee's Virtualdub
//    GPU Shader filter. v1.6 - 20.00.08 Jukka Korhonen from the original code by Avery Lee.
//
// ---------------------------------------------------------------------------------------

// ------------ TWEAKABLE VARIABLES ------------

const Strenght: f32 = 0.5; // file
const BaseGamma: f32 = 2.2; // file
const Fade: f32 = 0.05; // file
const Contrast: f32 = 1.0; // file
const Saturation: f32 = -0.2; // file
const Bleach: f32 = 0.09; // file
const RedCurve: f32 = 1.0; // file
const GreenCurve: f32 = 1.0; // file
const BlueCurve: f32 = 1.0; // file
const BaseCurve: f32 = 1.0; // file
const EffectGammaR: f32 = 1.0; // file
const EffectGammaG: f32 = 1.0; // file
const EffectGammaB: f32 = 0.75; // file
const EffectGamma: f32 = 1.0; // file
const Linearization: f32 = 2.2; // file

// ------------ END OF TWEAKABLE VARIABLES ------------

// The second row of the matrix XYZ_RGB of the file.
fn GetLuminance(rgb: vec3f) -> f32 {
    return dot(vec3f(0.212671, 0.715160, 0.072169), rgb);
}

// The soft light of the file for one part: `d` selects the curve, `b` is the base.
fn soft(d: f32, b: f32) -> f32 {
    if d < 0.5 {
        return (2.0 * d - 1.0) * (b - b * b) + b;
    }
    return (2.0 * d - 1.0) * (sqrt(b) - b) + b;
}

@fragment fn FilmPass(in: Quad) -> @location(0) vec4f {
    var B = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0);
    let H = vec4f(0.01);

    B = pow(abs(B), vec4f(Linearization));
    B = mix(H, B, Contrast);

    let A = GetLuminance(B.rgb);
    var D = vec4f(A);

    B = pow(abs(B), vec4f(1.0 / BaseGamma));

    let a = RedCurve;
    let b = GreenCurve;
    let c = BlueCurve;
    let d = BaseCurve;

    let y = 1.0 / (1.0 + exp(a / 2.0));
    let z = 1.0 / (1.0 + exp(b / 2.0));
    let w = 1.0 / (1.0 + exp(c / 2.0));
    let v = 1.0 / (1.0 + exp(d / 2.0));

    var C = B;

    D.r = (1.0 / (1.0 + exp(-a * (D.r - 0.5))) - y) / (1.0 - 2.0 * y);
    D.g = (1.0 / (1.0 + exp(-b * (D.g - 0.5))) - z) / (1.0 - 2.0 * z);
    D.b = (1.0 / (1.0 + exp(-c * (D.b - 0.5))) - w) / (1.0 - 2.0 * w);

    D = pow(abs(D), vec4f(1.0 / EffectGamma));

    let Di = 1.0 - D;

    D = mix(D, Di, Bleach);

    D.r = pow(abs(D.r), 1.0 / EffectGammaR);
    D.g = pow(abs(D.g), 1.0 / EffectGammaG);
    D.b = pow(abs(D.b), 1.0 / EffectGammaB);

    C.r = soft(D.r, B.r);
    C.g = soft(D.g, B.g);
    C.b = soft(D.b, B.b);

    var F = mix(B, C, Strenght);

    F = (1.0 / (1.0 + exp(-d * (F - 0.5))) - v) / (1.0 - 2.0 * v);

    let r2R = 1.0 - Saturation;
    let g2R = 0.0 + Saturation;
    let b2R = 0.0 + Saturation;

    let r2G = 0.0 + Saturation;
    let g2G = (1.0 - Fade) - Saturation;
    let b2G = (0.0 + Fade) + Saturation;

    let r2B = 0.0 + Saturation;
    let g2B = (0.0 + Fade) + Saturation;
    let b2B = (1.0 - Fade) - Saturation;

    let iF = F;

    F.r = (iF.r * r2R + iF.g * g2R + iF.b * b2R);
    F.g = (iF.r * r2G + iF.g * g2G + iF.b * b2G);
    F.b = (iF.r * r2B + iF.g * g2B + iF.b * b2B);

    let N = GetLuminance(F.rgb);
    var Cn = F;

    if N < 0.5 {
        Cn = (2.0 * N - 1.0) * (F - F * F) + F;
    } else {
        Cn = (2.0 * N - 1.0) * (sqrt(F) - F) + F;
    }

    Cn = pow(abs(Cn), vec4f(1.0 / Linearization));

    let Fn = mix(B, Cn, Strenght);
    return Fn;
}
";

// The deband of the Morrowind Graphics Guide: `deband_fogaware.fx`.
const DEBAND_FOGAWARE: &str = "
// Deband shader by haasn
// https://github.com/mpv-player/mpv/blob/master/video/out/opengl/video_shaders.c
// Modified and optimized for ReShade by JPulowski
// http://reshade.me/forum/shader-presentation/768-deband
// 1847.64 - Ported to MGE XE 0.10.0
// intensity tied to interiors and fog
// full intensity for interiors, reduced intensity for close view exteriors if fog is closer then 500 units

const fogun: f32 = 470.0; // file
const Threshold: f32 = 0.006; // file
const Range: f32 = 5.3; // file
const Grain: f32 = 0.006; // file
const drandom: i32 = 3914; // file

fn toView(tex: vec2f) -> vec3f {
    let t = 2.0 * tan(0.5 * radians(u.eyepos.w));
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let x = (tex.x - 0.5) * depth * t;
    let y = (tex.y - 0.5) * depth * t / rcpres().y * rcpres().x;
    return vec3f(x, y, depth);
}

fn rand(x: f32) -> f32 {
    return fract(x * 0.024390243);
}

fn permute(x: f32) -> f32 {
    return ((34.0 * x + 1.0) * x) % 289.0;
}

fn average(pos: vec2f, range: f32, h: ptr<function, f32>) -> vec3f {
    // Helper: Compute a stochastic approximation of the avg color around a pixel

    // Compute a random rangle and distance
    let dist = rand(*h) * range;
    *h = permute(*h);
    let dir = rand(*h) * 6.2831853;
    *h = permute(*h);

    let pt = dist * rcpres();
    let o = vec2f(cos(dir), sin(dir));

    // Sample at quarter-turn intervals around the source pixel
    var r = textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(o.x, o.y), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(-o.y, o.x), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(-o.x, -o.y), 0.0).rgb;
    r += textureSampleLevel(lastshader, point_wrap, pos + pt * vec2f(o.y, -o.x), 0.0).rgb;

    // Return the mean
    return 0.25 * r;
}

@fragment fn PS_Deband(in: Quad) -> @location(0) vec4f {
    let texcoord = in.tex;
    let isInterior = u.sunamb.w;
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;

    // Initialize the PRNG by hashing the position + a random uniform
    let m = vec3f(texcoord, f32(drandom) * 0.0002) + 1.0;
    var h = permute(permute(permute(m.x) + m.y) + m.z);

    // Sample the source pixel
    var col = textureSampleLevel(lastshader, point_wrap, texcoord, 0.0).rgb;

    // Sample the average pixel and use it instead of the original if the difference is below the given threshold
    let avg = average(texcoord, Range, &h);
    let diff = abs(col - avg);
    let dist = length(toView(texcoord));
    var fog = saturate((fogrange - dist) / (fogrange - fogstart));
    fog = saturate(1.0 - fog);
    fog = mix(fog, isInterior, isInterior);
    let fogac = saturate(-fogstart * 0.1 + fogun * 0.1);
    fog = mix(1.0, fog, fogac);

    col = select(avg, col, diff > vec3f(fog * Threshold));

    if Grain > 0.0 {
        // Add some random noise to smooth out residual differences
        var noise: vec3f;
        noise.x = rand(h);
        h = permute(h);
        noise.y = rand(h);
        h = permute(h);
        noise.z = rand(h);
        h = permute(h);

        col += Grain * (noise - 0.5);
    }

    return vec4f(col, 1.0);
}
";


// gammavtdt of the STEP Shaders Compilation: a bloom from a quarter-size copy in the corner
// of the picture, a tone curve with the adaptation point, and a lens pass.
const GAMMAVTDT: &str = "
const scale: f32 = 4.0; // file
const BloomBoost: f32 = 0.07; // file
const thre: f32 = 0.1; // file
const intenb: f32 = 1.03; // file
const bsize: f32 = 1.0; // file
const bsize2: f32 = 1.0; // file
const dscale: f32 = 1.05; // file
const chromaticDispersionRC: f32 = 0.02; // file
const chromaticDispersionBY: f32 = 0.01; // file
const distortion: f32 = 0.0001; // file
const cubicDistortion: f32 = 0.0001; // file

fn midlevel() -> vec3f {
    return pow(sqrt(1.0 + vec3f(0.05, 0.05, 0.05)) - vec3f(0.1, 0.1, 0.1), vec3f(2.2));
}

fn Uncharted2Tonemap(x: vec3f) -> vec3f {
    let A = 0.25;
    let B = 0.50;
    let C = 0.10;
    let D = 0.20;
    let E = 0.02;
    let F = 0.30;
    return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F;
}

fn screen(org: vec3f, bloom: vec3f) -> vec3f {
    return 1.0 - (1.0 - org) * (1.0 - bloom);
}

fn bright(t: vec2f) -> vec3f {
    return pow(textureSampleLevel(lastshader, point_clamp, t, 0.0).rgb * intenb, vec3f(2.2)) - midlevel() * thre;
}

// The bright part of the picture, at a quarter of its size in the corner of the target.
@fragment fn ThreshDownScale(in: Quad) -> @location(0) vec4f {
    let Tex = in.tex;
    var ColorNew = vec3f(0.0);
    if !(Tex.x * scale > 1.05 || Tex.y * scale > 1.05) {
        var blurPixs2 = array<vec2f, 4>(vec2f(1.5, 0.5), vec2f(0.5, -1.5), vec2f(-0.5, 1.5), vec2f(-1.5, -0.5));
        ColorNew = bright(Tex * scale);
        for (var i = 0; i < 4; i++) {
            ColorNew += bright(Tex * scale + blurPixs2[i] * bsize * rcpres());
        }
    }
    return vec4f(ColorNew / 5.0, 1.0);
}

fn bloom(Tex: vec2f, axis: vec2f) -> vec4f {
    var ColorNew = vec3f(0.0);
    if !(Tex.x * scale > 1.05 || Tex.y * scale > 1.05) {
        var blurPixs = array<f32, 7>(-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0);
        var blurMags = array<f32, 7>(0.05, 0.14, 0.23, 0.32, 0.23, 0.14, 0.05);
        for (var i = 0; i < 7; i++) {
            ColorNew += textureSampleLevel(lastpass, linear_clamp, Tex + axis * blurPixs[i] * rcpres() * bsize2, 0.0).rgb * blurMags[i];
        }
    }
    return vec4f(ColorNew, 1.0);
}

@fragment fn MyBloomH(in: Quad) -> @location(0) vec4f {
    return bloom(in.tex, vec2f(1.0, 0.0));
}

@fragment fn MyBloomV(in: Quad) -> @location(0) vec4f {
    return bloom(in.tex, vec2f(0.0, 1.0));
}

@fragment fn gamma(in: Quad) -> @location(0) vec4f {
    let W = 11.2;
    var texColor = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    let bloomc = textureSampleLevel(lastpass, linear_clamp, in.tex / scale, 0.0).rgb;
    texColor = screen(pow(texColor, vec3f(2.2)), bloomc * BloomBoost);
    var multip = 0.0;
    if u.sunamb.w == 0.0 {
        multip = 1.0 - 0.5 * saturate(1.0 - u.sunpos.z * 1.4);
    }
    multip = saturate(multip + u.hdr.x - 0.2);
    texColor *= mix(interiorbloom(), exteriorbloom(), multip); // Hardcoded Exposure Adjustment
    let ExposureBias = 2.0;
    let curr = Uncharted2Tonemap(ExposureBias * texColor);
    let whiteScale = 1.0 / Uncharted2Tonemap(vec3f(W));
    return vec4f(pow(max(curr * whiteScale, vec3f(0.0)), vec3f(1.0 / 2.2)), 1.0);
}

@fragment fn barrelvig(in: Quad) -> @location(0) vec4f {
    let TexD = in.tex;
    let ratio = rcpres().x / rcpres().y;
    let scaleFactor = 1.0 / dscale;
    // index of refraction of each color channel, causing chromatic dispersion
    let eta = vec3f(1.0 + chromaticDispersionRC * 0.09, 1.0 + chromaticDispersionRC * 0.06, 1.0 + chromaticDispersionRC * 0.03);
    let eta2 = vec3f(1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.06, 1.0 + chromaticDispersionBY * 0.03);
    let tex = 2.0 * (TexD - 0.5);
    var uv = TexD;
    uv *= 1.0 - uv.yx;
    let r2 = ratio * ratio * tex.x * tex.x + tex.y * tex.y;
    // compute the cubic distortion
    let f = 1.0 + r2 * (distortion + cubicDistortion * sqrt(r2));
    // Apply CA
    let rCoords = (f * eta.r * eta2.r) * scaleFactor * (tex * 0.5) + 0.5;
    let gCoords = (f * eta.g * eta2.g) * scaleFactor * (tex * 0.5) + 0.5;
    let bCoords = (f * eta.b * eta2.b) * scaleFactor * (tex * 0.5) + 0.5;
    var scene = vec3f(
        textureSampleLevel(lastpass, linear_clamp, rCoords, 0.0).r,
        textureSampleLevel(lastpass, linear_clamp, gCoords, 0.0).g,
        textureSampleLevel(lastpass, linear_clamp, bCoords, 0.0).b);
    scene *= 0.2 + 0.8 * pow(16.0 * uv.x * uv.y * (1.0 - uv.x) * (1.0 - uv.y), 0.1);
    return vec4f(scene, 1.0);
}
";

// Cinematic Bokeh DoF (vtastek, after Martins Upitis): a blur by the depth with rings of
// samples, bright samples made brighter, and a second pass that blurs where the first pass
// says so in its alpha. The parts of the file that are off there (`MANUALDOF`, `PENTAGON`,
// `SHOWFOCUS`, the vignette) are not here. `sungain` of the file is a number from a vector:
// HLSL takes the red. With no sun colour the file divides 0 by 0, and the port takes 0 for
// the quotient.
const CINEMATIC_BOKEH: &str = "
const PI: f32 = 3.14159265;
const unit2m: f32 = 0.0142; // file
const samples: i32 = 3; // file
const rings: i32 = 3; // file
const maxblur: f32 = 2.5; // file
const threshold: f32 = 0.7; // file
const gain: f32 = 2.0; // file
const sbright: f32 = 0.5; // file
const bias: f32 = 0.5; // file
const fringe: f32 = 0.6; // file
const namount: f32 = 0.0001; // file
const dbsize: f32 = 1.25; // file
const focus: vec2f = vec2f(0.5, 0.5);

// smoothstep of HLSL: its edges can be in any order.
fn sstep(a: f32, b: f32, x: f32) -> f32 {
    let t = saturate((x - a) / (b - a));
    return t * t * (3.0 - 2.0 * t);
}

fn sungain() -> f32 {
    let top = max(u.suncol.r, max(u.suncol.g, u.suncol.b));
    var part = 0.0;
    if top > 0.0 {
        part = saturate(u.suncol.r / top);
    }
    return (1.03 - part) * 3.0;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}

// blurring depth
fn tapblur(coords: vec2f, dp: f32) -> vec4f {
    var d = vec4f(0.0);
    let wh = rcpres() * dbsize;
    let df = 0.3 * (1.0 + dp * 6.0);
    let au = vec4f(1.0, 1.0, 1.0, 3.0);
    for (var i = 0; i < 9; i++) {
        let offset = vec2f(f32(i % 3 - 1), f32(i / 3 - 1)) * wh;
        let weight = f32((2 - abs(i % 3 - 1)) * (2 - abs(i / 3 - 1))) / 16.0;
        let tmp = pow(clamp(textureSampleLevel(lastpass, point_clamp, coords + offset * df, 0.0), vec4f(0.0), vec4f(1.0)) * au, vec4f(2.2));
        d = max(d, d + tmp * weight);
    }
    return pow(d, vec4f(1.0 / 2.2));
}

// generating noise/pattern texture for dithering
fn rand(coord: vec2f) -> vec2f {
    let noiseX = fract(sin(dot(coord, vec2f(12.9898, 78.233))) * 43758.5453) * 2.0 - 1.0;
    let noiseY = fract(sin(dot(coord, vec2f(12.9898, 78.233) * 2.0)) * 43758.5453) * 2.0 - 1.0;
    return vec2f(noiseX, noiseY);
}

// processing the sample
fn color(coords: vec2f, blur: f32, desky: f32) -> vec3f {
    let texel = rcpres();
    let col = vec3f(
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(0.0, 1.0) * texel * fringe * blur, 0.0).r,
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(-0.866, -0.5) * texel * fringe * blur, 0.0).g,
        textureSampleLevel(lastshader, point_clamp, coords + vec2f(0.866, -0.5) * texel * fringe * blur, 0.0).b);
    let lum = dot(col, vec3f(0.299, 0.587, 0.114));
    let thresh = max((lum - threshold) * gain * sungain() * desky, 0.0);
    return col + mix(vec3f(0.0), col, thresh * blur);
}

@fragment fn dof(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    let interior = f32(u.sunamb.w != 0.0);
    // scene depth calculation, with the focus at the middle of the picture
    let fDepth = depth_at(focus) * unit2m;
    let depth = depth_at(tex) * unit2m;
    let skdepth = saturate(sstep(9999.0, 1000.0, depth) + sbright);

    // dof blur factor calculation
    let depthme = depth * 0.01;
    let fclose = 1.0 - exp(-depthme);
    let ffar = exp(-depthme * 100.0);
    var blur = mix(fclose * 1.5, ffar * 9.7, saturate(fDepth * 0.05));
    blur = mix(fclose, blur * (1.0 - interior * 0.9), saturate(fDepth * 0.05));
    var blur2 = mix(0.0, ffar * 9.7, saturate(fDepth * 0.05));
    blur2 = mix(0.0, blur2 * (1.0 - interior * 0.9), saturate(fDepth * 0.05));

    blur = pow(blur, 1.0 + interior) * (1.0 + interior * 32.0) - 0.1;
    blur = min(blur * 1.75, 0.55);
    blur += sstep(200.0, 250.0, depth) * 0.4;
    blur = saturate(blur);

    blur2 = pow(blur2, 1.0 + interior) * (1.0 + interior * 32.0) - 0.1;
    blur2 = min(blur2 * 1.75, 0.55);
    blur2 = saturate(blur2);

    // calculation of pattern for ditering
    let noise = rand(tex) * namount * blur;
    // getting blur x and y step factor
    let w = rcpres().x * blur * maxblur + noise.x;
    let h = rcpres().y * blur * maxblur + noise.y;

    // calculation of final color
    var col = pow(textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb, vec3f(2.2));
    var s = 1.0;
    for (var i = 1; i <= rings; i++) {
        let ringsamples = i * samples;
        let edge = mix(1.0, f32(i) / f32(rings), bias);
        for (var j = 0; j < ringsamples; j++) {
            let turn = PI * 2.0 / f32(ringsamples);
            let pw = cos(f32(j) * turn) * f32(i);
            let ph = sin(f32(j) * turn) * f32(i);
            col += pow(color(tex + vec2f(pw * w, ph * h), blur, skdepth) * edge, vec3f(2.2));
            s = max(s, s + edge);
        }
    }
    col /= s * 0.90; // divide by sample count
    col = pow(col, vec3f(1.0 / 2.2));
    return vec4f(col, blur * blur2);
}

@fragment fn dof2(in: Quad) -> @location(0) vec4f {
    let c = textureSampleLevel(lastpass, point_clamp, in.tex, 0.0);
    let dp = 1.0 - sstep(0.0, 100.0, depth_at(in.tex)) * c.a;
    let blur = tapblur(in.tex, dp);
    return vec4f(mix(c.rgb, blur.rgb, blur.a), c.a);
}
";

/// Each port: those of this file, and those of the Joy of Painting.
fn ports() -> impl Iterator<Item = &'static Shader> {
    SHADERS.iter().chain(super::post_jop::SHADERS)
}

/// The shaders that this program has a port of.
pub const SHADERS: &[Shader] = &[
    Shader { name: "Sunshafts", source: SUNSHAFTS, passes: &[full("stretch"), full("blur_r_half"), full("rays"), full("blur_t"), full("combine")], disable_flags: 9, no_sun_glare: true, priority: 2_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Bloom Fine", source: BLOOM_FINE, passes: &[full("energy_levels"), full("blur_vert"), full("blur_horz_combine")], disable_flags: 0, no_sun_glare: false, priority: 4_000_000, textures: &[], requires_hdr: false, variables: &[] },
    // `mgeflags` 22: not in a true exterior, not in an interior that behaves as an exterior,
    // not above the water. 17: not in an interior, not above the water.
    Shader { name: "Underwater Interior Effects", source: UNDERWATER_INTERIOR, passes: &[full("wobble")], disable_flags: 22, no_sun_glare: false, priority: 1_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Underwater Effects", source: UNDERWATER, passes: &[full("rays")], disable_flags: 17, no_sun_glare: false, priority: 1_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Eye Adaptation (HDR)", source: EYE_ADAPTATION, passes: &[full("tone")], disable_flags: 0, no_sun_glare: false, priority: 5_000_000, textures: &[], requires_hdr: true, variables: &[] },
    Shader { name: "SSAO Fast", source: SSAO_FAST, passes: &[full("ssao"), full("smartblur"), full("smartblur"), full("combine")], disable_flags: 8, no_sun_glare: false, priority: 1_000_000 - 10_000, textures: &["MGE/poisson_nrm.dds"], requires_hdr: false, variables: &[] },
    Shader { name: "SSAO HQ", source: SSAO_HQ, passes: &[full("ssao"), full("smartblur"), full("smartblur"), full("combine")], disable_flags: 8, no_sun_glare: false, priority: 1_000_000 - 10_000, textures: &["MGE/poisson_nrm.dds"], requires_hdr: false, variables: &[] },
    Shader { name: "Depth of Field", source: DEPTH_OF_FIELD, passes: &[full("dof"), full("smartblur")], disable_flags: 0, no_sun_glare: false, priority: 3_000_000, textures: &["MGE/noise64.dds"], requires_hdr: false, variables: &[] },
    Shader {
        name: "Bloom Soft",
        source: BLOOM_SOFT,
        passes: &[full("fix_sky"), full("blur_hor_i"), full("blur_vert_i"), full("bright_pass"), full("blur_hor_2"), full("blur_vert_2"), full("recombine_a"), full("blur_hor_3"), full("blur_vert_3"), full("bloom_b")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 4_000_000,
        textures: &[],
        requires_hdr: false, variables: &[],
    },
    // Bloom Soft for a file with the sky and the hands in the bloom: the file has no pass
    // for the depth then.
    Shader {
        name: "Bloom Soft without the depth pass",
        source: BLOOM_SOFT,
        passes: &[full("blur_hor_i"), full("blur_vert_i"), full("bright_pass"), full("blur_hor_2"), full("blur_vert_2"), full("recombine_a"), full("blur_hor_3"), full("blur_vert_3"), full("bloom_b")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 4_000_000,
        textures: &[],
        requires_hdr: false,
        variables: &[],
    },
    Shader {
        name: "Bloom S_A",
        source: BLOOM_S_A,
        passes: &[scaled("prefilter", 2), scaled("down", 4), scaled("down", 8), scaled("down", 16), scaled("down", 32), scaled("down", 64), scaled("up4", 32), scaled("up3", 16), scaled("up2", 8), scaled("up1", 4), scaled("up0", 2), full("combine")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 4_000_000,
        textures: &[],
        requires_hdr: false, variables: &[],
    },
    Shader { name: "SSAO S_A", source: SSAO_S_A, passes: &[scaled("ao", 2), scaled("blur_x", 2), scaled("blur_y", 2), full("combine")], disable_flags: 8, no_sun_glare: false, priority: 1_000_000 - 10_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Depth of Field S_A", source: DOF_S_A, passes: &[scaled("coc", 2), scaled("gather", 2), scaled("postfilter", 2), full("combine")], disable_flags: 0, no_sun_glare: false, priority: 3_000_000, textures: &[], requires_hdr: false, variables: &[] },
    // The technique of `MGG SSAO.fx` has no category. MGE XE gives such a shader the
    // priority of the nearest shader of the chain with a category; the chain of the
    // Morrowind Graphics Guide puts it first, before the underwater shaders (scene).
    Shader { name: "MGG SSAO", source: MGG_SSAO, passes: &[full("ssao"), full("smartblur"), full("combine")], disable_flags: 8, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    // The technique of the file has no category: the place of `none`.
    Shader {
        name: "heathaze",
        source: HEATHAZE,
        passes: &[full("warpfx")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 6_000_000,
        textures: &["heathaze_nm.dds"],
        requires_hdr: false,
        variables: &[
            Variable { name: "warpint", slots: 1, per: 1, default: &[0.003] },
            Variable { name: "dmaskint1", slots: 1, per: 1, default: &[5005.0] },
            Variable { name: "dmaskint2", slots: 1, per: 1, default: &[9000.0] },
        ],
    },
    // The technique of the file has no category: the place of `none`.
    Shader { name: "Special Process", source: SPECIAL_PROCESS, passes: &[full("brightpass"), full("blurx"), full("blury"), full("specialcontrast")], disable_flags: 0, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: true, variables: &[] },
    // The two files have no category: the place of `none`.
    Shader { name: "ColorMood", source: COLOR_MOOD, passes: &[full("colorMood")], disable_flags: 1, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "MGG darker interiors", source: MGG_DARKER_INTERIORS, passes: &[full("gammatoo")], disable_flags: 6, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    // The fog boxes that a mod sets from Lua: the centres, the half sizes and the colours of
    // the boxes (3 floats each), and their densities.
    Shader {
        name: "oj_me_fog_box",
        source: OJ_ME_FOG_BOX,
        passes: &[full("draw")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 1_000_000,
        textures: &[],
        requires_hdr: false,
        variables: &[
            Variable { name: "fogCenters", slots: 2, per: 3, default: &[] },
            Variable { name: "fogRadi", slots: 2, per: 3, default: &[] },
            Variable { name: "fogColors", slots: 2, per: 3, default: &[] },
            Variable { name: "fogDensities", slots: 2, per: 1, default: &[] },
        ],
    },
    Shader {
        name: "tew_fogbox",
        source: TEW_FOGBOX,
        passes: &[full("draw")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 2_000_000,
        textures: &[],
        requires_hdr: false,
        variables: &[
            Variable { name: "fogCenters", slots: 3, per: 3, default: &[] },
            Variable { name: "fogRadi", slots: 3, per: 3, default: &[] },
            Variable { name: "fogColors", slots: 3, per: 3, default: &[] },
            Variable { name: "fogDensities", slots: 3, per: 1, default: &[] },
        ],
    },
    Shader {
        name: "skoomaesthesia",
        source: SKOOMAESTHESIA,
        passes: &[full("lut"), full("blurx"), full("blury")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 6_000_000,
        textures: &["x0_psychedelic.tga"],
        requires_hdr: false,
        variables: &[
            Variable { name: "intensity", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "cycle", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "radius", slots: 1, per: 1, default: &[0.0] },
        ],
    },
    Shader {
        name: "Disorient",
        source: DISORIENT,
        passes: &[full("shakervib")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 6_000_000,
        textures: &[],
        requires_hdr: false,
        variables: &[
            Variable { name: "srange", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "sspeed", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "zoom", slots: 1, per: 1, default: &[1.0] },
            Variable { name: "offset", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "intensity", slots: 1, per: 1, default: &[0.0] },
            Variable { name: "vibright", slots: 1, per: 1, default: &[0.5] },
            Variable { name: "seed", slots: 1, per: 1, default: &[5.0] },
        ],
    },
    Shader {
        name: "Invisibility",
        source: INVISIBILITY,
        passes: &[full("brightpass"), full("warpfx")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 6_000_000,
        textures: &["warpfxnorm.dds"],
        requires_hdr: false,
        variables: &[Variable { name: "invisint", slots: 1, per: 1, default: &[1.0] }, Variable { name: "warpint", slots: 1, per: 1, default: &[1.0] }],
    },
    Shader {
        name: "InspectIt/Bokeh",
        source: BOKEH,
        passes: &[full("dof"), full("smartblur")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 3_000_000,
        textures: &["MGE/noise64.dds"],
        requires_hdr: false,
        variables: &[Variable { name: "focus_distance", slots: 1, per: 1, default: &[0.0] }, Variable { name: "focal_length", slots: 1, per: 1, default: &[1.0] }],
    },
    Shader { name: "ashfall", source: ASHFALL, passes: &[full("main")], disable_flags: 0, no_sun_glare: false, priority: 9_000_000 + 40, textures: &[], requires_hdr: false, variables: &[Variable { name: "temperature", slots: 1, per: 1, default: &[20.0] }] },
    Shader { name: "harness_fill_opaque", source: HARNESS_FILL_OPAQUE, passes: &[full("fill")], disable_flags: 0, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "harness_fill_scene", source: HARNESS_FILL_SCENE, passes: &[full("fill")], disable_flags: 0, no_sun_glare: false, priority: 1_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Lens Effects", source: LENS_EFFECTS, passes: &[full("barrelvig")], disable_flags: 0, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "film", source: FILM, passes: &[full("FilmPass")], disable_flags: 65, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "deband_fogaware", source: DEBAND_FOGAWARE, passes: &[full("PS_Deband")], disable_flags: 0, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader {
        name: "gammavtdt",
        source: GAMMAVTDT,
        passes: &[full("ThreshDownScale"), full("MyBloomH"), full("MyBloomV"), full("MyBloomH"), full("MyBloomV"), full("gamma"), full("barrelvig")],
        disable_flags: 0,
        no_sun_glare: false,
        priority: 6_000_000,
        textures: &[],
        requires_hdr: true,
        variables: &[Variable { name: "exteriorbloom", slots: 1, per: 1, default: &[2.0] }, Variable { name: "interiorbloom", slots: 1, per: 1, default: &[3.0] }],
    },
    Shader { name: "Cinematic Bokeh DoF", source: CINEMATIC_BOKEH, passes: &[full("dof"), full("dof2")], disable_flags: 0, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: true, variables: &[] },
    // Files of mods with the name of a stock shader and other code: see `MOD_FILES`.
    Shader { name: "Sunshafts of Watch the Skies", source: SUNSHAFTS_WTS, passes: &[full("stretch"), full("blur_r_half"), full("rays"), full("blur_t"), full("combine")], disable_flags: 9, no_sun_glare: true, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Sunshafts of STEP", source: SUNSHAFTS_STEP, passes: &[full("stretch"), full("blur_r_half"), full("rays"), full("blur_t"), full("combine")], disable_flags: 9, no_sun_glare: true, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "Underwater Effects of Krokantor", source: UNDERWATER_KROKANTOR, passes: &[full("rays"), full("chroma")], disable_flags: 17, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: false, variables: &[] },
    Shader { name: "SSAO HQ of STEP", source: SSAO_HQ_STEP, passes: &[full("ssao"), full("smartblur"), full("smartblur"), full("combine")], disable_flags: 8, no_sun_glare: false, priority: 6_000_000, textures: &["noise8q.tga"], requires_hdr: false, variables: &[] },
    Shader { name: "Eye Adaptation (HDR) of STEP", source: EYE_ADAPTATION_STEP, passes: &[full("tone")], disable_flags: 5, no_sun_glare: false, priority: 6_000_000, textures: &[], requires_hdr: true, variables: &[] },
];

/// The linear depth of the frame, as the depth frame of the Direct3D 9 path has it
/// (`DistantLand::renderDepth`): the depth of the game where the game drew, else the depth
/// of the distant land and the distant statics, else `1e38`. `p` has the two depth numbers
/// of the projection of the game, then those of the projection of the distant stage.
const DEPTH_SHADER: &str = "
@group(0) @binding(0) var<uniform> p: vec4f;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var distant: texture_depth_2d;

struct Quad {
    @builtin(position) position: vec4f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    out.position = vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
    return out;
}

@fragment fn fs(in: Quad) -> @location(0) vec4f {
    let d = textureLoad(depth, vec2i(in.position.xy), 0);
    if d < 1.0 {
        return vec4f(p.y / (d - p.x), 0.0, 0.0, 1.0);
    }
    let far = textureLoad(distant, vec2i(in.position.xy), 0);
    if far < 1.0 {
        return vec4f(p.w / (far - p.z), 0.0, 0.0, 1.0);
    }
    return vec4f(1.0e38, 0.0, 0.0, 1.0);
}
";

/// The copy of the result into the picture of the frame.
pub(super) const BLIT_SHADER: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

struct Quad {
    @builtin(position) position: vec4f,
    @location(0) tex: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.tex = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}

@fragment fn fs(in: Quad) -> @location(0) vec4f {
    return textureSampleLevel(source, source_sampler, in.tex, 0.0);
}
";

/// A look at a term, for a test: MGE_XE_POST_DEBUG 1 tints the result, 2 shows the linear
/// depth over 10000 units as grey.
const DEBUG_BLIT_SHADER: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> mode: vec4f;

struct Quad {
    @builtin(position) position: vec4f,
    @location(0) tex: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.tex = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}

@fragment fn fs(in: Quad) -> @location(0) vec4f {
    let c = textureSampleLevel(source, source_sampler, in.tex, 0.0);
    if mode.x == 2.0 {
        return vec4f(vec3f(clamp(c.r / 10000.0, 0.0, 1.0)), 1.0);
    }
    return vec4f(c.r, 0.5 * c.g, 0.5 * c.b, 1.0);
}
";

/// A pass as it runs: its pipeline, the scale and the format of its target, and the uniform
/// with the size of the target.
struct CompiledPass {
    pipeline: wgpu::RenderPipeline,
    scale: u32,
    half_float: bool,
    info: wgpu::Buffer,
}

struct Compiled {
    name: String,
    disable_flags: u32,
    /// The place in the chain, from `own_priority` by the order rule.
    priority: i64,
    /// The priority of the shader itself: its category, or what a mod set.
    own_priority: i64,
    /// The number of the shader in the order that the shaders came in.
    sequence: usize,
    /// A mod can turn a shader off.
    enabled: bool,
    no_sun_glare: bool,
    /// The code of the shader reads the depth frame (`depthframe`; the common part only
    /// declares it).
    reads_depth: bool,
    passes: Vec<CompiledPass>,
    variables: Vec<VariableDef>,
    /// The uniform `modv` of the shader, when it has variables.
    values: Option<wgpu::Buffer>,
    /// Group 1 of the shader: `modv` and its textures.
    group: wgpu::BindGroup,
}

/// A texture of the pool: the scale and the format of its class, and its view.
struct Pooled {
    scale: u32,
    half_float: bool,
    view: wgpu::TextureView,
}

pub struct Post {
    /// The shaders of the chain that have a port, and the shaders that a mod loaded, in
    /// their order.
    chain: Vec<Compiled>,
    /// The names that have no shader here, or one that does not compile: said one time.
    failed: Vec<String>,
    /// The shaders whose unknown variables were told, one time each.
    told_unknown: Vec<String>,
    /// The result of the last load of each shader, for the file that the game reads.
    pub status: ShaderStatus,
    /// The layouts of the groups of the shaders: the frame, the shader, the pass.
    frame_layout: wgpu::BindGroupLayout,
    shader_layout: wgpu::BindGroupLayout,
    pass_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    /// The uniform `modv` of a shader without variables.
    no_values: wgpu::Buffer,
    hdr_reaction: f32,
    /// The shaders that came from a port, and from a file of a mod.
    counts: (u32, u32),
    uniforms: wgpu::Buffer,
    samplers: [wgpu::Sampler; 6],
    white: wgpu::TextureView,
    /// A black pixel: what a shader reads from a texture that was not found, as from an
    /// unset texture of Direct3D 9.
    black: wgpu::TextureView,
    /// The targets of the passes, by their class, for the size of the picture.
    pool: Vec<Pooled>,
    size: (u32, u32),
    /// The linear depth of the game.
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    depth_pipeline: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    depth_uniform: wgpu::Buffer,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    debug: f32,
    debug_pipeline: wgpu::RenderPipeline,
    debug_layout: wgpu::BindGroupLayout,
    debug_uniform: wgpu::Buffer,
    waves: wgpu::TextureView,
    /// The sun glare of the game is off: a shader of the chain at the start has
    /// `disableSunglare`.
    pub no_sun_glare: bool,
    /// The number of reloads of each shader that a mod reloaded, as the last state had it.
    reloads: std::collections::HashMap<String, u32>,
    /// The HDR measure, when a shader of the chain needs it. `hdr_on` is the flag of MGE
    /// XE (`mge.render.updateHDR`): off, the measure stops and the shaders keep its last
    /// point, as `evalAdaptHDR` is skipped on Direct3D 9.
    hdr: Option<Hdr>,
    hdr_on: bool,
    pub frames: u64,
    pub shaders_run: u64,
    /// The frames that made the depth frame (a shader of the chain read it).
    pub depth_frames: u64,
}

impl Post {
    /// Makes the chain from the names of the settings. A name is a WGSL file of a mod
    /// (`shaders/XEshaders/<name>.wgsl`, see [`Definition::of_file`]) or, without such a file,
    /// a port of this program; a name with neither, and a file that does not compile, is
    /// reported and skipped. `waves` is the 3D wave texture of the water; `read` gives the
    /// bytes of a file under `Data Files`; `hdr_reaction` is `render.hdr_reaction_time`.
    pub fn new(scene: &SceneView, chain: &[String], waves: wgpu::TextureView, read: &dyn Fn(&str) -> Option<Vec<u8>>, hdr_reaction: f32) -> Post {
        let device = scene.device;
        // The groups of the shaders (`COMMON`): 0 the frame, 1 the shader, 2 the pass.
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let colour = |binding| texture(binding, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2);
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post frame"),
            entries: &[
                uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture(1, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3),
                sampler(3, wgpu::SamplerBindingType::NonFiltering),
                sampler(4, wgpu::SamplerBindingType::Filtering),
                sampler(5, wgpu::SamplerBindingType::Filtering),
                sampler(6, wgpu::SamplerBindingType::Filtering),
                sampler(7, wgpu::SamplerBindingType::NonFiltering),
                sampler(8, wgpu::SamplerBindingType::NonFiltering),
            ],
        });
        let mut entries = vec![uniform(0, wgpu::ShaderStages::FRAGMENT)];
        entries.extend((1..=10).map(colour));
        let shader_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post shader"), entries: &entries });
        let mut entries = vec![uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)];
        entries.extend((1..=10).map(colour));
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post pass"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post shaders"),
            bind_group_layouts: &[Some(&frame_layout), Some(&shader_layout), Some(&pass_layout)],
            immediate_size: 0,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: size_of::<Variables>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let make_sampler = |filter: wgpu::FilterMode, address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: address, address_mode_v: address, address_mode_w: address, mag_filter: filter, min_filter: filter, ..Default::default() })
        };
        let samplers = [
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::Repeat),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::MirrorRepeat),
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::Repeat),
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::MirrorRepeat),
        ];
        // A white pixel, for a shader without a texture.
        use wgpu::util::DeviceExt;
        let white = device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("post white"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[255, 255, 255, 255],
            )
            .create_view(&Default::default());
        let black = device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("post black"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[0, 0, 0, 255],
            )
            .create_view(&Default::default());
        // The linear depth.
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post depth"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
            ],
        });
        let depth_pipeline = full_screen_pipeline(device, "post depth", DEPTH_SHADER, &depth_layout, wgpu::TextureFormat::R32Float);
        let depth_uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post depth"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post blit"),
            entries: &[texture(0, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2), sampler(1, wgpu::SamplerBindingType::Filtering)],
        });
        let blit_pipeline = full_screen_pipeline(device, "post blit", BLIT_SHADER, &blit_layout, scene.target_format);
        let debug = std::env::var("MGE_XE_POST_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let debug_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post debug"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2),
                sampler(1, wgpu::SamplerBindingType::NonFiltering),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let debug_pipeline = full_screen_pipeline(device, "post debug", DEBUG_BLIT_SHADER, &debug_layout, scene.target_format);
        let debug_uniform = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("post debug"), contents: bytemuck::cast_slice(&[debug, 0.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM })
        };
        let no_values = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post no values"), size: (MOD_SLOTS * 16) as u64, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: false });
        let mut post = Post {
            chain: Vec::new(),
            failed: Vec::new(),
            told_unknown: Vec::new(),
            status: ShaderStatus::default(),
            frame_layout,
            shader_layout,
            pass_layout,
            pipeline_layout,
            no_values,
            hdr_reaction,
            counts: (0, 0),
            reloads: std::collections::HashMap::new(),
            uniforms,
            samplers,
            white,
            black,
            pool: Vec::new(),
            size: (0, 0),
            depth: None,
            depth_pipeline,
            depth_layout,
            depth_uniform,
            blit_pipeline,
            blit_layout,
            debug,
            debug_pipeline,
            debug_layout,
            debug_uniform,
            waves,
            no_sun_glare: false,
            hdr: None,
            hdr_on: true,
            frames: 0,
            shaders_run: 0,
            depth_frames: 0,
        };
        for name in chain {
            post.load(scene, name, read);
        }
        post.order();
        // The sun glare of the game is off when a shader of the chain has
        // `disableSunglare`: MGE XE on Direct3D 9 decides it one time, after it loads the
        // chain (`MWPatches::disableSunglare`). A shader that a mod loads or turns off
        // later does not change it.
        post.no_sun_glare = post.chain.iter().any(|c| c.no_sun_glare);
        println!("post: {} of {} shaders of the chain run: {} ports, {} files of mods", post.chain.len(), chain.len(), post.counts.0, post.counts.1);
        post
    }

    /// Compiles the shader `name` and puts it at the end of the chain: a WGSL file of a mod,
    /// or a port. Returns false, and tells the log one time, for a name with neither and
    /// for a shader that does not compile. The caller calls `order` after it.
    fn load(&mut self, scene: &SceneView, name: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> bool {
        if self.chain.iter().any(|c| c.name == name) {
            return true;
        }
        if self.failed.iter().any(|failed| failed == name) {
            return false;
        }
        let device = scene.device;
        let file = read(&format!("shaders/XEshaders/{name}.wgsl"));
        let shader = match &file {
            Some(bytes) => match Definition::of_file(name, &String::from_utf8_lossy(bytes)) {
                Ok(definition) => definition,
                Err(problem) => {
                    println!("post: the shader file {name}.wgsl is not used: {problem}");
                    self.status.note(name, Some(format!("The first lines of {name}.wgsl (the lines with //!) are not right: {problem}")));
                    self.failed.push(name.to_string());
                    return false;
                }
            },
            None => {
                // The effect file that the name resolves to, from a mod or from MGE XE. A
                // file of a mod with the name of another shader has a port of its own.
                let effect = read(&format!("shaders/XEshaders/{name}.fx")).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                let port_name = effect.as_deref().map(code_key).and_then(|key| MOD_FILES.iter().find(|(mod_name, mod_key, _)| *mod_name == name && *mod_key == key)).map_or(name, |(_, _, port)| *port);
                match ports().find(|shader| shader.name == port_name) {
                    Some(port) => {
                        let mut definition = Definition::of_port(port);
                        definition.name = name.to_string();
                        // The port takes the numbers of the file, and the log says when
                        // the file is not the one the port was made from.
                        if let Some(effect) = effect.as_deref() {
                            port_for_file(&mut definition, effect);
                        }
                        definition
                    }
                    None => {
                        // The key tells which file this is, for a report.
                        match effect.as_deref().map(code_key) {
                            Some(key) => println!("post: no file {name}.wgsl and no port of the shader {name} (the code key of {name}.fx is {key:#018x}); skipped"),
                            None => println!("post: no file {name}.wgsl, no file {name}.fx and no port of the shader {name}; skipped"),
                        }
                        self.status.note(name, Some(format!("The wgpu renderer has no file {name}.wgsl and no port of the shader {name}.")));
                        self.failed.push(name.to_string());
                        return false;
                    }
                }
            }
        };
        let functions = variables_wgsl(&shader.variables);
        let source = format!("{COMMON}\n{functions}\n{}", shader.source);
        // An error of the WGSL of a file, or a pass that is not a function of it, comes
        // back from the scope and does not stop the program.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&shader.name), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let passes: Vec<CompiledPass> = shader
            .passes
            .iter()
            .map(|(entry, scale, half_float)| {
                let format = if *half_float { wgpu::TextureFormat::Rgba16Float } else { wgpu::TextureFormat::Rgba8Unorm };
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&shader.name),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                    primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(entry.as_str()),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                let info = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post pass"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                CompiledPass { pipeline, scale: *scale, half_float: *half_float, info }
            })
            .collect();
        if let Some(error) = pollster::block_on(scope.pop()) {
            // The lines of the message count the common part and the functions of the
            // variables first.
            let before = COMMON.lines().count() + functions.lines().count() + 2;
            println!("post: the shader {} does not compile; skipped (its first line is the line {before} of the message):\n{error}", shader.name);
            // For the author: the line numbers of the file. A port has no file of its own.
            let error = error.to_string();
            let message = if file.is_some() { own_lines(compiler_part(&error), before - 1) } else { compiler_part(&error).to_string() };
            self.status.note(name, Some(message));
            self.failed.push(name.to_string());
            return false;
        }
        self.status.note(name, None);
        if file.is_some() {
            self.counts.1 += 1;
        } else {
            self.counts.0 += 1;
        }
        if shader.requires_hdr && self.hdr.is_none() {
            self.hdr = Some(Hdr::new(device, &self.blit_layout, self.hdr_reaction));
        }
        // The textures of the shader. One that is not found reads as black, as on Direct3D 9.
        let textures: Vec<wgpu::TextureView> = shader
            .textures
            .iter()
            .map(|name| {
                let path = format!("textures/{name}");
                match read(&path).ok_or_else(|| "no file".to_string()).and_then(|bytes| super::image_texture(scene, name, &bytes)) {
                    Ok(view) => view,
                    Err(problem) => {
                        println!("post: the texture {name} of the shader {} was not read ({problem}); black", shader.name);
                        self.black.clone()
                    }
                }
            })
            .collect();
        // The variables, with their values before a mod sets them.
        let values = (!shader.variables.is_empty()).then(|| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("post values"), contents: bytemuck::cast_slice(&values_of(&shader.variables, &[])), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST })
        });
        // Group 1 of the shader; it keeps the textures.
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: values.as_ref().unwrap_or(&self.no_values).as_entire_binding() }];
        entries.extend((0..10).map(|i| wgpu::BindGroupEntry { binding: i as u32 + 1, resource: wgpu::BindingResource::TextureView(textures.get(i).unwrap_or(&self.white)) }));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(&shader.name), layout: &self.shader_layout, entries: &entries });
        self.chain.push(Compiled {
            name: shader.name,
            disable_flags: shader.disable_flags,
            priority: shader.priority,
            own_priority: shader.priority,
            sequence: self.chain.len() + self.failed.len(),
            enabled: true,
            no_sun_glare: shader.no_sun_glare,
            reads_depth: shader.source.contains("depthframe"),
            passes,
            variables: shader.variables,
            values,
            group,
        });
        true
    }

    /// Puts the shaders in the order of MGE XE (`PostShaders::orderShaders`): by the
    /// category, stable in the order that they came in. A shader without a category stays
    /// where that order has it: it takes the place of the shader with a category before
    /// it, or of the first such shader.
    fn order(&mut self) {
        const NO_CATEGORY: i64 = 6_000_000;
        self.chain.sort_by_key(|c| c.sequence);
        let mut last = self.chain.iter().map(|c| c.own_priority).find(|priority| *priority != NO_CATEGORY).unwrap_or(NO_CATEGORY);
        for shader in self.chain.iter_mut() {
            if shader.own_priority == NO_CATEGORY {
                shader.priority = last;
            } else {
                shader.priority = shader.own_priority;
                last = shader.priority;
            }
        }
        self.chain.sort_by_key(|c| c.priority);
        println!("post: the order: {}", self.chain.iter().map(|c| if c.enabled { c.name.clone() } else { format!("{} (off)", c.name) }).collect::<Vec<_>>().join(", "));
    }

    /// Takes the state of a shader that a mod loaded: the shader comes into the chain when
    /// it is not there, and gets its on or off state, its priority and its variables.
    pub fn set_state(&mut self, scene: &SceneView, state: &ShaderState, read: &dyn Fn(&str) -> Option<Vec<u8>>) {
        // A mod reloaded the shader: its WGSL file or its port, the numbers of its effect
        // file and its textures are read again, as a reload on Direct3D 9 makes the effect
        // again. A shader that did not compile gets a new try. The shader keeps its place
        // among the shaders that came in.
        let mut place = None;
        if self.reloads.get(&state.name).copied().unwrap_or(0) != state.reloads {
            self.reloads.insert(state.name.clone(), state.reloads);
            self.failed.retain(|failed| *failed != state.name);
            if let Some(at) = self.chain.iter().position(|c| c.name == state.name) {
                place = Some(self.chain.remove(at).sequence);
            }
            println!("post: {}: reloaded by a mod", state.name);
        }
        let known = self.chain.iter().any(|c| c.name == state.name);
        if !known && !self.load(scene, &state.name, read) {
            return;
        }
        let Some(shader) = self.chain.iter_mut().find(|c| c.name == state.name) else { return };
        if let Some(sequence) = place {
            shader.sequence = sequence;
        }
        let reorder = !known || shader.enabled != state.enabled || state.priority.is_some_and(|priority| priority != shader.own_priority);
        shader.enabled = state.enabled;
        if let Some(priority) = state.priority {
            shader.own_priority = priority;
        }
        if let Some(buffer) = shader.values.as_ref() {
            scene.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&values_of(&shader.variables, &state.values)));
        }
        if !self.told_unknown.contains(&state.name) {
            let unknown: Vec<&str> = state.values.iter().map(|(name, _, _)| name.as_str()).filter(|name| !shader.variables.iter().any(|v| v.name == *name)).collect();
            if !unknown.is_empty() {
                self.told_unknown.push(state.name.clone());
                println!("post: {}: a mod sets variables that the shader does not have here: {}", state.name, unknown.join(", "));
            }
        }
        if reorder {
            self.order();
        }
    }

    /// The HDR flag and the reaction speed of the settings record of MGE XE.
    pub fn set_hdr(&mut self, on: bool, reaction: f32) {
        self.hdr_on = on;
        self.hdr_reaction = reaction;
        if let Some(hdr) = self.hdr.as_mut() {
            hdr.reaction = reaction.clamp(0.01, 30.0);
        }
    }

    /// No shader is on.
    pub fn is_empty(&self) -> bool {
        !self.chain.iter().any(|c| c.enabled)
    }

    /// The depth texture for the size of the picture; the pool starts again at a new size.
    fn size_textures(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.size == size && self.depth.is_some() {
            return;
        }
        self.size = size;
        self.pool.clear();
        let depth = make_texture(device, size, wgpu::TextureFormat::R32Float);
        let view = depth.create_view(&Default::default());
        self.depth = Some((depth, view));
    }

    /// The pool texture for a pass: one of its class that is not in use, or a new one.
    fn pooled(&mut self, device: &wgpu::Device, scale: u32, half_float: bool, in_use: &[usize]) -> usize {
        if let Some(index) = (0..self.pool.len()).find(|i| self.pool[*i].scale == scale && self.pool[*i].half_float == half_float && !in_use.contains(i)) {
            return index;
        }
        let size = (self.size.0.div_ceil(scale).max(1), self.size.1.div_ceil(scale).max(1));
        let format = if half_float { wgpu::TextureFormat::Rgba16Float } else { wgpu::TextureFormat::Rgba8Unorm };
        let view = make_texture(device, size, format).create_view(&Default::default());
        self.pool.push(Pooled { scale, half_float, view });
        self.pool.len() - 1
    }

    /// Runs the chain on the picture and writes the result into `target`.
    /// `distant` has the two depth numbers of the projection of the distant stage (the
    /// places 10 and 14 of a projection of Direct3D), when its depth is a part of the depth
    /// frame.
    pub fn run(&mut self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, variables: &Variables, environment_flags: u32, distant: Option<[f32; 2]>) {
        let (Some(picture), Some(depth)) = (scene.picture, scene.depth) else { return };
        if self.is_empty() {
            return;
        }
        let device = scene.device;
        self.size_textures(device, scene.target_size);
        self.frames += 1;
        // The adaptation point of the frame, from the picture before the chain.
        let mut variables = *variables;
        if let Some(hdr) = self.hdr.as_mut() {
            variables.hdr = if self.hdr_on { hdr.frame(device, encoder, picture, &self.blit_layout, &self.samplers[1], environment_flags) } else { hdr.point };
        }
        let variables = &variables;
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(variables));
        // The linear depth of the game.
        // Without a depth of the distant stage, the depth of the game stands in for it:
        // it is 1 where the first read was 1.
        let (distant_depth, distant) = match (scene.before_depth, distant) {
            (Some(view), Some(numbers)) => (view, numbers),
            _ => (depth, [0.0, 0.0]),
        };
        scene.queue.write_buffer(&self.depth_uniform, 0, bytemuck::cast_slice(&[variables.mproj[2][2], variables.mproj[3][2], distant[0], distant[1]]));
        let depth_view = &self.depth.as_ref().unwrap().1.clone();
        // The depth frame only when a shader of the chain of this frame reads it, or the
        // look at it is on.
        let reads_depth = self.debug == 2.0 || self.chain.iter().any(|c| c.enabled && c.disable_flags & environment_flags == 0 && c.reads_depth);
        if reads_depth {
            self.depth_frames += 1;
            let group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
                label: Some("post depth"),
                layout: &self.depth_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.depth_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(distant_depth) },
                ],
            });
            let mut pass = full_screen_pass(encoder, depth_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        // The chain. `last` is the pool texture that holds the input of the shader: `None` is
        // the picture. The passes of a shader each get a texture of their class that is not
        // the input and not an earlier pass of the shader (`pass0` to `pass7` read those).
        let mut last: Option<usize> = None;
        let frame_group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
            label: Some("post frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.waves) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.samplers[1]) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.samplers[2]) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.samplers[3]) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.samplers[4]) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&self.samplers[5]) },
            ],
        });
        let chain = std::mem::take(&mut self.chain);
        for compiled in &chain {
            if !compiled.enabled || compiled.disable_flags & environment_flags != 0 {
                continue;
            }
            self.shaders_run += 1;
            let mut in_use: Vec<usize> = last.into_iter().collect();
            let mut targets: Vec<usize> = Vec::with_capacity(compiled.passes.len());
            for pass in &compiled.passes {
                let sink = self.pooled(device, pass.scale, pass.half_float, &in_use);
                in_use.push(sink);
                let size = (self.size.0.div_ceil(pass.scale).max(1) as f32, self.size.1.div_ceil(pass.scale).max(1) as f32);
                scene.queue.write_buffer(&pass.info, 0, bytemuck::cast_slice(&[size.0, size.1, 1.0 / size.0, 1.0 / size.1]));
                let lastshader_view = match last {
                    Some(index) => &self.pool[index].view,
                    None => picture,
                };
                let lastpass_view = match targets.last() {
                    Some(index) => &self.pool[*index].view,
                    None => lastshader_view,
                };
                let earlier = |i: usize| targets.get(i).map_or(&self.white, |index| &self.pool[*index].view);
                let mut entries = vec![
                    wgpu::BindGroupEntry { binding: 0, resource: pass.info.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(lastshader_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(lastpass_view) },
                ];
                entries.extend((0..8).map(|i| wgpu::BindGroupEntry { binding: i as u32 + 3, resource: wgpu::BindingResource::TextureView(earlier(i)) }));
                let pass_group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor { label: Some(&compiled.name), layout: &self.pass_layout, entries: &entries });
                // A pass keeps what the texture held where it discards a pixel.
                let mut render = full_screen_pass(encoder, &self.pool[sink].view, wgpu::LoadOp::Load);
                render.set_pipeline(&pass.pipeline);
                render.set_bind_group(0, &frame_group, &[]);
                render.set_bind_group(1, &compiled.group, &[]);
                render.set_bind_group(2, &pass_group, &[]);
                render.draw(0..3, 0..1);
                drop(render);
                targets.push(sink);
            }
            // The output of the last pass is the input of the next shader.
            last = targets.last().copied();
        }
        self.chain = chain;
        // A look at a term, for a test.
        if self.debug > 0.0 {
            let source = if self.debug == 2.0 { depth_view } else { &self.pool[last.unwrap_or(0)].view };
            let group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
                label: Some("post debug"),
                layout: &self.debug_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.debug_uniform.as_entire_binding() },
                ],
            });
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.debug_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        // The result into the picture of the frame.
        if let Some(index) = last {
            let group = nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
                label: Some("post blit"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.pool[index].view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) }],
            });
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// A texture that a pass draws into and a shader reads.
fn make_texture(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("post"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn texture(binding: u32, sample_type: wgpu::TextureSampleType, dimension: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: dimension, multisampled: false }, count: None }
}

fn sampler(binding: u32, kind: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(kind), count: None }
}

pub(super) fn full_screen_pipeline(device: &wgpu::Device, label: &str, source: &str, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        multiview_mask: None,
        cache: None,
    })
}

pub(super) fn full_screen_pass<'a>(encoder: &'a mut wgpu::CommandEncoder, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("post"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "// A shader.\n#define N 16 // samples\nstatic const float strength = 1.8; // more is more\nfloat fov;\nfloat4 pass(float2 t : TEXCOORD0) : COLOR0 { return strength * N; }\ntechnique T0 < string MGEinterface = \"MGE XE 0\"; string category = \"scene\"; int priorityAdjust = -10000; > { pass { PixelShader = compile ps_3_0 pass(); } }\n";

    #[test]
    fn the_code_key_ignores_numbers_comments_space_and_annotations() {
        let other_numbers = FILE.replace("#define N 16 // samples", "#define   N   32").replace("strength = 1.8; // more is more", "strength = 1.2;");
        assert_eq!(code_key(FILE), code_key(&other_numbers));
        let no_category = FILE.replace(" string category = \"scene\"; int priorityAdjust = -10000;", "");
        assert_eq!(code_key(FILE), code_key(&no_category));
        let windows = FILE.replace('\n', "\r\n");
        assert_eq!(code_key(FILE), code_key(&windows));
        let other_code = FILE.replace("return strength * N;", "return strength + N;");
        assert_ne!(code_key(FILE), code_key(&other_code));
    }

    #[test]
    fn the_numbers_of_a_file_are_read() {
        let numbers = file_numbers("#define N 16\nstatic const float a = 1.5f;\nextern int b = 5005;\nstatic float c = -0.06;\nconst bool d = true;\nfloat fov;\nstatic const float2 e = float2(1, 2);\n#define BLUR\n");
        assert_eq!(numbers, vec![("N".to_string(), 16.0), ("a".to_string(), 1.5), ("b".to_string(), 5005.0), ("c".to_string(), -0.06), ("d".to_string(), 1.0)]);
    }

    #[test]
    fn a_port_takes_the_marked_numbers() {
        let port = "const N: i32 = 16;\nconst strength: f32 = 1.8; // file\nconst other: f32 = 2.0; // file\n";
        let (fitted, taken) = with_file_numbers(port, &[("N".to_string(), 32.0), ("strength".to_string(), 1.0)]);
        assert_eq!(fitted, "const N: i32 = 16;\nconst strength: f32 = 1.0; // file\nconst other: f32 = 2.0; // file\n");
        assert_eq!(taken, vec!["strength".to_string()]);
    }

    #[test]
    fn a_port_variable_takes_the_number_of_the_file() {
        let mut variables = vec![
            VariableDef { name: "warpint".to_string(), slots: 1, per: 1, default: vec![0.003] },
            VariableDef { name: "fogColors".to_string(), slots: 8, per: 4, default: vec![0.0; 32] },
        ];
        let taken = fit_variable_defaults(&mut variables, &[("warpint".to_string(), 0.005), ("fogColors".to_string(), 1.0), ("other".to_string(), 2.0)]);
        assert_eq!(taken, vec!["warpint".to_string()]);
        assert_eq!(variables[0].default, vec![0.005f32]);
        assert_eq!(variables[1].default, vec![0.0; 32]);
    }

    #[test]
    fn the_status_file_has_the_result_of_each_shader() {
        let mut status = ShaderStatus::default();
        status.note("a", None);
        status.note("b c", Some("first line\nsecond line".to_string()));
        assert_eq!(status.text(), "sequence 2\nshader a\nloads 1\nok\nshader b c\nloads 1\nerror\n| first line\n| second line\n");
        // A new result of a shader takes the place of the old one, and the sequence changes.
        status.note("b c", None);
        assert_eq!(status.text(), "sequence 3\nshader a\nloads 1\nok\nshader b c\nloads 2\nok\n");
    }

    #[test]
    fn an_error_gets_the_line_numbers_of_the_file() {
        let message = "Shader 'x' parsing error: no definition in scope for identifier: `colr`\n   \u{250c}\u{2500} wgsl:215:12\n    \u{2502}\n215 \u{2502}     return colr;\n    \u{2502}            ^^^^ unknown identifier\n";
        let own = own_lines(message, 200);
        assert!(own.contains("wgsl:15:12"), "{own}");
        assert!(own.contains("15 \u{2502}     return colr;"), "{own}");
        assert!(own.contains("unknown identifier"));
        // A line of the common part keeps its number.
        assert!(own_lines("wgsl:50:1", 200).contains("wgsl:50:1"));
        // The lines of wgpu before the message of the compiler go.
        let wrapped = format!("Validation Error\n\nCaused by:\n  In Device::create_shader_module, label = 'x'\n    \n{message}");
        assert!(compiler_part(&wrapped).starts_with("Shader 'x' parsing error"));
        assert_eq!(compiler_part("no compiler part"), "no compiler part");
    }

    #[test]
    fn the_place_of_a_file_comes_from_its_technique() {
        assert_eq!(file_priority(FILE), 1_000_000 - 10_000);
        assert_eq!(file_priority("technique T0 < string MGEinterface=\"MGE XE 0\"; >"), 6_000_000);
        assert_eq!(file_priority("technique T0<string MGEinterface = \"MGE XE 0\"; string category = \"atmosphere\";> {"), 2_000_000);
    }

    /// A record as [`PostShaders::sendShaderStates`] writes it.
    fn record(name: &str, flags: u32, priority: i32, variables: &[(&str, u8, &[f32])]) -> Vec<u8> {
        let padded = |text: &str| {
            let mut bytes = [0u8; 64];
            bytes[..text.len()].copy_from_slice(text.as_bytes());
            bytes
        };
        let mut out = Vec::new();
        out.extend(1u32.to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend(priority.to_le_bytes());
        out.extend((variables.len() as u32).to_le_bytes());
        out.extend(padded(name));
        for (variable, kind, values) in variables {
            out.extend(padded(variable));
            out.extend((*kind as u32).to_le_bytes());
            out.extend((values.len() as u32).to_le_bytes());
            for value in *values {
                out.extend(value.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn the_state_of_a_shader_is_read() {
        let bytes = record("oj_me_fog_box", 3, -5, &[("fogDensities", b'F', &[0.5, 0.25]), ("fogCenters", b'F', &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])]);
        let state = ShaderState::read(&bytes).unwrap();
        assert_eq!(state.name, "oj_me_fog_box");
        assert!(state.enabled);
        assert_eq!(state.priority, Some(-5));
        assert_eq!(state.values.len(), 2);
        assert_eq!(state.values[1], ("fogCenters".to_string(), b'F', vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        // A record that ends before its values, and another version.
        assert!(ShaderState::read(&bytes[..bytes.len() - 4]).is_none());
        assert!(ShaderState::read(&record("x", 0, 0, &[])[4..]).is_none());
        let off = ShaderState::read(&record("x", 0, 9, &[])).unwrap();
        assert!(!off.enabled && off.priority.is_none());
        assert_eq!(off.reloads, 0);
        assert_eq!(ShaderState::read(&record("x", 1 | (3 << 8), 0, &[])).unwrap().reloads, 3);
    }

    #[test]
    fn the_variables_go_to_their_places() {
        let variables = vec![
            VariableDef { name: "fogCenters".to_string(), slots: 2, per: 3, default: vec![] },
            VariableDef { name: "fogDensities".to_string(), slots: 2, per: 1, default: vec![7.0, 8.0] },
            VariableDef { name: "tints".to_string(), slots: 2, per: 3, default: vec![] },
        ];
        let set = vec![
            ("fogCenters".to_string(), b'F', vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 99.0]),
            // An array of vectors of the API has 4 floats for each vector.
            ("tints".to_string(), b'V', vec![0.1, 0.2, 0.3, 9.0, 0.4, 0.5, 0.6, 9.0]),
        ];
        let values = values_of(&variables, &set);
        assert_eq!(values.len(), MOD_SLOTS * 4);
        assert_eq!(&values[0..8], &[1.0, 2.0, 3.0, 0.0, 4.0, 5.0, 6.0, 0.0]);
        // The default of a variable that no mod set.
        assert_eq!(&values[8..16], &[7.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0]);
        assert_eq!(&values[16..24], &[0.1, 0.2, 0.3, 0.0, 0.4, 0.5, 0.6, 0.0]);
        let functions = variables_wgsl(&variables);
        assert!(functions.contains("fn fogCenters(i: i32) -> vec3f { return modv.values[0 + clamp(i, 0, 1)].xyz; }"));
        assert!(functions.contains("fn fogDensities(i: i32) -> f32 { return modv.values[2 + clamp(i, 0, 1)].x; }"));
        assert!(functions.contains("fn tints(i: i32) -> vec3f { return modv.values[4 + clamp(i, 0, 1)].xyz; }"));
    }

    #[test]
    fn a_variable_line_of_a_file_is_read() {
        let variable = parse_variable("tint vec3 = 1 0.5 0").unwrap();
        assert_eq!((variable.name.as_str(), variable.slots, variable.per, variable.default.as_slice()), ("tint", 1, 3, &[1.0, 0.5, 0.0][..]));
        let array = parse_variable("densities f32[8]").unwrap();
        assert_eq!((array.slots, array.per, array.default.len()), (8, 1, 0));
        assert!(parse_variable("x mat4").is_err());
        assert!(parse_variable("x f32[0]").is_err());
    }

    /// The WGSL of each port is valid, with the common part and the functions of its
    /// variables, and each pass is a fragment function of it.
    #[test]
    fn each_port_is_valid_wgsl() {
        use wgpu::naga;
        let mut problems = Vec::new();
        for shader in ports() {
            let definition = Definition::of_port(shader);
            let source = format!("{COMMON}\n{}\n{}", variables_wgsl(&definition.variables), shader.source);
            let module = match naga::front::wgsl::parse_str(&source) {
                Ok(module) => module,
                Err(error) => {
                    problems.push(format!("{}: {}", shader.name, error.emit_to_string(&source)));
                    continue;
                }
            };
            if let Err(error) = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module) {
                problems.push(format!("{}: {}", shader.name, error.emit_to_string(&source)));
            }
            for pass in shader.passes {
                if !module.entry_points.iter().any(|entry| entry.name == pass.entry && entry.stage == naga::ShaderStage::Fragment) {
                    problems.push(format!("{}: the pass {} is not a fragment function", shader.name, pass.entry));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn each_port_names_passes_and_each_stock_port_has_its_file() {
        for shader in ports() {
            assert!(!shader.passes.is_empty(), "{}", shader.name);
            for pass in shader.passes {
                assert!(shader.source.contains(&format!("fn {}(", pass.entry)), "{}: the pass {}", shader.name, pass.entry);
            }
        }
        for (name, text) in STOCK_FILES {
            let port = ports().find(|shader| shader.name == *name).unwrap_or_else(|| panic!("{name} has no port"));
            // The place that the table gives a stock port is that of its file.
            assert_eq!(port.priority, file_priority(text), "{name}");
        }
        for (name, _, port) in MOD_FILES {
            assert!(ports().any(|shader| shader.name == *port), "{name} has no port {port}");
        }
    }
}
