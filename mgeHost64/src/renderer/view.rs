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

/// The groups of the views of their own that get the full tier (`views.full_tier_max`, at most
/// [`TIER_VIEWS`]): the groups from [`TIER_FIRST`] on, one for each view of a frame.
pub const TIER_FIRST: usize = 5;
pub const TIER_VIEWS: usize = 4;
/// The groups of the grass of the views of their own: the projection of the game, as [`GAME`]
/// has it, one for each call at the end of the world (D82 section 3.4f, slice 2).
pub const TIER_GAME_FIRST: usize = TIER_FIRST + TIER_VIEWS;
pub const VIEWS: usize = TIER_GAME_FIRST + TIER_VIEWS;

/// The group of the `k`th view of its own in a frame (0 to [`TIER_VIEWS`] less 1).
pub const fn tier(k: usize) -> usize {
    TIER_FIRST + k
}

/// The group with the projection of the game for the `k`th call at the end of the world of a
/// view of its own in a frame (0 to [`TIER_VIEWS`] less 1).
pub const fn tier_game(k: usize) -> usize {
    TIER_GAME_FIRST + k
}

/// The group of a set of the statics. The sets 0 to 3 have the groups of the same numbers (the
/// game view between them has no set); the sets from 4 on are the views of their own, in the
/// groups from [`TIER_FIRST`].
pub const fn group_of_set(set: usize) -> usize {
    if set < GAME { set } else { set + 1 }
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

// The clip position of a position in the space of the world: relative to the eye (see
// `View::relative`).
fn clip_of(world: vec3f) -> vec4f {
    return u.relative * vec4f(world - u.eye.xyz, 1.0);
}
";

/// The code of the view group for the start of a shader: `u` and `baked_light`.
pub fn wgsl() -> String {
    format!("{VIEW_WGSL}{}", distant_lights::baked_wgsl_at(0, 1))
}

/// The views of their own that drew the full tier ([`TIER_VIEWS`] places at most): which
/// group and sets a call gets, and the counts for the report. What a view needs for its own
/// draws (its view group, its cull sets) is keyed to the PLACE of the full tier that the host
/// gives it (`ViewKey::place`), so a mirror that keeps its place keeps its group. The writes of
/// the view's values and of the cull values land before all commands of the frame (the
/// one-submit rule), so a place has one call at the start and one at the end of the world in
/// a frame: a second call of the place in the same frame would show the last call's values in
/// both, so it is refused and counted.
#[derive(Default)]
pub struct TierViews {
    /// The frame of the last call at the start and at the end of the world of each place.
    started: [Option<u64>; TIER_VIEWS],
    ended: [Option<u64>; TIER_VIEWS],
    /// The view slot that each place had in its last call, and the changes of it (a place that
    /// a view keeps does not change).
    holder: [Option<u32>; TIER_VIEWS],
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
    /// The bytes of GPU memory that the sets of the views of their own made.
    pub bytes: u64,
    /// The near plane of the last call that was logged, and how many lines were logged.
    last_near: f32,
    near_lines: u32,
}

/// The most lines of the near plane of a view of its own in a session.
const NEAR_LINES: u32 = 12;

/// The answer to a call that asks for its place.
enum Claim {
    Served(usize),
    /// The view has no place, or one above the cap of [`TIER_VIEWS`].
    Refused,
    /// The place had a call of this kind in the same frame already.
    Repeated,
}

impl TierViews {
    /// The number of the place for a call at the start of the world in `frame` (the place of the
    /// view's key, 0 to [`TIER_VIEWS`] less 1); `None` when the view has no place or one above
    /// the cap, or when the place had its call in this frame already.
    pub fn claim(&mut self, frame: u64, place: Option<u8>) -> Option<usize> {
        match Self::take(&mut self.started, frame, place) {
            Claim::Served(place) => {
                self.calls += 1;
                Some(place)
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
    pub fn claim_after(&mut self, frame: u64, place: Option<u8>) -> Option<usize> {
        match Self::take(&mut self.ended, frame, place) {
            Claim::Served(place) => {
                self.calls_after += 1;
                Some(place)
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

    fn take(frames: &mut [Option<u64>; TIER_VIEWS], frame: u64, place: Option<u8>) -> Claim {
        let Some(place) = place.map(usize::from).filter(|&place| place < TIER_VIEWS) else { return Claim::Refused };
        if frames[place] == Some(frame) {
            return Claim::Repeated;
        }
        frames[place] = Some(frame);
        Claim::Served(place)
    }

    /// Notes the view slot of a call of `place`; true when the place had another slot in its last
    /// call, or none yet (the call then logs the change).
    pub fn note_holder(&mut self, place: usize, slot: u32) -> bool {
        if self.holder[place] == Some(slot) {
            return false;
        }
        self.holder[place] = Some(slot);
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
            "views of their own: {} calls at the start of the world (land, statics, glows), {} refused (no place, or a place above {TIER_VIEWS}), {} refused for a second call of a place in a frame, {} could not draw, {} had an orthographic projection; {} land mesh draws, {} statics draw calls, {} calls drew glows; {} calls at the end of the world (grass), {} refused, {} refused for a second call of a place in a frame, {} drew {} grass places (the bound views.full_tier_max is the host's, in its full tier line); {} changes of the view that holds a place; {:.1} MB made for their sets",
            self.calls,
            self.refused,
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
            self.bytes as f64 / 1e6
        )
    }
}

pub struct ViewBinding {
    pub layout: wgpu::BindGroupLayout,
    buffers: [wgpu::Buffer; VIEWS],
    groups: Vec<wgpu::BindGroup>,
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
        let buffers = core::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some("view"), size: size_of::<View>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
        });
        ViewBinding { layout, buffers, groups: Vec::new(), generation: None }
    }

    /// Makes the groups again when the baked light has changed since they were made.
    pub fn refresh(&mut self, device: &wgpu::Device, light: &LightBinding) {
        if self.generation == Some(light.generation) {
            return;
        }
        self.generation = Some(light.generation);
        self.groups = self
            .buffers
            .iter()
            .map(|buffer| {
                let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
                entries.extend(light.entries(1));
                device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("view"), layout: &self.layout, entries: &entries })
            })
            .collect();
    }

    /// The values of a view for the frame. Each view has a buffer of its own: the writes come
    /// before all commands of the frame.
    pub fn write(&self, queue: &wgpu::Queue, view: usize, values: &View) {
        queue.write_buffer(&self.buffers[view], 0, bytemuck::bytes_of(values));
    }

    /// The group of a view; `refresh` makes the groups.
    pub fn group(&self, view: usize) -> &wgpu::BindGroup {
        &self.groups[view]
    }

    /// The groups of all views, by their numbers; empty before `refresh`.
    pub fn groups(&self) -> &[wgpu::BindGroup] {
        &self.groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_struct_has_the_size_of_its_wgsl() {
        // A matrix, twelve vectors of four numbers, and the relative matrix.
        assert_eq!(size_of::<View>(), 4 * (16 + 12 * 4 + 16));
        assert_eq!(VIEW_WGSL.matches(": vec4f,").count(), 12);
        assert_eq!(VIEW_WGSL.matches(": mat4x4f,").count(), 2);
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

    /// The sets of the statics take the groups of their views: the four fixed ones by their own
    /// numbers, the views of their own after the game view, and no two sets share a group.
    #[test]
    fn each_set_of_the_statics_has_a_group_of_its_own() {
        let groups: Vec<usize> = (0..4 + TIER_VIEWS).map(group_of_set).collect();
        assert_eq!(&groups[..4], &[MAIN, MIRROR, SHADOW[0], SHADOW[1]]);
        assert!(!groups.contains(&GAME));
        assert_eq!(groups[4], tier(0));
        assert_eq!(*groups.last().unwrap(), tier(TIER_VIEWS - 1));
        let mut sorted = groups.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), groups.len());
        assert!(groups.iter().all(|&group| group < VIEWS));
        // The groups with the game's projection for the grass of the views of their own come
        // after the groups of the sets and are all different.
        assert_eq!(tier_game(0), tier(TIER_VIEWS - 1) + 1);
        assert_eq!(tier_game(TIER_VIEWS - 1) + 1, VIEWS);
    }

    /// A call is keyed to the place of its view: the number it gets is the place, in any order of
    /// the calls, and the calls at the start and at the end of the world are counted apart.
    #[test]
    fn a_call_gets_the_number_of_its_place_in_any_order() {
        let mut tier = TierViews::default();
        assert_eq!(tier.claim(3, Some(2)), Some(2));
        assert_eq!(tier.claim_after(3, Some(0)), Some(0));
        assert_eq!(tier.claim(3, Some(0)), Some(0));
        assert_eq!(tier.claim_after(3, Some(2)), Some(2));
        // The next frame gives the same places the same numbers.
        assert_eq!(tier.claim(4, Some(0)), Some(0));
        assert_eq!(tier.claim(4, Some(2)), Some(2));
        assert_eq!((tier.calls, tier.calls_after), (4, 2));
        assert_eq!((tier.refused, tier.repeated, tier.refused_after, tier.repeated_after), (0, 0, 0, 0));
    }

    /// A view with no place, or a place above the cap, gets no call. A second call of a place in
    /// one frame is refused apart, because the place's buffers hold one call's values.
    #[test]
    fn a_call_without_a_place_or_a_second_call_of_a_place_is_refused() {
        let mut tier = TierViews::default();
        assert_eq!(tier.claim(7, None), None);
        assert_eq!(tier.claim(7, Some(TIER_VIEWS as u8)), None);
        assert_eq!(tier.claim(7, Some(1)), Some(1));
        assert_eq!(tier.claim(7, Some(1)), None);
        assert_eq!(tier.claim_after(7, Some(1)), Some(1));
        assert_eq!(tier.claim_after(7, Some(1)), None);
        assert_eq!(tier.claim_after(7, None), None);
        assert_eq!((tier.calls, tier.refused, tier.repeated), (1, 2, 1));
        assert_eq!((tier.calls_after, tier.refused_after, tier.repeated_after), (1, 1, 1));
        // The next frame serves the place again.
        assert_eq!((tier.claim(8, Some(1)), tier.claim_after(8, Some(1))), (Some(1), Some(1)));
        let line = tier.line();
        assert!(line.contains("refused") && line.contains("second call of a place"));
    }

    /// The view slot of a place is noted once: the same slot again is no change, another slot is.
    #[test]
    fn a_change_of_the_view_that_holds_a_place_is_noted() {
        let mut tier = TierViews::default();
        assert!(tier.note_holder(0, 5));
        assert!(!tier.note_holder(0, 5));
        assert!(tier.note_holder(1, 5));
        assert!(tier.note_holder(0, 6));
        assert_eq!(tier.holder_changes, 3);
        assert!(tier.line().contains("3 changes of the view that holds a place"));
    }

    /// A change of the near plane of more than 1 % is logged; the same value again is not, and the
    /// lines of a session are few.
    #[test]
    fn a_change_of_the_near_plane_is_logged_once() {
        let mut tier = TierViews::default();
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
