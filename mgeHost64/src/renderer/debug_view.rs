//! The debug view of the frame: the one shader view that is on (at most one, with its term
//! for the water and the post chain) and the set of line sources. Every input sets this one
//! value: the tunes `debug view <name> [term]`, `debug view off` and `debug lines <name>
//! on|off` (Lua `nirender.debug.view` and `nirender.debug.lines` send them), the debug word
//! of the Settings record (`mge.render.lightDebug`, bit 0: the lights view), and in a replay
//! the arguments `--debug-view <name> [term]` and `--debug-lines <name,name>`.
//!
//! A replay takes its view only from its arguments: the view is pinned there, and a tune or
//! a Settings record of the capture does not change it. So one capture serves every view.

use nirender_wgpu::debug_lines::{LineSources, UnknownSource, view_id};

/// A shader view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    None,
    /// A draw lit for each pixel shows only the light it takes, without its material, its
    /// textures and the fog.
    Lights,
    /// The draw shader colours each pixel by the layer of the shadow map that serves it (red
    /// the first, green the second, blue outside both, grey a draw without the shadow test),
    /// darker where the map shadows the pixel.
    ShadowLayers,
    /// The shadow map, drawn small over the frame.
    ShadowMap,
    /// One term of the water's shader (`water_color` of the built-in water only).
    Water(f32),
    /// One term of the post chain: 1 tints the result, 2 shows the linear depth.
    Post(f32),
}

impl View {
    /// The names of the views, in the order of their ids (1 to 5).
    const NAMES: [&'static str; 5] = ["lights", "shadow-layers", "shadow-map", "water", "post"];

    /// The view of a name and its term. The water and the post chain take a term above 0;
    /// the other views take none.
    pub fn parse(name: &str, term: Option<&str>) -> Result<View, String> {
        let term = term.map(|text| text.parse::<f32>().ok().filter(|term| term.is_finite() && *term > 0.0).ok_or_else(|| format!("the term {text:?} is not a number above 0")));
        let view = match (name, term) {
            ("lights", None) => View::Lights,
            ("shadow-layers", None) => View::ShadowLayers,
            ("shadow-map", None) => View::ShadowMap,
            ("water", Some(term)) => View::Water(term?),
            ("post", Some(term)) => View::Post(term?),
            ("water" | "post", None) => return Err(format!("the view {name} takes a term")),
            ("lights" | "shadow-layers" | "shadow-map", Some(_)) => return Err(format!("the view {name} takes no term")),
            _ => return Err(format!("no view is named {name:?} (the views: {})", View::NAMES.join(", "))),
        };
        Ok(view)
    }

    /// The id of the view ([`view_id`]): 0 none, then 1 to 5 in the order of [`View::NAMES`].
    pub fn id(self) -> u32 {
        match self {
            View::None => view_id::NONE,
            View::Lights => view_id::LIGHTS,
            View::ShadowLayers => view_id::SHADOW_LAYERS,
            View::ShadowMap => view_id::SHADOW_MAP,
            View::Water(_) => view_id::WATER,
            View::Post(_) => view_id::POST,
        }
    }

    /// The view as the tune names it.
    fn describe(self) -> String {
        match self {
            View::None => "off".to_owned(),
            View::Water(term) | View::Post(term) => format!("{} {term}", View::NAMES[self.id() as usize - 1]),
            _ => View::NAMES[self.id() as usize - 1].to_owned(),
        }
    }
}

/// The debug view of the frame and its inputs.
#[derive(Clone, Debug, Default)]
pub struct DebugView {
    pub view: View,
    pub lines: LineSources,
    /// A replay: only its arguments set the view.
    pinned: bool,
    /// The lights bit of the last Settings record; `None` before the first record.
    settings_lights: Option<bool>,
}

impl DebugView {
    /// The view of a session. A replay (`pinned`) takes `--debug-view` and `--debug-lines` out
    /// of the arguments, so the crate does not see them; a live session has no such arguments.
    pub fn from_arguments(arguments: &mut Vec<String>, pinned: bool) -> Result<DebugView, String> {
        let mut debug = DebugView { pinned, ..DebugView::default() };
        while let Some(at) = arguments.iter().position(|argument| argument == "--debug-view" || argument == "--debug-lines") {
            if !pinned {
                return Err(format!("{} is an argument of a replay", arguments[at]));
            }
            let flag = arguments.remove(at);
            let value = (at < arguments.len()).then(|| arguments.remove(at)).ok_or_else(|| format!("{flag} takes a name"))?;
            if flag == "--debug-lines" {
                debug.lines = LineSources::parse_list(&value).map_err(|problem| problem.to_string())?;
                continue;
            }
            let term = (matches!(value.as_str(), "water" | "post") && at < arguments.len()).then(|| arguments.remove(at));
            debug.view = View::parse(&value, term.as_deref())?;
        }
        Ok(debug)
    }

    pub fn lights(&self) -> bool {
        self.view == View::Lights
    }

    pub fn shadow_layers(&self) -> bool {
        self.view == View::ShadowLayers
    }

    pub fn shadow_map(&self) -> bool {
        self.view == View::ShadowMap
    }

    /// The term of the water's view, 0 when it is off.
    pub fn water_term(&self) -> f32 {
        if let View::Water(term) = self.view { term } else { 0.0 }
    }

    /// The term of the post chain's view, 0 when it is off.
    pub fn post_term(&self) -> f32 {
        if let View::Post(term) = self.view { term } else { 0.0 }
    }

    /// The tune `debug view ...` or `debug lines ...` (the text after `debug `). Returns the
    /// line for the log.
    pub fn tune(&mut self, text: &str) -> String {
        if self.pinned {
            return format!("tune: debug {text}: a replay takes its view from its arguments");
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let problem = match words.as_slice() {
            ["view", "off"] => {
                self.view = View::None;
                None
            }
            ["view", name] => View::parse(name, None).map(|view| self.view = view).err(),
            ["view", name, term] => View::parse(name, Some(*term)).map(|view| self.view = view).err(),
            ["lines", name, state @ ("on" | "off")] => match LineSources::from_name(name) {
                Some(source) => {
                    self.lines.set(source, *state == "on");
                    None
                }
                None => Some(UnknownSource((*name).to_owned()).to_string()),
            },
            _ => Some("not view <name> [term], view off or lines <name> on|off".to_owned()),
        };
        match problem {
            Some(problem) => format!("tune: debug {text}: {problem}"),
            None => format!("tune: debug view {}, lines {}", self.view.describe(), self.lines_text()),
        }
    }

    /// The debug word of a Settings record (`mge.render.lightDebug`): only bit 0, the lights
    /// view, and only when it changes, so a record sent for another setting keeps a view that
    /// a tune set. The first record of a session counts as a change only when its bit is 1.
    /// The bit going off clears only the lights view.
    pub fn settings_word(&mut self, word: u32) {
        let lights = word & SETTINGS_DEBUG_LIGHT != 0;
        let before = self.settings_lights.replace(lights);
        if self.pinned || before == Some(lights) {
            return;
        }
        if lights {
            self.view = View::Lights;
        } else if before.is_some() && self.view == View::Lights {
            self.view = View::None;
        }
    }

    fn lines_text(&self) -> String {
        let names: Vec<&str> = self.lines.names().collect();
        if names.is_empty() { "none".to_owned() } else { names.join(",") }
    }
}

/// `Settings::debug`: the bit of the lights view (`mge.render.lightDebug`,
/// `FixedFunctionShader::lightDebug`).
pub const SETTINGS_DEBUG_LIGHT: u32 = 1;

/// The start-time switches that set the debug views before this carrier. The helper reads
/// none of them.
pub const RETIRED_SWITCHES: [&str; 4] = ["MGE_XE_SHADOW_LAYERS", "MGE_XE_SHADOW_DEBUG", "MGE_XE_WATER_DEBUG", "MGE_XE_POST_DEBUG"];

/// The log line for the retired switches that are set (`is_set` answers for one name), so that
/// a variable left from an old test does not look like a view that failed. [`None`] when none
/// is set.
pub fn retired_switches_line(is_set: impl Fn(&str) -> bool) -> Option<String> {
    let set: Vec<&str> = RETIRED_SWITCHES.into_iter().filter(|name| is_set(name)).collect();
    (!set.is_empty()).then(|| format!("debug view: {} set, but retired: use the tune `debug view <name>`, nirender.debug.view or --debug-view", set.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn a_retired_switch_gets_one_line() {
        assert_eq!(retired_switches_line(|_| false), None);
        let line = retired_switches_line(|name| name == "MGE_XE_SHADOW_DEBUG" || name == "MGE_XE_POST_DEBUG").unwrap();
        assert!(line.contains("MGE_XE_SHADOW_DEBUG, MGE_XE_POST_DEBUG set"), "{line}");
        assert!(!line.contains("MGE_XE_WATER_DEBUG"), "{line}");
    }

    #[test]
    fn names_parse() {
        assert_eq!(View::parse("lights", None), Ok(View::Lights));
        assert_eq!(View::parse("shadow-layers", None), Ok(View::ShadowLayers));
        assert_eq!(View::parse("shadow-map", None), Ok(View::ShadowMap));
        assert_eq!(View::parse("water", Some("3")), Ok(View::Water(3.0)));
        assert_eq!(View::parse("post", Some("2")), Ok(View::Post(2.0)));
        assert!(View::parse("water", None).is_err());
        assert!(View::parse("water", Some("0")).is_err());
        assert!(View::parse("post", Some("x")).is_err());
        assert!(View::parse("lights", Some("1")).is_err());
        assert!(View::parse("light", None).is_err());
        for (id, name) in View::NAMES.iter().enumerate() {
            let term = matches!(*name, "water" | "post").then_some("1");
            assert_eq!(View::parse(name, term).unwrap().id(), id as u32 + 1);
        }
    }

    #[test]
    fn replay_arguments_are_taken_out() {
        let mut list = arguments("numidium.exe a.cap b.png --debug-view water 4 --frame 3 --debug-lines lights,bounds");
        let debug = DebugView::from_arguments(&mut list, true).unwrap();
        assert_eq!(debug.view, View::Water(4.0));
        assert_eq!(debug.lines, LineSources::from_bits(1 | 4));
        assert_eq!(list, arguments("numidium.exe a.cap b.png --frame 3"));

        let mut list = arguments("x --debug-view lights");
        assert_eq!(DebugView::from_arguments(&mut list, true).unwrap().view, View::Lights);
        assert!(DebugView::from_arguments(&mut arguments("x --debug-view water"), true).is_err());
        assert!(DebugView::from_arguments(&mut arguments("x --debug-view"), true).is_err());
        assert!(DebugView::from_arguments(&mut arguments("x --debug-lines wires"), true).is_err());
        assert!(DebugView::from_arguments(&mut arguments("x --live ring --debug-view lights"), false).is_err());
        let mut list = arguments("x --live ring");
        assert_eq!(DebugView::from_arguments(&mut list, false).unwrap().view, View::None);
        assert_eq!(list, arguments("x --live ring"));
    }

    #[test]
    fn tunes_set_the_view() {
        let mut debug = DebugView::default();
        debug.tune("view shadow-layers");
        assert!(debug.shadow_layers());
        debug.tune("view post 2");
        assert_eq!(debug.post_term(), 2.0);
        assert_eq!(debug.water_term(), 0.0);
        debug.tune("lines lights on");
        debug.tune("lines shadow-boxes on");
        debug.tune("lines lights off");
        assert_eq!(debug.lines, LineSources::SHADOW_BOXES);
        assert!(debug.tune("view nothing").contains("no view"));
        assert_eq!(debug.view, View::Post(2.0));
        assert!(debug.tune("lines wires on").contains("wires"));
        debug.tune("view off");
        assert_eq!(debug.view, View::None);
    }

    #[test]
    fn a_pinned_view_ignores_the_capture() {
        let mut debug = DebugView::from_arguments(&mut arguments("x --debug-view shadow-map"), true).unwrap();
        debug.tune("view lights");
        debug.tune("lines lights on");
        debug.settings_word(SETTINGS_DEBUG_LIGHT);
        debug.settings_word(0);
        assert_eq!(debug.view, View::ShadowMap);
        assert!(debug.lines.is_empty());
    }

    #[test]
    fn the_first_settings_record() {
        // A first record with the bit 0 keeps a view that a tune set before it.
        let mut debug = DebugView::default();
        debug.tune("view water 2");
        debug.settings_word(0);
        assert_eq!(debug.view, View::Water(2.0));
        // A first record with the bit 1 sets the lights view.
        let mut debug = DebugView::default();
        debug.settings_word(SETTINGS_DEBUG_LIGHT);
        assert!(debug.lights());
    }

    #[test]
    fn the_settings_bit_acts_on_its_edges() {
        let mut debug = DebugView::default();
        debug.settings_word(0);
        debug.tune("view shadow-map");
        // A record for another setting: the same bit, no change.
        debug.settings_word(0);
        assert!(debug.shadow_map());
        // 0 to 1: the lights view.
        debug.settings_word(SETTINGS_DEBUG_LIGHT | 2);
        assert!(debug.lights());
        // The other bits are not views.
        debug.settings_word(SETTINGS_DEBUG_LIGHT);
        assert!(debug.lights());
        // 1 to 0 clears the lights view.
        debug.settings_word(0);
        assert_eq!(debug.view, View::None);
        // 1 to 0 keeps another view.
        debug.settings_word(SETTINGS_DEBUG_LIGHT);
        debug.tune("view shadow-layers");
        debug.settings_word(0);
        assert!(debug.shadow_layers());
    }
}
