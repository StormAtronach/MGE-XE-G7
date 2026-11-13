
#include "mwsenative.h"
#include "support/log.h"

#include <cstdint>
#include <cstdlib>
#include <vector>
#include "MWSENativeApi.h"

namespace MwseNative {
    static const char* const MODULE = "MGE XE";

    static const MwseNativeApiV1* table = nullptr;
    static void (*uiConfiguredListener)() = nullptr;
    static void (__cdecl* gameIdsResolvedListener)() = nullptr;
    static void (__cdecl* playerCellChangedListener)(const void*, const void*, bool) = nullptr;

    // The handles of the subscriptions and of the claims: giveBack returns them to MWSE.
    static std::vector<MwseNativeHandle> subscriptions;
    static std::vector<MwseNativeHandle> claims;

    // A test switch: a number, or 0 when it is not set or is not a number. Its use is logged.
    static unsigned testSwitch(const char* name) {
        char value[16];
        const DWORD length = GetEnvironmentVariableA(name, value, sizeof(value));
        if (length == 0 || length >= sizeof(value)) {
            return 0;
        }
        char* end = nullptr;
        const unsigned long number = std::strtoul(value, &end, 10);
        if (!end || *end != '\0') {
            LOG::logline("-- switch %s=%s: not a number; not used", name, value);
            return 0;
        }
        return static_cast<unsigned>(number);
    }

    // MGE_XE_NATIVE_ABI_VERSION=<n>: the table is asked at version n. Only a version above
    // MGE XE's own is taken, so the switch can only take the table away (a test of the plain
    // game with an MWSE that is too old).
    static uint32_t versionToAsk() {
        const unsigned asked = testSwitch("MGE_XE_NATIVE_ABI_VERSION");
        if (asked == 0) {
            return MWSE_NATIVE_API_VERSION;
        }
        if (asked <= MWSE_NATIVE_API_VERSION) {
            LOG::logline("-- switch MGE_XE_NATIVE_ABI_VERSION=%u: not above %u; not used", asked, MWSE_NATIVE_API_VERSION);
            return MWSE_NATIVE_API_VERSION;
        }
        LOG::logline("-- switch MGE_XE_NATIVE_ABI_VERSION=%u: the native ABI is asked at version %u (a test)", asked, asked);
        return asked;
    }

    bool init(HMODULE mwse, unsigned* askedVersion) {
        const uint32_t version = versionToAsk();
        if (askedVersion) {
            *askedVersion = version;
        }
        auto getNativeApi = mwse ? reinterpret_cast<MwseGetNativeApi>(GetProcAddress(mwse, "MWSE_GetNativeApi")) : nullptr;
        const MwseNativeApiV1* found = getNativeApi ? getNativeApi(version) : nullptr;
        if (!found || found->structSize < sizeof(MwseNativeApiV1)) {
            LOG::logline("-- MWSE has no native ABI of version %u", version);
            return false;
        }
        table = found;
        LOG::logline("-- MWSE native ABI: MWSE %s", table->mwseVersion);
        return true;
    }

    bool available() {
        return table != nullptr;
    }

    void giveBack() {
        if (!table) {
            return;
        }
        for (auto handle : subscriptions) {
            table->unsubscribe(handle);
        }
        for (auto handle : claims) {
            table->release(handle);
        }
        if (!subscriptions.empty() || !claims.empty()) {
            LOG::logline("-- MWSE native ABI: %u subscriptions and %u claims given back to MWSE",
                         unsigned(subscriptions.size()), unsigned(claims.size()));
        }
        subscriptions.clear();
        claims.clear();
        uiConfiguredListener = nullptr;
        gameIdsResolvedListener = nullptr;
        playerCellChangedListener = nullptr;
        table = nullptr;
    }

    // The log line of a refusal: the feature is off for the session.
    static void logRefusal(const char* what, MwseNativeStatus status) {
        LOG::logline("!! MWSE native ABI: %s was refused (status %d, see MWSE.log): %s is off this session", what, int(status), what);
    }

    // A request of MWSE; `what` names it in the log of a refusal.
    static bool request(uint32_t id, const void* packet, const char* what) {
        if (!table) {
            return false;
        }
        auto status = table->request(MODULE, id, packet);
        if (status != MWSE_NATIVE_OK) {
            logRefusal(what, status);
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
            logRefusal(what, status);
            return false;
        }
        claims.push_back(handle);
        return true;
    }

    static bool subscribe(uint32_t id, MwseNativeListener listener, const char* what) {
        if (!table) {
            return false;
        }
        MwseNativeHandle handle = 0;
        auto status = table->subscribe(MODULE, id, listener, nullptr, 0, &handle);
        if (status != MWSE_NATIVE_OK) {
            logRefusal(what, status);
            return false;
        }
        subscriptions.push_back(handle);
        // MGE_XE_NATIVE_REFUSE_EVENT=<id>: the subscription of that event counts as refused
        // after MWSE took it, so the switch to the plain game and the give-back run (a test).
        // It can only turn the new renderer off.
        static const unsigned refuseEvent = testSwitch("MGE_XE_NATIVE_REFUSE_EVENT");
        if (refuseEvent != 0 && refuseEvent == id) {
            LOG::logline("-- switch MGE_XE_NATIVE_REFUSE_EVENT=%u: %s counts as refused (a test)", refuseEvent, what);
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

    static void __cdecl onPlayerCellChanged(void*, const void* data) {
        auto packet = static_cast<const MwseNativePlayerCellChangedV1*>(data);
        if (playerCellChangedListener && packet->structSize >= sizeof(MwseNativePlayerCellChangedV1)) {
            playerCellChangedListener(packet->previousCell, packet->cell, packet->gameLoad != 0);
        }
    }

    bool listenPlayerCellChanged(void (__cdecl* listener)(const void* previousCell, const void* cell, bool gameLoad)) {
        playerCellChangedListener = listener;
        if (!subscribe(MWSE_NATIVE_EVENT_PLAYER_CELL_CHANGED, onPlayerCellChanged, "the event of the player's change of cell")) {
            playerCellChangedListener = nullptr;
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
