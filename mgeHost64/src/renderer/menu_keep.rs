//! The picture of the world under a menu: `runtime.menu_caching` (`pauseRenderingInMenus` of
//! the API), off by default. As `DistantLand::postProcess` of the Direct3D 9 path has it: at
//! the first frame of a menu the picture after the post shaders is kept, and the later frames
//! of the menu show the kept picture under the interface, with no work of the distant land,
//! the shadows, the water and the post shaders. A mouse click ends the keep: the frame after
//! it is drawn again and kept again.
//!
//! The world draws of the game are still replayed, and the kept picture covers them, as the
//! game still draws its world under a menu on Direct3D 9. Direct3D 9 keeps a picture in a
//! cell with distant land only (its keep is a part of the distant land stages); here it is
//! kept in each cell.

use nirender_wgpu::{SceneView, wgpu};

use super::post::{full_screen_pass, full_screen_pipeline, BLIT_SHADER};

pub struct MenuKeep {
    texture: Option<wgpu::Texture>,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl MenuKeep {
    pub fn new(scene: &SceneView) -> MenuKeep {
        let device = scene.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("menu keep"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline = full_screen_pipeline(device, "menu keep", BLIT_SHADER, &layout, scene.target_format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("menu keep"), ..Default::default() });
        MenuKeep { texture: None, layout, pipeline, sampler }
    }

    /// The texture that keeps the picture, of the size and the format of the target; made
    /// again when the size changes.
    pub fn target(&mut self, scene: &SceneView) -> wgpu::TextureView {
        let (width, height) = scene.target_size;
        let fits = self.texture.as_ref().is_some_and(|texture| texture.width() == width && texture.height() == height);
        if !fits {
            self.texture = Some(scene.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("menu keep"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: scene.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
        self.texture.as_ref().unwrap().create_view(&Default::default())
    }

    /// Draws `source` into `target`, the whole picture.
    pub fn copy(&self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView, target: &wgpu::TextureView) {
        let group = scene.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("menu keep"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        });
        let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Draws the kept picture into `target`. Nothing before a picture is kept.
    pub fn show(&self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        if let Some(texture) = self.texture.as_ref() {
            self.copy(scene, encoder, &texture.create_view(&Default::default()), target);
        }
    }
}
