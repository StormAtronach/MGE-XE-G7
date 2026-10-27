//! The GPU time of the work that the renderer sends to the GPU in command buffers of its own,
//! before the frame (the shadow map, the reflection in the water), with `MGE_XE_GPU_TIME=1`:
//! the time stamps of the frame (nirender) do not have it. The renderer waits for the stamps
//! of each part, so only a replay (the extension offline) reads them.

use nirender_wgpu::wgpu;

/// The parts, by their index.
pub(super) const PARTS: [&str; 2] = ["shadow map", "reflection"];
pub(super) const SHADOW: usize = 0;
pub(super) const REFLECTION: usize = 1;

pub(super) struct GpuTime {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// Nanoseconds for each stamp step.
    period: f64,
    /// The nanoseconds and the number of times of each part.
    totals: [(f64, u32); PARTS.len()],
}

impl GpuTime {
    /// [`None`] without `MGE_XE_GPU_TIME=1` or without stamps in an encoder.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTime> {
        let features = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if !nirender_wgpu::switches::gpu_time() || !device.features().contains(features) {
            return None;
        }
        let buffer = |usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some("gpu time"), size: 16, usage, mapped_at_creation: false });
        Some(GpuTime {
            set: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("gpu time"), ty: wgpu::QueryType::Timestamp, count: 2 }),
            resolve: buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            read: buffer(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
            period: queue.get_timestamp_period() as f64,
            totals: [(0.0, 0); PARTS.len()],
        })
    }

    /// The start of a part, at the start of its encoder.
    pub fn start(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.write_timestamp(&self.set, 0);
    }

    /// The end of a part, at the end of its encoder: the stamps go to the buffer that is read.
    pub fn stop(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.write_timestamp(&self.set, 1);
        encoder.resolve_query_set(&self.set, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, 16);
    }

    /// After the submit of the encoder of `part`: waits for its stamps and adds its time.
    pub fn add(&mut self, device: &wgpu::Device, part: usize) {
        let slice = self.read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if let Ok(mapped) = slice.get_mapped_range() {
            let stamp = |i: usize| u64::from_le_bytes(mapped[i * 8..i * 8 + 8].try_into().unwrap());
            let (begin, end) = (stamp(0), stamp(1));
            if begin != 0 && end > begin {
                self.totals[part].0 += (end - begin) as f64 * self.period;
                self.totals[part].1 += 1;
            }
        }
        self.read.unmap();
    }

    /// The line of the report: the mean time of each part that ran.
    pub fn report(&self) -> String {
        let parts: Vec<String> =
            PARTS.iter().zip(self.totals).filter(|(_, (_, count))| *count > 0).map(|(name, (total, count))| format!("{name} {:.3} ms ({count} times)", total / 1e6 / count as f64)).collect();
        format!("gpu of the parts before the frame: {}", if parts.is_empty() { "none".to_string() } else { parts.join(", ") })
    }
}
