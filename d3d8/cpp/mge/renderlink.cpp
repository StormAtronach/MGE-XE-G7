// The host side of the render link. See mge_render_link.h for the contract and
// docs/architecture/render-link.md for the design.

#include "renderlink.h"

#include "camerarelative.h"
#include "ffeshader.h"
#include "mwbridge.h"
#include "mged3d8device.h"
#include "bc7format.h"
#include "proxydx/d3d8device.h"
#include "proxydx/d3d8texture.h"
#include "support/log.h"

#include <algorithm>
#include <atomic>
#include <vector>
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
        | (1u << MGE_LINK_FACT_SCENE_KIND) | (1u << MGE_LINK_FACT_MOON_SHADOW) | (1u << MGE_LINK_FACT_STATE_DRAW)
        | (1u << MGE_LINK_FACT_DEVICE_RENDER_STATES) | (1u << MGE_LINK_FACT_DEVICE_STAGE_STATES)
        | (1u << MGE_LINK_FACT_DEVICE_TEXTURES) | (1u << MGE_LINK_FACT_DEVICE_BUFFERS)
        | (1u << MGE_LINK_FACT_DEVICE_TRANSFORMS) | (1u << MGE_LINK_FACT_DEVICE_LIGHTS)
        | (1u << MGE_LINK_FACT_CLIENT_CAMERA_SPACE);
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

    // The device of the game. textureCreate can run on a thread that loads a cell.
    std::atomic<ProxyDevice*> linkDevice{nullptr};
    std::atomic<uint32_t> texturesMade{0}, texturesFailed{0};

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
        CameraRelative::setExternalOrigin((currentScene.flags & MGE_LINK_SCENE_HAS_CAMERA_ORIGIN) ? currentScene.cameraOrigin : nullptr);
        sceneViewPending = true;
        sceneOpen = true;
        drawValid = false;
        ++counters.scenes;
    }

    // The state of the device, as the D3D8 handlers of the proxy got it. A value is not
    // known before its first call.
    struct DeviceValue {
        uint32_t value;
        bool known;

        bool is(uint32_t other) const {
            return known && value == other;
        }
        void set(uint32_t next) {
            value = next;
            known = true;
        }
    };
    DeviceValue deviceRenderStates[256];
    DeviceValue deviceStageStates[MGE_LINK_STATE_STAGES][32];
    DeviceValue deviceTextures[MGE_LINK_STATE_STAGES];
    DeviceValue deviceVertexBuffer, deviceStride, deviceFormat, deviceIndexBuffer, deviceBaseIndex;
    // The material, as the game sent it.
    D3DMATERIAL8 deviceMaterial = {};
    bool deviceMaterialKnown = false;
    // The world matrices 0 to 3 with their space, and the texture transforms, as the game
    // sent them.
    struct DeviceMatrix {
        D3DMATRIX matrix;
        bool known;

        bool is(const float* other) const {
            return known && std::memcmp(&matrix, other, sizeof(matrix)) == 0;
        }
    };
    DeviceMatrix deviceTextureTransforms[MGE_LINK_STATE_STAGES], deviceWorlds[4];
    bool deviceWorldRelative[4] = {};
    // The lights, as the game sent them, and the lights that are on, the oldest first.
    std::unordered_map<uint32_t, D3DLIGHT8> deviceLights;
    std::vector<uint32_t> deviceLightsOn;

    // The render states of a state packet: the D3D8 state and the value.
    struct PacketRenderState {
        uint32_t state;
        uint32_t value;
    };
    const uint32_t kPacketRenderStates = 24;
    void renderStatesOfPacket(const MgeLinkDrawStateV1& s, PacketRenderState out[kPacketRenderStates]) {
        const PacketRenderState list[kPacketRenderStates] = {
            { D3DRS_ZENABLE, s.depthEnable }, { D3DRS_ZWRITEENABLE, s.depthWrite }, { D3DRS_ALPHATESTENABLE, s.alphaTestEnable },
            { D3DRS_SRCBLEND, s.sourceBlend }, { D3DRS_DESTBLEND, s.destinationBlend }, { D3DRS_CULLMODE, s.cullMode },
            { D3DRS_ZFUNC, s.depthFunction }, { D3DRS_ALPHAREF, s.alphaReference }, { D3DRS_ALPHAFUNC, s.alphaFunction },
            { D3DRS_ALPHABLENDENABLE, s.blendEnable }, { D3DRS_FOGENABLE, s.fogEnable },
            { D3DRS_STENCILENABLE, s.stencil[0] }, { D3DRS_STENCILFAIL, s.stencil[1] }, { D3DRS_STENCILZFAIL, s.stencil[2] },
            { D3DRS_STENCILPASS, s.stencil[3] }, { D3DRS_STENCILFUNC, s.stencil[4] }, { D3DRS_STENCILREF, s.stencil[5] },
            { D3DRS_STENCILMASK, s.stencil[6] }, { D3DRS_STENCILWRITEMASK, s.stencil[7] },
            { D3DRS_LIGHTING, s.lighting }, { D3DRS_AMBIENT, s.ambient }, { D3DRS_DIFFUSEMATERIALSOURCE, s.diffuseMaterialSource },
            { D3DRS_EMISSIVEMATERIALSOURCE, s.emissiveMaterialSource }, { D3DRS_VERTEXBLEND, s.vertexBlend },
        };
        std::memcpy(out, list, sizeof(list));
    }

    // The D3D8 numbers of the stage states and of the sampler states of a state packet.
    const uint32_t kStageStates[MGE_LINK_STATE_STAGE_STATES] = { 1, 2, 3, 4, 5, 6, 26, 27, 28, 11, 24, 7, 8, 9, 10, 22, 23 };
    const uint32_t kSamplerStates[MGE_LINK_STATE_SAMPLER_STATES] = { 13, 14, 16, 17, 18 };
    const uint32_t kColorOp = 0;

    bool usesFact(uint32_t fact) {
        return (usedFacts >> fact) & 1u;
    }

    // Puts the parts of a state packet that the client asked for on the device, through the
    // D3D8 handlers of the proxy. A value that the device has is not sent again.
    void applyToDevice(const MgeLinkDrawStateV1& s) {
        auto device = static_cast<MGEProxyDevice*>(linkDevice.load());
        const uint32_t kinds = (1u << MGE_LINK_FACT_DEVICE_RENDER_STATES) | (1u << MGE_LINK_FACT_DEVICE_STAGE_STATES)
            | (1u << MGE_LINK_FACT_DEVICE_TEXTURES) | (1u << MGE_LINK_FACT_DEVICE_BUFFERS);
        if (!device || !(usedFacts & (kinds | (1u << MGE_LINK_FACT_DEVICE_TRANSFORMS) | (1u << MGE_LINK_FACT_DEVICE_LIGHTS)))
                || s.structSize < sizeof(MgeLinkDrawStateV1)) {
            return;
        }
        if (usesFact(MGE_LINK_FACT_DEVICE_TRANSFORMS)) {
            // A skinned draw: all four matrices, because the packet does not tell how many
            // the vertex format needs.
            const uint32_t worlds = (s.flags & MGE_LINK_STATE_SKINNED) ? 4 : 1;
            for (uint32_t i = 0; i != worlds; ++i) {
                const float* m = worldOfPacket(s, i);
                const bool relative = (s.worldRelativeMask >> i) & 1u;
                if (!deviceWorlds[i].is(m) || deviceWorldRelative[i] != relative) {
                    // The handler takes the space of the matrix from this flag.
                    CameraRelative::setWorldRelative(relative);
                    device->MGEProxyDevice::SetTransform(D3DTS_WORLDMATRIX(i), reinterpret_cast<const D3DMATRIX*>(m));
                }
            }
            const uint32_t kTransformFlags = 10;
            for (uint32_t stage = 0; stage != MGE_LINK_STATE_STAGES; ++stage) {
                if (s.stages[stage][kTransformFlags] != D3DTTFF_DISABLE && s.stages[stage][kTransformFlags] != MGE_LINK_STATE_UNKNOWN
                        && !deviceTextureTransforms[stage].is(s.textureTransforms[stage])) {
                    device->MGEProxyDevice::SetTransform(static_cast<D3DTRANSFORMSTATETYPE>(D3DTS_TEXTURE0 + stage),
                        reinterpret_cast<const D3DMATRIX*>(s.textureTransforms[stage]));
                }
                if (s.stages[stage][kColorOp] == D3DTOP_DISABLE) {
                    break;
                }
            }
        }
        if (usesFact(MGE_LINK_FACT_DEVICE_LIGHTS)) {
            // The values of the lights of the packet.
            for (uint32_t i = 0; i != s.lightCount; ++i) {
                const MgeLinkStateLightV1& l = stateLights[i];
                const auto found = deviceLights.find(l.deviceIndex);
                if (found == deviceLights.end() || std::memcmp(&found->second, l.d3dLight, sizeof(D3DLIGHT8)) != 0) {
                    device->MGEProxyDevice::SetLight(l.deviceIndex, reinterpret_cast<const D3DLIGHT8*>(l.d3dLight));
                }
            }
            // The lights that are on, in the order of the game: the packet has the newest
            // first. When the list of the device is different, all go off and on again.
            bool sameList = deviceLightsOn.size() == s.lightCount;
            for (uint32_t i = 0; i != s.lightCount && sameList; ++i) {
                sameList = deviceLightsOn[i] == stateLights[s.lightCount - 1 - i].deviceIndex;
            }
            if (!sameList) {
                const std::vector<uint32_t> on = deviceLightsOn;
                for (uint32_t index : on) {
                    device->MGEProxyDevice::LightEnable(index, FALSE);
                }
                for (uint32_t i = s.lightCount; i-- != 0;) {
                    device->MGEProxyDevice::LightEnable(stateLights[i].deviceIndex, TRUE);
                }
            }
        }
        if (usesFact(MGE_LINK_FACT_DEVICE_RENDER_STATES)) {
            PacketRenderState list[kPacketRenderStates];
            renderStatesOfPacket(s, list);
            for (const auto& item : list) {
                if (item.value != MGE_LINK_STATE_UNKNOWN && !deviceRenderStates[item.state].is(item.value)) {
                    device->MGEProxyDevice::SetRenderState(static_cast<D3DRENDERSTATETYPE>(item.state), item.value);
                }
            }
        }
        // The material is a part of the render state kind.
        if (usesFact(MGE_LINK_FACT_DEVICE_RENDER_STATES)
                && (!deviceMaterialKnown || std::memcmp(&deviceMaterial, s.material, sizeof(D3DMATERIAL8)) != 0)) {
            device->MGEProxyDevice::SetMaterial(reinterpret_cast<const D3DMATERIAL8*>(s.material));
        }
        // The stages, to the first one that is off. The client does not have the others.
        for (uint32_t stage = 0; stage != MGE_LINK_STATE_STAGES; ++stage) {
            if (usesFact(MGE_LINK_FACT_DEVICE_STAGE_STATES)) {
                const auto send = [&](uint32_t state, uint32_t value) {
                    if (value != MGE_LINK_STATE_UNKNOWN && !deviceStageStates[stage][state].is(value)) {
                        device->MGEProxyDevice::SetTextureStageState(stage, static_cast<D3DTEXTURESTAGESTATETYPE>(state), value);
                    }
                };
                for (uint32_t k = 0; k != MGE_LINK_STATE_STAGE_STATES; ++k) {
                    send(kStageStates[k], s.stages[stage][k]);
                }
                for (uint32_t k = 0; k != MGE_LINK_STATE_SAMPLER_STATES; ++k) {
                    send(kSamplerStates[k], s.samplers[stage][k]);
                }
            }
            if (usesFact(MGE_LINK_FACT_DEVICE_TEXTURES) && !deviceTextures[stage].is(s.textures[stage])) {
                device->MGEProxyDevice::SetTexture(stage, reinterpret_cast<IDirect3DBaseTexture8*>(static_cast<uintptr_t>(s.textures[stage])));
            }
            if (s.stages[stage][kColorOp] == D3DTOP_DISABLE) {
                break;
            }
        }
        if (usesFact(MGE_LINK_FACT_DEVICE_BUFFERS)) {
            if (!deviceVertexBuffer.is(s.vertexBuffer) || !deviceStride.is(s.vertexStride)) {
                device->MGEProxyDevice::SetStreamSource(0, reinterpret_cast<IDirect3DVertexBuffer8*>(static_cast<uintptr_t>(s.vertexBuffer)), s.vertexStride);
            }
            if (!deviceFormat.is(s.vertexFormat)) {
                device->MGEProxyDevice::SetVertexShader(s.vertexFormat);
            }
            if (!deviceIndexBuffer.is(s.indexBuffer) || !deviceBaseIndex.is(s.baseVertexIndex)) {
                device->MGEProxyDevice::SetIndices(reinterpret_cast<IDirect3DIndexBuffer8*>(static_cast<uintptr_t>(s.indexBuffer)), s.baseVertexIndex);
            }
        }
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
        applyToDevice(currentState);
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
            // The textures have their own counts: another thread can make a texture. A reset
            // does not clear them: most textures come during a change of cell.
            counters.facts[MGE_LINK_FACT_TEXTURE_CREATE].bothYes = texturesMade;
            counters.facts[MGE_LINK_FACT_TEXTURE_CREATE].hostOnly = texturesFailed;
            std::memcpy(out, &counters, size);
            out->structSize = size;
        }
        if (reset) {
            counters = { sizeof(MgeLinkCountersV1) };
        }
    }

    uint32_t __cdecl useFacts(uint32_t factMask) {
        const uint32_t before = usedFacts;
        usedFacts = factMask & kUsableFacts;
        // The matrices of the client come only in the state packets.
        if (!(usedFacts & (1u << MGE_LINK_FACT_DEVICE_TRANSFORMS))) {
            usedFacts &= ~(1u << MGE_LINK_FACT_CLIENT_CAMERA_SPACE);
        }
        CameraRelative::setExternalOwner((usedFacts >> MGE_LINK_FACT_CLIENT_CAMERA_SPACE) & 1u);
        // The client takes the lights back: all lights go off, and the client puts its
        // lights on again.
        const uint32_t lightsBit = 1u << MGE_LINK_FACT_DEVICE_LIGHTS;
        auto device = static_cast<MGEProxyDevice*>(linkDevice.load());
        if ((before & lightsBit) && !(usedFacts & lightsBit) && device) {
            const std::vector<uint32_t> on = deviceLightsOn;
            for (uint32_t index : on) {
                device->MGEProxyDevice::LightEnable(index, FALSE);
            }
        }
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

    // Copies the pixels of each level into a texture.
    bool fillTexture(IDirect3DTexture9* texture, const MgeLinkTextureV1& t) {
        // The NiPixelFormat numbers 6 to 8 are the compressed formats.
        const bool compressed = t.pixelFormat >= 6 && t.pixelFormat <= 8;
        for (uint32_t level = 0; level != t.levels; ++level) {
            D3DLOCKED_RECT locked;
            if (FAILED(texture->LockRect(level, &locked, nullptr, 0))) {
                return false;
            }
            const uint8_t* source = t.pixels + t.levelOffsets[level];
            const uint32_t rowBytes = t.levelWidths[level] * t.bytesPerPixel;
            if (compressed || static_cast<uint32_t>(locked.Pitch) == rowBytes) {
                std::memcpy(locked.pBits, source, t.levelOffsets[level + 1] - t.levelOffsets[level]);
            } else {
                auto row = static_cast<uint8_t*>(locked.pBits);
                for (uint32_t y = 0; y != t.levelHeights[level]; ++y, row += locked.Pitch, source += rowBytes) {
                    std::memcpy(row, source, rowBytes);
                }
            }
            texture->UnlockRect(level);
        }
        return true;
    }

    IDirect3DTexture9* makeTexture(IDirect3DDevice9* device, const MgeLinkTextureV1& t) {
        D3DFORMAT format = static_cast<D3DFORMAT>(t.d3dFormat);
        // The mark that the DDS reader of the proxy puts on BC7 data. See mwtextureloader.cpp.
        if (t.pixelFormat == 8 && t.pixelFormatTag == static_cast<uint32_t>(MGE_D3DFMT_BC7)) {
            format = MGE_D3DFMT_BC7;
        }
        // The pixels go into a texture that the game can lock. For a texture that does not
        // change, they then go from there into a texture in the default pool.
        const bool dynamic = (t.flags & MGE_LINK_TEXTURE_DYNAMIC) != 0;
        IDirect3DTexture9* first = nullptr;
        if (FAILED(device->CreateTexture(t.width, t.height, t.levels, 0, format, dynamic ? D3DPOOL_MANAGED : D3DPOOL_SYSTEMMEM, &first, nullptr))) {
            return nullptr;
        }
        if (!fillTexture(first, t)) {
            first->Release();
            return nullptr;
        }
        if (dynamic) {
            return first;
        }
        IDirect3DTexture9* texture = nullptr;
        if (SUCCEEDED(device->CreateTexture(t.width, t.height, t.levels, 0, format, D3DPOOL_DEFAULT, &texture, nullptr))) {
            device->UpdateTexture(first, texture);
        }
        first->Release();
        return texture;
    }

    uint32_t __cdecl textureCreate(const MgeLinkTextureV1* texture) {
        ProxyDevice* device = linkDevice.load();
        if (!device || !texture || texture->structSize < sizeof(MgeLinkTextureV1) || !texture->pixels || texture->levels == 0) {
            ++texturesFailed;
            return 0;
        }
        IDirect3DTexture9* real = makeTexture(device->realDevice, *texture);
        if (!real) {
            ++texturesFailed;
            LOG::logline("-- Render link: texture not made: %u x %u, %u levels, D3D format %u, pixel format %u",
                texture->width, texture->height, texture->levels, texture->d3dFormat, texture->pixelFormat);
            return 0;
        }
        ++texturesMade;
        return static_cast<uint32_t>(reinterpret_cast<uintptr_t>(device->factoryProxyTexture(real)));
    }

    uint32_t __cdecl cameraSpace(double origin[3]) {
        return origin && CameraRelative::activeOrigin(origin) ? 1u : 0u;
    }

    uint32_t __cdecl worldSpace() {
        return CameraRelative::peekWorldRelative() ? 1u : 0u;
    }

    const MgeRenderLinkHostV1 host = {
        sizeof(MgeRenderLinkHostV1),
        MGE_RENDER_LINK_VERSION,
        MGE_RENDER_LINK_CAP_COMPARE | MGE_RENDER_LINK_CAP_USE_FACTS | MGE_RENDER_LINK_CAP_DRAW_STATE
            | MGE_RENDER_LINK_CAP_TEXTURES | MGE_RENDER_LINK_CAP_WORLD_SPACE | MGE_RENDER_LINK_CAP_CAMERA_SPACE,
        0,
        sceneBegin,
        sceneEnd,
        frameEvent,
        draw,
        readCounters,
        useFacts,
        drawState,
        textureCreate,
        worldSpace,
        cameraSpace,
    };
}

namespace RenderLink {
    void noteRenderState(uint32_t state, uint32_t value) {
        if (state < 256) {
            deviceRenderStates[state].set(value);
        }
    }

    void noteStageState(uint32_t stage, uint32_t state, uint32_t value) {
        if (stage < MGE_LINK_STATE_STAGES && state < 32) {
            deviceStageStates[stage][state].set(value);
        }
    }

    void noteTexture(uint32_t stage, const void* texture) {
        if (stage < MGE_LINK_STATE_STAGES) {
            deviceTextures[stage].set(static_cast<uint32_t>(reinterpret_cast<uintptr_t>(texture)));
        }
    }

    void noteStreamSource(const void* buffer, uint32_t stride) {
        deviceVertexBuffer.set(static_cast<uint32_t>(reinterpret_cast<uintptr_t>(buffer)));
        deviceStride.set(stride);
    }

    void noteIndices(const void* buffer, uint32_t baseVertexIndex) {
        deviceIndexBuffer.set(static_cast<uint32_t>(reinterpret_cast<uintptr_t>(buffer)));
        deviceBaseIndex.set(baseVertexIndex);
    }

    void noteVertexFormat(uint32_t format) {
        deviceFormat.set(format);
    }

    void noteMaterial(const void* material) {
        deviceMaterial = *static_cast<const D3DMATERIAL8*>(material);
        deviceMaterialKnown = true;
    }

    void noteTransform(uint32_t state, const void* matrix, bool worldIsRelative) {
        DeviceMatrix* to = nullptr;
        if (state >= D3DTS_WORLDMATRIX(0) && state < D3DTS_WORLDMATRIX(4)) {
            to = &deviceWorlds[state - D3DTS_WORLDMATRIX(0)];
            deviceWorldRelative[state - D3DTS_WORLDMATRIX(0)] = worldIsRelative;
        } else if (state >= D3DTS_TEXTURE0 && state <= D3DTS_TEXTURE7) {
            to = &deviceTextureTransforms[state - D3DTS_TEXTURE0];
        }
        if (to) {
            to->matrix = *static_cast<const D3DMATRIX*>(matrix);
            to->known = true;
        }
    }

    void noteLight(uint32_t index, const void* light) {
        deviceLights[index] = *static_cast<const D3DLIGHT8*>(light);
    }

    void noteLightEnable(uint32_t index, bool on) {
        const auto found = std::find(deviceLightsOn.begin(), deviceLightsOn.end(), index);
        if (on && found == deviceLightsOn.end()) {
            deviceLightsOn.push_back(index);
        } else if (!on && found != deviceLightsOn.end()) {
            deviceLightsOn.erase(found);
        }
    }

    void setDevice(ProxyDevice* device) {
        linkDevice = device;
    }

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
                if (!bonesEqual && loggedDifferences[MGE_LINK_FACT_STATE_BONES] < kLoggedDifferences) {
                    const float* g = &rs.worldTransforms[i]._11;
                    LOG::logline("-- Render link: bone %u of %u (vertex blend %u, relative mask %u): packet row 1 %g %g %g row 4 %g %g %g, proxy row 1 %g %g %g row 4 %g %g %g",
                        i, worldCount(s), s.vertexBlend, s.worldRelativeMask, m[0], m[1], m[2], m[12], m[13], m[14], g[0], g[1], g[2], g[12], g[13], g[14]);
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

        // The depth and stencil states, and the sampler states of the stages that are on.
        // A value that the device or the game does not know is not compared.
        const auto same = [](const DeviceValue& device, uint32_t packet) {
            return !device.known || packet == MGE_LINK_STATE_UNKNOWN || device.value == packet;
        };
        bool depthStencilEqual = same(deviceRenderStates[D3DRS_ZENABLE], s.depthEnable) && same(deviceRenderStates[D3DRS_ZFUNC], s.depthFunction);
        for (uint32_t i = 0; i != 8; ++i) {
            depthStencilEqual = depthStencilEqual && same(deviceRenderStates[D3DRS_STENCILENABLE + i], s.stencil[i]);
        }
        if (!depthStencilEqual && loggedDifferences[MGE_LINK_FACT_STATE_DEPTH_STENCIL] < kLoggedDifferences) {
            LOG::logline("-- Render link: depth and stencil: packet enable %u function %u stencil %u, device %u %u %u",
                s.depthEnable, s.depthFunction, s.stencil[0], deviceRenderStates[D3DRS_ZENABLE].value,
                deviceRenderStates[D3DRS_ZFUNC].value, deviceRenderStates[D3DRS_STENCILENABLE].value);
        }
        count(MGE_LINK_FACT_STATE_DEPTH_STENCIL, true, depthStencilEqual);

        bool samplersEqual = true;
        for (uint32_t stage = 0; stage != MGE_LINK_STATE_STAGES && samplersEqual; ++stage) {
            for (uint32_t k = 0; k != MGE_LINK_STATE_SAMPLER_STATES && samplersEqual; ++k) {
                samplersEqual = same(deviceStageStates[stage][kSamplerStates[k]], s.samplers[stage][k]);
                if (!samplersEqual && loggedDifferences[MGE_LINK_FACT_STATE_SAMPLERS] < kLoggedDifferences) {
                    LOG::logline("-- Render link: sampler state %u of stage %u: packet %u, device %u",
                        kSamplerStates[k], stage, s.samplers[stage][k], deviceStageStates[stage][kSamplerStates[k]].value);
                }
            }
            if (frs.stage[stage].colorOp == D3DTOP_DISABLE) {
                break;
            }
        }
        count(MGE_LINK_FACT_STATE_SAMPLERS, true, samplersEqual);
    }

    bool applyDrawState(RenderedState& rs, FragmentState& frs, LightState& lights) {
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
                    if ((s.worldRelativeMask >> i) & 1u) {
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
