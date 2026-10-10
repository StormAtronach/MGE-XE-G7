// The host side of the render link. See mge_render_link.h for the contract and
// docs/architecture/render-link.md for the design.

#include "renderlink.h"

#include "camerarelative.h"
#include "ffeshader.h"
#include "mwbridge.h"
#include "proxydx/d3d8texture.h"
#include "support/log.h"

#include <unordered_map>

#include <cmath>
#include <cstring>

namespace {
    const uint32_t kMaxLights = 64;
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
    // The state packet of the D3D8 draw call that comes next.
    MgeLinkDrawStateV1 currentState = {};
    bool stateValid = false;
    // The state packet of the D3D8 draw that the proxy inspects now. observeDrawState sets
    // it, applyDrawState reads it.
    bool stateForThisDraw = false;
    const uint32_t kMaxStateLights = 64;
    MgeLinkStateLightV1 stateLights[kMaxStateLights];
    // The facts that the client told the host to take from the packets. Bit n is fact n.
    uint32_t usedFacts = 0;
    // The facts that the proxy can take from a packet.
    const uint32_t kUsableFacts = (1u << MGE_LINK_FACT_UI_SCENE) | (1u << MGE_LINK_FACT_WATER_PLANE)
        | (1u << MGE_LINK_FACT_SUN_LIGHT) | (1u << MGE_LINK_FACT_SKY_DRAW) | (1u << MGE_LINK_FACT_LAND_SPLAT)
        | (1u << MGE_LINK_FACT_SCENE_KIND) | (1u << MGE_LINK_FACT_MOON_SHADOW) | (1u << MGE_LINK_FACT_STATE_DRAW);
    // The radius of a light is not in this list. The light fade needs the radius that agrees
    // with the attenuation of the light. A light of a mod can have an attenuation that does
    // not come from the radius that the game has for it. The comparison stays.

    // The radius of the light record and the attach radius of each point light that went
    // through the shim of the light fade. The key is the address of the NiPointLight.
    struct LightRadius {
        int record;
        int attach;
    };
    std::unordered_map<uint32_t, LightRadius> lightRadii;
    const size_t kMaxLightRadii = 8192;
    // The mark that the proxy puts in the material of the water plane, and the value that
    // was there before.
    const float kWaterMark = 99999.0f;
    float waterShininess = 0.0f;
    bool waterMarked = false;
    bool waterMarkRemoved = false;
    // The same for the mark in the materials of the two moons.
    const float kMoonMark = 88888.0f;
    float moonShininess[2] = {};
    bool moonMarked = false;
    bool moonMarkRemoved = false;
    // The scene packet came and the view transform of the scene did not come yet.
    bool sceneViewPending = false;

    // The texture of the proxy for a handle of a state packet. In this version the handle
    // is the D3D8 interface pointer, and the object behind it is a texture of the proxy.
    IDirect3DTexture9* textureOfHandle(uint32_t handle) {
        const auto proxy = reinterpret_cast<IDirect3DBaseTexture8*>(static_cast<uintptr_t>(handle));
        return proxy ? static_cast<ProxyTexture*>(proxy)->realTexture : nullptr;
    }

    // The number of world matrices that a draw uses.
    uint32_t worldCount(const MgeLinkDrawStateV1& s) {
        const bool blend = (s.flags & MGE_LINK_STATE_SKINNED) && s.vertexBlend >= D3DVBF_1WEIGHTS && s.vertexBlend <= D3DVBF_3WEIGHTS;
        return blend ? s.vertexBlend + 1 : 1;
    }

    const float* worldOfPacket(const MgeLinkDrawStateV1& s, uint32_t index) {
        return index == 0 ? s.world : s.blendWorlds[index - 1];
    }

    // The colour of D3DRS_AMBIENT as the proxy keeps it.
    bool ambientIsWhite(uint32_t ambient) {
        return ambient == 0xffffffff;
    }

    bool uses(uint32_t fact) {
        return (usedFacts >> fact) & 1u;
    }

    bool sceneIsUi() {
        const uint32_t kind = currentScene.kind;
        return kind == MGE_LINK_SCENE_UI || kind == MGE_LINK_SCENE_SPLASH || kind == MGE_LINK_SCENE_RACE_MENU_HEAD;
    }

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
        sceneViewPending = true;
        sceneOpen = true;
        drawValid = false;
        ++counters.scenes;
    }

    void __cdecl drawState(const MgeLinkDrawStateV1* state) {
        if (!state || state->structSize < sizeof(uint32_t) * 2) {
            return;
        }
        copyPacket(currentState, state);
        // The light array belongs to the client. Keep a copy.
        if (state->structSize < sizeof(MgeLinkDrawStateV1) || !currentState.lights) {
            currentState.lightCount = 0;
        }
        if (currentState.lightCount > kMaxStateLights) {
            currentState.lightCount = kMaxStateLights;
        }
        std::memcpy(stateLights, currentState.lights, currentState.lightCount * sizeof(MgeLinkStateLightV1));
        currentState.lights = stateLights;
        stateValid = true;
    }

    void __cdecl sceneEnd() {
        stateValid = false;
        sceneViewPending = false;
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

    uint32_t __cdecl useFacts(uint32_t factMask) {
        usedFacts = factMask & kUsableFacts;
        LOG::logline("-- Render link: facts taken from the packets: mask 0x%x", usedFacts);

        // The water plane comes from the packets: take the mark out of the material of the
        // water. Put it back when the inference is in use again.
        auto mwBridge = MWBridge::get();
        if (waterMarked && mwBridge->IsLoaded() && uses(MGE_LINK_FACT_WATER_PLANE) != waterMarkRemoved) {
            mwBridge->markWaterNode(waterMarkRemoved ? kWaterMark : waterShininess);
            waterMarkRemoved = !waterMarkRemoved;
            LOG::logline("-- Render link: water mark %s", waterMarkRemoved ? "removed" : "put back");
        }
        if (moonMarked && mwBridge->IsLoaded() && uses(MGE_LINK_FACT_MOON_SHADOW) != moonMarkRemoved) {
            const float marks[2] = { kMoonMark, kMoonMark };
            mwBridge->markMoonNodes(moonMarkRemoved ? marks : moonShininess, nullptr);
            moonMarkRemoved = !moonMarkRemoved;
            LOG::logline("-- Render link: moon mark %s", moonMarkRemoved ? "removed" : "put back");
        }
        return usedFacts;
    }

    const MgeRenderLinkHostV1 host = {
        sizeof(MgeRenderLinkHostV1),
        MGE_RENDER_LINK_VERSION,
        MGE_RENDER_LINK_CAP_COMPARE | MGE_RENDER_LINK_CAP_USE_FACTS | MGE_RENDER_LINK_CAP_DRAW_STATE,
        0,
        sceneBegin,
        sceneEnd,
        frameEvent,
        draw,
        readCounters,
        useFacts,
        drawState,
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

    bool drawIsMoon() {
        const uint32_t c = drawClass();
        return c == MGE_LINK_CLASS_SKY_MOON || c == MGE_LINK_CLASS_SKY_MOON_SHADOW;
    }

    bool drawIsWaterPlane() {
        return drawClass() == MGE_LINK_CLASS_WATER_PLANE;
    }

    bool lightIsDirectional(uint32_t deviceIndex) {
        if (drawValid) {
            for (uint32_t i = 0; i != currentDraw.lightCount; ++i) {
                if (currentLights[i].deviceIndex == deviceIndex) {
                    return currentLights[i].type == MGE_LINK_LIGHT_DIRECTIONAL;
                }
            }
        }
        return false;
    }

    void compare(uint32_t fact, bool inferred, bool fromPacket) {
        if (drawValid) {
            count(fact, inferred, fromPacket);
        }
    }

    bool resolve(uint32_t fact, bool inferred, bool fromPacket) {
        if (!drawValid) {
            return inferred;
        }
        count(fact, inferred, fromPacket);
        return uses(fact) ? fromPacket : inferred;
    }

    bool resolveMainView(bool inferredMainView) {
        if (!sceneOpen) {
            return inferredMainView;
        }
        if (!sceneViewPending) {
            // A later view of the scene: a screen polygon, the sun glare. Compare only.
            count(MGE_LINK_FACT_LATER_VIEW, !inferredMainView, sceneIsUi());
            return inferredMainView;
        }
        sceneViewPending = false;
        const bool packetMainView = !sceneIsUi();
        count(MGE_LINK_FACT_UI_SCENE, !inferredMainView, !packetMainView);
        return uses(MGE_LINK_FACT_UI_SCENE) ? packetMainView : inferredMainView;
    }

    int resolveSceneCount(int inferred) {
        if (!sceneOpen) {
            return inferred;
        }
        const bool worldScene = currentScene.kind == MGE_LINK_SCENE_WORLD;
        count(MGE_LINK_FACT_SCENE_KIND, inferred == 0, worldScene);
        if (!uses(MGE_LINK_FACT_SCENE_KIND)) {
            return inferred;
        }
        return worldScene ? 0 : (inferred < 1 ? 1 : inferred);
    }

    void observeWorldDraw() {
        if (!connected) {
            return;
        }
        if (!drawValid) {
            ++counters.drawsWithoutPacket;
            return;
        }
        ++d3dDraws;
    }

    void observeDrawState(const RenderedState& rs, const FragmentState& frs, const LightState& lights) {
        stateForThisDraw = false;
        if (!connected) {
            return;
        }
        if (!stateValid) {
            // Both no: the draw has no state packet.
            count(MGE_LINK_FACT_STATE_DRAW, false, false);
            return;
        }
        const MgeLinkDrawStateV1& s = currentState;
        // The packet is for one D3D8 draw call.
        stateValid = false;
        stateForThisDraw = s.structSize >= sizeof(MgeLinkDrawStateV1);

        const bool drawEqual = s.primitiveType == static_cast<uint32_t>(rs.primType) && s.baseVertexIndex == rs.baseIndex
            && s.minIndex == rs.minIndex && s.vertexCount == rs.vertCount && s.startIndex == rs.startIndex
            && s.primitiveCount == rs.primCount;
        count(MGE_LINK_FACT_STATE_DRAW, true, drawEqual);

        const bool buffersEqual = s.vertexBuffer == static_cast<uint32_t>(reinterpret_cast<uintptr_t>(rs.vb))
            && s.vertexStride == rs.vbStride && s.vertexFormat == rs.fvf
            && s.indexBuffer == static_cast<uint32_t>(reinterpret_cast<uintptr_t>(rs.ib));
        count(MGE_LINK_FACT_STATE_BUFFERS, true, buffersEqual);

        if (!(s.flags & MGE_LINK_STATE_SKINNED)) {
            // The game sends its world matrix. With camera-relative rendering the matrix of a
            // node has a translation relative to the camera, and the proxy keeps the absolute
            // matrix that it makes from it.
            bool worldEqual = std::memcmp(s.world, &rs.worldTransforms[0], sizeof(s.world)) == 0;
            if (!worldEqual && CameraRelative::active()) {
                D3DXMATRIX absolute;
                CameraRelative::absoluteFromRelative(reinterpret_cast<const D3DMATRIX*>(s.world), &absolute);
                worldEqual = std::memcmp(&absolute, &rs.worldTransforms[0], sizeof(absolute)) == 0;
            }
            if (!worldEqual && loggedDifferences[MGE_LINK_FACT_STATE_TRANSFORM] < kLoggedDifferences) {
                const float* m = &rs.worldTransforms[0]._11;
                LOG::logline("-- Render link: world matrix: packet row 4 %g %g %g, proxy row 4 %g %g %g; row 1 %g %g %g against %g %g %g",
                    s.world[12], s.world[13], s.world[14], m[12], m[13], m[14], s.world[0], s.world[1], s.world[2], m[0], m[1], m[2]);
            }
            count(MGE_LINK_FACT_STATE_TRANSFORM, true, worldEqual);
        }
        if (s.structSize < sizeof(MgeLinkDrawStateV1)) {
            // A client of the first version: no render states and no material.
            return;
        }

        // The proxy keeps these states as bytes.
        const auto byte = [](uint32_t value) { return static_cast<BYTE>(value); };
        const bool blendEqual = byte(s.blendEnable) == rs.blendEnable && byte(s.sourceBlend) == rs.srcBlend
            && byte(s.destinationBlend) == rs.destBlend && byte(s.alphaTestEnable) == rs.alphaTest
            && byte(s.alphaFunction) == rs.alphaFunc && byte(s.alphaReference) == rs.alphaRef;
        if (!blendEqual && loggedDifferences[MGE_LINK_FACT_STATE_BLEND] < kLoggedDifferences) {
            LOG::logline("-- Render link: blend: packet %u %u %u test %u %u %u, proxy %u %u %u test %u %u %u",
                s.blendEnable, s.sourceBlend, s.destinationBlend, s.alphaTestEnable, s.alphaFunction, s.alphaReference,
                rs.blendEnable, rs.srcBlend, rs.destBlend, rs.alphaTest, rs.alphaFunc, rs.alphaRef);
        }
        count(MGE_LINK_FACT_STATE_BLEND, true, blendEqual);

        const bool depthEqual = s.depthWrite == rs.zWrite && s.cullMode == rs.cullMode && byte(s.fogEnable) == rs.useFog;
        if (!depthEqual && loggedDifferences[MGE_LINK_FACT_STATE_DEPTH] < kLoggedDifferences) {
            LOG::logline("-- Render link: depth: packet write %u cull %u fog %u, proxy write %u cull %u fog %u",
                s.depthWrite, s.cullMode, s.fogEnable, rs.zWrite, rs.cullMode, rs.useFog);
        }
        count(MGE_LINK_FACT_STATE_DEPTH, true, depthEqual);

        const bool lightingEqual = byte(s.lighting) == rs.useLighting && byte(s.diffuseMaterialSource) == rs.matSrcDiffuse
            && byte(s.emissiveMaterialSource) == rs.matSrcEmissive && s.vertexBlend == rs.vertexBlendState;
        if (!lightingEqual && loggedDifferences[MGE_LINK_FACT_STATE_LIGHTING] < kLoggedDifferences) {
            LOG::logline("-- Render link: lighting: packet on %u diffuse %u emissive %u blend %u, proxy on %u diffuse %u emissive %u blend %u",
                s.lighting, s.diffuseMaterialSource, s.emissiveMaterialSource, s.vertexBlend,
                rs.useLighting, rs.matSrcDiffuse, rs.matSrcEmissive, rs.vertexBlendState);
        }
        count(MGE_LINK_FACT_STATE_LIGHTING, true, lightingEqual);

        // The proxy keeps the diffuse, ambient and emissive colours, and the power in the
        // alpha of the emissive colour.
        const float* diffuse = s.material;
        const float* ambient = s.material + 4;
        const float* emissive = s.material + 12;
        const float power = s.material[16];
        const auto& m = frs.material;
        const bool materialEqual = std::memcmp(diffuse, &m.diffuse, 4 * sizeof(float)) == 0
            && std::memcmp(ambient, &m.ambient, 3 * sizeof(float)) == 0
            && std::memcmp(emissive, &m.emissive, 3 * sizeof(float)) == 0 && power == m.emissive.a;
        if (!materialEqual && loggedDifferences[MGE_LINK_FACT_STATE_MATERIAL] < kLoggedDifferences) {
            LOG::logline("-- Render link: material: packet diffuse %g %g %g %g power %g, proxy diffuse %g %g %g %g power %g",
                diffuse[0], diffuse[1], diffuse[2], diffuse[3], power, m.diffuse.r, m.diffuse.g, m.diffuse.b, m.diffuse.a, m.emissive.a);
        }
        count(MGE_LINK_FACT_STATE_MATERIAL, true, materialEqual);

        // The stages, to the first one that is off. The proxy reads no stage after that one.
        bool stagesEqual = true;
        uint32_t badStage = 0, badState = 0;
        for (uint32_t i = 0; i != MGE_LINK_STATE_STAGES && stagesEqual; ++i) {
            const uint32_t* p = s.stages[i];
            const auto& g = frs.stage[i];
            const uint32_t proxy[MGE_LINK_STATE_STAGE_STATES] = {
                g.colorOp, g.colorArg1, g.colorArg2, g.alphaOp, g.alphaArg1, g.alphaArg2, g.colorArg0, g.alphaArg0, g.resultArg,
                g.texcoordIndex, g.texTransformFlags, 0, 0, 0, 0, 0, 0,
            };
            // The proxy keeps the operations and arguments as bytes.
            for (uint32_t k = 0; k != 11 && stagesEqual; ++k) {
                const uint32_t packet = k < 9 ? static_cast<BYTE>(p[k]) : p[k];
                if (packet != proxy[k]) {
                    stagesEqual = false;
                    badStage = i;
                    badState = k;
                }
            }
            // The bump values are floats. The proxy reads them only for a bump stage.
            const bool bumpStage = g.colorOp == D3DTOP_BUMPENVMAP || g.colorOp == D3DTOP_BUMPENVMAPLUMINANCE;
            if (stagesEqual && bumpStage && (std::memcmp(&p[11], g.bumpEnvMat, 4 * sizeof(float)) != 0
                    || std::memcmp(&p[15], &g.bumpLumiScale, sizeof(float)) != 0
                    || std::memcmp(&p[16], &g.bumpLumiBias, sizeof(float)) != 0)) {
                stagesEqual = false;
                badStage = i;
                badState = 11;
            }
            if (g.colorOp == D3DTOP_DISABLE) {
                break;
            }
        }
        if (!stagesEqual && loggedDifferences[MGE_LINK_FACT_STATE_STAGES] < kLoggedDifferences) {
            const auto& g = frs.stage[badStage];
            LOG::logline("-- Render link: stage %u state %u: packet %u; proxy op %u args %u %u alpha %u %u %u coord %u flags %u",
                badStage, badState, s.stages[badStage][badState], g.colorOp, g.colorArg1, g.colorArg2, g.alphaOp, g.alphaArg1,
                g.alphaArg2, g.texcoordIndex, g.texTransformFlags);
        }
        count(MGE_LINK_FACT_STATE_STAGES, true, stagesEqual);

        // The lights: the same set, and for each light the values that the proxy keeps.
        bool lightsEqual = s.lightCount == lights.active.size();
        uint32_t badLight = 0;
        const char* why = "count";
        // The list of the game has the newest light first. The list of the proxy has the
        // oldest first, and the order has an effect on its shading.
        for (uint32_t i = 0; i != s.lightCount && lightsEqual; ++i) {
            if (lights.active[i] != stateLights[s.lightCount - 1 - i].deviceIndex) {
                lightsEqual = false;
                why = "order";
                badLight = lights.active[i];
            }
        }
        for (uint32_t i = 0; i != s.lightCount && lightsEqual; ++i) {
            const MgeLinkStateLightV1& l = stateLights[i];
            const D3DLIGHT8* d3d = reinterpret_cast<const D3DLIGHT8*>(l.d3dLight);
            badLight = l.deviceIndex;
            bool on = false;
            for (DWORD id : lights.active) {
                on = on || id == l.deviceIndex;
            }
            const auto found = lights.lights.find(l.deviceIndex);
            if (!on || found == lights.lights.end()) {
                lightsEqual = false;
                why = "not on in the proxy";
                break;
            }
            const auto& g = found->second;
            if (g.type != d3d->Type || std::memcmp(&g.diffuse, &d3d->Diffuse, sizeof(D3DCOLORVALUE)) != 0) {
                lightsEqual = false;
                why = "type or diffuse";
            } else if (d3d->Type == D3DLIGHT_POINT) {
                if (std::memcmp(&g.position, &d3d->Position, sizeof(D3DVECTOR)) != 0 || g.falloff.x != d3d->Attenuation0
                        || g.falloff.y != d3d->Attenuation1 || g.falloff.z != d3d->Attenuation2) {
                    lightsEqual = false;
                    why = "position or attenuation";
                }
            } else {
                // The proxy keeps the direction with length 1.
                D3DXVECTOR3 direction;
                D3DXVec3Normalize(&direction, reinterpret_cast<const D3DXVECTOR3*>(&d3d->Direction));
                if (std::memcmp(&g.position, &direction, sizeof(D3DVECTOR)) != 0 || g.ambient.x != d3d->Ambient.r
                        || g.ambient.y != d3d->Ambient.g || g.ambient.z != d3d->Ambient.b) {
                    lightsEqual = false;
                    why = "direction or ambient";
                }
            }
        }
        if (!lightsEqual && loggedDifferences[MGE_LINK_FACT_STATE_LIGHTS] < kLoggedDifferences) {
            LOG::logline("-- Render link: lights: %s; packet has %u lights, proxy %u; light %u",
                why, s.lightCount, static_cast<uint32_t>(lights.active.size()), badLight);
        }
        count(MGE_LINK_FACT_STATE_LIGHTS, true, lightsEqual);

        // The textures and the texture transforms, to the first stage that is off.
        bool texturesEqual = true, textureTransformsEqual = true;
        uint32_t badTexture = 0;
        for (uint32_t i = 0; i != MGE_LINK_STATE_STAGES; ++i) {
            const auto& g = frs.stage[i];
            if (texturesEqual && textureOfHandle(s.textures[i]) != g.texture) {
                texturesEqual = false;
                badTexture = i;
            }
            if (g.texTransformFlags != D3DTTFF_DISABLE
                    && std::memcmp(s.textureTransforms[i], &g.textureTransform, sizeof(D3DMATRIX)) != 0) {
                textureTransformsEqual = false;
            }
            if (g.colorOp == D3DTOP_DISABLE) {
                break;
            }
        }
        if (!texturesEqual && loggedDifferences[MGE_LINK_FACT_STATE_TEXTURES] < kLoggedDifferences) {
            LOG::logline("-- Render link: texture of stage %u: packet handle %08X, proxy texture %p",
                badTexture, s.textures[badTexture], frs.stage[badTexture].texture);
        }
        count(MGE_LINK_FACT_STATE_TEXTURES, true, texturesEqual);
        count(MGE_LINK_FACT_STATE_TEXTURE_TRANSFORMS, true, textureTransformsEqual);

        // The bones. With indexed skinning the proxy has a matrix palette and the packet
        // does not.
        if ((s.flags & MGE_LINK_STATE_SKINNED) && !(rs.fvf & D3DFVF_LASTBETA_UBYTE4)) {
            bool bonesEqual = true;
            for (uint32_t i = 0; i != worldCount(s) && bonesEqual; ++i) {
                const float* m = worldOfPacket(s, i);
                bonesEqual = std::memcmp(m, &rs.worldTransforms[i], sizeof(D3DMATRIX)) == 0;
                if (!bonesEqual && CameraRelative::active()) {
                    D3DXMATRIX absolute;
                    CameraRelative::absoluteFromRelative(reinterpret_cast<const D3DMATRIX*>(m), &absolute);
                    bonesEqual = std::memcmp(&absolute, &rs.worldTransforms[i], sizeof(absolute)) == 0;
                }
            }
            count(MGE_LINK_FACT_STATE_BONES, true, bonesEqual);
        }

        // The ambient colour. The proxy keeps the last colour that was not white.
        bool ambientEqual = ambientIsWhite(s.ambient) == lights.ambientWhite;
        if (ambientEqual && !lights.ambientWhite) {
            const RGBVECTOR colour = D3DCOLOR(s.ambient);
            ambientEqual = colour.r == lights.globalAmbient.r && colour.g == lights.globalAmbient.g
                && colour.b == lights.globalAmbient.b;
        }
        if (!ambientEqual && loggedDifferences[MGE_LINK_FACT_STATE_AMBIENT] < kLoggedDifferences) {
            LOG::logline("-- Render link: ambient: packet %08X, proxy white %u colour %g %g %g",
                s.ambient, lights.ambientWhite ? 1u : 0u, lights.globalAmbient.r, lights.globalAmbient.g, lights.globalAmbient.b);
        }
        count(MGE_LINK_FACT_STATE_AMBIENT, true, ambientEqual);
    }

    bool applyDrawState(RenderedState& rs, FragmentState& frs, LightState& lights, bool worldIsRelative) {
        if (!stateForThisDraw || !uses(MGE_LINK_FACT_STATE_DRAW)) {
            return false;
        }
        const MgeLinkDrawStateV1& s = currentState;
        const auto byte = [](uint32_t value) { return static_cast<BYTE>(value); };

        rs.primType = static_cast<D3DPRIMITIVETYPE>(s.primitiveType);
        rs.baseIndex = s.baseVertexIndex;
        rs.minIndex = s.minIndex;
        rs.vertCount = s.vertexCount;
        rs.startIndex = s.startIndex;
        rs.primCount = s.primitiveCount;
        rs.vb = reinterpret_cast<IDirect3DVertexBuffer9*>(static_cast<uintptr_t>(s.vertexBuffer));
        rs.vbStride = s.vertexStride;
        rs.fvf = s.vertexFormat;
        rs.ib = reinterpret_cast<IDirect3DIndexBuffer9*>(static_cast<uintptr_t>(s.indexBuffer));

        rs.zWrite = s.depthWrite;
        rs.cullMode = s.cullMode;
        rs.vertexBlendState = s.vertexBlend;
        rs.blendEnable = byte(s.blendEnable);
        rs.srcBlend = byte(s.sourceBlend);
        rs.destBlend = byte(s.destinationBlend);
        rs.alphaTest = byte(s.alphaTestEnable);
        rs.alphaFunc = byte(s.alphaFunction);
        rs.alphaRef = byte(s.alphaReference);
        rs.useLighting = byte(s.lighting);
        rs.useFog = byte(s.fogEnable);
        rs.matSrcDiffuse = byte(s.diffuseMaterialSource);
        rs.matSrcEmissive = byte(s.emissiveMaterialSource);

        // The material, as captureMaterial of the proxy keeps it: the power goes into the
        // alpha of the emissive colour.
        std::memcpy(&rs.diffuseMaterial, s.material, sizeof(D3DCOLORVALUE));
        std::memcpy(&frs.material.diffuse, s.material, sizeof(D3DCOLORVALUE));
        std::memcpy(&frs.material.ambient, s.material + 4, sizeof(D3DCOLORVALUE));
        std::memcpy(&frs.material.emissive, s.material + 12, sizeof(D3DCOLORVALUE));
        frs.material.emissive.a = s.material[16];

        for (uint32_t i = 0; i != MGE_LINK_STATE_STAGES; ++i) {
            const uint32_t* p = s.stages[i];
            auto& g = frs.stage[i];
            g.colorOp = byte(p[0]);
            g.colorArg1 = byte(p[1]);
            g.colorArg2 = byte(p[2]);
            g.alphaOp = byte(p[3]);
            g.alphaArg1 = byte(p[4]);
            g.alphaArg2 = byte(p[5]);
            g.colorArg0 = byte(p[6]);
            g.alphaArg0 = byte(p[7]);
            g.resultArg = byte(p[8]);
            g.texcoordIndex = p[9];
            g.texTransformFlags = p[10];
            std::memcpy(g.bumpEnvMat, &p[11], 4 * sizeof(float));
            std::memcpy(&g.bumpLumiScale, &p[15], sizeof(float));
            std::memcpy(&g.bumpLumiBias, &p[16], sizeof(float));
        }

        // The textures and the texture transforms, to the first stage that is off. The
        // client does not have the stages after that one.
        for (uint32_t i = 0; i != MGE_LINK_STATE_STAGES; ++i) {
            auto& g = frs.stage[i];
            g.texture = textureOfHandle(s.textures[i]);
            if (i == 0) {
                rs.texture = g.texture;
            }
            if (g.texTransformFlags != D3DTTFF_DISABLE) {
                std::memcpy(&g.textureTransform, s.textureTransforms[i], sizeof(D3DMATRIX));
            }
            if (g.colorOp == D3DTOP_DISABLE) {
                break;
            }
        }

        // The world matrices, as captureTransform of the proxy keeps them.
        if (!(rs.fvf & D3DFVF_LASTBETA_UBYTE4)) {
            for (uint32_t i = 0; i != worldCount(s); ++i) {
                const auto m = reinterpret_cast<const D3DMATRIX*>(worldOfPacket(s, i));
                if (CameraRelative::active()) {
                    D3DXMATRIX world, absolute;
                    if (worldIsRelative) {
                        world = *m;
                        CameraRelative::absoluteFromRelative(m, &absolute);
                    } else {
                        CameraRelative::relativeWorld(m, &world);
                        absolute = *m;
                    }
                    rs.worldTransforms[i] = absolute;
                    CameraRelative::multiplyWorldView(&world, &rs.viewTransform, &rs.worldViewTransforms[i]);
                } else {
                    rs.worldTransforms[i] = *m;
                    D3DXMatrixMultiply(&rs.worldViewTransforms[i], static_cast<const D3DXMATRIX*>(m), &rs.viewTransform);
                }
            }
        }

        lights.ambientWhite = ambientIsWhite(s.ambient);
        if (!lights.ambientWhite) {
            const RGBVECTOR colour = D3DCOLOR(s.ambient);
            lights.globalAmbient.r = colour.r;
            lights.globalAmbient.g = colour.g;
            lights.globalAmbient.b = colour.b;
        }

        // The lights that are on, oldest first as the proxy keeps them. The radius of a
        // light and its place in view space stay as the proxy has them.
        lights.active.clear();
        for (uint32_t i = s.lightCount; i-- != 0;) {
            const MgeLinkStateLightV1& l = stateLights[i];
            const D3DLIGHT8* d3d = reinterpret_cast<const D3DLIGHT8*>(l.d3dLight);
            lights.active.push_back(l.deviceIndex);
            auto& g = lights.lights[l.deviceIndex];
            g.type = d3d->Type;
            g.diffuse = d3d->Diffuse;
            if (d3d->Type == D3DLIGHT_POINT) {
                g.position = d3d->Position;
                g.falloff.x = d3d->Attenuation0;
                g.falloff.y = d3d->Attenuation1;
                g.falloff.z = d3d->Attenuation2;
            } else {
                D3DXVec3Normalize(reinterpret_cast<D3DXVECTOR3*>(&g.position), reinterpret_cast<const D3DXVECTOR3*>(&d3d->Direction));
                g.ambient.x = d3d->Ambient.r;
                g.ambient.y = d3d->Ambient.g;
                g.ambient.z = d3d->Ambient.b;
            }
        }
        return true;
    }

    void noteLightRadius(const void* light, int recordRadius, int attachRadius) {
        if (lightRadii.size() >= kMaxLightRadii) {
            lightRadii.clear();
        }
        const uint32_t key = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(light));
        const auto found = lightRadii.find(key);
        // Some callers of the game pass the radius that is in the light object, which is the
        // attach radius of an earlier call. That value is not the radius of the record.
        if (found != lightRadii.end() && found->second.attach == recordRadius && attachRadius == recordRadius) {
            return;
        }
        lightRadii[key] = { recordRadius, attachRadius };
    }

    float resolveLightRadius(uint32_t deviceIndex, float inferred) {
        if (!drawValid) {
            return inferred;
        }
        const MgeLinkLightV1* light = nullptr;
        for (uint32_t i = 0; i != currentDraw.lightCount; ++i) {
            if (currentLights[i].deviceIndex == deviceIndex && currentLights[i].type == MGE_LINK_LIGHT_POINT) {
                light = &currentLights[i];
            }
        }
        // The packet has the radius that the game keeps in the light object. A light that the
        // game never tested against an object has 0 there.
        if (!light || light->radius <= 0.0f) {
            return inferred;
        }
        // The shim of the light fade raised that radius. The table has the radius of the
        // record, when the entry is of this light and not of an old light at the same address.
        float direct = light->radius;
        const auto found = lightRadii.find(light->light);
        if (found != lightRadii.end() && static_cast<float>(found->second.attach) == light->radius) {
            direct = static_cast<float>(found->second.record);
        }

        // Yes means a radius that is not 0. When the two radii are different, the larger one
        // gets the count.
        if (std::fabs(inferred - direct) <= kRadiusTolerance) {
            count(MGE_LINK_FACT_LIGHT_RADIUS, true, true);
        } else {
            if (loggedDifferences[MGE_LINK_FACT_LIGHT_RADIUS] < kLoggedDifferences) {
                // The name of a NiObjectNET is a C string pointer at offset 8.
                const char* name = *reinterpret_cast<const char* const*>(light->light + 8);
                LOG::logline("-- Render link: light %u '%s' radius: MGE XE %g, game %g (in the light object %g)",
                    deviceIndex, name ? name : "", inferred, direct, light->radius);
            }
            count(MGE_LINK_FACT_LIGHT_RADIUS, inferred > direct, inferred <= direct);
        }
        return uses(MGE_LINK_FACT_LIGHT_RADIUS) ? direct : inferred;
    }

    bool usesWaterPlaneFact() {
        return uses(MGE_LINK_FACT_WATER_PLANE);
    }

    void noteWaterMark(float original) {
        waterShininess = original;
        waterMarked = true;
    }

    void noteMoonMark(const float original[2]) {
        // The proxy marks the moons again when it makes its shaders again. Then the value
        // that was there is the mark, and the first value stays.
        if (!moonMarked) {
            moonShininess[0] = original[0];
            moonShininess[1] = original[1];
            moonMarked = true;
        }
        moonMarkRemoved = false;
    }
}

extern "C" const MgeRenderLinkHostV1* __cdecl MGE_RenderLinkConnect(uint32_t clientVersion) {
    if (clientVersion == 0) {
        return nullptr;
    }
    connected = true;
    return &host;
}
