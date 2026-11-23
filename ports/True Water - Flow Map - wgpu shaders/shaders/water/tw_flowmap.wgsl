//! interface: mge-xe-water-wgsl 1
// tw_flowmap.wgsl: the water shader of `tw_flowmap.fx`, for the wgpu renderer of MGE XE.
// Ripples from a normal map of the mod's own, which move along a flow map. The water itself is
// the standard water.
//
// The mesh gives the two textures:
//   base texture     a normal map of ripples that repeats (red and green are the slopes)
//   second texture   the flow map, over the second texture coordinates of the mesh: red and
//                    green are the direction of the flow in the axes of the mesh (0.5 is
//                    none), blue is how fast it is there (0 to 1)
// and the look line names the shader:
//     wv: shader=tw_flowmap p0=520,110,5,0.6
// p0: the size of one repeat of the normal map in the world, the speed of the ripples where
//     the flow is full, in units per second, the seconds of one cycle, and how strong the
//     ripples are (1 is the normal map as it is).
// p1: 1 shows the flow as colours in place of the water, to check a flow map.
//
// A normal map that is pushed along a flow stretches without end. So it is read two times,
// each pushed for one cycle and then put back, half a cycle apart, and the two are mixed so
// that each is gone at the moment it is put back.
//
// Far away, in distant land, the mesh has no second texture: there the water has the
// standard ripples.
//
// The textures are read before any choice that depends on the look (textureSample needs
// uniform control flow), and `select` makes the choices: the view of the flow, and the
// standard ripples far away.

fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);

    let tile = select(420.0, look.p0.x, look.p0.x > 0.0);
    let speed = select(110.0, look.p0.y, look.p0.y > 0.0);
    let cycle = select(5.0, look.p0.z, look.p0.z > 0.0);
    let strength = select(1.0, look.p0.w, look.p0.w > 0.0);

    // The flow at this place, turned from the axes of the mesh into the world (the world
    // matrix of the surface, as the vertex shader uses it)
    let flow_map = textureSample(mesh1, mesh_sampler, surface.uv.zw);
    let flow = (s.world * vec4f(flow_map.rg * 2.0 - 1.0, 0.0, 0.0)).xy * speed;

    let place = surface.world.xy / tile;
    let turn = water_time() / cycle;
    let first = fract(turn);
    let second = fract(turn + 0.5);
    let pushed = flow * cycle / tile;
    let a = textureSample(mesh0, mesh_sampler, place - pushed * (first - 0.5)).rg;
    let b = textureSample(mesh0, mesh_sampler, place + 0.37 - pushed * (second - 0.5)).rg;
    var slopes = mix(a, b, abs(1.0 - 2.0 * first)) * 2.0 - 1.0;

    // Faster water is rougher
    slopes *= strength * mix(0.7, 1.7, flow_map.b);

    // Far away the mesh has no second texture, and the water has the standard ripples. The
    // shading is asked for one time, with the one or the other.
    let ripple = select(normalize(vec3f(slopes, 1.0)), standard_ripple(surface, look), look.distant);

    let shade = shade_water_rippled(surface, look, ripple);
    let water_colour = finish_water(surface, shade, look);

    // For the making of a flow map: p1=1 shows the flow as it arrives here, red for east,
    // green for north, grey for none, blue for the speed.
    let flow_view = vec4f(0.5 + 0.5 * flow / speed, flow_map.b, 1.0);
    return select(water_colour, flow_view, look.p1.x > 0.5);
}
