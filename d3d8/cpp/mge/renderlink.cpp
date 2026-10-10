// The host side of the render link. See mge_render_link.h for the contract and
// docs/architecture/render-link.md for the design.

#include "renderlink.h"

#include "ffeshader.h"
#include "support/log.h"

#include <cmath>
#include <cstring>

namespace {
    const uint32_t kMaxLights = 64;
    // The game sends the sun and the interior "sun" as light 6. See MGEProxyDevice::SetLight.
    const uint32_t kSunLightIndex = 6;
    // A radius of a light record is an integer. The radius that the proxy solves from the
    // attenuation is equal when it is this near.
    const float kRadiusTolerance = 0.5f;

    MgeLinkCountersV1 counters = { sizeof(MgeLinkCountersV1) };
    bool connected = false;
    bool sceneOpen = false;
    bool drawValid = false;
    MgeLinkSceneV1 currentScene = {};
    MgeLinkDrawV1 currentDraw = {};
    MgeLinkLightV1 currentLights[kMaxLights];
    // The number of D3D8 draws that the proxy saw for the current packet.
    uint32_t d3dDraws = 0;

    // Copies a packet of a client that can be older or newer than this build.
    template <typename T>
    void copyPacket(T& to, const T* from) {
        to = {};
        const uint32_t size = from->structSize < sizeof(T) ? from->structSize : sizeof(T);
        std::memcpy(&to, from, size);
    }

    // The first differences of each fact go to the log, with the object of the draw.
    const uint32_t kLoggedDifferences = 12;
    uint32_t loggedDifferences[MGE_LINK_FACT_COUNT] = {};

    void logDifference(uint32_t fact, bool inferred) {
        if (loggedDifferences[fact] >= kLoggedDifferences) {
            return;
        }
        ++loggedDifferences[fact];
        // The name of a NiObjectNET is a C string pointer at offset 8.
        const char* name = nullptr;
        if (drawValid && currentDraw.geometry) {
            name = *reinterpret_cast<const char* const*>(currentDraw.geometry + 8);
        }
        LOG::logline("-- Render link: fact %u differs: MGE XE %s, packet %s; scene kind %u, class %u, object '%s', "
            "shininess %g, D3D8 draw %u of the packet",
            fact, inferred ? "yes" : "no", inferred ? "no" : "yes", sceneOpen ? currentScene.kind : 0,
            drawValid ? currentDraw.objectClass : 0, name ? name : "", drawValid ? currentDraw.materialShininess : 0.0f,
            d3dDraws);
    }

    void count(uint32_t fact, bool inferred, bool fromPacket) {
        if (fact < MGE_LINK_FACT_COUNT) {
            auto& entry = counters.facts[fact];
            if (inferred == fromPacket) {
                ++(inferred ? entry.bothYes : entry.bothNo);
            } else {
                ++(inferred ? entry.hostOnly : entry.packetOnly);
                logDifference(fact, inferred);
            }
        }
    }

    void __cdecl sceneBegin(const MgeLinkSceneV1* scene) {
        if (!scene || scene->structSize < sizeof(uint32_t) * 2) {
            return;
        }
        copyPacket(currentScene, scene);
        sceneOpen = true;
        drawValid = false;
        ++counters.scenes;
    }

    void __cdecl sceneEnd() {
        sceneOpen = false;
        drawValid = false;
    }

    void __cdecl frameEvent(uint32_t kind, uint32_t a, uint32_t b) {
        (void)kind;
        (void)a;
        (void)b;
    }

    void __cdecl draw(const MgeLinkDrawV1* packet) {
        if (!packet || packet->structSize < sizeof(uint32_t) * 2) {
            return;
        }
        copyPacket(currentDraw, packet);
        // The light array belongs to the client. Keep a copy.
        if (!currentDraw.lights) {
            currentDraw.lightCount = 0;
        }
        if (currentDraw.lightCount > kMaxLights) {
            currentDraw.lightCount = kMaxLights;
        }
        std::memcpy(currentLights, currentDraw.lights, currentDraw.lightCount * sizeof(MgeLinkLightV1));
        currentDraw.lights = currentLights;
        drawValid = true;
        d3dDraws = 0;
        ++counters.draws;
    }

    void __cdecl readCounters(MgeLinkCountersV1* out, uint32_t reset) {
        if (out && out->structSize >= sizeof(uint32_t)) {
            const uint32_t size = out->structSize < sizeof(counters) ? out->structSize : sizeof(counters);
            std::memcpy(out, &counters, size);
            out->structSize = size;
        }
        if (reset) {
            counters = { sizeof(MgeLinkCountersV1) };
        }
    }

    const MgeRenderLinkHostV1 host = {
        sizeof(MgeRenderLinkHostV1),
        MGE_RENDER_LINK_VERSION,
        MGE_RENDER_LINK_CAP_COMPARE,
        0,
        sceneBegin,
        sceneEnd,
        frameEvent,
        draw,
        readCounters,
    };
}

namespace RenderLink {
    bool isConnected() {
        return connected;
    }

    const MgeLinkSceneV1* scene() {
        return sceneOpen ? &currentScene : nullptr;
    }

    const MgeLinkDrawV1* currentDrawPacket() {
        return drawValid ? &currentDraw : nullptr;
    }

    uint32_t drawClass() {
        return drawValid ? currentDraw.objectClass : MGE_LINK_CLASS_UNKNOWN;
    }

    bool drawIsSky() {
        const uint32_t c = drawClass();
        return c >= MGE_LINK_CLASS_SKY_DOME && c <= MGE_LINK_CLASS_SKY_OTHER;
    }

    bool drawIsLandSplat() {
        // observeWorldDraw counted the current D3D8 draw already.
        return drawClass() == MGE_LINK_CLASS_LANDSCAPE && d3dDraws > 1;
    }

    void compare(uint32_t fact, bool inferred, bool fromPacket) {
        if (drawValid) {
            count(fact, inferred, fromPacket);
        }
    }

    void observeBeginScene(bool mainView, int sceneIndex) {
        if (!sceneOpen) {
            return;
        }
        const uint32_t kind = currentScene.kind;
        const bool uiScene = kind == MGE_LINK_SCENE_UI || kind == MGE_LINK_SCENE_SPLASH
            || kind == MGE_LINK_SCENE_RACE_MENU_HEAD;
        count(MGE_LINK_FACT_UI_SCENE, !mainView, uiScene);
        if (mainView) {
            count(MGE_LINK_FACT_SCENE_KIND, sceneIndex == 0, kind == MGE_LINK_SCENE_WORLD);
        }
    }

    void observeWorldDraw(bool waterMaterial, const LightState& lights) {
        if (!connected) {
            return;
        }
        if (!drawValid) {
            ++counters.drawsWithoutPacket;
            return;
        }
        ++d3dDraws;
        count(MGE_LINK_FACT_WATER_PLANE, waterMaterial, currentDraw.objectClass == MGE_LINK_CLASS_WATER_PLANE);
        if (d3dDraws != 1) {
            // The lights of a packet are the same for each of its D3D8 draws.
            return;
        }
        for (uint32_t i = 0; i != currentDraw.lightCount; ++i) {
            const MgeLinkLightV1& light = currentLights[i];
            if (light.type == MGE_LINK_LIGHT_DIRECTIONAL) {
                // The proxy takes light 6 as the sun. The packet says which light is directional.
                count(MGE_LINK_FACT_SUN_LIGHT, light.deviceIndex == kSunLightIndex, true);
            } else if (light.type == MGE_LINK_LIGHT_POINT) {
                const auto found = lights.lights.find(light.deviceIndex);
                const float inferred = found != lights.lights.end() ? found->second.radius : 0.0f;
                // Yes means a radius that is not 0. When the two radii are different, the
                // larger one gets the count.
                if (std::fabs(inferred - light.radius) <= kRadiusTolerance) {
                    count(MGE_LINK_FACT_LIGHT_RADIUS, inferred != 0.0f, inferred != 0.0f);
                } else {
                    if (loggedDifferences[MGE_LINK_FACT_LIGHT_RADIUS] < kLoggedDifferences) {
                        const auto& l = found != lights.lights.end() ? found->second : LightState::Light{};
                        LOG::logline("-- Render link: light %u radius: MGE XE %g, packet %g; attenuation %g %g %g",
                            light.deviceIndex, inferred, light.radius, l.falloff.x, l.falloff.y, l.falloff.z);
                    }
                    count(MGE_LINK_FACT_LIGHT_RADIUS, inferred > light.radius, inferred <= light.radius);
                }
            }
        }
    }
}

extern "C" const MgeRenderLinkHostV1* __cdecl MGE_RenderLinkConnect(uint32_t clientVersion) {
    if (clientVersion == 0) {
        return nullptr;
    }
    connected = true;
    return &host;
}
