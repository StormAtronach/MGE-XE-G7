#pragma once

#include <cstdint>

// The look of a water volume surface, beyond its colour: what the mesh and the mod that owns
// it set. A mod reports looks through MGE_WaterLookSet, one per slot, and marks a surface
// with its slot in the specular power of the material (100000 + slot). The struct is a flat
// C layout that is shared with the mod: size comes first, so that a mod built against an
// older or a newer layout can be told apart.
struct WaterLook {
    // sizeof(WaterLook) as the caller has it
    std::uint32_t size;
    // WaterLookFlags
    std::uint32_t flags;
    // Drift of the ripples, in the axes of the mesh, in units per second
    float flow[2];
    // Speed of the ripples; 1 is the standard
    float speed;
    // Size of the ripples; 1 is the standard
    float scale;
    // Light that the surface gives, 0 to 1
    float glow;
    // 1 is water; lower shows what is behind the surface
    float opacity;
    // Free parameters p0 to p3, for a water shader of the mod's own
    float params[4][4];
    // The name of the water shader of a mod, the file Data Files\shaders\water\<name>.fx,
    // NUL terminated; empty for the standard shading. A name that MGE has no shader for
    // gives the standard shading.
    char shader[32];
};

enum WaterLookFlags : std::uint32_t {
    // The surface reflects what is on screen; otherwise the sky only
    WATER_LOOK_REFLECTS_SCENE = 1,
    // The vertex colour of the mesh tints the water
    WATER_LOOK_TINT_FROM_VERTEX = 2,
    // The vertex alpha of the mesh is the opacity of the water
    WATER_LOOK_OPACITY_FROM_VERTEX = 4,
};

namespace WaterLooks {
    // Slots that a mod can use. Slot 0 is the standard look and cannot be set.
    constexpr unsigned int maxSlots = 4096;

    // The specular power of a material that marks a surface with a look slot
    constexpr float slotMarkerBase = 100000.0f;

    // Keeps a copy of the look for the slot; a null look clears the slot. Fields past the
    // caller's size keep their standard values.
    void set(unsigned int slot, const WaterLook* look);

    // The look of a slot: the standard look for slot 0, for a slot never set, and for a slot
    // out of range.
    const WaterLook& get(unsigned int slot);

    // The standard look
    const WaterLook& standard();
}
