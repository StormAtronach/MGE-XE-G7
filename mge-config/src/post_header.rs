//! The `//!` lines at the start of a post shader file of the wgpu renderer
//! (`Data Files/shaders/XEshaders/<name>.wgsl`): the one parser of them. The helper reads the
//! file with it to compile the shader, and d3d8.dll reads it to list the variables and the
//! settings of the shader to the API of MGE XE. One parser, so the two cannot differ.
//!
//! The keys (mgeHost64's POST_SHADERS.md, "The file"):
//!
//! - `interface: mge-xe-wgsl 1` (needed)
//! - `passes: a, b /2 f16, c` (needed): the fragment functions, one for each pass, in their
//!   order; `/N` draws the pass into a target of the picture size over N, `f16` into 16-bit
//!   floats (8-bit colours without it)
//! - `category: scene | atmosphere | lens | sensor | tone | none | final` (`none`)
//! - `priority_adjust: N` (0)
//! - `mgeflags: N` (0): the environment flags that stop the shader
//! - `requires_hdr: true | false` (false)
//! - `disable_sunglare: true | false` (false)
//! - `texture: path, path` (none): at most [`TEXTURES`] files under `Data Files/textures`
//! - `variables: a f32 = 1, b vec3, c f32[8]` (none): the variables that a mod sets from Lua,
//!   with their values before that
//! - `settings: a, d` (none): the variables that a player adjusts and that MGE XE keeps
//!   between game sessions, in the values file of the shader (mge-config's `post_values`)
//!
//! The header ends at the first line that does not start with `//!`.

/// The most textures that a shader file can name.
pub const TEXTURES: usize = 10;

/// The text of the `interface` line that the renderer reads.
pub const INTERFACE: &str = "mge-xe-wgsl 1";

/// A pass of the shader: a fragment function and its target.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    /// The fragment function.
    pub entry: String,
    /// The target has the picture size over this number (1 or more).
    pub scale: u32,
    /// The target has 16-bit floats; else 8-bit colours.
    pub half_float: bool,
}

/// A variable that a mod sets from Lua.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    /// The number of elements: 1, or N of `type[N]`.
    pub slots: u32,
    /// The values of each element: 1 for f32, 2 to 4 for vec2 to vec4.
    pub per: u32,
    /// The values before a mod sets the variable, `per` for each element; fewer are 0.
    pub default: Vec<f32>,
    /// The type has `[N]`, also `[1]`.
    pub array: bool,
}

impl Variable {
    /// The variable as the API of MGE XE lists it (`MGEAPI::shaderGetVariableInfo`): its type
    /// letter and its values. The letters are those of an effect file: `f` for an f32, `2` to
    /// `4` for a vector, `F` for an array of f32, and `V` for an array of vectors, with 4
    /// values for each vector. The values are the defaults, 0 where the line has none.
    pub fn api_shape(&self) -> (char, Vec<f32>) {
        let elements = self.slots as usize;
        let per = self.per as usize;
        let (letter, stride) = match (self.array, per) {
            (false, 1) => ('f', 1),
            (false, _) => (char::from(b'0' + per as u8), per),
            (true, 1) => ('F', 1),
            (true, _) => ('V', 4),
        };
        let mut values = vec![0.0; elements * stride];
        for (n, value) in self.default.iter().enumerate() {
            let (element, part) = (n / per, n % per);
            if element < elements {
                values[element * stride + part] = *value;
            }
        }
        (letter, values)
    }
}

/// The values of the `//!` lines of a shader file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub passes: Vec<Pass>,
    /// The category, empty for `none` and for a file without the key.
    pub category: String,
    pub priority_adjust: i64,
    /// The value of `mgeflags`, `None` for a file without the key.
    pub mgeflags: Option<u32>,
    pub requires_hdr: bool,
    pub disable_sunglare: bool,
    pub textures: Vec<String>,
    pub variables: Vec<Variable>,
    /// The names of the settings, in the order of the line; each is a variable of one f32.
    pub settings: Vec<String>,
}

impl Header {
    /// The place of the shader in the order of the chain: the category (scene 1M, atmosphere
    /// 2M, lens 3M, sensor 4M, tone 5M, none 6M, final 9M) plus `priority_adjust`.
    pub fn priority(&self) -> i64 {
        category_priority(&self.category) + self.priority_adjust
    }

    /// The variable `name`.
    pub fn variable(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|variable| variable.name == name)
    }

    /// The variable is a setting.
    pub fn is_setting(&self, name: &str) -> bool {
        self.settings.iter().any(|setting| setting == name)
    }

    /// The default of each setting, in the order of the settings.
    pub fn setting_defaults(&self) -> Vec<f32> {
        self.settings
            .iter()
            .map(|name| {
                self.variable(name)
                    .map_or(0.0, |variable| variable.default.first().copied().unwrap_or(0.0))
            })
            .collect()
    }
}

/// The priority of a category; an empty or unknown category is `none`.
fn category_priority(category: &str) -> i64 {
    match category {
        "scene" => 1_000_000,
        "atmosphere" => 2_000_000,
        "lens" => 3_000_000,
        "sensor" => 4_000_000,
        "tone" => 5_000_000,
        "final" => 9_000_000,
        _ => 6_000_000,
    }
}

/// Reads the `//!` lines at the start of `text`. An error names the first line or value that
/// the renderer does not accept; then neither the helper nor d3d8.dll uses the file.
pub fn parse(text: &str) -> Result<Header, String> {
    let mut header = Header::default();
    let mut interface = false;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("//!") else { break };
        let Some((key, value)) = rest.split_once(':') else {
            return Err(format!("the line `{line}` has no `key: value`"));
        };
        let value = value.trim();
        match key.trim() {
            "interface" => interface = value == INTERFACE,
            "passes" => header.passes = items(value).map(parse_pass).collect::<Result<_, _>>()?,
            "category" => {
                if !matches!(value, "scene" | "atmosphere" | "lens" | "sensor" | "tone" | "none" | "final") {
                    return Err(format!("the category `{value}` is not known"));
                }
                header.category = if value == "none" { String::new() } else { value.to_string() };
            }
            "priority_adjust" => {
                header.priority_adjust = value
                    .parse()
                    .map_err(|_| format!("the priority adjust `{value}` is not a number"))?
            }
            "mgeflags" => {
                header.mgeflags = Some(value.parse().map_err(|_| format!("the flags `{value}` are not a number"))?)
            }
            "requires_hdr" => header.requires_hdr = flag(value)?,
            "disable_sunglare" => header.disable_sunglare = flag(value)?,
            "texture" => {
                header.textures = items(value).map(str::to_string).collect();
                if header.textures.len() > TEXTURES {
                    return Err(format!("more than {TEXTURES} textures"));
                }
            }
            "variables" => {
                header.variables = Vec::new();
                for variable in items(value).map(parse_variable) {
                    let variable = variable?;
                    if header.variable(&variable.name).is_some() {
                        return Err(format!("the variable {} is named twice", variable.name));
                    }
                    header.variables.push(variable);
                }
            }
            "settings" => header.settings = items(value).map(str::to_string).collect(),
            other => return Err(format!("the key `{other}` is not known")),
        }
    }
    if !interface {
        return Err(format!("no line `//! interface: {INTERFACE}` at its start"));
    }
    if header.passes.is_empty() {
        return Err("no line `//! passes: ...` with the fragment functions".to_string());
    }
    check_settings(&header)?;
    Ok(header)
}

/// The items of a list with commas, trimmed, without the empty ones.
fn items(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|item| !item.is_empty())
}

/// `true` or `false`.
fn flag(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("`{other}` is not true or false")),
    }
}

/// A pass: `name`, then `/N` and `f16` in any order.
fn parse_pass(item: &str) -> Result<Pass, String> {
    let mut words = item.split_whitespace();
    let Some(name) = words.next() else {
        return Err("a pass without a name".to_string());
    };
    let mut pass = Pass {
        entry: name.to_string(),
        scale: 1,
        half_float: false,
    };
    for word in words {
        if let Some(number) = word.strip_prefix('/') {
            pass.scale = number
                .parse()
                .ok()
                .filter(|n: &u32| *n >= 1)
                .ok_or_else(|| format!("the scale `{word}` of the pass {name} is not /N with N 1 or more"))?;
        } else if word == "f16" {
            pass.half_float = true;
        } else {
            return Err(format!("the word `{word}` of the pass {name} is not known (/N or f16)"));
        }
    }
    Ok(pass)
}

/// A variable: `name type`, `name type[N]`, with `= number number ...` after it for the values
/// before a mod sets it. The types are f32, vec2, vec3 and vec4.
fn parse_variable(item: &str) -> Result<Variable, String> {
    let (left, default) = item.split_once('=').unwrap_or((item, ""));
    let mut words = left.split_whitespace();
    let (Some(name), Some(kind), None) = (words.next(), words.next(), words.next()) else {
        return Err(format!("the variable `{item}` is not `name type`"));
    };
    let (kind, slots, array) = match kind.split_once('[') {
        Some((kind, count)) => (
            kind,
            count
                .trim_end_matches(']')
                .parse::<u32>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("the size of the variable {name} is not [N]"))?,
            true,
        ),
        None => (kind, 1, false),
    };
    let per = match kind {
        "f32" => 1,
        "vec2" => 2,
        "vec3" => 3,
        "vec4" => 4,
        other => {
            return Err(format!(
                "the type `{other}` of the variable {name} is not f32, vec2, vec3 or vec4"
            ));
        }
    };
    let default = default
        .split_whitespace()
        .map(|v| {
            v.parse::<f32>()
                .map_err(|_| format!("the value `{v}` of the variable {name} is not a number"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Variable {
        name: name.to_string(),
        slots,
        per,
        default,
        array,
    })
}

/// Each setting is a variable of one f32, named once: the values file keeps one number for
/// each name.
fn check_settings(header: &Header) -> Result<(), String> {
    for (index, name) in header.settings.iter().enumerate() {
        if header.settings[..index].contains(name) {
            return Err(format!("the setting {name} is named twice"));
        }
        let Some(variable) = header.variable(name) else {
            return Err(format!("the setting {name} is not a variable of the line `variables`"));
        };
        if variable.per != 1 || variable.array {
            return Err(format!("the setting {name} is not a variable of type f32"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(lines: &[&str]) -> String {
        let mut text = String::from("//! interface: mge-xe-wgsl 1\n//! passes: main\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("@fragment fn main(in: Quad) -> @location(0) vec4f { return vec4f(0.0); }\n");
        text
    }

    /// Every key, with its value read.
    #[test]
    fn the_keys_give_their_values() {
        let header = parse(&file(&[
            "//! passes: first, small /4 f16, last f16 /2",
            "//! category: lens",
            "//! priority_adjust: -25",
            "//! mgeflags: 40",
            "//! requires_hdr: true",
            "//! disable_sunglare: true",
            "//! texture: a.dds, b/c.tga",
            "//! variables: radius f32 = 0.5, tint vec3 = 1 0.5, points vec4[3], one f32[1] = 2",
            "//! settings: radius",
        ]))
        .unwrap();
        let pass = |entry: &str, scale, half_float| Pass {
            entry: entry.to_string(),
            scale,
            half_float,
        };
        assert_eq!(
            header.passes,
            [pass("first", 1, false), pass("small", 4, true), pass("last", 2, true)]
        );
        assert_eq!(header.category, "lens");
        assert_eq!(header.priority(), 3_000_000 - 25);
        assert_eq!(header.mgeflags, Some(40));
        assert!(header.requires_hdr && header.disable_sunglare);
        assert_eq!(header.textures, ["a.dds", "b/c.tga"]);
        let variable = |name: &str, slots, per, default: &[f32], array| Variable {
            name: name.to_string(),
            slots,
            per,
            default: default.to_vec(),
            array,
        };
        assert_eq!(
            header.variables,
            [
                variable("radius", 1, 1, &[0.5], false),
                variable("tint", 1, 3, &[1.0, 0.5], false),
                variable("points", 3, 4, &[], true),
                variable("one", 1, 1, &[2.0], true),
            ]
        );
        assert_eq!(header.settings, ["radius"]);
        assert!(header.is_setting("radius") && !header.is_setting("tint"));
        assert_eq!(header.setting_defaults(), [0.5]);
    }

    /// A file with only the two needed lines: the defaults of the other keys.
    #[test]
    fn a_file_without_the_other_keys_has_their_defaults() {
        let header = parse(&file(&[])).unwrap();
        assert_eq!(header.category, "");
        assert_eq!(header.priority(), 6_000_000);
        assert_eq!(header.mgeflags, None);
        assert!(!header.requires_hdr && !header.disable_sunglare);
        assert!(header.textures.is_empty() && header.variables.is_empty() && header.settings.is_empty());
        let none = parse(&file(&["//! category: none"])).unwrap();
        assert_eq!((none.category.as_str(), none.priority()), ("", 6_000_000));
    }

    /// The header ends at the first line that is not `//!`: a key after it is WGSL's comment.
    #[test]
    fn the_header_ends_at_the_first_other_line() {
        let text = format!("{}//! category: unknown\n", file(&[]));
        assert_eq!(parse(&text).unwrap().category, "");
    }

    /// Lines with CR LF read as with LF.
    #[test]
    fn lines_with_cr_lf_read_the_same() {
        let text = file(&["//! category: tone", "//! variables: a f32 = 1", "//! settings: a"]);
        assert_eq!(parse(&text.replace('\n', "\r\n")).unwrap(), parse(&text).unwrap());
    }

    /// Each value that the renderer does not accept gives its reason.
    #[test]
    fn a_wrong_value_gives_its_reason() {
        let cases: &[(&[&str], &str)] = &[
            (&["//! category"], "the line `//! category` has no `key: value`"),
            (&["//! category: sky"], "the category `sky` is not known"),
            (&["//! priority_adjust: high"], "the priority adjust `high` is not a number"),
            (&["//! mgeflags: -1"], "the flags `-1` are not a number"),
            (&["//! requires_hdr: yes"], "`yes` is not true or false"),
            (&["//! texture: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11"], "more than 10 textures"),
            (&["//! variables: a"], "the variable `a` is not `name type`"),
            (&["//! variables: a f32 b"], "the variable `a f32 b` is not `name type`"),
            (
                &["//! variables: a mat4"],
                "the type `mat4` of the variable a is not f32, vec2, vec3 or vec4",
            ),
            (&["//! variables: a f32[0]"], "the size of the variable a is not [N]"),
            (
                &["//! variables: a f32 = x"],
                "the value `x` of the variable a is not a number",
            ),
            (&["//! variables: a f32, a vec2"], "the variable a is named twice"),
            (
                &["//! passes: main /0"],
                "the scale `/0` of the pass main is not /N with N 1 or more",
            ),
            (
                &["//! passes: main half"],
                "the word `half` of the pass main is not known (/N or f16)",
            ),
            (&["//! colour: red"], "the key `colour` is not known"),
        ];
        for (lines, reason) in cases {
            assert_eq!(parse(&file(lines)).unwrap_err(), *reason, "{lines:?}");
        }
        assert_eq!(
            parse("//! passes: main\n").unwrap_err(),
            "no line `//! interface: mge-xe-wgsl 1` at its start"
        );
        assert_eq!(
            parse("//! interface: mge-xe-wgsl 2\n//! passes: main\n").unwrap_err(),
            "no line `//! interface: mge-xe-wgsl 1` at its start"
        );
        assert_eq!(
            parse("//! interface: mge-xe-wgsl 1\n").unwrap_err(),
            "no line `//! passes: ...` with the fragment functions"
        );
        assert_eq!(
            parse("//! interface: mge-xe-wgsl 1\n//! passes: ,\n").unwrap_err(),
            "no line `//! passes: ...` with the fragment functions"
        );
    }

    /// The API's letters and values: a vector of an array takes 4 places, and the values
    /// that the line does not give are 0.
    #[test]
    fn the_api_shape_has_the_letters_of_an_effect_file() {
        let header = parse(&file(&[
            "//! variables: a f32 = 2, b vec3 = 1 2, c f32[3] = 4 5, d vec2[2] = 1 2 3 4 5, e vec4 = 1 2 3 4 5",
        ]))
        .unwrap();
        let shapes: Vec<(char, Vec<f32>)> = header.variables.iter().map(Variable::api_shape).collect();
        assert_eq!(
            shapes,
            [
                ('f', vec![2.0]),
                ('3', vec![1.0, 2.0, 0.0]),
                ('F', vec![4.0, 5.0, 0.0]),
                ('V', vec![1.0, 2.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0]),
                ('4', vec![1.0, 2.0, 3.0, 4.0]),
            ]
        );
    }

    /// A setting must be a variable of one f32, named once.
    #[test]
    fn a_setting_is_a_variable_of_one_f32() {
        let reason = |lines: &[&str]| parse(&file(lines)).unwrap_err();
        assert_eq!(
            reason(&["//! variables: a f32", "//! settings: a, b"]),
            "the setting b is not a variable of the line `variables`"
        );
        assert_eq!(
            reason(&["//! settings: a"]),
            "the setting a is not a variable of the line `variables`"
        );
        assert_eq!(
            reason(&["//! variables: a vec2", "//! settings: a"]),
            "the setting a is not a variable of type f32"
        );
        assert_eq!(
            reason(&["//! variables: a f32[1]", "//! settings: a"]),
            "the setting a is not a variable of type f32"
        );
        assert_eq!(
            reason(&["//! variables: a f32", "//! settings: a, a"]),
            "the setting a is named twice"
        );
        // The order of the lines does not matter.
        let header = parse(&file(&["//! settings: b, a", "//! variables: a f32 = 1, b f32"])).unwrap();
        assert_eq!(header.settings, ["b", "a"]);
        assert_eq!(header.setting_defaults(), [0.0, 1.0]);
    }
}
