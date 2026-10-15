//! The distant statics chosen and drawn from the GPU.
//!
//! The geometry of the statics that the draws use is in pools, one for each texture: a vertex
//! buffer and an index buffer with the subsets one after the other. A draw of a pool names its
//! subset by the first index and the first vertex, and its place by the first instance, so one
//! call draws all the places of a texture that are in view.
//!
//! The places of a world space are in a list on the GPU, sorted by texture, subset and place:
//! the order of the draws of the CPU. For each set of a frame (the main view, the mirror, the
//! two layers of the shadow map) two compute passes choose the places:
//!
//! - `cull`, one thread for each place: the vis group, the subset on the GPU, the band of its
//!   tier and the end of the fog, the handoff to the game, the view, and the level of detail,
//!   as the loop of the CPU has them. It writes the triangles of the place, or 0.
//! - `compact`, one workgroup for each texture: a prefix sum over the places of the texture in
//!   the order of the list, which writes the indirect draws of the places with triangles one
//!   after the other, and their count. The order does not change from frame to frame.
//!
//! The draws are `multi_draw_indexed_indirect_count` calls, one for each texture.

use bytemuck::{Pod, Zeroable};
use nirender_wgpu::wgpu;
use nirender_wgpu::wgpu::util::DeviceExt;

/// The bytes of a vertex of a statics file.
const VERTEX: u64 = 20;
/// The places of a workgroup.
const GROUP: u32 = 256;
/// The first size of a pool, in vertices and in indexes.
const FIRST_VERTICES: u32 = 1 << 18;
const FIRST_INDEXES: u32 = 1 << 20;

/// Free ranges of a pool, first fit. The ranges are in units (vertices or indexes).
#[derive(Debug)]
pub struct Ranges {
    /// Start and length, sorted by the start, not touching each other.
    free: Vec<(u32, u32)>,
    capacity: u32,
}

impl Ranges {
    pub fn new(capacity: u32) -> Ranges {
        Ranges { free: if capacity > 0 { vec![(0, capacity)] } else { Vec::new() }, capacity }
    }

    pub fn alloc(&mut self, length: u32) -> Option<u32> {
        let at = self.free.iter().position(|&(_, free)| free >= length)?;
        let (start, free) = self.free[at];
        if free == length {
            self.free.remove(at);
        } else {
            self.free[at] = (start + length, free - length);
        }
        Some(start)
    }

    pub fn free(&mut self, start: u32, length: u32) {
        if length == 0 {
            return;
        }
        let at = self.free.partition_point(|&(other, _)| other < start);
        self.free.insert(at, (start, length));
        // Join with the range after, then with the range before.
        if at + 1 < self.free.len() && self.free[at].0 + self.free[at].1 == self.free[at + 1].0 {
            self.free[at].1 += self.free[at + 1].1;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].0 + self.free[at - 1].1 == self.free[at].0 {
            self.free[at - 1].1 += self.free[at].1;
            self.free.remove(at);
        }
    }

    /// A larger capacity: the new units at the end are free.
    pub fn grow(&mut self, capacity: u32) {
        let old = self.capacity;
        self.capacity = capacity;
        self.free(old, capacity - old);
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

/// The place of a subset in the pool of its texture.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub texture: u32,
    pub base_vertex: u32,
    pub first_index: u32,
    vertex_count: u32,
    /// The indexes in the pool: the indexes of the subset, made even.
    index_count: u32,
}

struct Pool {
    vertices: wgpu::Buffer,
    indexes: wgpu::Buffer,
    vertex_ranges: Ranges,
    index_ranges: Ranges,
}

/// The pools of the textures.
pub struct Pools {
    pools: Vec<Option<Pool>>,
    /// The largest buffer of the device.
    max_bytes: u64,
    /// The slots that went in this frame: the draws recorded in this frame can still read them.
    /// They are free at the start of the next frame.
    pending: Vec<Slot>,
    /// The bytes of the buffers of the pools.
    pub bytes: u64,
}

impl Pools {
    pub fn new(textures: usize, device: &wgpu::Device) -> Pools {
        Pools { pools: (0..textures).map(|_| None).collect(), max_bytes: device.limits().max_buffer_size, pending: Vec::new(), bytes: 0 }
    }

    /// The vertex and index buffers of a texture, when it has a pool.
    pub fn buffers(&self, texture: u32) -> Option<(&wgpu::Buffer, &wgpu::Buffer)> {
        self.pools.get(texture as usize)?.as_ref().map(|pool| (&pool.vertices, &pool.indexes))
    }

    /// Puts the vertices and the indexes of a subset into the pool of its texture. `None` when
    /// the pool cannot grow to hold it (the largest buffer of the device).
    pub fn put(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, texture: u32, vertices: &[u8], indexes: &[u8]) -> Option<Slot> {
        let vertex_count = (vertices.len() as u64 / VERTEX) as u32;
        let index_count = (indexes.len() as u32 / 2 + 1) & !1;
        let pool = self.pools.get_mut(texture as usize)?;
        if pool.is_none() {
            let vertex_capacity = FIRST_VERTICES.max(vertex_count.next_power_of_two());
            let index_capacity = FIRST_INDEXES.max(index_count.next_power_of_two());
            *pool = Some(Pool {
                vertices: pool_buffer(device, vertex_capacity as u64 * VERTEX, wgpu::BufferUsages::VERTEX),
                indexes: pool_buffer(device, index_capacity as u64 * 2, wgpu::BufferUsages::INDEX),
                vertex_ranges: Ranges::new(vertex_capacity),
                index_ranges: Ranges::new(index_capacity),
            });
            self.bytes += vertex_capacity as u64 * VERTEX + index_capacity as u64 * 2;
        }
        let pool = pool.as_mut()?;
        let base_vertex = match pool.vertex_ranges.alloc(vertex_count) {
            Some(start) => start,
            None => {
                let grown = grow(device, queue, &mut pool.vertices, &mut pool.vertex_ranges, vertex_count, VERTEX, self.max_bytes)?;
                self.bytes += grown;
                pool.vertex_ranges.alloc(vertex_count)?
            }
        };
        let first_index = match pool.index_ranges.alloc(index_count) {
            Some(start) => start,
            None => match grow(device, queue, &mut pool.indexes, &mut pool.index_ranges, index_count, 2, self.max_bytes) {
                Some(grown) => {
                    self.bytes += grown;
                    pool.index_ranges.alloc(index_count)?
                }
                None => {
                    pool.vertex_ranges.free(base_vertex, vertex_count);
                    return None;
                }
            },
        };
        queue.write_buffer(&pool.vertices, base_vertex as u64 * VERTEX, vertices);
        if indexes.len() % 4 == 0 {
            queue.write_buffer(&pool.indexes, first_index as u64 * 2, indexes);
        } else {
            let mut padded = indexes.to_vec();
            padded.extend_from_slice(&[0, 0]);
            queue.write_buffer(&pool.indexes, first_index as u64 * 2, &padded);
        }
        Some(Slot { texture, base_vertex, first_index, vertex_count, index_count })
    }

    /// The slot of a subset that left the GPU: free from the next frame on.
    pub fn remove(&mut self, slot: Slot) {
        self.pending.push(slot);
    }

    /// The start of a frame: the slots that went in the frame before are free.
    pub fn next_frame(&mut self) {
        for slot in self.pending.drain(..) {
            if let Some(Some(pool)) = self.pools.get_mut(slot.texture as usize) {
                pool.vertex_ranges.free(slot.base_vertex, slot.vertex_count);
                pool.index_ranges.free(slot.first_index, slot.index_count);
            }
        }
    }
}

fn pool_buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("distant statics, a pool"),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// A larger buffer for a pool, with the bytes of the old one. Gives the bytes added.
fn grow(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &mut wgpu::Buffer, ranges: &mut Ranges, needed: u32, unit: u64, max_bytes: u64) -> Option<u64> {
    let old = ranges.capacity();
    let wanted = (old as u64 * 2).max(old as u64 + needed as u64);
    let capacity = wanted.min(max_bytes / unit).min(u32::MAX as u64) as u32;
    if capacity < old + needed {
        return None;
    }
    let larger = pool_buffer(device, capacity as u64 * unit, buffer.usage() & (wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("distant statics, a pool grows") });
    encoder.copy_buffer_to_buffer(buffer, 0, &larger, 0, old as u64 * unit);
    queue.submit([encoder.finish()]);
    *buffer = larger;
    ranges.grow(capacity);
    Some((capacity - old) as u64 * unit)
}

/// A place on the GPU.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Place {
    pub center: [f32; 3],
    pub radius: f32,
    pub subset: u32,
    pub tier: u32,
    pub group: u32,
    pub _pad: u32,
}

/// A subset on the GPU: where it is in its pool, and its triangles for each level of detail.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub struct Subset {
    pub first_index: u32,
    pub base_vertex: u32,
    pub faces: [u32; 3],
    pub resident: u32,
    pub _pad: [u32; 2],
}

/// The values of a set for the compute passes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CullUniforms {
    pub planes: [[f32; 4]; 6],
    pub eye: [f32; 4],
    /// The ends of the three tiers, and the end of the fog.
    pub bands: [f32; 4],
    /// x: the distance before which a static is gone (the handoff).
    pub limits: [f32; 4],
    /// x: the places in the list. y: the textures (the place of the counts of the work).
    pub counts: [u32; 4],
}

/// The counts of the work of a set, on the GPU: places visited, out of their band, out of the
/// view, not on the GPU, drawn, triangles (the low word), of hidden groups, triangles (the high
/// word).
pub const STATS: usize = 8;

const SHADER: &str = "
struct Cull {
    planes: array<vec4f, 6>,
    eye: vec4f,
    bands: vec4f,
    limits: vec4f,
    counts: vec4u,
}
struct Place {
    center: vec3f,
    radius: f32,
    subset: u32,
    tier: u32,
    group: u32,
    pad: u32,
}
struct Subset {
    first_index: u32,
    base_vertex: u32,
    faces0: u32,
    faces1: u32,
    faces2: u32,
    resident: u32,
    pad0: u32,
    pad1: u32,
}
struct Draw {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}
@group(0) @binding(0) var<uniform> c: Cull;
@group(0) @binding(1) var<storage, read> places: array<Place>;
@group(0) @binding(2) var<storage, read> subsets: array<Subset>;
@group(0) @binding(3) var<storage, read> groups: array<u32>;
@group(0) @binding(4) var<storage, read> list: array<u32>;
@group(0) @binding(5) var<storage, read> buckets: array<vec2u>;
@group(0) @binding(6) var<storage, read_write> flags: array<u32>;
@group(0) @binding(7) var<storage, read_write> draws: array<Draw>;
// The draws of each texture, then the counts of the work (`STATS`, from c.counts.y on).
@group(0) @binding(8) var<storage, read_write> counts: array<atomic<u32>>;

@compute @workgroup_size(256)
fn cull(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if i >= c.counts.x {
        return;
    }
    flags[i] = 0u;
    let place = places[list[i]];
    if place.group != 0u && place.group < arrayLength(&groups) && groups[place.group] == 0u {
        atomicAdd(&counts[c.counts.y + 6u], 1u);
        return;
    }
    atomicAdd(&counts[c.counts.y + 0u], 1u);
    let s = subsets[place.subset];
    if s.resident == 0u {
        atomicAdd(&counts[c.counts.y + 3u], 1u);
        return;
    }
    let range = length(place.center - c.eye.xyz);
    let band_end = min(c.bands[place.tier], c.bands.w);
    if range > band_end + place.radius || range + place.radius < c.limits.x {
        atomicAdd(&counts[c.counts.y + 1u], 1u);
        return;
    }
    for (var k = 0u; k < 6u; k++) {
        let p = c.planes[k];
        if dot(p.xyz, place.center) + p.w < -place.radius {
            atomicAdd(&counts[c.counts.y + 2u], 1u);
            return;
        }
    }
    var faces = s.faces2;
    if range <= c.bands.x + place.radius {
        faces = s.faces0;
    } else if range <= c.bands.y + place.radius {
        faces = s.faces1;
    }
    if faces != 0u {
        flags[i] = faces;
        atomicAdd(&counts[c.counts.y + 4u], 1u);
        // The triangles in two words: the low word, and its carry in the high word (7).
        let low = atomicAdd(&counts[c.counts.y + 5u], faces);
        if low > 0xffffffffu - faces {
            atomicAdd(&counts[c.counts.y + 7u], 1u);
        }
    }
}

var<workgroup> scan: array<u32, 256>;
var<workgroup> base: u32;

@compute @workgroup_size(256)
fn compact(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) t: u32) {
    let bucket = buckets[group.x];
    if t == 0u {
        base = 0u;
    }
    workgroupBarrier();
    for (var chunk = 0u; chunk < bucket.y; chunk += 256u) {
        let j = chunk + t;
        var faces = 0u;
        if j < bucket.y {
            faces = flags[bucket.x + j];
        }
        let v = select(0u, 1u, faces != 0u);
        scan[t] = v;
        workgroupBarrier();
        for (var offset = 1u; offset < 256u; offset *= 2u) {
            var add = 0u;
            if t >= offset {
                add = scan[t - offset];
            }
            workgroupBarrier();
            scan[t] += add;
            workgroupBarrier();
        }
        let start = base;
        if v == 1u {
            let instance = list[bucket.x + j];
            let s = subsets[places[instance].subset];
            draws[bucket.x + start + scan[t] - 1u] = Draw(faces * 3u, 1u, s.first_index, i32(s.base_vertex), instance);
        }
        workgroupBarrier();
        if t == 255u {
            base = start + scan[255];
        }
        workgroupBarrier();
    }
    if t == 0u {
        atomicStore(&counts[group.x], base);
    }
}
";

/// The buffers of one set for the compute passes and the draws.
struct SetBuffers {
    uniforms: wgpu::Buffer,
    draws: wgpu::Buffer,
    /// The draws of each texture, then the counts of the work.
    counts: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// The list of the places of a world space, by texture.
struct WorldList {
    name: Vec<u8>,
    list: wgpu::Buffer,
    buckets: wgpu::Buffer,
    /// Start and length in the list for each texture.
    ranges: Vec<(u32, u32)>,
    length: u32,
    sets: Vec<SetBuffers>,
}

/// The compute passes and their buffers.
pub struct Culling {
    layout: wgpu::BindGroupLayout,
    cull: wgpu::ComputePipeline,
    compact: wgpu::ComputePipeline,
    places: wgpu::Buffer,
    pub subsets: wgpu::Buffer,
    groups: wgpu::Buffer,
    world: Option<WorldList>,
    textures: usize,
}

/// True when the device can choose and draw the statics from the GPU.
pub fn supported(device: &wgpu::Device) -> bool {
    device.features().contains(wgpu::Features::INDIRECT_FIRST_INSTANCE | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
}

impl Culling {
    pub fn new(device: &wgpu::Device, places: &[Place], subset_count: usize, textures: usize) -> Culling {
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("distant statics, cull"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, true),
                storage(5, true),
                storage(6, false),
                storage(7, false),
                storage(8, false),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("distant statics, cull"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("distant statics, cull"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("distant statics, cull"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let places_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("distant statics, the places"),
            contents: if places.is_empty() { bytemuck::bytes_of(&[0u32; 8]) } else { bytemuck::cast_slice(places) },
            usage: wgpu::BufferUsages::STORAGE,
        });
        let subsets = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics, the subsets"),
            size: (subset_count.max(1) * size_of::<Subset>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let groups = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("distant statics, the vis groups"),
            contents: bytemuck::bytes_of(&[1u32; 4]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        Culling { layout, cull: make("cull"), compact: make("compact"), places: places_buffer, subsets, groups, world: None, textures }
    }

    /// The state of the vis groups: a value for each group, 0 for a hidden one; empty for all
    /// shown.
    pub fn set_groups(&mut self, device: &wgpu::Device, shown: &[u8]) {
        let values: Vec<u32> = if shown.is_empty() { vec![1; 4] } else { shown.iter().map(|&s| s as u32).collect() };
        self.groups = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("distant statics, the vis groups"),
            contents: bytemuck::cast_slice(&values),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // The bind groups name the buffer: they are made again.
        if let Some(world) = self.world.take() {
            self.world = Some(self.make_world(device, world.name, &[], Some((world.list, world.buckets, world.ranges, world.length))));
        }
    }

    /// True when the list is that of this world space.
    pub fn has_world(&self, name: &[u8]) -> bool {
        self.world.as_ref().is_some_and(|world| world.name == name)
    }

    /// The list of a world space: `places` are the places of its statics sorted by texture,
    /// subset and place, with their texture.
    pub fn set_world(&mut self, device: &wgpu::Device, name: &[u8], places: &[(u32, u32)]) {
        if self.world.as_ref().is_some_and(|world| world.name == name) {
            return;
        }
        self.world = Some(self.make_world(device, name.to_vec(), places, None));
    }

    fn make_world(&self, device: &wgpu::Device, name: Vec<u8>, places: &[(u32, u32)], kept: Option<(wgpu::Buffer, wgpu::Buffer, Vec<(u32, u32)>, u32)>) -> WorldList {
        let (list, buckets, ranges, length) = kept.unwrap_or_else(|| {
            let mut ranges = vec![(0u32, 0u32); self.textures.max(1)];
            for (at, &(texture, _)) in places.iter().enumerate() {
                let range = &mut ranges[texture as usize];
                if range.1 == 0 {
                    range.0 = at as u32;
                }
                range.1 += 1;
            }
            let indexes: Vec<u32> = places.iter().map(|&(_, place)| place).collect();
            let list = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("distant statics, the places of a world space"),
                contents: if indexes.is_empty() { bytemuck::bytes_of(&[0u32; 4]) } else { bytemuck::cast_slice(&indexes) },
                usage: wgpu::BufferUsages::STORAGE,
            });
            let pairs: Vec<[u32; 2]> = ranges.iter().map(|&(start, length)| [start, length]).collect();
            let buckets = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("distant statics, the textures of a world space"),
                contents: bytemuck::cast_slice(&pairs),
                usage: wgpu::BufferUsages::STORAGE,
            });
            (list, buckets, ranges, indexes.len() as u32)
        });
        let slots = length.max(1) as u64;
        let sets = (0..4)
            .map(|_| {
                let make = |label: &str, size: u64, usage: wgpu::BufferUsages| {
                    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
                };
                let uniforms = make("distant statics, cull values", size_of::<CullUniforms>() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
                let flags = make("distant statics, the triangles of each place", slots * 4, wgpu::BufferUsages::STORAGE);
                let draws = make("distant statics, the draws", slots * 20, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT);
                let counts = make(
                    "distant statics, the draws of each texture and the counts",
                    (ranges.len() + STATS) as u64 * 4,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                );
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("distant statics, cull of a world"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: self.places.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: self.subsets.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 3, resource: self.groups.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 4, resource: list.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 5, resource: buckets.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 6, resource: flags.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 7, resource: draws.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 8, resource: counts.as_entire_binding() },
                    ],
                });
                SetBuffers { uniforms, draws, counts, group }
            })
            .collect();
        WorldList { name, list, buckets, ranges, length, sets }
    }

    /// Runs the compute passes of a set in a command buffer of their own, before the commands
    /// of the frame that draw them.
    pub fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, uniforms: &CullUniforms) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("distant statics, cull") });
        self.encode(queue, &mut encoder, set, uniforms);
        queue.submit([encoder.finish()]);
    }

    /// Puts the compute passes of a set into `encoder`, which goes to the GPU before the
    /// commands that draw the set.
    pub fn encode(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, set: usize, uniforms: &CullUniforms) {
        let Some(world) = self.world.as_ref() else { return };
        let Some(buffers) = world.sets.get(set) else { return };
        let mut values = *uniforms;
        values.counts[0] = world.length;
        values.counts[1] = world.ranges.len() as u32;
        queue.write_buffer(&buffers.uniforms, 0, bytemuck::bytes_of(&values));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("distant statics, cull"), timestamp_writes: None });
        pass.set_bind_group(0, &buffers.group, &[]);
        if world.length > 0 {
            pass.set_pipeline(&self.cull);
            pass.dispatch_workgroups(world.length.div_ceil(GROUP), 1, 1);
        }
        pass.set_pipeline(&self.compact);
        pass.dispatch_workgroups(world.ranges.len() as u32, 1, 1);
    }

    /// Records the draws of a set: one indirect call for each texture with places. `bind`
    /// sets the texture of a texture and gives false when it has no pool.
    pub fn draw(&self, set: usize, pass: &mut wgpu::RenderPass<'static>, mut bind: impl FnMut(&mut wgpu::RenderPass<'static>, u32) -> bool) -> u32 {
        let Some(world) = self.world.as_ref() else { return 0 };
        let Some(buffers) = world.sets.get(set) else { return 0 };
        let mut calls = 0;
        for (texture, &(start, length)) in world.ranges.iter().enumerate() {
            if length == 0 || !bind(pass, texture as u32) {
                continue;
            }
            pass.multi_draw_indexed_indirect_count(&buffers.draws, start as u64 * 20, &buffers.counts, texture as u64 * 4, length);
            calls += 1;
        }
        calls
    }

    /// Reads the counts of the sets since the last call, and sets them to 0. Waits for the GPU.
    pub fn take_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<[u32; STATS]> {
        let Some(world) = self.world.as_ref() else { return Vec::new() };
        let size = STATS as u64 * 4;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("distant statics, cull counts read"),
            size: size * world.sets.len() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("distant statics, cull counts") });
        for (at, buffers) in world.sets.iter().enumerate() {
            let start = world.ranges.len() as u64 * 4;
            encoder.copy_buffer_to_buffer(&buffers.counts, start, &read, at as u64 * size, size);
            encoder.clear_buffer(&buffers.counts, start, Some(size));
        }
        queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = read.slice(..).get_mapped_range() else { return Vec::new() };
        let words: &[u32] = bytemuck::cast_slice(&data);
        words.chunks(STATS).map(|chunk| core::array::from_fn(|i| chunk[i])).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Ranges;

    #[test]
    fn ranges_give_the_first_fit_and_join_the_free_ones() {
        let mut ranges = Ranges::new(100);
        assert_eq!(ranges.alloc(30), Some(0));
        assert_eq!(ranges.alloc(30), Some(30));
        assert_eq!(ranges.alloc(30), Some(60));
        assert_eq!(ranges.alloc(30), None);
        ranges.free(30, 30);
        assert_eq!(ranges.alloc(40), None);
        ranges.free(0, 30);
        assert_eq!(ranges.alloc(40), Some(0));
        ranges.free(0, 40);
        ranges.free(60, 30);
        assert_eq!(ranges.free, vec![(0, 100)]);
    }

    #[test]
    fn ranges_grow_at_the_end() {
        let mut ranges = Ranges::new(10);
        assert_eq!(ranges.alloc(10), Some(0));
        ranges.grow(25);
        assert_eq!(ranges.alloc(15), Some(10));
        assert_eq!(ranges.capacity(), 25);
    }
}
