
#include "mwsenative.h"
#include "support/log.h"

#include <cstdint>
#include "MWSENativeApi.h"

namespace MwseNative {
    static const char* const MODULE = "MGE XE";

    static const MwseNativeApiV1* table = nullptr;
    static void (*uiConfiguredListener)() = nullptr;
    static void (__cdecl* gameIdsResolvedListener)() = nullptr;

    bool init(HMODULE mwse) {
        auto getNativeApi = reinterpret_cast<MwseGetNativeApi>(GetProcAddress(mwse, "MWSE_GetNativeApi"));
        const MwseNativeApiV1* found = getNativeApi ? getNativeApi(MWSE_NATIVE_API_VERSION) : nullptr;
        if (!found || found->structSize < sizeof(MwseNativeApiV1)) {
            LOG::logline("-- MWSE has no native ABI of version %u: MGE XE changes the engine code itself", MWSE_NATIVE_API_VERSION);
            return false;
        }
        table = found;
        LOG::logline("-- MWSE native ABI: MWSE %s", table->mwseVersion);
        return true;
    }

    // A request of MWSE; `what` names it in the log of a refusal.
    static bool request(uint32_t id, const void* packet, const char* what) {
        if (!table) {
            return false;
        }
        auto status = table->request(MODULE, id, packet);
        if (status != MWSE_NATIVE_OK) {
            LOG::logline("!! MWSE native ABI: %s was refused (status %d, see MWSE.log); MGE XE changes the engine code itself", what, int(status));
            return false;
        }
        return true;
    }

    static bool claim(uint32_t id, void* function, const char* what) {
        if (!table) {
            return false;
        }
        MwseNativeHandle handle = 0;
        void* engineFunction = nullptr;
        auto status = table->claim(MODULE, id, function, &engineFunction, &handle);
        if (status != MWSE_NATIVE_OK) {
            LOG::logline("!! MWSE native ABI: %s was refused (status %d, see MWSE.log); MGE XE changes the engine code itself", what, int(status));
            return false;
        }
        return true;
    }

    static bool subscribe(uint32_t id, MwseNativeListener listener, const char* what) {
        if (!table) {
            return false;
        }
        MwseNativeHandle handle = 0;
        auto status = table->subscribe(MODULE, id, listener, nullptr, 0, &handle);
        if (status != MWSE_NATIVE_OK) {
            LOG::logline("!! MWSE native ABI: %s was refused (status %d, see MWSE.log); MGE XE changes the engine code itself", what, int(status));
            return false;
        }
        return true;
    }

    bool requestUIScale(float scale) {
        MwseNativeUIScaleRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        packet.scale = scale;
        return request(MWSE_NATIVE_REQUEST_UI_SCALE, &packet, "the UI scale");
    }

    static void __cdecl onUIConfigured(void*, const void*) {
        if (uiConfiguredListener) {
            uiConfiguredListener();
        }
    }

    bool listenUIConfigured(void (*listener)()) {
        uiConfiguredListener = listener;
        if (!subscribe(MWSE_NATIVE_EVENT_UI_CONFIGURED, onUIConfigured, "the event of the UI configuration")) {
            uiConfiguredListener = nullptr;
            return false;
        }
        return true;
    }

    static void __cdecl onGameIdsResolved(void*, const void*) {
        if (gameIdsResolvedListener) {
            gameIdsResolvedListener();
        }
    }

    bool listenGameIdsResolved(void (__cdecl* listener)()) {
        gameIdsResolvedListener = listener;
        if (!subscribe(MWSE_NATIVE_EVENT_GAME_IDS_RESOLVED, onGameIdsResolved, "the event of the resolve of the game's ids")) {
            gameIdsResolvedListener = nullptr;
            return false;
        }
        return true;
    }

    bool requestLightLimit(unsigned int limit) {
        MwseNativeLightLimitRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        packet.limit = limit;
        return request(MWSE_NATIVE_REQUEST_LIGHT_LIMIT, &packet, "the light limit");
    }

    bool requestRipples(bool enabled) {
        MwseNativeRipplesRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        packet.enabled = enabled ? 1 : 0;
        return request(MWSE_NATIVE_REQUEST_RIPPLES, &packet, "the ripples");
    }

    bool requestIntroMoviesSkipped() {
        MwseNativeIntroMoviesRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        packet.play = 0;
        return request(MWSE_NATIVE_REQUEST_INTRO_MOVIES, &packet, "the intro movies");
    }

    bool requestLightParticlesFix() {
        MwseNativeLightParticlesFixRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        return request(MWSE_NATIVE_REQUEST_LIGHT_PARTICLES_FIX, &packet, "the light particles fix");
    }

    bool requestSunGlareOff() {
        MwseNativeSunGlareRequestV1 packet = {};
        packet.structSize = sizeof(packet);
        packet.enabled = 0;
        return request(MWSE_NATIVE_REQUEST_SUN_GLARE, &packet, "the sun glare");
    }

    bool claimLightAttachRadius(int (__cdecl* function)(const void* light, int radius)) {
        return claim(MWSE_NATIVE_OVERRIDE_LIGHT_ATTACH_RADIUS, reinterpret_cast<void*>(function), "the light attach radius");
    }

    // The engine's clock is __stdcall without arguments; a __cdecl function without arguments has
    // the same calling convention.
    bool claimFrameTimer(int (__cdecl* function)()) {
        return claim(MWSE_NATIVE_OVERRIDE_FRAME_TIMER, reinterpret_cast<void*>(function), "the frame timer");
    }
}
