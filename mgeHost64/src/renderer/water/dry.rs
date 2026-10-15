//! Dry spaces (B5): closed meshes inside which there is no water, the hold of a boat or a room
//! under a pond. A mod reports their triangles (`MGE_WaterMasksSet`, record 7). Whether a
//! point of a water surface is inside one is counted in a bit of the stencil buffer, as on
//! Direct3D 9 (`countDrySpaces` of `renderwater.cpp`): each face of the dry spaces behind the
//! point, along the ray from the eye, turns the bit over, and an odd count is inside. For the
//! plane of the sea the faces behind it are those on its far side, which needs no depth; for a
//! surface of a volume they are those behind the depth that the surface wrote first.
//!
//! The bit is set to zero over the picture before the count and again after the draw: the
//! game leaves the count of its shadow volumes in the stencil buffer, and reads it no more.
//! Meshes of mods that draw through a mask of their own write 0xf4; this bit is not one of
//! theirs.

use nirender_wgpu::{SceneView, wgpu};

use super::shading;

/// The bit of the stencil buffer that says "inside a dry space".
pub(super) const DRY_BIT: u32 = 0x02;

/// The test of a surface that is cut by the dry spaces: the bit is zero.
pub(super) fn outside() -> wgpu::StencilState {
    let face = wgpu::StencilFaceState { compare: wgpu::CompareFunction::Equal, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op: wgpu::StencilOperation::Keep };
    wgpu::StencilState { front: face, back: face, read_mask: DRY_BIT, write_mask: 0 }
}

const DRY: &str = "
@vertex fn vs_zero(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.5, 1.0);
}

@fragment fn fs_none() -> @location(0) vec4f {
    return vec4f(0.0);
}

struct Mask {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
}

// A corner of a dry space, with the clip depth of the plane of the sea (`water.rs`).
@vertex fn vs_mask_plane(@location(0) world: vec3f) -> Mask {
    var out: Mask;
    out.world = world;
    out.position = u.view_projection * vec4f(world, 1.0);
    out.position.z = out.position.w * water.land.z + water.land.w;
    return out;
}

// The same with the depth of the game, for the surfaces of volumes.
@vertex fn vs_mask(@location(0) world: vec3f) -> Mask {
    var out: Mask;
    out.world = world;
    out.position = u.view_projection * vec4f(world, 1.0);
    return out;
}

// Only the faces on the far side of the plane of the sea (the level less 1) from the eye.
@fragment fn fs_mask_plane(in: Mask) -> @location(0) vec4f {
    let level = water.world.z - 1.0;
    if (in.world.z - level) * (u.eye.z - level) > 0.0 {
        discard;
    }
    return vec4f(0.0);
}
";

fn shader() -> String {
    format!("{}{}{}", super::super::view::wgsl(), shading::COMMON, DRY)
}

pub(super) struct Dry {
    zero: wgpu::RenderPipeline,
    count_plane: wgpu::RenderPipeline,
    count_behind: wgpu::RenderPipeline,
    /// The corners of the triangles of the dry spaces in the world, and their number.
    masks: Option<(wgpu::Buffer, u32)>,
}

impl Dry {
    /// `view_layout` is that of the view group (`view.rs`), `frame_layout` that of the group of
    /// the shared shading (`Water::layout`).
    pub fn new(scene: &SceneView, view_layout: &wgpu::BindGroupLayout, frame_layout: &wgpu::BindGroupLayout) -> Dry {
        let device = scene.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water dry spaces"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("water dry spaces"), bind_group_layouts: &[Some(view_layout), Some(frame_layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3];
        let make = |vertex: &str, fragment: &str, depth: wgpu::CompareFunction, operation: wgpu::StencilOperation, with_corners: bool, bias: wgpu::DepthBiasState| {
            let face = wgpu::StencilFaceState { compare: wgpu::CompareFunction::Always, fail_op: operation, depth_fail_op: wgpu::StencilOperation::Keep, pass_op: operation };
            let corners = [Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water dry spaces"),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &corners[..with_corners as usize] },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: scene.depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(depth),
                    stencil: wgpu::StencilState { front: face, back: face, read_mask: 0xFF, write_mask: DRY_BIT },
                    bias,
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: scene.target_format, blend: None, write_mask: wgpu::ColorWrites::empty() })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // The zero: the reference of the pass is 0.
        let zero = make("vs_zero", "fs_none", wgpu::CompareFunction::Always, wgpu::StencilOperation::Replace, false, Default::default());
        let count_plane = make("vs_mask_plane", "fs_mask_plane", wgpu::CompareFunction::Always, wgpu::StencilOperation::Invert, true, Default::default());
        // The faces behind the depth and at it: a point on a face of a dry space is in it (the
        // floor of a hold is often the face of its mask). The bias takes a face a little back,
        // so that the same surface in two meshes, with other matrices, counts.
        let at_face = wgpu::DepthBiasState { constant: 4, slope_scale: 2.0, clamp: 0.0 };
        let count_behind = make("vs_mask", "fs_none", wgpu::CompareFunction::GreaterEqual, wgpu::StencilOperation::Invert, true, at_face);
        Dry { zero, count_plane, count_behind, masks: None }
    }

    /// The triangles of the dry spaces, three corners each, in the world.
    pub fn set_masks(&mut self, scene: &SceneView, corners: &[[f32; 3]]) {
        use wgpu::util::DeviceExt;
        self.masks = (corners.len() >= 3).then(|| {
            let buffer = scene.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("water dry spaces"), contents: bytemuck::cast_slice(corners), usage: wgpu::BufferUsages::VERTEX });
            (buffer, (corners.len() / 3 * 3) as u32)
        });
    }

    pub fn any(&self) -> bool {
        self.masks.is_some()
    }

    /// Sets the bit to zero over the picture. The group of the shared shading is bound.
    pub fn zero(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_pipeline(&self.zero);
        pass.set_stencil_reference(0);
        pass.draw(0..3, 0..1);
    }

    /// Counts the faces behind the plane of the sea into the bit. The group of the shared
    /// shading is bound.
    pub fn count_plane(&self, pass: &mut wgpu::RenderPass<'static>) {
        self.count(pass, &self.count_plane);
    }

    /// Counts the faces behind the depth into the bit, and those at it: that of the surfaces of
    /// volumes, or that of the game for the caustics under the sea.
    pub fn count_behind(&self, pass: &mut wgpu::RenderPass<'static>) {
        self.count(pass, &self.count_behind);
    }

    fn count(&self, pass: &mut wgpu::RenderPass<'static>, pipeline: &wgpu::RenderPipeline) {
        let Some((buffer, count)) = self.masks.as_ref() else { return };
        pass.set_pipeline(pipeline);
        pass.set_stencil_reference(0);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..*count, 0..1);
    }
}

/// The triangles of the dry spaces from the record of the masks (7): the version, the number
/// of triangles, then three corners of three floats for each. `None` for a record that is not
/// one.
pub fn read_masks(bytes: &[u8]) -> Option<Vec<[f32; 3]>> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if word(0)? != 1 {
        return None;
    }
    let triangles = word(4)? as usize;
    let floats = bytes.get(8..8 + triangles * 36)?;
    Some(floats.chunks_exact(12).map(|corner| core::array::from_fn(|i| f32::from_le_bytes(corner[i * 4..i * 4 + 4].try_into().unwrap()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_of_the_masks() {
        let mut record = Vec::new();
        for word in [1u32, 1] {
            record.extend_from_slice(&word.to_le_bytes());
        }
        for value in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.5] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(read_masks(&record), Some(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 2.5]]));
        assert_eq!(read_masks(&record[..record.len() - 4]), None);
        assert_eq!(read_masks(&[1, 0, 0, 0, 0, 0, 0, 0]), Some(Vec::new()));
    }

    #[test]
    fn the_shader_is_valid() {
        use wgpu::naga;
        let source = shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
    }
}
