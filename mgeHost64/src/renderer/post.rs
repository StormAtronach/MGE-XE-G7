//! The post-processing chain of MGE XE: the shaders of `[shaders] chain`, ported by hand to
//! WGSL, run on the picture of the frame after the world views and before the user
//! interface of the game. The port specification is in
//! `moreFPS/docs/plans/mge-xe-post-process-spec.md`.
//!
//! A shader is a WGSL module with one fragment function for each pass; each pass draws a
//! full-screen triangle into one of three `rgba8unorm` textures of the size of the picture.
//! `lastshader` is the input of the shader (the picture, or the output of the shader before)
//! and `lastpass` the output of the pass before. The variables of MGE XE are one uniform.

use nirender_wgpu::{SceneView, wgpu};

/// The variables that MGE XE gives a shader (section 2 of the specification), as one
/// uniform. The matrices are the rows of Direct3D, so that `mview[row][col]` of HLSL is
/// `u.mview[row][col]` here.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Variables {
    pub mview: [[f32; 4]; 4],
    pub mproj: [[f32; 4]; 4],
    /// xyz, and `sunvis` in w.
    pub eyevec: [f32; 4],
    /// xyz, and `fov` (degrees) in w.
    pub eyepos: [f32; 4],
    /// xyz, and `time` in w.
    pub sunvec: [f32; 4],
    /// xyz, and `waterlevel` in w.
    pub suncol: [f32; 4],
    /// xyz, and `isInterior` in w.
    pub sunamb: [f32; 4],
    /// xyz, and `isUnderwater` in w.
    pub sunpos: [f32; 4],
    pub hdr: [f32; 4],
    /// xyz, and `fogstart` in w.
    pub fogcol: [f32; 4],
    /// xyz, and `fogrange` in w.
    pub fognearcol: [f32; 4],
    /// `fognearstart`, `fognearrange`, and `rcpres`.
    pub fog_near: [f32; 4],
}

/// The part of each shader module that every shader has: the variables, the textures, the
/// samplers, the quad, and the helpers of HLSL that WGSL does not have.
const COMMON: &str = "
struct Variables {
    mview: mat4x4f,
    mproj: mat4x4f,
    eyevec: vec4f,
    eyepos: vec4f,
    sunvec: vec4f,
    suncol: vec4f,
    sunamb: vec4f,
    sunpos: vec4f,
    hdr: vec4f,
    fogcol: vec4f,
    fognearcol: vec4f,
    fog_near: vec4f,
}
@group(0) @binding(0) var<uniform> u: Variables;
@group(0) @binding(1) var lastshader: texture_2d<f32>;
@group(0) @binding(2) var lastpass: texture_2d<f32>;
@group(0) @binding(3) var depthframe: texture_2d<f32>;
@group(0) @binding(4) var watertexture: texture_3d<f32>;
@group(0) @binding(5) var point_clamp: sampler;
@group(0) @binding(6) var linear_clamp: sampler;
@group(0) @binding(7) var linear_wrap: sampler;

// The rows of the matrices of Direct3D: mview[row][col] of HLSL.
fn mview(row: u32, col: u32) -> f32 {
    return u.mview[row][col];
}
fn mproj(row: u32, col: u32) -> f32 {
    return u.mproj[row][col];
}
fn rcpres() -> vec2f {
    return u.fog_near.zw;
}
fn saturate(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}
fn saturate3(x: vec3f) -> vec3f {
    return clamp(x, vec3f(0.0), vec3f(1.0));
}

struct Quad {
    @builtin(position) position: vec4f,
    @location(0) tex: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.tex = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}
";

/// A shader of the chain: its name, the names of its pass functions in its module, and the
/// environment flags that disable it (`mgeflags`).
pub struct Shader {
    pub name: &'static str,
    pub source: &'static str,
    pub passes: &'static [&'static str],
    pub disable_flags: u32,
    /// The shader has `disableSunglare`.
    pub no_sun_glare: bool,
}

/// Sunshafts by phal and Hrnchamd, MGE XE 0.11.0, as `Sunshafts.fx` has it: a mask of the
/// sky at a quarter of the size, a radial blur, the rays toward the sun, a tangential blur,
/// and the sum with a sun disc.
const SUNSHAFTS: &str = "
const N: i32 = 20;
const raysunradius: f32 = 0.25;
const raystrength: f32 = 1.7;
const rayfalloff: f32 = 1.10;
const rayfalloffconst: f32 = 0.125;
const raysunfalloff: f32 = 1.5;
const centervis: f32 = 0.3;
const sunrayocclude: f32 = 0.75;
const brightnessadd: f32 = 1.25;
const offscreenrange: f32 = 0.5;
const sundiscradius: f32 = 0.025;
const sundiscbrightness: f32 = 1.9;
const sundiscdesaturate: f32 = 0.5;
const sundiscocclude: f32 = 0.75;
const rscale: f32 = 0.5;
const scale: f32 = 2.0;
const threshold: f32 = 1e7;

// The values that the effect computes from the variables (its static consts).
struct Statics {
    raspect: f32,
    sundir: vec3f,
    forward: f32,
    sunview: vec2f,
    sunviewhalf: vec2f,
    light: f32,
    sharpness: f32,
    strength: f32,
    suncoldisc: vec3f,
    azi_horizon: f32,
}

fn statics() -> Statics {
    var s: Statics;
    let rcp = rcpres();
    s.raspect = rcp.x / rcp.y;
    s.sundir = -normalize(u.sunpos.xyz);
    s.forward = dot(s.sundir, u.eyevec.xyz);
    let fov = u.eyepos.w;
    let texproj = 0.5 * vec2f(1.0, -rcp.y / rcp.x) / tan(radians(fov * 0.5));
    // mul(float3, mview): a row vector by the rotation of the view.
    let v = u.sunpos.xyz / dot(u.eyevec.xyz, u.sunpos.xyz);
    let sunview_v = v.x * u.mview[0].xyz + v.y * u.mview[1].xyz + v.z * u.mview[2].xyz;
    s.sunview = vec2f(0.5) + sunview_v.xy * texproj;
    s.sunviewhalf = 0.5 * s.sunview;
    let sunvis = u.eyevec.w;
    s.light = 1.0 - pow(1.0 - sunvis, 2.0);
    let fogstart = u.fogcol.w;
    s.sharpness = mix(60.0, 450.0 + 100.0 * s.sundir.z, saturate(fogstart / 480.0));
    s.strength = raystrength * s.light * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.x - 0.5)) * smoothstep(-offscreenrange, 0.0, 0.5 - abs(s.sunview.y - 0.5));
    let suncol = u.suncol.xyz;
    s.suncoldisc = vec3f(1.0, 0.76 + 0.24 * u.sunpos.z, 0.54 + 0.46 * u.sunpos.z) * saturate3(suncol / max(suncol.r, max(suncol.g, suncol.b)) * (1.0 - sundiscdesaturate) + vec3f(sundiscdesaturate));
    let fogrange = u.fognearcol.w;
    let waterlevel = u.suncol.w;
    s.azi_horizon = normalize(vec2f(4.0 * fogrange, waterlevel - u.eyepos.z)).y;
    return s;
}

fn depth_at(t: vec2f) -> f32 {
    return textureSampleLevel(depthframe, point_clamp, t, 0.0).r;
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn stretch(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    var depth = 0.0;
    if s.forward < 0.0 {
        let src = scale * in.tex;
        let rcp = rcpres();
        depth = step(threshold, depth_at(src));
        depth += step(threshold, depth_at(src + vec2f(rcp.x, 0.0)));
        depth += step(threshold, depth_at(src + vec2f(0.0, rcp.y)));
        depth += step(threshold, depth_at(src + rcp));
        depth *= 0.25;
    }
    return vec4f(0.0, 0.0, 0.0, depth);
}

@fragment fn blur_r_half(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if any(vec2f(1.1 * rscale) - in.tex < vec2f(0.0)) {
        discard;
    }
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let radial = normalize(in.tex - s.sunviewhalf) * rcpres().yx;
    var alpha = 0.3333 * pass_at(in.tex).a;
    alpha += 0.2222 * pass_at(in.tex + radial).a;
    alpha += 0.2222 * pass_at(in.tex - radial).a;
    alpha += 0.1111 * pass_at(in.tex + 2.0 * radial).a;
    alpha += 0.1111 * pass_at(in.tex - 2.0 * radial).a;
    return vec4f(0.0, 0.0, 0.0, alpha);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    var screendir = in.tex - s.sunview;
    let screendist = length(screendir * vec2f(1.0, s.raspect));
    screendir /= screendist;
    let sunr = min(raysunradius, screendist);
    var l = 0.0;
    for (var i = 1; i <= N; i++) {
        let sundist = f32(i) / f32(N) * sunr;
        let at = clamp(s.sunview + sundist * screendir, vec2f(0.0), vec2f(1.0)) * rscale;
        l += pass_at(at).a * exp(-((screendist - sundist) / (rayfalloffconst + sundist)) * rayfalloff) * pow(1.0 - saturate(sundist / raysunradius), raysunfalloff);
    }
    l *= s.strength / f32(N) * (screendist / raysunradius * (1.0 - centervis) + centervis);
    var col = vec4f(u.suncol.r, 0.8 * u.suncol.g, 0.8 * u.suncol.b, l);
    col = vec4f(col.rgb * (1.0 + brightnessadd * pow(col.a, 3.0)), col.a);
    return col;
}

@fragment fn blur_t(in: Quad) -> @location(0) vec4f {
    let s = statics();
    if s.forward >= 0.0 {
        return vec4f(0.0);
    }
    let tangent = normalize(in.tex - s.sunview).yx * vec2f(rcpres().y, -rcpres().x);
    var col = 0.3333 * pass_at(in.tex);
    col += 0.2222 * pass_at(in.tex + tangent);
    col += 0.2222 * pass_at(in.tex - tangent);
    col += 0.1111 * pass_at(in.tex + 2.0 * tangent);
    col += 0.1111 * pass_at(in.tex - 2.0 * tangent);
    return col;
}

fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0u, 2u), mview(1u, 2u), mview(2u, 2u));
    v += (1.0 / mproj(0u, 0u) * (2.0 * tex.x - 1.0)) * vec3f(mview(0u, 0u), mview(1u, 0u), mview(2u, 0u));
    v += (-1.0 / mproj(1u, 1u) * (2.0 * tex.y - 1.0)) * vec3f(mview(0u, 1u), mview(1u, 1u), mview(2u, 1u));
    return v;
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let s = statics();
    let ray = pass_at(in.tex);
    var col = textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb;
    col *= saturate(1.0 - sunrayocclude * ray.a);
    col = saturate3(col + ray.rgb * ray.a);
    if s.forward < 0.0 {
        var screendir = in.tex - s.sunview;
        screendir.y *= s.raspect;
        var occl = s.light * step(threshold, depth_at(in.tex));
        occl *= saturate(exp2(s.sharpness * (sundiscradius - length(screendir))));
        if occl > 0.004 {
            var scol = s.suncoldisc * sundiscbrightness;
            let azi = normalize(to_world(in.tex)).z;
            occl *= smoothstep(-0.005, 0.010, azi - s.azi_horizon);
            let gb = smoothstep(-0.03, 0.090, azi - s.azi_horizon);
            scol = vec3f(scol.r, scol.g * gb, scol.b * gb);
            col = mix(col, scol, sundiscocclude * occl);
        }
    }
    return vec4f(col, 1.0);
}
";

/// The shaders that this program has a port of.
pub const SHADERS: &[Shader] = &[Shader { name: "Sunshafts", source: SUNSHAFTS, passes: &["stretch", "blur_r_half", "rays", "blur_t", "combine"], disable_flags: 9, no_sun_glare: true }];

/// The linear depth of the game from its depth buffer: `XE Depth.fx` for the native depth,
/// `1e38` where the game drew nothing.
const DEPTH_SHADER: &str = "
@group(0) @binding(0) var<uniform> p: vec4f;
@group(0) @binding(1) var depth: texture_depth_2d;

struct Quad {
    @builtin(position) position: vec4f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    out.position = vec4f(f32(i32(index & 1u) * 4 - 1), f32(i32(index >> 1u) * 4 - 1), 0.0, 1.0);
    return out;
}

@fragment fn fs(in: Quad) -> @location(0) vec4f {
    let d = textureLoad(depth, vec2i(in.position.xy), 0);
    if d >= 1.0 {
        return vec4f(1.0e38, 0.0, 0.0, 1.0);
    }
    return vec4f(p.y / (d - p.x), 0.0, 0.0, 1.0);
}
";

/// The copy of the result into the picture of the frame.
const BLIT_SHADER: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

struct Quad {
    @builtin(position) position: vec4f,
    @location(0) tex: vec2f,
}

@vertex fn vs(@builtin(vertex_index) index: u32) -> Quad {
    var out: Quad;
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    out.position = vec4f(x, y, 0.0, 1.0);
    out.tex = vec2f(0.5 + 0.5 * x, 0.5 - 0.5 * y);
    return out;
}

@fragment fn fs(in: Quad) -> @location(0) vec4f {
    return textureSampleLevel(source, source_sampler, in.tex, 0.0);
}
";

struct Compiled {
    index: usize,
    pipelines: Vec<wgpu::RenderPipeline>,
}

pub struct Post {
    /// The shaders of the chain that have a port, in the order of the chain.
    chain: Vec<Compiled>,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    samplers: [wgpu::Sampler; 3],
    /// The three textures of the size of the picture, and their size.
    textures: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    size: (u32, u32),
    /// The linear depth of the game.
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    depth_pipeline: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    depth_uniform: wgpu::Buffer,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    waves: wgpu::TextureView,
    pub no_sun_glare: bool,
    pub frames: u64,
    pub shaders_run: u64,
}

impl Post {
    /// Makes the chain from the names of the settings; a name without a port is reported
    /// and skipped. `waves` is the 3D wave texture of the water.
    pub fn new(scene: &SceneView, chain: &[String], waves: wgpu::TextureView) -> Post {
        let device = scene.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
                texture(3, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2),
                texture(4, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D3),
                sampler(5, wgpu::SamplerBindingType::NonFiltering),
                sampler(6, wgpu::SamplerBindingType::Filtering),
                sampler(7, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let mut compiled = Vec::new();
        let mut no_sun_glare = false;
        for name in chain {
            let Some((index, shader)) = SHADERS.iter().enumerate().find(|(_, shader)| shader.name == name) else {
                println!("post: no port of the shader {name}; skipped");
                continue;
            };
            let source = format!("{COMMON}\n{}", shader.source);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(shader.name), source: wgpu::ShaderSource::Wgsl(source.into()) });
            let pipelines = shader
                .passes
                .iter()
                .map(|pass| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(shader.name),
                        layout: Some(&pipeline_layout),
                        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some(pass),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
                })
                .collect();
            no_sun_glare |= shader.no_sun_glare;
            compiled.push(Compiled { index, pipelines });
        }
        println!("post: {} of {} shaders of the chain have a port", compiled.len(), chain.len());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: size_of::<Variables>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let make_sampler = |filter: wgpu::FilterMode, address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: address, address_mode_v: address, address_mode_w: address, mag_filter: filter, min_filter: filter, ..Default::default() })
        };
        let samplers = [
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::Repeat),
        ];
        // The linear depth.
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post depth"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
            ],
        });
        let depth_pipeline = full_screen_pipeline(device, "post depth", DEPTH_SHADER, &depth_layout, wgpu::TextureFormat::R32Float);
        let depth_uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post depth"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post blit"),
            entries: &[texture(0, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2), sampler(1, wgpu::SamplerBindingType::Filtering)],
        });
        let blit_pipeline = full_screen_pipeline(device, "post blit", BLIT_SHADER, &blit_layout, scene.target_format);
        Post {
            chain: compiled,
            layout,
            uniforms,
            samplers,
            textures: Vec::new(),
            views: Vec::new(),
            size: (0, 0),
            depth: None,
            depth_pipeline,
            depth_layout,
            depth_uniform,
            blit_pipeline,
            blit_layout,
            waves,
            no_sun_glare,
            frames: 0,
            shaders_run: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chain.is_empty()
    }

    /// The textures of the size of the picture.
    fn size_textures(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.size == size && !self.textures.is_empty() {
            return;
        }
        self.size = size;
        let make = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("post"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        self.textures = (0..3).map(|_| make(wgpu::TextureFormat::Rgba8Unorm, usage)).collect();
        self.views = self.textures.iter().map(|t| t.create_view(&Default::default())).collect();
        let depth = make(wgpu::TextureFormat::R32Float, usage);
        let view = depth.create_view(&Default::default());
        self.depth = Some((depth, view));
    }

    /// Runs the chain on the picture and writes the result into `target`.
    pub fn run(&mut self, scene: &SceneView, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, variables: &Variables, environment_flags: u32) {
        let (Some(picture), Some(depth)) = (scene.picture, scene.depth) else { return };
        if self.chain.is_empty() {
            return;
        }
        let device = scene.device;
        self.size_textures(device, scene.target_size);
        self.frames += 1;
        scene.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(variables));
        // The linear depth of the game.
        scene.queue.write_buffer(&self.depth_uniform, 0, bytemuck::cast_slice(&[variables.mproj[2][2], variables.mproj[3][2], 0.0, 0.0]));
        let (_, depth_view) = self.depth.as_ref().unwrap();
        {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post depth"),
                layout: &self.depth_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.depth_uniform.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) }],
            });
            let mut pass = full_screen_pass(encoder, depth_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        // The chain. `last` is the texture that holds the input of the shader: `None` is the
        // picture. A pass writes into a texture that is not `last` and not the pass before.
        let mut last: Option<usize> = None;
        let mut previous: Option<usize> = None;
        for compiled in &self.chain {
            let shader = &SHADERS[compiled.index];
            if shader.disable_flags & environment_flags != 0 {
                continue;
            }
            self.shaders_run += 1;
            let lastshader_view = match last {
                Some(index) => &self.views[index],
                None => picture,
            };
            for pipeline in &compiled.pipelines {
                let sink = (0..3).find(|i| Some(*i) != last && Some(*i) != previous).unwrap();
                let lastpass_view = match previous {
                    Some(index) => &self.views[index],
                    None => lastshader_view,
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(shader.name),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(lastshader_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(lastpass_view) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(depth_view) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.waves) },
                        wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
                        wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.samplers[1]) },
                        wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.samplers[2]) },
                    ],
                });
                // A pass keeps what the texture held where it discards a pixel.
                let mut pass = full_screen_pass(encoder, &self.views[sink], wgpu::LoadOp::Load);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
                drop(pass);
                previous = Some(sink);
            }
            // The output of the last pass is the input of the next shader.
            last = previous;
        }
        // The result into the picture of the frame.
        if let Some(index) = last {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post blit"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.views[index]) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) }],
            });
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn texture(binding: u32, sample_type: wgpu::TextureSampleType, dimension: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: dimension, multisampled: false }, count: None }
}

fn sampler(binding: u32, kind: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(kind), count: None }
}

fn full_screen_pipeline(device: &wgpu::Device, label: &str, source: &str, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(layout)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        multiview_mask: None,
        cache: None,
    })
}

fn full_screen_pass<'a>(encoder: &'a mut wgpu::CommandEncoder, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("post"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
