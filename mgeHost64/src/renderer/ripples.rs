//! The dynamic waves of MGE XE (`distant_land.water.dynamic_ripples`): the ripples of the rain
//! and the ripples around the player, as `DistantLand::simulateDynamicWaves` and the passes
//! `WaveStepPS` and `PlayerWavePS` of `XE Main.fx` have them.
//!
//! Each of the two is a height field in a texture of 512 x 512 pixels of 16-bit floats: red
//! is the height now, green the height of the step before, blue and alpha the slope (the
//! part of the normal that the water shading adds). One step of the wave equation is one
//! pass from one texture into the other, 80 steps for each second of the game.
//!
//! - The rain texture repeats over the water (the coordinates of the close waves). Drops
//!   fall on it at a rate that follows the weather.
//! - The player texture lies on the world around the player, 2.5 units for each pixel, and
//!   moves with the player by whole pixels. The player in the water pushes it down in a
//!   ring of 12 units.
//!
//! The textures are in the world by x and y, and not by a water level: each water surface
//! reads them with its own position (the plane now; a water volume reads the same two on
//! the Direct3D 9 path).

use nirender_wgpu::{SceneView, wgpu};

/// The size of each texture, in pixels (`waveTexResolution`).
pub const SIZE: u32 = 512;
/// The world units of one pixel of the player texture (`waveTexWorldRes`).
pub const WORLD_PIXEL: f32 = 2.5;
/// The world size of the player texture (`waveTexWorldSize`).
pub const WORLD_SIZE: f32 = SIZE as f32 * WORLD_PIXEL;
/// The time of one step of the wave equation, in seconds (1 / 80).
const STEP: f32 = 0.0125;
/// The most game time that one frame simulates, in seconds.
const MOST_FRAME_TIME: f32 = 0.5;
/// The uniform slots of one frame: one for each player pass (0.5 s / 0.0125 s = 40), and the
/// first for the step passes.
const SLOTS: u64 = 48;
const SLOT_BYTES: u64 = 256;

const SHADER: &str = "
struct Uniforms {
    // The middle of the ring of the player in the texture, the time of the game.
    origin: vec2f,
    time: f32,
    pad: f32,
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

const RCP: f32 = 1.0 / 512.0;
// 12 world units, in the texture.
const PLAYER_WAVE_SIZE: f32 = 12.0 / (512.0 * 2.5);

@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    // One triangle over all of the texture.
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
}

fn at(tex: vec2f) -> vec4f {
    return textureSampleLevel(source, source_sampler, tex, 0.0);
}

// WaveStepPS: the damped wave equation in two dimensions.
//   u(t + 1) = a * nsum + ((2 - udamp - vdamp) - 4 * a) * u(t) - (1 - vdamp) * u(t - 1)
@fragment fn step_fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let tex = position.xy * RCP;
    let c = at(tex);
    let n = vec4f(at(tex + vec2f(RCP, 0.0)).r, at(tex + vec2f(-RCP, 0.0)).r, at(tex + vec2f(0.0, RCP)).r, at(tex + vec2f(0.0, -RCP)).r);
    let n2 = vec4f(at(tex + vec2f(1.5 * RCP, 0.0)).r, at(tex + vec2f(-1.5 * RCP, 0.0)).r, at(tex + vec2f(0.0, 1.5 * RCP)).r, at(tex + vec2f(0.0, -1.5 * RCP)).r);
    let nsum = n.x + n.y + n.z + n.w;
    let height = 0.14 * nsum + (1.96 - 0.56) * c.r - 0.98 * c.g;
    // The slope, for the normal of the water.
    let slope = 2.0 * (n.xy - n.zw) + 0.5 * (n2.xy - n2.zw);
    return vec4f(height, c.r, slope);
}

// PlayerWavePS: the water goes down in a ring around the player. The size of the ring
// moves a little with the time.
@fragment fn player_fs(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let tex = position.xy * RCP;
    var ret = at(tex);
    let wavesize = (1.0 + 0.055 * sin(16.0 * u.time) + 0.065 * sin(12.87645 * u.time)) * PLAYER_WAVE_SIZE;
    let displace = clamp(2.0 * abs(length(tex - u.origin) / wavesize - 1.0), 0.0, 1.0);
    ret = vec4f(mix(vec2f(-1.0), ret.rg, displace), ret.ba);
    return ret;
}
";

/// The number of steps of the wave equation for a frame of `frame_time` seconds; `remaining`
/// keeps the part of a step that is left for the next frame.
pub fn steps_of(remaining: &mut f32, frame_time: f32) -> u32 {
    *remaining += frame_time.min(MOST_FRAME_TIME);
    let steps = (*remaining / STEP) as u32;
    *remaining -= steps as f32 * STEP;
    steps
}

/// The part of the player texture that stays when the player moved by (`shift_x`,
/// `shift_y`) pixels: its corner in the texture before, its corner in the texture after,
/// and its size. `None` when nothing stays. The first row and the first column are never
/// kept, as on the Direct3D 9 path.
pub fn kept_part(shift_x: i32, shift_y: i32) -> Option<([u32; 2], [u32; 2], [u32; 2])> {
    let size = SIZE as i64;
    let (xp, xn) = ((shift_x as i64).max(0), (-(shift_x as i64)).max(0));
    let (yp, yn) = ((shift_y as i64).max(0), (-(shift_y as i64)).max(0));
    let (width, height) = (size - xn - 1 - xp, size - yn - 1 - yp);
    (width > 0 && height > 0).then(|| ([(1 + xp) as u32, (1 + yp) as u32], [(1 + xn) as u32, (1 + yn) as u32], [width as u32, height as u32]))
}

/// The three rectangles of a drop at the pixel (`x`, `y`): each with its corner, its size
/// and its value, cut to the texture. The later ones go over the earlier ones.
pub fn drop_parts(x: i32, y: i32) -> Vec<([u32; 2], [u32; 2], f32)> {
    // ColorFill with 0x6060 and 0x4040: green and blue, as parts of 255.
    const STRONG: f32 = 96.0 / 255.0;
    const WEAK: f32 = 64.0 / 255.0;
    let size = SIZE as i32;
    [(x - 2, x + 2, y - 1, y + 1, STRONG), (x - 1, x + 1, y - 2, y + 2, STRONG), (x - 1, x + 1, y - 1, y + 1, WEAK)]
        .into_iter()
        .filter_map(|(left, right, top, bottom, value)| {
            let (left, right, top, bottom) = (left.max(0), right.min(size), top.max(0), bottom.min(size));
            (left < right && top < bottom).then(|| ([left as u32, top as u32], [(right - left) as u32, (bottom - top) as u32], value))
        })
        .collect()
}

/// The bits of a 16-bit float for a value that is 0 or an ordinary number.
pub fn half_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    if value == 0.0 || exponent <= 0 {
        return sign;
    }
    if exponent >= 31 {
        return sign | 0x7C00;
    }
    sign | ((exponent as u16) << 10) | ((bits >> 13) & 0x3FF) as u16
}

/// The places of the drops. Its numbers repeat from run to run, so that a replay of a
/// capture gives the same picture.
pub struct Drops {
    state: u64,
    remaining: f32,
}

impl Drops {
    pub fn new() -> Drops {
        Drops { state: 0x9E37_79B9_7F4A_7C15, remaining: 0.0 }
    }

    /// The drops of a frame: `precipitation` is 1 for rain and snow, 1.5 for a
    /// thunderstorm, and below 0 for no drops; 150 drops in a second at 1.
    pub fn of_frame(&mut self, precipitation: f32, frame_time: f32) -> Vec<(i32, i32)> {
        let frequency = 150.0 * precipitation;
        if !(frequency > 0.0) {
            return Vec::new();
        }
        self.remaining += frequency * frame_time;
        let count = self.remaining.floor();
        self.remaining -= count;
        (0..count as u32).map(|_| (self.pixel(), self.pixel())).collect()
    }

    fn pixel(&mut self) -> i32 {
        // xorshift64*.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        ((self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as u32 % SIZE) as i32
    }
}

/// What the simulation needs of a frame.
pub struct Inputs {
    /// The time of the simulation of the game, in seconds. It stops in a menu.
    pub time: f32,
    /// The cell is an exterior or an interior that is like one: the rain falls.
    pub like_exterior: bool,
    pub precipitation: f32,
    /// The position of the feet of the player, when it is known.
    pub foot: Option<[f32; 3]>,
    pub water_level: f32,
    pub player_height: f32,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The texture as the source of a pass.
    group: wgpu::BindGroup,
}

pub struct Ripples {
    /// The rain: the texture with the state, and the other one of the pair.
    rain: [Target; 2],
    rain_now: usize,
    /// The player: 0 has the state between the frames, 1 is the buffer.
    player: [Target; 2],
    step_pipeline: wgpu::RenderPipeline,
    player_pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    /// The part of a step that is left for the next frame.
    remaining: f32,
    drops: Drops,
    /// The rain texture has drops in it: it is cleared one time in a cell without rain.
    rain_used: bool,
    last_time: Option<f32>,
    /// The pixel of the world under the player in the frame before.
    last_pixel: Option<(i32, i32)>,
    /// The world position of the corner of the player texture.
    pub origin: [f32; 2],
    pub steps: u64,
    pub drops_made: u64,
    pub frames: u64,
}

impl Ripples {
    pub fn new(scene: &SceneView) -> Ripples {
        let device = scene.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ripples"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(16) },
                    count: None,
                },
            ],
        });
        // Both simulations read their texture with the repeat mode and the linear filter
        // (`sampRain` of the Direct3D 9 path).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ripples"), size: SLOTS * SLOT_BYTES, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let target = |label: &str| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: wgpu::BufferSize::new(16) }) },
                ],
            });
            Target { texture, view, group }
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("ripples"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("ripples"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ripples"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let ripples = Ripples {
            rain: [target("ripples rain"), target("ripples rain")],
            rain_now: 0,
            player: [target("ripples player"), target("ripples player")],
            step_pipeline: pipeline("step_fs"),
            player_pipeline: pipeline("player_fs"),
            uniforms,
            remaining: 0.0,
            drops: Drops::new(),
            rain_used: false,
            last_time: None,
            last_pixel: None,
            origin: [0.0, 0.0],
            steps: 0,
            drops_made: 0,
            frames: 0,
        };
        // A new texture has no defined content for a copy or a read: all four start flat.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ripples start") });
        for target in ripples.rain.iter().chain(ripples.player.iter()) {
            clear(&mut encoder, &target.view);
        }
        scene.queue.submit([encoder.finish()]);
        ripples
    }

    /// The rain texture, for the water shading.
    pub fn rain_view(&self) -> &wgpu::TextureView {
        &self.rain[self.rain_now].view
    }

    /// The player texture, for the water shading.
    pub fn player_view(&self) -> &wgpu::TextureView {
        &self.player[0].view
    }

    /// One frame of the two simulations. Nothing moves while the time of the game stands
    /// (a menu), as on the Direct3D 9 path.
    pub fn simulate(&mut self, scene: &SceneView, inputs: &Inputs) {
        let frame_time = self.last_time.map_or(0.0, |last| inputs.time - last);
        self.last_time = Some(inputs.time);
        if !(frame_time > 0.0) {
            return;
        }
        let frame_time = frame_time.min(MOST_FRAME_TIME);
        self.frames += 1;
        // The steps of the frame, for both simulations.
        let steps = steps_of(&mut self.remaining, frame_time);
        let mut encoder = scene.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ripples") });
        // The first slot is for the step passes: they read no uniform.
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[0.0f32, 0.0, inputs.time, 0.0]));

        // The rain.
        if inputs.like_exterior {
            for (x, y) in self.drops.of_frame(inputs.precipitation, frame_time) {
                for (corner, size, value) in drop_parts(x, y) {
                    let pixel = [0u16, half_bits(value), half_bits(value), 0];
                    let pixels: Vec<u16> = (0..size[0] * size[1]).flat_map(|_| pixel).collect();
                    scene.queue.write_texture(
                        wgpu::TexelCopyTextureInfo { texture: &self.rain[self.rain_now].texture, mip_level: 0, origin: wgpu::Origin3d { x: corner[0], y: corner[1], z: 0 }, aspect: wgpu::TextureAspect::All },
                        bytemuck::cast_slice(&pixels),
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 8), rows_per_image: None },
                        wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    );
                }
                self.rain_used = true;
                self.drops_made += 1;
            }
            // A flat texture stays flat: no steps before the first drop.
            if self.rain_used {
                for _ in 0..steps {
                    self.pass(&mut encoder, &self.step_pipeline, &self.rain[self.rain_now].group, &self.rain[1 - self.rain_now].view, 0);
                    self.rain_now = 1 - self.rain_now;
                }
                self.steps += steps as u64;
            }
        } else if self.rain_used {
            // A cell without the weather: no ripples of the rain.
            clear(&mut encoder, &self.rain[0].view);
            clear(&mut encoder, &self.rain[1].view);
            self.rain_used = false;
        }

        // The player. The texture moves with the player by whole pixels, so that the
        // ripples do not shake.
        if let Some(foot) = inputs.foot {
            let pixel = ((foot[0] / WORLD_PIXEL).floor() as i32, (foot[1] / WORLD_PIXEL).floor() as i32);
            let last = self.last_pixel.unwrap_or(pixel);
            let (shift_x, shift_y) = (pixel.0.wrapping_sub(last.0), pixel.1.wrapping_sub(last.1));
            self.last_pixel = Some(pixel);
            // The part that stays goes into the buffer, at its new place.
            clear(&mut encoder, &self.player[1].view);
            if let Some((from, to, size)) = kept_part(shift_x, shift_y) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: &self.player[0].texture, mip_level: 0, origin: wgpu::Origin3d { x: from[0], y: from[1], z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo { texture: &self.player[1].texture, mip_level: 0, origin: wgpu::Origin3d { x: to[0], y: to[1], z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                );
            }
            // The passes go from the buffer to the state and back.
            let mut source = 1usize;
            let depth = foot[2] - inputs.water_level;
            if depth < 0.0 && depth > -inputs.player_height {
                for i in 0..steps.min(SLOTS as u32 - 1) {
                    // Between the place before and the place now, so that a low frame
                    // rate does not make fewer waves.
                    let w = -(i as f32) / steps as f32 / SIZE as f32;
                    let slot = 1 + i as u64;
                    scene.queue.write_buffer(&self.uniforms, slot * SLOT_BYTES, bytemuck::cast_slice(&[w * shift_x as f32 + 0.5, w * shift_y as f32 + 0.5, inputs.time, 0.0]));
                    self.pass(&mut encoder, &self.player_pipeline, &self.player[source].group, &self.player[1 - source].view, (slot * SLOT_BYTES) as u32);
                    source = 1 - source;
                }
            }
            for _ in 0..steps {
                self.pass(&mut encoder, &self.step_pipeline, &self.player[source].group, &self.player[1 - source].view, 0);
                source = 1 - source;
            }
            self.steps += steps as u64;
            if source != 0 {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: &self.player[1].texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo { texture: &self.player[0].texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                );
            }
            self.origin = [pixel.0 as f32 * WORLD_PIXEL - 0.5 * WORLD_SIZE, pixel.1 as f32 * WORLD_PIXEL - 0.5 * WORLD_SIZE];
        }
        scene.queue.submit([encoder.finish()]);
    }

    fn pass(&self, encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, source: &wgpu::BindGroup, target: &wgpu::TextureView, offset: u32) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ripples"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, source, &[offset]);
        pass.draw(0..3, 0..1);
    }
}

/// Makes a texture flat: all zeros.
fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("ripples clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_follow_the_time_of_the_game() {
        let mut remaining = 0.0;
        // 80 steps in a second, whatever the frames are.
        let steps: u32 = (0..144).map(|_| steps_of(&mut remaining, 1.0 / 144.0)).sum();
        assert!((79..=80).contains(&steps), "{steps}");
        let mut remaining = 0.0;
        assert_eq!(steps_of(&mut remaining, 0.05), 4);
        // A long frame counts as half a second.
        let mut remaining = 0.0;
        assert_eq!(steps_of(&mut remaining, 3.0), 40);
    }

    #[test]
    fn the_part_that_stays_moves_against_the_player() {
        // No move: all but the first row and the first column, at the same place.
        assert_eq!(kept_part(0, 0), Some(([1, 1], [1, 1], [511, 511])));
        // The player went 3 pixels to +x: the content goes 3 pixels to -x.
        assert_eq!(kept_part(3, 0), Some(([4, 1], [1, 1], [508, 511])));
        assert_eq!(kept_part(-3, 2), Some(([1, 3], [4, 1], [508, 509])));
        // A jump of the player: nothing stays.
        assert_eq!(kept_part(600, 0), None);
        assert_eq!(kept_part(0, -511), None);
        assert_eq!(kept_part(i32::MAX, i32::MIN), None);
    }

    #[test]
    fn a_drop_is_cut_to_the_texture() {
        let parts = drop_parts(100, 100);
        assert_eq!(parts.len(), 3);
        assert_eq!((parts[0].0, parts[0].1), ([98, 99], [4, 2]));
        assert_eq!((parts[1].0, parts[1].1), ([99, 98], [2, 4]));
        assert_eq!((parts[2].0, parts[2].1), ([99, 99], [2, 2]));
        assert!(parts[2].2 < parts[0].2);
        // At the corner the parts are cut, and none is outside.
        for (corner, size, _) in drop_parts(0, 511).into_iter().chain(drop_parts(511, 0)) {
            assert!(corner[0] + size[0] <= SIZE && corner[1] + size[1] <= SIZE && size[0] > 0 && size[1] > 0);
        }
    }

    #[test]
    fn the_drops_follow_the_weather_and_repeat() {
        let mut drops = Drops::new();
        assert!(drops.of_frame(-1.5, 1.0).is_empty());
        assert!(drops.of_frame(0.0, 1.0).is_empty());
        let rain: usize = (0..100).map(|_| drops.of_frame(1.0, 0.01).len()).sum();
        assert!((149..=150).contains(&rain), "{rain}");
        let mut thunder = Drops::new();
        let count: usize = (0..100).map(|_| thunder.of_frame(1.5, 0.01).len()).sum();
        assert!((224..=225).contains(&count), "{count}");
        let (mut a, mut b) = (Drops::new(), Drops::new());
        assert_eq!(a.of_frame(1.0, 0.5), b.of_frame(1.0, 0.5));
        assert!(a.of_frame(1.0, 0.5).iter().all(|(x, y)| (0..SIZE as i32).contains(x) && (0..SIZE as i32).contains(y)));
    }

    #[test]
    fn the_bits_of_a_16_bit_float() {
        assert_eq!(half_bits(0.0), 0);
        assert_eq!(half_bits(1.0), 0x3C00);
        assert_eq!(half_bits(0.5), 0x3800);
        assert_eq!(half_bits(-1.0), 0xBC00);
        assert_eq!(half_bits(-2.0), 0xC000);
        // 96 / 255 is between 0.25 and 0.5.
        assert_eq!(half_bits(96.0 / 255.0) >> 10, 13);
    }

    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(SHADER).unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        for entry in ["vs", "step_fs", "player_fs"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
    }
}
