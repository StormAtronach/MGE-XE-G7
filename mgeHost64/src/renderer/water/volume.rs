//! The surfaces of water volumes near the camera: shapes of the game whose material a mod
//! marked (the specular power 99998, 99997, or 100000 and more for a look slot). The renderer
//! gives them to the extension (`Extension::hold`), and they are drawn here with the shading of
//! the water in place of their own look. `XE Mod Water Volume.fx` and `renderWaterVolume` of
//! the Direct3D 9 path.
//!
//! A surface can face any way: the ripples lie in its plane and the normal of the mesh turns
//! them. Its reflection is the sky in the reflected direction, with what a march of the
//! reflected ray finds in the picture over it: the mirror picture of the sea is at the level
//! of the sea, so it cannot serve a pond on a hill or a sloped face.

use std::collections::HashMap;

use nirender_wgpu::{FrameLights, HeldDraw, SceneView, wgpu};

use super::shading;

/// The specular power of a surface that reflects what is in the picture and the sky.
const REFLECTS_SCENE: f32 = 99998.0;
/// The specular power of a surface that reflects the sky only.
const REFLECTS_SKY: f32 = 99997.0;
/// The specular power of look slot 0; there are 4096 slots (`WaterLooks::maxSlots`).
const SLOT_FIRST: f32 = 100000.0;
const SLOTS: f32 = 4096.0;

/// True for a material that a mod marked as the surface of a water volume.
pub fn marked(specular_power: f32) -> bool {
    specular_power == REFLECTS_SCENE || specular_power == REFLECTS_SKY || (SLOT_FIRST..SLOT_FIRST + SLOTS).contains(&specular_power)
}

/// The look of a surface beyond its colour, as a mod sets it for a slot (`MGE_WaterLookSet`):
/// the layout of `WaterLook` of `waterlook.h`, which the record of the looks carries.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Look {
    size: u32,
    flags: u32,
    /// The drift of the ripples, in the axes of the mesh, in units for each second.
    flow: [f32; 2],
    /// The speed and the size of the ripples; 1 is the standard.
    speed: f32,
    scale: f32,
    /// The light that the surface gives, 0 to 1.
    glow: f32,
    /// 1 is water; lower shows what is behind the surface.
    opacity: f32,
    /// The free values p0 to p3, and the name of a water shader of a mod.
    params: [[f32; 4]; 4],
    shader: [u8; 32],
    /// What the surface reflects in place of the sky, with `LOOK_HAS_SKY`.
    sky: [f32; 3],
    /// Over how many units of water what is under the surface fades into the colour of deep
    /// water; 800 is the standard, 0 shows nothing.
    clarity: f32,
}

/// The flags of a look.
const LOOK_REFLECTS_SCENE: u32 = 1;
const LOOK_TINT_FROM_VERTEX: u32 = 2;
const LOOK_OPACITY_FROM_VERTEX: u32 = 4;
const LOOK_HAS_SKY: u32 = 8;

impl Look {
    /// The standard look (`standardLook` of `waterlook.cpp`).
    pub const STANDARD: Look = Look {
        size: size_of::<Look>() as u32,
        flags: LOOK_REFLECTS_SCENE,
        flow: [0.0; 2],
        speed: 1.0,
        scale: 1.0,
        glow: 0.0,
        opacity: 1.0,
        params: [[0.0; 4]; 4],
        shader: [0; 32],
        sky: [0.0; 3],
        clarity: 800.0,
    };

    /// The looks of the slots from the record of the looks (6): the version, the number of
    /// slots, then for each its number and its `WaterLook`. A look that is shorter than this
    /// layout keeps the standard values after its end. `None` for a record that is not one.
    pub fn read_table(bytes: &[u8]) -> Option<HashMap<u32, Look>> {
        let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        if word(0)? != 1 {
            return None;
        }
        let count = word(4)?;
        let mut table = HashMap::new();
        let mut at = 8;
        for _ in 0..count {
            let slot = word(at)?;
            let size = (word(at + 4)? as usize).max(8);
            let source = bytes.get(at + 4..at + 4 + size)?;
            let mut raw = bytemuck::bytes_of(&Look::STANDARD).to_vec();
            let length = size.min(raw.len());
            raw[..length].copy_from_slice(&source[..length]);
            table.insert(slot, bytemuck::pod_read_unaligned(&raw));
            at += 4 + size;
        }
        Some(table)
    }

    /// The look of a water subset among the distant statics: kind 1 reflects the scene and 2
    /// the sky only (flags 0x04 and 0x08 of the subset), and the look line that the generator
    /// copied from the mesh, when there is one (`WaterLooks::parse` and the load of
    /// `distantstatics.cpp`).
    pub fn of_distant(kind: u8, line: Option<&str>) -> Look {
        let mut look = line.map_or(Look::STANDARD, Look::parse);
        if kind == 2 || (line.is_none() && kind != 1) {
            look.flags &= !LOOK_REFLECTS_SCENE;
        }
        look
    }

    /// A look line, as "flow=0,-140 speed=1.2 sky=FF8040 reflect=sky": the keys of
    /// `WaterLooks::parse`. A key that it does not know is left out.
    pub fn parse(line: &str) -> Look {
        let mut look = Look::STANDARD;
        // Up to `out.len()` numbers with commas between them; a number that does not read
        // ends them.
        fn numbers(value: &str, out: &mut [f32]) {
            for (slot, part) in out.iter_mut().zip(value.split(',')) {
                match part.trim().parse::<f32>() {
                    Ok(number) => *slot = number,
                    Err(_) => return,
                }
            }
        }
        for word in line.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else { continue };
            match key.to_ascii_lowercase().as_str() {
                "flow" => numbers(value, &mut look.flow),
                "speed" => numbers(value, core::slice::from_mut(&mut look.speed)),
                "scale" => numbers(value, core::slice::from_mut(&mut look.scale)),
                "glow" => numbers(value, core::slice::from_mut(&mut look.glow)),
                "clarity" => numbers(value, core::slice::from_mut(&mut look.clarity)),
                "opacity" if value.eq_ignore_ascii_case("vertex") => look.flags |= LOOK_OPACITY_FROM_VERTEX,
                "opacity" => {
                    look.flags &= !LOOK_OPACITY_FROM_VERTEX;
                    numbers(value, core::slice::from_mut(&mut look.opacity));
                }
                "sky" => {
                    let digits = value.strip_prefix('#').unwrap_or(value);
                    match (digits.len() == 6).then(|| u32::from_str_radix(digits, 16).ok()).flatten() {
                        Some(colour) => look.sky = [(colour >> 16 & 255) as f32 / 255.0, (colour >> 8 & 255) as f32 / 255.0, (colour & 255) as f32 / 255.0],
                        None => numbers(value, &mut look.sky),
                    }
                    look.flags |= LOOK_HAS_SKY;
                }
                "reflect" if value.eq_ignore_ascii_case("sky") => look.flags &= !LOOK_REFLECTS_SCENE,
                "shader" => {
                    look.shader = [0; 32];
                    for (slot, byte) in look.shader.iter_mut().zip(value.bytes().take(31)) {
                        *slot = byte;
                    }
                }
                key if key.len() == 2 && key.starts_with('p') && (b'0'..=b'3').contains(&key.as_bytes()[1]) => {
                    numbers(value, &mut look.params[(key.as_bytes()[1] - b'0') as usize]);
                }
                _ => {}
            }
        }
        look
    }

    /// The name of the water shader of a mod that the look names, in small letters; `None` for
    /// the standard look.
    pub fn shader_name(&self) -> Option<String> {
        let length = self.shader.iter().position(|&b| b == 0).unwrap_or(self.shader.len());
        let name = String::from_utf8_lossy(&self.shader[..length]).trim().to_ascii_lowercase();
        (!name.is_empty()).then_some(name)
    }

    /// The look of a marked surface: that of its slot, or the standard look; 99997 reflects
    /// the sky only.
    pub fn of(specular_power: f32, table: &HashMap<u32, Look>) -> Look {
        if specular_power == REFLECTS_SKY {
            return Look { flags: Look::STANDARD.flags & !LOOK_REFLECTS_SCENE, ..Look::STANDARD };
        }
        if specular_power >= SLOT_FIRST {
            if let Some(look) = table.get(&((specular_power - SLOT_FIRST) as u32)) {
                return *look;
            }
        }
        Look::STANDARD
    }
}

/// A water subset among the distant statics at one place, for a frame.
pub struct FarWater {
    pub vertices: wgpu::Buffer,
    pub indexes: wgpu::Buffer,
    pub triangles: u32,
    /// The world matrix of the place, row after row.
    pub world: [f32; 16],
    /// The look of the mesh: its look line, or the standard look (the sky only for a subset
    /// of that kind).
    pub look: Look,
    /// The place is in a cell that the game has loaded: the game draws the mesh near the
    /// camera, and this one is left out there.
    pub loaded: bool,
    /// The base texture of the baked mesh, for a water shader of a mod.
    pub texture: Option<wgpu::TextureView>,
}

/// What the frame says about the look of the surfaces.
pub struct VolumeLook {
    /// How far from the camera a surface reflects what is in the picture
    /// (`distant_land.water.volume_reflection_cells`, in units); 0 for the sky only.
    pub reflect_range: f32,
    /// A colour in place of the sky, and how much of it: the light of the room in an
    /// interior, as the water of the cell has it.
    pub sky: [f32; 4],
    /// The view and the projection of the distant land, for the water among the distant
    /// statics, and the view range of the game.
    pub land_view_projection: [f32; 16],
    pub view_range: f32,
}

/// The light of the frame on the surfaces: each part is on when its setting is on and the
/// frame has it.
pub struct VolumeLight<'a> {
    /// The shadow map of the frame (`shadow.rs`): the atlas, the matrices of its two layers from
    /// the world (the layout of Direct3D), and 1 over its resolution.
    pub shadow: Option<(&'a wgpu::TextureView, [[f32; 16]; 2], f32)>,
    /// The lights of the frame for each pixel.
    pub lights: Option<&'a FrameLights<'a>>,
    /// The strength of the caustics under the surfaces (`water::caustic_strength`); 0 for none.
    pub caustics: f32,
    /// The direction that the light of the sun goes in.
    pub sun_direction: [f32; 3],
}

/// The values of `Lighting` in the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Lighting {
    shadow: [[f32; 16]; 2],
    esm: [f32; 4],
    sun: [f32; 4],
    origin: [f32; 4],
}

/// The values of one surface for the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Surface {
    /// The world matrix of the mesh, in the space of the world of the game.
    world: [f32; 16],
    /// The colour of the water (the emissive colour of the material; black for the usual
    /// colour), and 1 when the surface reflects what is in the picture.
    tint: [f32; 4],
    /// The point that the ripples of a sloped surface turn about: the place of the mesh.
    anchor: [f32; 4],
    /// A colour in place of the sky, and how much of it.
    sky: [f32; 4],
    /// The drift of the ripples in the world (x, y), their speed and their size.
    flow: [f32; 4],
    /// The glow, the opacity and the clarity of the look.
    look: [f32; 3],
    /// Bits: 1 the mesh has normals, 2 it has vertex colours, 4 the vertex colour tints the
    /// water, 8 the vertex alpha is the opacity, 16 the mesh is drawn without the depth test.
    flags: u32,
    /// The view and the projection: those of the game, or those of the distant land for
    /// water among the distant statics.
    clip: [f32; 16],
    /// Water among the distant statics: 1; the view depth before which it is left out (the
    /// game draws the mesh there), and how far it reflects what is in the picture.
    far: [f32; 4],
    /// The free values p0 to p3 of the look, for a water shader.
    params: [[f32; 4]; 4],
}

/// The space of one surface in the buffer of the surfaces: a uniform offset is a multiple
/// of 256 bytes.
const SLOT: u64 = 512;

/// The entry points of a surface, after the shared shading.
const VOLUME: &str = "
struct Surface {
    world: mat4x4f,
    tint: vec4f,
    anchor: vec4f,
    sky: vec4f,
    flow: vec4f,
    look: vec3f,
    flags: u32,
    clip: mat4x4f,
    far: vec4f,
    params: array<vec4f, 4>,
}
@group(3) @binding(0) var<uniform> s: Surface;

// The textures of the mesh, for a water shader: its base texture (white without one), and its
// second texture (a dark, detail or decal map; transparent black without one, and far away),
// with mip levels, wrapped.
@group(2) @binding(0) var mesh0: texture_2d<f32>;
@group(2) @binding(1) var mesh1: texture_2d<f32>;
@group(2) @binding(2) var mesh_sampler: sampler;

// The light of the frame on the surfaces (`VolumeLight`): the shadow map of the frame, the
// lights of the frame with their clusters (`nirender_wgpu::FrameLights`), the caustics.
struct Light {
    diffuse: vec4f,
    ambient: vec4f,
    position: vec4f,
    direction: vec4f,
    attenuation: vec4f,
    cone: vec4f,
}
struct Clusters {
    grid: vec4u,
    screen: vec4f,
    depth: vec4f,
}
struct Lighting {
    // From the world to the two layers of the shadow map.
    shadow0: mat4x4f,
    shadow1: mat4x4f,
    // The constants of the shadow: c, the bias, the scale, and 1 over the resolution.
    esm: vec4f,
    // The direction that the light of the sun goes in; w the strength of the caustics, 0 for
    // none.
    sun: vec4f,
    // The point that the positions of the lights are relative to; w bits: 1 the shadow, 2 the
    // lights.
    origin: vec4f,
}
@group(1) @binding(11) var<uniform> lighting: Lighting;
@group(1) @binding(12) var shadow_atlas: texture_2d<f32>;
@group(1) @binding(13) var<storage, read> lights: array<Light>;
@group(1) @binding(14) var<uniform> clusters: Clusters;
@group(1) @binding(15) var<storage, read> cluster_table: array<vec2u>;
@group(1) @binding(16) var<storage, read> cluster_indexes: array<u32>;

// How much of the sun reaches a place, 0 to 1: the shadow map of the frame, read as the draws
// of the game read it (`extension_shade` of the renderer), so the water has the shadow of the
// shore next to it. All of it outside the map.
fn sunlight_at(place: vec3f) -> f32 {
    if (u32(lighting.origin.w) & 1u) == 0u {
        return 1.0;
    }
    let s0 = lighting.shadow0 * vec4f(place, 1.0);
    let s1 = lighting.shadow1 * vec4f(place, 1.0);
    let p0 = vec3f(s0.xy, s0.z / s0.w);
    let p1 = vec3f(s1.xy, s1.z / s1.w);
    let margin = vec3f(1.0 - 8.0 * lighting.esm.w, 1.0 - 8.0 * lighting.esm.w, 1.0);
    var dz = 1e-6;
    if all(abs(p0) < margin) {
        let uv = vec2f(0.5 + 0.5 * p0.x, 0.5 - 0.5 * p0.y);
        dz = textureSampleLevel(shadow_atlas, clamp_sampler, vec2f(uv.x * 0.5, uv.y), 0.0).r / lighting.esm.z - p0.z;
    } else if all(abs(p1) < margin) {
        let uv = vec2f(0.5 + 0.5 * p1.x, 0.5 - 0.5 * p1.y);
        dz = textureSampleLevel(shadow_atlas, clamp_sampler, vec2f(0.5 + uv.x * 0.5, uv.y), 0.0).r / lighting.esm.z - p1.z;
    }
    if dz >= 0.0 {
        return 1.0;
    }
    // shadowESM, and the fade at the edge of the outer layer.
    let fade = clamp(25.0 * (1.0 - abs(p1.xy)), vec2f(0.0), vec2f(1.0));
    return 1.0 - (1.0 - clamp(exp(lighting.esm.x * dz + lighting.esm.y), 0.0, 1.0)) * fade.x * fade.y;
}

// The light of a point or spot light at a distance: the sum of its falloff (the numbers of
// the game, or those of the light for each pixel, whose middle number is 0), going to zero
// over the last quarter of its reach (`nirender_wgpu::light_reach`), where its clusters end.
fn light_strength(light: Light, distance: f32) -> f32 {
    var reach = light.direction.w;
    if light.cone.x > 0.0 {
        reach = 1.0 / light.cone.x;
    } else if light.cone.z > 0.0 {
        reach = light.cone.z;
    }
    reach = max(min(reach, LIGHT_REACH_MAX), 0.0);
    if distance >= reach {
        return 0.0;
    }
    let a = light.attenuation;
    var fade = clamp(4.0 * distance / reach - 3.0, 0.0, 1.0);
    fade = 1.0 - fade * fade;
    return fade * fade / max(a.x + a.y * distance + a.z * distance * distance, 1e-4);
}

// The lamps and fires of the frame on the surface at a pixel: in `glint` the mirror image of
// each light in the ripples, in `glow` the light that the body of the water takes from them.
// The lights are those of the cluster of the pixel: each light that reaches it.
struct Lit {
    glint: vec3f,
    glow: vec3f,
}

fn point_lights(place: vec3f, eye_vec: vec3f, normal: vec3f, face: vec3f, pixel: vec2f, view_z: f32) -> Lit {
    var lit: Lit;
    if (u32(lighting.origin.w) & 2u) == 0u || clusters.grid.w == 0u {
        return lit;
    }
    let tile = vec2u(clamp(vec2i(pixel * clusters.screen.xy), vec2i(0), vec2i(clusters.grid.xy) - 1));
    let slice = u32(clamp(log(max(view_z, clusters.depth.z)) * clusters.depth.x + clusters.depth.y, 0.0, f32(clusters.grid.z) - 1.0));
    let cell = cluster_table[(slice * clusters.grid.y + tile.y) * clusters.grid.x + tile.x];
    for (var i = 0u; i < cell.y; i++) {
        let light = lights[cluster_indexes[cell.x + i]];
        // The sun has its own glint.
        if light.position.w == 3.0 {
            continue;
        }
        let to_light = light.position.xyz + lighting.origin.xyz - place;
        let distance = length(to_light);
        let direction = to_light / max(distance, 1e-3);
        let color = light.diffuse.rgb * light_strength(light, distance);
        let mirrored = clamp(dot(-eye_vec, reflect(-direction, normal)), 0.0, 1.0);
        lit.glint += color * (pow(mirrored, 90.0) + 0.06 * pow(mirrored, 6.0));
        lit.glow += color * clamp(dot(direction, face), 0.0, 1.0);
    }
    return lit;
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) world: vec3f,
    @location(1) facing: vec3f,
    @location(2) clip_w: f32,
    @location(3) color: vec4f,
    // The colour of the water.
    @location(4) tint: vec3f,
    // The texture coordinates of the mesh: the first set, and the second (the first again
    // when the mesh has one).
    @location(5) uv: vec4f,
}

// The normal, the colour and the coordinates of a vertex that has none come from a buffer of
// zeros.
@vertex fn vs(@location(0) local: vec3f, @location(1) normal: vec3f, @location(2) color: vec4f, @location(3) uv0: vec2f, @location(4) uv1: vec2f) -> Out {
    var out: Out;
    let bits = s.flags;
    out.world = (s.world * vec4f(local, 1.0)).xyz;
    // A mesh without normals faces up.
    out.facing = select(vec3f(0.0, 0.0, 1.0), (s.world * vec4f(normal, 0.0)).xyz, (bits & 1u) != 0u);
    // A D3DCOLOR has blue in its first byte.
    out.color = select(vec4f(1.0), color.zyxw, (bits & 2u) != 0u);
    out.tint = s.tint.rgb;
    out.uv = vec4f(uv0, uv1);
    out.position = s.clip * vec4f(out.world, 1.0);
    out.clip_w = out.position.w;
    return out;
}

// Water among the distant statics (`WaterVolumeDistantVS`): the vertex of a distant static.
// The generator writes the colour of the water into the vertex colour, and the opacity of the
// mesh's vertex into its alpha when the look asks for it.
@vertex fn vs_far(@location(0) position: vec4f, @location(1) normal: vec4f, @location(2) color: vec4f, @location(3) uv: vec2f) -> Out {
    var out: Out;
    out.world = (s.world * vec4f(position.xyz, 1.0)).xyz;
    out.facing = (s.world * vec4f(normal.xyz * 2.0 - 1.0, 0.0)).xyz;
    out.color = vec4f(1.0, 1.0, 1.0, color.w);
    out.tint = color.zyx;
    out.uv = vec4f(uv, uv);
    out.position = s.clip * vec4f(out.world, 1.0);
    out.clip_w = out.position.w;
    return out;
}

// The depth of a surface only, for the count of the dry spaces behind it.
@fragment fn fs_depth() -> @location(0) vec4f {
    return vec4f(0.0);
}

// The surface at a pixel, for a water shader (`docs/water-shaders.md`).
struct WaterSurface {
    // The place in the world.
    world: vec3f,
    // The way the mesh faces, length 1 (up for a mesh without normals).
    facing: vec3f,
    // The colour of the water: the emissive colour of the material; black for the usual.
    tint: vec3f,
    // The vertex colour of the mesh (white for a mesh without one; from the distant land,
    // white with the alpha of the mesh when the look has `opacity=vertex`).
    color: vec4f,
    // The texture coordinates of the mesh: the first set in xy, the second in zw.
    uv: vec4f,
    // The place in the plane of the ripples: that of the sea for a level surface.
    flat: vec3f,
    // The place in the picture (0 to 1), the pixel, and the view depth.
    screen: vec2f,
    pixel: vec2f,
    view_depth: f32,
}

// The look of a surface (`MGE_WaterLookSet`, the look line of a mesh).
struct WaterLook {
    // The drift of the ripples in the world, in units for each second; their speed and their
    // size (1 is the standard).
    drift: vec2f,
    speed: f32,
    scale: f32,
    glow: f32,
    opacity: f32,
    // Over how many units what is under the surface fades into the colour of deep water.
    clarity: f32,
    // The tint of the vertex colour, when the look takes it.
    tint: vec3f,
    reflects_scene: bool,
    distant: bool,
    no_depth_test: bool,
    // The free values of the look.
    p0: vec4f,
    p1: vec4f,
    p2: vec4f,
    p3: vec4f,
}

// The result of the standard shading, before the glow and the opacity.
struct WaterShade {
    colour: vec3f,
    eye_vec: vec3f,
    dist: f32,
    // rgb what the fog adds, a what is left of the colour of the thing.
    fog: vec4f,
    // The normal of the ripples as on a level surface, and the way the surface faces (to the
    // side of the eye).
    ripple: vec3f,
    face: vec3f,
    // How far the view ray goes through the water to what is behind the surface, and how
    // deep that is under the surface; 4000 or more where the water is deep.
    ray_depth: f32,
    water_depth: f32,
    // How much of the sun reaches the point, 0 to 1.
    sun: f32,
}

fn surface_look(surface: WaterSurface) -> WaterLook {
    var look: WaterLook;
    let bits = s.flags;
    look.drift = s.flow.xy;
    look.speed = s.flow.z;
    look.scale = s.flow.w;
    look.glow = s.look.x;
    look.opacity = s.look.y * select(1.0, surface.color.a, (bits & 8u) != 0u);
    look.clarity = max(s.look.z, 1.0);
    look.tint = select(vec3f(1.0), surface.color.rgb, (bits & 4u) != 0u);
    look.reflects_scene = s.tint.w > 0.5;
    look.distant = s.far.x > 0.5;
    look.no_depth_test = (bits & 16u) != 0u;
    look.p0 = s.params[0];
    look.p1 = s.params[1];
    look.p2 = s.params[2];
    look.p3 = s.params[3];
    return look;
}

// The time of the water, in seconds.
fn water_time() -> f32 {
    return water.time.x;
}

// A colour with the fog of the shade on it.
fn fog_apply(colour: vec3f, fog: vec4f) -> vec3f {
    return fog.a * colour + fog.rgb;
}

// The colour of the water: a water blue for the usual colour.
fn glow_colour(tint: vec3f) -> vec3f {
    return select(vec3f(0.25, 0.5, 0.6), tint, dot(tint, vec3f(1.0)) > 0.0);
}

// The ripples of the sea with the drift, the speed and the size of the look
// (`surfaceRipples`): the normal as on a level surface, z up. The drift moves the
// coordinates; it is wrapped, so it keeps its precision over hours.
fn standard_ripple(surface: WaterSurface, look: WaterLook) -> vec3f {
    let texcoords = vec4f(surface.flat.xy / 3900.0, surface.flat.xy / 527.0);
    let distance = length(surface.world - u.eye.xyz);
    let t = 0.4 * water.time.x * look.speed;
    let moved = look.drift * water.time.x;
    let scale = max(look.scale, 1e-3);
    let far = textureSample(waves, wave_sampler, vec3f(texcoords.xy / scale + fract(moved / 3900.0), t)).rg;
    var close = textureSample(waves, wave_sampler, vec3f(texcoords.zw / scale + fract(moved / 527.0), t)).rg;
    if water.wave.w > 0.5 {
        close += textureSampleLevel(rain, wave_sampler, texcoords.zw, 0.0).ba;
        let at = (surface.flat.xy - water.wave.xy) / 1280.0;
        if all(at >= vec2f(0.0)) && all(at <= vec2f(1.0)) {
            close += textureSampleLevel(player_waves, clamp_sampler, at, 0.0).ba;
        }
    }
    let n = 2.0 * mix(close, far, clamp(distance / 8000.0, 0.0, 1.0)) - 1.0;
    return normalize(vec3f(n, 1.0));
}

// The standard shading, with the standard ripples.
fn shade_water(surface: WaterSurface, look: WaterLook) -> WaterShade {
    return shade_water_rippled(surface, look, standard_ripple(surface, look));
}

// The standard shading up to the look, with the ripples given: `ripple` is the normal of the
// ripples as on a level surface, z up, of length 1.
fn shade_water_rippled(surface: WaterSurface, look: WaterLook, ripple: vec3f) -> WaterShade {
    var eye_vec = surface.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    let uv = surface.screen;
    let w = surface.view_depth;
    // The tests of the branches are of uniforms only: the caustics take a change from pixel
    // to pixel.
    let far = s.far.x > 0.5;
    let no_depth = (s.flags & 16u) != 0u;
    // The way the surface faces, turned to the side of the eye; the ripples tilted to it.
    let face = select(surface.facing, -surface.facing, dot(surface.facing, eye_vec) > 0.0);
    let fog = fog_water(eye_vec, dist);
    let normal = tilt_to(face, ripple);
    let tint = surface.tint;
    // In the shadow of a thing the water has no glint of the sun, and its depth has no share
    // of the sun.
    let sun = sunlight_at(surface.world);
    let deep = fog.a * depth_base_sun(tint, sun) * look.tint + fog.rgb;
    let reffactor = (water.sun_col.w * dist + 0.1) * ripple.xy;
    // The caustics on what is under a surface that faces up, where the sun reaches it: the
    // point under the surface is that of the depth along the ray of the eye.
    // A surface drawn without the depth test (the water of a well, under the ground) has no
    // depth of what is under it: it shows deep water.
    var bed_light = 1.0;
    if lighting.sun.w > 0.0 && !no_depth {
        let first = max(0.0, ground_at(uv + reffactor.yx / w, far) - w);
        let ray = first / max(dot(eye_vec, vec3f(water.view[0].z, water.view[1].z, water.view[2].z)), 1e-3);
        let bed = surface.world + eye_vec * ray;
        let under = vec3f(bed.xy, bed.z - surface.world.z);
        bed_light = 1.0 + caustic_light(under, lighting.sun.xyz, water.time.x, dist + ray, lighting.sun.w) * sun * clamp(4.0 * face.z - 3.0, 0.0, 1.0);
    }
    var below = Refraction(deep, 0.0, 4000.0);
    if !no_depth {
        below = refraction(uv, w, reffactor, eye_vec, ripple, deep, tint, look.clarity, look.tint, far, bed_light);
    }
    // Far away and at a flat angle the surface is calm: a ripple is smaller than a pixel.
    let calm = (1.0 + dot(eye_vec, face)) * (1.0 - clamp(1.0 / (dist / 1000.0 + 1.0), 0.0, 1.0));
    var adjust = mix(0.1 * face, normal, pow(clamp(1.05 * fog.a, 0.0, 1.0), 2.0));
    adjust = mix(adjust, face, calm);
    // The ripples tilt the reflected direction only a little; a direction into the surface
    // is turned back out of it.
    var reflect_dir = reflect(eye_vec, normalize(mix(face, normal, 0.35 * (1.0 - calm))));
    reflect_dir -= 2.0 * min(0.0, dot(reflect_dir, face)) * face;
    reflect_dir = normalize(reflect_dir);
    var reflected = mix(sky_color(reflect_dir), s.sky.rgb, s.sky.a);
    // Far water reflects what is in the picture out to its range, and fades out over the
    // last quarter of a cell.
    let scene_weight = select(1.0, clamp((s.far.z - dist) / 2048.0, 0.0, 1.0), far);
    if s.tint.w > 0.5 && scene_weight > 0.0 {
        let seen = reflect_scene(surface.world, reflect_dir, far);
        reflected = mix(reflected, seen.rgb, seen.a * scene_weight);
    }
    reflected = mix(fog.rgb, reflected, fog.a);
    let fresnel = 0.02 + pow(clamp(0.9988 - 0.28 * dot(-eye_vec, adjust), 0.0, 1.0), 16.0);
    var result = mix(below.color, reflected, fresnel);
    result += sun_glint(eye_vec, normal, fog.a) * sun;
    // The lamps and fires near the surface: their glint, and a little of their light in the
    // body of the water. Not for far water: the lights of the frame are near the camera.
    if !far {
        let lit = point_lights(surface.world, eye_vec, normal, face, surface.pixel, w);
        result += (lit.glint + 0.08 * lit.glow * glow_colour(tint)) * fog.a;
    }
    result = mix(result, below.color, below.shore * fog.a);
    var shade: WaterShade;
    shade.colour = result;
    shade.eye_vec = eye_vec;
    shade.dist = dist;
    shade.fog = fog;
    shade.ripple = ripple;
    shade.face = face;
    // From the difference of the view depths to the way along the ray.
    shade.ray_depth = below.depth_first / max(dot(eye_vec, vec3f(water.view[0].z, water.view[1].z, water.view[2].z)), 1e-3);
    shade.water_depth = shade.ray_depth * abs(dot(eye_vec, face));
    shade.sun = sun;
    return shade;
}

// The last steps of the shading, which belong to the look: the light of the surface's own,
// in the colour of the water, and what is behind a surface that is not all water.
fn finish_water(surface: WaterSurface, shade: WaterShade, look: WaterLook) -> vec4f {
    var result = shade.colour + look.glow * glow_colour(surface.tint) * shade.fog.a;
    result = mix(textureSampleLevel(picture, clamp_sampler, surface.screen, 0.0).rgb, result, look.opacity);
    return vec4f(result, 1.0);
}

@fragment fn fs(in: Out) -> @location(0) vec4f {
    var eye_vec = in.world - u.eye.xyz;
    let dist = length(eye_vec);
    eye_vec /= dist;
    let uv = in.position.xy / water.size.xy;
    let w = in.clip_w;
    // Water among the distant statics has no depth test of the pass: its depth is that of the
    // distant land, and the game's is another. It is behind what the game drew and behind the
    // distant land, and not nearer than the view range of the game for a mesh that the game
    // draws itself.
    let far = s.far.x > 0.5;
    if far && (w < s.far.y || w > distance_at(uv) || w > land_distance_at(uv) * 1.0005 + 1.0) {
        discard;
    }
    var facing = vec3f(0.0, 0.0, 1.0);
    if dot(in.facing, in.facing) > 0.25 {
        facing = normalize(in.facing);
    }
    // The ripples lie in the plane of the surface: the place, turned about the anchor of the
    // mesh by the turn that takes the surface up. A level surface has the coordinates of the
    // sea, so the two agree at a seam.
    let upward = select(facing, -facing, facing.z < 0.0);
    let flat = tilt_to(vec3f(-upward.x, -upward.y, upward.z), in.world - s.anchor.xyz) + s.anchor.xyz;
    // From under the water each surface has the look from below, as the plane has it.
    if water.modes.y > 0.5 {
        return underwater(uv, w, vec4f(flat.xy / 3900.0, flat.xy / 527.0), flat.xy, eye_vec, dist);
    }
    var surface: WaterSurface;
    surface.world = in.world;
    surface.facing = facing;
    surface.tint = in.tint;
    surface.color = in.color;
    surface.uv = in.uv;
    surface.flat = flat;
    surface.screen = uv;
    surface.pixel = in.position.xy;
    surface.view_depth = w;
    return water_shader(surface);
}
";

/// The standard look: the standard shading and nothing more (`XE Mod Water Standard.fx`).
const STANDARD: &str = "
fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);
    return finish_water(surface, shade_water(surface, look), look);
}
";

/// The shader of the surfaces with the water shader of the look: the shared shading, the
/// entry points of a surface, and the function `water_shader`.
fn shader_with(water: &str) -> String {
    format!("{}{}{}{}{}", super::super::view::wgsl(), shading::COMMON, shading::CAUSTICS, VOLUME, water).replace("LIGHT_REACH_MAX", &format!("{:?}", nirender_wgpu::LIGHT_REACH_MAX))
}

/// The shader of the surfaces with the standard look.
fn shader() -> String {
    shader_with(STANDARD)
}

/// The source of a water shader of a mod (`Data Files/shaders/water/<name>.wgsl`), checked as
/// the device checks it. The file starts with the line `//! interface: mge-xe-water-wgsl 1`;
/// the rest is WGSL with the function `water_shader` (`docs/water-shaders.md`).
pub fn water_shader_source(file: &str) -> Result<String, String> {
    let mut interface = false;
    for line in file.lines() {
        let Some(rest) = line.strip_prefix("//!") else { break };
        let Some((key, value)) = rest.split_once(':') else { return Err(format!("the line `{line}` has no `key: value`")) };
        match key.trim() {
            "interface" => interface = value.trim() == "mge-xe-water-wgsl 1",
            other => return Err(format!("the key `{other}` is not known")),
        }
    }
    if !interface {
        return Err("no line `//! interface: mge-xe-water-wgsl 1` at its start".to_string());
    }
    let source = shader_with(file);
    use wgpu::naga;
    let module = naga::front::wgsl::parse_str(&source).map_err(|error| error.emit_to_string(&source))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).map_err(|error| error.emit_to_string(&source))?;
    Ok(source)
}

/// What makes the pipelines of two surfaces differ: the vertex layout, the topology and the
/// fill.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    stride: u64,
    position: u64,
    normal: Option<u64>,
    color: Option<u64>,
    topology: wgpu::PrimitiveTopology,
    lines: bool,
    /// The vertex of a distant static, and the depth tests in the shader.
    far: bool,
    /// Drawn only outside the dry spaces (`dry::outside`).
    cut: bool,
    /// The depth only, for the count of the dry spaces behind it.
    depth_only: bool,
    /// The mesh is drawn with the depth test (and writes its depth); without it, it is drawn
    /// whatever the depth is, as it asks.
    depth_test: bool,
    /// The stencil test of the mesh, when it has one: the function, the mask, and the
    /// operations on a failed test, a failed depth test and a pass (Direct3D values). The
    /// reference is set for each draw.
    stencil: Option<[u32; 5]>,
    /// The offsets of the two sets of texture coordinates (the first again for a mesh with
    /// one set); none without coordinates.
    coordinates: [Option<u64>; 2],
    /// The water shader: 0 the standard one, else one of a mod (`Volumes::modules`).
    shader: u16,
}

/// The tests that a held shape asks for: the depth test, and its own stencil test (see `Key`).
fn tests_of(held: &HeldDraw) -> (bool, Option<[u32; 5]>) {
    // D3DCMP_ALWAYS is 8: the game turns the depth test off that way too.
    let depth_test = held.depth[0] != 0 && held.depth[2] != 8;
    let s = held.stencil;
    (depth_test, (s[0] != 0).then_some([s[1], s[3], s[4], s[5], s[6]]))
}

pub(super) struct Volumes {
    /// The shader with each water shader: the standard one first, then those of mods, by
    /// their names in `shaders`. A name whose file is missing or fails has the standard one.
    modules: Vec<wgpu::ShaderModule>,
    shaders: HashMap<String, u16>,
    /// The group of the textures of the mesh, its sampler, and a group of white textures for
    /// the standard look.
    mesh_layout: wgpu::BindGroupLayout,
    mesh_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    clear: wgpu::TextureView,
    plain_mesh: wgpu::BindGroup,
    /// The mesh groups of the water shaders of mods in this frame, by their two textures.
    mesh_groups: HashMap<(wgpu::TextureView, wgpu::TextureView), wgpu::BindGroup>,
    pipeline_layout: wgpu::PipelineLayout,
    surface_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    /// The values of the surfaces of a frame, one slot each, and their group.
    surfaces: wgpu::Buffer,
    capacity: u64,
    /// The slots that the draws of this frame have used: a frame can draw surfaces more than
    /// once (`draw_held`, and a surface in its place), and each draw writes its own slots, since
    /// the writes come before the commands of the frame.
    used: u64,
    group: wgpu::BindGroup,
    /// Zeros for the inputs that a vertex does not have.
    defaults: wgpu::Buffer,
    /// The light of the frame (`VolumeLight`) in the water group: its values, and what the
    /// group has in place of a part that the frame does not have: a texture of one texel, and
    /// empty lights and clusters.
    lighting: wgpu::Buffer,
    no_shadow: wgpu::TextureView,
    no_lights: [wgpu::Buffer; 4],
    target_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    /// The near surfaces and the far water drawn since the start.
    pub drawn: u64,
    pub far_drawn: u64,
}

impl Volumes {
    /// `view_layout` is that of the view group (`view.rs`), `frame_layout` that of the group
    /// of the shared shading (`Water::layout`, with `light_layout_entries`).
    pub fn new(scene: &SceneView, view_layout: &wgpu::BindGroupLayout, frame_layout: &wgpu::BindGroupLayout) -> Volumes {
        let device = scene.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water volume"), source: wgpu::ShaderSource::Wgsl(shader().into()) });
        let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water volume mesh"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let mesh_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water volume mesh"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water volume"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(size_of::<Surface>() as u64) },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water volume"),
            bind_group_layouts: &[Some(view_layout), Some(frame_layout), Some(&mesh_layout), Some(&surface_layout)],
            immediate_size: 0,
        });
        let lighting = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume light"), size: size_of::<Lighting>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        use wgpu::util::DeviceExt;
        let no_shadow = device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some("water volume no shadow"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &[0u8; 2],
            )
            .create_view(&Default::default());
        // A light of zeros, clusters that are off, one empty cluster and one index.
        let zeros = |label, size: usize, usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &vec![0u8; size], usage });
        let no_lights = [
            zeros("water volume no lights", size_of::<nirender_wgpu::LightData>(), wgpu::BufferUsages::STORAGE),
            zeros("water volume no clusters", 48, wgpu::BufferUsages::UNIFORM),
            zeros("water volume no cluster table", 8, wgpu::BufferUsages::STORAGE),
            zeros("water volume no cluster lights", 4, wgpu::BufferUsages::STORAGE),
        ];
        let one_texel = |label, texel: [u8; 4]| device
            .create_texture_with_data(
                scene.queue,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &texel,
            )
            .create_view(&Default::default());
        let white = one_texel("water volume white", [255; 4]);
        let clear = one_texel("water volume clear", [0; 4]);
        let plain_mesh = Self::mesh_group_of(device, &mesh_layout, &mesh_sampler, &white, &clear);
        let (surfaces, group) = Self::slots(device, &surface_layout, 16);
        let defaults = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume defaults"), size: 32, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false });
        Volumes {
            modules: vec![module],
            shaders: HashMap::new(),
            mesh_layout,
            mesh_sampler,
            white,
            clear,
            plain_mesh,
            mesh_groups: HashMap::new(),
            pipeline_layout,
            surface_layout,
            pipelines: HashMap::new(),
            surfaces,
            capacity: 16,
            used: 0,
            group,
            defaults,
            lighting,
            no_shadow,
            no_lights,
            target_format: scene.target_format,
            depth_format: scene.depth_format,
            drawn: 0,
            far_drawn: 0,
        }
    }

    /// The entries of the light of the frame in the layout of the water group: bindings 11 to
    /// 16, for the fragments.
    pub fn light_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
        let uniform = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None };
        let storage = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None };
        [
            entry(11, uniform),
            entry(12, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
            entry(13, storage),
            entry(14, uniform),
            entry(15, storage),
            entry(16, storage),
        ]
    }

    /// The entries of the light of the frame in the water group (`light_layout_entries`).
    /// With a light, its values are written; without one (the plane, which reads none of it),
    /// the group has the texture of one texel and the empty lights.
    pub fn light_entries<'a>(&'a self, scene: &SceneView, light: Option<&VolumeLight<'a>>) -> [wgpu::BindGroupEntry<'a>; 6] {
        let [no_lights, no_clusters, no_table, no_indexes] = &self.no_lights;
        let (atlas, buffers) = match light {
            Some(light) => {
                self.write_light(scene, light);
                (light.shadow.map_or(&self.no_shadow, |(atlas, ..)| atlas), light.lights.map_or([no_lights, no_clusters, no_table, no_indexes], |l| [l.lights, l.clusters, l.table, l.indexes]))
            }
            None => (&self.no_shadow, [no_lights, no_clusters, no_table, no_indexes]),
        };
        [
            wgpu::BindGroupEntry { binding: 11, resource: self.lighting.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::TextureView(atlas) },
            wgpu::BindGroupEntry { binding: 13, resource: buffers[0].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 14, resource: buffers[1].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 15, resource: buffers[2].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 16, resource: buffers[3].as_entire_binding() },
        ]
    }

    /// Writes the values of the light of the frame.
    fn write_light(&self, scene: &SceneView, light: &VolumeLight) {
        let mut values = <Lighting as bytemuck::Zeroable>::zeroed();
        let mut bits = 0;
        if let Some((_, layers, rcp)) = light.shadow {
            values.shadow = layers;
            values.esm = [super::super::shadow::ESM_C, super::super::shadow::ESM_BIAS, super::super::shadow::ESM_SCALE, rcp];
            bits |= 1;
        }
        if let Some(lights) = light.lights {
            values.origin = [lights.origin[0] as f32, lights.origin[1] as f32, lights.origin[2] as f32, 0.0];
            bits |= 2;
        }
        values.origin[3] = bits as f32;
        let d = light.sun_direction;
        values.sun = [d[0], d[1], d[2], light.caustics];
        scene.queue.write_buffer(&self.lighting, 0, bytemuck::bytes_of(&values));
    }

    /// The start of a frame: no slots are used.
    pub fn start_frame(&mut self) {
        self.used = 0;
        self.mesh_groups.clear();
    }

    /// True when the water shader of this name has been asked for before.
    pub fn knows_shader(&self, name: &str) -> bool {
        self.shaders.contains_key(name)
    }

    /// Adds the water shader of a mod by its name, from its file (`None`: there is none). A
    /// shader that fails has the standard look, and the reason is returned.
    pub fn add_shader(&mut self, device: &wgpu::Device, name: &str, file: Option<&str>) -> Result<(), String> {
        let result = file.ok_or_else(|| "no file shaders/water/<name>.wgsl".to_string()).and_then(water_shader_source);
        let index = match &result {
            Ok(source) => {
                let label = format!("water shader {name}");
                self.modules.push(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&label), source: wgpu::ShaderSource::Wgsl(source.as_str().into()) }));
                (self.modules.len() - 1) as u16
            }
            Err(_) => 0,
        };
        self.shaders.insert(name.to_string(), index);
        result.map(|_| ())
    }

    /// The water shader of a look.
    fn shader_of(&self, look: &Look) -> u16 {
        look.shader_name().and_then(|name| self.shaders.get(&name).copied()).unwrap_or(0)
    }

    fn mesh_group_of(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, base: &wgpu::TextureView, second: &wgpu::TextureView) -> wgpu::BindGroup {
        nirender_wgpu::gpu::frame_group(device, &wgpu::BindGroupDescriptor {
            label: Some("water volume mesh"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(base) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(second) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    /// The textures of a held mesh for a water shader: its base map, and the first of its
    /// other maps (a dark, detail or decal map).
    fn mesh_group(&mut self, device: &wgpu::Device, held: &HeldDraw) -> wgpu::BindGroup {
        let base = held.maps.iter().find(|m| m.map == nirender_wgpu::material::BASE).map_or(&self.white, |m| &*m.texture).clone();
        let second = held.maps.iter().filter(|m| m.map != nirender_wgpu::material::BASE).min_by_key(|m| m.map).map_or(&self.clear, |m| &*m.texture).clone();
        self.cached_mesh_group(device, base, second)
    }

    /// The mesh group of two textures, made once in a frame.
    fn cached_mesh_group(&mut self, device: &wgpu::Device, base: wgpu::TextureView, second: wgpu::TextureView) -> wgpu::BindGroup {
        let (layout, sampler) = (&self.mesh_layout, &self.mesh_sampler);
        self.mesh_groups.entry((base, second)).or_insert_with_key(|(base, second)| Self::mesh_group_of(device, layout, sampler, base, second)).clone()
    }

    /// A buffer of `count` slots and its group.
    fn slots(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, count: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("water volume surfaces"), size: count * SLOT, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water volume surfaces"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(size_of::<Surface>() as u64) }),
            }],
        });
        (buffer, group)
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: Key) -> &wgpu::RenderPipeline {
        let (module, layout, target_format, depth_format) = (&self.modules[key.shader as usize], &self.pipeline_layout, self.target_format, self.depth_format);
        self.pipelines.entry(key).or_insert_with(|| {
            if key.far {
                // A distant static: the position with the palette entry, the normal with the
                // light of the material, the colour, the coordinate (statics.rs).
                let attributes = wgpu::vertex_attr_array![0 => Float16x4, 1 => Unorm8x4, 2 => Unorm8x4, 3 => Float16x2];
                return device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("water volume, far"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_far"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        polygon_mode: if key.lines { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: depth_format,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState { format: target_format, blend: None, write_mask: wgpu::ColorWrites::COLOR })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            }
            let mut attributes = vec![wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: key.position, shader_location: 0 }];
            let mut missing = Vec::new();
            match key.normal {
                Some(normal) => attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: normal, shader_location: 1 }),
                None => missing.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 1 }),
            }
            match key.color {
                Some(color) => attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: color, shader_location: 2 }),
                None => missing.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 16, shader_location: 2 }),
            }
            for (set, location) in key.coordinates.iter().zip([3, 4]) {
                match set {
                    Some(offset) => attributes.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: *offset, shader_location: location }),
                    None => missing.push(wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: location }),
                }
            }
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water volume"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout { array_stride: key.stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }),
                        Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &missing }),
                    ],
                },
                // Both sides: a surface is seen from under the water too.
                primitive: wgpu::PrimitiveState {
                    topology: key.topology,
                    cull_mode: None,
                    polygon_mode: if key.lines { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                    ..Default::default()
                },
                // A mesh drawn whatever the depth is writes no depth: it can lie under the
                // ground, as the water of a well that only its mask shows.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(key.depth_test),
                    depth_compare: Some(if key.depth_test { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always }),
                    stencil: match key.stencil {
                        // The test of the mesh, without the bit of the dry spaces.
                        Some([function, mask, fail, depth_fail, pass]) => {
                            let face = wgpu::StencilFaceState {
                                compare: nirender_wgpu::d3d_compare(function),
                                fail_op: nirender_wgpu::d3d_stencil_operation(fail),
                                depth_fail_op: nirender_wgpu::d3d_stencil_operation(depth_fail),
                                pass_op: nirender_wgpu::d3d_stencil_operation(pass),
                            };
                            let others = 0xFF & !super::dry::DRY_BIT;
                            wgpu::StencilState { front: face, back: face, read_mask: mask & others, write_mask: others }
                        }
                        None if key.cut => super::dry::outside(),
                        None => Default::default(),
                    },
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(if key.depth_only { "fs_depth" } else { "fs" }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: if key.depth_only { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::COLOR },
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Draws the water among the distant statics, then the marked surfaces among `held`,
    /// with the group of the shared shading of the frame. The far water writes no depth and
    /// the near surfaces do: they go over it. Returns the number drawn.
    /// With `dry`, the dry spaces cut the near surfaces: they write their depth first, the
    /// faces of the dry spaces behind it are counted, and then they are drawn outside
    /// (`flushWaterVolumes`). The far water is not cut.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, scene: &SceneView, pass: &mut wgpu::RenderPass<'static>, view: &wgpu::BindGroup, frame_group: &wgpu::BindGroup, held: &[HeldDraw], far: &[FarWater], frame_look: &VolumeLook, looks: &HashMap<u32, Look>, dry: Option<&super::dry::Dry>) -> usize {
        let surfaces: Vec<&HeldDraw> = held.iter().filter(|h| marked(h.facts.specular_power)).collect();
        if surfaces.is_empty() && far.is_empty() {
            return 0;
        }
        let count = (far.len() + surfaces.len()) as u64;
        if self.used + count > self.capacity {
            // A new buffer: the draws before in the frame keep the old one.
            self.capacity = (self.used + count).next_power_of_two();
            (self.surfaces, self.group) = Self::slots(scene.device, &self.surface_layout, self.capacity);
            self.used = 0;
        }
        let base = self.used;
        self.used += count;
        let mut bytes = vec![0u8; (count * SLOT) as usize];
        let game_view_projection = crate::renderer::multiply(&scene.view, &scene.projection);
        for (index, water) in far.iter().enumerate() {
            let look = &water.look;
            let world = water.world;
            let drift = [look.flow[0] * world[0] + look.flow[1] * world[4], look.flow[0] * world[1] + look.flow[1] * world[5]];
            // The alpha of the vertex is the opacity when the look asks for it; its colour is
            // the colour of the water.
            let bits = 1 | 2 | (((look.flags & LOOK_OPACITY_FROM_VERTEX != 0) as u32) << 3);
            let surface = Surface {
                world,
                tint: [0.0, 0.0, 0.0, if look.flags & LOOK_REFLECTS_SCENE != 0 && frame_look.reflect_range > 0.0 { 1.0 } else { 0.0 }],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: if look.flags & LOOK_HAS_SKY != 0 { [look.sky[0], look.sky[1], look.sky[2], 1.0] } else { frame_look.sky },
                flow: [drift[0], drift[1], look.speed, look.scale],
                look: [look.glow, look.opacity, look.clarity],
                flags: bits,
                clip: frame_look.land_view_projection,
                far: [1.0, if water.loaded { frame_look.view_range } else { 0.0 }, frame_look.reflect_range, 0.0],
                params: look.params,
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        for (index, held) in surfaces.iter().enumerate() {
            let index = far.len() + index;
            // The world of a draw can be relative to an origin; the water is in the space of
            // the world of the game.
            let mut world = held.data.world[0];
            if let Some(origin) = held.origin {
                for axis in 0..3 {
                    world[12 + axis] = (world[12 + axis] as f64 + origin[axis]) as f32;
                }
            }
            let look = Look::of(held.facts.specular_power, looks);
            let reflects = look.flags & LOOK_REFLECTS_SCENE != 0 && frame_look.reflect_range >= 1.0;
            let emissive = held.facts.emissive;
            // The drift is given in the axes of the mesh: turned into the world.
            let drift = [look.flow[0] * world[0] + look.flow[1] * world[4], look.flow[0] * world[1] + look.flow[1] * world[5]];
            let has_color = held.layout.color.is_some();
            let bits = held.layout.normal.is_some() as u32
                | (has_color as u32) << 1
                | ((has_color && look.flags & LOOK_TINT_FROM_VERTEX != 0) as u32) << 2
                | ((has_color && look.flags & LOOK_OPACITY_FROM_VERTEX != 0) as u32) << 3
                | (!tests_of(held).0 as u32) << 4;
            let surface = Surface {
                world,
                tint: [emissive[0], emissive[1], emissive[2], if reflects { 1.0 } else { 0.0 }],
                anchor: [world[12], world[13], world[14], 0.0],
                sky: if look.flags & LOOK_HAS_SKY != 0 { [look.sky[0], look.sky[1], look.sky[2], 1.0] } else { frame_look.sky },
                flow: [drift[0], drift[1], look.speed, look.scale],
                look: [look.glow, look.opacity, look.clarity],
                flags: bits,
                clip: game_view_projection,
                far: [0.0; 4],
                params: look.params,
            };
            let at = index * SLOT as usize;
            bytes[at..at + size_of::<Surface>()].copy_from_slice(bytemuck::bytes_of(&surface));
        }
        scene.queue.write_buffer(&self.surfaces, base * SLOT, &bytes);
        let lines = scene.wireframe && scene.device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        // The view and the water group with the light of the frame: the same for all surfaces
        // and for the dry spaces, whose pipelines have these two groups.
        pass.set_bind_group(0, view, &[]);
        pass.set_bind_group(1, frame_group, &[]);
        if !far.is_empty() {
            for (index, water) in far.iter().enumerate() {
                let shader = self.shader_of(&water.look);
                let key = Key { stride: 20, position: 0, normal: None, color: None, topology: wgpu::PrimitiveTopology::TriangleList, lines, far: true, cut: false, depth_only: false, depth_test: true, stencil: None, coordinates: [None; 2], shader };
                pass.set_pipeline(self.pipeline(scene.device, key));
                // A water shader of a mod has the base texture of the baked mesh.
                let mesh = match (&water.texture, shader) {
                    (Some(texture), 1..) => self.cached_mesh_group(scene.device, texture.clone(), self.clear.clone()),
                    _ => self.plain_mesh.clone(),
                };
                pass.set_bind_group(2, &mesh, &[]);
                pass.set_bind_group(3, &self.group, &[((base + index as u64) * SLOT) as u32]);
                pass.set_vertex_buffer(0, water.vertices.slice(..));
                pass.set_index_buffer(water.indexes.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..water.triangles * 3, 0, 0..1);
            }
            self.far_drawn += far.len() as u64;
        }
        let dry = dry.filter(|_| !surfaces.is_empty());
        // The passes over the near surfaces: with the dry spaces, the depth, then the count,
        // then the surfaces outside them.
        let passes: &[(bool, bool)] = if dry.is_some() { &[(true, false), (false, true)] } else { &[(false, false)] };
        for &(depth_only, cut) in passes {
            if cut {
                if let Some(dry) = dry {
                    dry.zero(pass);
                    dry.count_behind(pass);
                }
            }
            for (index, held) in surfaces.iter().enumerate() {
                let index = far.len() + index;
                let layout = &held.layout;
                // A surface with a stencil test of its own is not cut by the dry spaces, and one
                // without the depth test has no depth to count behind.
                let (depth_test, stencil) = tests_of(held);
                if depth_only && (stencil.is_some() || !depth_test) {
                    continue;
                }
                let cut = cut && stencil.is_none();
                let shader = if depth_only { 0 } else { self.shader_of(&Look::of(held.facts.specular_power, looks)) };
                let first = layout.coordinates.first().copied();
                let coordinates = [first, layout.coordinates.get(1).copied().or(first)];
                let key = Key { stride: layout.stride, position: layout.position, normal: layout.normal, color: layout.color, topology: held.topology, lines: lines && !cut && !depth_only, far: false, cut, depth_only, depth_test, stencil, coordinates, shader };
                pass.set_pipeline(self.pipeline(scene.device, key));
                let mesh = if shader == 0 { self.plain_mesh.clone() } else { self.mesh_group(scene.device, held) };
                pass.set_bind_group(2, &mesh, &[]);
                pass.set_stencil_reference(if stencil.is_some() { held.stencil[2] } else { 0 });
                pass.set_bind_group(3, &self.group, &[((base + index as u64) * SLOT) as u32]);
                pass.set_vertex_buffer(0, held.vertices.slice(..));
                pass.set_vertex_buffer(1, self.defaults.slice(..));
                pass.set_index_buffer(held.indexes.slice(..), wgpu::IndexFormat::Uint16);
                for &(first, count) in &held.parts {
                    pass.draw_indexed(first..first + count, held.base_vertex, 0..1);
                }
            }
        }
        if let Some(dry) = dry {
            dry.zero(pass);
        }
        self.drawn += surfaces.len() as u64;
        count as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_markers() {
        assert!(marked(99998.0) && marked(99997.0) && marked(100000.0) && marked(104095.0));
        assert!(!marked(99999.0) && !marked(0.0) && !marked(104096.0) && !marked(10.0));
        let table = HashMap::from([(3, Look { glow: 0.5, flags: 0, ..Look::STANDARD })]);
        assert_eq!(Look::of(99998.0, &table), Look::STANDARD);
        assert_eq!(Look::of(99997.0, &table).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of(100003.0, &table).glow, 0.5);
        assert_eq!(Look::of(100004.0, &table), Look::STANDARD);
    }

    #[test]
    fn a_look_line() {
        let look = Look::parse("flow=0,-140 speed=1.2 SKY=#FF8040 reflect=sky opacity=vertex p2=1,2 shader=foam clarity=x");
        assert_eq!(look.flow, [0.0, -140.0]);
        assert_eq!(look.speed, 1.2);
        assert_eq!(look.sky, [1.0, 128.0 / 255.0, 64.0 / 255.0]);
        assert_eq!(look.flags, LOOK_HAS_SKY | LOOK_OPACITY_FROM_VERTEX);
        assert_eq!(look.params[2], [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(&look.shader[..5], b"foam\0");
        assert_eq!(look.shader_name().as_deref(), Some("foam"));
        assert_eq!(Look::STANDARD.shader_name(), None);
        assert_eq!(Look::parse("shader=TW_Lava").shader_name().as_deref(), Some("tw_lava"));
        assert_eq!(look.clarity, 800.0);
        assert_eq!(Look::parse("sky=0.5,0.25,1 opacity=0.4").sky, [0.5, 0.25, 1.0]);
        assert_eq!(Look::of_distant(1, None), Look::STANDARD);
        assert_eq!(Look::of_distant(2, None).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of_distant(2, Some("glow=0.5")).flags & LOOK_REFLECTS_SCENE, 0);
        assert_eq!(Look::of_distant(1, Some("glow=0.5")).glow, 0.5);
    }

    #[test]
    fn the_record_of_the_looks() {
        // The layout of WaterLook of waterlook.h.
        assert_eq!(size_of::<Look>(), 144);
        let full = Look { flags: LOOK_HAS_SKY, flow: [0.0, -140.0], speed: 1.2, sky: [1.0, 0.5, 0.25], clarity: 200.0, ..Look::STANDARD };
        // A look of a mod built against a shorter layout: up to the opacity (32 bytes).
        let short = Look { glow: 0.3, opacity: 0.6, ..Look::STANDARD };
        let mut record = Vec::new();
        for word in [1u32, 2, 5] {
            record.extend_from_slice(&word.to_le_bytes());
        }
        record.extend_from_slice(bytemuck::bytes_of(&full));
        record.extend_from_slice(&9u32.to_le_bytes());
        let mut short_bytes = bytemuck::bytes_of(&short)[..32].to_vec();
        short_bytes[..4].copy_from_slice(&32u32.to_le_bytes());
        record.extend_from_slice(&short_bytes);
        let table = Look::read_table(&record).unwrap();
        assert_eq!(table[&5], full);
        assert_eq!(table[&9], Look { size: 32, ..short });
        assert!(Look::read_table(&record[..record.len() - 4]).is_none());
        assert!(Look::read_table(&[2, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    /// The shader compiles: the WGSL is read and checked as the device does it.
    #[test]
    fn the_shader_is_valid() {
        use wgpu::naga;
        let source = shader();
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        for entry in ["vs", "vs_far", "fs", "fs_depth"] {
            assert!(module.entry_points.iter().any(|e| e.name == *entry), "no entry {entry}");
        }
    }

    /// A water shader of a mod: the interface line, the functions it may call, the textures
    /// of the mesh, and an error for a file that is not one.
    #[test]
    fn a_water_shader_of_a_mod() {
        let file = "//! interface: mge-xe-water-wgsl 1
fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);
    var shade = shade_water_rippled(surface, look, standard_ripple(surface, look));
    let base = textureSample(mesh0, mesh_sampler, surface.uv.xy).rgb * look.p0.x;
    shade.colour = fog_apply(mix(base, textureSample(mesh1, mesh_sampler, surface.uv.zw).rgb, 0.5), shade.fog) + 0.0 * water_time();
    return finish_water(surface, shade, look);
}
";
        assert!(water_shader_source(file).is_ok(), "{:?}", water_shader_source(file).err());
        assert!(water_shader_source("fn water_shader(surface: WaterSurface) -> vec4f { return vec4f(1.0); }").unwrap_err().contains("interface"));
        assert!(water_shader_source("//! interface: mge-xe-water-wgsl 1\nfn water_shader(x: f32) -> f32 { return x; }").is_err());
    }
}
