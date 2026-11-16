//! The distant lights of the wgpu renderer: the light sources of the exterior beyond the cells
//! that the game has loaded, from `distantland\lights.data` of the generator.
//!
//! - The glows: a compute pass chooses the sources with a mesh that are in the view, past the
//!   handoff to the game (where the game draws the real light) and within the draw distance,
//!   and writes them into a list; one indirect draw puts a quad that faces the camera at each,
//!   added to the picture, with the colour of the light. A glow has a least size on the screen,
//!   so that a far light stays a point; it is dimmer then. It goes away in the day, and the fog
//!   takes it as it takes the land. A light that flickers or pulses in the game does so here.
//! - The lamps of the file's exterior block go to [`super::lamp_grid`], which makes the list and
//!   the grid of the light that the lamps give the distant land and statics.

use nirender_wgpu::objects::Create;
use std::path::{Path, PathBuf};

use nirender_wgpu::pass_report::Skip;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use super::lamp_grid::{FileLamps, SourceLamp};
use distantland::mge_xe::lights::{EXTERIOR_KEY, Key, parse_lights};
use nirender_wgpu::SceneView;
use nirender_wgpu::wgpu;

/// The values of a source in the list of the glows: two rows of four numbers.
const GLOW_WORDS: u64 = 8;

/// What the file has.
struct LightsFile {
    /// The sources as the file has them, 32 bytes each: the layout of `Source` of the shader.
    sources: Vec<u8>,
    source_count: u32,
    /// The lamps, for their light on the distant land and statics.
    lamps: FileLamps,
    /// The light records of the exterior's lamps, with the values that the file has for them.
    records: Vec<FileLampRecord>,
}

/// What the lights file has for one light record of its exterior lamps: the values of the
/// plugins at the time of the generation.
#[derive(Clone, Debug, PartialEq)]
pub struct FileLampRecord {
    /// The key of the id.
    pub id: Key,
    pub radius: f32,
    pub color: [u8; 3],
    /// The number of the exterior's lamps of this record.
    pub lamps: u32,
}

/// The records of a block's lamps with the radius and the colour of the first lamp of each,
/// and the count of its lamps. A record without a lamp is left out.
fn file_records(block: &distantland::mge_xe::lights::LightBlock) -> Vec<FileLampRecord> {
    let mut records: Vec<Option<FileLampRecord>> = vec![None; block.records.len()];
    for lamp in &block.lamps {
        let Some(slot) = records.get_mut(lamp.record as usize) else {
            continue;
        };
        match slot {
            Some(record) => record.lamps += 1,
            None => {
                *slot = Some(FileLampRecord {
                    id: block.records[lamp.record as usize].id.clone(),
                    radius: lamp.radius,
                    color: lamp.color,
                    lamps: 1,
                });
            }
        }
    }
    records.into_iter().flatten().collect()
}

/// A lamp of the file with the values that its light takes.
fn source_lamp(lamp: &distantland::mge_xe::lights::Lamp) -> SourceLamp {
    SourceLamp {
        position: lamp.position,
        radius: lamp.radius,
        color: lamp.color,
        flags: lamp.flags,
        record: lamp.record,
    }
}

/// Reads `lights.data` through the formats crate, which checks each count and length against
/// the file's end and gives the reason of a refusal. Only the exterior's block: the lamps of an
/// interior's block have that interior's coordinates.
fn read(path: &Path) -> Result<LightsFile, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let lights = parse_lights(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    let exterior = lights.block(&EXTERIOR_KEY);
    let (sources, source_count) = exterior.map_or((Vec::new(), 0), |block| (block.lamp_bytes(), block.lamps.len() as u32));
    Ok(LightsFile {
        sources,
        source_count,
        lamps: exterior.map_or_else(FileLamps::default, |block| FileLamps {
            lamps: block.lamps.iter().map(source_lamp).collect(),
            record_ids: block.records.iter().map(|record| record.id.clone()).collect(),
        }),
        records: exterior.map_or_else(Vec::new, file_records),
    })
}

const CULL_SHADER: &str = "
struct Cull {
    planes: array<vec4f, 6>,
    eye: vec4f,
    // x: where a glow starts (the handoff), y: where it is whole, z: the draw distance,
    // w: the time of the game.
    range: vec4f,
    // x: the sources.
    counts: vec4u,
}
struct Source {
    position: vec3f,
    radius: f32,
    color: u32,
    flags: u32,
    pad0: u32,
    pad1: u32,
}
@group(0) @binding(0) var<uniform> c: Cull;
@group(0) @binding(1) var<storage, read> sources: array<Source>;
@group(0) @binding(2) var<storage, read_write> glows: array<vec4f>;
@group(0) @binding(3) var<storage, read_write> args: array<atomic<u32>, 4>;

fn hash(p: vec3f) -> f32 {
    return fract(sin(dot(p, vec3f(12.9898, 78.233, 37.719))) * 43758.5453);
}

@compute @workgroup_size(256)
fn cull(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= c.counts.x {
        return;
    }
    let s = sources[i];
    let color = unpack4x8unorm(s.color);
    // A light without a mesh has nothing that glows.
    if color.w < 0.5 {
        return;
    }
    let distance = length(s.position - c.eye.xyz);
    if distance < c.range.x || distance > c.range.z {
        return;
    }
    for (var k = 0u; k < 6u; k++) {
        let p = c.planes[k];
        if dot(p.xyz, s.position) + p.w < -s.radius {
            return;
        }
    }
    var strength = smoothstep(c.range.x, c.range.y, distance);
    // The flags of the light record: flicker 0x8, flicker slow 0x40, pulse 0x80, pulse slow 0x100.
    let phase = hash(s.position) * 6.2831853;
    let t = c.range.w;
    if (s.flags & 0x8u) != 0u {
        strength *= 0.8 + 0.2 * sin(t * 11.0 + phase) * sin(t * 6.3 + 2.0 * phase);
    } else if (s.flags & 0x40u) != 0u {
        strength *= 0.85 + 0.15 * sin(t * 4.0 + phase) * sin(t * 2.3 + 2.0 * phase);
    } else if (s.flags & 0x80u) != 0u {
        strength *= 0.65 + 0.35 * sin(t * 3.0 + phase);
    } else if (s.flags & 0x100u) != 0u {
        strength *= 0.65 + 0.35 * sin(t * 1.0 + phase);
    }
    let slot = atomicAdd(&args[1], 1u);
    glows[slot * 2u] = vec4f(s.position, s.radius);
    glows[slot * 2u + 1u] = vec4f(color.rgb, strength);
}
";

/// The glows in the main view: after the view group (`view::wgsl`).
const GLOW_SHADER: &str = "
struct Glow {
    // x: the view range of the game.
    range: vec4f,
    // x: the size as a part of the radius, y: the strength, z: the least size in pixels,
    // w: the most size in pixels.
    glow: vec4f,
    // x, y: the size of the picture; z: the pixels of one unit at a distance of one unit.
    screen: vec4f,
}
@group(1) @binding(0) var<uniform> g: Glow;
@group(1) @binding(1) var<storage, read> glows: array<vec4f>;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) corner: vec2f,
    @location(1) color: vec3f,
}

// The fog of `fog_of` of the land, without the colour that the air adds: what is left of a
// light at this distance.
fn fog_left(world: vec3f) -> f32 {
    let distance = length(world - u.eye.xyz);
    let linear = clamp((u.fog.w - distance) / max(u.fog.w - u.fog.z, 1.0), 0.0, 1.0);
    let exponential = clamp(exp(-(distance - u.fog.x) / max(u.fog.y, 1.0)), 0.0, 1.0);
    return select(linear, exponential, u.flags.x > 0.5 && distance > g.range.x);
}

@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Out {
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let place = glows[instance * 2u];
    let look = glows[instance * 2u + 1u];
    var clip = clip_of(place.xyz);
    let w = max(clip.w, 1.0);
    let wanted = place.w * g.glow.x * g.screen.z / w;
    let pixels = clamp(wanted, g.glow.z, g.glow.w);
    // A glow drawn larger than it is gives the light of its size, not more.
    let dim = clamp(wanted / pixels, 0.25, 1.0);
    let corner = corners[vertex];
    var out: Out;
    out.position = vec4f(clip.xy + corner * pixels * 2.0 / g.screen.xy * clip.w, clip.zw);
    out.corner = corner;
    out.color = look.rgb * look.w * dim * g.glow.y * fog_left(place.xyz);
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let r2 = dot(in.corner, in.corner);
    if r2 >= 1.0 {
        discard;
    }
    let falloff = (1.0 - r2) * (1.0 - r2) * exp(-3.0 * r2);
    return vec4f(in.color * falloff, 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CullUniforms {
    planes: [[f32; 4]; 6],
    eye: [f32; 4],
    range: [f32; 4],
    counts: [u32; 4],
}

/// Group 1 of the glows: `Glow` of [`GLOW_SHADER`].
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlowUniforms {
    range: [f32; 4],
    glow: [f32; 4],
    screen: [f32; 4],
}

/// The values of a frame for the glows. Those of the view (the matrix, the fog) are in the
/// view group of the main view.
pub struct GlowFrame {
    /// The camera, and the view range of the game in w.
    pub eye: [f32; 4],
    pub planes: [[f32; 4]; 6],
    /// Where a glow starts (the handoff to the game) and where it is whole.
    pub fade: [f32; 2],
    pub draw_distance: f32,
    pub time: f32,
    /// The size of the picture, and the pixels of one unit at a distance of one unit.
    pub screen: [f32; 3],
    /// The size as a part of the radius, the strength (with the night in it), the least size
    /// in pixels.
    pub glow: [f32; 3],
}

/// The buffers of one choice of glows and its draw: the main view's, and one for each view of
/// its own of a frame (the writes of a frame land before all its commands, so no two views can
/// share them).
struct GlowSet {
    cull_group: wgpu::BindGroup,
    cull_uniforms: wgpu::Buffer,
    args: wgpu::Buffer,
    /// Group 1 of the draw: the values of the glows ([`GlowUniforms`]) and the list.
    draw_group: wgpu::BindGroup,
    values: wgpu::Buffer,
    /// The bytes of the set's buffers on the GPU.
    bytes: u64,
}

struct Glows {
    cull: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    /// The samples of `draw`. A pass with another count (a view of its own into a rendered
    /// texture has one) takes `draw_picture`.
    samples: u32,
    draw_picture: wgpu::RenderPipeline,
    /// The sources, and the layouts that a set takes its groups from.
    source_buffer: wgpu::Buffer,
    cull_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    /// Set 0 is the main view's; the next ones are made at the first use of a view of its own,
    /// and are `None` while they are not made (or after they were freed).
    sets: Vec<Option<GlowSet>>,
    sources: u32,
}

/// The bytes of the buffers of one glow set for `sources` sources.
fn set_bytes(sources: u32) -> u64 {
    u64::from(sources.max(1)) * GLOW_WORDS * 4 + 16 + (size_of::<CullUniforms>() + size_of::<GlowUniforms>()) as u64
}

impl Glows {
    /// The buffers of one set for `sources` sources, with the groups that read them.
    fn make_set(&self, device: &wgpu::Device) -> GlowSet {
        let list_bytes = u64::from(self.sources.max(1)) * GLOW_WORDS * 4;
        let list = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("distant lights, the glows of a frame"),
            size: list_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let args = device.counted_buffer(&wgpu::BufferDescriptor {
            label: Some("distant lights, the draw"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buffer = |label: &str, size: usize| {
            device.counted_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let cull_uniforms = uniform_buffer("distant lights, cull", size_of::<CullUniforms>());
        let values = uniform_buffer("distant lights, glows", size_of::<GlowUniforms>());
        let cull_group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distant lights, cull"),
            layout: &self.cull_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cull_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.source_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: list.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: args.as_entire_binding(),
                },
            ],
        });
        let draw_group = device.counted_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distant lights, glows"),
            layout: &self.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: values.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: list.as_entire_binding(),
                },
            ],
        });
        GlowSet {
            cull_group,
            cull_uniforms,
            args,
            draw_group,
            values,
            bytes: set_bytes(self.sources),
        }
    }
}

pub struct DistantLights {
    loading: Option<Receiver<Result<LightsFile, String>>>,
    glows: Option<Glows>,
    /// The light records of the file's exterior lamps, once the file is read.
    file_records: Option<Vec<FileLampRecord>>,
    /// The lamps of the file's exterior block, once the file is read.
    file_lamps: Option<FileLamps>,
}

impl DistantLights {
    /// Lights with the glows of `sources` sources made as the first frame makes them, with the
    /// main view's set: for the tests of the views of their own.
    #[cfg(test)]
    pub fn with_glows(scene: &SceneView, view_layout: &wgpu::BindGroupLayout, sources: u32) -> DistantLights {
        let file = LightsFile {
            sources: vec![0; sources as usize * 32],
            source_count: sources,
            lamps: FileLamps::default(),
            records: Vec::new(),
        };
        DistantLights {
            loading: None,
            glows: Some(make_glows(scene, &file, view_layout)),
            file_records: None,
            file_lamps: None,
        }
    }

    /// Starts the read of `<folder>\lights.data`.
    pub fn start(folder: &Path) -> DistantLights {
        let path: PathBuf = folder.join("lights.data");
        let (sender, receiver) = channel();
        thread::spawn(move || {
            let _ = sender.send(read(&path));
        });
        DistantLights {
            loading: Some(receiver),
            glows: None,
            file_records: None,
            file_lamps: None,
        }
    }

    /// The answer of the thread that reads the file, once it is there. A thread that ended
    /// without an answer (a panic) is a failed read, not a read still to come.
    fn take_file(&mut self, synchronous: bool) -> Option<Result<LightsFile, String>> {
        let receiver = self.loading.as_ref()?;
        let answer = if synchronous {
            receiver.recv().map_err(|_| TryRecvError::Disconnected)
        } else {
            receiver.try_recv()
        };
        match answer {
            Ok(result) => {
                self.loading = None;
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.loading = None;
                Some(Err("the thread that reads the file ended without an answer".to_string()))
            }
        }
    }

    /// Takes the file when it has been read: the glows, whose pipeline takes the view group of
    /// `view_layout`, and the lamps ([`DistantLights::file_lamps`]). True when this call took a
    /// file.
    pub fn poll(&mut self, scene: &SceneView, view_layout: &wgpu::BindGroupLayout, synchronous: bool) -> bool {
        let Some(result) = self.take_file(synchronous) else {
            return false;
        };
        let file = match result {
            Ok(file) => file,
            Err(problem) => {
                println!("distant lights: none ({problem})");
                return false;
            }
        };
        println!("distant lights: {} light sources", file.source_count);
        if file.source_count > 0 {
            self.glows = Some(super::timed("the distant lights' pipeline", || {
                make_glows(scene, &file, view_layout)
            }));
        }
        self.file_records = Some(file.records);
        self.file_lamps = Some(file.lamps);
        true
    }

    /// The lamps of the file's exterior block; [`None`] until the file is read.
    pub fn file_lamps(&self) -> Option<&FileLamps> {
        self.file_lamps.as_ref()
    }

    /// The light records of the file's exterior lamps; [`None`] until the file is read.
    pub fn file_records(&self) -> Option<&[FileLampRecord]> {
        self.file_records.as_deref()
    }

    /// Makes the buffers of the set `set` (0 is the main view's, made with the glows): a view of
    /// its own, which a frame without them does not pay for. Returns the bytes that this call
    /// made, 0 when the set was there or there are no glows.
    pub fn ensure_set(&mut self, device: &wgpu::Device, set: usize) -> u64 {
        let Some(glows) = self.glows.as_mut() else { return 0 };
        if glows.sets.len() <= set {
            glows.sets.resize_with(set + 1, || None);
        }
        if glows.sets[set].is_some() {
            return 0;
        }
        let buffers = glows.make_set(device);
        let bytes = buffers.bytes;
        glows.sets[set] = Some(buffers);
        bytes
    }

    /// Frees the buffers of the set `set` of a view of its own.
    pub fn release_set(&mut self, set: usize) {
        if let Some(buffers) = self.glows.as_mut().and_then(|glows| glows.sets.get_mut(set)) {
            *buffers = None;
        }
    }

    /// The bytes that a new set takes; 0 without glows.
    pub fn next_set_bytes(&self) -> u64 {
        self.glows.as_ref().map_or(0, |glows| set_bytes(glows.sources))
    }

    /// The bytes of the sets of the views of their own that exist (all sets but the main view's).
    pub fn views_bytes(&self) -> u64 {
        self.glows
            .as_ref()
            .map_or(0, |glows| glows.sets.iter().skip(1).flatten().map(|set| set.bytes).sum())
    }

    /// True when the set `set` has its buffers.
    pub fn has_set(&self, set: usize) -> bool {
        self.glows
            .as_ref()
            .is_some_and(|glows| glows.sets.get(set).is_some_and(Option::is_some))
    }

    /// True when the file has glows: a view of its own can have a set.
    pub fn has_glows(&self) -> bool {
        self.glows.is_some()
    }

    /// Chooses the glows of the frame, in a command buffer of its own before the frame. `set` is
    /// the main view's (0) or a view of its own's, which [`DistantLights::ensure_set`] made.
    /// Answers why it chose nothing: no glows, or no buffers for `set`.
    pub fn prepare(&mut self, scene: &SceneView, frame: &GlowFrame, set: usize) -> Result<(), Skip> {
        let glows = self.glows.as_ref().ok_or(NO_GLOWS)?;
        let buffers = glows.sets.get(set).and_then(Option::as_ref).ok_or(NO_SET)?;
        let cull = CullUniforms {
            planes: frame.planes,
            eye: frame.eye,
            range: [frame.fade[0], frame.fade[1], frame.draw_distance, frame.time],
            counts: [glows.sources, 0, 0, 0],
        };
        scene.queue.write_buffer(&buffers.cull_uniforms, 0, bytemuck::bytes_of(&cull));
        // Six corners for each glow; the cull adds the glows.
        scene
            .queue
            .write_buffer(&buffers.args, 0, bytemuck::cast_slice(&[6u32, 0, 0, 0]));
        let values = GlowUniforms {
            range: [frame.eye[3], 0.0, 0.0, 0.0],
            glow: [frame.glow[0], frame.glow[1], frame.glow[2], 64.0],
            screen: [frame.screen[0], frame.screen[1], frame.screen[2], 0.0],
        };
        scene.queue.write_buffer(&buffers.values, 0, bytemuck::bytes_of(&values));
        let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("distant lights, cull"),
        });
        let at = scene.commands.stamps().begin(
            &mut encoder,
            "distant lights: the glow cull",
            nirender_wgpu::stamps::StampKind::Detail,
        );
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("distant lights, cull"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&glows.cull);
            pass.set_bind_group(0, &buffers.cull_group, &[]);
            pass.dispatch_workgroups(glows.sources.div_ceil(256), 1, 1);
        }
        scene.commands.stamps().end(&mut encoder, at);
        scene.commands.defer(encoder.finish());
        Ok(())
    }

    /// Draws the glows chosen in `prepare` into the pass: after the land and the statics,
    /// against their depth. `view` is the view group of the view that draws, `set` its set of
    /// buffers and `samples` the samples of the pass.
    /// Answers why it drew nothing, as [`DistantLights::prepare`].
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'static>,
        view: &wgpu::BindGroup,
        set: usize,
        samples: u32,
    ) -> Result<(), Skip> {
        let glows = self.glows.as_ref().ok_or(NO_GLOWS)?;
        let buffers = glows.sets.get(set).and_then(Option::as_ref).ok_or(NO_SET)?;
        pass.set_pipeline(if samples == glows.samples {
            &glows.draw
        } else {
            &glows.draw_picture
        });
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, &buffers.draw_group, &[]);
        pass.draw_indirect(&buffers.args, 0);
        Ok(())
    }
}

/// The skips of the glows' calls.
const NO_GLOWS: Skip = "no glows (the lights' file is not read, or has none)";
const NO_SET: Skip = "no buffers for the set of the view";

fn make_glows(scene: &SceneView, file: &LightsFile, view_layout: &wgpu::BindGroupLayout) -> Glows {
    let device = scene.device;
    let source_buffer = device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant lights, the sources"),
        contents: &file.sources,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let entry = |binding: u32, stage: wgpu::ShaderStages, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let compute = wgpu::ShaderStages::COMPUTE;
    let cull_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant lights, cull"),
        entries: &[
            entry(0, compute, wgpu::BufferBindingType::Uniform),
            entry(1, compute, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(2, compute, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(3, compute, wgpu::BufferBindingType::Storage { read_only: false }),
        ],
    });
    let cull_module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("distant lights, cull"),
        source: wgpu::ShaderSource::Wgsl(CULL_SHADER.into()),
    });
    let cull_pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("distant lights, cull"),
        bind_group_layouts: &[Some(&cull_layout)],
        immediate_size: 0,
    });
    let cull = device.counted_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("distant lights, cull"),
        layout: Some(&cull_pipeline_layout),
        module: &cull_module,
        entry_point: Some("cull"),
        compilation_options: Default::default(),
        cache: None,
    });
    let draw_stages = wgpu::ShaderStages::VERTEX_FRAGMENT;
    let draw_layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant lights, glows"),
        entries: &[
            entry(0, draw_stages, wgpu::BufferBindingType::Uniform),
            entry(
                1,
                wgpu::ShaderStages::VERTEX,
                wgpu::BufferBindingType::Storage { read_only: true },
            ),
        ],
    });
    let source = format!("{}{GLOW_SHADER}", super::view::wgsl());
    let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("distant lights, glows"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    // Group 0: the view group of the view; group 1: the glows.
    let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("distant lights, glows"),
        bind_group_layouts: &[Some(view_layout), Some(&draw_layout)],
        immediate_size: 0,
    });
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    // The glows of the frame go into the pass of the game (its samples); a view of its own into
    // a rendered texture has one sample.
    let make_draw = |multisample: wgpu::MultisampleState| {
        device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("distant lights, glows"),
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: scene.depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(nirender_wgpu::depth::compare(wgpu::CompareFunction::LessEqual)),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene.target_format,
                    blend: Some(wgpu::BlendState { color: add, alpha: add }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let draw = make_draw(scene.multisample(false));
    let draw_picture = make_draw(Default::default());
    let mut glows = Glows {
        cull,
        draw,
        samples: scene.samples,
        draw_picture,
        source_buffer,
        cull_layout,
        draw_layout,
        sets: Vec::new(),
        sources: file.source_count,
    };
    let main = glows.make_set(device);
    glows.sets.push(Some(main));
    glows
}

/// How much of the night there is, from the colour of the sun of the frame: 1 at night, 0 in
/// the day. The game gives the moonlight as the sun at night: (0.23, 0.38, 0.69) on a clear
/// night at 23:00, (1.0, 0.99, 0.93) at noon.
pub fn night_of(sun_color: [f32; 3]) -> f32 {
    let brightness = (sun_color[0] + sun_color[1] + sun_color[2]) / 3.0;
    let t = ((brightness - 0.45) / (0.8 - 0.45)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread that ends without an answer is a failed read, once; one that has not answered
    /// yet is still to come.
    #[test]
    fn a_read_thread_that_ends_without_an_answer_is_a_failed_read() {
        let (sender, receiver) = channel();
        let mut lights = DistantLights {
            loading: Some(receiver),
            glows: None,
            file_records: None,
            file_lamps: None,
        };
        assert!(lights.take_file(false).is_none());
        assert!(lights.loading.is_some());
        drop(sender);
        assert!(matches!(lights.take_file(false), Some(Err(_))));
        assert!(lights.loading.is_none());
        assert!(lights.take_file(false).is_none());
    }

    /// The shaders of the distant lights compile as the device checks them: the cull of the
    /// glows in view, and the glows.
    #[test]
    fn the_shaders_of_the_lights_are_valid() {
        let check = crate::renderer::shader_check::check;
        check("distant lights, cull", CULL_SHADER).unwrap();
        check(
            "distant lights, glows",
            &format!("{}{GLOW_SHADER}", super::super::view::wgsl()),
        )
        .unwrap();
    }

    #[test]
    fn the_night_is_whole_in_moonlight_and_gone_in_the_day() {
        assert_eq!(night_of([0.23137257, 0.3803922, 0.6901961]), 1.0);
        assert_eq!(night_of([1.0, 0.98823535, 0.9333334]), 0.0);
        let dusk = night_of([0.6, 0.6, 0.6]);
        assert!(dusk > 0.0 && dusk < 1.0);
    }
}
