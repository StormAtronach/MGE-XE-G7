//! The view group: group 0 of the pipelines of MGE XE, with one layout for all of them and one
//! group for each view of the frame (the main view, the mirror of the water, the two layers of
//! the shadow map, the game view). It has the values of the view (`View`, the uniform `u` of a
//! shader) and the baked light of the distant lights (`baked_light`, from `LightBinding`). The
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
const VIEWS: usize = 5;

/// The values of a view; the layout of `View` in `wgsl()`.
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
    /// water plane to keep (1 above, -1 below), z the plane (the water level less 1).
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
}

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
}
@group(0) @binding(0) var<uniform> u: View;
";

/// The code of the view group for the start of a shader: `u` and `baked_light`.
pub fn wgsl() -> String {
    format!("{VIEW_WGSL}{}", distant_lights::baked_wgsl_at(0, 1))
}

pub struct ViewBinding {
    pub layout: wgpu::BindGroupLayout,
    buffers: [wgpu::Buffer; VIEWS],
    groups: Vec<wgpu::BindGroup>,
    /// The `LightBinding::generation` of the groups.
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
        // A matrix and twelve vectors of four numbers.
        assert_eq!(size_of::<View>(), 4 * (16 + 12 * 4));
        assert_eq!(VIEW_WGSL.matches(": vec4f,").count(), 12);
    }

    #[test]
    fn the_baked_light_of_the_view_group_starts_at_binding_1() {
        let code = wgsl();
        assert!(code.contains("@group(0) @binding(1) var<uniform> light_map"));
        assert!(code.contains("@group(0) @binding(4) var light_sampler"));
    }
}
