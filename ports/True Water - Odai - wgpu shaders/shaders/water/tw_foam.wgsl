//! interface: mge-xe-water-wgsl 1
// tw_foam.wgsl: the water shader of `tw_foam.fx`, for the wgpu renderer of MGE XE.
// True Water showcase: standard water with foam near the shore.
//
//     wv: shader=tw_foam p0=0.4,0.5,1.5
//
// p0: the strength of the foam (0 to 1), then how deep the water is where the foam of the
// shore ends, and where the foam of open water ends, in metres. A quay wall wants about
// 0.5 and 1.5; a beach 2 and 3.
// The base texture of the mesh is the foam: red is foam on open water, green is foam at the
// shore (textures\tw\water_foam.dds).
//
// The foam is the one of tuxalin's water shader, https://github.com/tuxalin/water-shader:
// two layers of a foam texture that move against each other, each faded by the depth of the
// water. The ripples move the two ranges, in place of the noise of the original.
//
// MIT License. Copyright (c) 2017 tuxalin. Permission is hereby granted, free of charge, to
// any person obtaining a copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including without limitation the
// rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so, subject to
// the following conditions: The above copyright notice and this permission notice shall be
// included in all copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED
// "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
// WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO
// EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
// CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

// World units in a metre.
const tw_foam_metre = 70.0;

// One layer of foam: two samples of a channel of the foam texture, and a fade into the base
// over a range of water depth.
fn tw_foam_layer(texcoord: vec2f, texcoord2: vec2f, lod: f32, shore: bool, range: vec2f, factors: vec2f, water_depth: f32, base: f32) -> f32 {
    let f1 = textureSampleLevel(mesh0, mesh_sampler, texcoord, lod).rg;
    let f2 = textureSampleLevel(mesh0, mesh_sampler, texcoord2, lod).rg;
    let f = select(f1.r * factors.x + f2.r * factors.y, f1.g * factors.x + f2.g * factors.y, shore);
    return mix(f, base, smoothstep(range.x, range.y, water_depth));
}

// surface_pos and scene_pos in the world; water_depth is how deep the water is under the
// surface point; drift is how far the water has moved, so that the foam goes with a current.
fn tw_shore_foam(surface_pos: vec3f, scene_pos: vec3f, eye_vec: vec3f, water_depth: f32, ripple: vec2f, drift: vec2f, dist: f32, ranges_in_metres: vec2f) -> f32 {
    let time = water_time();
    let position = (surface_pos.xy / tw_foam_metre + eye_vec.xy * 0.1) * 0.5 - drift / (2.0 * tw_foam_metre);
    let wind = 0.02 * time * vec2f(1.0, 0.6);

    var s = 0.05 * sin(0.6 * time + scene_pos.x / tw_foam_metre);
    var texcoord = position + wind + s;
    s = 0.05 * sin(0.6 * time + scene_pos.y / tw_foam_metre);
    var texcoord2 = (position + 1.5 * wind + s) * -0.5;
    let texcoord3 = texcoord * 2.0;
    let texcoord4 = (position - 0.45 * wind + s) * -0.5 * 2.0;
    texcoord *= 0.5;
    texcoord2 *= 0.5;

    // The mip level: the texture has 512 texels over 4 m, and a pixel covers more of it with
    // the distance and at a flat angle of view.
    let lod = max(0.0, log2(0.0026 * dist / max(abs(eye_vec.z), 0.15)));

    var ranges = ranges_in_metres * tw_foam_metre + ripple * ranges_in_metres * vec2f(0.3, 0.33) * tw_foam_metre;
    ranges = max(ranges, vec2f(1.0));
    let foam_edge = max(ranges.x, ranges.y);

    let deep_foam = tw_foam_layer(texcoord, texcoord2, lod, false, vec2f(ranges.x, foam_edge), vec2f(1.0, 0.5), water_depth, 0.0);
    return clamp(tw_foam_layer(texcoord3 * 0.25, texcoord4, lod + 1.0, true, vec2f(0.0, ranges.x), vec2f(0.4, 0.6), water_depth, deep_foam), 0.0, 1.0);
}

fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);
    var shade = shade_water(surface, look);

    let strength = look.p0.x;
    let ranges = select(vec2f(0.5, 1.5), look.p0.yz, look.p0.y > 0.0);
    let scene_pos = surface.world + shade.eye_vec * shade.ray_depth;
    let foam = strength * tw_shore_foam(surface.world, scene_pos, shade.eye_vec, shade.water_depth, shade.ripple.xy, look.drift * water_time(), shade.dist, ranges)
        * clamp(1.5 - shade.dist / 4000.0, 0.0, 1.0);

    // Foam, lit by the sun and the sky (sunColAdjusted and skyCol of Direct3D 9)
    let lit = 0.55 * water.sun_col.rgb + 0.35 * water.sky_col.rgb;
    shade.colour = mix(shade.colour, fog_apply(lit, shade.fog), clamp(foam, 0.0, 1.0));

    return finish_water(surface, shade, look);
}
