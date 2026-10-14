
#include "waterlook.h"

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <string>
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
        { 0.0f, 0.0f, 0.0f },
        800.0f,
    };

    std::vector<WaterLook> looks;
    std::vector<WaterLook> distantLooks;

    // Up to count numbers with commas between them.
    void readNumbers(const std::string& value, float* out, int count) {
        const char* at = value.c_str();
        for (int i = 0; i < count && *at != '\0'; ++i) {
            char* end = nullptr;
            const float number = std::strtof(at, &end);
            if (end == at) {
                return;
            }
            out[i] = number;
            at = *end == ',' ? end + 1 : end;
        }
    }
}

WaterLook WaterLooks::parse(const char* text, size_t length) {
    WaterLook look = standardLook;
    size_t at = 0;
    while (at < length) {
        while (at < length && std::isspace(static_cast<unsigned char>(text[at]))) {
            ++at;
        }
        size_t end = at;
        while (end < length && !std::isspace(static_cast<unsigned char>(text[end]))) {
            ++end;
        }
        std::string word(text + at, end - at);
        at = end;
        const auto equals = word.find('=');
        if (equals == std::string::npos) {
            continue;
        }
        std::string key = word.substr(0, equals);
        const std::string value = word.substr(equals + 1);
        std::transform(key.begin(), key.end(), key.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });

        if (key == "flow") {
            readNumbers(value, look.flow, 2);
        } else if (key == "speed") {
            readNumbers(value, &look.speed, 1);
        } else if (key == "scale") {
            readNumbers(value, &look.scale, 1);
        } else if (key == "glow") {
            readNumbers(value, &look.glow, 1);
        } else if (key == "clarity") {
            readNumbers(value, &look.clarity, 1);
        } else if (key == "opacity") {
            // The word "vertex": the generator wrote the opacity of the mesh's vertex into
            // the vertex of the distant subset.
            if (_stricmp(value.c_str(), "vertex") == 0) {
                look.flags |= WATER_LOOK_OPACITY_FROM_VERTEX;
            } else {
                look.flags &= ~WATER_LOOK_OPACITY_FROM_VERTEX;
                readNumbers(value, &look.opacity, 1);
            }
        } else if (key == "sky") {
            // A colour as RRGGBB, or as three numbers from 0 to 1
            const char* digits = value.c_str() + (value[0] == '#' ? 1 : 0);
            const bool isHex = std::strlen(digits) == 6 && std::strspn(digits, "0123456789abcdefABCDEF") == 6;
            if (isHex) {
                const unsigned long colour = std::strtoul(digits, nullptr, 16);
                look.sky[0] = ((colour >> 16) & 255) / 255.0f;
                look.sky[1] = ((colour >> 8) & 255) / 255.0f;
                look.sky[2] = (colour & 255) / 255.0f;
            } else {
                readNumbers(value, look.sky, 3);
            }
            look.flags |= WATER_LOOK_HAS_SKY;
        } else if (key == "reflect") {
            if (_stricmp(value.c_str(), "sky") == 0) {
                look.flags &= ~static_cast<std::uint32_t>(WATER_LOOK_REFLECTS_SCENE);
            }
        } else if (key == "shader") {
            strncpy_s(look.shader, value.c_str(), _TRUNCATE);
        } else if (key.length() == 2 && key[0] == 'p' && key[1] >= '0' && key[1] <= '3') {
            readNumbers(value, look.params[key[1] - '0'], 4);
        }
    }
    return look;
}

void WaterLooks::clearDistant() {
    distantLooks.clear();
}

std::uint8_t WaterLooks::addDistant(const WaterLook& look, std::uint8_t kind) {
    for (size_t i = 0; i < distantLooks.size(); ++i) {
        if (std::memcmp(&distantLooks[i], &look, sizeof(WaterLook)) == 0) {
            return static_cast<std::uint8_t>(firstDistantLook + i);
        }
    }
    if (firstDistantLook + distantLooks.size() > 255) {
        return kind;
    }
    distantLooks.push_back(look);
    return static_cast<std::uint8_t>(firstDistantLook + distantLooks.size() - 1);
}

const WaterLook& WaterLooks::distant(std::uint8_t water) {
    if (water < firstDistantLook || water - firstDistantLook >= distantLooks.size()) {
        return standardLook;
    }
    return distantLooks[water - firstDistantLook];
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

// In main.cpp: sends data to the wgpu renderer in the helper.
void sendToRenderer(unsigned int id, const void* data, unsigned int length);

void WaterLooks::sendToRenderer() {
    // The record 6 of the data that MGE XE puts into the stream, with the bit that makes the
    // stream keep it (a capture file then has the looks that a mod set before it): the
    // version, the number of slots, then for each slot that is not the standard look its
    // number and its WaterLook. mgeHost64/src/renderer/water/volume.rs reads it.
    static const unsigned int WaterLooksId = 6 | 0x80000000u;
    struct Head {
        std::uint32_t version, count;
    };
    std::vector<unsigned char> record(sizeof(Head));
    std::uint32_t count = 0;
    for (std::uint32_t slot = 1; slot < looks.size(); ++slot) {
        if (std::memcmp(&looks[slot], &standardLook, sizeof(WaterLook)) == 0) {
            continue;
        }
        const auto at = record.size();
        record.resize(at + sizeof(slot) + sizeof(WaterLook));
        std::memcpy(&record[at], &slot, sizeof(slot));
        std::memcpy(&record[at + sizeof(slot)], &looks[slot], sizeof(WaterLook));
        ++count;
    }
    const Head head = { 1, count };
    std::memcpy(record.data(), &head, sizeof(head));
    ::sendToRenderer(WaterLooksId, record.data(), static_cast<unsigned int>(record.size()));
}
