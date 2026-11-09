//! The view group: group 0 of the pipelines of MGE XE, with one layout for all of them and one
//! group for each view of the frame (the main view, the mirror of the water, the two layers of
//! the shadow map, the game view). It has the values of the view ([`View`], the uniform `u` of a
//! shader) and the baked light of the distant lights (`baked_light`, from [`LightBinding`]). The
//! values of one pipeline only (the terrain of the land, the view range of a handoff, the
//! light of the frame on the water volumes) are in its group 1. The pipelines that draw into
//! the world take it: the land, the statics (with their shadow casters), the glows, the grass,
//! the water plane, the dry spaces, the caustics and the water volumes. The number of a view
//! is the number of a set of the statics.

use nirender_wgpu::wgpu;

use super::distant_lights::{self, LightBinding};

/// The views of a frame, and the group of each. The main view and its mirror have the
/// projection of the land (a far plane at the draw distance); the game view has the
/// projection of the game, for the draws at the end of the world scene of the game.
pub const MAIN: usize = 0;
pub const MIRROR: usize = 1;
pub const SHADOW: [usize; 2] = [2, 3];
pub const GAME: usize = 4;

/// The most places that the helper takes: one for each view slot of the host. A place is held
/// by a view slot, so a frame cannot use more (D82 caps, design B). The player's setting
/// (`views.full_tier_max`) is at most this, and is the number of places of a session
/// ([`TierViews::new`]).
pub const MAX_PLACES: usize = nirender_wgpu::stream_format::VIEW_SLOTS;

/// The groups of the views of their own that get the full tier start here. A SET is a place
/// (the sets from 0 to the number of places less 1) or a spare for a later call of a place in
/// one frame (D82 section 3.4f, R2; the sets from the number of places on). Each set has two
/// groups, side by side: the one with the projection of the land, and the one with the
/// projection of the game for the grass at the end of the world (D82 section 3.4f, slice 2).
/// The numbers are arithmetic with no upper bound; a group is made at the first call that
/// needs it ([`ViewBinding::ensure_set`]) and freed with its set.
pub const TIER_FIRST: usize = 5;

/// The group with the projection of the land of the set `set`.
pub const fn tier(set: usize) -> usize {
    TIER_FIRST + 2 * set
}

/// The group with the projection of the game, for the grass at the end of the world, of the
/// set `set`.
pub const fn tier_game(set: usize) -> usize {
    TIER_FIRST + 2 * set + 1
}

/// The group of a set of the statics. The sets 0 to 3 have the groups of the same numbers (the
/// game view between them has no set); the sets from 4 on are the views of their own, in the
/// groups from [`TIER_FIRST`].
pub const fn group_of_set(set: usize) -> usize {
    if set < GAME { set } else { tier(set - GAME) }
}

/// The values of a view; the layout of [`View`] in [`wgsl()`].
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct View {
    pub view_projection: [f32; 16],
    /// The position of the camera; w is 0 (the view range of a handoff is a value of each
    /// pipeline).
    pub eye: [f32; 4],
    pub fog_color: [f32; 4],
    /// The exponential fog: its start and its divisor. The linear fog: its start and its end.
    pub fog: [f32; 4],
    /// x: 1 for the exponential fog past the view range. In a reflection: y the side of the
    /// water plane to keep (1 above, -1 below), z the plane (`Environment::mirror_level`). w: the
    /// height of the engine's water plane; the eye is above the sea above it.
    pub flags: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_color: [f32; 4],
    pub ambient: [f32; 4],
    /// The scattering: the position of the sun and the good weather value; the sky colour
    /// with the far scatter colour in it and 1 when the scattering is on; the two scatter
    /// colours; the three values of the height of the sun.
    pub sun_pos: [f32; 4],
    pub sky: [f32; 4],
    pub outscatter: [f32; 4],
    pub inscatter: [f32; 4],
    pub sun_altitude: [f32; 4],
    /// The view and the projection for a position less `eye` (`clip_of` in WGSL). Far from
    /// the origin of the world a product of the absolute position with `view_projection` in
    /// f32 rounds the depth row and the w row of the clip position apart, by about 0.004
    /// units at 74000 units; the depth is their quotient, so the error becomes about 0.004
    /// times the distance in depth (half a unit at 135 units). The grass then fights the
    /// terrain of the game, which is relative to the camera, and itself. Relative to the eye
    /// the two rows round together, as the view and then the projection of the Direct3D 9
    /// path do (`transformGrassVert`).
    pub relative: [f32; 16],
}

/// `relative` of a view: `view` and `projection` with the layout of Direct3D, `eye` the
/// position that the shader takes from each vertex ([`View::eye`]). The translation of the view
/// less the eye is small, and it is made in f64 before the projection.
pub fn relative(view: &[f32; 16], projection: &[f32; 16], eye: [f32; 3]) -> [f32; 16] {
    let v = view.map(f64::from);
    let mut r = v;
    for c in 0..3 {
        r[12 + c] = eye[0] as f64 * v[c] + eye[1] as f64 * v[4 + c] + eye[2] as f64 * v[8 + c] + v[12 + c];
    }
    let p = projection.map(f64::from);
    core::array::from_fn(|i| (0..4).map(|k| r[i / 4 * 4 + k] * p[k * 4 + i % 4]).sum::<f64>() as f32)
}

/// `relative` of a view and projection that come as one matrix (the layers of the shadow
/// map, orthographic: their w row has no position in it).
pub fn relative_of_product(view_projection: &[f32; 16], eye: [f32; 3]) -> [f32; 16] {
    relative(view_projection, &IDENTITY, eye)
}

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

const VIEW_WGSL: &str = "
struct View {
    view_projection: mat4x4f,
    eye: vec4f,
    fog_color: vec4f,
    fog: vec4f,
    flags: vec4f,
    sun_direction: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    sun_pos: vec4f,
    sky: vec4f,
    outscatter: vec4f,
    inscatter: vec4f,
    sun_altitude: vec4f,
    relative: mat4x4f,
}
@group(0) @binding(0) var<uniform> u: View;

// The clip position of a position in the space of the world, by a matrix that is relative to
// the eye (made as `View::relative` makes it, for the projection that the caller needs): the
// position less the eye goes through the matrix, never the absolute position.
fn clip_with(relative: mat4x4f, world: vec3f) -> vec4f {
    return relative * vec4f(world - u.eye.xyz, 1.0);
}

// The clip position of a position in the space of the world, for the projection of this view
// (see `View::relative`).
fn clip_of(world: vec3f) -> vec4f {
    return clip_with(u.relative, world);
}
";

/// The code of the view group for the start of a shader: `u` and `baked_light`.
pub fn wgsl() -> String {
    format!("{VIEW_WGSL}{}", distant_lights::baked_wgsl_at(0, 1))
}

/// The views of their own that drew the full tier (the places that the player allows): which
/// group and sets a call gets, and the counts for the report. What a view needs for its own
/// draws (its view group, its cull sets) is keyed to the PLACE of the full tier that the host
/// gives it (`ViewKey::place`), so a mirror that keeps its place keeps its group. The writes of
/// the view's values and of the cull values land before all commands of the frame (the
/// one-submit rule), so a place has one call at the start and one at the end of the world in
/// a frame with its own set: a second call of the place in the same frame would show the last
/// call's values in both, so it takes a spare set. There is no count cap on the spares: each
/// (place, repeat) of a frame has one, and the sets are made on demand within the memory
/// that the views may use (the size rule and the scope of the caller). A spare that no call
/// used for [`SPARE_IDLE_FRAMES`] frames is listed by [`TierViews::idle_spares`], and the
/// caller frees all its sets.
pub struct TierViews {
    /// The places that the player allows (`views.full_tier_max`, at most [`MAX_PLACES`]): the
    /// sets from this number on are spares. The number does not change in a session, so a set
    /// keeps its number.
    places: usize,
    /// The calls at the start and at the end of the world, and the post chains (the call at the
    /// end of the scene), of the frame in progress.
    started: Calls,
    ended: Calls,
    posted: Calls,
    spares: SpareTable,
    /// The frame of the last call of any kind on each spare set (index: the set less the number
    /// of places); `None` for a spare that has no sets, or whose sets were listed as idle.
    spare_used: Vec<Option<u64>>,
    /// The calls (of all three kinds) that took a spare set, and the most spare sets that one
    /// kind used in a frame.
    pub spare_calls: u64,
    pub spare_peak: usize,
    /// The calls of the post chain that were served, the ones refused (no place, or a place above
    /// the cap), the ones refused because their place had a call in the same frame already, and
    /// the ones that could not run (no environment record).
    pub post_calls: u64,
    pub post_refused: u64,
    pub post_repeated: u64,
    pub post_unserved: u64,
    /// The view slot that each set had in its last call (by the number of the set), and the
    /// changes of it (a place that a view keeps does not change).
    holder: Vec<Option<u32>>,
    pub holder_changes: u64,
    /// The calls at the end of the world (the grass) that were served, the ones refused (no
    /// place, or a place above the cap) and the ones refused because their place had a call in
    /// the same frame already, the ones among the served that drew grass, and the places they
    /// drew.
    pub calls_after: u64,
    pub refused_after: u64,
    pub repeated_after: u64,
    pub grass_calls: u64,
    pub grass_places: u64,
    /// The calls at the start of the world that drew glows.
    pub glow_calls: u64,
    /// The calls at the start of the world that were served, the ones refused (no place, or a
    /// place above the cap), the ones refused because their place had a call in the same frame
    /// already, and the ones that could not draw (the land or the statics were not loaded).
    pub calls: u64,
    pub refused: u64,
    pub repeated: u64,
    pub unserved: u64,
    /// The calls of a view with an orthographic projection, which gets no parts.
    pub orthographic: u64,
    /// The draws of the land meshes and the draw calls of the statics, in all calls.
    pub land_draws: u64,
    pub static_calls: u64,
    /// The bytes of GPU memory that the sets of the views of their own made, in all (what
    /// exists now is counted from the sets, not from this: [`TierViews::note_live`]).
    pub bytes: u64,
    /// The most bytes that the sets of the views of their own had at one time.
    pub peak_live: u64,
    /// After an out-of-memory error at a creation, no new set is made until one is freed.
    held: bool,
    /// The sets (or growths) that the size rule refused, the ones refused while the views were
    /// held, and the creations whose scope caught an out-of-memory error.
    pub refused_budget: u64,
    pub refused_held: u64,
    pub creation_errors: u64,
    /// The sets of spares and places that were freed (all their parts).
    pub sets_freed: u64,
    /// The near plane of the last call that was logged, and how many lines were logged.
    last_near: f32,
    near_lines: u32,
}

/// The most lines of the near plane of a view of its own in a session.
const NEAR_LINES: u32 = 12;

/// The calls of one kind in the frame in progress.
struct Calls {
    /// The frame of the last call of each place's own set (by the number of the place).
    placed: Vec<Option<u64>>,
}

impl Calls {
    fn new(places: usize) -> Calls {
        Calls { placed: vec![None; places] }
    }
}

/// The spare sets of the frame in progress: which repeat of which place has which spare. All
/// three kinds of call read it, so that a repeat holds one spare for the world start, its end and
/// its post chain, whichever of them the scene skips (D82 R2).
#[derive(Default)]
struct SpareTable {
    /// The frame that `assigned` is for.
    frame: Option<u64>,
    /// The (place, repeat) of each spare, in the order of their first call.
    assigned: Vec<(u8, u8)>,
}

impl SpareTable {
    /// The index of the spare of the repeat `repeat` of `place` in `frame`: the one that an
    /// earlier call of the frame assigned, or the next one. The set is the number of places
    /// plus this index.
    fn spare_for(&mut self, frame: u64, place: u8, repeat: u8) -> usize {
        if self.frame != Some(frame) {
            self.frame = Some(frame);
            self.assigned.clear();
        }
        match self.assigned.iter().position(|assigned| *assigned == (place, repeat)) {
            Some(index) => index,
            None => {
                self.assigned.push((place, repeat));
                self.assigned.len() - 1
            }
        }
    }
}

/// The frames that a spare may stay unused before all its sets are freed (D82 section 3.4f, R2,
/// and design B of the caps).
pub const SPARE_IDLE_FRAMES: u64 = 60;

/// What the views of their own may have on the GPU when the budget of the memory heap is not
/// known (a backend other than Vulkan, or no `VK_EXT_memory_budget`).
pub const FIXED_BUDGET: u64 = 256 << 20;

/// The most bytes that the sets of the views of their own may have, those that exist now
/// (`live`) included. `heap` is the budget and the use of the largest memory heap of the GPU
/// when the driver tells them ([`super::statics::memory_budget`]): the views get what the
/// budget leaves after the memory in use and the headroom, by the rule of the merged statics
/// ([`super::statics::cap_from_budget`]; the use counts `live` already). Without it the
/// budget is [`FIXED_BUDGET`].
pub fn views_budget(heap: Option<(u64, u64)>, live: u64) -> u64 {
    match heap {
        Some((budget, used)) => super::statics::cap_from_budget(budget, used, live),
        None => FIXED_BUDGET,
    }
}

/// The size rule: bytes `wanted` for a new set, a growth of buffers or a larger post state are
/// admitted when the bytes that exist (`live`) and these stay within `budget`.
pub fn admits(live: u64, wanted: u64, budget: u64) -> bool {
    live.saturating_add(wanted) <= budget
}

/// The bytes that a run makes for a state that must hold `estimate` bytes when it holds `current`
/// now: nothing when its textures are of the right size already, else the difference (nothing
/// when the new state is smaller). A new state, a state for another size of the picture and a
/// remake of a pool all pass through the size rule with this number.
pub fn bytes_needed(textures_fit: bool, estimate: u64, current: u64) -> u64 {
    if textures_fit { 0 } else { estimate.saturating_sub(current) }
}

/// The parts that a set of the views of their own can have. Freeing a set frees all of them: the
/// caller matches every part, so a new part is a compile error until it is freed too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetPart {
    /// The two view groups ([`ViewBinding::ensure_set`]).
    ViewGroups,
    /// The cull buffers, the uniforms and the group of group 1 of the statics.
    Statics,
    /// The places, the uniforms and the group of the grass of the views.
    Grass,
    /// The buffers of the glows of the distant lights.
    Glows,
    /// The pool, the depth frame and the buffers of the post chain.
    Post,
}

impl SetPart {
    pub const ALL: [SetPart; 5] = [SetPart::ViewGroups, SetPart::Statics, SetPart::Grass, SetPart::Glows, SetPart::Post];
}

/// An error scope of the device with the filter out-of-memory, as [`within_scope`] sees it. The
/// device's scope is [`DeviceScope`]; a test has its own, since a real out-of-memory error
/// cannot be made in a test.
pub trait OomScope {
    fn push(&mut self);
    /// Pops the scope; true when it caught an out-of-memory error.
    fn pop_failed(&mut self) -> bool;
}

/// The error scope of a device, `ErrorFilter::OutOfMemory`. wgpu keeps error scopes for one
/// thread, and a pop on another thread panics: every GPU object is made on the frame thread
/// (`mgeHost64::main`, `live::run` of the host), and a scope is pushed and popped in one hook.
pub struct DeviceScope<'a> {
    device: &'a wgpu::Device,
    scope: Option<wgpu::ErrorScopeGuard>,
    thread: std::thread::ThreadId,
}

impl<'a> DeviceScope<'a> {
    pub fn new(device: &'a wgpu::Device) -> DeviceScope<'a> {
        DeviceScope { device, scope: None, thread: std::thread::current().id() }
    }
}

impl OomScope for DeviceScope<'_> {
    fn push(&mut self) {
        debug_assert_eq!(self.thread, std::thread::current().id(), "an error scope is pushed on the thread that pops it");
        self.scope = Some(self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory));
    }

    fn pop_failed(&mut self) -> bool {
        debug_assert_eq!(self.thread, std::thread::current().id(), "an error scope is popped on the thread that pushed it");
        self.scope.take().is_some_and(|scope| pollster::block_on(scope.pop()).is_some())
    }
}

/// Runs `make` inside an out-of-memory scope. Gives what it made, or `None` when the scope caught
/// an error: what was made is dropped, and the caller counts the refusal
/// ([`TierViews::creation_failed`]) and frees the other parts of the set.
pub fn within_scope<T>(scope: &mut impl OomScope, make: impl FnOnce() -> T) -> Option<T> {
    scope.push();
    let made = make();
    if scope.pop_failed() {
        drop(made);
        return None;
    }
    Some(made)
}

/// The answer to a call that asks for its place.
enum Claim {
    /// The place's own set: the number of the place.
    Served(usize),
    /// A spare set, for a later call of the place in the same frame: its number, from the
    /// number of places on.
    Spare(usize),
    /// The view has no place, or one at or above the number of places.
    Refused,
    /// The place had a call of this kind in the same frame already.
    Repeated,
}

impl TierViews {
    /// The views of their own for `places` places (`views.full_tier_max`); a number above
    /// [`MAX_PLACES`] is clamped.
    pub fn new(places: usize) -> TierViews {
        let places = places.min(MAX_PLACES);
        TierViews {
            places,
            started: Calls::new(places),
            ended: Calls::new(places),
            posted: Calls::new(places),
            spares: SpareTable::default(),
            spare_used: Vec::new(),
            spare_calls: 0,
            spare_peak: 0,
            post_calls: 0,
            post_refused: 0,
            post_repeated: 0,
            post_unserved: 0,
            holder: Vec::new(),
            holder_changes: 0,
            calls_after: 0,
            refused_after: 0,
            repeated_after: 0,
            grass_calls: 0,
            grass_places: 0,
            glow_calls: 0,
            calls: 0,
            refused: 0,
            repeated: 0,
            unserved: 0,
            orthographic: 0,
            land_draws: 0,
            static_calls: 0,
            bytes: 0,
            peak_live: 0,
            held: false,
            refused_budget: 0,
            refused_held: 0,
            creation_errors: 0,
            sets_freed: 0,
            last_near: 0.0,
            near_lines: 0,
        }
    }

    /// The number of places; the sets from this number on are spares.
    pub fn places(&self) -> usize {
        self.places
    }

    /// The number of the place for a call at the start of the world in `frame` (the place of the
    /// view's key, below the number of places); `None` when the view has no place or one at or
    /// above the number of places, or when the place had its call in this frame already.
    pub fn claim(&mut self, frame: u64, place: Option<u8>, repeat: u8) -> Option<usize> {
        match Self::take(self.places, &mut self.started, &mut self.spares, frame, place, repeat) {
            Claim::Served(place) => {
                self.calls += 1;
                Some(place)
            }
            Claim::Spare(set) => {
                self.calls += 1;
                self.note_spare(set, frame);
                Some(set)
            }
            Claim::Repeated => {
                self.repeated += 1;
                None
            }
            Claim::Refused => {
                self.refused += 1;
                None
            }
        }
    }

    /// The number of the place for a call at the end of the world, as [`claim`](Self::claim)
    /// does for the calls at the start.
    pub fn claim_after(&mut self, frame: u64, place: Option<u8>, repeat: u8) -> Option<usize> {
        match Self::take(self.places, &mut self.ended, &mut self.spares, frame, place, repeat) {
            Claim::Served(place) => {
                self.calls_after += 1;
                Some(place)
            }
            Claim::Spare(set) => {
                self.calls_after += 1;
                self.note_spare(set, frame);
                Some(set)
            }
            Claim::Repeated => {
                self.repeated_after += 1;
                None
            }
            Claim::Refused => {
                self.refused_after += 1;
                None
            }
        }
    }

    /// The number of the place for a call of the post chain at the end of the scene, as
    /// [`claim`](Self::claim) does for the calls at the start. The post state of a place
    /// ([`super::post::PostView`]) is written once in a frame.
    pub fn claim_post(&mut self, frame: u64, place: Option<u8>, repeat: u8) -> Option<usize> {
        match Self::take(self.places, &mut self.posted, &mut self.spares, frame, place, repeat) {
            Claim::Served(place) => {
                self.post_calls += 1;
                Some(place)
            }
            Claim::Spare(set) => {
                self.post_calls += 1;
                self.note_spare(set, frame);
                Some(set)
            }
            Claim::Repeated => {
                self.post_repeated += 1;
                None
            }
            Claim::Refused => {
                self.post_refused += 1;
                None
            }
        }
    }

    /// The report line of the post chain of the views of their own.
    pub fn post_line(&self) -> String {
        format!(
            "post chain of the views of their own: {} calls, {} refused (no place, or a place at or above {}), {} refused for a second call of a place in a frame, {} could not run",
            self.post_calls, self.post_refused, self.places, self.post_repeated, self.post_unserved
        )
    }

    /// The first scene of a place in a frame (`repeat` 0) takes the place's own set. A later
    /// scene of the place (`repeat` from 1, counted by the host) takes the spare of its (place,
    /// repeat), the one-submit rule: the writes of a set land before all commands of the frame,
    /// so two calls cannot share one. A second call of one set by one kind is refused as a
    /// repeat.
    fn take(places: usize, calls: &mut Calls, spares: &mut SpareTable, frame: u64, place: Option<u8>, repeat: u8) -> Claim {
        let Some(place) = place.filter(|&place| usize::from(place) < places) else { return Claim::Refused };
        if repeat == 0 {
            if calls.placed[usize::from(place)] == Some(frame) {
                return Claim::Repeated;
            }
            calls.placed[usize::from(place)] = Some(frame);
            return Claim::Served(usize::from(place));
        }
        Claim::Spare(places + spares.spare_for(frame, place, repeat))
    }

    /// The numbers of the spares that no call used for more than [`SPARE_IDLE_FRAMES`] frames
    /// before `frame`. Each is listed once: the caller frees all its sets (the view groups, the
    /// statics, the glows, the grass and the post state), and the spare starts again at its next
    /// call. A call of any kind on a spare counts as a use.
    pub fn idle_spares(&mut self, frame: u64) -> Vec<usize> {
        let places = self.places;
        let mut idle = Vec::new();
        for (index, used) in self.spare_used.iter_mut().enumerate() {
            if used.is_some_and(|last| frame.saturating_sub(last) > SPARE_IDLE_FRAMES) {
                *used = None;
                idle.push(places + index);
            }
        }
        for &set in &idle {
            if let Some(holder) = self.holder.get_mut(set) {
                *holder = None;
            }
        }
        idle
    }

    /// Counts a call on a spare set `set` (a number from the number of places on), marks it used
    /// in `frame` and keeps the peak of the spares of a frame.
    fn note_spare(&mut self, set: usize, frame: u64) {
        let index = set - self.places;
        self.spare_calls += 1;
        self.spare_peak = self.spare_peak.max(index + 1);
        if self.spare_used.len() <= index {
            self.spare_used.resize(index + 1, None);
        }
        self.spare_used[index] = Some(frame);
    }

    /// Asks to make something of `wanted` bytes when `live` bytes exist and `budget` is the most.
    /// False (and counted) while the views are held after a creation error, or when the size
    /// rule refuses: the call then draws without that part. The caller makes it inside an
    /// out-of-memory scope ([`within_scope`]) and tells a failure ([`creation_failed`]).
    ///
    /// [`creation_failed`]: Self::creation_failed
    pub fn admit(&mut self, live: u64, wanted: u64, budget: u64) -> bool {
        if self.held {
            self.refused_held += 1;
            return false;
        }
        if !admits(live, wanted, budget) {
            self.refused_budget += 1;
            return false;
        }
        true
    }

    /// The scope of a creation caught an out-of-memory error: the caller dropped the set. No new
    /// set is made until one is freed ([`freed`](Self::freed)).
    pub fn creation_failed(&mut self) {
        self.creation_errors += 1;
        self.held = true;
    }

    /// Something was freed (a set, or the post state of a place): the views are no longer held.
    pub fn freed(&mut self) {
        self.held = false;
    }

    /// True while no new set is made.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Notes the bytes that the sets of the views of their own have now.
    pub fn note_live(&mut self, live: u64) {
        self.peak_live = self.peak_live.max(live);
    }

    /// Notes the view slot of a call of the set `set`; true when the set had another slot in its
    /// last call, or none yet (the call then logs the change).
    pub fn note_holder(&mut self, set: usize, slot: u32) -> bool {
        if self.holder.len() <= set {
            self.holder.resize(set + 1, None);
        }
        if self.holder[set] == Some(slot) {
            return false;
        }
        self.holder[set] = Some(slot);
        self.holder_changes += 1;
        true
    }

    /// True when the near plane of a call differs from the last logged one by more than
    /// 1 %, for the first [`NEAR_LINES`] such changes of a session: the call then logs it.
    pub fn note_near(&mut self, near: f32) -> bool {
        if self.near_lines >= NEAR_LINES || (near - self.last_near).abs() <= self.last_near.abs() * 0.01 {
            return false;
        }
        self.last_near = near;
        self.near_lines += 1;
        true
    }

    /// The report line, for the end of the session.
    pub fn line(&self) -> String {
        format!(
            "views of their own: {} calls at the start of the world (land, statics, glows), {} refused (no place, or a place at or above {}), {} refused for a second call of a place in a frame, {} could not draw, {} had an orthographic projection; {} land mesh draws, {} statics draw calls, {} calls drew glows; {} calls at the end of the world (grass), {} refused, {} refused for a second call of a place in a frame, {} drew {} grass places (the bound views.full_tier_max is the host's, in its full tier line); {} changes of the view that holds a place; {} calls took a spare set (at most {} spares in a frame, for all places and kinds of call together); {:.1} MB made for their sets, {:.1} MB at most at one time; {} sets or growths refused by the size rule, {} refused while held after an out-of-memory error, {} creations caught an out-of-memory error, {} sets freed",
            self.calls,
            self.refused,
            self.places,
            self.repeated,
            self.unserved,
            self.orthographic,
            self.land_draws,
            self.static_calls,
            self.glow_calls,
            self.calls_after,
            self.refused_after,
            self.repeated_after,
            self.grass_calls,
            self.grass_places,
            self.holder_changes,
            self.spare_calls,
            self.spare_peak,
            self.bytes as f64 / 1e6,
            self.peak_live as f64 / 1e6,
            self.refused_budget,
            self.refused_held,
            self.creation_errors,
            self.sets_freed
        )
    }
}

/// The inputs of the grass that the game's environment record carries: the smoothed wind and the
/// time of the simulation. The watch keeps what the records had, so that a report can say whether
/// two runs had the same wind and time (a picture of the grass moves with both).
#[derive(Default)]
pub struct EnvironmentWatch {
    records: u64,
    time_first: f32,
    time_last: f32,
    wind_first: [f32; 2],
    wind_last: [f32; 2],
    wind_min: [f32; 2],
    wind_max: [f32; 2],
    /// The wind at the last logged change.
    logged: [f32; 2],
    lines: u32,
}

/// The change of the wind in one component, from the last logged one, that is logged.
const WIND_LOGGED_STEP: f32 = 0.25;
/// The logged changes of the wind in a session.
const WIND_LINES: u32 = 12;

impl EnvironmentWatch {
    /// Notes one record of the environment. True for the first record and for a change of the
    /// wind by more than [`WIND_LOGGED_STEP`] in a component, for the first [`WIND_LINES`]
    /// times: the caller logs those.
    pub fn note(&mut self, time: f32, wind: [f32; 2]) -> bool {
        if self.records == 0 {
            self.time_first = time;
            self.wind_first = wind;
            self.wind_min = wind;
            self.wind_max = wind;
        }
        self.records += 1;
        self.time_last = time;
        self.wind_last = wind;
        self.wind_min = core::array::from_fn(|i| self.wind_min[i].min(wind[i]));
        self.wind_max = core::array::from_fn(|i| self.wind_max[i].max(wind[i]));
        let first = self.records == 1;
        let moved = (0..2).any(|i| (wind[i] - self.logged[i]).abs() > WIND_LOGGED_STEP);
        if !(first || moved) || self.lines >= WIND_LINES {
            return false;
        }
        self.logged = wind;
        self.lines += 1;
        true
    }

    /// The report line, for the end of the session.
    pub fn line(&self) -> String {
        format!(
            "environment records: {}; simulation time {:.1} to {:.1} s; wind first ({:.2}, {:.2}), last ({:.2}, {:.2}), x from {:.2} to {:.2}, y from {:.2} to {:.2}",
            self.records, self.time_first, self.time_last, self.wind_first[0], self.wind_first[1], self.wind_last[0], self.wind_last[1], self.wind_min[0], self.wind_max[0], self.wind_min[1], self.wind_max[1]
        )
    }
}

/// The views of the frame by the number of their group: the fixed ones ([`MAIN`] to [`GAME`])
/// from the start, and the two groups of each set of the views of their own ([`tier`],
/// [`tier_game`]) when [`ViewBinding::ensure_set`] makes them. A slot that is not made is
/// `None`.
pub struct ViewBinding {
    pub layout: wgpu::BindGroupLayout,
    buffers: Vec<Option<wgpu::Buffer>>,
    groups: Vec<Option<wgpu::BindGroup>>,
    /// The [`LightBinding::generation`] of the groups.
    generation: Option<u64>,
}

impl ViewBinding {
    pub fn new(device: &wgpu::Device) -> ViewBinding {
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(size_of::<View>() as u64) },
            count: None,
        }];
        entries.extend(LightBinding::layout_entries(1));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("view"), entries: &entries });
        let buffers = (0..TIER_FIRST).map(|_| Some(Self::make_buffer(device))).collect();
        ViewBinding { layout, buffers, groups: Vec::new(), generation: None }
    }

    fn make_buffer(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("view"), size: size_of::<View>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }

    fn make_group(&self, device: &wgpu::Device, light: &LightBinding, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
        entries.extend(light.entries(1));
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("view"), layout: &self.layout, entries: &entries })
    }

    /// Makes the groups again when the baked light has changed since they were made: the fixed
    /// views and the sets that exist.
    pub fn refresh(&mut self, device: &wgpu::Device, light: &LightBinding) {
        if self.generation == Some(light.generation) {
            return;
        }
        self.generation = Some(light.generation);
        let groups = self.buffers.iter().map(|buffer| buffer.as_ref().map(|buffer| self.make_group(device, light, buffer))).collect();
        self.groups = groups;
    }

    /// True when `refresh` has made the groups of the fixed views.
    pub fn is_ready(&self) -> bool {
        self.generation.is_some()
    }

    /// Makes the two groups of the set `set` (and their buffers) when they are not there. Gives
    /// false before `refresh` has made the groups of the fixed views: the light is not known.
    pub fn ensure_set(&mut self, device: &wgpu::Device, light: &LightBinding, set: usize) -> bool {
        if self.generation != Some(light.generation) {
            return false;
        }
        let last = tier_game(set);
        if self.buffers.len() <= last {
            self.buffers.resize_with(last + 1, || None);
        }
        if self.groups.len() <= last {
            self.groups.resize_with(last + 1, || None);
        }
        for number in [tier(set), tier_game(set)] {
            if self.buffers[number].is_none() {
                let buffer = Self::make_buffer(device);
                self.groups[number] = Some(self.make_group(device, light, &buffer));
                self.buffers[number] = Some(buffer);
            }
        }
        true
    }

    /// Frees the two groups of the set `set`.
    pub fn release_set(&mut self, set: usize) {
        for number in [tier(set), tier_game(set)] {
            if let Some(buffer) = self.buffers.get_mut(number) {
                *buffer = None;
            }
            if let Some(group) = self.groups.get_mut(number) {
                *group = None;
            }
        }
    }

    /// The values of a view for the frame. Each view has a buffer of its own: the writes come
    /// before all commands of the frame. A view whose set is not made is not written.
    pub fn write(&self, queue: &wgpu::Queue, view: usize, values: &View) {
        if let Some(buffer) = self.buffers.get(view).and_then(Option::as_ref) {
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(values));
        }
    }

    /// The group of a view; `None` before `refresh` has made the groups, and for a set that
    /// is not made.
    pub fn group(&self, view: usize) -> Option<&wgpu::BindGroup> {
        self.groups.get(view).and_then(Option::as_ref)
    }

    /// The groups of all views, by their numbers; empty before `refresh`.
    pub fn groups(&self) -> &[Option<wgpu::BindGroup>] {
        &self.groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The places of the views of their own in these tests (the player's setting).
    const PLACES: usize = 4;

    #[test]
    fn the_view_struct_has_the_size_of_its_wgsl() {
        // A matrix, twelve vectors of four numbers, and the relative matrix.
        assert_eq!(size_of::<View>(), 4 * (16 + 12 * 4 + 16));
        assert_eq!(VIEW_WGSL.matches(": vec4f,").count(), 12);
        // The two matrices of the struct and the parameter of `clip_with`.
        assert_eq!(VIEW_WGSL.matches(": mat4x4f,").count(), 3);
    }

    /// The relative matrix gives the clip position of the absolute one, computed in f64.
    #[test]
    fn the_relative_matrix_gives_the_clip_position_of_the_view() {
        // A view with the camera at (19724, -74050, 353), turned 30 degrees about z.
        let (s, c) = (0.5f64, 0.75f64.sqrt());
        let eye = [19724.25f64, -74050.5, 353.75];
        let rot = [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0];
        let mut view = [0.0f32; 16];
        for r in 0..3 {
            for k in 0..3 {
                view[r * 4 + k] = rot[r * 3 + k] as f32;
            }
        }
        for k in 0..3 {
            view[12 + k] = -(0..3).map(|r| eye[r] * rot[r * 3 + k]).sum::<f64>() as f32;
        }
        view[15] = 1.0;
        let projection = [0.9774, 0.0, 0.0, 0.0, 0.0, 1.7376, 0.0, 0.0, 0.0, 0.0, 1.0001, 1.0, 0.0, 0.0, -1.0001, 0.0];
        let eye32 = eye.map(|e| e as f32);
        let m = relative(&view, &projection, eye32);
        let world = [19760.0f64, -73930.0, 230.0];
        // The shader: the position less the eye in f32, then the matrix.
        let d: [f32; 3] = core::array::from_fn(|i| world[i] as f32 - eye32[i]);
        let clip: [f64; 4] = core::array::from_fn(|i| (0..3).map(|k| d[k] as f64 * m[k * 4 + i] as f64).sum::<f64>() + m[12 + i] as f64);
        // The same in f64 from the absolute position.
        let vs: [f64; 3] = core::array::from_fn(|k| (0..3).map(|r| (world[r] - eye[r]) * rot[r * 3 + k]).sum());
        let exact = [vs[0] * 0.9774, vs[1] * 1.7376, vs[2] * 1.0001 - 1.0001, vs[2]];
        let depth = |c: &[f64; 4]| c[2] / c[3];
        // The view translation rounds to f32 (about 0.004 units); the depth stays within
        // what 0.01 units of distance give at this range.
        let slope = 1.0001 / (vs[2] * vs[2]);
        assert!((depth(&clip) - depth(&exact)).abs() < 0.01 * slope, "{} {}", depth(&clip), depth(&exact));
    }

    /// A view of a camera at `eye`, turned 30 degrees about z and tilted 25 degrees about x, with the rotation that made it
    /// (row `r`, column `k` at `r * 3 + k`). The tilt puts the position of the world into the depth row and the w row; a view
    /// that only turns about z has no x or y in them, so far from the origin it cannot round them apart.
    fn tilted_view(eye: [f64; 3]) -> ([f32; 16], [f64; 9]) {
        let (s, c) = (0.5f64, 0.75f64.sqrt());
        let (sp, cp) = 25f64.to_radians().sin_cos();
        let rot = [c, -s * cp, s * sp, s, c * cp, -c * sp, 0.0, sp, cp];
        let mut view = [0.0f32; 16];
        for r in 0..3 {
            for k in 0..3 {
                view[r * 4 + k] = rot[r * 3 + k] as f32;
            }
        }
        for k in 0..3 {
            view[12 + k] = -(0..3).map(|r| eye[r] * rot[r * 3 + k]).sum::<f64>() as f32;
        }
        view[15] = 1.0;
        (view, rot)
    }

    /// Far from the origin the relative matrix keeps the depth, and the absolute product in f32 loses it. The second half is the
    /// planted fault of the far-origin fix (the product put back): the test must be able to fail, so it asserts the failure. The
    /// eyes are at 74000 units from the origin (the grass flicker, records entry 45) and at 368000 units.
    #[test]
    fn the_relative_matrix_keeps_the_depth_far_from_the_origin_and_the_absolute_product_does_not() {
        let projection = [0.9774, 0.0, 0.0, 0.0, 0.0, 1.7376, 0.0, 0.0, 0.0, 0.0, 1.0001, 1.0, 0.0, 0.0, -1.0001, 0.0];
        for eye in [[19724.25f64, -74050.5, 353.75], [-6500.25, -368500.5, 353.75]] {
            let (view, rot) = tilted_view(eye);
            let eye32 = eye.map(|e| e as f32);
            let m = relative(&view, &projection, eye32);
            // The old shader: `view_projection` made in f32, times the absolute position in f32.
            let view_projection: [f32; 16] = core::array::from_fn(|i| (0..4).map(|k| view[i / 4 * 4 + k] as f64 * projection[k * 4 + i % 4] as f64).sum::<f64>() as f32);
            for offset in [[36.0f64, 120.0, -123.75], [-60.0, 90.0, -30.0]] {
                let world: [f64; 3] = core::array::from_fn(|i| eye[i] + offset[i]);
                let vs: [f64; 3] = core::array::from_fn(|k| (0..3).map(|r| (world[r] - eye[r]) * rot[r * 3 + k]).sum());
                let exact = [vs[0] * 0.9774, vs[1] * 1.7376, vs[2] * 1.0001 - 1.0001, vs[2]];
                let depth = exact[2] / exact[3];
                let tolerance = 0.01 * 1.0001 / (vs[2] * vs[2]);

                let d: [f32; 3] = core::array::from_fn(|i| world[i] as f32 - eye32[i]);
                let clip: [f64; 4] = core::array::from_fn(|i| (0..3).map(|k| d[k] as f64 * m[k * 4 + i] as f64).sum::<f64>() + m[12 + i] as f64);
                let relative_error = (clip[2] / clip[3] - depth).abs();
                assert!(relative_error < tolerance, "eye {eye:?} offset {offset:?}: the relative matrix is off by {relative_error}, the limit is {tolerance}");

                let absolute: [f32; 4] = core::array::from_fn(|i| {
                    let mut sum = 0.0f32;
                    for k in 0..3 {
                        sum += world[k] as f32 * view_projection[k * 4 + i];
                    }
                    sum + view_projection[12 + i]
                });
                let absolute_error = (absolute[2] as f64 / absolute[3] as f64 - depth).abs();
                assert!(absolute_error > tolerance, "eye {eye:?} offset {offset:?}: the absolute product is off by only {absolute_error}, so this test cannot see the fault");
            }
        }
    }

    /// The sets of the statics take the groups of their views: the four fixed ones by their own
    /// numbers, the views of their own after the game view, and no two sets share a group.
    #[test]
    fn each_set_of_the_statics_has_a_group_of_its_own() {
        // The places and the spares of a session with the most places, and many spares.
        let sets = 4 + MAX_PLACES + 200;
        let groups: Vec<usize> = (0..sets).map(group_of_set).collect();
        assert_eq!(&groups[..4], &[MAIN, MIRROR, SHADOW[0], SHADOW[1]]);
        assert!(!groups.contains(&GAME));
        assert_eq!(groups[4], tier(0));
        assert_eq!(*groups.last().unwrap(), tier(sets - 5));
        // The groups of the land and of the game's projection, of every set: all different,
        // none of them a fixed view, and in the order of the sets.
        let mut all: Vec<usize> = (0..sets - 4).flat_map(|set| [tier(set), tier_game(set)]).collect();
        assert!(all.iter().all(|&group| group >= TIER_FIRST));
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        all.extend(groups[..4].iter().copied().chain([GAME]));
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 2 * (sets - 4) + 5);
        // The groups of the sets are the land's, one for each set of the statics.
        assert!((0..sets - 4).all(|set| group_of_set(4 + set) == tier(set)));
    }

    /// A call is keyed to the place of its view: the number it gets is the place, in any order of
    /// the calls, and the calls at the start and at the end of the world are counted apart.
    #[test]
    fn a_call_gets_the_number_of_its_place_in_any_order() {
        let mut tier = TierViews::new(PLACES);
        assert_eq!(tier.claim(3, Some(2), 0), Some(2));
        assert_eq!(tier.claim_after(3, Some(0), 0), Some(0));
        assert_eq!(tier.claim(3, Some(0), 0), Some(0));
        assert_eq!(tier.claim_after(3, Some(2), 0), Some(2));
        // The next frame gives the same places the same numbers.
        assert_eq!(tier.claim(4, Some(0), 0), Some(0));
        assert_eq!(tier.claim(4, Some(2), 0), Some(2));
        assert_eq!((tier.calls, tier.calls_after), (4, 2));
        assert_eq!((tier.refused, tier.repeated, tier.refused_after, tier.repeated_after), (0, 0, 0, 0));
    }

    /// A view with no place, or a place above the cap, gets no call. A repeat of a place in one
    /// frame (the host's `repeat` from 1) takes a spare set, because the place's buffers hold one
    /// call's values; the spares of a frame go in the order of the first call, and a call with no
    /// spare left is refused apart. A kind of call that comes later gets the spare that the first
    /// call of the repeat took.
    #[test]
    fn a_call_without_a_place_is_refused_and_a_repeat_of_a_place_takes_a_spare() {
        let mut tier = TierViews::new(PLACES);
        assert_eq!(tier.claim(7, None, 0), None);
        assert_eq!(tier.claim(7, Some(PLACES as u8), 0), None);
        assert_eq!(tier.claim(7, Some(1), 0), Some(1));
        assert_eq!(tier.claim(7, Some(1), 1), Some(PLACES));
        assert_eq!(tier.claim(7, Some(1), 2), Some(PLACES + 1));
        // A repeat of another place takes the next spares.
        assert_eq!(tier.claim(7, Some(2), 0), Some(2));
        assert_eq!(tier.claim(7, Some(2), 1), Some(PLACES + 2));
        assert_eq!(tier.claim(7, Some(2), 2), Some(PLACES + 3));
        // There is no count cap on the spares: the 5th to the 8th repeat of one place in a frame
        // each have a spare of their own.
        let later: Vec<Option<usize>> = (3..=6).map(|repeat| tier.claim(7, Some(1), repeat)).collect();
        assert_eq!(later, vec![Some(PLACES + 4), Some(PLACES + 5), Some(PLACES + 6), Some(PLACES + 7)]);
        // The calls at the end of the world count apart and take the same spares.
        assert_eq!(tier.claim_after(7, Some(1), 0), Some(1));
        assert_eq!(tier.claim_after(7, Some(1), 1), Some(PLACES));
        assert_eq!(tier.claim_after(7, Some(1), 6), Some(PLACES + 7));
        assert_eq!(tier.claim_after(7, None, 0), None);
        assert_eq!((tier.calls, tier.refused, tier.repeated), (10, 2, 0));
        assert_eq!((tier.calls_after, tier.refused_after, tier.repeated_after), (3, 1, 0));
        assert_eq!((tier.spare_calls, tier.spare_peak), (10, 8));
        // The next frame serves the place again, with its own set, and the spares start again.
        assert_eq!((tier.claim(8, Some(1), 0), tier.claim_after(8, Some(1), 0)), (Some(1), Some(1)));
        assert_eq!(tier.claim(8, Some(1), 1), Some(PLACES));
        // A second call of the same kind on the place's own set is a repeat that was not counted by
        // the host: refused apart.
        assert_eq!(tier.claim(8, Some(1), 0), None);
        assert_eq!(tier.repeated, 1);
        let line = tier.line();
        assert!(line.contains("refused") && line.contains("11 calls took a spare set"), "{line}");
    }

    /// A place at or above the number of places that the player allows is refused, and the number
    /// of places is clamped to the bound of the code.
    #[test]
    fn the_number_of_places_bounds_the_claims() {
        let mut tier = TierViews::new(2);
        assert_eq!((tier.claim(1, Some(1), 0), tier.claim(1, Some(2), 0)), (Some(1), None));
        // The spares start at the number of places.
        assert_eq!(tier.claim(1, Some(1), 1), Some(2));
        let mut wide = TierViews::new(1000);
        assert_eq!(wide.places(), MAX_PLACES);
        assert_eq!(wide.claim(1, Some(MAX_PLACES as u8 - 1), 0), Some(MAX_PLACES - 1));
        assert_eq!(wide.claim(1, Some(MAX_PLACES as u8), 0), None);
        assert_eq!(wide.claim(1, Some(0), 1), Some(MAX_PLACES));
    }

    /// The view slot of a place is noted once: the same slot again is no change, another slot is.
    #[test]
    fn a_change_of_the_view_that_holds_a_place_is_noted() {
        let mut tier = TierViews::new(PLACES);
        assert!(tier.note_holder(0, 5));
        assert!(!tier.note_holder(0, 5));
        assert!(tier.note_holder(1, 5));
        assert!(tier.note_holder(0, 6));
        assert_eq!(tier.holder_changes, 3);
        assert!(tier.line().contains("3 changes of the view that holds a place"));
    }

    /// The post chain of a place goes once in a frame on the place's own state: a second call of
    /// the place in the frame takes a spare, a view with no place or a place above the cap is
    /// refused, and a later frame is served again. The calls at the start and at the end of the
    /// world do not count against it.
    #[test]
    fn the_post_chain_of_a_place_goes_once_in_a_frame_on_its_own_state() {
        let mut tier = TierViews::new(PLACES);
        assert_eq!(tier.claim_post(10, Some(2), 0), Some(2));
        assert_eq!(tier.claim_post(10, Some(2), 1), Some(PLACES));
        assert_eq!(tier.claim_post(10, Some(0), 0), Some(0));
        assert_eq!(tier.claim_post(10, None, 0), None);
        assert_eq!(tier.claim_post(10, Some(PLACES as u8), 0), None);
        assert_eq!(tier.claim_post(11, Some(2), 0), Some(2));
        assert_eq!((tier.post_calls, tier.post_repeated, tier.post_refused), (4, 0, 2));
        assert_eq!(tier.claim(10, Some(2), 0), Some(2));
        assert_eq!(tier.claim_after(10, Some(2), 0), Some(2));
        let line = tier.post_line();
        assert!(line.contains("4 calls, 2 refused") && line.contains("or above 4"), "{line}");
    }

    /// A repeat holds one spare for all three kinds of call, also when a kind skips some scenes
    /// (the coordinator's case: places 1 and 2 both repeat, and only place 2 runs the post chain
    /// and the grass). Each kind numbers by (place, repeat), not by its own count, so the spare of
    /// place 2's repeat is the same in every kind, whichever kind calls first.
    #[test]
    fn a_repeat_holds_one_spare_for_all_kinds_when_a_kind_skips_a_scene() {
        let mut tier = TierViews::new(PLACES);
        // The world start: place 1's repeat gets the first spare, place 2's the second.
        assert_eq!(tier.claim(7, Some(1), 0), Some(1));
        assert_eq!(tier.claim(7, Some(1), 1), Some(PLACES));
        assert_eq!(tier.claim(7, Some(2), 0), Some(2));
        assert_eq!(tier.claim(7, Some(2), 1), Some(PLACES + 1));
        // The post chain and the grass run on place 2 only: their repeat has the second spare
        // (counted by each kind alone it would be the first, place 1's).
        assert_eq!(tier.claim_post(7, Some(2), 0), Some(2));
        assert_eq!(tier.claim_post(7, Some(2), 1), Some(PLACES + 1));
        assert_eq!(tier.claim_after(7, Some(2), 0), Some(2));
        assert_eq!(tier.claim_after(7, Some(2), 1), Some(PLACES + 1));
        // The other way: the kind that skips place 1 comes first in the next frame.
        assert_eq!(tier.claim_post(8, Some(2), 1), Some(PLACES));
        assert_eq!(tier.claim(8, Some(1), 1), Some(PLACES + 1));
        assert_eq!(tier.claim(8, Some(2), 1), Some(PLACES));
        assert_eq!(tier.claim_after(8, Some(1), 1), Some(PLACES + 1));
    }

    /// A spare is listed as idle when no call of any kind used it for more than
    /// [`SPARE_IDLE_FRAMES`] frames, once; a call of any kind marks it used, a place's own set is
    /// never listed, and a spare that was listed starts again at its next call.
    #[test]
    fn an_idle_spare_is_listed_after_the_idle_frames_and_a_used_one_is_not() {
        let mut tier = TierViews::new(PLACES);
        // The first spare is used by the world start, the second by the grass only, a third by
        // the post chain only; a place's own set is called too.
        assert_eq!(tier.claim(100, Some(0), 0), Some(0));
        assert_eq!(tier.claim(100, Some(1), 1), Some(PLACES));
        assert_eq!(tier.claim_after(100, Some(2), 1), Some(PLACES + 1));
        assert_eq!(tier.claim_post(100, Some(3), 1), Some(PLACES + 2));
        // At the limit they stay; one frame later they go.
        assert!(tier.idle_spares(100 + SPARE_IDLE_FRAMES).is_empty());
        // A call of the grass marks the second spare used: it stays while the others go.
        assert_eq!(tier.claim_after(130, Some(1), 1), Some(PLACES));
        assert_eq!(tier.idle_spares(101 + SPARE_IDLE_FRAMES), vec![PLACES + 1, PLACES + 2]);
        // They are listed once.
        assert_eq!(tier.idle_spares(102 + SPARE_IDLE_FRAMES), Vec::<usize>::new());
        assert_eq!(tier.idle_spares(131 + SPARE_IDLE_FRAMES), vec![PLACES]);
        // A place's own set is not listed however old it is, and a spare starts again at its
        // next call.
        assert!(tier.idle_spares(100_000).is_empty());
        assert_eq!(tier.claim(100_001, Some(1), 1), Some(PLACES));
        assert!(tier.idle_spares(100_001 + SPARE_IDLE_FRAMES).is_empty());
        assert_eq!(tier.idle_spares(100_002 + SPARE_IDLE_FRAMES), vec![PLACES]);
        // A frame before the last call (a restarted counter) lists nothing.
        assert_eq!(tier.claim(200, Some(2), 1), Some(PLACES));
        assert!(tier.idle_spares(50).is_empty());
    }

    /// An idle spare forgets its holder, so that the next view that takes the number is noted as
    /// a new holder.
    #[test]
    fn an_idle_spare_forgets_its_holder() {
        let mut tier = TierViews::new(PLACES);
        assert_eq!(tier.claim(10, Some(1), 1), Some(PLACES));
        assert!(tier.note_holder(PLACES, 5));
        assert!(!tier.note_holder(PLACES, 5));
        assert_eq!(tier.idle_spares(11 + SPARE_IDLE_FRAMES + 10), vec![PLACES]);
        assert!(tier.note_holder(PLACES, 5));
    }

    /// A test scope: `error` is what each pop reports.
    struct FakeScope {
        error: bool,
        pushed: u32,
        popped: u32,
    }

    impl OomScope for FakeScope {
        fn push(&mut self) {
            self.pushed += 1;
        }

        fn pop_failed(&mut self) -> bool {
            self.popped += 1;
            self.error
        }
    }

    /// Counts its drops.
    struct Probe(std::rc::Rc<std::cell::Cell<u32>>);

    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    /// A creation in a scope that caught an out-of-memory error is dropped and gives `None`; a
    /// creation in a clean scope is kept. A scope is pushed and popped once for each creation.
    #[test]
    fn a_creation_in_a_scope_that_caught_an_error_is_dropped() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut failing = FakeScope { error: true, pushed: 0, popped: 0 };
        assert!(within_scope(&mut failing, || Probe(drops.clone())).is_none());
        assert_eq!((failing.pushed, failing.popped, drops.get()), (1, 1, 1));
        let mut clean = FakeScope { error: false, pushed: 0, popped: 0 };
        let kept = within_scope(&mut clean, || Probe(drops.clone()));
        assert!(kept.is_some());
        assert_eq!((clean.pushed, clean.popped, drops.get()), (1, 1, 1));
    }

    /// The whole path of a creation: the size rule, the scope, and the hold. An error in the
    /// scope drops the set, counts it, and holds the views: no new set is made, whatever the
    /// bytes, until a set is freed. A set that the rule refuses is counted apart.
    #[test]
    fn an_error_in_the_scope_holds_the_views_until_a_set_is_freed() {
        let mut tier = TierViews::new(PLACES);
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut made = 0u32;
        // The steps of the caller (`DistantLand::make_for_set`), with a test scope.
        let mut make = |tier: &mut TierViews, error: bool, live: u64, wanted: u64, budget: u64| {
            if !tier.admit(live, wanted, budget) {
                return false;
            }
            let mut scope = FakeScope { error, pushed: 0, popped: 0 };
            match within_scope(&mut scope, || Probe(drops.clone())) {
                Some(_probe) => {
                    made += 1;
                    true
                }
                None => {
                    tier.creation_failed();
                    false
                }
            }
        };
        assert!(make(&mut tier, false, 0, 10, 100));
        // The size rule: 91 more bytes do not fit with 10 that exist.
        assert!(!make(&mut tier, false, 10, 91, 100));
        assert_eq!(tier.refused_budget, 1);
        // The error: the set is dropped and counted, and the views are held.
        assert!(!make(&mut tier, true, 10, 10, 100));
        assert_eq!((tier.creation_errors, tier.held(), drops.get()), (1, true, 2));
        // Held: a tiny set that fits is refused.
        assert!(!make(&mut tier, false, 10, 1, 100));
        assert_eq!(tier.refused_held, 1);
        // A free lets the next set through.
        tier.freed();
        assert!(!tier.held());
        assert!(make(&mut tier, false, 10, 1, 100));
        assert_eq!(made, 2);
        assert!(tier.line().contains("1 sets or growths refused by the size rule, 1 refused while held after an out-of-memory error, 1 creations caught an out-of-memory error"), "{}", tier.line());
    }

    /// The size rule: what exists plus what is wanted stays within the budget; no overflow.
    #[test]
    fn the_size_rule_admits_what_fits_within_the_budget() {
        assert!(admits(100, 50, 150));
        assert!(!admits(100, 51, 150));
        assert!(admits(0, 0, 0));
        // The sum saturates: it never wraps round to a small number.
        assert!(!admits(u64::MAX, 1, u64::MAX - 1));
    }

    /// The budget of the views: what the heap leaves after its use and the headroom of an eighth
    /// (at least 256 MB, at most 1 GB), counted from the bytes that the views have; a fixed
    /// amount where the heap is not known.
    #[test]
    fn the_budget_of_the_views_follows_the_heap_or_is_fixed() {
        let gb = 1u64 << 30;
        let mb = 1u64 << 20;
        assert_eq!(views_budget(None, 0), FIXED_BUDGET);
        assert_eq!(views_budget(None, 900 * mb), FIXED_BUDGET);
        // An 8 GB budget, 3 GB in use: the headroom is the most, 1 GB; 4 GB remain, and the 512 MB
        // that the views have (the use counts them) are theirs too.
        assert_eq!(views_budget(Some((8 * gb, 3 * gb)), 512 * mb), 4 * gb + 512 * mb);
        // A 1 GB budget: the headroom is the least, 256 MB.
        assert_eq!(views_budget(Some((gb, 200 * mb)), 0), gb - 200 * mb - 256 * mb);
        // A heap in use above its budget leaves nothing: the views keep what they have.
        assert_eq!(views_budget(Some((gb, 2 * gb)), 64 * mb), 64 * mb);
    }

    /// A new state, a state of another picture size and a remake of a pool pass through the rule
    /// with the bytes they add; a state of the right size adds none; a smaller one adds none.
    #[test]
    fn a_growth_of_a_state_is_counted_by_what_it_adds() {
        assert_eq!(bytes_needed(true, 14, 7), 0);
        assert_eq!(bytes_needed(false, 14, 0), 14);
        assert_eq!(bytes_needed(false, 14, 7), 7);
        assert_eq!(bytes_needed(false, 7, 14), 0);
        // At the limit, the resize is refused although the state exists.
        let (live, budget) = (100, 107);
        assert!(admits(live, bytes_needed(false, 14, 7), budget));
        assert!(!admits(live + 1, bytes_needed(false, 14, 7), budget));
    }

    /// Freeing a set frees every part: the list has each part once, and the renderer matches all.
    #[test]
    fn a_set_has_five_parts_and_each_is_listed_once() {
        let index = |part: SetPart| match part {
            SetPart::ViewGroups => 0,
            SetPart::Statics => 1,
            SetPart::Grass => 2,
            SetPart::Glows => 3,
            SetPart::Post => 4,
        };
        let mut seen: Vec<usize> = SetPart::ALL.iter().map(|&part| index(part)).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    /// Real objects: the view groups of a set are made on demand, side by side, in any order;
    /// a freed set has none, the fixed views and the other sets stay, and a refresh for a new light
    /// remakes the groups that exist.
    #[test]
    #[ignore = "needs a GPU"]
    fn the_groups_of_a_set_are_made_on_demand_and_freed_whole() {
        static GPU_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one_at_a_time = GPU_TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let mut light = LightBinding::new(&device, &queue);
        let mut views = ViewBinding::new(&device);
        assert!(!views.is_ready());
        // Before a refresh there is no light: no set is made.
        assert!(!views.ensure_set(&device, &light, 3));
        views.refresh(&device, &light);
        assert!(views.is_ready() && (MAIN..=GAME).all(|number| views.group(number).is_some()));
        assert!(views.group(tier(0)).is_none());
        // A set far from the first is made alone.
        assert!(views.ensure_set(&device, &light, 40));
        assert!(views.group(tier(40)).is_some() && views.group(tier_game(40)).is_some());
        assert!(views.group(tier(39)).is_none() && views.group(tier(41)).is_none());
        assert!(views.ensure_set(&device, &light, 2));
        // The values of a set go to its own buffer.
        views.write(&queue, tier(40), &bytemuck::Zeroable::zeroed());
        views.write(&queue, tier(41), &bytemuck::Zeroable::zeroed());
        // A new light remakes the groups that exist, and only those.
        light.generation += 1;
        views.refresh(&device, &light);
        assert!(views.group(tier(40)).is_some() && views.group(tier_game(2)).is_some());
        assert!(views.group(tier(3)).is_none());
        // Freed: both groups of the set go, and nothing else.
        views.release_set(40);
        assert!(views.group(tier(40)).is_none() && views.group(tier_game(40)).is_none());
        assert!(views.group(tier(2)).is_some() && views.group(MAIN).is_some() && views.group(GAME).is_some());
        // The set is made again with the same numbers.
        assert!(views.ensure_set(&device, &light, 40));
        assert!(views.group(tier(40)).is_some());
    }

    /// The first record of the environment and a step of the wind above a quarter are logged; a
    /// small change is not; the line keeps the first and last time and the range of the wind.
    #[test]
    fn the_wind_and_the_time_of_the_environment_are_watched() {
        let mut watch = EnvironmentWatch::default();
        assert!(watch.note(10.0, [0.0, 0.0]));
        assert!(!watch.note(11.0, [0.1, -0.1]));
        assert!(watch.note(12.0, [0.5, 0.0]));
        assert!(!watch.note(13.0, [0.5, 0.1]));
        let line = watch.line();
        assert!(line.contains("environment records: 4"), "{line}");
        assert!(line.contains("simulation time 10.0 to 13.0 s"), "{line}");
        assert!(line.contains("x from 0.00 to 0.50, y from -0.10 to 0.10"), "{line}");
        // The lines of a session are few.
        let logged = (0..40).filter(|step| watch.note(20.0 + *step as f32, [*step as f32, 0.0])).count();
        assert_eq!(logged as u32 + 2, WIND_LINES);
        assert!(!watch.note(99.0, [100.0, 0.0]));
    }

    /// A change of the near plane of more than 1 % is logged; the same value again is not, and the
    /// lines of a session are few.
    #[test]
    fn a_change_of_the_near_plane_is_logged_once() {
        let mut tier = TierViews::new(PLACES);
        assert!(tier.note_near(1.0));
        assert!(!tier.note_near(1.0));
        assert!(!tier.note_near(1.005));
        assert!(tier.note_near(2000.0));
        assert!(!tier.note_near(2000.0));
        for index in 0..40 {
            tier.note_near(10.0 * 2f32.powi(index));
        }
        assert!(!tier.note_near(1.0e9));
    }

    #[test]
    fn the_baked_light_of_the_view_group_starts_at_binding_1() {
        let code = wgsl();
        assert!(code.contains("@group(0) @binding(1) var<uniform> light_map"));
        assert!(code.contains("@group(0) @binding(4) var light_sampler"));
    }
}
