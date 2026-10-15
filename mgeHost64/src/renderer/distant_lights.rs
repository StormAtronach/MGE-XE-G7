//! The distant lights of the wgpu renderer: the light sources of the exterior beyond the cells
//! that the game has loaded, from `distantland\lights.data` of the generator.
//!
//! - The glows: a compute pass chooses the sources with a mesh that are in the view, past the
//!   handoff to the game (where the game draws the real light) and within the draw distance,
//!   and writes them into a list; one indirect draw puts a quad that faces the camera at each,
//!   added to the picture, with the colour of the light. A glow has a least size on the screen,
//!   so that a far light stays a point; it is dimmer then. It goes away in the day, and the fog
//!   takes it as it takes the land. A light that flickers or pulses in the game does so here.
//! - The baked light: the generator gives the light of the sources on the land of each lit
//!   cell, a tile of 65 x 65 values (the vertices of the land). The tiles are in one texture,
//!   with a texture of the tile of each cell. The shaders of the land and of the statics add it
//!   to their light at night (`baked_light`, in the bind group of `LightBinding`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use nirender_wgpu::SceneView;
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;

const MAGIC: &[u8; 8] = b"XELIGHT1";
const HEADER: usize = 40;
const SOURCE: usize = 32;
/// The values on a side of a tile.
const TILE: u32 = 65;
/// The tiles in a row of the texture of the tiles.
const TILES_IN_ROW: u32 = 64;
/// The bytes of a value of a tile: red, green, blue, 255, each half of the light.
const TEXEL: u32 = 4;
/// The values of a source in the list of the glows: two rows of four numbers.
const GLOW_WORDS: u64 = 8;

/// The baked light for a shader of the land or of the statics: `GROUP` is the number of the
/// bind group of `LightBinding` in that shader. `baked_light(world)` is the light at a place.
const BAKED_WGSL: &str = "
struct LightMap {
    // The first cell of the texture of the tiles of the cells, and its size in cells.
    origin: vec4f,
    // x: the strength (0 for none). y: the tiles in a row of the texture of the tiles.
    values: vec4f,
}
@group(GROUP) @binding(0) var<uniform> light_map: LightMap;
@group(GROUP) @binding(1) var light_tiles: texture_2d<f32>;
@group(GROUP) @binding(2) var light_index: texture_2d<i32>;
@group(GROUP) @binding(3) var light_sampler: sampler;

fn baked_light(world: vec3f) -> vec3f {
    if light_map.values.x <= 0.0 {
        return vec3f(0.0);
    }
    let cell = vec2i(floor(world.xy / 8192.0));
    let at = cell - vec2i(light_map.origin.xy);
    if any(at < vec2i(0)) || any(at >= vec2i(light_map.origin.zw)) {
        return vec3f(0.0);
    }
    let tile = textureLoad(light_index, at, 0).r;
    if tile < 0 {
        return vec3f(0.0);
    }
    let in_row = i32(light_map.values.y);
    let corner = vec2f(f32(tile % in_row), f32(tile / in_row)) * 65.0;
    // The values are at the vertices of the land, 128 units apart, from the corner of the cell.
    let local = clamp((world.xy - vec2f(cell) * 8192.0) / 128.0, vec2f(0.0), vec2f(64.0));
    let uv = (corner + local + 0.5) / vec2f(textureDimensions(light_tiles));
    // A byte holds half of the light.
    return 2.0 * textureSampleLevel(light_tiles, light_sampler, uv, 0.0).rgb * light_map.values.x;
}
";

/// `BAKED_WGSL` for the bind group `group`.
pub fn baked_wgsl(group: u32) -> String {
    BAKED_WGSL.replace("GROUP", &group.to_string())
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MapUniforms {
    origin: [f32; 4],
    values: [f32; 4],
}

/// The bind group of the baked light that the land and the statics read. It has no light until
/// a file comes (`DistantLights::poll`).
pub struct LightBinding {
    pub layout: wgpu::BindGroupLayout,
    pub group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    map: MapUniforms,
}

impl LightBinding {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> LightBinding {
        let texture_entry = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("distant lights, baked"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture_entry(1, wgpu::TextureSampleType::Float { filterable: true }),
                texture_entry(2, wgpu::TextureSampleType::Sint),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let map = MapUniforms { origin: [0.0, 0.0, 1.0, 1.0], values: [0.0, TILES_IN_ROW as f32, 0.0, 0.0] };
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("distant lights, baked"),
            contents: bytemuck::bytes_of(&map),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("distant lights, baked"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let tiles = texture(device, queue, "distant lights, the tiles", 1, 1, wgpu::TextureFormat::Rgba8Unorm, &[0; 4]);
        let index = texture(device, queue, "distant lights, the tile of each cell", 1, 1, wgpu::TextureFormat::R32Sint, &(-1i32).to_le_bytes());
        let group = make_group(device, &layout, &uniforms, &tiles, &index, &sampler);
        LightBinding { layout, group, uniforms, sampler, map }
    }

    /// The tiles of a file: one texture with the tiles in rows, and one with the tile of each
    /// cell (-1 for none).
    fn set_tiles(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, file: &LightsFile) {
        if file.tiles.is_empty() {
            return;
        }
        let (min_x, min_y) = file.tiles.iter().fold((i32::MAX, i32::MAX), |(x, y), cell| (x.min(cell.0), y.min(cell.1)));
        let (max_x, max_y) = file.tiles.iter().fold((i32::MIN, i32::MIN), |(x, y), cell| (x.max(cell.0), y.max(cell.1)));
        let (width, height) = ((max_x - min_x + 1) as u32, (max_y - min_y + 1) as u32);
        let mut index = vec![-1i32; (width * height) as usize];
        for (tile, cell) in file.tiles.iter().enumerate() {
            index[((cell.1 - min_y) as u32 * width + (cell.0 - min_x) as u32) as usize] = tile as i32;
        }
        let count = file.tiles.len() as u32;
        let rows = count.div_ceil(TILES_IN_ROW);
        let (atlas_width, atlas_height) = (TILES_IN_ROW.min(count) * TILE, rows * TILE);
        // From the tiles one after the other to rows of tiles.
        let mut atlas = vec![0u8; (atlas_width * atlas_height * TEXEL) as usize];
        let tile_bytes = (TILE * TILE * TEXEL) as usize;
        for tile in 0..count {
            let (column, row) = (tile % TILES_IN_ROW, tile / TILES_IN_ROW);
            let source = &file.texels[tile as usize * tile_bytes..(tile as usize + 1) * tile_bytes];
            for y in 0..TILE {
                let from = (y * TILE * TEXEL) as usize;
                let to = (((row * TILE + y) * atlas_width + column * TILE) * TEXEL) as usize;
                atlas[to..to + (TILE * TEXEL) as usize].copy_from_slice(&source[from..from + (TILE * TEXEL) as usize]);
            }
        }
        let tiles = texture(device, queue, "distant lights, the tiles", atlas_width, atlas_height, wgpu::TextureFormat::Rgba8Unorm, &atlas);
        let index_bytes: Vec<u8> = index.iter().flat_map(|value| value.to_le_bytes()).collect();
        let index = texture(device, queue, "distant lights, the tile of each cell", width, height, wgpu::TextureFormat::R32Sint, &index_bytes);
        self.group = make_group(device, &self.layout, &self.uniforms, &tiles, &index, &self.sampler);
        self.map.origin = [min_x as f32, min_y as f32, width as f32, height as f32];
    }

    /// The strength of the baked light for the frame: 0 for none.
    pub fn set_strength(&mut self, queue: &wgpu::Queue, strength: f32) {
        self.map.values[0] = strength;
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&self.map));
    }
}

fn texture(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, width: u32, height: u32, format: wgpu::TextureFormat, data: &[u8]) -> wgpu::TextureView {
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            data,
        )
        .create_view(&Default::default())
}

fn make_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, tiles: &wgpu::TextureView, index: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("distant lights, baked"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tiles) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(index) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// What the file has.
struct LightsFile {
    /// The sources as the file has them, 32 bytes each: the layout of `Source` of the shader.
    sources: Vec<u8>,
    source_count: u32,
    tiles: Vec<(i32, i32)>,
    /// The values of the tiles, one tile after the other.
    texels: Vec<u8>,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read(path: &Path) -> Result<LightsFile, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || word(&bytes, 8) != 2 || word(&bytes, 20) != TILE {
        return Err(format!("{} is not a lights file of version 2", path.display()));
    }
    let (source_count, tile_count) = (word(&bytes, 12) as usize, word(&bytes, 16) as usize);
    let sources_end = HEADER + source_count * SOURCE;
    let cells_end = sources_end + tile_count * 8;
    let texels_end = cells_end + tile_count * (TILE * TILE * TEXEL) as usize;
    if bytes.len() != texels_end {
        return Err(format!("{} has {} bytes, not {texels_end}", path.display(), bytes.len()));
    }
    let tiles = (0..tile_count)
        .map(|tile| {
            let at = sources_end + tile * 8;
            (word(&bytes, at) as i32, word(&bytes, at + 4) as i32)
        })
        .collect();
    Ok(LightsFile {
        sources: bytes[HEADER..sources_end].to_vec(),
        source_count: source_count as u32,
        tiles,
        texels: bytes[cells_end..texels_end].to_vec(),
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

const GLOW_SHADER: &str = "
struct View {
    view_projection: mat4x4f,
    eye: vec4f,
    fog: vec4f,
    flags: vec4f,
    // x: the size as a part of the radius, y: the strength, z: the least size in pixels,
    // w: the most size in pixels.
    glow: vec4f,
    // x, y: the size of the picture; z: the pixels of one unit at a distance of one unit.
    screen: vec4f,
}
@group(0) @binding(0) var<uniform> u: View;
@group(0) @binding(1) var<storage, read> glows: array<vec4f>;

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
    return select(linear, exponential, u.flags.x > 0.5 && distance > u.eye.w);
}

@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Out {
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let place = glows[instance * 2u];
    let look = glows[instance * 2u + 1u];
    var clip = u.view_projection * vec4f(place.xyz, 1.0);
    let w = max(clip.w, 1.0);
    let wanted = place.w * u.glow.x * u.screen.z / w;
    let pixels = clamp(wanted, u.glow.z, u.glow.w);
    // A glow drawn larger than it is gives the light of its size, not more.
    let dim = clamp(wanted / pixels, 0.25, 1.0);
    let corner = corners[vertex];
    var out: Out;
    out.position = vec4f(clip.xy + corner * pixels * 2.0 / u.screen.xy * clip.w, clip.zw);
    out.corner = corner;
    out.color = look.rgb * look.w * dim * u.glow.y * fog_left(place.xyz);
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniforms {
    view_projection: [f32; 16],
    eye: [f32; 4],
    fog: [f32; 4],
    flags: [f32; 4],
    glow: [f32; 4],
    screen: [f32; 4],
}

/// The values of a frame for the glows.
pub struct GlowFrame {
    pub view_projection: [f32; 16],
    /// The camera, and the view range of the game in w.
    pub eye: [f32; 4],
    pub planes: [[f32; 4]; 6],
    pub fog: [f32; 4],
    pub flags: [f32; 4],
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

struct Glows {
    cull: wgpu::ComputePipeline,
    cull_group: wgpu::BindGroup,
    cull_uniforms: wgpu::Buffer,
    args: wgpu::Buffer,
    draw: wgpu::RenderPipeline,
    draw_group: wgpu::BindGroup,
    view_uniforms: wgpu::Buffer,
    sources: u32,
}

pub struct DistantLights {
    loading: Option<Receiver<Result<LightsFile, String>>>,
    glows: Option<Glows>,
    /// The frames with glows chosen, for the report.
    pub frames: u64,
}

impl DistantLights {
    /// Starts the read of `<folder>\lights.data`.
    pub fn start(folder: &Path) -> DistantLights {
        let path: PathBuf = folder.join("lights.data");
        let (sender, receiver) = channel();
        thread::spawn(move || {
            let _ = sender.send(read(&path));
        });
        DistantLights { loading: Some(receiver), glows: None, frames: 0 }
    }

    /// Takes the file when it has been read: the baked light into `binding`, and the glows.
    pub fn poll(&mut self, scene: &SceneView, binding: &mut LightBinding, synchronous: bool) {
        let Some(receiver) = self.loading.as_ref() else { return };
        let result = if synchronous { receiver.recv().ok() } else { receiver.try_recv().ok() };
        let Some(result) = result else { return };
        self.loading = None;
        let file = match result {
            Ok(file) => file,
            Err(problem) => {
                println!("distant lights: none ({problem})");
                return;
            }
        };
        println!("distant lights: {} light sources, {} lit cells baked", file.source_count, file.tiles.len());
        binding.set_tiles(scene.device, scene.queue, &file);
        if file.source_count > 0 {
            self.glows = Some(make_glows(scene, &file));
        }
    }

    /// Chooses the glows of the frame, in a command buffer of its own before the frame.
    pub fn prepare(&mut self, scene: &SceneView, frame: &GlowFrame) {
        let Some(glows) = self.glows.as_ref() else { return };
        let cull = CullUniforms {
            planes: frame.planes,
            eye: frame.eye,
            range: [frame.fade[0], frame.fade[1], frame.draw_distance, frame.time],
            counts: [glows.sources, 0, 0, 0],
        };
        scene.queue.write_buffer(&glows.cull_uniforms, 0, bytemuck::bytes_of(&cull));
        // Six corners for each glow; the cull adds the glows.
        scene.queue.write_buffer(&glows.args, 0, bytemuck::cast_slice(&[6u32, 0, 0, 0]));
        let view = ViewUniforms {
            view_projection: frame.view_projection,
            eye: frame.eye,
            fog: frame.fog,
            flags: frame.flags,
            glow: [frame.glow[0], frame.glow[1], frame.glow[2], 64.0],
            screen: [frame.screen[0], frame.screen[1], frame.screen[2], 0.0],
        };
        scene.queue.write_buffer(&glows.view_uniforms, 0, bytemuck::bytes_of(&view));
        let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("distant lights, cull") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("distant lights, cull"), timestamp_writes: None });
            pass.set_pipeline(&glows.cull);
            pass.set_bind_group(0, &glows.cull_group, &[]);
            pass.dispatch_workgroups(glows.sources.div_ceil(256), 1, 1);
        }
        scene.commands.defer(encoder.finish());
        self.frames += 1;
    }

    /// Draws the glows chosen in `prepare` into the pass: after the land and the statics,
    /// against their depth.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        let Some(glows) = self.glows.as_ref() else { return };
        pass.set_pipeline(&glows.draw);
        pass.set_bind_group(0, &glows.draw_group, &[]);
        pass.draw_indirect(&glows.args, 0);
    }
}

fn make_glows(scene: &SceneView, file: &LightsFile) -> Glows {
    let device = scene.device;
    let sources = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("distant lights, the sources"),
        contents: &file.sources,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let list = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("distant lights, the glows of a frame"),
        size: file.source_count.max(1) as u64 * GLOW_WORDS * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let args = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("distant lights, the draw"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_buffer = |label: &str, size: usize| {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    };
    let cull_uniforms = uniform_buffer("distant lights, cull", size_of::<CullUniforms>());
    let view_uniforms = uniform_buffer("distant lights, glows", size_of::<ViewUniforms>());
    let entry = |binding: u32, stage: wgpu::ShaderStages, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let compute = wgpu::ShaderStages::COMPUTE;
    let cull_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant lights, cull"),
        entries: &[
            entry(0, compute, wgpu::BufferBindingType::Uniform),
            entry(1, compute, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(2, compute, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(3, compute, wgpu::BufferBindingType::Storage { read_only: false }),
        ],
    });
    let cull_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("distant lights, cull"),
        layout: &cull_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cull_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: sources.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: list.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: args.as_entire_binding() },
        ],
    });
    let cull_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant lights, cull"), source: wgpu::ShaderSource::Wgsl(CULL_SHADER.into()) });
    let cull_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("distant lights, cull"), bind_group_layouts: &[Some(&cull_layout)], immediate_size: 0 });
    let cull = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("distant lights, cull"),
        layout: Some(&cull_pipeline_layout),
        module: &cull_module,
        entry_point: Some("cull"),
        compilation_options: Default::default(),
        cache: None,
    });
    let draw_stages = wgpu::ShaderStages::VERTEX_FRAGMENT;
    let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("distant lights, glows"),
        entries: &[entry(0, draw_stages, wgpu::BufferBindingType::Uniform), entry(1, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Storage { read_only: true })],
    });
    let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("distant lights, glows"),
        layout: &draw_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: view_uniforms.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: list.as_entire_binding() }],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant lights, glows"), source: wgpu::ShaderSource::Wgsl(GLOW_SHADER.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("distant lights, glows"), bind_group_layouts: &[Some(&draw_layout)], immediate_size: 0 });
    let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
    let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("distant lights, glows"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: scene.depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
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
    });
    Glows { cull, cull_group, cull_uniforms, args, draw, draw_group, view_uniforms, sources: file.source_count }
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

    #[test]
    fn the_night_is_whole_in_moonlight_and_gone_in_the_day() {
        assert_eq!(night_of([0.23137257, 0.3803922, 0.6901961]), 1.0);
        assert_eq!(night_of([1.0, 0.98823535, 0.9333334]), 0.0);
        let dusk = night_of([0.6, 0.6, 0.6]);
        assert!(dusk > 0.0 && dusk < 1.0);
    }

    #[test]
    fn the_baked_code_names_its_group() {
        let code = baked_wgsl(2);
        assert!(code.contains("@group(2) @binding(1)"));
        assert!(!code.contains("GROUP"));
    }
}
