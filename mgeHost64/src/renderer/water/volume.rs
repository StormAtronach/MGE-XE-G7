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

/// The surface reflects what is in the picture. A look slot has the standard look, which
/// does.
fn reflects_scene(specular_power: f32) -> bool {
    specular_power != REFLECTS_SKY
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
}
@group(1) @binding(0) var<uniform> s: Surface;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) facing: vec3f,
    @location(2) clip_w: f32,
}

fn place(local: vec3f, facing: vec3f) -> Out {
    var out: Out;
    out.world = (s.world * vec4f(local, 1.0)).xyz;
    out.facing = facing;
    out.position = u.view_projection * vec4f(out.world, 1.0);
    out.clip_w = out.position.w;
    return out;
}

@vertex fn vs(@location(0) local: vec3f, @location(1) normal: vec3f) -> Out {
    return place(local, (s.world * vec4f(normal, 0.0)).xyz);
}

// A mesh without normals faces up.
@vertex fn vs_flat(@location(0) local: vec3f) -> Out {
    return place(local, vec3f(0.0, 0.0, 1.0));
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
    let ripple = water_normal(texcoords, dist, flat.xy);
    let normal = tilt_to(face, ripple);
    let tint = s.tint.rgb;
    let reffactor = (u.sun_col.w * dist + 0.1) * ripple.xy;
    let below = refraction(uv, w, reffactor, eye_vec, ripple, deep_color(fog, tint), tint);
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
        Volumes {
            module,
            pipeline_layout,
            surface_layout,
            pipelines: HashMap::new(),
            surfaces,
            capacity: 16,
            group,
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
            if let Some(normal) = key.normal {
                attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: normal, shader_location: 1 });
            }
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water volume"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(if key.normal.is_some() { "vs" } else { "vs_flat" }),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: key.stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
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
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, frame_group: &wgpu::BindGroup, held: &[HeldDraw], look: &VolumeLook) -> usize {
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
            let reflects = reflects_scene(held.facts.specular_power) && look.reflect_range >= 1.0;
            let emissive = held.facts.emissive;
            let surface = Surface {
                world,
                tint: [emissive[0], emissive[1], emissive[2], if reflects { 1.0 } else { 0.0 }],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: look.sky,
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        scene.queue.write_buffer(&self.surfaces, 0, &bytes);
        let lines = scene.wireframe && scene.device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        pass.set_bind_group(0, frame_group, &[]);
        for (index, held) in surfaces.iter().enumerate() {
            let layout = &held.layout;
            let key = Key { stride: layout.stride, position: layout.position, normal: layout.normal, topology: held.topology, lines };
            pass.set_pipeline(self.pipeline(scene.device, key));
            pass.set_bind_group(1, &self.group, &[(index as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, held.vertices.slice(..));
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
        assert!(reflects_scene(99998.0) && reflects_scene(100003.0) && !reflects_scene(99997.0));
    }

    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        use wgpu::naga;
        let source = shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        for entry in ["vs", "vs_flat", "fs"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
    }
}
