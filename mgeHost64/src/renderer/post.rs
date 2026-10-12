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
@group(0) @binding(8) var linear_mirror: sampler;
@group(0) @binding(9) var point_wrap: sampler;
// The texture of the shader (its `src` annotation), white without one.
@group(0) @binding(10) var tex1: texture_2d<f32>;

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
    /// The priority of the order: the category (scene 1M, atmosphere 2M, lens 3M, sensor
    /// 4M, tone 5M, none 6M, final 9M) plus `priorityAdjust`.
    pub priority: i64,
    /// The texture of the shader under `Data Files/textures`, from its `src` annotation.
    pub texture: Option<&'static str>,
    /// The shader has `requiresHDR`: it reads the adaptation point `u.hdr`.
    pub requires_hdr: bool,
}

/// A shader of the chain as this program compiles it: from a WGSL file of a mod, or from
/// the table of the ports.
struct Definition {
    name: String,
    source: String,
    passes: Vec<String>,
    disable_flags: u32,
    no_sun_glare: bool,
    priority: i64,
    texture: Option<String>,
    requires_hdr: bool,
}

impl Definition {
    fn of_port(shader: &Shader) -> Definition {
        Definition {
            name: shader.name.to_string(),
            source: shader.source.to_string(),
            passes: shader.passes.iter().map(|pass| pass.to_string()).collect(),
            disable_flags: shader.disable_flags,
            no_sun_glare: shader.no_sun_glare,
            priority: shader.priority,
            texture: shader.texture.map(str::to_string),
            requires_hdr: shader.requires_hdr,
        }
    }

    /// A shader file of a mod: `Data Files/shaders/XEshaders/<name>.wgsl`. The file starts
    /// with lines `//! key: value`, which have what the technique of an effect file of
    /// MGE XE has in its annotations; the rest is WGSL, compiled after the common part
    /// (`COMMON`: the variables `u`, the textures, the samplers, the vertex function `vs`
    /// and the struct `Quad`). The keys:
    ///
    /// - `interface: mge-xe-wgsl 1` (needed)
    /// - `passes: a, b` (needed): the fragment functions, one for each pass, in their order
    /// - `category: scene | atmosphere | lens | sensor | tone | none | final` (`none`)
    /// - `priority_adjust: N` (0)
    /// - `mgeflags: N` (0): the shader does not run when a bit is set in the frame: 1
    ///   interior, 2 exterior, 4 interior that behaves as an exterior, 8 under the water,
    ///   16 above the water, 32 the sun is visible, 64 the sun is not visible
    /// - `requires_hdr: true | false` (false)
    /// - `disable_sunglare: true | false` (false)
    /// - `texture: path under Data Files/textures` (none): bound as `tex1`
    fn of_file(name: &str, text: &str) -> Result<Definition, String> {
        let mut definition = Definition { name: name.to_string(), source: text.to_string(), passes: Vec::new(), disable_flags: 0, no_sun_glare: false, priority: 0, texture: None, requires_hdr: false };
        let (mut category, mut adjust, mut interface) = (6_000_000i64, 0i64, false);
        let flag = |value: &str| match value {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("`{other}` is not true or false")),
        };
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("//!") else { break };
            let Some((key, value)) = rest.split_once(':') else { return Err(format!("the line `{line}` has no `key: value`")) };
            let value = value.trim();
            match key.trim() {
                "interface" => interface = value == "mge-xe-wgsl 1",
                "passes" => definition.passes = value.split(',').map(|pass| pass.trim().to_string()).filter(|pass| !pass.is_empty()).collect(),
                "category" => {
                    category = match value {
                        "scene" => 1_000_000,
                        "atmosphere" => 2_000_000,
                        "lens" => 3_000_000,
                        "sensor" => 4_000_000,
                        "tone" => 5_000_000,
                        "none" => 6_000_000,
                        "final" => 9_000_000,
                        other => return Err(format!("the category `{other}` is not known")),
                    }
                }
                "priority_adjust" => adjust = value.parse().map_err(|_| format!("the priority adjust `{value}` is not a number"))?,
                "mgeflags" => definition.disable_flags = value.parse().map_err(|_| format!("the flags `{value}` are not a number"))?,
                "requires_hdr" => definition.requires_hdr = flag(value)?,
                "disable_sunglare" => definition.no_sun_glare = flag(value)?,
                "texture" => definition.texture = Some(value.to_string()),
                other => return Err(format!("the key `{other}` is not known")),
            }
        }
        if !interface {
            return Err("no line `//! interface: mge-xe-wgsl 1` at its start".to_string());
        }
        if definition.passes.is_empty() {
            return Err("no line `//! passes: ...` with the fragment functions".to_string());
        }
        definition.priority = category + adjust;
        Ok(definition)
    }
}

/// The HDR measure of MGE XE (`PostShaders::evalAdaptHDR`): the picture before the chain
/// goes into 512 x 512 pixels and then by halves to 1 pixel, which is read back some frames
/// later; its luminance moves the adaptation point of each frame, the `HDR` of a shader.
struct Hdr {
    texture: wgpu::Texture,
    /// The levels of the texture, 512 x 512 to 1 x 1.
    levels: Vec<wgpu::TextureView>,
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    state: HdrState,
    /// The last luminance read, 0.5 before the first one.
    luminance: f32,
    point: [f32; 4],
    last: Option<std::time::Instant>,
    /// `render.hdr_reaction_time`, seconds.
    reaction: f32,
}

/// The copy of the pixel into the buffer is in the commands of a frame; the map of the
/// buffer is asked for in the frame after, when those commands are with the GPU.
enum HdrState {
    Idle,
    Copied,
    Mapping(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

impl Hdr {
    fn new(device: &wgpu::Device, blit_layout: &wgpu::BindGroupLayout, reaction: f32) -> Hdr {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post hdr"),
            size: wgpu::Extent3d { width: 512, height: 512, depth_or_array_layers: 1 },
            mip_level_count: 10,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let levels = (0..10).map(|level| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() })).collect();
        let pipeline = full_screen_pipeline(device, "post hdr", BLIT_SHADER, blit_layout, wgpu::TextureFormat::Rgba8Unorm);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post hdr"), size: 4, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Hdr { texture, levels, pipeline, buffer, state: HdrState::Idle, luminance: 0.5, point: [1.0; 4], last: None, reaction: reaction.clamp(0.01, 30.0) }
    }

    /// One frame of the measure. Returns the adaptation point.
    fn frame(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, picture: &wgpu::TextureView, blit_layout: &wgpu::BindGroupLayout, linear: &wgpu::Sampler, environment_flags: u32) -> [f32; 4] {
        use std::sync::atomic::{AtomicBool, Ordering};
        match &self.state {
            HdrState::Idle => {
                // A linear copy into each level is the linear `StretchRect` of Direct3D 9:
                // the mean of 2 x 2 pixels from the second level on.
                let mut source = picture;
                for level in &self.levels {
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("post hdr"),
                        layout: blit_layout,
                        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(linear) }],
                    });
                    let mut pass = full_screen_pass(encoder, level, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &group, &[]);
                    pass.draw(0..3, 0..1);
                    drop(pass);
                    source = level;
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 9, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyBufferInfo { buffer: &self.buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: None, rows_per_image: None } },
                    wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                );
                self.state = HdrState::Copied;
            }
            HdrState::Copied => {
                let ready = std::sync::Arc::new(AtomicBool::new(false));
                let flag = ready.clone();
                self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_ok() {
                        flag.store(true, Ordering::Relaxed);
                    }
                });
                self.state = HdrState::Mapping(ready);
            }
            HdrState::Mapping(ready) => {
                if ready.load(Ordering::Relaxed) {
                    if let Ok(pixel) = self.buffer.slice(..).get_mapped_range() {
                        let (r, g, b) = (pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[2] as f32 / 255.0);
                        self.luminance = 0.27 * r + 0.67 * g + 0.06 * b;
                    }
                    self.buffer.unmap();
                    self.state = HdrState::Idle;
                }
            }
        }
        // Each frame: the point follows the luminance with the reaction time. The scale is
        // for an exterior and an interior that behaves as one (the flags 2 and 4).
        let now = std::time::Instant::now();
        let dt = self.last.map_or(0.0, |last| (now - last).as_secs_f32());
        self.last = Some(now);
        let scale = if environment_flags & 6 != 0 { 2.5 } else { 1.0 };
        let lambda = (-dt / self.reaction).exp();
        let p = &mut self.point;
        p[3] = self.luminance;
        p[2] = p[3] * scale + (p[2] - p[3] * scale) * lambda;
        p[1] = p[3] + (p[1] - p[3]) * lambda;
        p[0] = p[2] / scale;
        *p
    }
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

/// Bloom, fine, by Hrnchamd, v2, as `Bloom Fine.fx` has it: the energy over a threshold,
/// a vertical and a horizontal blur, the sum in a linear space.
const BLOOM_FINE: &str = "
const bloomThreshold: f32 = 0.48;
const bloomLevel: f32 = 0.23;
const bloomColourSens: f32 = 0.39;
const bloomGamma: f32 = 2.45;

fn shader_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastshader, point_clamp, t, 0.0);
}
fn pass_at(t: vec2f) -> vec4f {
    return textureSampleLevel(lastpass, linear_clamp, t, 0.0);
}

@fragment fn energy_levels(in: Quad) -> @location(0) vec4f {
    var radiance = shader_at(in.tex);
    let depth = textureSampleLevel(depthframe, point_clamp, in.tex, 0.0).r;
    let lum = dot(vec3f(0.27, 0.54, 0.19), radiance.rgb);
    // Mix with luminance to avoid oversaturated single channels.
    radiance = mix(vec4f(lum), radiance, bloomColourSens);
    // Cut sky/distant illumination down.
    let fogstart = u.fogcol.w;
    let fogrange = u.fognearcol.w;
    radiance *= 1.0 - 0.45 * saturate(2.0 * (depth - fogstart) / (fogrange - fogstart));
    return clamp(radiance - bloomThreshold, vec4f(0.0), vec4f(1.0));
}

@fragment fn blur_vert(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(0.0, rcpres().y * taps[i]));
    }
    return radiance;
}

@fragment fn blur_horz_combine(in: Quad) -> @location(0) vec4f {
    var kernel = array<f32, 7>(0.05, 0.14, 0.31, 0.22, 0.31, 0.14, 0.05);
    var taps = array<f32, 7>(-6.5, -4.5, -2.5, 0.0, 2.5, 4.5, 6.5);
    var base = shader_at(in.tex);
    var radiance = vec4f(0.0);
    for (var i = 0; i < 7; i++) {
        radiance += kernel[i] * pass_at(in.tex + vec2f(rcpres().x * taps[i], 0.0));
    }
    // Mix in linear space.
    base = pow(max(base, vec4f(0.0)), vec4f(bloomGamma));
    return pow(max(base + bloomLevel * radiance, vec4f(0.0)), vec4f(1.0 / bloomGamma));
}
";

/// Screen space ambient occlusion, the fast version with 8 taps, from ssao v09 by Knu, as
/// `SSAO Fast.fx` has it with BLUR and ROTATE: the occlusion, two smart blurs, the sum.
const SSAO_FAST: &str = "
const N: i32 = 8;
const R: f32 = 15.0;
const multiplier: f32 = 1.8;
const occlusion_falloff: f32 = 20.0;
const blur_falloff: f32 = 0.1;
const blur_radius: f32 = 5.0;
const depth_scale: f32 = 10000.0;
const eps: f32 = 1e-6;
const sky: f32 = 1e6;
const M: i32 = 8;

fn invproj() -> vec2f {
    let fov = u.eyepos.w;
    return 2.0 * tan(0.5 * radians(fov)) * vec2f(1.0, rcpres().x / rcpres().y);
}

fn to_view(tex: vec2f) -> vec3f {
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;
    let xy = depth * (tex - 0.5) * invproj();
    return vec3f(xy, depth);
}

fn from_view(view: vec3f) -> vec2f {
    return (view.xy / view.z) / invproj() + vec2f(0.5, 0.5);
}

fn pack2(f: f32) -> vec2f {
    return vec2f(f, fract(f * 255.0 - 0.5));
}

fn unpack2(f: vec2f) -> f32 {
    return f.x + ((f.y - 0.5) / 255.0);
}

@fragment fn ssao(in: Quad) -> @location(0) vec4f {
    var dirs = array<vec3f, 8>(
        vec3f(0.00762, -0.01247, 0.03311), vec3f(-0.61057, 0.20510, 0.58876),
        vec3f(0.55319, 0.67960, -0.19194), vec3f(-0.43533, 0.62404, 0.45133),
        vec3f(-0.02386, -0.03104, 0.01502), vec3f(-0.20990, 0.10082, 0.03849),
        vec3f(0.06331, -0.17620, -0.31359), vec3f(-0.12261, 0.00720, -0.12465));
    let tex = in.tex;
    let xylength = sqrt(1.0 - u.eyevec.z * u.eyevec.z);
    let pos = to_view(tex);
    let waterlevel = u.suncol.w;
    let water = pos.z * u.eyevec.z - pos.y * xylength + u.eyepos.z;
    if pos.z <= 0.0 || pos.z > sky || (water - waterlevel) < 0.0 {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    let rcp = rcpres();
    let left = pos - to_view(tex + rcp * vec2f(-1.0, 0.0));
    let right = to_view(tex + rcp * vec2f(1.0, 0.0)) - pos;
    let up = pos - to_view(tex + rcp * vec2f(0.0, -1.0));
    let down = to_view(tex + rcp * vec2f(0.0, 1.0)) - pos;
    var dx = select(right, left, length(left) < length(right));
    var dy = select(down, up, length(up) < length(down));
    let normal = normalize(cross(dy, dx));
    dy = normalize(cross(dx, normal));
    dx = normalize(dx);
    let rnd = textureSampleLevel(tex1, point_wrap, tex / rcp / 8.0, 0.0).xyz * 2.0 - 1.0;
    var ao = 0.0;
    var amount = 0.0;
    for (var j = 0; j < N; j++) {
        var ray = reflect(dirs[j] * R, rnd);
        ray *= sign(ray.z);
        ray = dx * ray.x + dy * ray.y + normal * ray.z;
        let weight = dot(normalize(ray), normal);
        let occ = to_view(from_view(pos + ray));
        // Bias occ.z to avoid self-occlusion errors from point sampling depth.
        let diff = (pos.z + ray.z) - 1.00025 * occ.z;
        amount += weight;
        ao += weight * step(0.0, diff) * exp2(-diff / occlusion_falloff);
    }
    return vec4f(ao / amount, pack2(pos.z / depth_scale), 1.0);
}

@fragment fn smartblur(in: Quad) -> @location(0) vec4f {
    var taps = array<vec2f, 8>(
        vec2f(-0.695914, 0.457137), vec2f(-0.203345, 0.620716),
        vec2f(0.96234, -0.194983), vec2f(0.473434, -0.480026),
        vec2f(0.507431, 0.064425), vec2f(0.89642, 0.412458),
        vec2f(-0.32194, -0.932615), vec2f(-0.791559, -0.59771));
    let data = textureSampleLevel(lastpass, linear_mirror, in.tex, 0.0);
    var total = data.r;
    let depth = unpack2(data.gb);
    let rev = blur_radius * (2.0 * data.a - 1.0);
    var amount = 1.0;
    for (var i = 0; i < M; i++) {
        let s_tex = in.tex + rcpres() * taps[i] * rev;
        let s_data = textureSampleLevel(lastpass, linear_mirror, s_tex, 0.0).xyz;
        let s_depth = unpack2(s_data.gb);
        let weight = exp2(-abs(depth - s_depth) / depth / blur_falloff);
        amount += weight;
        total += s_data.r * weight;
    }
    return vec4f(total / amount, data.gb, 1.0 - data.a);
}

@fragment fn combine(in: Quad) -> @location(0) vec4f {
    let dist = length(to_view(in.tex));
    let fognearstart = u.fog_near.x;
    let fognearrange = u.fog_near.y;
    let fog = saturate((fognearrange - dist) / (fognearrange - fognearstart));
    let final_ao = fog * multiplier * textureSampleLevel(lastpass, point_clamp, in.tex, 0.0).r;
    let result = mix(textureSampleLevel(lastshader, point_clamp, in.tex, 0.0).rgb, u.fognearcol.xyz * (1.0 - fog), final_ao);
    return vec4f(result, 1.0);
}
";

/// The part that the two underwater shaders share: the ray of a pixel in the world for a
/// depth of 1 (`toWorld`), the samplers as the effect declares them (linear or point, with
/// the wrap addressing that Direct3D 9 has without a declaration), and the wobble of the
/// picture from the wave texture. `pow(x, 32)` of the effect is repeated squaring: its base
/// is negative on a half of the screen.
macro_rules! underwater_common {
    () => {
        "
fn to_world(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}
fn water_at(t: vec3f) -> vec4f {
    return textureSampleLevel(watertexture, linear_wrap, t, 0.0);
}
fn wobbled(tex: vec2f) -> vec2f {
    let time = u.sunvec.w;
    var wobble = 0.01 * (2.0 * water_at(vec3f(0.25 * tex, 0.2 * time)).rg - vec2f(1.0));
    let x = 2.0 * tex - vec2f(1.0);
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    wobble *= vec2f(1.0) - x16 * x16;
    return tex + wobble;
}
"
    };
}

/// Underwater Interior Effects by Hrnchamd, v1, as `Underwater Interior Effects.fx` has it:
/// the wobble of the picture, and caustics near the surface of the water.
const UNDERWATER_INTERIOR: &str = concat!(underwater_common!(), "
@fragment fn wobble(in: Quad) -> @location(0) vec4f {
    let time = u.sunvec.w;
    let waterlevel = u.suncol.w;
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    let d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    let v = u.eyepos.xyz + d * to_world(tex);
    let k = 1.0 - water_at(vec3f(v.xy / 1783.0, 0.4 * time)).b;
    return c * (1.0 - 0.2 * step(v.z, waterlevel - 1.0) * saturate(exp((v.z - waterlevel) / 100.0)) * k);
}
");

/// Underwater Effects by Hrnchamd, v2, as `Underwater Effects.fx` has it: the wobble of the
/// picture, and the rays of the light from three shells of the wave texture. The effect
/// takes the water of an exterior at the height 0.
const UNDERWATER: &str = concat!(underwater_common!(), "
const lightcolour: vec4f = vec4f(0.6, 0.91, 1.0, 0.0);

fn fetch(tex: vec3f, uw: f32) -> f32 {
    let time = u.sunvec.w;
    let c = water_at(vec3f(tex.xy / 3308.0, 0.22 * time)).b;
    return pow(c, 0.25) * saturate(exp(tex.z / 90.0)) * saturate(1.0 + tex.z / uw);
}

@fragment fn rays(in: Quad) -> @location(0) vec4f {
    let sunvis = u.eyevec.w;
    let suncol = u.suncol.xyz;
    let sunpos = u.sunpos.xyz;
    let eyepos = u.eyepos.xyz;
    let lf = (0.2 + 0.8 * sunvis) * (0.32 * suncol.r + 0.47 * suncol.g + 0.21 * suncol.b);
    let tex = wobbled(in.tex);
    let c = textureSampleLevel(lastshader, linear_wrap, tex, 0.0);
    var d = textureSampleLevel(depthframe, point_wrap, tex, 0.0).r;
    var v = to_world(tex);
    let e = vec3f(0.1, 0.1, 1.0) * eyepos;

    d = min(d, -eyepos.z / max(1e-5, v.z));
    v = v / length(v.xy);
    let r = -sunpos.xy - v.xy * dot(-sunpos.xy, v.xy);

    var shellpos = e + 800.0 * v;
    shellpos = vec3f(shellpos.xy + 0.8 * r * shellpos.z, shellpos.z);
    var rayz = fetch(shellpos, min(eyepos.z, 400.0)) * saturate(d / 800.0) * 0.294;

    var shellpos2 = e + 1400.0 * v;
    shellpos2 = vec3f(shellpos2.xy + 1.0 * r * shellpos2.z, shellpos2.z);
    rayz += fetch(shellpos2, 2.0 * eyepos.z) * saturate(d / 800.0 - 1.0) * 0.132;

    var shellpos3 = e + 2000.0 * v;
    shellpos3 = vec3f(shellpos3.xy + 1.2 * r * shellpos3.z, shellpos3.z);
    rayz += fetch(shellpos3, 3.0 * eyepos.z) * saturate(d / 800.0 - 2.0) * 0.065;

    return c + rayz * lf * lightcolour;
}
");

/// Eye Adaptation (HDR) by Hrnchamd, v4, as `Eye Adaptation (HDR).fx` has it: the exposure
/// from the adaptation point, a film response curve, and less colour in low light.
const EYE_ADAPTATION: &str = "
const neutral_point: f32 = 0.335;
const sensitivity: f32 = 0.11;
const max_exposure: f32 = 3.2;

@fragment fn tone(in: Quad) -> @location(0) vec4f {
    let exposure = clamp(mix(1.0, neutral_point / u.hdr.x, sensitivity), 0.5, max_exposure) * sqrt(u.hdr.y / u.hdr.x);
    let scotopic = 0.5 * saturate(exposure / max_exposure - 0.66666);
    let c = textureSampleLevel(lastshader, point_wrap, in.tex, 0.0).rgb;
    // More dynamic range in low light.
    var g = exposure * c + (1.0 - exposure) * (c * c * c * c);
    // The toe and the knee of a film response.
    g = (((0.727408 * g + vec3f(-2.05477)) * g + vec3f(1.63747)) * g + vec3f(0.712032)) * g;
    // Less colour when the exposure is high.
    g = mix(g, vec3f(dot(g, vec3f(0.33333))), scotopic);
    return vec4f(saturate3(g), 1.0);
}
";

/// The shaders that this program has a port of.
pub const SHADERS: &[Shader] = &[
    Shader { name: "Sunshafts", source: SUNSHAFTS, passes: &["stretch", "blur_r_half", "rays", "blur_t", "combine"], disable_flags: 9, no_sun_glare: true, priority: 2_000_000, texture: None, requires_hdr: false },
    Shader { name: "Bloom Fine", source: BLOOM_FINE, passes: &["energy_levels", "blur_vert", "blur_horz_combine"], disable_flags: 0, no_sun_glare: false, priority: 4_000_000, texture: None, requires_hdr: false },
    // `mgeflags` 22: not in a true exterior, not in an interior that behaves as an exterior,
    // not above the water. 17: not in an interior, not above the water.
    Shader { name: "Underwater Interior Effects", source: UNDERWATER_INTERIOR, passes: &["wobble"], disable_flags: 22, no_sun_glare: false, priority: 1_000_000, texture: None, requires_hdr: false },
    Shader { name: "Underwater Effects", source: UNDERWATER, passes: &["rays"], disable_flags: 17, no_sun_glare: false, priority: 1_000_000, texture: None, requires_hdr: false },
    Shader { name: "Eye Adaptation (HDR)", source: EYE_ADAPTATION, passes: &["tone"], disable_flags: 0, no_sun_glare: false, priority: 5_000_000, texture: None, requires_hdr: true },
    Shader { name: "SSAO Fast", source: SSAO_FAST, passes: &["ssao", "smartblur", "smartblur", "combine"], disable_flags: 8, no_sun_glare: false, priority: 1_000_000 - 10_000, texture: Some("MGE/poisson_nrm.dds"), requires_hdr: false },
];

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

/// A look at a term, for a test: MGE_XE_POST_DEBUG 1 tints the result, 2 shows the linear
/// depth over 10000 units as grey.
const DEBUG_BLIT_SHADER: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> mode: vec4f;

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
    let c = textureSampleLevel(source, source_sampler, in.tex, 0.0);
    if mode.x == 2.0 {
        return vec4f(vec3f(clamp(c.r / 10000.0, 0.0, 1.0)), 1.0);
    }
    return vec4f(c.r, 0.5 * c.g, 0.5 * c.b, 1.0);
}
";

struct Compiled {
    name: String,
    disable_flags: u32,
    priority: i64,
    pipelines: Vec<wgpu::RenderPipeline>,
    texture: Option<wgpu::TextureView>,
}

pub struct Post {
    /// The shaders of the chain that have a port, in the order of the chain.
    chain: Vec<Compiled>,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    samplers: [wgpu::Sampler; 5],
    white: wgpu::TextureView,
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
    debug: f32,
    debug_pipeline: wgpu::RenderPipeline,
    debug_layout: wgpu::BindGroupLayout,
    debug_uniform: wgpu::Buffer,
    waves: wgpu::TextureView,
    pub no_sun_glare: bool,
    /// The HDR measure, when a shader of the chain needs it.
    hdr: Option<Hdr>,
    pub frames: u64,
    pub shaders_run: u64,
}

impl Post {
    /// Makes the chain from the names of the settings. A name is a WGSL file of a mod
    /// (`shaders/XEshaders/<name>.wgsl`, see `Definition::of_file`) or, without such a file,
    /// a port of this program; a name with neither, and a file that does not compile, is
    /// reported and skipped. `waves` is the 3D wave texture of the water; `read` gives the
    /// bytes of a file under `Data Files`; `hdr_reaction` is `render.hdr_reaction_time`.
    pub fn new(scene: &SceneView, chain: &[String], waves: wgpu::TextureView, read: &dyn Fn(&str) -> Option<Vec<u8>>, hdr_reaction: f32) -> Post {
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
                sampler(8, wgpu::SamplerBindingType::Filtering),
                sampler(9, wgpu::SamplerBindingType::NonFiltering),
                texture(10, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let mut compiled = Vec::new();
        let mut no_sun_glare = false;
        let mut requires_hdr = false;
        let (mut ports, mut files) = (0, 0);
        for name in chain {
            let file = read(&format!("shaders/XEshaders/{name}.wgsl"));
            let shader = match &file {
                Some(bytes) => match Definition::of_file(name, &String::from_utf8_lossy(bytes)) {
                    Ok(definition) => definition,
                    Err(problem) => {
                        println!("post: the shader file {name}.wgsl is not used: {problem}");
                        continue;
                    }
                },
                None => match SHADERS.iter().find(|shader| shader.name == name) {
                    Some(port) => Definition::of_port(port),
                    None => {
                        println!("post: no file {name}.wgsl and no port of the shader {name}; skipped");
                        continue;
                    }
                },
            };
            let source = format!("{COMMON}\n{}", shader.source);
            // An error of the WGSL of a file, or a pass that is not a function of it, comes
            // back from the scope and does not stop the program.
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&shader.name), source: wgpu::ShaderSource::Wgsl(source.into()) });
            let pipelines: Vec<wgpu::RenderPipeline> = shader
                .passes
                .iter()
                .map(|pass| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&shader.name),
                        layout: Some(&pipeline_layout),
                        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some(pass.as_str()),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
                })
                .collect();
            if let Some(error) = pollster::block_on(scope.pop()) {
                // The lines of the message count the common part first.
                println!("post: the shader {} does not compile; skipped (its first line is the line {} of the message):\n{error}", shader.name, COMMON.lines().count() + 1);
                continue;
            }
            if file.is_some() {
                files += 1;
            } else {
                ports += 1;
            }
            no_sun_glare |= shader.no_sun_glare;
            requires_hdr |= shader.requires_hdr;
            // The texture of the shader.
            let texture = shader.texture.as_deref().and_then(|name| {
                let path = format!("textures/{name}");
                let bytes = read(&path);
                let view = bytes.and_then(|bytes| super::dds_texture(scene, name, &bytes).ok());
                if view.is_none() {
                    println!("post: the texture {name} of the shader {} was not read; white", shader.name);
                }
                view
            });
            compiled.push(Compiled { name: shader.name, disable_flags: shader.disable_flags, priority: shader.priority, pipelines, texture });
        }
        // The order of MGE XE: by the category, stable in the order of the chain.
        compiled.sort_by_key(|c| c.priority);
        println!("post: {} of {} shaders of the chain run: {ports} ports, {files} files of mods", compiled.len(), chain.len());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: size_of::<Variables>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let make_sampler = |filter: wgpu::FilterMode, address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: address, address_mode_v: address, address_mode_w: address, mag_filter: filter, min_filter: filter, ..Default::default() })
        };
        let samplers = [
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::ClampToEdge),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::Repeat),
            make_sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::MirrorRepeat),
            make_sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::Repeat),
        ];
        // A white pixel, for a shader without a texture.
        use wgpu::util::DeviceExt;
        let white = device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("post white"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[255, 255, 255, 255],
            )
            .create_view(&Default::default());
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
        let hdr = requires_hdr.then(|| Hdr::new(device, &blit_layout, hdr_reaction));
        let debug = std::env::var("MGE_XE_POST_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let debug_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post debug"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2),
                sampler(1, wgpu::SamplerBindingType::NonFiltering),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let debug_pipeline = full_screen_pipeline(device, "post debug", DEBUG_BLIT_SHADER, &debug_layout, scene.target_format);
        let debug_uniform = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("post debug"), contents: bytemuck::cast_slice(&[debug, 0.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM })
        };
        Post {
            chain: compiled,
            layout,
            uniforms,
            samplers,
            white,
            textures: Vec::new(),
            views: Vec::new(),
            size: (0, 0),
            depth: None,
            depth_pipeline,
            depth_layout,
            depth_uniform,
            blit_pipeline,
            blit_layout,
            debug,
            debug_pipeline,
            debug_layout,
            debug_uniform,
            waves,
            no_sun_glare,
            hdr,
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
        // The adaptation point of the frame, from the picture before the chain.
        let mut variables = *variables;
        if let Some(hdr) = self.hdr.as_mut() {
            variables.hdr = hdr.frame(device, encoder, picture, &self.blit_layout, &self.samplers[1], environment_flags);
        }
        let variables = &variables;
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
            if compiled.disable_flags & environment_flags != 0 {
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
                    label: Some(&compiled.name),
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
                        wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&self.samplers[3]) },
                        wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&self.samplers[4]) },
                        wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(compiled.texture.as_ref().unwrap_or(&self.white)) },
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
        // A look at a term, for a test.
        if self.debug > 0.0 {
            let source = if self.debug == 2.0 { depth_view } else { &self.views[last.unwrap_or(0)] };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post debug"),
                layout: &self.debug_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.debug_uniform.as_entire_binding() },
                ],
            });
            let mut pass = full_screen_pass(encoder, target, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.debug_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            return;
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
