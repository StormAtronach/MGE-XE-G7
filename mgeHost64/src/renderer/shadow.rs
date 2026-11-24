//! The shadow map of MGE XE: an atlas of four cascades (`R16Float`, 2 x res by 2 x res) with
//! the depth of the distant terrain and the distant statics from the sun. The cascades are
//! W437's (`moreFPS/docs/Numidium Project/designs/w437-stable-cascades-2026-11-24.md`): each
//! covers the bounding sphere of its slice of the camera's view, snapped to whole texels of
//! the sun's view from the world's origin, so the shadow's edges stay in place when the camera
//! moves. The casters, the depth and the blur are those of `renderShadowMap` of the Direct3D 9
//! path (`XE Shadowmap.fx`; the port specification `moreFPS/docs/plans/
//! mge-xe-shadow-port-spec.md`). This module has the resources, the matrices, the clear and the
//! blur, and a debug view; the casters are drawn by the terrain and the statics modules into
//! the passes that this module starts.

use nirender_wgpu::objects::{Create, CreateView};
use std::rc::Rc;

use nirender_wgpu::{SceneView, wgpu};

use super::core_files::SHADOWMAP;
use super::debug_view::DebugView;
#[cfg(test)]
use super::multiply;

/// The constants of `XE Mod Shadow Data.fx`: the built-in values of a core mod's
/// `XE Mod Shadow Data.wgsl` (`core_mods::ShadowValues`). `ESM_SCALE` matches the map's format.
pub const ESM_C: f32 = 60.0;
pub const ESM_BIAS: f32 = 2e-3 * ESM_C;
pub const ESM_SCALE: f32 = 32768.0;
pub const SHADE: f32 = 0.4;
pub const SHADE_COLOR: [f32; 3] = [1.0, 0.97, 0.81];

/// The light of the sun for `shadowSunEstimate`: the luminance of its colour, times 0.25 plus
/// 0.75 of its visibility, times the day of the hour (1 less the night). The ground, the
/// statics and the grass get the same value, so their shadows fade together through dusk
/// and dawn, and there is none when the night is whole.
pub fn sun_light(color: [f32; 3], sun_vis: f32, day: f32) -> f32 {
    let luminance = color[0] * 0.36 + color[1] * 0.53 + color[2] * 0.11;
    luminance * (0.25 + 0.75 * sun_vis) * day
}

/// The direction of the shadow camera: the sun light of the game, normalized, at every hour. The
/// receivers take their light from the same vector (`shadowSunEstimate`), so the map shows where
/// that light is blocked. None without a sun light: then the frame makes no shadow map.
pub fn light_vector(sun_direction: Option<[f32; 3]>) -> Option<[f32; 3]> {
    let [x, y, z] = sun_direction?;
    let length = (x * x + y * y + z * z).sqrt();
    (length > 1e-6).then(|| [x / length, y / length, z / length])
}

/// The radii of the two layers before the cascades (a test shows their snap fault), and the
/// depth range of a layer in units.
#[cfg(test)]
pub const RADII: [f32; 2] = [1000.0, 4000.0];
const ZRANGE: f32 = 8192.0;

const DEBUG_SHADER: &str = "
@group(0) @binding(0) var<uniform> u: vec4f;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Out {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

// The lower left part of the picture: the atlas of the four cascades at a quarter of the
// width and an eighth of the height (the place of the atlas of the two layers, so the square
// atlas shows squeezed).
@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    var out: Out;
    let corner = vec2f(f32(index & 1u), f32(index >> 1u));
    out.position = vec4f(-1.0 + corner.x * 0.5, -1.0 + corner.y * 0.25, 0.0, 1.0);
    out.uv = vec2f(corner.x, 1.0 - corner.y);
    return out;
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let d = textureSample(source, source_sampler, in.uv).r / u.x;
    return vec4f(d, d, d, 1.0);
}
";

pub struct Shadow {
    pub resolution: u32,
    /// The atlas of the four cascades, 2 x res by 2 x res: cascade i in the square
    /// (i mod 2, i div 2).
    pub atlas_view: Rc<wgpu::TextureView>,
    /// The depth from the sun of the cascade being drawn (res by res), before its blur.
    cast_view: wgpu::TextureView,
    blur_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    blur_pipeline: wgpu::RenderPipeline,
    /// The blur along x (the cast texture into the blur texture) and along y (the blur texture
    /// into the square of a cascade).
    blur_groups: [wgpu::BindGroup; 2],
    /// The uniforms of the blur groups, kept alive with them.
    _blur_uniforms: [wgpu::Buffer; 2],
    debug_pipeline: wgpu::RenderPipeline,
    /// The sample count of [`Shadow::debug_pipeline`]: that of the frame when the map was made.
    debug_samples: u32,
    debug_group: wgpu::BindGroup,
    /// The debug view `shadow-map`: the map drawn small over the frame.
    pub debug: bool,
    /// The cascades of this frame.
    pub cascades: Cascades,
    /// Their world matrices in f32, with the layout of Direct3D: the casters and the cull.
    pub viewproj: [[f32; 16]; CASCADES],
}

/// The values of the cascades for a receiver that has its points less the eye (the grass, the
/// surfaces of the water volumes).
#[derive(Clone, Copy)]
pub struct Receiver {
    /// From a position less the eye to each cascade, with the layout of Direct3D.
    pub matrices: [[f32; 16]; CASCADES],
    /// The far ends of the slices along the view axis.
    pub ends: [f32; CASCADES],
    /// The depth ranges of the cascades / [`DEPTH_UNIT`].
    pub ranges: [f32; CASCADES],
    /// The view axis of the camera (w 0).
    pub axis: [f32; 4],
}

/// `D3DXMatrixLookAtRH`, with the layout of Direct3D (a position is a row vector).
#[cfg(test)]
fn look_at_rh(eye: [f32; 3], at: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normalize = |a: [f32; 3]| {
        let l = dot(a, a).sqrt().max(1e-12);
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let z = normalize(sub(eye, at));
    let x = normalize(cross(up, z));
    let y = cross(z, x);
    [
        x[0],
        y[0],
        z[0],
        0.0,
        x[1],
        y[1],
        z[1],
        0.0,
        x[2],
        y[2],
        z[2],
        0.0,
        -dot(x, eye),
        -dot(y, eye),
        -dot(z, eye),
        1.0,
    ]
}

/// `D3DXMatrixOrthoRH`.
#[cfg(test)]
fn ortho_rh(width: f32, height: f32, near: f32, far: f32) -> [f32; 16] {
    let mut m = [0.0f32; 16];
    m[0] = 2.0 / width;
    m[5] = 2.0 / height;
    m[10] = 1.0 / (near - far);
    m[14] = near / (near - far);
    m[15] = 1.0;
    m
}

/// The matrix of a layer, as `renderShadowLayer` of the Direct3D 9 path makes it: an
/// orthographic view from the sun, centred `radius` ahead of the camera (half of that up),
/// snapped to whole texels. The cascades replace it; a test keeps it to show its snap fault.
#[cfg(test)]
pub fn layer_matrix(eye: [f32; 3], eye_vec: [f32; 3], light_vec: [f32; 3], radius: f32, resolution: u32) -> [f32; 16] {
    let look_at = [
        eye[0] + radius * eye_vec[0],
        eye[1] + radius * eye_vec[1],
        eye[2] + 0.5 * radius * eye_vec[2],
    ];
    let look_at_eye = eye.map(|v| 16.0 * (0.0625 * v).floor());
    let camera = [
        look_at_eye[0] - ZRANGE * light_vec[0],
        look_at_eye[1] - ZRANGE * light_vec[1],
        look_at_eye[2] - ZRANGE * light_vec[2],
    ];
    let view = look_at_rh(camera, look_at_eye, [0.0, 0.0, 1.0]);
    let projection = ortho_rh(2.0 * radius, (1.0 + light_vec[2].abs()) * radius, 0.0, 2.0 * ZRANGE);
    let mut viewproj = multiply(&view, &projection);
    // The point `look_at` goes to the middle of the layer, in whole texels.
    let delta = [
        look_at_eye[0] - look_at[0],
        look_at_eye[1] - look_at[1],
        look_at_eye[2] - look_at[2],
    ];
    let dv: [f32; 3] =
        core::array::from_fn(|i| delta[0] * viewproj[i] + delta[1] * viewproj[4 + i] + delta[2] * viewproj[8 + i]);
    let quantizer = 2.0 / resolution as f32;
    viewproj[12] += quantizer * (dv[0] / quantizer).floor();
    viewproj[13] += quantizer * (dv[1] / quantizer).floor();
    viewproj[14] += dv[2];
    viewproj
}

/// The number of cascades of the map. Cascade i has the square (i mod 2, i div 2) of the atlas.
pub const CASCADES: usize = 4;
/// The reach of a cascade's depth range toward the sun, past its sphere: the casters that far
/// from the sphere shadow it.
const REACH: f64 = ZRANGE as f64;
/// The depth unit of the ESM values: the depth range of a layer before the cascades (16384
/// units). A cascade's depth goes into this unit, so `esm_c` and `esm_bias` keep their meaning
/// in the world in every cascade.
pub const DEPTH_UNIT: f32 = 2.0 * ZRANGE;
/// The last part of each slice: there a pixel blends its cascade's shadow with the next one's,
/// and in the last slice the shadow fades out. The shader has the same number
/// (`shadow_cascade_of`; a test of core_files checks it).
pub const BAND: f32 = 0.1;
/// The far end of cascade 0 when the draw distance is 4000 units or more.
const FIRST_END: f32 = 1000.0;

/// The far ends of the cascades' slices along the view axis: cascade 0 ends at
/// F = min(1000, D / 4), and the ends go geometric from F to the draw distance D.
pub fn slice_ends(draw_distance: f32) -> [f32; CASCADES] {
    let first = FIRST_END.min(draw_distance / 4.0) as f64;
    let ratio = draw_distance as f64 / first;
    core::array::from_fn(|i| (first * ratio.powf(i as f64 / 3.0)) as f32)
}

/// The near end of the slice of a cascade: the far end of the cascade before it, and 0 for the
/// first.
fn slice_start(ends: &[f32; CASCADES], cascade: usize) -> f32 {
    if cascade == 0 { 0.0 } else { ends[cascade - 1] }
}

/// The start of the band at the end of a slice: there the pixels start to blend in the next
/// cascade.
fn band_start(ends: &[f32; CASCADES], cascade: usize) -> f32 {
    let end = ends[cascade];
    end - BAND * (end - slice_start(ends, cascade))
}

/// The near end of what a cascade covers: the start of the band of the slice before it, where
/// its pixels first take it (addendum 7, verdict 795 4.1), and 0 for the first.
fn fit_start(ends: &[f32; CASCADES], cascade: usize) -> f32 {
    if cascade == 0 { 0.0 } else { band_start(ends, cascade - 1) }
}

/// The bounding sphere of a slice of the camera's view from `near` to `far` along the view axis,
/// for a view with the tangents `tan_x` and `tan_y` of its half angles: the distance of the
/// centre along the view axis, and the radius. Both depend on the slice and the field of view
/// only, so the turn of the camera does not change them.
pub fn slice_sphere(near: f32, far: f32, tan_x: f32, tan_y: f32) -> (f64, f64) {
    let (near, far) = (near as f64, far as f64);
    let k2 = (tan_x as f64).powi(2) + (tan_y as f64).powi(2);
    // The centre that is equally far from the corners of both ends of the slice; past the far
    // end, the centre of the far end, whose corners then bound the slice.
    let centre = (1.0 + k2) * (near + far) / 2.0;
    if centre >= far {
        return (far, far * k2.sqrt());
    }
    let radius = ((centre - near).powi(2) + near * near * k2).sqrt();
    (centre, radius)
}

fn dot64(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross64(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize64(a: [f64; 3]) -> [f64; 3] {
    let length = dot64(a, a).sqrt().max(1e-12);
    a.map(|v| v / length)
}

/// The basis of the sun's view: its x and y in the map, and the light vector as its depth axis.
/// It depends on the sun only. Its up is the world's z axis, as the layers have it, and the
/// world's y axis for a sun within 0.99 of the zenith, where z gives no basis.
fn sun_basis(light_vec: [f32; 3]) -> [[f64; 3]; 3] {
    let forward = normalize64(light_vec.map(f64::from));
    let up = if forward[2].abs() > 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    // As `look_at_rh`: z is toward the sun, so the map's x and y keep the layers' turn.
    let z = forward.map(|v| -v);
    let x = normalize64(cross64(up, z));
    let y = cross64(z, x);
    [x, y, forward]
}

/// The world matrix of a cascade, with the layout of Direct3D: the sun's view of the square of
/// 2 x `radius` around `centre`, with the centre moved to the nearest whole texel of the sun's
/// basis from the world's origin. So a fixed world point keeps its place in its texel when the
/// camera moves. The depth goes from 0, `REACH` past the sphere toward the sun, to 1 at the far
/// side of the sphere.
pub fn cascade_matrix(centre: [f64; 3], radius: f64, light_vec: [f32; 3], resolution: u32) -> [f64; 16] {
    let [x, y, forward] = sun_basis(light_vec);
    let texel = 2.0 * radius / resolution as f64;
    let snap = |v: f64| (v / texel).round() * texel;
    let (centre_x, centre_y) = (snap(dot64(centre, x)), snap(dot64(centre, y)));
    let range = 2.0 * radius + REACH;
    let near = dot64(centre, forward) - radius - REACH;
    let mut m = [0.0; 16];
    for i in 0..3 {
        m[i * 4] = x[i] / radius;
        m[i * 4 + 1] = y[i] / radius;
        m[i * 4 + 2] = forward[i] / range;
    }
    m[12] = -centre_x / radius;
    m[13] = -centre_y / radius;
    m[14] = -near / range;
    m[15] = 1.0;
    m
}

/// The cascades of a frame.
#[derive(Clone, Default)]
pub struct Cascades {
    /// The far ends of the slices along the view axis.
    pub ends: [f32; CASCADES],
    /// The depth range of each cascade in units: the sphere's diameter and the reach.
    pub ranges: [f32; CASCADES],
    /// The spheres: the centre in the world, and the radius (the tests read them).
    #[cfg_attr(not(test), allow(dead_code))]
    pub spheres: [([f64; 3], f64); CASCADES],
    /// The world matrices.
    pub world: [[f64; 16]; CASCADES],
}

impl Cascades {
    /// The cascades of a camera at `eye` with the view axis `eye_vec` and the half-angle
    /// tangents `tan`, to the draw distance, for the sun's `light_vec`.
    pub fn new(
        eye: [f32; 3],
        eye_vec: [f32; 3],
        tan: [f32; 2],
        draw_distance: f32,
        light_vec: [f32; 3],
        resolution: u32,
    ) -> Self {
        let ends = slice_ends(draw_distance);
        let axis = normalize64(eye_vec.map(f64::from));
        let spheres: [([f64; 3], f64); CASCADES] = core::array::from_fn(|i| {
            let (along, radius) = slice_sphere(fit_start(&ends, i), ends[i], tan[0], tan[1]);
            let centre = core::array::from_fn(|c| eye[c] as f64 + along * axis[c]);
            (centre, radius)
        });
        Cascades {
            ends,
            ranges: spheres.map(|(_, radius)| (2.0 * radius + REACH) as f32),
            spheres,
            world: spheres.map(|(centre, radius)| cascade_matrix(centre, radius, light_vec, resolution)),
        }
    }
}

/// The cascade of a pixel at the distance `z` along the view axis, and the weight of the next
/// cascade in it: 0 before the band at the end of the slice, 1 at the slice's end. In the band
/// of the last cascade the weight is that of no shadow. The cascade is `CASCADES` past the last
/// slice. The receivers' shader does the same (`shadow_cascade_of`); the tests use this one.
#[cfg(test)]
pub fn cascade_of(z: f32, ends: &[f32; CASCADES]) -> (usize, f32) {
    for (cascade, &end) in ends.iter().enumerate() {
        if z < end {
            let band = band_start(ends, cascade);
            return (cascade, ((z - band) / (end - band)).clamp(0.0, 1.0));
        }
    }
    (CASCADES, 1.0)
}

/// The step of the f16 map at the far end of a cascade's depth range, in the depth unit: the
/// receivers add it to the bias, so the rounding of the map does not shadow a lit surface.
#[cfg(test)]
pub fn depth_step(range: f32) -> f32 {
    range / 2048.0 / DEPTH_UNIT
}

/// The depth difference of a cascade in the depth unit, as the receivers make it: the map's
/// depth less the pixel's, both fractions of the cascade's depth range, times the range, plus
/// the cascade's f16 step. The shader does the same (`shadow_cascade_amount`).
#[cfg(test)]
pub fn delta_in_unit(map_depth: f32, pixel_depth: f32, range: f32) -> f32 {
    (map_depth - pixel_depth) * range / DEPTH_UNIT + depth_step(range)
}

/// The inverse of a matrix with the layout of Direct3D, in f64 (the identity when there is none).
fn inverse64(m: &[f64; 16]) -> [f64; 16] {
    let a = |r: usize, c: usize| m[r * 4 + c];
    let s0 = a(0, 0) * a(1, 1) - a(1, 0) * a(0, 1);
    let s1 = a(0, 0) * a(1, 2) - a(1, 0) * a(0, 2);
    let s2 = a(0, 0) * a(1, 3) - a(1, 0) * a(0, 3);
    let s3 = a(0, 1) * a(1, 2) - a(1, 1) * a(0, 2);
    let s4 = a(0, 1) * a(1, 3) - a(1, 1) * a(0, 3);
    let s5 = a(0, 2) * a(1, 3) - a(1, 2) * a(0, 3);
    let c5 = a(2, 2) * a(3, 3) - a(3, 2) * a(2, 3);
    let c4 = a(2, 1) * a(3, 3) - a(3, 1) * a(2, 3);
    let c3 = a(2, 1) * a(3, 2) - a(3, 1) * a(2, 2);
    let c2 = a(2, 0) * a(3, 3) - a(3, 0) * a(2, 3);
    let c1 = a(2, 0) * a(3, 2) - a(3, 0) * a(2, 2);
    let c0 = a(2, 0) * a(3, 1) - a(3, 0) * a(2, 1);
    let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;
    if det.abs() < 1e-30 {
        let mut identity = [0.0; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        return identity;
    }
    let d = 1.0 / det;
    [
        (a(1, 1) * c5 - a(1, 2) * c4 + a(1, 3) * c3) * d,
        (-a(0, 1) * c5 + a(0, 2) * c4 - a(0, 3) * c3) * d,
        (a(3, 1) * s5 - a(3, 2) * s4 + a(3, 3) * s3) * d,
        (-a(2, 1) * s5 + a(2, 2) * s4 - a(2, 3) * s3) * d,
        (-a(1, 0) * c5 + a(1, 2) * c2 - a(1, 3) * c1) * d,
        (a(0, 0) * c5 - a(0, 2) * c2 + a(0, 3) * c1) * d,
        (-a(3, 0) * s5 + a(3, 2) * s2 - a(3, 3) * s1) * d,
        (a(2, 0) * s5 - a(2, 2) * s2 + a(2, 3) * s1) * d,
        (a(1, 0) * c4 - a(1, 1) * c2 + a(1, 3) * c0) * d,
        (-a(0, 0) * c4 + a(0, 1) * c2 - a(0, 3) * c0) * d,
        (a(3, 0) * s4 - a(3, 1) * s2 + a(3, 3) * s0) * d,
        (-a(2, 0) * s4 + a(2, 1) * s2 - a(2, 3) * s0) * d,
        (-a(1, 0) * c3 + a(1, 1) * c1 - a(1, 2) * c0) * d,
        (a(0, 0) * c3 - a(0, 1) * c1 + a(0, 2) * c0) * d,
        (-a(3, 0) * s3 + a(3, 1) * s1 - a(3, 2) * s0) * d,
        (a(2, 0) * s3 - a(2, 1) * s1 + a(2, 2) * s0) * d,
    ]
}

/// The product of two matrices with the layout of Direct3D, in f64.
fn multiply64(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    core::array::from_fn(|i| (0..4).map(|k| a[(i / 4) * 4 + k] * b[k * 4 + i % 4]).sum())
}

/// The reach around the camera of what animates for the shadow (`FrameParameters::shadow_reach`
/// of the helper): the radius of the outer layer before the cascades, kept as it was.
pub const ANIMATION_REACH: f32 = 4000.0;

impl Shadow {
    /// The depth format of the shadow map for a frame whose depth has the format `frame_depth`:
    /// the same depth bits with no stencil, as the shadow map uses no stencil. The pipelines that
    /// draw into the map take this format too.
    pub fn depth_format(frame_depth: wgpu::TextureFormat) -> wgpu::TextureFormat {
        match frame_depth {
            wgpu::TextureFormat::Depth32FloatStencil8 => wgpu::TextureFormat::Depth32Float,
            wgpu::TextureFormat::Depth24PlusStencil8 => wgpu::TextureFormat::Depth24Plus,
            other => other,
        }
    }

    pub fn new(scene: &SceneView, resolution: u32) -> Shadow {
        let device = scene.device;
        // The atlas is two cascades wide and high: each at most half of the largest texture of
        // the GPU.
        let resolution = resolution.clamp(256, 4096).min(device.limits().max_texture_dimension_2d / 2);
        let target = |label, format, usage, size: u32| {
            device.counted_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let atlas = target("shadow atlas", wgpu::TextureFormat::R16Float, usage, 2 * resolution);
        let cast = target("shadow cast", wgpu::TextureFormat::R16Float, usage, resolution);
        let blur = target("shadow blur", wgpu::TextureFormat::R16Float, usage, resolution);
        let depth_view = target(
            "shadow depth",
            Shadow::depth_format(scene.depth_format),
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            resolution,
        )
        .counted_view(&Default::default());
        let atlas_view = Rc::new(atlas.counted_view(&Default::default()));
        let cast_view = cast.counted_view(&Default::default());
        let blur_view = blur.counted_view(&Default::default());
        let sampler = device.counted_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.counted_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // `samples`: the sample count of the pass the pipeline draws into.
        let make_pipeline = |label: &str,
                             source: &str,
                             format: wgpu::TextureFormat,
                             depth: Option<wgpu::DepthStencilState>,
                             samples: u32| {
            let module = device.counted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline_layout = device.counted_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            device.counted_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // The blur draws into the atlas (1 sample); the debug view draws into the main pass.
        let blur_pipeline = make_pipeline("shadow blur", SHADOWMAP.part("blur"), wgpu::TextureFormat::R16Float, None, 1);
        let debug_pipeline = make_pipeline(
            "shadow debug",
            DEBUG_SHADER,
            scene.target_format,
            Some(wgpu::DepthStencilState {
                format: scene.depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            scene.samples,
        );
        let uniform = |values: [f32; 4]| {
            device.counted_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow"),
                contents: bytemuck::cast_slice(&values),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        // The blur steps in texels as the layers had them (shadowRcpRes of the Direct3D 9 path,
        // 1 / res on both axes of an atlas 2 res wide): 2 texels along x, 1 along y.
        let rcp = 1.0 / resolution as f32;
        let blur_uniforms = [uniform([2.0 * rcp, 0.0, 0.0, 0.0]), uniform([0.0, rcp, 0.0, 0.0])];
        let group = |uniform: &wgpu::Buffer, view: &wgpu::TextureView| {
            device.counted_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        // Along x: the cast texture into the blur texture. Along y: into the cascade's square.
        let blur_groups = [group(&blur_uniforms[0], &cast_view), group(&blur_uniforms[1], &blur_view)];
        let debug_uniform = uniform([ESM_SCALE, 0.0, 0.0, 0.0]);
        let debug_group = group(&debug_uniform, &atlas_view);
        Shadow {
            resolution,
            atlas_view,
            cast_view,
            blur_view,
            depth_view,
            blur_pipeline,
            blur_groups,
            _blur_uniforms: blur_uniforms,
            debug_pipeline,
            debug_samples: scene.samples,
            debug_group,
            debug: false,
            cascades: Cascades::default(),
            viewproj: [[0.0; 16]; CASCADES],
        }
    }

    /// The cascades of a frame: the camera at `eye` with the view axis `eye_vec`, its
    /// projection (the half angles of its view), the draw distance, and the sun's light.
    pub fn set_frame(
        &mut self,
        eye: [f32; 3],
        eye_vec: [f32; 3],
        projection: &[f32; 16],
        draw_distance: f32,
        light_vec: [f32; 3],
    ) {
        let tan = [1.0 / projection[0].abs().max(1e-6), 1.0 / projection[5].abs().max(1e-6)];
        self.cascades = Cascades::new(eye, eye_vec, tan, draw_distance, light_vec, self.resolution);
        self.viewproj = self.cascades.world.map(|world| world.map(|v| v as f32));
    }

    /// Starts the pass of the casters of a cascade: the cast texture cleared to [`ESM_SCALE`],
    /// the depth to 1. The caller draws the casters, ends the pass, and then calls `blur`.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) -> wgpu::RenderPass<'static> {
        nirender_wgpu::gpu::note_other_pass(super::part::SHADOW_MAP);
        encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.cast_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: ESM_SCALE as f64,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime()
    }

    /// The two blurs of a cascade, after its casters: along x from the cast texture into the
    /// blur texture, along y from the blur texture into the cascade's square of the atlas.
    pub fn blur(&self, encoder: &mut wgpu::CommandEncoder, cascade: usize) {
        for step in 0..2 {
            nirender_wgpu::gpu::note_inner_pass("shadow blur");
            let (target, load) = if step == 0 {
                (&self.blur_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK))
            } else {
                (&*self.atlas_view, wgpu::LoadOp::Load)
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if step == 1 {
                let res = self.resolution;
                let (x, y) = ((cascade as u32 % 2) * res, (cascade as u32 / 2) * res);
                pass.set_viewport(x as f32, y as f32, res as f32, res as f32, 0.0, 1.0);
                pass.set_scissor_rect(x, y, res, res);
            }
            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &self.blur_groups[step], &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The shadow map's debug view as the frame's view has it. The view of the cascades is a
    /// variant of the world shader (`DebugView::frame_id`).
    pub fn set_debug(&mut self, view: &DebugView) {
        self.debug = view.shadow_map();
    }

    /// The atlas in the lower left of the picture, for a test. `samples`: the sample count of
    /// the frame (`SceneView::samples` of the main view), not of the pass. A frame whose count
    /// is not the one the pipeline was made with (the count changed after the map was made)
    /// gets no atlas. The check does not see a pass of its own count, such as a rendered
    /// picture.
    pub fn draw_debug(&self, pass: &mut wgpu::RenderPass<'static>, samples: u32) {
        if samples != self.debug_samples {
            return;
        }
        pass.set_pipeline(&self.debug_pipeline);
        pass.set_bind_group(0, &self.debug_group, &[]);
        pass.draw(0..4, 0..1);
    }

    /// The values of the cascades for a receiver at the camera `eye` with the view axis
    /// `axis`: the matrices from a position less the eye, made in f64.
    pub fn receiver(&self, eye: [f64; 3], axis: [f32; 3]) -> Receiver {
        let matrices = self.cascades.world.map(|world| {
            let mut relative = world;
            for c in 0..4 {
                relative[12 + c] += eye[0] * world[c] + eye[1] * world[4 + c] + eye[2] * world[8 + c];
            }
            relative.map(|v| v as f32)
        });
        Receiver {
            matrices,
            ends: self.cascades.ends,
            ranges: self.cascades.ranges.map(|range| range / DEPTH_UNIT),
            axis: [axis[0], axis[1], axis[2], 0.0],
        }
    }

    /// The values of the frame for the shader functions of the draws of the game: the
    /// matrices from the view space of the game to cascades 0 and 1 (rows 0 to 7, the rows of
    /// Direct3D), the values of the shadow (`shadow_values`, the built-in ones or those of a
    /// core mod; row 8 and 9), the visibility of the sun and 1 when the camera is above the sea
    /// (row 10), the matrices of cascades 2 and 3 (rows 13 to 20), the far ends of the slices
    /// (row 21) and the depth ranges / [`DEPTH_UNIT`] (row 22). Rows 11 and 12 are the
    /// renderer's.
    pub fn frame_values(
        &self,
        sun_vis: f32,
        view: &[f32; 16],
        above_sea: bool,
        shadow_values: &super::core_mods::ShadowValues,
    ) -> [[f32; 4]; 32] {
        let mut values = [[0.0f32; 4]; 32];
        let from_view = inverse64(&view.map(f64::from));
        for (cascade, world) in self.cascades.world.iter().enumerate() {
            let to_shadow = multiply64(&from_view, world);
            let first = CASCADE_ROWS[cascade];
            for row in 0..4 {
                values[first + row] = core::array::from_fn(|c| to_shadow[row * 4 + c] as f32);
            }
        }
        values[8] = shadow_values.esm(1.0 / self.resolution as f32);
        values[9] = shadow_values.shade();
        values[10] = [sun_vis, if above_sea { 1.0 } else { 0.0 }, 0.0, 0.0];
        values[21] = self.cascades.ends;
        values[22] = self.cascades.ranges.map(|range| range / DEPTH_UNIT);
        values
    }
}

/// The first row of each cascade's matrix in the frame values (`shadow_place` of the world
/// shader): rows 8 to 12 keep the places they had before the cascades.
const CASCADE_ROWS: [usize; CASCADES] = [0, 4, 13, 17];

#[cfg(test)]
mod tests {
    use super::{
        BAND, CASCADES, Cascades, DEBUG_SHADER, DEPTH_UNIT, ESM_BIAS, ESM_C, ESM_SCALE, RADII, Shadow, cascade_of,
        delta_in_unit, depth_step, layer_matrix, light_vector, slice_ends, slice_sphere, sun_light,
    };
    use nirender_wgpu::wgpu::TextureFormat;

    /// The half-angle tangents of the default view: 75 degrees across, 16:9.
    fn default_tan() -> [f32; 2] {
        let tan_x = (37.5f32).to_radians().tan();
        [tan_x, tan_x * 9.0 / 16.0]
    }

    /// The default draw distance: 7 cells.
    const DRAW_DISTANCE: f32 = 7.0 * 8192.0;

    /// A point in the map of a world matrix: x and y from -1 to 1 over the map, and the depth.
    fn project64(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|c| p[0] * m[c] + p[1] * m[4 + c] + p[2] * m[8 + c] + m[12 + c])
    }

    fn unit(v: [f64; 3]) -> [f64; 3] {
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|c| c / length)
    }

    /// The corners of the slice of a view at `eye` along the unit `axis`, from `near` to `far`
    /// along it, for the half-angle tangents `tan`, with the world's z as the view's up (or y
    /// for a view straight up or down).
    fn slice_corners(eye: [f64; 3], axis: [f64; 3], tan: [f32; 2], near: f64, far: f64) -> Vec<[f64; 3]> {
        let up = if axis[2].abs() > 0.99 {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let right = unit(cross(axis, up));
        let top = cross(right, axis);
        let mut corners = Vec::new();
        for distance in [near, far] {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                corners.push(core::array::from_fn(|c| {
                    eye[c] + distance * (axis[c] + sx * tan[0] as f64 * right[c] + sy * tan[1] as f64 * top[c])
                }));
            }
        }
        corners
    }

    /// The splits of the default: F = 1000, the ends 1000, 3856, 14870 and 57344 within one
    /// unit, the last at the draw distance, and the three ratios equal. A draw distance of
    /// 2000 gives F = 500.
    #[test]
    fn the_slices_are_geometric_from_the_first_end_to_the_draw_distance() {
        let ends = slice_ends(DRAW_DISTANCE);
        for (end, expected) in ends.iter().zip([1000.0, 3856.0, 14870.0, 57344.0]) {
            assert!((end - expected).abs() < 1.0, "{ends:?}");
        }
        assert_eq!(ends[3], DRAW_DISTANCE);
        let ratios = [ends[1] / ends[0], ends[2] / ends[1], ends[3] / ends[2]];
        for ratio in ratios {
            assert!((ratio - ratios[0]).abs() < 1e-4, "{ratios:?}");
        }
        let short = slice_ends(2000.0);
        assert!((short[0] - 500.0).abs() < 1e-3, "{short:?}");
        assert!((short[3] - 2000.0).abs() < 1e-2, "{short:?}");
    }

    /// The sphere of each slice has the radius of the design's table at the default, and holds
    /// all corners of its slice for 8 view directions, with the same radius for each.
    #[test]
    fn the_sphere_of_a_slice_holds_it_in_every_view_direction() {
        let tan = default_tan();
        let ends = slice_ends(DRAW_DISTANCE);
        let eye = [32864.0, -107932.0, 1835.0];
        let directions = [
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [-1.0, -1.0, 0.0],
            [0.3, -1.0, -0.4],
            [0.0, 1.0, 0.9],
            [-0.5, 0.2, -1.0],
            [0.0, 0.0, -1.0],
            [0.7, 0.7, 0.3],
        ];
        for (cascade, expected) in [888.0, 3395.0, 13092.0, 50485.0].into_iter().enumerate() {
            // From the start of the band of the slice before (addendum 7): no change of the radius
            // at 75 degrees.
            let near = if cascade == 0 {
                0.0
            } else {
                let before = if cascade == 1 { 0.0 } else { ends[cascade - 2] };
                ends[cascade - 1] - BAND * (ends[cascade - 1] - before)
            };
            for direction in directions {
                let cascades = Cascades::new(
                    eye.map(|v| v as f32),
                    direction.map(|v| v as f32),
                    tan,
                    DRAW_DISTANCE,
                    [0.4, -0.5, -0.75],
                    2048,
                );
                let (centre, radius) = cascades.spheres[cascade];
                assert!((radius - expected).abs() < 1.0, "cascade {cascade}: {radius}");
                assert_eq!(radius, slice_sphere(near, ends[cascade], tan[0], tan[1]).1);
                for corner in slice_corners(eye, unit(direction), tan, near as f64, ends[cascade] as f64) {
                    let distance = (0..3).map(|c| (corner[c] - centre[c]).powi(2)).sum::<f64>().sqrt();
                    assert!(
                        distance <= radius * (1.0 + 1e-6),
                        "cascade {cascade} {direction:?}: {distance} {radius}"
                    );
                }
            }
        }
    }

    /// The place of a fixed world point in its texel, in each cascade, while the eye moves in
    /// steps of one unit over 64 units: the snap keeps it. The same test on the layers of today
    /// finds a change.
    #[test]
    fn a_fixed_point_keeps_its_place_in_its_texel_when_the_eye_moves() {
        let light = light_vector(Some([-0.416, 0.546, -0.728])).unwrap();
        let point = [32964.0f64, -107732.0, 1700.0];
        let resolution = 2048.0;
        let place = |x: f64| {
            let texel = (x + 1.0) * 0.5 * resolution;
            texel - texel.floor()
        };
        let eye_at = |step: usize| [32864.0 + step as f32, -107932.0 + 0.5 * step as f32, 1835.0];
        for cascade in 0..CASCADES {
            let first = Cascades::new(eye_at(0), [0.0, 1.0, 0.0], default_tan(), DRAW_DISTANCE, light, 2048);
            let start = project64(&first.world[cascade], point);
            for step in 1..=64 {
                let cascades = Cascades::new(eye_at(step), [0.0, 1.0, 0.0], default_tan(), DRAW_DISTANCE, light, 2048);
                let moved = project64(&cascades.world[cascade], point);
                for axis in 0..2 {
                    let (a, b) = (place(start[axis]), place(moved[axis]));
                    let difference = (a - b).abs().min(1.0 - (a - b).abs());
                    assert!(difference < 1e-6, "cascade {cascade} step {step} axis {axis}: {a} {b}");
                }
            }
        }
        // Today's layers: the place in the texel changes.
        let mut largest = 0.0f64;
        for radius in RADII {
            let start = layer_matrix(eye_at(0), [0.0, 1.0, 0.0], light, radius, 2048).map(f64::from);
            let start = project64(&start, point);
            for step in 1..=64 {
                let moved = layer_matrix(eye_at(step), [0.0, 1.0, 0.0], light, radius, 2048).map(f64::from);
                let moved = project64(&moved, point);
                for axis in 0..2 {
                    let (a, b) = (place(start[axis]), place(moved[axis]));
                    largest = largest.max((a - b).abs().min(1.0 - (a - b).abs()));
                }
            }
        }
        assert!(largest > 0.1, "{largest}");
    }

    /// The cascade of a distance and the weights of the cascades are continuous over each
    /// band: the weight of each cascade changes by a small amount for a small step, also at the
    /// ends of the slices. The last band fades to no shadow at the draw distance, from 53097.
    #[test]
    fn the_choice_of_a_cascade_and_its_blend_are_continuous() {
        let ends = slice_ends(DRAW_DISTANCE);
        let weights = |z: f32| {
            let (cascade, blend) = cascade_of(z, &ends);
            let mut w = [0.0f32; CASCADES + 1];
            // The last weight is that of no shadow: past the last slice, and the fade of the
            // last cascade.
            if cascade < CASCADES {
                w[cascade] = 1.0 - blend;
                w[cascade + 1] += blend;
            } else {
                w[CASCADES] = 1.0;
            }
            w
        };
        let mut z = 0.0f32;
        let mut before = weights(z);
        while z < DRAW_DISTANCE + 100.0 {
            z += 0.25;
            let now = weights(z);
            for i in 0..=CASCADES {
                assert!((now[i] - before[i]).abs() < 0.01, "z {z}: {before:?} {now:?}");
            }
            before = now;
        }
        assert_eq!(cascade_of(899.0, &ends), (0, 0.0));
        assert_eq!(cascade_of(3569.0, &ends).1, 0.0);
        assert!(cascade_of(3700.0, &ends).1 > 0.0);
        let last_band = ends[3] - 0.1 * (ends[3] - ends[2]);
        assert!((last_band - 53097.0).abs() < 1.0, "{last_band}");
        assert_eq!(cascade_of(53090.0, &ends), (3, 0.0));
        assert_eq!(cascade_of(DRAW_DISTANCE, &ends), (CASCADES, 1.0));
    }

    /// A sun at the zenith, and one just past the limit of the basis, give valid matrices: no
    /// value that is not a number, and a point under the sun goes deeper along the light.
    #[test]
    fn a_sun_at_the_zenith_gives_a_valid_map() {
        for light in [[0.0, 0.0, -1.0], [0.01, 0.0, -1.0], [0.2, 0.1, -0.99]] {
            let light = light_vector(Some(light)).unwrap();
            let cascades = Cascades::new(
                [100.0, 200.0, 300.0],
                [0.0, 1.0, 0.0],
                default_tan(),
                DRAW_DISTANCE,
                light,
                2048,
            );
            for world in cascades.world {
                assert!(world.iter().all(|v| v.is_finite()), "{light:?}");
                let p = [100.0, 1000.0, 0.0];
                let a = project64(&world, p);
                let b = project64(&world, core::array::from_fn(|c| p[c] + 50.0 * light[c] as f64));
                assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9, "{light:?}");
                assert!(b[2] > a[2], "{light:?}");
            }
        }
    }

    /// The value of the f16 map nearest to `v`.
    fn f16_round(v: f32) -> f32 {
        if v <= 0.0 {
            return 0.0;
        }
        let step = 2f32.powi(v.log2().floor() as i32 - 10);
        (v / step).round() * step
    }

    /// The ESM in world units: a caster 100 units above a receiver toward the sun gives the
    /// same depth difference in each cascade, in the depth unit, within the cascade's f16
    /// step, and so the same amount of `shadow_esm`.
    #[test]
    fn a_caster_gives_the_same_shadow_in_every_cascade() {
        let light = light_vector(Some([-0.416, 0.546, -0.728])).unwrap();
        let cascades = Cascades::new(
            [32864.0, -107932.0, 1835.0],
            [0.0, 1.0, 0.0],
            default_tan(),
            DRAW_DISTANCE,
            light,
            2048,
        );
        let esm = |dz: f32| 1.0 - (ESM_C * dz + ESM_BIAS).exp().clamp(0.0, 1.0);
        let exact = -100.0 / DEPTH_UNIT;
        for cascade in 0..CASCADES {
            let (receiver, _) = cascades.spheres[cascade];
            let caster: [f64; 3] = core::array::from_fn(|c| receiver[c] - 100.0 * light[c] as f64);
            let world = &cascades.world[cascade];
            let (pixel, map) = (project64(world, receiver)[2] as f32, project64(world, caster)[2] as f32);
            let range = cascades.ranges[cascade];
            let stored = f16_round(map * ESM_SCALE) / ESM_SCALE;
            let dz = delta_in_unit(stored, pixel, range);
            let step = depth_step(range);
            assert!((dz - (exact + step)).abs() <= step, "cascade {cascade}: {dz} {exact} {step}");
            let (low, high) = (esm(exact), esm(exact + 2.0 * step));
            let amount = esm(dz);
            assert!(
                amount <= low + 1e-6 && amount >= high - 1e-6,
                "cascade {cascade}: {amount} {low} {high}"
            );
            assert!(amount > 0.0, "cascade {cascade}: {amount}");
            println!("cascade {cascade}: dz {dz}, step {step}, amount {amount}");
        }
    }

    /// The half-angle tangents of a 16:9 view with the horizontal field of view `degrees`.
    fn tan_of(degrees: f32) -> [f32; 2] {
        let tan_x = (0.5 * degrees).to_radians().tan();
        [tan_x, tan_x * 9.0 / 16.0]
    }

    /// Every point of a slice is in the map of its cascade, in x, y and depth, for a high
    /// camera, a high sun and a side sun, at fields of view of 75, 40, 25 and 10 degrees. So is
    /// every point of the band at the end of the slice before it, where a pixel mixes in this
    /// cascade (verdict 795, 4.1). And the ground under the camera is in cascade 0 when its
    /// distance along the view axis is in slice 0.
    #[test]
    fn each_slice_is_inside_the_map_of_its_cascade() {
        let ends = slice_ends(DRAW_DISTANCE);
        let poses: [([f32; 3], [f32; 3]); 3] = [
            // A camera 600 units above the ground looking down 40 degrees.
            ([1000.0, 2000.0, 600.0], [0.0, 0.766, -0.643]),
            ([-50000.0, 30000.0, 2500.0], [0.6, 0.8, -0.1]),
            ([32864.0, -107932.0, 1835.0], [0.0, 1.0, 0.0]),
        ];
        let suns = [[0.05, 0.1, -1.0], [-0.95, 0.1, -0.25], [-0.416, 0.546, -0.728]];
        for degrees in [75.0, 40.0, 25.0, 10.0] {
            let tan = tan_of(degrees);
            for (eye, axis) in poses {
                for sun in suns {
                    let light = light_vector(Some(sun)).unwrap();
                    let cascades = Cascades::new(eye, axis, tan, DRAW_DISTANCE, light, 2048);
                    for cascade in 0..CASCADES {
                        let near = if cascade == 0 { 0.0 } else { ends[cascade - 1] as f64 };
                        let mut corners =
                            slice_corners(eye.map(f64::from), unit(axis.map(f64::from)), tan, near, ends[cascade] as f64);
                        if cascade > 0 {
                            // The band at the end of the slice before: from its start to `near`.
                            let before = if cascade == 1 { 0.0 } else { ends[cascade - 2] as f64 };
                            let band = near - BAND as f64 * (near - before);
                            corners.extend(slice_corners(eye.map(f64::from), unit(axis.map(f64::from)), tan, band, near));
                        }
                        for corner in corners {
                            let p = project64(&cascades.world[cascade], corner);
                            let inside = p[0].abs() <= 1.0 && p[1].abs() <= 1.0 && (0.0..=1.0).contains(&p[2]);
                            assert!(inside, "{degrees} degrees, cascade {cascade}, sun {sun:?}: {p:?}");
                        }
                    }
                    // The ground under the high camera: 600 units down and 500 ahead, in the view
                    // at 75 degrees.
                    if eye[2] == 600.0 && degrees == 75.0 {
                        let ground = [eye[0] as f64, eye[1] as f64 + 500.0, 0.0];
                        let along = (0..3)
                            .map(|c| (ground[c] - eye[c] as f64) * unit(axis.map(f64::from))[c])
                            .sum::<f64>();
                        assert_eq!(cascade_of(along as f32, &ends).0, 0, "{along}");
                        let p = project64(&cascades.world[0], ground);
                        assert!(p[0].abs() <= 1.0 && p[1].abs() <= 1.0 && (0.0..=1.0).contains(&p[2]), "{p:?}");
                    }
                }
            }
        }
    }

    /// The shadow camera follows the sun light of the game, normalized, by day and at night
    /// alike: the light's direction is the only input. A frame without a sun light, or with a
    /// light of no length, makes no map.
    #[test]
    fn the_shadow_camera_follows_the_sun_light() {
        // The light of W48's noon frame (toward the sun 0.416 -0.546 0.728), scaled; and a
        // night light, from below the horizon.
        for direction in [[-0.832, 1.092, -1.456], [0.3, -0.2, 0.5]] {
            let unit = light_vector(Some(direction)).unwrap();
            let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
            for axis in 0..3 {
                assert!((unit[axis] - direction[axis] / length).abs() < 1e-6, "{direction:?}");
            }
        }
        assert_eq!(light_vector(None), None);
        assert_eq!(light_vector(Some([0.0, 0.0, 0.0])), None);
    }

    /// The camera of each layer looks along the light vector it is given: a point moved along
    /// that vector keeps its place in the map and only goes deeper.
    #[test]
    fn a_layer_looks_along_the_light_vector() {
        let light = light_vector(Some([-0.416, 0.546, -0.728])).unwrap();
        let eye = [32864.0, -107932.0, 1835.0];
        let project = |m: &[f32; 16], p: [f32; 3]| {
            let w = p[0] * m[3] + p[1] * m[7] + p[2] * m[11] + m[15];
            [0, 1, 2].map(|c| (p[0] * m[c] + p[1] * m[4 + c] + p[2] * m[8 + c] + m[12 + c]) / w)
        };
        for radius in RADII {
            let matrix = layer_matrix(eye, [0.0, 1.0, 0.0], light, radius, 2048);
            let near = project(&matrix, eye);
            let far = project(&matrix, [0, 1, 2].map(|c| eye[c] + 500.0 * light[c]));
            assert!(
                (near[0] - far[0]).abs() < 1e-4 && (near[1] - far[1]).abs() < 1e-4,
                "radius {radius}: {near:?} {far:?}"
            );
            assert!(far[2] > near[2], "radius {radius}: {near:?} {far:?}");
        }
    }

    /// The shaders of the shadow map compile as the device checks them: the blur of the map,
    /// and the debug view of its atlas.
    #[test]
    fn the_shaders_of_the_shadow_map_are_valid() {
        let check = crate::renderer::shader_check::check;
        check("shadow blur", crate::renderer::core_files::SHADOWMAP.part("blur")).unwrap();
        check("shadow debug", DEBUG_SHADER).unwrap();
    }

    /// The shadow map keeps the depth bits of the frame's format and drops the stencil. A format
    /// with no stencil stays as it is.
    #[test]
    fn the_shadow_depth_has_no_stencil() {
        assert_eq!(
            Shadow::depth_format(TextureFormat::Depth32FloatStencil8),
            TextureFormat::Depth32Float
        );
        assert_eq!(
            Shadow::depth_format(TextureFormat::Depth24PlusStencil8),
            TextureFormat::Depth24Plus
        );
        assert_eq!(Shadow::depth_format(TextureFormat::Depth32Float), TextureFormat::Depth32Float);
        for frame in [TextureFormat::Depth32FloatStencil8, TextureFormat::Depth24PlusStencil8] {
            assert!(!Shadow::depth_format(frame).has_stencil_aspect(), "{frame:?}");
            assert!(Shadow::depth_format(frame).has_depth_aspect(), "{frame:?}");
        }
    }

    /// The grass and the ground take the same light of the sun for their shadow: none when
    /// the night is whole, the full strength by day, and the fade of the hour between.
    #[test]
    fn the_shadow_light_fades_with_the_day() {
        let white = [1.0, 1.0, 1.0];
        assert_eq!(sun_light(white, 1.0, 0.0), 0.0);
        assert!((sun_light(white, 1.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((sun_light(white, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((sun_light(white, 0.0, 1.0) - 0.25).abs() < 1e-6);
    }
}
