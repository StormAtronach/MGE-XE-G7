//! The surfaces of water volumes near the camera: shapes of the game whose material a mod
//! marked (the specular power 99998, 99997, or 100000 and more for a look slot). The renderer
//! gives them to the extension (`Extension::hold`), and they are drawn here with the shading of
//! the water in place of their own look. `XE Mod Water Volume.fx` and `renderWaterVolume` of
//! the Direct3D 9 path.
//!
//! A surface can face any way: the ripples lie in its plane and the normal of the mesh turns
//! them. Its reflection is the sky in the reflected direction, with what a march of the
//! reflected ray finds in the picture over it: the mirror picture of the sea is at the level
//! of the sea, so it cannot serve a pond on a hill or a sloped face.

use nirender_wgpu::objects::{Create, CreateView};
use std::collections::HashMap;

use nirender_wgpu::{FrameLights, HeldDraw, SceneView, wgpu};

use super::shading;
use crate::renderer::core_files::{WATER_STANDARD, WATER_VOLUME};

/// The specular power of a surface that reflects what is in the picture and the sky.
const REFLECTS_SCENE: f32 = 99998.0;
/// The specular power of a surface that reflects the sky only.
const REFLECTS_SKY: f32 = 99997.0;
/// `D3DCMP_LESSEQUAL`, the function of the far water's depth test.
const D3DCMP_LESSEQUAL: u32 = 4;
/// The specular power of look slot 0; there are 4096 slots (`WaterLooks::maxSlots`).
const SLOT_FIRST: f32 = 100000.0;
const SLOTS: f32 = 4096.0;

/// True for a material that a mod marked as the surface of a water volume.
pub fn marked(specular_power: f32) -> bool {
    specular_power == REFLECTS_SCENE
        || specular_power == REFLECTS_SKY
        || (SLOT_FIRST..SLOT_FIRST + SLOTS).contains(&specular_power)
}

/// The look of a surface beyond its colour, as a mod sets it for a slot (`MGE_WaterLookSet`):
/// the layout of `WaterLook` of `waterlook.h`, which the record of the looks carries.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Look {
    size: u32,
    flags: u32,
    /// The drift of the ripples, in the axes of the mesh, in units for each second.
    flow: [f32; 2],
    /// The speed and the size of the ripples; 1 is the standard.
    speed: f32,
    scale: f32,
    /// The light that the surface gives, 0 to 1.
    glow: f32,
    /// 1 is water; lower shows what is behind the surface.
    opacity: f32,
    /// The free values p0 to p3, and the name of a water shader of a mod.
    params: [[f32; 4]; 4],
    shader: [u8; 32],
    /// What the surface reflects in place of the sky, with [`LOOK_HAS_SKY`].
    sky: [f32; 3],
    /// Over how many units of water what is under the surface fades into the colour of deep
    /// water; 800 is the standard, 0 shows nothing.
    clarity: f32,
}

/// The flags of a look.
const LOOK_REFLECTS_SCENE: u32 = 1;
const LOOK_TINT_FROM_VERTEX: u32 = 2;
const LOOK_OPACITY_FROM_VERTEX: u32 = 4;
const LOOK_HAS_SKY: u32 = 8;

impl Look {
    /// The standard look (`standardLook` of `waterlook.cpp`).
    pub const STANDARD: Look = Look {
        size: size_of::<Look>() as u32,
        flags: LOOK_REFLECTS_SCENE,
        flow: [0.0; 2],
        speed: 1.0,
        scale: 1.0,
        glow: 0.0,
        opacity: 1.0,
        params: [[0.0; 4]; 4],
        shader: [0; 32],
        sky: [0.0; 3],
        clarity: 800.0,
    };

    /// The looks of the slots from the record of the looks (6): the version, the number of
    /// slots, then for each its number and its `WaterLook`. A look that is shorter than this
    /// layout keeps the standard values after its end. [`None`] for a record that is not one.
    pub fn read_table(bytes: &[u8]) -> Option<HashMap<u32, Look>> {
        let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        if word(0)? != 1 {
            return None;
        }
        let count = word(4)?;
        let mut table = HashMap::new();
        let mut at = 8;
        for _ in 0..count {
            let slot = word(at)?;
            let size = (word(at + 4)? as usize).max(8);
            let source = bytes.get(at + 4..at + 4 + size)?;
            let mut raw = bytemuck::bytes_of(&Look::STANDARD).to_vec();
            let length = size.min(raw.len());
            raw[..length].copy_from_slice(&source[..length]);
            table.insert(slot, bytemuck::pod_read_unaligned(&raw));
            at += 4 + size;
        }
        Some(table)
    }

    /// The look of a water subset among the distant statics: kind 1 reflects the scene and 2
    /// the sky only (flags 0x04 and 0x08 of the subset), and the look line that the generator
    /// copied from the mesh, when there is one (`WaterLooks::parse` and the load of
    /// `distantstatics.cpp`).
    pub fn of_distant(kind: u8, line: Option<&str>) -> Look {
        let mut look = line.map_or(Look::STANDARD, Look::parse);
        if kind == 2 || (line.is_none() && kind != 1) {
            look.flags &= !LOOK_REFLECTS_SCENE;
        }
        look
    }

    /// A look line, as "flow=0,-140 speed=1.2 sky=FF8040 reflect=sky": the keys of
    /// `WaterLooks::parse`. A key that it does not know is left out.
    pub fn parse(line: &str) -> Look {
        let mut look = Look::STANDARD;
        // Up to `out.len()` numbers with commas between them; a number that does not read
        // ends them.
        fn numbers(value: &str, out: &mut [f32]) {
            for (slot, part) in out.iter_mut().zip(value.split(',')) {
                match part.trim().parse::<f32>() {
                    Ok(number) => *slot = number,
                    Err(_) => return,
                }
            }
        }
        for word in line.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else { continue };
            match key.to_ascii_lowercase().as_str() {
                "flow" => numbers(value, &mut look.flow),
                "speed" => numbers(value, core::slice::from_mut(&mut look.speed)),
                "scale" => numbers(value, core::slice::from_mut(&mut look.scale)),
                "glow" => numbers(value, core::slice::from_mut(&mut look.glow)),
                "clarity" => numbers(value, core::slice::from_mut(&mut look.clarity)),
                "opacity" if value.eq_ignore_ascii_case("vertex") => look.flags |= LOOK_OPACITY_FROM_VERTEX,
                "opacity" => {
                    look.flags &= !LOOK_OPACITY_FROM_VERTEX;
                    numbers(value, core::slice::from_mut(&mut look.opacity));
                }
                "sky" => {
                    let digits = value.strip_prefix('#').unwrap_or(value);
                    match (digits.len() == 6).then(|| u32::from_str_radix(digits, 16).ok()).flatten() {
                        Some(colour) => {
                            look.sky = [
                                (colour >> 16 & 255) as f32 / 255.0,
                                (colour >> 8 & 255) as f32 / 255.0,
                                (colour & 255) as f32 / 255.0,
                            ]
                        }
                        None => numbers(value, &mut look.sky),
                    }
                    look.flags |= LOOK_HAS_SKY;
                }
                "reflect" if value.eq_ignore_ascii_case("sky") => look.flags &= !LOOK_REFLECTS_SCENE,
                "shader" => {
                    look.shader = [0; 32];
                    for (slot, byte) in look.shader.iter_mut().zip(value.bytes().take(31)) {
                        *slot = byte;
                    }
                }
                key if key.len() == 2 && key.starts_with('p') && (b'0'..=b'3').contains(&key.as_bytes()[1]) => {
                    numbers(value, &mut look.params[(key.as_bytes()[1] - b'0') as usize]);
                }
                _ => {}
            }
        }
        look
    }

    /// The name of the water shader of a mod that the look names, in small letters; [`None`] for
    /// the standard look.
    pub fn shader_name(&self) -> Option<String> {
        let length = self.shader.iter().position(|&b| b == 0).unwrap_or(self.shader.len());
        let name = String::from_utf8_lossy(&self.shader[..length]).trim().to_ascii_lowercase();
        (!name.is_empty()).then_some(name)
    }

    /// The look of a marked surface: that of its slot, or the standard look; 99997 reflects
    /// the sky only.
    pub fn of(specular_power: f32, table: &HashMap<u32, Look>) -> Look {
        if specular_power == REFLECTS_SKY {
            return Look {
                flags: Look::STANDARD.flags & !LOOK_REFLECTS_SCENE,
                ..Look::STANDARD
            };
        }
        if specular_power >= SLOT_FIRST {
            if let Some(look) = table.get(&((specular_power - SLOT_FIRST) as u32)) {
                return *look;
            }
        }
        Look::STANDARD
    }
}

/// A water subset among the distant statics at one place, for a frame.
pub struct FarWater {
    pub vertices: wgpu::Buffer,
    pub indexes: wgpu::Buffer,
    pub triangles: u32,
    /// The world matrix of the place, row after row.
    pub world: [f32; 16],
    /// The look of the mesh: its look line, or the standard look (the sky only for a subset
    /// of that kind).
    pub look: Look,
    /// The place is in a cell that the game has loaded: the game draws the mesh near the
    /// camera, and this one is left out there.
    pub loaded: bool,
    /// The base texture of the baked mesh, for a water shader of a mod.
    pub texture: Option<wgpu::TextureView>,
}

/// What the frame says about the look of the surfaces.
pub struct VolumeLook {
    /// How far from the camera a surface reflects what is in the picture
    /// (`distant_land.water.volume_reflection_cells`, in units); 0 for the sky only.
    pub reflect_range: f32,
    /// A colour in place of the sky, and how much of it: the light of the room in an
    /// interior, as the water of the cell has it.
    pub sky: [f32; 4],
    /// The view and the projection of the distant land, relative to the eye (`view::relative`),
    /// for the water among the distant statics, and the view range of the game.
    pub land_clip: [f32; 16],
    pub view_range: f32,
}

/// The light of the frame on the surfaces: each part is on when its setting is on and the
/// frame has it.
pub struct VolumeLight<'a> {
    /// The shadow map of the frame (`shadow.rs`): the atlas, the matrices of its two layers from
    /// the world (the layout of Direct3D), and 1 over its resolution.
    pub shadow: Option<(&'a wgpu::TextureView, [[f32; 16]; 2], f32)>,
    /// The values of the shadow: the built-in ones or those of a core mod.
    pub shadow_values: crate::renderer::core_mods::ShadowValues,
    /// The lights of the frame for each pixel.
    pub lights: Option<&'a FrameLights<'a>>,
    /// The strength of the caustics under the surfaces (`water::caustic_strength`); 0 for none.
    pub caustics: f32,
    /// The direction that the light of the sun goes in.
    pub sun_direction: [f32; 3],
}

/// The values of [`Lighting`] in the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Lighting {
    shadow: [[f32; 16]; 2],
    esm: [f32; 4],
    sun: [f32; 4],
    origin: [f32; 4],
}

/// The values of one surface for the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Surface {
    /// The world matrix of the mesh, in the space of the world of the game.
    world: [f32; 16],
    /// The colour of the water (the emissive colour of the material; black for the usual
    /// colour), and 1 when the surface reflects what is in the picture.
    tint: [f32; 4],
    /// The point that the ripples of a sloped surface turn about: the place of the mesh.
    anchor: [f32; 4],
    /// A colour in place of the sky, and how much of it.
    sky: [f32; 4],
    /// The drift of the ripples in the world (x, y), their speed and their size.
    flow: [f32; 4],
    /// The glow, the opacity and the clarity of the look.
    look: [f32; 3],
    /// Bits: 1 the mesh has normals, 2 it has vertex colours, 4 the vertex colour tints the
    /// water, 8 the vertex alpha is the opacity, 16 the mesh is drawn without the depth test.
    flags: u32,
    /// The view and the projection: those of the game, or those of the distant land for
    /// water among the distant statics.
    clip: [f32; 16],
    /// Water among the distant statics: 1; the view depth before which it is left out (the
    /// game draws the mesh there), and how far it reflects what is in the picture.
    far: [f32; 4],
    /// The free values p0 to p3 of the look, for a water shader.
    params: [[f32; 4]; 4],
}

/// The space of one surface in the buffer of the surfaces: a uniform offset is a multiple
/// of 256 bytes.
const SLOT: u64 = 512;

/// The shader of the surfaces with the water shader of the look: the shared shading, the
/// entry points of a surface, the function `water_shader`, and the shadow hooks of the session
/// (`core_mods.rs`).
fn shader_with(water: &str, shadow_hooks: &str) -> String {
    format!(
        "{}{}{}{}{}{}{}{}{}",
        super::super::view::wgsl(),
        nirender_wgpu::depth::wgsl(),
        shading::common(),
        shading::water_normal(),
        shading::caustics(),
        WATER_VOLUME.part("volume"),
        nirender_wgpu::spot::WGSL,
        water,
        shadow_hooks
    )
    .replace("LIGHT_REACH_MAX", &format!("{:?}", nirender_wgpu::LIGHT_REACH_MAX))
}

/// The shader of the surfaces with the standard look: the standard shading and nothing more
/// (`XE Mod Water Standard.fx`).
pub(super) fn standard_source(shadow_hooks: &str) -> String {
    shader_with(WATER_STANDARD.part("standard"), shadow_hooks)
}

/// The water shader of a mod with the built-in hooks, for a test.
#[cfg(test)]
fn test_water_shader(file: &str) -> Result<String, String> {
    water_shader_source("test", file, crate::renderer::core_mods::CoreMods::built_in().shadow_hooks())
}

/// The shader of the surfaces with the standard look and the built-in hooks, for a test.
#[cfg(test)]
fn shader() -> String {
    standard_source(crate::renderer::core_mods::CoreMods::built_in().shadow_hooks())
}

/// The source of the water shader `name` of a mod (`Data Files/shaders/water/<name>.wgsl`),
/// checked as the device checks it. The file starts with the line
/// `//! interface: mge-xe-water-wgsl 1`; the rest is WGSL with the function `water_shader`
/// (`docs/water-shaders.md`).
pub fn water_shader_source(name: &str, file: &str, shadow_hooks: &str) -> Result<String, String> {
    let mut interface = false;
    for line in file.lines() {
        let Some(rest) = line.strip_prefix("//!") else { break };
        let Some((key, value)) = rest.split_once(':') else {
            return Err(format!("the line `{line}` has no `key: value`"));
        };
        match key.trim() {
            "interface" => interface = value.trim() == "mge-xe-water-wgsl 1",
            other => return Err(format!("the key `{other}` is not known")),
        }
    }
    if !interface {
        return Err("no line `//! interface: mge-xe-water-wgsl 1` at its start".to_string());
    }
    let source = shader_with(file, shadow_hooks);
    crate::renderer::shader_check::check(name, &source)?;
    Ok(source)
}

/// What makes the pipelines of two surfaces differ: the vertex layout, the topology and the
/// fill.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    stride: u64,
    position: u64,
    normal: Option<u64>,
    color: Option<u64>,
    topology: wgpu::PrimitiveTopology,
    lines: bool,
    /// The vertex of a distant static, and the depth tests in the shader.
    far: bool,
    /// Drawn only outside the dry spaces (`dry::outside`).
    cut: bool,
    /// The depth only, for the count of the dry spaces behind it.
    depth_only: bool,
    /// The mesh is drawn with the depth test; without it, it is drawn whatever the depth is,
    /// as it asks.
    depth_test: bool,
    /// The mesh writes its depth, as the game's z-buffer property asks: independent of the test.
    depth_write: bool,
    /// The function of the depth test (`D3DCMPFUNC`); it counts with `depth_test` on.
    depth_function: u32,
    /// The stencil test of the mesh, when it has one: the function, the mask, and the
    /// operations on a failed test, a failed depth test and a pass (Direct3D values). The
    /// reference is set for each draw.
    stencil: Option<[u32; 5]>,
    /// The offsets of the two sets of texture coordinates (the first again for a mesh with
    /// one set); none without coordinates.
    coordinates: [Option<u64>; 2],
    /// The water shader: 0 the standard one, else one of a mod ([`Volumes::modules`]).
    shader: u16,
    /// The debug view `water` is on: the module of the shader with `WATER_DEBUG`
    /// ([`Volumes::set_debug`]).
    debug: bool,
}

/// The depth state of a held shape as the game's z-buffer property asks for it: the test, the
/// write, and the function (see [`Key`]). The test and the write are separate: the game writes
/// the depth of a shape by its own flag, with the test off too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DepthState {
    test: bool,
    write: bool,
    function: u32,
}

/// The state of [`HeldDraw::depth`]: on, write, function (`D3DCMPFUNC`).
fn depth_state(depth: [u32; 3]) -> DepthState {
    // D3DCMP_ALWAYS is 8: the game turns the depth test off that way too.
    DepthState {
        test: depth[0] != 0 && depth[2] != 8,
        write: depth[1] != 0,
        function: depth[2],
    }
}

/// The function of a surface in the pass after its depth-only pass (`D3DCMPFUNC`): the surface
/// must also pass at the depth that it wrote itself. Where that pass failed, the depth differs
/// from the surface's own, and the answer is the one that the first test gave. One known case
/// differs: where the buffer already had the surface's own depth, LESS, GREATER and NOTEQUAL
/// failed the first test and pass here, so the surface draws there.
fn function_after_depth_pass(function: u32) -> u32 {
    match function {
        2 => 4, // LESS: LESSEQUAL
        5 => 7, // GREATER: GREATEREQUAL
        6 => 3, // NOTEQUAL: EQUAL
        other => other,
    }
}

/// The tests that a held shape asks for: the depth test, and its own stencil test (see [`Key`]).
fn tests_of(held: &HeldDraw) -> (bool, Option<[u32; 5]>) {
    let s = held.stencil;
    (
        depth_state(held.depth).test,
        (s[0] != 0).then_some([s[1], s[3], s[4], s[5], s[6]]),
    )
}

pub(super) struct Volumes {
    /// The shader with each water shader: the standard one first, then those of mods, by
    /// their names in `shaders`. A name whose file is missing or fails has the standard one.
    modules: Vec<wgpu::ShaderModule>,
    /// The text of each module, for its module of the debug view `water`.
    sources: Vec<String>,
    /// The modules of the debug view, by their shader, made at their first draw while the view
    /// is on, and dropped with the view. None inside: the text has no one `WATER_DEBUG` line.
    debug_modules: HashMap<u16, Option<wgpu::ShaderModule>>,
    /// The debug view `water` is on in this frame.
    debug: bool,
    shaders: HashMap<String, u16>,
    /// The core mods of the session (`core_mods.rs`): the shadow hooks of the standard shader at
    /// its first pipeline, and of the shaders of mods.
    core_mods: crate::renderer::core_mods::Shared,
    /// The standard shader has had its first pipeline, the first build of its home.
    standard_built: bool,
    /// The shadow hooks of the standard module made at the start: the first build reuses that
    /// module when its hooks are the same.
    standard_hooks: String,
    /// The group of the textures of the mesh, its sampler, and a group of white textures for
    /// the standard look.
    mesh_layout: wgpu::BindGroupLayout,
    mesh_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    clear: wgpu::TextureView,
    plain_mesh: wgpu::BindGroup,
    /// The mesh groups of the water shaders of mods in this frame, by their two textures.
    mesh_groups: HashMap<(wgpu::TextureView, wgpu::TextureView), wgpu::BindGroup>,
    pipeline_layout: wgpu::PipelineLayout,
    surface_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    /// The values of the surfaces of a frame, one slot each, and their group.
    surfaces: wgpu::Buffer,
    capacity: u64,
    /// The slots that the draws of this frame have used: a frame can draw surfaces more than
    /// once (`draw_held`, and a surface in its place), and each draw writes its own slots, since
    /// the writes come before the commands of the frame.
    used: u64,
    group: wgpu::BindGroup,
    /// Zeros for the inputs that a vertex does not have.
    defaults: wgpu::Buffer,
    /// The light of the frame ([`VolumeLight`]) in the water group: its values, and what the
    /// group has in place of a part that the frame does not have: a texture of one texel, and
    /// empty lights and clusters.
    lighting: wgpu::Buffer,
    no_shadow: wgpu::TextureView,
    no_lights: [wgpu::Buffer; 4],
    target_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    /// The samples of the game pass. Every pipeline here draws into that pass.
    multisample: wgpu::MultisampleState,
    /// The near surfaces and the far water drawn since the start.
    pub drawn: u64,
    pub far_drawn: u64,
}

impl Volumes {
    /// `view_layout` is that of the view group (`view.rs`), `frame_layout` that of the group
    /// of the shared shading (`Water::layout`, with `light_layout_entries`).
    /// `core_mods`: the core mods of the session, whose shadow hooks join every shader of the
    /// surfaces.
    pub fn new(
        scene: &SceneView,
        view_layout: &wgpu::BindGroupLayout,
        frame_layout: &wgpu::BindGroupLayout,
        core_mods: &crate::renderer::core_mods::Shared,
    ) -> Volumes {
        let device = scene.device;
        let standard_hooks = core_mods.shadow_hooks();
        let source = standard_source(&standard_hooks);
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water volume"),
            source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
        });
        let mesh_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water volume mesh"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let mesh_sampler = device.counted_sampler(&wgpu::SamplerDescriptor {
            label: Some("water volume mesh"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let surface_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water volume"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Surface>() as u64),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water volume"),
            bind_group_layouts: &[
                Some(view_layout),
                Some(frame_layout),
                Some(&mesh_layout),
                Some(&surface_layout),
            ],
            immediate_size: 0,
        });
        let lighting = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("water volume light"),
            size: size_of::<Lighting>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let no_shadow = device
            .counted_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("water volume no shadow"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[0u8; 2],
            )
            .counted_view(&Default::default());
        // A light of zeros, clusters that are off, one empty cluster and one index.
        let zeros = |label, size: usize, usage| {
            device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &vec![0u8; size],
                usage,
            })
        };
        let no_lights = [
            zeros(
                "water volume no lights",
                size_of::<nirender_wgpu::LightData>(),
                wgpu::BufferUsages::STORAGE,
            ),
            zeros("water volume no clusters", 48, wgpu::BufferUsages::UNIFORM),
            zeros("water volume no cluster table", 8, wgpu::BufferUsages::STORAGE),
            zeros("water volume no cluster lights", 4, wgpu::BufferUsages::STORAGE),
        ];
        let one_texel = |label, texel: [u8; 4]| {
            device
                .counted_texture_with_data(
                    scene.queue,
                    &wgpu::TextureDescriptor {
                        label: Some(label),
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
                    &texel,
                )
                .counted_view(&Default::default())
        };
        let white = one_texel("water volume white", [255; 4]);
        let clear = one_texel("water volume clear", [0; 4]);
        let plain_mesh = Self::mesh_group_of(device, &mesh_layout, &mesh_sampler, &white, &clear);
        let (surfaces, group) = Self::slots(device, &surface_layout, 16);
        let defaults = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("water volume defaults"),
            size: 32,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        Volumes {
            modules: vec![module],
            sources: vec![source],
            debug_modules: HashMap::new(),
            debug: false,
            shaders: HashMap::new(),
            core_mods: core_mods.clone(),
            standard_built: false,
            standard_hooks,
            mesh_layout,
            mesh_sampler,
            white,
            clear,
            plain_mesh,
            mesh_groups: HashMap::new(),
            pipeline_layout,
            surface_layout,
            pipelines: HashMap::new(),
            surfaces,
            capacity: 16,
            used: 0,
            group,
            defaults,
            lighting,
            no_shadow,
            no_lights,
            target_format: scene.target_format,
            depth_format: scene.depth_format,
            multisample: scene.multisample(false),
            drawn: 0,
            far_drawn: 0,
        }
    }

    /// The entries of the light of the frame in the layout of the water group: bindings 11 to
    /// 16, for the fragments.
    pub fn light_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        [
            entry(11, uniform),
            entry(
                12,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            entry(13, storage),
            entry(14, uniform),
            entry(15, storage),
            entry(16, storage),
        ]
    }

    /// The entries of the light of the frame in the water group (`light_layout_entries`).
    /// With a light, its values are written; without one (the plane, which reads none of it),
    /// the group has the texture of one texel and the empty lights.
    pub fn light_entries<'a>(&'a self, scene: &SceneView, light: Option<&VolumeLight<'a>>) -> [wgpu::BindGroupEntry<'a>; 6] {
        let [no_lights, no_clusters, no_table, no_indexes] = &self.no_lights;
        let (atlas, buffers) = match light {
            Some(light) => {
                self.write_light(scene, light);
                (
                    light.shadow.map_or(&self.no_shadow, |(atlas, ..)| atlas),
                    light.lights.map_or([no_lights, no_clusters, no_table, no_indexes], |l| {
                        [l.lights, l.clusters, l.table, l.indexes]
                    }),
                )
            }
            None => (&self.no_shadow, [no_lights, no_clusters, no_table, no_indexes]),
        };
        [
            wgpu::BindGroupEntry {
                binding: 11,
                resource: self.lighting.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::TextureView(atlas),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: buffers[0].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: buffers[1].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: buffers[2].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: buffers[3].as_entire_binding(),
            },
        ]
    }

    /// Writes the values of the light of the frame.
    fn write_light(&self, scene: &SceneView, light: &VolumeLight) {
        let mut values = <Lighting as bytemuck::Zeroable>::zeroed();
        let mut bits = 0;
        if let Some((_, layers, rcp)) = light.shadow {
            let eye = scene.eye.map(|v| v as f32);
            values.shadow = layers.map(|layer| super::super::view::relative_of_product(&layer, eye));
            values.esm = light.shadow_values.esm(rcp);
            bits |= 1;
        }
        if let Some(lights) = light.lights {
            values.origin = [lights.origin[0] as f32, lights.origin[1] as f32, lights.origin[2] as f32, 0.0];
            bits |= 2;
        }
        values.origin[3] = bits as f32;
        let d = light.sun_direction;
        values.sun = [d[0], d[1], d[2], light.caustics];
        scene.queue.write_buffer(&self.lighting, 0, bytemuck::bytes_of(&values));
    }

    /// The start of a frame: no slots are used.
    pub fn start_frame(&mut self) {
        self.used = 0;
        self.mesh_groups.clear();
    }

    /// True when the water shader of this name has been asked for before.
    pub fn knows_shader(&self, name: &str) -> bool {
        self.shaders.contains_key(name)
    }

    /// Adds the water shader of a mod by its name, from its file ([`None`]: there is none). A
    /// shader that fails has the standard look, and the reason is returned.
    pub fn add_shader(&mut self, device: &wgpu::Device, name: &str, file: Option<&str>) -> Result<(), String> {
        let result = file
            .ok_or_else(|| "no file shaders/water/<name>.wgsl".to_string())
            .and_then(|file| water_shader_source(name, file, &self.core_mods.shadow_hooks()));
        let index = match &result {
            Ok(source) => {
                let label = format!("water shader {name}");
                self.modules.push(device.counted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&label),
                    source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
                }));
                self.sources.push(source.clone());
                (self.modules.len() - 1) as u16
            }
            Err(_) => 0,
        };
        self.shaders.insert(name.to_string(), index);
        result.map(|_| ())
    }

    /// The water shader of a look.
    fn shader_of(&self, look: &Look) -> u16 {
        look.shader_name()
            .and_then(|name| self.shaders.get(&name).copied())
            .unwrap_or(0)
    }

    fn mesh_group_of(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        base: &wgpu::TextureView,
        second: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        nirender_wgpu::gpu::frame_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("water volume mesh"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(second),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            },
        )
    }

    /// The textures of a held mesh for a water shader: its base map, and the first of its
    /// other maps (a dark, detail or decal map).
    fn mesh_group(&mut self, device: &wgpu::Device, held: &HeldDraw) -> wgpu::BindGroup {
        let base = held
            .maps
            .iter()
            .find(|m| m.map == nirender_wgpu::material::BASE)
            .map_or(&self.white, |m| &*m.texture)
            .clone();
        let second = held
            .maps
            .iter()
            .filter(|m| m.map != nirender_wgpu::material::BASE)
            .min_by_key(|m| m.map)
            .map_or(&self.clear, |m| &*m.texture)
            .clone();
        self.cached_mesh_group(device, base, second)
    }

    /// The mesh group of two textures, made once in a frame.
    fn cached_mesh_group(
        &mut self,
        device: &wgpu::Device,
        base: wgpu::TextureView,
        second: wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let (layout, sampler) = (&self.mesh_layout, &self.mesh_sampler);
        self.mesh_groups
            .entry((base, second))
            .or_insert_with_key(|(base, second)| Self::mesh_group_of(device, layout, sampler, base, second))
            .clone()
    }

    /// A buffer of `count` slots and its group.
    fn slots(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, count: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("water volume surfaces"),
            size: count * SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water volume surfaces"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<Surface>() as u64),
                }),
            }],
        });
        (buffer, group)
    }

    /// The debug view `water` in this frame. A frame without it drops the modules and the
    /// pipelines of the view.
    pub fn set_debug(&mut self, on: bool) {
        if on == self.debug {
            return;
        }
        self.debug = on;
        if !on {
            self.debug_modules.clear();
            self.pipelines.retain(|key, _| !key.debug);
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: Key) -> &wgpu::RenderPipeline {
        let (layout, target_format, depth_format, multisample) = (
            self.pipeline_layout.clone(),
            self.target_format,
            self.depth_format,
            self.multisample,
        );
        let make = |module: &wgpu::ShaderModule, source: &str| {
            Self::make_pipeline(device, key, module, source, &layout, target_format, depth_format, multisample)
        };
        // The first pipeline of the standard shader is the first build of its home (W317), under
        // the guard of the core mods, with the shadow hooks of the session now. The module of the
        // start serves when those hooks did not change since; else a new module is made. The
        // water shaders of mods stay outside it.
        if key.shader == 0 && !key.debug && !self.standard_built {
            self.standard_built = true;
            let core_mods = self.core_mods.clone();
            let (start_hooks, start_module, start_source) = (&self.standard_hooks, &self.modules[0], &self.sources[0]);
            let (module, source, pipeline) = core_mods.first_build(
                device,
                crate::renderer::core_mods::WATER_VOLUME_HOME,
                crate::renderer::core_mods::ModFile::Shadow,
                |shadow_hooks| {
                    let (module, source) = if shadow_hooks == start_hooks {
                        (start_module.clone(), start_source.clone())
                    } else {
                        let source = standard_source(shadow_hooks);
                        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
                            label: Some("water volume"),
                            source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
                        });
                        (module, source)
                    };
                    let pipeline = make(&module, &source);
                    (module, source, pipeline)
                },
            );
            self.modules[0] = module;
            self.sources[0] = source;
            self.debug_modules.remove(&0);
            self.pipelines.retain(|kept, _| kept.shader != 0);
            self.pipelines.insert(key, pipeline);
        }
        if key.debug && !self.debug_modules.contains_key(&key.shader) {
            let text = shading::debug_source(&self.sources[key.shader as usize]);
            let module = text.map(|text| {
                device.counted_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("water volume, debug view"),
                    source: wgpu::ShaderSource::Wgsl(text.into()),
                })
            });
            self.debug_modules.insert(key.shader, module);
        }
        // A shader whose text has no debug line draws the view with its own module.
        let debug_module = key
            .debug
            .then(|| self.debug_modules.get(&key.shader).and_then(Option::as_ref))
            .flatten();
        let module = debug_module.unwrap_or(&self.modules[key.shader as usize]);
        let source = &self.sources[key.shader as usize];
        self.pipelines.entry(key).or_insert_with(|| make(module, source))
    }

    /// The pipeline of `key` with `module`, whose text is `source`.
    #[allow(clippy::too_many_arguments)]
    fn make_pipeline(
        device: &wgpu::Device,
        key: Key,
        module: &wgpu::ShaderModule,
        source: &str,
        layout: &wgpu::PipelineLayout,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        multisample: wgpu::MultisampleState,
    ) -> wgpu::RenderPipeline {
        let fragment_entry = |entry: &'static str| crate::renderer::core_mods::fragment_entry(source, entry);
        {
            let _long = nirender_wgpu::LongStep::begin();
            if key.far {
                // A distant static: the position with the palette entry, the normal with the
                // light of the material, the colour, the coordinate (statics.rs).
                let attributes = wgpu::vertex_attr_array![0 => Float16x4, 1 => Unorm8x4, 2 => Unorm8x4, 3 => Float16x2];
                return device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("water volume, far"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_far"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 20,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        polygon_mode: if key.lines {
                            wgpu::PolygonMode::Line
                        } else {
                            wgpu::PolygonMode::Fill
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: depth_format,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample,
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(fragment_entry("fs")),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: target_format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::COLOR,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            }
            let mut attributes = vec![wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: key.position,
                shader_location: 0,
            }];
            let mut missing = Vec::new();
            match key.normal {
                Some(normal) => attributes.push(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: normal,
                    shader_location: 1,
                }),
                None => missing.push(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 1,
                }),
            }
            match key.color {
                Some(color) => attributes.push(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4,
                    offset: color,
                    shader_location: 2,
                }),
                None => missing.push(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4,
                    offset: 16,
                    shader_location: 2,
                }),
            }
            for (set, location) in key.coordinates.iter().zip([3, 4]) {
                match set {
                    Some(offset) => attributes.push(wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: *offset,
                        shader_location: location,
                    }),
                    None => missing.push(wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 24,
                        shader_location: location,
                    }),
                }
            }
            device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water volume"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: key.stride,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 32,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &missing,
                        }),
                    ],
                },
                // Both sides: a surface is seen from under the water too.
                primitive: wgpu::PrimitiveState {
                    topology: key.topology,
                    cull_mode: None,
                    polygon_mode: if key.lines {
                        wgpu::PolygonMode::Line
                    } else {
                        wgpu::PolygonMode::Fill
                    },
                    ..Default::default()
                },
                // The depth as the game's z-buffer property asks: the function when the test is
                // on, and the write by its own flag. A mesh that tests nothing and writes (a
                // well's water under the ground, which only its mask shows) writes its depth
                // where it draws.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(key.depth_write),
                    depth_compare: Some(if key.depth_test {
                        nirender_wgpu::depth::compare(nirender_wgpu::d3d_compare(key.depth_function))
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: match key.stencil {
                        // The test of the mesh, without the bit of the dry spaces.
                        Some([function, mask, fail, depth_fail, pass]) => {
                            let face = wgpu::StencilFaceState {
                                compare: nirender_wgpu::d3d_compare(function),
                                fail_op: nirender_wgpu::d3d_stencil_operation(fail),
                                depth_fail_op: nirender_wgpu::d3d_stencil_operation(depth_fail),
                                pass_op: nirender_wgpu::d3d_stencil_operation(pass),
                            };
                            let others = 0xFF & !super::dry::DRY_BIT;
                            wgpu::StencilState {
                                front: face,
                                back: face,
                                read_mask: mask & others,
                                write_mask: others,
                            }
                        }
                        None if key.cut => super::dry::outside(),
                        None => Default::default(),
                    },
                    bias: Default::default(),
                }),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment_entry(if key.depth_only { "fs_depth" } else { "fs" })),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: if key.depth_only {
                            wgpu::ColorWrites::empty()
                        } else {
                            wgpu::ColorWrites::COLOR
                        },
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }
    }

    /// Draws the water among the distant statics, then the marked surfaces among `held`,
    /// with the group of the shared shading of the frame. The far water writes no depth and
    /// the near surfaces do: they go over it. Returns the number drawn.
    /// With `dry`, the dry spaces cut the near surfaces: they write their depth first, the
    /// faces of the dry spaces behind it are counted, and then they are drawn outside
    /// (`flushWaterVolumes`). The far water is not cut. `before_plane`: the sea plane of the
    /// frame comes after these surfaces.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        scene: &SceneView,
        pass: &mut wgpu::RenderPass<'static>,
        view: &wgpu::BindGroup,
        frame_group: &wgpu::BindGroup,
        held: &[HeldDraw],
        far: &[FarWater],
        frame_look: &VolumeLook,
        looks: &HashMap<u32, Look>,
        dry: Option<&super::dry::Dry>,
        before_plane: bool,
    ) -> usize {
        let surfaces: Vec<&HeldDraw> = held.iter().filter(|h| marked(h.facts.specular_power)).collect();
        if surfaces.is_empty() && far.is_empty() {
            return 0;
        }
        let count = (far.len() + surfaces.len()) as u64;
        if self.used + count > self.capacity {
            // A new buffer: the draws before in the frame keep the old one.
            self.capacity = (self.used + count).next_power_of_two();
            (self.surfaces, self.group) = Self::slots(scene.device, &self.surface_layout, self.capacity);
            self.used = 0;
        }
        let base = self.used;
        self.used += count;
        let mut bytes = vec![0u8; (count * SLOT) as usize];
        // The clip matrices are relative to the eye and made in f64: the product of an absolute position with a matrix of
        // the world rounds the rows of the clip position apart far from the origin (the grass of the statics had it).
        let game_clip = crate::renderer::view::relative(
            &scene.view,
            &nirender_wgpu::depth::gpu_projection(&scene.projection),
            scene.eye.map(|v| v as f32),
        );
        for (index, water) in far.iter().enumerate() {
            let look = &water.look;
            let world = water.world;
            let drift = [
                look.flow[0] * world[0] + look.flow[1] * world[4],
                look.flow[0] * world[1] + look.flow[1] * world[5],
            ];
            // The alpha of the vertex is the opacity when the look asks for it; its colour is
            // the colour of the water.
            let bits = 1 | 2 | (((look.flags & LOOK_OPACITY_FROM_VERTEX != 0) as u32) << 3);
            let surface = Surface {
                world,
                tint: [
                    0.0,
                    0.0,
                    0.0,
                    if look.flags & LOOK_REFLECTS_SCENE != 0 && frame_look.reflect_range > 0.0 {
                        1.0
                    } else {
                        0.0
                    },
                ],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: if look.flags & LOOK_HAS_SKY != 0 {
                    [look.sky[0], look.sky[1], look.sky[2], 1.0]
                } else {
                    frame_look.sky
                },
                flow: [drift[0], drift[1], look.speed, look.scale],
                look: [look.glow, look.opacity, look.clarity],
                flags: bits,
                clip: frame_look.land_clip,
                far: [
                    1.0,
                    if water.loaded { frame_look.view_range } else { 0.0 },
                    frame_look.reflect_range,
                    0.0,
                ],
                params: look.params,
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        for (index, held) in surfaces.iter().enumerate() {
            let index = far.len() + index;
            // The world of a draw can be relative to an origin; the water is in the space of
            // the world of the game.
            let mut world = held.data.world[0];
            if let Some(origin) = held.origin {
                for axis in 0..3 {
                    world[12 + axis] = (world[12 + axis] as f64 + origin[axis]) as f32;
                }
            }
            let look = Look::of(held.facts.specular_power, looks);
            let reflects = look.flags & LOOK_REFLECTS_SCENE != 0 && frame_look.reflect_range >= 1.0;
            let emissive = held.facts.emissive;
            // The drift is given in the axes of the mesh: turned into the world.
            let drift = [
                look.flow[0] * world[0] + look.flow[1] * world[4],
                look.flow[0] * world[1] + look.flow[1] * world[5],
            ];
            let has_color = held.layout.color.is_some();
            let bits = held.layout.normal.is_some() as u32
                | (has_color as u32) << 1
                | ((has_color && look.flags & LOOK_TINT_FROM_VERTEX != 0) as u32) << 2
                | ((has_color && look.flags & LOOK_OPACITY_FROM_VERTEX != 0) as u32) << 3
                | (!tests_of(held).0 as u32) << 4;
            let surface = Surface {
                world,
                tint: [emissive[0], emissive[1], emissive[2], if reflects { 1.0 } else { 0.0 }],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: if look.flags & LOOK_HAS_SKY != 0 {
                    [look.sky[0], look.sky[1], look.sky[2], 1.0]
                } else {
                    frame_look.sky
                },
                flow: [drift[0], drift[1], look.speed, look.scale],
                look: [look.glow, look.opacity, look.clarity],
                flags: bits,
                clip: game_clip,
                far: [0.0; 4],
                params: look.params,
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        scene.queue.write_buffer(&self.surfaces, base * SLOT, &bytes);
        let lines = scene.wireframe && scene.device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        // The view and the water group with the light of the frame: the same for all surfaces
        // and for the dry spaces, whose pipelines have these two groups.
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, frame_group, &[]);
        if !far.is_empty() {
            for (index, water) in far.iter().enumerate() {
                let shader = self.shader_of(&water.look);
                let key = Key {
                    stride: 20,
                    position: 0,
                    normal: None,
                    color: None,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    lines,
                    far: true,
                    cut: false,
                    depth_only: false,
                    depth_test: true,
                    // The near surfaces go over the far water.
                    depth_write: false,
                    depth_function: D3DCMP_LESSEQUAL,
                    stencil: None,
                    coordinates: [None; 2],
                    shader,
                    debug: self.debug,
                };
                pass.set_pipeline(self.pipeline(scene.device, key));
                // A water shader of a mod has the base texture of the baked mesh.
                let mesh = match (&water.texture, shader) {
                    (Some(texture), 1..) => self.cached_mesh_group(scene.device, texture.clone(), self.clear.clone()),
                    _ => self.plain_mesh.clone(),
                };
                pass.set_bind_group(2, &mesh, &[]);
                pass.set_bind_group(3, &self.group, &[((base + index as u64) * SLOT) as u32]);
                pass.set_vertex_buffer(0, water.vertices.slice(..));
                pass.set_index_buffer(water.indexes.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..water.triangles * 3, 0, 0..1);
            }
            self.far_drawn += far.len() as u64;
        }
        let dry = dry.filter(|_| !surfaces.is_empty());
        // The passes over the near surfaces: with the dry spaces, the depth, then the count,
        // then the surfaces outside them.
        let passes: &[(bool, bool)] = if dry.is_some() {
            &[(true, false), (false, true)]
        } else {
            &[(false, false)]
        };
        for &(depth_only, cut) in passes {
            if cut {
                if let Some(dry) = dry {
                    dry.zero(pass);
                    dry.count_behind(pass);
                }
            }
            for (index, held) in surfaces.iter().enumerate() {
                let index = far.len() + index;
                let layout = &held.layout;
                // A surface with a stencil test of its own is not cut by the dry spaces, and one
                // without the depth test has no depth to count behind.
                let (depth_test, stencil) = tests_of(held);
                if depth_only && (stencil.is_some() || !depth_test) {
                    continue;
                }
                // The pass that counts the dry spaces behind the surface writes its depth
                // whatever the flag says: with dry spaces a surface that asks for no write
                // still leaves its depth in this frame.
                // Before the sea plane, a surface that wrote no depth would be under the plane.
                let depth = depth_state(held.depth);
                let depth_write = depth_only || depth.write || before_plane;
                let cut = cut && stencil.is_none();
                let depth_function = if cut {
                    function_after_depth_pass(depth.function)
                } else {
                    depth.function
                };
                let shader = if depth_only {
                    0
                } else {
                    self.shader_of(&Look::of(held.facts.specular_power, looks))
                };
                let first = layout.coordinates.first().copied();
                let coordinates = [first, layout.coordinates.get(1).copied().or(first)];
                let key = Key {
                    stride: layout.stride,
                    position: layout.position,
                    normal: layout.normal,
                    color: layout.color,
                    topology: held.topology,
                    lines: lines && !cut && !depth_only,
                    far: false,
                    cut,
                    depth_only,
                    depth_test,
                    depth_write,
                    depth_function,
                    stencil,
                    coordinates,
                    shader,
                    debug: self.debug,
                };
                pass.set_pipeline(self.pipeline(scene.device, key));
                let mesh = if shader == 0 {
                    self.plain_mesh.clone()
                } else {
                    self.mesh_group(scene.device, held)
                };
                pass.set_bind_group(2, &mesh, &[]);
                pass.set_stencil_reference(if stencil.is_some() { held.stencil[2] } else { 0 });
                pass.set_bind_group(3, &self.group, &[((base + index as u64) * SLOT) as u32]);
                pass.set_vertex_buffer(0, held.vertices.slice(..));
                pass.set_vertex_buffer(1, self.defaults.slice(..));
                pass.set_index_buffer(held.indexes.slice(..), wgpu::IndexFormat::Uint16);
                for &(first, count) in &held.parts {
                    pass.draw_indexed(first..first + count, held.base_vertex, 0..1);
                }
            }
        }
        if let Some(dry) = dry {
            dry.zero(pass);
        }
        self.drawn += surfaces.len() as u64;
        count as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_markers() {
        assert!(marked(99998.0) && marked(99997.0) && marked(100000.0) && marked(104095.0));
        assert!(!marked(99999.0) && !marked(0.0) && !marked(104096.0) && !marked(10.0));
        let table = HashMap::from([(
            3,
            Look {
                glow: 0.5,
                flags: 0,
                ..Look::STANDARD
            },
        )]);
        assert_eq!(Look::of(99998.0, &table), Look::STANDARD);
        assert_eq!(Look::of(99997.0, &table).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of(100003.0, &table).glow, 0.5);
        assert_eq!(Look::of(100004.0, &table), Look::STANDARD);
    }

    #[test]
    fn the_depth_state_follows_the_game() {
        // The kit's surfaces: test on, write off, function less or equal (D3DCMP 4).
        assert_eq!(
            depth_state([1, 0, 4]),
            DepthState {
                test: true,
                write: false,
                function: 4
            }
        );
        // The default z-buffer property: test and write.
        assert_eq!(
            depth_state([1, 1, 4]),
            DepthState {
                test: true,
                write: true,
                function: 4
            }
        );
        // Another function keeps its value, for the pipeline.
        assert_eq!(
            depth_state([1, 1, 5]),
            DepthState {
                test: true,
                write: true,
                function: 5
            }
        );
        // ALWAYS (8) is no test, and the write stays as the game asks.
        assert_eq!(
            depth_state([1, 1, 8]),
            DepthState {
                test: false,
                write: true,
                function: 8
            }
        );
        // No test, as the game sends it (function 8): the write is independent.
        assert_eq!(
            depth_state([0, 1, 8]),
            DepthState {
                test: false,
                write: true,
                function: 8
            }
        );
        assert_eq!(
            depth_state([0, 0, 8]),
            DepthState {
                test: false,
                write: false,
                function: 8
            }
        );
    }

    #[test]
    fn a_surface_passes_at_the_depth_of_its_depth_only_pass() {
        // The compare of each function at the depth that passed the first test (equal), and at
        // one where the first test failed.
        let passes = |function: u32, surface: f32, buffer: f32| match function {
            1 => false,
            2 => surface < buffer,
            3 => surface == buffer,
            4 => surface <= buffer,
            5 => surface > buffer,
            6 => surface != buffer,
            7 => surface >= buffer,
            _ => true,
        };
        for function in 1..=8 {
            let after = function_after_depth_pass(function);
            // Each value of the buffer: nearer than the surface, and farther.
            for buffer in [0.25, 0.75] {
                let first = passes(function, 0.5, buffer);
                let written = if first { 0.5 } else { buffer };
                assert_eq!(passes(after, 0.5, written), first, "function {function}, depth {buffer}");
            }
        }
        // The known case: the buffer already has the surface's own depth.
        for function in [2, 5, 6] {
            assert!(!passes(function, 0.5, 0.5), "function {function}");
            assert!(passes(function_after_depth_pass(function), 0.5, 0.5), "function {function}");
        }
    }

    #[test]
    fn a_look_line() {
        let look = Look::parse("flow=0,-140 speed=1.2 SKY=#FF8040 reflect=sky opacity=vertex p2=1,2 shader=foam clarity=x");
        assert_eq!(look.flow, [0.0, -140.0]);
        assert_eq!(look.speed, 1.2);
        assert_eq!(look.sky, [1.0, 128.0 / 255.0, 64.0 / 255.0]);
        assert_eq!(look.flags, LOOK_HAS_SKY | LOOK_OPACITY_FROM_VERTEX);
        assert_eq!(look.params[2], [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(&look.shader[..5], b"foam\0");
        assert_eq!(look.shader_name().as_deref(), Some("foam"));
        assert_eq!(Look::STANDARD.shader_name(), None);
        assert_eq!(Look::parse("shader=TW_Lava").shader_name().as_deref(), Some("tw_lava"));
        assert_eq!(look.clarity, 800.0);
        assert_eq!(Look::parse("sky=0.5,0.25,1 opacity=0.4").sky, [0.5, 0.25, 1.0]);
        assert_eq!(Look::of_distant(1, None), Look::STANDARD);
        assert_eq!(Look::of_distant(2, None).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of_distant(2, Some("glow=0.5")).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of_distant(1, Some("glow=0.5")).glow, 0.5);
    }

    #[test]
    fn the_record_of_the_looks() {
        // The layout of WaterLook of waterlook.h.
        assert_eq!(size_of::<Look>(), 144);
        let full = Look {
            flags: LOOK_HAS_SKY,
            flow: [0.0, -140.0],
            speed: 1.2,
            sky: [1.0, 0.5, 0.25],
            clarity: 200.0,
            ..Look::STANDARD
        };
        // A look of a mod built against a shorter layout: up to the opacity (32 bytes).
        let short = Look {
            glow: 0.3,
            opacity: 0.6,
            ..Look::STANDARD
        };
        let mut record = Vec::new();
        for word in [1u32, 2, 5] {
            record.extend_from_slice(&word.to_le_bytes());
        }
        record.extend_from_slice(bytemuck::bytes_of(&full));
        record.extend_from_slice(&9u32.to_le_bytes());
        let mut short_bytes = bytemuck::bytes_of(&short)[..32].to_vec();
        short_bytes[..4].copy_from_slice(&32u32.to_le_bytes());
        record.extend_from_slice(&short_bytes);
        let table = Look::read_table(&record).unwrap();
        assert_eq!(table[&5], full);
        assert_eq!(table[&9], Look { size: 32, ..short });
        assert!(Look::read_table(&record[..record.len() - 4]).is_none());
        assert!(Look::read_table(&[2, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        let module = crate::renderer::shader_check::check("water volume", &shader()).unwrap();
        for entry in ["vs", "vs_far", "fs", "fs_depth"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
    }

    /// The cone of a spot light has one home, `nirender_wgpu::spot`: the shader of the surfaces
    /// has the crate's text once, and the part of this file calls it and has no cone of its own.
    #[test]
    fn the_surfaces_take_the_cone_of_the_crate() {
        let source = shader();
        assert_eq!(source.matches(nirender_wgpu::spot::WGSL).count(), 1);
        assert_eq!(source.matches("fn spot_cone").count(), 1);
        let volume = WATER_VOLUME.part("volume");
        assert!(
            volume.contains("spot_cone(light, direction)")
                && !volume.contains("fn spot_cone")
                && !volume.contains("light.cone.x")
        );
    }

    /// The light that the surface takes from a spot light (`light_strength` of the surface
    /// shader, run on the GPU): all of it on the axis of the cone, none at 90 degrees from the
    /// axis or behind the light; a point light at the same place lights the place at 90 degrees.
    /// Needs a GPU: `cargo test -p mgeHost64 -- --ignored a_spot_light_gives`.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_spot_light_gives_the_surface_no_light_outside_its_cone() {
        // A spot light at the origin along +x: inner half angle about 5 degrees, outer about 10,
        // exponent 1, constant falloff 1, radius 300. Each place is 100 from the light.
        const PROBE: &str = "
@group(0) @binding(90) var<storage, read_write> strengths: array<f32, 4>;
@compute @workgroup_size(1) fn probe_light_strength() {
    let spot = Light(vec4f(1.0), vec4f(0.0), vec4f(0.0, 0.0, 0.0, 2.0), vec4f(1.0, 0.0, 0.0, 1000.0),
        vec4f(1.0, 0.0, 0.0, 1.0), vec4f(0.996, 0.985, 300.0, 0.0));
    var point = spot;
    point.position.w = 1.0;
    // The direction goes from the place to the light: a place on the axis, at 90 degrees, behind.
    strengths[0] = light_strength(spot, 100.0, vec3f(-1.0, 0.0, 0.0));
    strengths[1] = light_strength(spot, 100.0, vec3f(0.0, -1.0, 0.0));
    strengths[2] = light_strength(spot, 100.0, vec3f(1.0, 0.0, 0.0));
    strengths[3] = light_strength(point, 100.0, vec3f(0.0, -1.0, 0.0));
}
";
        static GPU_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one_at_a_time = GPU_TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");

        let source = format!("{}{PROBE}", shader());
        let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water light probe"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.counted_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water light probe"),
            layout: None,
            module: &module,
            entry_point: Some("probe_light_strength"),
            compilation_options: Default::default(),
            cache: None,
        });
        let size = 4 * 4;
        let strengths = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("water light probe"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("water light probe, read"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water light probe"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 90,
                resource: strengths.as_entire_binding(),
            }],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("water light probe"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("water light probe"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&strengths, 0, &read, 0, size);
        queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = read.slice(..).get_mapped_range().expect("the read buffer is mapped");
        let values: [f32; 4] = bytemuck::cast_slice::<u8, f32>(&data).try_into().expect("four values");

        assert!((values[0] - 1.0).abs() < 1e-4, "on the axis: {values:?}");
        assert_eq!(values[1], 0.0, "at 90 degrees from the axis: {values:?}");
        assert_eq!(values[2], 0.0, "behind the light: {values:?}");
        assert!((values[3] - 1.0).abs() < 1e-4, "a point light at 90 degrees: {values:?}");
    }

    /// A water shader of a mod: the interface line, the functions it may call, the textures
    /// of the mesh, and an error for a file that is not one.
    #[test]
    fn a_water_shader_of_a_mod() {
        let file = "//! interface: mge-xe-water-wgsl 1
fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);
    var shade = shade_water_rippled(surface, look, standard_ripple(surface, look));
    let base = textureSample(mesh0, mesh_sampler, surface.uv.xy).rgb * look.p0.x;
    shade.colour = fog_apply(mix(base, textureSample(mesh1, mesh_sampler, surface.uv.zw).rgb, 0.5), shade.fog) + 0.0 * water_time();
    return finish_water(surface, shade, look);
}
";
        assert!(test_water_shader(file).is_ok(), "{:?}", test_water_shader(file).err());
        assert!(
            test_water_shader("fn water_shader(surface: WaterSurface) -> vec4f { return vec4f(1.0); }")
                .unwrap_err()
                .contains("interface")
        );
        assert!(
            test_water_shader("//! interface: mge-xe-water-wgsl 1\nfn water_shader(x: f32) -> f32 { return x; }").is_err()
        );
    }

    /// The water shaders of the True Water packs in ports/ compile against the water library.
    #[test]
    fn the_true_water_packs_give_water_shaders() {
        let files = [
            include_str!("../../../../ports/True Water - Odai - wgpu shaders/shaders/water/tw_foam.wgsl"),
            include_str!("../../../../ports/True Water - Flow Map - wgpu shaders/shaders/water/tw_flowmap.wgsl"),
        ];
        for file in files {
            assert!(test_water_shader(file).is_ok(), "{:?}", test_water_shader(file).err());
        }
    }
}
