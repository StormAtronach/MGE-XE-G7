
#include "mwsenative.h"
#include "support/log.h"

#include <cstdint>
#include "MWSENativeApi.h"

namespace MwseNative {
    static const char* const MODULE = "MGE XE";

    static const MwseNativeApiV1* table = nullptr;
    static void (*uiConfiguredListener)() = nullptr;

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

    bool requestUIScale(float scale) {
        if (!table) {
            return false;
        }
        MwseNativeUIScaleRequestV1 request = {};
        request.structSize = sizeof(request);
        request.scale = scale;
        auto status = table->request(MODULE, MWSE_NATIVE_REQUEST_UI_SCALE, &request);
        if (status != MWSE_NATIVE_OK) {
            LOG::logline("!! MWSE native ABI: the UI scale %.3f was refused (status %d, see MWSE.log)", scale, int(status));
            return false;
        }
        return true;
    }

    static void __cdecl onUIConfigured(void*, const void*) {
        if (uiConfiguredListener) {
            uiConfiguredListener();
        }
    }

    bool listenUIConfigured(void (*listener)()) {
        if (!table) {
            return false;
        }
        uiConfiguredListener = listener;
        MwseNativeHandle handle = 0;
        auto status = table->subscribe(MODULE, MWSE_NATIVE_EVENT_UI_CONFIGURED, onUIConfigured, nullptr, 0, &handle);
        if (status != MWSE_NATIVE_OK) {
            LOG::logline("!! MWSE native ABI: the event of the UI configuration was refused (status %d, see MWSE.log)", int(status));
            uiConfiguredListener = nullptr;
            return false;
        }
        return true;
    }
}
