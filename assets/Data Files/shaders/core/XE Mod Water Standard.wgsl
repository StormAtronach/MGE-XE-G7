//! part: standard

fn water_shader(surface: WaterSurface) -> vec4f {
    let look = surface_look(surface);
    return finish_water(surface, shade_water(surface, look), look);
}
