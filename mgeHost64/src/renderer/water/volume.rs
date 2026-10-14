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

use std::collections::HashMap;

use nirender_wgpu::{HeldDraw, SceneView, wgpu};

use super::shading;

/// The specular power of a surface that reflects what is in the picture and the sky.
const REFLECTS_SCENE: f32 = 99998.0;
/// The specular power of a surface that reflects the sky only.
const REFLECTS_SKY: f32 = 99997.0;
/// The specular power of look slot 0; there are 4096 slots (`WaterLooks::maxSlots`).
const SLOT_FIRST: f32 = 100000.0;
const SLOTS: f32 = 4096.0;

/// True for a material that a mod marked as the surface of a water volume.
pub fn marked(specular_power: f32) -> bool {
    specular_power == REFLECTS_SCENE || specular_power == REFLECTS_SKY || (SLOT_FIRST..SLOT_FIRST + SLOTS).contains(&specular_power)
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
    /// The free values p0 to p3, and the name of a water shader of a mod: for B10.
    params: [[f32; 4]; 4],
    shader: [u8; 32],
    /// What the surface reflects in place of the sky, with `LOOK_HAS_SKY`.
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
    /// layout keeps the standard values after its end. `None` for a record that is not one.
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

    /// The look of a marked surface: that of its slot, or the standard look; 99997 reflects
    /// the sky only.
    fn of(specular_power: f32, table: &HashMap<u32, Look>) -> Look {
        if specular_power == REFLECTS_SKY {
            return Look { flags: Look::STANDARD.flags & !LOOK_REFLECTS_SCENE, ..Look::STANDARD };
        }
        if specular_power >= SLOT_FIRST {
            if let Some(look) = table.get(&((specular_power - SLOT_FIRST) as u32)) {
                return *look;
            }
        }
        Look::STANDARD
    }
}

/// What the frame says about the look of the surfaces.
pub struct VolumeLook {
    /// How far from the camera a surface reflects what is in the picture
    /// (`distant_land.water.volume_reflection_cells`, in units); 0 for the sky only.
    pub reflect_range: f32,
    /// A colour in place of the sky, and how much of it: the light of the room in an
    /// interior, as the water of the cell has it.
    pub sky: [f32; 4],
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
    /// The glow, the opacity and the clarity of the look; then bits: 1 the mesh has normals,
    /// 2 it has vertex colours, 4 the vertex colour tints the water, 8 the vertex alpha is
    /// the opacity.
    look: [f32; 4],
}

/// The space of one surface in the buffer of the surfaces: a uniform offset is a multiple
/// of 256 bytes.
const SLOT: u64 = 256;

/// The entry points of a surface, after the shared shading.
const VOLUME: &str = "
struct Surface {
    world: mat4x4f,
    tint: vec4f,
    anchor: vec4f,
    sky: vec4f,
    flow: vec4f,
    look: vec4f,
}
@group(1) @binding(0) var<uniform> s: Surface;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) facing: vec3f,
    @location(2) clip_w: f32,
    @location(3) color: vec4f,
}

// The normal and the colour of a vertex that has none come from a buffer of zeros.
@vertex fn vs(@location(0) local: vec3f, @location(1) normal: vec3f, @location(2) color: vec4f) -> Out {
    var out: Out;
    let bits = u32(s.look.w);
    out.world = (s.world * vec4f(local, 1.0)).xyz;
    // A mesh without normals faces up.
    out.facing = select(vec3f(0.0, 0.0, 1.0), (s.world * vec4f(normal, 0.0)).xyz, (bits & 1u) != 0u);
    // A D3DCOLOR has blue in its first byte.
    out.color = select(vec4f(1.0), color.zyxw, (bits & 2u) != 0u);
    out.position = u.view_projection * vec4f(out.world, 1.0);
    out.clip_w = out.position.w;
    return out;
}

// The ripples of the sea with the drift, the speed and the size of the look
// (`surfaceRipples`). The drift moves the coordinates; it is wrapped, so it keeps its
// precision over hours.
fn look_ripples(texcoords: vec4f, distance: f32, world_xy: vec2f) -> vec3f {
    let t = 0.4 * u.eye.w * s.flow.z;
    let moved = s.flow.xy * u.eye.w;
    let scale = max(s.flow.w, 1e-3);
    let far = textureSample(waves, wave_sampler, vec3f(texcoords.xy / scale + fract(moved / 3900.0), t)).rg;
    var close = textureSample(waves, wave_sampler, vec3f(texcoords.zw / scale + fract(moved / 527.0), t)).rg;
    if u.wave.w > 0.5 {
        close += textureSampleLevel(rain, wave_sampler, texcoords.zw, 0.0).ba;
        let at = (world_xy - u.wave.xy) / 1280.0;
        if all(at >= vec2f(0.0)) && all(at <= vec2f(1.0)) {
            close += textureSampleLevel(player_waves, clamp_sampler, at, 0.0).ba;
        }
    }
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    var eye_vec = in.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    let uv = in.position.xy / u.size.xy;
    let w = in.clip_w;
    var facing = vec3f(0.0, 0.0, 1.0);
    if dot(in.facing, in.facing) > 0.25 {
        facing = normalize(in.facing);
    }
    // The ripples lie in the plane of the surface: the place, turned about the anchor of the
    // mesh by the turn that takes the surface up. A level surface has the coordinates of the
    // sea, so the two agree at a seam.
    let upward = select(facing, -facing, facing.z < 0.0);
    let flat = tilt_to(vec3f(-upward.x, -upward.y, upward.z), in.world - s.anchor.xyz) + s.anchor.xyz;
    let texcoords = vec4f(flat.xy / 3900.0, flat.xy / 527.0);
    // From under the water each surface has the look from below, as the plane has it.
    if u.fog.w > 0.5 {
        return underwater(uv, w, texcoords, flat.xy, eye_vec, dist);
    }
    // The way the surface faces, turned to the side of the eye; the ripples tilted to it.
    let face = select(facing, -facing, dot(facing, eye_vec) > 0.0);
    let fog = fog_water(eye_vec, dist);
    let ripple = look_ripples(texcoords, dist, flat.xy);
    let normal = tilt_to(face, ripple);
    let tint = s.tint.rgb;
    // The look: the vertex colour tints the water, and its alpha is the opacity, when the
    // look says so.
    let bits = u32(s.look.w);
    let look_tint = select(vec3f(1.0), in.color.rgb, (bits & 4u) != 0u);
    let opacity = s.look.y * select(1.0, in.color.a, (bits & 8u) != 0u);
    let deep = fog.a * depth_base(tint) * look_tint + fog.rgb;
    let reffactor = (u.sun_col.w * dist + 0.1) * ripple.xy;
    let below = refraction(uv, w, reffactor, eye_vec, ripple, deep, tint, max(s.look.z, 1.0), look_tint);
    // Far away and at a flat angle the surface is calm: a ripple is smaller than a pixel.
    let calm = (1.0 + dot(eye_vec, face)) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0));
    var adjust = mix(0.1 * face, normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, face, calm);
    // The ripples tilt the reflected direction only a little; a direction into the surface
    // is turned back out of it.
    var reflect_dir = reflect(eye_vec, normalize(mix(face, normal, 0.35 * (1.0 - calm))));
    reflect_dir -= 2.0 * min(0.0, dot(reflect_dir, face)) * face;
    reflect_dir = normalize(reflect_dir);
    var reflected = mix(sky_color(reflect_dir), s.sky.rgb, s.sky.a);
    if s.tint.w > 0.5 {
        let seen = reflect_scene(in.world, reflect_dir);
        reflected = mix(reflected, seen.rgb, seen.a);
    }
    reflected = mix(fog.rgb, reflected, fog.a);
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(below.color, reflected, fresnel);
    result += sun_glint(eye_vec, normal, fog.a);
    result = mix(result, below.color, below.shore * fog.a);
    // The light of the surface's own, in the colour of the water: a water blue for the usual
    // colour. Then what is behind a surface that is not all water.
    let glow_color = select(vec3f(0.25, 0.5, 0.6), tint, dot(tint, vec3f(1.0)) > 0.0);
    result += s.look.x * glow_color * fog.a;
    result = mix(textureSampleLevel(picture, clamp_sampler, uv, 0.0).rgb, result, opacity);
    return vec4f(result, 1.0);
}
";

/// The shader of the surfaces: the shared shading and the entry points of a surface.
fn shader() -> String {
    format!("{}{}", shading::COMMON, VOLUME)
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
}

pub(super) struct Volumes {
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    surface_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    /// The values of the surfaces of a frame, one slot each, and their group.
    surfaces: wgpu::Buffer,
    capacity: u64,
    group: wgpu::BindGroup,
    /// Zeros for the inputs that a vertex does not have.
    defaults: wgpu::Buffer,
    target_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    pub drawn: u64,
}

impl Volumes {
    /// `frame_layout` is that of the group of the shared shading (`Water::layout`).
    pub fn new(scene: &SceneView, frame_layout: &wgpu::BindGroupLayout) -> Volumes {
        let device = scene.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water volume"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water volume"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(size_of::<Surface>() as u64) },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("water volume"), bind_group_layouts: &[Some(frame_layout), Some(&surface_layout)], immediate_size: 0 });
        let (surfaces, group) = Self::slots(device, &surface_layout, 16);
        let defaults = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume defaults"), size: 32, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false });
        Volumes {
            module,
            pipeline_layout,
            surface_layout,
            pipelines: HashMap::new(),
            surfaces,
            capacity: 16,
            group,
            defaults,
            target_format: scene.target_format,
            depth_format: scene.depth_format,
            drawn: 0,
        }
    }

    /// A buffer of `count` slots and its group.
    fn slots(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, count: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume surfaces"), size: count * SLOT, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water volume surfaces"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(size_of::<Surface>() as u64) }),
            }],
        });
        (buffer, group)
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: Key) -> &wgpu::RenderPipeline {
        let (module, layout, target_format, depth_format) = (&self.module, &self.pipeline_layout, self.target_format, self.depth_format);
        self.pipelines.entry(key).or_insert_with(|| {
            let mut attributes = vec![wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: key.position, shader_location: 0 }];
            let mut missing = Vec::new();
            match key.normal {
                Some(normal) => attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: normal, shader_location: 1 }),
                None => missing.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 1 }),
            }
            match key.color {
                Some(color) => attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: color, shader_location: 2 }),
                None => missing.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 16, shader_location: 2 }),
            }
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water volume"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout { array_stride: key.stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }),
                        Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &missing }),
                    ],
                },
                // Both sides: a surface is seen from under the water too.
                primitive: wgpu::PrimitiveState {
                    topology: key.topology,
                    cull_mode: None,
                    polygon_mode: if key.lines { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: target_format, blend: None, write_mask: wgpu::ColorWrites::COLOR })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Draws the marked surfaces among `held` with the group of the shared shading of the
    /// frame. Returns the number drawn.
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame_group: &wgpu::BindGroup, held: &[HeldDraw], frame_look: &VolumeLook, looks: &HashMap<u32, Look>) -> usize {
        let surfaces: Vec<&HeldDraw> = held.iter().filter(|h| marked(h.facts.specular_power)).collect();
        if surfaces.is_empty() {
            return 0;
        }
        let count = surfaces.len() as u64;
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            (self.surfaces, self.group) = Self::slots(scene.device, &self.surface_layout, self.capacity);
        }
        let mut bytes = vec![0u8; (count * SLOT) as usize];
        for (index, held) in surfaces.iter().enumerate() {
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
            let drift = [look.flow[0] * world[0] + look.flow[1] * world[4], look.flow[0] * world[1] + look.flow[1] * world[5]];
            let has_color = held.layout.color.is_some();
            let bits = held.layout.normal.is_some() as u32
                | (has_color as u32) << 1
                | ((has_color && look.flags & LOOK_TINT_FROM_VERTEX != 0) as u32) << 2
                | ((has_color && look.flags & LOOK_OPACITY_FROM_VERTEX != 0) as u32) << 3;
            let surface = Surface {
                world,
                tint: [emissive[0], emissive[1], emissive[2], if reflects { 1.0 } else { 0.0 }],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: if look.flags & LOOK_HAS_SKY != 0 { [look.sky[0], look.sky[1], look.sky[2], 1.0] } else { frame_look.sky },
                flow: [drift[0], drift[1], look.speed, look.scale],
                look: [look.glow, look.opacity, look.clarity, bits as f32],
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        scene.queue.write_buffer(&self.surfaces, 0, &bytes);
        let lines = scene.wireframe && scene.device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        pass.set_bind_group(0, frame_group, &[]);
        for (index, held) in surfaces.iter().enumerate() {
            let layout = &held.layout;
            let key = Key { stride: layout.stride, position: layout.position, normal: layout.normal, color: layout.color, topology: held.topology, lines };
            pass.set_pipeline(self.pipeline(scene.device, key));
            pass.set_bind_group(1, &self.group, &[(index as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, held.vertices.slice(..));
            pass.set_vertex_buffer(1, self.defaults.slice(..));
            pass.set_index_buffer(held.indexes.slice(..), wgpu::IndexFormat::Uint16);
            for &(first, count) in &held.parts {
                pass.draw_indexed(first..first + count, 0, 0..1);
            }
        }
        self.drawn += count;
        surfaces.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_markers() {
        assert!(marked(99998.0) && marked(99997.0) && marked(100000.0) && marked(104095.0));
        assert!(!marked(99999.0) && !marked(0.0) && !marked(104096.0) && !marked(10.0));
        let table = HashMap::from([(3, Look { glow: 0.5, flags: 0, ..Look::STANDARD })]);
        assert_eq!(Look::of(99998.0, &table), Look::STANDARD);
        assert_eq!(Look::of(99997.0, &table).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of(100003.0, &table).glow, 0.5);
        assert_eq!(Look::of(100004.0, &table), Look::STANDARD);
    }

    #[test]
    fn the_record_of_the_looks() {
        // The layout of WaterLook of waterlook.h.
        assert_eq!(size_of::<Look>(), 144);
        let full = Look { flags: LOOK_HAS_SKY, flow: [0.0, -140.0], speed: 1.2, sky: [1.0, 0.5, 0.25], clarity: 200.0, ..Look::STANDARD };
        // A look of a mod built against a shorter layout: up to the opacity (32 bytes).
        let short = Look { glow: 0.3, opacity: 0.6, ..Look::STANDARD };
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
        use wgpu::naga;
        let source = shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        for entry in ["vs", "fs"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
    }
}
