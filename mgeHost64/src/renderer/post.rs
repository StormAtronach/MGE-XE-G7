//! The post-processing chain of MGE XE: the shaders of `[shaders] chain`, from their WGSL
//! files (`Data Files/shaders/XEshaders/<name>.wgsl`, of our package or of a mod), run on the
//! picture of the frame after the world views and before the user interface of the game. The
//! specification of the chain is in `moreFPS/docs/plans/mge-xe-post-process-spec.md`.
//!
//! A shader is a WGSL module with one fragment function for each pass; each pass draws a
//! full-screen triangle into a texture of its own: `rgba8unorm` of the size of the picture
//! without a word, or a fraction of the size and 16-bit floats when the pass asks for them
//! (a pool of textures by size and format serves all the shaders of the chain).
//! `lastshader` is the input of the shader (the picture, or the output of the shader before),
//! `lastpass` the output of the pass before, and `pass0` to `pass7` the outputs of the first
//! eight passes of the shader. The variables of MGE XE are one uniform.

use mge_config::post_header;
use nirender_wgpu::gpu::KeptGroup;
use nirender_wgpu::objects::{Create, CreateView};
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
    /// x: 1 with the exponential fog (the macro `USE_EXPFOG` of an effect file). y: the night,
    /// 0 in the day and 1 at night (the environment's `night`), which no variable of MGE XE
    /// has: a shader file reads it in place of a mod's Lua that set a variable by the hour.
    pub switches: [f32; 4],
    /// The weather of the game (record 16, W622): the index of the weather now, the index of
    /// the weather it changes to, the ratio of the change, and 1 when the record came (all 0
    /// without it). The `weather_*` functions of [`COMMON`] read it.
    pub weather: [f32; 4],
}

/// The uniform of the lights of the frame, for a shader that reads them (`frame_lights` and
/// `frame_light_info` of [`COMMON`]; the lights are the crate's `LightData`).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameLightInfo {
    /// The point that the positions of the lights are relative to, less the eye: a position
    /// plus it is the place of the light relative to the eye.
    offset: [f32; 4],
    /// The number of lights in `frame_lights`.
    count: u32,
    pad: [u32; 3],
}

impl FrameLightInfo {
    /// The uniform for `lights` of the frame and the eye.
    fn of(lights: Option<&nirender_wgpu::FrameLights>, eye: [f32; 3]) -> FrameLightInfo {
        let Some(lights) = lights else {
            return FrameLightInfo::default();
        };
        let offset: [f32; 3] = core::array::from_fn(|axis| (lights.origin[axis] - eye[axis] as f64) as f32);
        FrameLightInfo {
            offset: [offset[0], offset[1], offset[2], 0.0],
            count: lights.count,
            pad: [0; 3],
        }
    }
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
    weather: vec4f,
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
// The lights of the frame (the crate's `LightData`), for a shader that names `frame_lights`:
// the lights of the lit draws of the world in this frame. `position.w` is the type (1 point,
// 2 spot, 3 directional); `cone.z` is the radius of the game's light. A position plus
// `frame_light_info.offset.xyz` is the place of the light relative to the eye.
struct FrameLight {
    diffuse: vec4f,
    ambient: vec4f,
    position: vec4f,
    direction: vec4f,
    attenuation: vec4f,
    cone: vec4f,
}
struct FrameLightInfo {
    offset: vec4f,
    count: u32,
}
@group(0) @binding(9) var<storage, read> frame_lights: array<FrameLight>;
@group(0) @binding(10) var<uniform> frame_light_info: FrameLightInfo;

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
// The weather of the game, by its index: 0 clear, 1 cloudy, 2 foggy, 3 overcast, 4 rain,
// 5 thunder, 6 ash, 7 blight, 8 snow, 9 blizzard. weather_known() is false without the weather
// record (an older d3d8.dll or capture); the other functions then give weather 0.
fn weather_known() -> bool {
    return u.weather.w > 0.5;
}
// The weather now, and the weather it changes to (the same when it does not change), 0 to 9.
fn weather_now() -> u32 {
    return min(u32(max(u.weather.x, 0.0)), 9u);
}
fn weather_next() -> u32 {
    return min(u32(max(u.weather.y, 0.0)), 9u);
}
// The ratio of the change, 0 to 1.
fn weather_ratio() -> f32 {
    return clamp(u.weather.z, 0.0, 1.0);
}
// A value for each weather, mixed from the weather now to the weather it changes to by the
// ratio. The table has one value for each index.
fn weather_mix(by_weather: array<f32, 10>) -> f32 {
    var table = by_weather;
    return mix(table[weather_now()], table[weather_next()], weather_ratio());
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

/// The places of the variables of a shader in the uniform `modv`: 256 vectors of 4 floats.
const MOD_SLOTS: usize = 256;

/// A variable of a shader as this program compiles it.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
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
        let loads = self
            .list
            .iter()
            .find(|(known, _, _)| known == name)
            .map_or(0, |(_, _, loads)| *loads)
            + 1;
        self.list.retain(|(known, _, _)| known != name);
        self.list.push((name.to_string(), error, loads));
        self.sequence += 1;
    }

    pub fn text(&self) -> String {
        let mut text = format!("sequence {}\n", self.sequence);
        for (name, error, loads) in &self.list {
            text.push_str(&format!(
                "shader {name}\nloads {loads}\n{}\n",
                if error.is_some() { "error" } else { "ok" }
            ));
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
    let own = |number: &str| {
        number
            .parse::<usize>()
            .ok()
            .filter(|n| *n > offset)
            .map(|n| (n - offset).to_string())
    };
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
        let name = |at: usize| {
            bytes
                .get(at..at + 64)
                .map(|b| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(64)]).into_owned())
        };
        if word(0)? != 1 {
            return None;
        }
        let flags = word(4)?;
        let mut state = ShaderState {
            name: name(16)?,
            enabled: flags & 1 != 0,
            priority: (flags & 2 != 0).then_some(word(8)? as i32 as i64),
            reloads: flags >> 8,
            values: Vec::new(),
        };
        let mut at = 80;
        for _ in 0..word(12)? {
            let (variable, kind, count) = (name(at)?, word(at + 64)?, word(at + 68)? as usize);
            let floats = bytes.get(at + 72..at + 72 + count * 4)?;
            state.values.push((
                variable,
                kind as u8,
                floats.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
            ));
            at += 72 + count * 4;
        }
        Some(state)
    }

    /// True when `other` differs from this state only in the values of its variables: the
    /// same shader, switch, priority and reloads, and the same variables with the same types
    /// and numbers of values. Such a change goes into the values that the shader reads, with
    /// no new GPU object.
    pub fn same_but_values(&self, other: &ShaderState) -> bool {
        let layout = |state: &ShaderState| {
            state
                .values
                .iter()
                .map(|(name, kind, floats)| (name.clone(), *kind, floats.len()))
                .collect::<Vec<_>>()
        };
        self.name == other.name
            && self.enabled == other.enabled
            && self.priority == other.priority
            && self.reloads == other.reloads
            && layout(self) == layout(other)
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
        "abs",
        "acos",
        "all",
        "any",
        "asin",
        "atan",
        "ceil",
        "clamp",
        "cos",
        "cross",
        "degrees",
        "distance",
        "dot",
        "exp",
        "floor",
        "fract",
        "length",
        "log",
        "max",
        "min",
        "mix",
        "modf",
        "normalize",
        "pow",
        "radians",
        "reflect",
        "round",
        "saturate",
        "sign",
        "sin",
        "smoothstep",
        "sqrt",
        "step",
        "tan",
        "trunc",
        "select",
        "sample",
        "filter",
        "target",
        "texture",
        "loop",
        "switch",
        "default",
        "const",
        "var",
        "let",
        "fn",
        "struct",
        "return",
        "if",
        "else",
        "for",
        "while",
        "break",
        "continue",
        "discard",
        "true",
        "false",
        "u",
        "vs",
        "modv",
        "rcpres",
        "mview",
        "mproj",
    ];
    if TAKEN.contains(&variable) {
        format!("{variable}_")
    } else {
        variable.to_string()
    }
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
            out.push_str(&format!(
                "fn {name}(i: i32) -> {kind} {{ return modv.values[{slot} + clamp(i, 0, {})]{parts}; }}\n",
                variable.slots - 1
            ));
        }
        slot += variable.slots;
    }
    out
}

/// A shader of the chain as this program compiles it: from its WGSL file.
#[cfg_attr(test, derive(Debug, PartialEq))]
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

impl Definition {
    /// A shader file of a mod: `Data Files/shaders/XEshaders/<name>.wgsl`. The file starts
    /// with lines `//! key: value`, which have what the technique of an effect file of
    /// MGE XE has in its annotations; the rest is WGSL, compiled after the common part
    /// ([`COMMON`]: the variables `u`, the textures, the samplers, the vertex function `vs`
    /// and the struct `Quad`). mge-config's `post_header` reads the lines and lists the keys;
    /// d3d8.dll reads them with the same parser. The variables that a mod sets from Lua are
    /// read in the WGSL as `a()`, `b()`, `c(i)`; the settings are variables that d3d8.dll
    /// keeps between game sessions, so the helper uses only the variables.
    fn of_file(name: &str, text: &str) -> Result<Definition, String> {
        let header = post_header::parse(text)?;
        Ok(Definition {
            name: name.to_string(),
            source: text.to_string(),
            passes: header
                .passes
                .iter()
                .map(|pass| (pass.entry.clone(), pass.scale, pass.half_float))
                .collect(),
            disable_flags: header.mgeflags.unwrap_or(0),
            no_sun_glare: header.disable_sunglare,
            priority: header.priority(),
            textures: header.textures,
            requires_hdr: header.requires_hdr,
            variables: header
                .variables
                .into_iter()
                .map(|variable| VariableDef {
                    name: variable.name,
                    slots: variable.slots,
                    per: variable.per,
                    default: variable.default,
                })
                .collect(),
        })
    }

    /// The shader of the chain name `name`: the file `shaders/XEshaders/<name>.wgsl` that `read`
    /// gives. A name with no such file is skipped, also when an effect file `<name>.fx` is there:
    /// the renderer has no shader of its own for an effect file (W609 step 5).
    fn of_chain_name(name: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Definition, Skip> {
        let Some(bytes) = read(&format!("shaders/XEshaders/{name}.wgsl")) else {
            return Err(Skip {
                line: format!("post: no file {name}.wgsl; skipped"),
                note: format!("The wgpu renderer has no file {name}.wgsl."),
            });
        };
        Definition::of_file(name, &String::from_utf8_lossy(&bytes)).map_err(|problem| Skip {
            line: format!("post: the shader file {name}.wgsl is not used: {problem}"),
            note: format!("The first lines of {name}.wgsl (the lines with //!) are not right: {problem}"),
        })
    }
}

/// Why a chain name gives no shader: the line of the log and the note of the status file.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Skip {
    line: String,
    note: String,
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
    /// The last luminance read, 0.5 before the first one.
    luminance: f32,
    point: [f32; 4],
    /// The step of the point from the clock of the game.
    clock: HdrClock,
    /// The call of the cycle that comes next.
    cycle: HdrCycle,
    /// Set by the callback of the map, from the call that asks for the map to the call that reads.
    mapped: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The newest submit at the call that asks for the map: it holds the commands of the copy,
    /// and the read waits for it, not for the submits of the frames after it.
    copy_submission: Option<wgpu::SubmissionIndex>,
    /// `render.hdr_reaction_time`, seconds.
    reaction: f32,
}

/// What a call of [`Hdr::frame`] does in the cycle of the measure.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HdrAction {
    /// Downsample the picture and copy the pixel into the buffer, in the commands of the frame.
    Copy,
    /// Ask for the map of the buffer, when the commands of the copy are with the GPU.
    Map,
    /// Nothing: the GPU works.
    Wait,
    /// Read the pixel that the copy made.
    Read,
}

/// The calls of the cycle, in the order of `PostShaders::evalAdaptHDR` of MGE XE (HDR_IDLE,
/// HDR_DOWNSAMPLING, HDR_WAITING, HDR_READY_TO_READ): the pixel is read at the third call after
/// its copy, whatever the progress of the GPU.
const HDR_CYCLE: [HdrAction; 4] = [HdrAction::Copy, HdrAction::Map, HdrAction::Wait, HdrAction::Read];

#[derive(Default)]
struct HdrCycle {
    next: usize,
}

impl HdrCycle {
    fn step(&mut self) -> HdrAction {
        let action = HDR_CYCLE[self.next];
        self.next = (self.next + 1) % HDR_CYCLE.len();
        action
    }

    /// The call that was just stepped comes again at the next call.
    fn repeat(&mut self) {
        self.next = (self.next + HDR_CYCLE.len() - 1) % HDR_CYCLE.len();
    }
}

/// The longest wait of the read for the GPU. A GPU that is slower than this (a hang) does not
/// hold the helper: the read comes again at the next call.
const HDR_MOST_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// The wait of the read: until the submit `copy` has run, for at most [`HDR_MOST_WAIT`].
fn hdr_wait(copy: Option<wgpu::SubmissionIndex>) -> wgpu::PollType {
    wgpu::PollType::Wait {
        submission_index: copy,
        timeout: Some(HDR_MOST_WAIT),
    }
}

/// The longest step of the point, in seconds. A step above it is a load or a jump of the clock,
/// not a frame: the clamp is that of the ripples (`MOST_FRAME_TIME`).
const HDR_MOST_STEP: f32 = 0.5;

/// The step of the point: the time that the clock of the game (`Environment::time`) advanced
/// since the call before, as MGE XE takes the frame time of the game.
#[derive(Default)]
struct HdrClock {
    last: Option<f32>,
}

impl HdrClock {
    /// The step to the time `time`: 0 for the first call, for a clock that stands (a menu) and for a
    /// clock that goes back (a load); at most [`HDR_MOST_STEP`].
    fn step(&mut self, time: f32) -> f32 {
        let step = self.last.map_or(0.0, |last| time - last);
        self.last = Some(time);
        if step > 0.0 { step.min(HDR_MOST_STEP) } else { 0.0 }
    }
}

impl Hdr {
    fn new(device: &wgpu::Device, blit_layout: &wgpu::BindGroupLayout, reaction: f32) -> Hdr {
        let texture = device.counted_texture(&wgpu::TextureDescriptor {
            label: Some("post hdr"),
            size: wgpu::Extent3d {
                width: 512,
                height: 512,
                depth_or_array_layers: 1,
            },
            mip_level_count: 10,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let levels = (0..10)
            .map(|level| {
                texture.counted_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let pipeline = full_screen_pipeline(device, "post hdr", BLIT_SHADER, blit_layout, wgpu::TextureFormat::Rgba8Unorm);
        let buffer = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post hdr"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Hdr {
            texture,
            levels,
            pipeline,
            buffer,
            luminance: 0.5,
            point: [1.0; 4],
            clock: HdrClock::default(),
            cycle: HdrCycle::default(),
            mapped: None,
            copy_submission: None,
            reaction: reaction.clamp(0.01, 30.0),
        }
    }

    /// One frame of the measure, at the time `time` of the clock of the game. Returns the
    /// adaptation point.
    fn frame(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        picture: &wgpu::TextureView,
        blit_layout: &wgpu::BindGroupLayout,
        linear: &wgpu::Sampler,
        environment_flags: u32,
        time: f32,
    ) -> [f32; 4] {
        use std::sync::atomic::{AtomicBool, Ordering};
        match self.cycle.step() {
            HdrAction::Copy => {
                // A linear copy into each level is the linear `StretchRect` of Direct3D 9:
                // the mean of 2 x 2 pixels from the second level on.
                let mut source = picture;
                for level in &self.levels {
                    let group = nirender_wgpu::gpu::frame_group(
                        device,
                        &wgpu::BindGroupDescriptor {
                            label: Some("post hdr"),
                            layout: blit_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(source),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(linear),
                                },
                            ],
                        },
                    );
                    let mut pass = full_screen_pass(
                        encoder,
                        level,
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        "post: the hdr levels",
                    );
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &group, &[]);
                    pass.draw(0..3, 0..1);
                    drop(pass);
                    source = level;
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: 9,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &self.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: None,
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            HdrAction::Map => {
                let ready = std::sync::Arc::new(AtomicBool::new(false));
                let flag = ready.clone();
                self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_ok() {
                        flag.store(true, Ordering::Relaxed);
                    }
                });
                self.mapped = Some(ready);
                self.copy_submission = nirender_wgpu::gpu::last_submission();
            }
            HdrAction::Wait => {}
            HdrAction::Read => {
                // The read is at this call. When the GPU has not finished the copy, the call waits
                // for the submit that holds it, as `LockRect` of Direct3D 9 does, so the value does
                // not depend on the progress of the GPU. The wait has a limit: when it ends before
                // the copy, the map stays and the read comes again at the next call (the buffer
                // is not copied into while the map is open).
                if let Some(ready) = self.mapped.take() {
                    if !ready.load(Ordering::Relaxed) && !nirender_wgpu::gpu::lost() {
                        let _ = device.poll(hdr_wait(self.copy_submission.clone()));
                    }
                    if ready.load(Ordering::Relaxed) {
                        if let Ok(pixel) = self.buffer.slice(..).get_mapped_range() {
                            let (r, g, b) = (pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[2] as f32 / 255.0);
                            self.luminance = 0.27 * r + 0.67 * g + 0.06 * b;
                        }
                        self.buffer.unmap();
                        self.copy_submission = None;
                    } else {
                        self.mapped = Some(ready);
                        self.cycle.repeat();
                    }
                }
            }
        }
        // Each frame: the point follows the luminance with the reaction time. The scale is
        // for an exterior and an interior that behaves as one (the flags 2 and 4).
        let dt = self.clock.step(time);
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

/// The linear depth of the frame, as the depth frame of the Direct3D 9 path has it
/// (`DistantLand::renderDepth`): the depth of the game where the game drew, else the depth
/// of the distant land and the distant statics, else `1e38`. `p` has the depth terms of the
/// projection of the game, then those of the projection of the distant stage
/// (`nirender_wgpu::depth`).
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
    if !depth_empty(d) {
        return vec4f(view_distance(d, p.xy), 0.0, 0.0, 1.0);
    }
    let far = textureLoad(distant, vec2i(in.position.xy), 0);
    if !depth_empty(far) {
        return vec4f(view_distance(far, p.zw), 0.0, 0.0, 1.0);
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

/// A look at a term, for a test: the debug view `post 1` tints the result, `post 2` shows the linear
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

/// A pass as it runs: its pipeline, and the scale and the format of its target. The uniform with
/// the size of the target is a view's own ([`PostView::pass_info`]).
struct CompiledPass {
    pipeline: wgpu::RenderPipeline,
    scale: u32,
    half_float: bool,
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
    /// The code of the shader reads the lights of the frame (`frame_lights`).
    reads_lights: bool,
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

/// What a run of the post chain writes or keeps for one view: the main view's picture, or the
/// picture of a place of the full tier. The chain, its pipelines, layouts, samplers and mod
/// values are shared ([`Post`]); everything below is the view's own, for two reasons. A write
/// with `queue.write_buffer` lands before all commands of the frame (the one-submit rule), so
/// two runs in one frame would read the last write of a shared buffer. A pass loads its target
/// (`LoadOp::Load`), so a pool texture holds the last run's output where a shader discards a
/// pixel, and two views would show each other's.
pub struct PostView {
    /// The size of the picture; the pool and the depth frame start again at a new size.
    size: (u32, u32),
    /// The targets of the passes, by their class, for the size of the picture.
    pool: Vec<Pooled>,
    /// The linear depth of the game.
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The variables of the frame and the terms of the depth frame.
    uniforms: wgpu::Buffer,
    depth_uniform: wgpu::Buffer,
    /// The uniform of the lights of the frame ([`FrameLightInfo`]).
    light_info: wgpu::Buffer,
    /// The uniform with the size of the target of each pass, by the name of the shader and the
    /// number of the pass in it, so that a reload or a reorder of the chain gives a pass no
    /// buffer of another. A buffer is written before its pass in every run.
    infos: std::collections::HashMap<String, Vec<wgpu::Buffer>>,
    /// The groups of the chain, each kept while what it binds stays the same: the depth frame,
    /// the frame, each pass by the name of its shader and its number (as `infos`), and the blit.
    depth_group: KeptGroup,
    frame_group: KeptGroup,
    pass_groups: std::collections::HashMap<String, Vec<KeptGroup>>,
    blit_group: KeptGroup,
    /// This view measures the adaptation point (the main view); the others read its point.
    owns_hdr: bool,
    hdr: Option<Hdr>,
}

impl PostView {
    /// The state of the main view, which has the HDR measure.
    pub fn main(device: &wgpu::Device) -> PostView {
        PostView::new(device, true)
    }

    /// The state of a place of the full tier: no measure of its own.
    pub fn place(device: &wgpu::Device) -> PostView {
        PostView::new(device, false)
    }

    /// A state of a place with its textures made for a picture of `size`: it has bytes, as one that
    /// ran the chain has. For the tests of the bytes that the views count.
    #[cfg(test)]
    pub fn place_with_textures(device: &wgpu::Device, size: (u32, u32)) -> PostView {
        let mut view = PostView::new(device, false);
        view.size_textures(device, size);
        view
    }

    fn new(device: &wgpu::Device, owns_hdr: bool) -> PostView {
        let uniforms = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: size_of::<Variables>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let depth_uniform = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post depth"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_info = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post lights"),
            size: size_of::<FrameLightInfo>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PostView {
            size: (0, 0),
            pool: Vec::new(),
            depth: None,
            uniforms,
            depth_uniform,
            light_info,
            infos: std::collections::HashMap::new(),
            depth_group: KeptGroup::default(),
            frame_group: KeptGroup::default(),
            pass_groups: std::collections::HashMap::new(),
            blit_group: KeptGroup::default(),
            owns_hdr,
            hdr: None,
        }
    }

    /// The adaptation point that this view measured; `[1; 4]` before the measure exists.
    pub fn hdr_point(&self) -> [f32; 4] {
        self.hdr.as_ref().map_or([1.0; 4], |hdr| hdr.point)
    }

    /// The bytes of GPU memory of the textures of this view: the pool, the depth frame and the
    /// measure (a texture of 512 x 512 pixels with its levels).
    pub fn bytes(&self) -> u64 {
        self.bytes_at(self.size)
    }

    /// The bytes of GPU memory that the textures of this view would take for a picture of `size`:
    /// the pool classes it has, the depth frame if it has one, and the measure. The size rule of
    /// the views of their own uses the main view's number at another view's size.
    pub fn bytes_at(&self, size: (u32, u32)) -> u64 {
        let pool: u64 = self
            .pool
            .iter()
            .map(|pooled| {
                u64::from(size.0.div_ceil(pooled.scale).max(1))
                    * u64::from(size.1.div_ceil(pooled.scale).max(1))
                    * if pooled.half_float { 8 } else { 4 }
            })
            .sum();
        let depth = self.depth.as_ref().map_or(0, |_| u64::from(size.0) * u64::from(size.1) * 4);
        let hdr = self.hdr.as_ref().map_or(0, |_| 512 * 512 * 4 * 4 / 3);
        pool + depth + hdr
    }

    /// A bound for the bytes of the textures of a view of `size` when no view has a pool to count
    /// from: the depth frame, and three textures of the picture's size at half float.
    pub fn bytes_bound(size: (u32, u32)) -> u64 {
        u64::from(size.0) * u64::from(size.1) * (4 + 3 * 8)
    }

    /// True when the depth frame and the pool are made for a picture of `size`: a run of the post
    /// chain makes no texture for the depth frame then.
    pub fn has_textures_of(&self, size: (u32, u32)) -> bool {
        self.size == size && self.depth.is_some()
    }

    /// The depth texture for the size of the picture; the pool starts again at a new size.
    fn size_textures(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.size == size && self.depth.is_some() {
            return;
        }
        self.size = size;
        self.pool.clear();
        self.depth_group = KeptGroup::default();
        self.frame_group = KeptGroup::default();
        self.pass_groups.clear();
        self.blit_group = KeptGroup::default();
        let depth = make_texture(device, size, wgpu::TextureFormat::R32Float);
        let view = depth.counted_view(&Default::default());
        self.depth = Some((depth, view));
    }

    /// The pool texture for a pass: one of its class that is not in use, or a new one.
    fn pooled(&mut self, device: &wgpu::Device, scale: u32, half_float: bool, in_use: &[usize]) -> usize {
        if let Some(index) = (0..self.pool.len())
            .find(|i| self.pool[*i].scale == scale && self.pool[*i].half_float == half_float && !in_use.contains(i))
        {
            return index;
        }
        let size = (self.size.0.div_ceil(scale).max(1), self.size.1.div_ceil(scale).max(1));
        let format = if half_float {
            wgpu::TextureFormat::Rgba16Float
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let view = make_texture(device, size, format).counted_view(&Default::default());
        self.pool.push(Pooled { scale, half_float, view });
        self.pool.len() - 1
    }

    /// The uniform with the size of the target of pass `number` of the shader `name`.
    fn pass_info(&mut self, device: &wgpu::Device, name: &str, number: usize) -> &wgpu::Buffer {
        if !self.infos.contains_key(name) {
            self.infos.insert(name.to_string(), Vec::new());
        }
        let buffers = self.infos.get_mut(name).expect("the entry was made above");
        while buffers.len() <= number {
            buffers.push(device.counted_buffer(&wgpu::BufferDescriptor {
                label: Some("post pass"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        &buffers[number]
    }
}

pub struct Post {
    /// The shaders of the chain that have a file, and the shaders that a mod loaded, in their
    /// order.
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
    /// The lights of a frame without lights: one light of zeros.
    no_lights: wgpu::Buffer,
    hdr_reaction: f32,
    samplers: [wgpu::Sampler; 6],
    white: wgpu::TextureView,
    /// A black pixel: what a shader reads from a texture that was not found, as from an
    /// unset texture of Direct3D 9.
    black: wgpu::TextureView,
    depth_pipeline: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    /// The term of the debug view `post` (0: off): [`Post::set_debug`].
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
    /// A shader of the chain needs the HDR measure. The measure itself (`Hdr`) is the state of
    /// the main view's [`PostView`]: the player's eye adapts once, and a view of its own reads the
    /// main view's point. `hdr_on` is the flag of MGE XE (`mge.render.updateHDR`): off, the
    /// measure stops and the shaders keep its last point, as `evalAdaptHDR` is skipped on
    /// Direct3D 9.
    needs_hdr: bool,
    hdr_on: bool,
    pub shaders_run: u64,
    /// The frames that made the depth frame (a shader of the chain read it).
    pub depth_frames: u64,
}

impl Post {
    /// Makes the chain from the names of the settings. A name is a WGSL file
    /// (`shaders/XEshaders/<name>.wgsl`, see [`Definition::of_file`]), of our package or of a
    /// mod; a name with no file, and a file that does not compile, is reported and skipped. `waves` is the 3D wave texture of the water; `read` gives the
    /// bytes of a file under `Data Files`; `hdr_reaction` is `render.hdr_reaction_time`.
    pub fn new(
        scene: &SceneView,
        chain: &[String],
        waves: wgpu::TextureView,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
        hdr_reaction: f32,
    ) -> Post {
        let device = scene.device;
        // The groups of the shaders (`COMMON`): 0 the frame, 1 the shader, 2 the pass.
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let colour = |binding| {
            texture(
                binding,
                wgpu::TextureSampleType::Float { filterable: true },
                wgpu::TextureViewDimension::D2,
            )
        };
        let frame_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post frame"),
            entries: &[
                uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture(
                    1,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                texture(
                    2,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D3,
                ),
                sampler(3, wgpu::SamplerBindingType::NonFiltering),
                sampler(4, wgpu::SamplerBindingType::Filtering),
                sampler(5, wgpu::SamplerBindingType::Filtering),
                sampler(6, wgpu::SamplerBindingType::Filtering),
                sampler(7, wgpu::SamplerBindingType::NonFiltering),
                sampler(8, wgpu::SamplerBindingType::NonFiltering),
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                uniform(10, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let mut entries = vec![uniform(0, wgpu::ShaderStages::FRAGMENT)];
        entries.extend((1..=10).map(colour));
        let shader_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post shader"),
            entries: &entries,
        });
        let mut entries = vec![uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)];
        entries.extend((1..=10).map(colour));
        let pass_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post pass"),
            entries: &entries,
        });
        let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post shaders"),
            bind_group_layouts: &[Some(&frame_layout), Some(&shader_layout), Some(&pass_layout)],
            immediate_size: 0,
        });
        let make_sampler = |filter: wgpu::FilterMode, address: wgpu::AddressMode| {
            device.counted_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
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
        let white = device
            .counted_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("post white"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
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
            .counted_view(&Default::default());
        let black = device
            .counted_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("post black"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
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
            .counted_view(&Default::default());
        // The linear depth.
        let depth_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post depth"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
            ],
        });
        let depth_pipeline = full_screen_pipeline(
            device,
            "post depth",
            &format!("{}{}", nirender_wgpu::depth::wgsl(), DEPTH_SHADER),
            &depth_layout,
            wgpu::TextureFormat::R32Float,
        );
        let blit_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post blit"),
            entries: &[
                texture(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                ),
                sampler(1, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let blit_pipeline = full_screen_pipeline(device, "post blit", BLIT_SHADER, &blit_layout, scene.target_format);
        let debug = 0.0f32;
        let debug_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post debug"),
            entries: &[
                texture(
                    0,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                sampler(1, wgpu::SamplerBindingType::NonFiltering),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let debug_pipeline =
            full_screen_pipeline(device, "post debug", DEBUG_BLIT_SHADER, &debug_layout, scene.target_format);
        let debug_uniform = {
            device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("post debug"),
                contents: bytemuck::cast_slice(&[debug, 0.0, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let no_values = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post no values"),
            size: (MOD_SLOTS * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let no_lights = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("post no lights"),
            size: size_of::<nirender_wgpu::LightData>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
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
            no_lights,
            hdr_reaction,
            reloads: std::collections::HashMap::new(),
            samplers,
            white,
            black,
            depth_pipeline,
            depth_layout,
            blit_pipeline,
            blit_layout,
            debug,
            debug_pipeline,
            debug_layout,
            debug_uniform,
            waves,
            no_sun_glare: false,
            needs_hdr: false,
            hdr_on: true,
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
        println!("post: {} of {} shaders of the chain run", post.chain.len(), chain.len());
        post
    }

    /// Compiles the shader `name` from its WGSL file and puts it at the end of the chain.
    /// Returns false, and tells the log one time, for a name with no file, for a file whose
    /// first lines are not right, and for a shader that does not compile. The caller calls
    /// `order` after it.
    fn load(&mut self, scene: &SceneView, name: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> bool {
        let _long = nirender_wgpu::LongStep::begin();
        if self.chain.iter().any(|c| c.name == name) {
            return true;
        }
        if self.failed.iter().any(|failed| failed == name) {
            return false;
        }
        let device = scene.device;
        let shader = match Definition::of_chain_name(name, read) {
            Ok(definition) => definition,
            Err(skip) => {
                println!("{}", skip.line);
                self.status.note(name, Some(skip.note));
                self.failed.push(name.to_string());
                return false;
            }
        };
        let functions = variables_wgsl(&shader.variables);
        let source = format!("{COMMON}\n{functions}\n{}", shader.source);
        // An error of the WGSL of a file, or a pass that is not a function of it, comes
        // back from the scope and does not stop the program.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&shader.name),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let passes: Vec<CompiledPass> = shader
            .passes
            .iter()
            .map(|(entry, scale, half_float)| {
                let format = if *half_float {
                    wgpu::TextureFormat::Rgba16Float
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                };
                let pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&shader.name),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(entry.as_str()),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                CompiledPass {
                    pipeline,
                    scale: *scale,
                    half_float: *half_float,
                }
            })
            .collect();
        if let Some(error) = pollster::block_on(scope.pop()) {
            // The lines of the message count the common part and the functions of the
            // variables first.
            let before = COMMON.lines().count() + functions.lines().count() + 2;
            println!(
                "post: the shader {} does not compile; skipped (its first line is the line {before} of the message):\n{error}",
                shader.name
            );
            // For the author: the line numbers of the file.
            let error = error.to_string();
            let message = own_lines(compiler_part(&error), before - 1);
            self.status.note(name, Some(message));
            self.failed.push(name.to_string());
            return false;
        }
        self.status.note(name, None);
        self.needs_hdr |= shader.requires_hdr;
        // The textures of the shader. One that is not found reads as black, as on Direct3D 9.
        let textures: Vec<wgpu::TextureView> = shader
            .textures
            .iter()
            .map(|name| {
                let path = format!("textures/{name}");
                match read(&path)
                    .ok_or_else(|| "no file".to_string())
                    .and_then(|bytes| super::image_texture(scene, name, &bytes))
                {
                    Ok(view) => view,
                    Err(problem) => {
                        println!(
                            "post: the texture {name} of the shader {} was not read ({problem}); black",
                            shader.name
                        );
                        self.black.clone()
                    }
                }
            })
            .collect();
        // The variables, with their values before a mod sets them.
        let values = (!shader.variables.is_empty()).then(|| {
            device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("post values"),
                contents: bytemuck::cast_slice(&values_of(&shader.variables, &[])),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        });
        // Group 1 of the shader; it keeps the textures.
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: values.as_ref().unwrap_or(&self.no_values).as_entire_binding(),
        }];
        entries.extend((0..10).map(|i| wgpu::BindGroupEntry {
            binding: i as u32 + 1,
            resource: wgpu::BindingResource::TextureView(textures.get(i).unwrap_or(&self.white)),
        }));
        let group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&shader.name),
            layout: &self.shader_layout,
            entries: &entries,
        });
        self.chain.push(Compiled {
            name: shader.name,
            disable_flags: shader.disable_flags,
            priority: shader.priority,
            own_priority: shader.priority,
            sequence: self.chain.len() + self.failed.len(),
            enabled: true,
            no_sun_glare: shader.no_sun_glare,
            reads_depth: shader.source.contains("depthframe"),
            reads_lights: shader.source.contains("frame_lights"),
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
        let mut last = self
            .chain
            .iter()
            .map(|c| c.own_priority)
            .find(|priority| *priority != NO_CATEGORY)
            .unwrap_or(NO_CATEGORY);
        for shader in self.chain.iter_mut() {
            if shader.own_priority == NO_CATEGORY {
                shader.priority = last;
            } else {
                shader.priority = shader.own_priority;
                last = shader.priority;
            }
        }
        self.chain.sort_by_key(|c| c.priority);
        println!(
            "post: the order: {}",
            self.chain
                .iter()
                .map(|c| if c.enabled {
                    c.name.clone()
                } else {
                    format!("{} (off)", c.name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// Takes the state of a shader that a mod loaded: the shader comes into the chain when
    /// it is not there, and gets its on or off state, its priority and its variables.
    pub fn set_state(&mut self, scene: &SceneView, state: &ShaderState, read: &dyn Fn(&str) -> Option<Vec<u8>>) {
        // A mod reloaded the shader: its WGSL file and its textures are read again, as a
        // reload on Direct3D 9 makes the effect again. A shader that did not compile gets a new try. The shader keeps its place
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
        let Some(shader) = self.chain.iter_mut().find(|c| c.name == state.name) else {
            return;
        };
        if let Some(sequence) = place {
            shader.sequence = sequence;
        }
        let reorder = !known
            || shader.enabled != state.enabled
            || state.priority.is_some_and(|priority| priority != shader.own_priority);
        shader.enabled = state.enabled;
        if let Some(priority) = state.priority {
            shader.own_priority = priority;
        }
        if let Some(buffer) = shader.values.as_ref() {
            scene
                .queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(&values_of(&shader.variables, &state.values)));
        }
        if !self.told_unknown.contains(&state.name) {
            let unknown: Vec<&str> = state
                .values
                .iter()
                .map(|(name, _, _)| name.as_str())
                .filter(|name| !shader.variables.iter().any(|v| v.name == *name))
                .collect();
            if !unknown.is_empty() {
                self.told_unknown.push(state.name.clone());
                println!(
                    "post: {}: a mod sets variables that the shader does not have here: {}",
                    state.name,
                    unknown.join(", ")
                );
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
    }

    /// The term of the debug view `post` (0: off). The uniform is written only when the term
    /// changes; the write lands before the frame's one submit.
    pub fn set_debug(&mut self, scene: &SceneView, term: f32) {
        if term != self.debug {
            self.debug = term;
            scene
                .queue
                .write_buffer(&self.debug_uniform, 0, bytemuck::cast_slice(&[term, 0.0, 0.0, 0.0]));
        }
    }

    /// No shader is on.
    pub fn is_empty(&self) -> bool {
        !self.chain.iter().any(|c| c.enabled)
    }

    /// A shader that is on reads the lights of the frame: the frame asks the renderer for them.
    pub fn reads_lights(&self) -> bool {
        self.chain.iter().any(|c| c.enabled && c.reads_lights)
    }

    /// Runs the chain on the picture and writes the result into `target`. `view` is the state of
    /// the view whose picture it is: everything that a run writes is in it, so that two runs in
    /// one frame (the main view and a picture) do not write the same buffer (the one-submit
    /// rule). `main_point` is the adaptation point of the main view, which a view without a
    /// measure of its own reads. `distant` has the depth terms of the projection of the distant
    /// stage (`nirender_wgpu::depth::terms`), when its depth is a part of the depth frame.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        view: &mut PostView,
        scene: &SceneView,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        variables: &Variables,
        environment_flags: u32,
        distant: Option<[f32; 2]>,
        main_point: [f32; 4],
    ) -> Result<(), nirender_wgpu::pass_report::Skip> {
        let picture = scene.picture.ok_or("no copy of the picture")?;
        let depth = scene.depth.ok_or("no copy of the depth")?;
        if self.is_empty() {
            return Err("the chain has no shader");
        }
        let device = scene.device;
        view.size_textures(device, scene.target_size);
        // The adaptation point of the frame, from the picture before the chain: the main view
        // measures it, a view of its own reads the main view's.
        let mut variables = *variables;
        if !view.owns_hdr {
            if self.needs_hdr {
                variables.hdr = main_point;
            }
        } else if self.needs_hdr {
            let hdr = view
                .hdr
                .get_or_insert_with(|| Hdr::new(device, &self.blit_layout, self.hdr_reaction));
            hdr.reaction = self.hdr_reaction.clamp(0.01, 30.0);
            variables.hdr = if self.hdr_on {
                hdr.frame(
                    device,
                    encoder,
                    picture,
                    &self.blit_layout,
                    &self.samplers[1],
                    environment_flags,
                    variables.sunvec[3],
                )
            } else {
                hdr.point
            };
        }
        let variables = &variables;
        scene.queue.write_buffer(&view.uniforms, 0, bytemuck::bytes_of(variables));
        let eye = [variables.eyepos[0], variables.eyepos[1], variables.eyepos[2]];
        let light_info = FrameLightInfo::of(scene.lights.as_ref(), eye);
        scene.queue.write_buffer(&view.light_info, 0, bytemuck::bytes_of(&light_info));
        let lights = scene
            .lights
            .as_ref()
            .filter(|l| l.count > 0)
            .map_or(&self.no_lights, |l| l.lights);
        // The linear depth of the game.
        // Without a depth of the distant stage, the depth of the game stands in for it:
        // it is 1 where the first read was 1.
        let (distant_depth, distant) = match (scene.before_depth, distant) {
            (Some(view), Some(numbers)) => (view, numbers),
            _ => (depth, [0.0, 0.0]),
        };
        scene.queue.write_buffer(
            &view.depth_uniform,
            0,
            bytemuck::cast_slice(&[nirender_wgpu::depth::terms(&scene.projection), distant].concat()),
        );
        let depth_view = &view.depth.as_ref().unwrap().1.clone();
        // The depth frame only when a shader of the chain of this frame reads it, or the
        // look at it is on.
        let reads_depth = self.debug == 2.0
            || self
                .chain
                .iter()
                .any(|c| c.enabled && c.disable_flags & environment_flags == 0 && c.reads_depth);
        if reads_depth {
            self.depth_frames += 1;
            let stamps = scene.commands.stamps();
            let at = stamps.begin(encoder, "post: the depth frame", nirender_wgpu::stamps::StampKind::Detail);
            let group = view.depth_group.get(
                device,
                &wgpu::BindGroupDescriptor {
                    label: Some("post depth"),
                    layout: &self.depth_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: view.depth_uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(distant_depth),
                        },
                    ],
                },
            );
            let mut pass = full_screen_pass(
                encoder,
                depth_view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                "post: the depth frame",
            );
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            stamps.end(encoder, at);
        }
        // The chain. `last` is the pool texture that holds the input of the shader: `None` is
        // the picture. The passes of a shader each get a texture of their class that is not
        // the input and not an earlier pass of the shader (`pass0` to `pass7` read those).
        let mut last: Option<usize> = None;
        let frame_group = view.frame_group.get(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("post frame"),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.waves),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[3]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[4]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&self.samplers[5]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: lights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: view.light_info.as_entire_binding(),
                    },
                ],
            },
        );
        let chain = std::mem::take(&mut self.chain);
        for compiled in &chain {
            // The GPU time of each shader, with `MGE_XE_GPU_TIME=1`, its passes in the census, and
            // its note in the pass report: one name for each. A shader is noted in a frame whose
            // chain runs, so a frame without the chain shows no shader as unknown.
            let shader_name = nirender_wgpu::stamps::name(&compiled.name);
            nirender_wgpu::pass_report::on_request(&[shader_name]);
            if !compiled.enabled {
                nirender_wgpu::pass_report::note_skipped(shader_name, "the shader is off");
                continue;
            }
            if compiled.disable_flags & environment_flags != 0 {
                nirender_wgpu::pass_report::note_skipped(shader_name, "its flags leave it out in this environment");
                continue;
            }
            nirender_wgpu::pass_report::note_ran(shader_name);
            self.shaders_run += 1;
            let stamps = scene.commands.stamps();
            let at = if stamps.on() {
                stamps.begin(encoder, shader_name, nirender_wgpu::stamps::StampKind::Detail)
            } else {
                nirender_wgpu::stamps::Stamp::NONE
            };
            let mut in_use: Vec<usize> = last.into_iter().collect();
            let mut targets: Vec<usize> = Vec::with_capacity(compiled.passes.len());
            for (number, pass) in compiled.passes.iter().enumerate() {
                let sink = view.pooled(device, pass.scale, pass.half_float, &in_use);
                in_use.push(sink);
                let size = (
                    view.size.0.div_ceil(pass.scale).max(1) as f32,
                    view.size.1.div_ceil(pass.scale).max(1) as f32,
                );
                let info = view.pass_info(device, &compiled.name, number).clone();
                scene
                    .queue
                    .write_buffer(&info, 0, bytemuck::cast_slice(&[size.0, size.1, 1.0 / size.0, 1.0 / size.1]));
                let lastshader_view = match last {
                    Some(index) => &view.pool[index].view,
                    None => picture,
                };
                let lastpass_view = match targets.last() {
                    Some(index) => &view.pool[*index].view,
                    None => lastshader_view,
                };
                let earlier = |i: usize| targets.get(i).map_or(&self.white, |index| &view.pool[*index].view);
                let mut entries = vec![
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: info.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(lastshader_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(lastpass_view),
                    },
                ];
                entries.extend((0..8).map(|i| wgpu::BindGroupEntry {
                    binding: i as u32 + 3,
                    resource: wgpu::BindingResource::TextureView(earlier(i)),
                }));
                // The group of this pass of this shader, by the name and the number, as its info.
                if !view.pass_groups.contains_key(compiled.name.as_str()) {
                    view.pass_groups.insert(compiled.name.clone(), Vec::new());
                }
                let kept = view
                    .pass_groups
                    .get_mut(compiled.name.as_str())
                    .expect("the entry was made above");
                if kept.len() <= number {
                    kept.resize_with(number + 1, KeptGroup::default);
                }
                let pass_group = kept[number].get(
                    device,
                    &wgpu::BindGroupDescriptor {
                        label: Some(&compiled.name),
                        layout: &self.pass_layout,
                        entries: &entries,
                    },
                );
                // A pass keeps what the texture held where it discards a pixel.
                let mut render = full_screen_pass(encoder, &view.pool[sink].view, wgpu::LoadOp::Load, shader_name);
                render.set_pipeline(&pass.pipeline);
                render.set_bind_group(0, &frame_group, &[]);
                render.set_bind_group(1, &compiled.group, &[]);
                render.set_bind_group(2, &pass_group, &[]);
                render.draw(0..3, 0..1);
                drop(render);
                targets.push(sink);
            }
            stamps.end(encoder, at);
            // The output of the last pass is the input of the next shader.
            last = targets.last().copied();
        }
        self.chain = chain;
        // A look at a term, for a test.
        if self.debug > 0.0 {
            let source = if self.debug == 2.0 {
                depth_view
            } else {
                &view.pool[last.unwrap_or(0)].view
            };
            let group = nirender_wgpu::gpu::frame_group(
                device,
                &wgpu::BindGroupDescriptor {
                    label: Some("post debug"),
                    layout: &self.debug_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.samplers[0]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.debug_uniform.as_entire_binding(),
                        },
                    ],
                },
            );
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load, "post: the debug view");
            pass.set_pipeline(&self.debug_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            return Ok(());
        }
        // The result into the picture of the frame.
        if let Some(index) = last {
            let group = view.blit_group.get(
                device,
                &wgpu::BindGroupDescriptor {
                    label: Some("post blit"),
                    layout: &self.blit_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view.pool[index].view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.samplers[0]),
                        },
                    ],
                },
            );
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load, "post: the result into the picture");
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(())
    }
}

/// A texture that a pass draws into and a shader reads.
fn make_texture(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
    device.counted_texture(&wgpu::TextureDescriptor {
        label: Some("post"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn texture(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler(binding: u32, kind: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(kind),
        count: None,
    }
}

pub(super) fn full_screen_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let _long = nirender_wgpu::LongStep::begin();
    let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A pass that covers `target`. `name`: the pass's name in the census of the passes; it is a
/// pass inside a part that notes itself (the post chain, the menu's kept picture).
pub(super) fn full_screen_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    name: &'static str,
) -> wgpu::RenderPass<'a> {
    nirender_wgpu::gpu::note_inner_pass(name);
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("post"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[cfg(test)]
mod shader_tests {
    use super::*;

    /// The shaders of the chain itself compile as the device checks them: the depth frame,
    /// the blit of a picture, and the debug tint.
    #[test]
    fn the_shaders_of_the_chain_are_valid() {
        let check = crate::renderer::shader_check::check;
        check("post depth", &format!("{}{DEPTH_SHADER}", nirender_wgpu::depth::wgsl())).unwrap();
        check("post blit", BLIT_SHADER).unwrap();
        check("post debug", DEBUG_BLIT_SHADER).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New values of the variables are no change of the layout; a new variable, another number
    /// of values, the switch and a reload are.
    #[test]
    fn a_shader_state_with_new_values_only_has_the_same_layout() {
        let flames = |time: Vec<f32>| ShaderState {
            name: "flames".into(),
            enabled: true,
            priority: None,
            reloads: 0,
            values: vec![("time".into(), b'f', time)],
        };
        let state = flames(vec![1.0]);
        assert!(state.same_but_values(&flames(vec![2.0])));
        assert!(!state.same_but_values(&flames(vec![2.0, 3.0])));
        let mut more = flames(vec![1.0]);
        more.values.push(("glow".into(), b'f', vec![0.5]));
        assert!(!state.same_but_values(&more));
        assert!(!state.same_but_values(&ShaderState {
            enabled: false,
            ..flames(vec![1.0])
        }));
        assert!(!state.same_but_values(&ShaderState {
            reloads: 1,
            ..flames(vec![1.0])
        }));
        assert!(!state.same_but_values(&ShaderState {
            name: "other".into(),
            ..flames(vec![1.0])
        }));
    }

    /// The step of the point is the advance of the clock of the game: 0 for the first call, for a
    /// clock that stands and for a clock that goes back (the next step counts from the new time),
    /// and at most HDR_MOST_STEP for a jump.
    #[test]
    fn the_hdr_step_follows_the_clock_of_the_game() {
        let mut clock = HdrClock::default();
        assert_eq!(clock.step(100.0), 0.0, "the first call");
        assert_eq!(clock.step(100.016), 100.016 - 100.0, "a frame");
        assert_eq!(clock.step(100.016), 0.0, "a clock that stands");
        assert_eq!(clock.step(100.05), 100.05 - 100.016, "the frame after a pause");
        assert_eq!(clock.step(40.0), 0.0, "a clock that goes back");
        assert_eq!(clock.step(40.02), 40.02 - 40.0, "counts from the new time");
        assert_eq!(clock.step(3640.0), HDR_MOST_STEP, "a jump");
        assert_eq!(clock.step(f32::NAN), 0.0, "a clock that is not a number");
        assert_eq!(clock.step(41.0), 0.0, "after the NaN there is no earlier time to count from");
    }

    /// The cycle of the measure is that of MGE XE: copy, map, wait, read, and again, at the same
    /// calls whatever the progress of the GPU.
    #[test]
    fn the_hdr_cycle_reads_at_the_third_call_after_the_copy() {
        let mut cycle = HdrCycle::default();
        let actions: Vec<HdrAction> = (0..9).map(|_| cycle.step()).collect();
        use HdrAction::*;
        assert_eq!(actions, vec![Copy, Map, Wait, Read, Copy, Map, Wait, Read, Copy]);
    }

    /// A read whose wait ended before the copy comes again at the next call, then the cycle goes on.
    #[test]
    fn a_read_that_timed_out_comes_again() {
        let mut cycle = HdrCycle::default();
        let mut actions: Vec<HdrAction> = (0..4).map(|_| cycle.step()).collect();
        cycle.repeat();
        actions.extend((0..3).map(|_| cycle.step()));
        use HdrAction::*;
        assert_eq!(actions, vec![Copy, Map, Wait, Read, Read, Copy, Map]);
    }

    /// The wait of the read names the submit of the copy and has a time limit.
    #[test]
    fn the_hdr_wait_has_a_limit_and_names_a_submit() {
        match hdr_wait(None) {
            wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(limit),
            } => assert_eq!(limit, HDR_MOST_WAIT),
            _ => panic!("a wait with a time limit"),
        }
    }

    /// What a run of the measure on a real device gave.
    struct HdrRun {
        /// The calls of `Hdr::frame` until the luminance was read.
        calls: u32,
        /// The longest call.
        longest: std::time::Duration,
        luminance: f32,
    }

    /// Runs the measure on a real device, on a white picture, while the GPU works for about
    /// `busy` before the copy and for about `busy_after_map` more from the call after the map. Nothing
    /// else polls the device. Stops at the first read, or at 40 calls.
    fn hdr_run_with_busy_gpu(busy: std::time::Duration, busy_after_map: std::time::Duration) -> HdrRun {
        // One GPU test at a time in the crate: `gpu::last_submission` is shared by the process, and
        // the busy GPU must be this test's only (W629).
        let _one_at_a_time = crate::renderer::one_gpu_test_at_a_time();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let blit_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("test blit"),
            entries: &[
                texture(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D2,
                ),
                sampler(1, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let linear = device.counted_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let picture = device.counted_texture(&wgpu::TextureDescriptor {
            label: Some("test picture"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = picture.counted_view(&wgpu::TextureViewDescriptor::default());

        // Work for the GPU: copies of 128 MB, as many as take about `busy`. One timed run of 100 copies
        // gives the time of a copy.
        let size = 128 << 20;
        let usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let from = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("test busy from"),
            size,
            usage,
            mapped_at_creation: false,
        });
        let to = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("test busy to"),
            size,
            usage,
            mapped_at_creation: false,
        });
        let work = |copies: u32| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for _ in 0..copies {
                encoder.copy_buffer_to_buffer(&from, 0, &to, 0, size);
            }
            nirender_wgpu::gpu::submit(&queue, [encoder.finish()])
        };
        let timed = std::time::Instant::now();
        let index = work(100);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        });
        let per_copy = timed.elapsed().as_secs_f64() / 100.0;
        let copies = (busy.as_secs_f64() / per_copy).ceil().max(1.0) as u32;
        work(copies);
        let copies_after_map = (busy_after_map.as_secs_f64() / per_copy).ceil() as u32;

        let mut hdr = Hdr::new(&device, &blit_layout, 1.0);
        assert_eq!(hdr.luminance, 0.5, "the luminance before the first read");
        let mut run = HdrRun {
            calls: 0,
            longest: std::time::Duration::ZERO,
            luminance: 0.5,
        };
        while hdr.luminance == 0.5 && run.calls < 40 {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            drop(full_screen_pass(
                &mut encoder,
                &view,
                wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                "test",
            ));
            let called = std::time::Instant::now();
            hdr.frame(
                &device,
                &mut encoder,
                &view,
                &blit_layout,
                &linear,
                0,
                run.calls as f32 * 0.016,
            );
            run.longest = run.longest.max(called.elapsed());
            nirender_wgpu::gpu::submit(&queue, [encoder.finish()]);
            run.calls += 1;
            // The map is asked for at the second call: the work after it comes after the copy.
            if run.calls == 2 && copies_after_map > 0 {
                work(copies_after_map);
            }
        }
        run.luminance = hdr.luminance;
        println!(
            "hdr read: busy {busy:?} before the copy and {busy_after_map:?} after the map ({copies} + {copies_after_map} copies, {per_copy:.6} s each): {} calls, longest call {:?}, luminance {}",
            run.calls, run.longest, run.luminance
        );
        run
    }

    /// The GPU is busy for about 100 ms before the copy, less than the limit: the read at the
    /// fourth call waits for the copy and gets its pixel (nothing else polls the device). Needs a
    /// GPU: `cargo test -p mgeHost64 --release -- --ignored --nocapture the_hdr_read`.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_hdr_read_waits_for_the_copy_while_the_gpu_is_busy() {
        let run = hdr_run_with_busy_gpu(std::time::Duration::from_millis(100), std::time::Duration::ZERO);
        assert_eq!(run.calls, 4, "the read comes at the fourth call");
        assert!(run.luminance > 0.99, "the read gets the white of the copy: {}", run.luminance);
    }

    /// The GPU works for about 150 ms before the copy and for 800 ms more after the call of the map.
    /// The read waits for the submit that holds the copy, not for the work after it: the read comes
    /// at the fourth call and no call waits long (a read that waits for the newest submit waits to the
    /// limit). This tells a wait for the copy's submit from a wait for the newest submit, which the
    /// tests with work before the copy only cannot. Reviewer's probe, bound 200 ms.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_hdr_read_does_not_wait_for_work_sent_after_the_map() {
        let run = hdr_run_with_busy_gpu(std::time::Duration::from_millis(150), std::time::Duration::from_millis(800));
        assert_eq!(run.calls, 4, "the read comes at the fourth call");
        assert!(
            run.longest < std::time::Duration::from_millis(200),
            "no call waits for the work after the map: {:?}",
            run.longest
        );
        assert!(run.luminance > 0.99, "the read gets the white of the copy: {}", run.luminance);
    }

    /// The GPU is busy for about 800 ms, more than the limit: the first read gives up, the
    /// map stays, and a later call reads the same pixel.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_read_that_waits_past_the_limit_comes_again_and_gets_the_pixel() {
        let run = hdr_run_with_busy_gpu(std::time::Duration::from_millis(800), std::time::Duration::ZERO);
        assert!(run.calls > 4, "the read gave up at least once: {} calls", run.calls);
        assert!(
            run.longest < HDR_MOST_WAIT * 2,
            "no call waits much longer than the limit: {:?}",
            run.longest
        );
        assert!(
            run.luminance > 0.99,
            "the later read gets the white of the copy: {}",
            run.luminance
        );
    }

    #[test]
    fn the_status_file_has_the_result_of_each_shader() {
        let mut status = ShaderStatus::default();
        status.note("a", None);
        status.note("b c", Some("first line\nsecond line".to_string()));
        assert_eq!(
            status.text(),
            "sequence 2\nshader a\nloads 1\nok\nshader b c\nloads 1\nerror\n| first line\n| second line\n"
        );
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
        let wrapped =
            format!("Validation Error\n\nCaused by:\n  In Device::create_shader_module, label = 'x'\n    \n{message}");
        assert!(compiler_part(&wrapped).starts_with("Shader 'x' parsing error"));
        assert_eq!(compiler_part("no compiler part"), "no compiler part");
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
        let bytes = record(
            "oj_me_fog_box",
            3,
            -5,
            &[
                ("fogDensities", b'F', &[0.5, 0.25]),
                ("fogCenters", b'F', &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            ],
        );
        let state = ShaderState::read(&bytes).unwrap();
        assert_eq!(state.name, "oj_me_fog_box");
        assert!(state.enabled);
        assert_eq!(state.priority, Some(-5));
        assert_eq!(state.values.len(), 2);
        assert_eq!(
            state.values[1],
            ("fogCenters".to_string(), b'F', vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        );
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
            VariableDef {
                name: "fogCenters".to_string(),
                slots: 2,
                per: 3,
                default: vec![],
            },
            VariableDef {
                name: "fogDensities".to_string(),
                slots: 2,
                per: 1,
                default: vec![7.0, 8.0],
            },
            VariableDef {
                name: "tints".to_string(),
                slots: 2,
                per: 3,
                default: vec![],
            },
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

    /// The `Data Files` of our package: `assets/Data Files` of the repo.
    fn package_folder() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("assets")
            .join("Data Files")
    }

    /// The `.wgsl` files of the `shaders/XEshaders` of a pack or of our package, with their names
    /// as a chain gives them: a file in a folder of its own (`InspectIt/Bokeh.wgsl`) has the folder
    /// in its name.
    fn post_files(root: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
        let mut found = Vec::new();
        let mut folders = vec![(String::new(), root.join("shaders").join("XEshaders"))];
        while let Some((prefix, folder)) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    folders.push((format!("{prefix}{name}/"), path));
                } else if path.extension().is_some_and(|extension| extension == "wgsl") {
                    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                    found.push((format!("{prefix}{stem}"), path));
                }
            }
        }
        found
    }

    /// The weather functions (W622) compile in a file that calls each of them: the table of
    /// `weather_mix` is indexed by values that are not constants.
    #[test]
    fn a_file_that_calls_the_weather_functions_compiles() {
        let text = "//! interface: mge-xe-wgsl 1\n//! passes: main\n\n\
            @fragment fn main(in: Quad) -> @location(0) vec4f {\n\
            \x20   var known = 0.0;\n\
            \x20   if weather_known() {\n\
            \x20       known = 1.0;\n\
            \x20   }\n\
            \x20   let index = f32(weather_now() + weather_next());\n\
            \x20   let air = weather_mix(array<f32, 10>(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0));\n\
            \x20   return vec4f(known, index, weather_ratio(), air);\n\
            }\n";
        let definition = Definition::of_file("weather", text).expect("the header");
        let source = format!("{COMMON}\n{}\n{}", variables_wgsl(&definition.variables), definition.source);
        crate::renderer::shader_check::check("weather", &source).expect("the file compiles");
    }

    /// The problems of post shader files: each must have a header that the renderer takes, and
    /// WGSL that is valid with the common part and the functions of its variables, with each pass
    /// a fragment function of it.
    fn file_problems(files: &[(String, std::path::PathBuf)]) -> Vec<String> {
        use wgpu::naga;
        let mut problems = Vec::new();
        for (name, path) in files {
            let text = std::fs::read_to_string(path).unwrap();
            let definition = match Definition::of_file(name, &text) {
                Ok(definition) => definition,
                Err(problem) => {
                    problems.push(format!("{}: {problem}", path.display()));
                    continue;
                }
            };
            let source = format!("{COMMON}\n{}\n{}", variables_wgsl(&definition.variables), definition.source);
            let module = match crate::renderer::shader_check::check(name, &source) {
                Ok(module) => module,
                Err(problem) => {
                    problems.push(problem);
                    continue;
                }
            };
            for (entry, _, _) in &definition.passes {
                if !module
                    .entry_points
                    .iter()
                    .any(|point| point.name == *entry && point.stage == naga::ShaderStage::Fragment)
                {
                    problems.push(format!("{name}: the pass {entry} is not a fragment function"));
                }
            }
        }
        problems
    }

    /// The number of post shader files in the packs of `ports/` at W609 step 5 (with the files in
    /// folders of `XEshaders`). A walk that finds fewer has lost files.
    const PACK_FILES_AT_LEAST: usize = 55;

    /// The number of post shader files of our package at W609 step 5: the five stock shaders of
    /// step 3, the three S_A shaders, and the four stock shaders of step 5.
    const STOCK_FILES_AT_LEAST: usize = 12;

    /// Each post shader file of the packs in `ports/` (`<pack>/shaders/XEshaders/*.wgsl`, also in
    /// a folder of `XEshaders`) is valid.
    #[test]
    fn each_post_file_of_the_packs_is_valid() {
        let ports = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ports");
        let files: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&ports)
            .expect("the folder ports")
            .flat_map(|pack| post_files(&pack.expect("a pack").path()))
            .collect();
        assert!(
            files.len() >= PACK_FILES_AT_LEAST,
            "{} post shader files in {}, and at least {PACK_FILES_AT_LEAST} were there",
            files.len(),
            ports.display()
        );
        let problems = file_problems(&files);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Each post shader file of our package (`assets/Data Files/shaders/XEshaders/*.wgsl`) is
    /// valid: the renderer has no other copy of the stock shaders.
    #[test]
    fn each_stock_file_is_valid() {
        let files = post_files(&package_folder());
        assert!(
            files.len() >= STOCK_FILES_AT_LEAST,
            "{} post shader files in our package, and at least {STOCK_FILES_AT_LEAST} ship",
            files.len()
        );
        for name in [
            "Sunshafts",
            "Underwater Effects",
            "SSAO HQ",
            "Eye Adaptation (HDR)",
            "Bloom Soft",
        ] {
            assert!(
                files.iter().any(|(file, _)| file == name),
                "no file {name}.wgsl in our package"
            );
        }
        let problems = file_problems(&files);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// The null of the two tests above: a file with a pass that is not a function of it, and a
    /// file with no header, are problems.
    #[test]
    fn a_bad_post_file_is_a_problem() {
        let folder = std::env::temp_dir().join(format!("w609-post-files-{}", std::process::id()));
        let shaders = folder.join("shaders").join("XEshaders");
        std::fs::create_dir_all(shaders.join("Sub")).unwrap();
        let good = "//! interface: mge-xe-wgsl 1\n//! passes: tint\n\n@fragment fn tint(in: Quad) -> @location(0) vec4f { return vec4f(1.0); }\n";
        std::fs::write(shaders.join("good.wgsl"), good).unwrap();
        std::fs::write(
            shaders.join("Sub").join("bad.wgsl"),
            good.replace("passes: tint", "passes: other"),
        )
        .unwrap();
        std::fs::write(shaders.join("none.wgsl"), "fn f() {}\n").unwrap();
        let mut files = post_files(&folder);
        files.sort();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        let problems = file_problems(&files);
        for file in ["good.wgsl", "none.wgsl", "Sub/bad.wgsl"] {
            std::fs::remove_file(shaders.join(file)).unwrap();
        }
        for empty in [shaders.join("Sub"), shaders.clone(), folder.join("shaders"), folder.clone()] {
            std::fs::remove_dir(empty).unwrap();
        }
        assert_eq!(names, ["Sub/bad", "good", "none"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("Sub/bad: the pass other is not a fragment function"),
            "{problems:?}"
        );
        assert!(problems[1].contains("none.wgsl"), "{problems:?}");
    }

    /// W609 step 5: a chain name with no `.wgsl` file is skipped with its line and its note, also
    /// when an effect file of that name is there. The null: with the file, the name gives its
    /// shader; a file whose first lines are not right is not used.
    #[test]
    fn a_name_without_a_wgsl_file_is_skipped() {
        let only_fx = |path: &str| (path == "shaders/XEshaders/Sunshafts.fx").then(|| b"technique T0 {}\n".to_vec());
        assert_eq!(
            Definition::of_chain_name("Sunshafts", &only_fx).unwrap_err(),
            Skip {
                line: "post: no file Sunshafts.wgsl; skipped".to_string(),
                note: "The wgpu renderer has no file Sunshafts.wgsl.".to_string(),
            }
        );
        let nothing = |_: &str| None::<Vec<u8>>;
        assert_eq!(
            Definition::of_chain_name("x", &nothing).unwrap_err().line,
            "post: no file x.wgsl; skipped"
        );
        let text = "//! interface: mge-xe-wgsl 1\n//! passes: tint\n\n@fragment fn tint(in: Quad) -> @location(0) vec4f { return vec4f(1.0); }\n";
        let with_file = |path: &str| (path == "shaders/XEshaders/tint.wgsl").then(|| text.as_bytes().to_vec());
        let definition = Definition::of_chain_name("tint", &with_file).unwrap();
        assert_eq!(definition.name, "tint");
        assert_eq!(definition.passes, vec![("tint".to_string(), 1, false)]);
        let bad = |_: &str| Some(b"//! passes tint\n".to_vec());
        let skip = Definition::of_chain_name("tint", &bad).unwrap_err();
        assert!(
            skip.line.starts_with("post: the shader file tint.wgsl is not used: "),
            "{}",
            skip.line
        );
        assert!(
            skip.note
                .starts_with("The first lines of tint.wgsl (the lines with //!) are not right: "),
            "{}",
            skip.note
        );
    }
}

/// The tests of the pack file of Eternal Shine (`ports/Eternal Shine - wgpu shaders`), which has no
/// port: its glow terms run on the GPU against a sum along the ray, and its header has the settings of
/// the mod. Moved from post_look.rs when the ports went (W609 step 5, verdict 734 4.1).
#[cfg(test)]
mod pack_tests {
    use super::*;

    /// The file of the pack "Eternal Shine - wgpu shaders".
    const ETERNAL_SHINE_FILE: &str =
        include_str!("../../../ports/Eternal Shine - wgpu shaders/shaders/XEshaders/Eternal Shine.wgsl");

    /// The glow terms of the file: from its first function to the function `glow_air`. They
    /// read nothing of the frame, so a test runs them alone.
    fn glow_terms() -> &'static str {
        let start = ETERNAL_SHINE_FILE.find("fn glow_density(").expect("glow_density in the file");
        let end = ETERNAL_SHINE_FILE.find("fn glow_air(").expect("glow_air in the file");
        &ETERNAL_SHINE_FILE[start..end]
    }

    /// Runs `cases`, WGSL with `fn case_of(i: u32) -> vec4f` after the glow terms, for `count`
    /// cases (at most 16) on the GPU, and gives the results.
    fn glow_cases(cases: &str, count: u32) -> Vec<[f32; 4]> {
        let _one_at_a_time = crate::renderer::one_gpu_test_at_a_time();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let target = device.counted_texture(&wgpu::TextureDescriptor {
            label: Some("test glow"),
            size: wgpu::Extent3d {
                width: count,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let source = format!(
            "{}{cases}
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {{
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) position: vec4f) -> @location(0) vec4f {{
    return case_of(u32(position.x));
}}
",
            glow_terms()
        );
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test glow"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test glow"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba32Float.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let readback = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("test glow"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let target_view = target.counted_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test glow"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Default::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        queue.submit([encoder.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("the map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("the poll");
        let bytes = readback.slice(..).get_mapped_range().expect("the mapped range");
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        floats
            .chunks(4)
            .take(count as usize)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }

    /// The density by a sum of small steps along the ray, in the units of the sphere.
    fn density_by_steps(o: [f32; 3], d: [f32; 3], far: f32) -> f32 {
        const STEPS: usize = 200_000;
        let (o, d, far) = (o.map(f64::from), d.map(f64::from), f64::from(far));
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let end = far.min(10.0);
        let step = end / STEPS as f64;
        let mut sum = 0.0;
        for i in 0..STEPS {
            let t = (i as f64 + 0.5) * step;
            let p: Vec<f64> = (0..3).map(|a| o[a] + t * d[a] / length).collect();
            sum += (1.0 - (p[0] * p[0] + p[1] * p[1] + p[2] * p[2])).max(0.0) * step;
        }
        (sum * 0.75) as f32
    }

    /// The density of the shader is the sum along the ray: through the centre, off the centre,
    /// from inside the sphere, cut by a surface, behind the eye, and past the sphere.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_density_is_the_sum_along_the_ray() {
        let cases: [([f32; 3], [f32; 3], f32); 6] = [
            ([0.0, -3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
            ([0.4, -3.0, 0.2], [0.0, 1.0, 0.0], 100.0),
            ([0.1, 0.2, -0.3], [0.6, 0.0, 0.8], 100.0),
            ([0.0, -3.0, 0.0], [0.0, 1.0, 0.0], 2.7),
            ([0.0, 3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
            ([1.5, -3.0, 0.0], [0.0, 1.0, 0.0], 100.0),
        ];
        let list = cases
            .iter()
            .map(|(o, d, far)| {
                format!(
                    "glow_density(vec3f({:?}, {:?}, {:?}), vec3f({:?}, {:?}, {:?}), {far:?})",
                    o[0], o[1], o[2], d[0], d[1], d[2]
                )
            })
            .collect::<Vec<_>>();
        let body = list
            .iter()
            .enumerate()
            .map(|(i, call)| format!("    if i == {i}u {{ return vec4f({call}); }}\n"))
            .collect::<String>();
        let results = glow_cases(
            &format!("fn case_of(i: u32) -> vec4f {{\n{body}    return vec4f(0.0);\n}}\n"),
            6,
        );
        for ((o, d, far), result) in cases.iter().zip(&results) {
            let want = density_by_steps(*o, *d, *far);
            assert!(
                (want - result[0]).abs() < 1e-3,
                "the ray from {o:?} along {d:?} to {far}: the shader gave {}, the sum {want}",
                result[0]
            );
        }
        // The ray through the centre gathers 1, and the ray past the sphere none.
        assert!((results[0][0] - 1.0).abs() < 1e-4);
        assert_eq!(results[5][0], 0.0);
    }

    /// A light of radius 1000 reaches 1000 units, and its glow sphere is about 3256: a ray that
    /// passes 2000 units from it, out of its reach, gets the glow. A ray past the sphere, and a
    /// pixel whose surface is before the sphere, get none.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_pixel_in_the_glow_and_out_of_the_reach_gets_the_glow() {
        let cases = "
fn case_of(i: u32) -> vec4f {
    let colour = vec3f(1.0, 0.8, 0.5);
    let centre = vec3f(2000.0, 6000.0, 0.0);
    let dir = vec3f(0.0, 1.0, 0.0);
    if i == 0u {
        return vec4f(glow_light(colour, 1000.0, 0.3, centre, dir, 1.0e7, 1.0), glow_sphere(glow_word(1000.0, 0.3)));
    }
    if i == 1u {
        return vec4f(glow_light(colour, 1000.0, 0.3, vec3f(4000.0, 6000.0, 0.0), dir, 1.0e7, 1.0), 0.0);
    }
    return vec4f(glow_light(colour, 1000.0, 0.3, centre, dir, 500.0, 1.0), 0.0);
}
";
        let results = glow_cases(cases, 3);
        let sphere = results[0][3];
        assert!(
            (sphere - 3256.0).abs() < 5.0,
            "the sphere of a light of radius 1000: {sphere}"
        );
        assert!(
            results[0][0] > 0.0 && results[0][1] > 0.0,
            "the glow out of the reach: {:?}",
            results[0]
        );
        assert_eq!(results[1][..3], [0.0; 3], "a ray past the sphere");
        assert_eq!(results[2][..3], [0.0; 3], "a surface before the sphere");
    }

    /// The file reads the lights of the frame (the chain then asks for them) and has the glow
    /// terms of the tests.
    #[test]
    fn the_glow_file_reads_the_lights_and_has_the_terms() {
        assert!(ETERNAL_SHINE_FILE.contains("frame_lights"));
        for term in ["glow_density", "glow_word", "glow_sphere", "glow_light"] {
            assert!(glow_terms().contains(&format!("fn {term}(")), "{term}");
        }
    }

    /// The variables of the file are its settings, with the defaults of the mod's Lua
    /// (`mwse.loadConfig` of its main.lua) and the owner's 100 units of `near_eye`, and the file
    /// reads each one.
    #[test]
    fn the_glow_values_are_the_settings_of_the_file() {
        let header = mge_config::post_header::parse(ETERNAL_SHINE_FILE).expect("the header");
        let have: Vec<(&str, f32)> = header
            .settings
            .iter()
            .map(String::as_str)
            .zip(header.setting_defaults())
            .collect();
        let want = [
            ("radmult", 0.3),
            ("int", 0.25),
            ("ext", 0.4),
            ("rain", 0.5),
            ("near_eye", 100.0),
            ("fog", 0.75),
            ("storm", 0.75),
        ];
        assert_eq!(have, want);
        assert_eq!(header.variables.len(), header.settings.len());
        for (name, _) in want {
            assert!(ETERNAL_SHINE_FILE.contains(&format!("{name}()")), "the file reads {name}");
        }
    }
}
