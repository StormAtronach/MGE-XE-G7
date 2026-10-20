// The grass of MGE XE in the wgpu renderer: `GrassInstVS` and `GrassPS` of `XE Mod Grass.fx`.
//
// One draw for each grass subset, with one instance record for each place. A blade moves
// with the wind and away from the player, the sun lights it from the two sides, and it
// takes the shadow map of the frame. A subset that arrived on the GPU comes in over a time
// with a 4 x 4 dither (the fade-in of the distant statics).
//
// Three looks of the grass shader of Remiros' Groundcover are options, off at the start
// (`[distant_land.grass]` in mgeXE.toml, `g.look`):
// - x, `soft_end`: near the end of the grass a blade sinks into the ground. The band starts
//   at 5300 / 7300 of the end of the grass, and a noise moves it for each vertex.
// - y, `vertex_color_sway`: the red of the vertex colour weights the sway, so a part with
//   no red stays where it is (a root, a stone).
// - z, `wrap_lighting`: the sun light wraps past the side of a blade, and 0.4 of it comes
//   through from the back. With `vertex_color_sway`, only the parts with red over 0.5 get
//   it; the other parts are lit from one side.
// The numbers of the soft end are those of Remiros' shader for a grass range of 7300 units,
// scaled to the range of the frame. The looks are Remiros' work, in MGE XE with Remiros'
// approval.

// The view group (`view.rs`, group 0) comes before this code: the game view in `u`, with the
// linear fog of the near cells in `u.fog.zw`. The values of the grass are in group 1.
struct Grass {
    // x: the end of the grass (the view range of the game).
    range: vec4f,
    // The fog colour of the near cells.
    fog_color: vec4f,
    // The light of the sun of the game and the ambient light, also in a frame without a
    // direction of the sun (the view has none of them then).
    sun_color: vec4f,
    ambient: vec4f,
    // xy: the wind; z: the time of the game.
    wind: vec4f,
    // xyz: the position of the player; w: 1 when it is known.
    foot: vec4f,
    // The matrices from the world to the two layers of the shadow map.
    shadow_layer0: mat4x4f,
    shadow_layer1: mat4x4f,
    // ESM_c, ESM_bias, ESM_scale, 1 over the resolution of a layer.
    esm: vec4f,
    // shadecolor, and shade of shadowSunEstimate.
    shade: vec4f,
    // x: 1 with a shadow map. y: the light of the sun for shadowSunEstimate. z: 1 above the
    // sea. w: 1 to end the shadow as on the ground (the edge of the map and the fog).
    shadow: vec4f,
    // x: soft end, y: vertex colour sway, z: wrap lighting (1 on).
    look: vec4f,
}
@group(1) @binding(0) var<uniform> g: Grass;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(2) var shadow_atlas: texture_2d<f32>;
@group(1) @binding(3) var shadow_sampler: sampler;
@group(2) @binding(0) var base_texture: texture_2d<f32>;

struct In {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    // The vertex colour, in the byte order of D3DCOLOR: z is the red.
    @location(2) color: vec4f,
    @location(3) coordinate: vec2f,
    // The world matrix of the place: its three columns that a position uses.
    @location(4) world0: vec4f,
    @location(5) world1: vec4f,
    @location(6) world2: vec4f,
    // x: the share of the subset that is in (the fade-in after its arrival).
    @location(7) fade: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
    @location(1) coordinate: vec2f,
    @location(2) fog: f32,
    @location(3) @interpolate(flat) fade: f32,
    // The light of the sun for the shadow (shadowSunEstimate), and the position in the two
    // layers of the shadow map, z divided by w.
    @location(4) light: f32,
    @location(5) shadow0: vec3f,
    @location(6) shadow1: vec3f,
}

// The order of the pixels of a block of 4 x 4 for the fade-in (Bayer), as the statics have it.
const FADE_ORDER = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);

// Remiros' wrap light: how far the light wraps past the side, its power, and the light
// through the back.
const WRAP_W: f32 = 0.6;
const WRAP_N: f32 = 1.5;
const BACK_LIGHT: f32 = 0.4;

// shadowDeltaZ: the depth of the shadow map less that of the pixel, in the layer that has
// the pixel; a positive number when the map has nothing nearer to the sun.
fn shadow_delta(s0: vec3f, s1: vec3f) -> f32 {
    let margin = vec3f(1.0 - 8.0 * g.esm.w, 1.0 - 8.0 * g.esm.w, 1.0);
    if all(abs(s0) < margin) {
        let uv = vec2f(0.5 + 0.5 * s0.x, 0.5 - 0.5 * s0.y);
        return textureSampleLevel(shadow_atlas, shadow_sampler, vec2f(uv.x * 0.5, uv.y), 0.0).r / g.esm.z - s0.z;
    }
    if all(abs(s1) < margin) {
        let uv = vec2f(0.5 + 0.5 * s1.x, 0.5 - 0.5 * s1.y);
        return textureSampleLevel(shadow_atlas, shadow_sampler, vec2f(0.5 + uv.x * 0.5, uv.y), 0.0).r / g.esm.z - s1.z;
    }
    return 1e-6;
}

// grassDisplacement: the wind in four waves, and the grass that the player pushes away. The
// height of a vertex in its model is the weight.
fn displacement(world: vec3f, height: f32) -> vec2f {
    let time = g.wind.z;
    let speed = length(g.wind.xy);
    let displace = 2.0 * g.wind.xy + 0.1;
    var harmonics = vec2f(0.0);
    harmonics += (1.0 - 0.10 * speed) * sin(1.0 * time + world.xy / 1104.0);
    harmonics += (1.0 - 0.04 * speed) * cos(2.0 * time + world.xy / 751.0);
    harmonics += (1.0 + 0.14 * speed) * sin(3.0 * time + world.xy / 526.0);
    harmonics += (1.0 + 0.28 * speed) * sin(5.0 * time + world.xy / 209.0);
    var stomp = vec2f(0.0);
    if g.foot.w > 0.5 {
        var d = length(world.xy - g.foot.xy);
        let above = 0.06 * max(0.0, g.foot.z - world.z - 60.0);
        d += above * above;
        if d < 150.0 {
            stomp = (60.0 / max(d, 0.001) - 0.4) * (world.xy - g.foot.xy);
        }
    }
    return clamp(0.02 * height, 0.0, 1.0) * (harmonics * displace + stomp);
}

// bnoise of Remiros' shader: a smooth noise of one number, -1 to 1.
fn bnoise(x: f32) -> f32 {
    let i = floor(x);
    let f = fract(x);
    let s = sign(fract(x / 2.0) - 0.5);
    let k = fract(i * 0.1731);
    return s * f * (f - 1.0) * ((16.0 * k - 4.0) * f * (f - 1.0) - 1.0);
}

@vertex fn vs(in: In) -> Out {
    var out: Out;
    let model = vec4f(in.position.xyz, 1.0);
    var world = vec3f(dot(model, in.world0), dot(model, in.world1), dot(model, in.world2));
    let red = in.color.z;
    // The soft end: how far the vertex sinks, from its distance before the wind moves it.
    var sink = 0.0;
    if g.look.x > 0.5 {
        let scale = g.range.x / 7300.0;
        let distance = length(world - u.eye.xyz) - 1900.0 * scale * clamp(bnoise(world.x + world.y), 0.0, 1.0);
        sink = 100.0 * smoothstep(5300.0 * scale, 7300.0 * scale, distance);
    }
    var moved = displacement(world, in.position.z);
    if g.look.y > 0.5 {
        moved *= red;
    }
    world = vec3f(world.xy + moved, world.z - sink);
    out.position = clip_of(world);
    // The normal goes through the world matrix and is not made of length 1 again.
    let n = vec4f(normalize(in.normal.xyz * 2.0 - 1.0), 0.0);
    let normal = vec3f(dot(n, in.world0), dot(n, in.world1), dot(n, in.world2));
    let to_vertex = world - u.eye.xyz;
    // The light of the sun for the side that the camera sees.
    let facing = dot(normal, -u.sun_direction.xyz) * -sign(dot(to_vertex, normal));
    var lambert: f32;
    if g.look.z > 0.5 {
        if g.look.y < 0.5 || red > 0.5 {
            // Remiros' wrap light, and the light through the back.
            lambert = pow(clamp((facing + WRAP_W) / (1.0 + WRAP_W), 0.0, 1.0), WRAP_N) * (WRAP_N + 1.0) / (2.0 * (1.0 + WRAP_W)) + max(0.0, -facing) * BACK_LIGHT;
        } else {
            // A part without red: the light of one side.
            lambert = dot(normal, -u.sun_direction.xyz);
        }
        lambert = max(0.0, lambert);
    } else {
        // The other side gets a part (the back-scatter of MGE XE).
        lambert = facing;
        if lambert < 0.0 {
            lambert *= -0.3;
        }
    }
    out.color = g.sun_color.rgb * lambert + g.ambient.rgb;
    out.coordinate = in.coordinate;
    // fogMWScalar: the linear fog of the near cells.
    out.fog = clamp((u.fog.w - length(to_vertex)) / max(u.fog.w - u.fog.z, 1.0), 0.0, 1.0);
    out.fade = in.fade.x;
    // shadowSunEstimate of the light of the blade.
    let x = lambert * g.shadow.y;
    out.light = x / (g.shade.w + x);
    let s0 = g.shadow_layer0 * vec4f(world, 1.0);
    let s1 = g.shadow_layer1 * vec4f(world, 1.0);
    out.shadow0 = vec3f(s0.xy, s0.z / s0.w);
    out.shadow1 = vec3f(s1.xy, s1.z / s1.w);
    return out;
}

// True for the pipeline with multisampling and alpha to coverage (a pipeline constant).
override ALPHA_TO_COVERAGE: bool = false;

@fragment fn fs(in: Out) -> @location(0) vec4f {
    let texel = textureSample(base_texture, base_sampler, in.coordinate);
    let cell = vec2u(in.position.xy) % 4u;
    // GrassPS discards below 64; then the alpha test of the pass (AlphaRef 128) cuts, or,
    // with alpha to coverage, which turns the alpha test off, the coverage
    // calc_coverage(a, 128 / 255, 4) of `XE Common.fx` gives the samples of the pixel.
    if texel.a < 64.0 / 255.0 || (!ALPHA_TO_COVERAGE && texel.a < 128.0 / 255.0) || in.fade < (FADE_ORDER[cell.y * 4u + cell.x] + 0.5) / 16.0 {
        discard;
    }
    let coverage = clamp(4.0 * (texel.a - 128.0 / 255.0) + 128.0 / 255.0, 0.0, 1.0);
    var color = texel.rgb * in.color;
    // The shadow, as GrassPS: shadowESM times the light of the sun, towards shadecolor.
    if g.shadow.x > 0.5 {
        let dz = shadow_delta(in.shadow0, in.shadow1);
        var v = (1.0 - clamp(exp(g.esm.x * dz + g.esm.y), 0.0, 1.0)) * in.light;
        if g.shadow.w > 0.5 {
            // As the ground under the blade (RenderShadowsPS): the fade at the edge of the
            // outer layer, and the fog weakens the shadow.
            let edge = clamp(25.0 * (1.0 - abs(in.shadow1.xy)), vec2f(0.0), vec2f(1.0));
            let fogatt = in.fog * in.fog;
            v *= edge.x * edge.y * select(clamp(4.0 * fogatt, 0.0, 1.0), fogatt, g.shadow.z > 0.5);
        }
        color *= 1.0 - v * g.shade.rgb;
    }
    // The pipeline writes no alpha: the alpha is only the coverage.
    return vec4f(mix(g.fog_color.rgb, color, in.fog), coverage);
}
