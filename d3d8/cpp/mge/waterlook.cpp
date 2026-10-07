
#include "waterlook.h"

#include <cstring>
#include <vector>

namespace {
    const WaterLook standardLook = {
        sizeof(WaterLook),
        WATER_LOOK_REFLECTS_SCENE,
        { 0.0f, 0.0f },
        1.0f,
        1.0f,
        0.0f,
        1.0f,
        {},
        "",
    };

    std::vector<WaterLook> looks;
}

void WaterLooks::set(unsigned int slot, const WaterLook* look) {
    if (slot == 0 || slot >= maxSlots) {
        return;
    }
    if (look == nullptr) {
        if (slot < looks.size()) {
            looks[slot] = standardLook;
        }
        return;
    }
    if (slot >= looks.size()) {
        looks.resize(slot + 1, standardLook);
    }
    WaterLook& stored = looks[slot];
    stored = standardLook;
    const auto bytes = look->size < sizeof(WaterLook) ? look->size : sizeof(WaterLook);
    std::memcpy(&stored, look, bytes);
    stored.size = sizeof(WaterLook);
    stored.shader[sizeof(stored.shader) - 1] = '\0';
}

const WaterLook& WaterLooks::get(unsigned int slot) {
    if (slot == 0 || slot >= looks.size()) {
        return standardLook;
    }
    return looks[slot];
}

const WaterLook& WaterLooks::standard() {
    return standardLook;
}
