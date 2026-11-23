//! part: ripples

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
