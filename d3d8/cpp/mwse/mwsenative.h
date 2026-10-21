#pragma once

#include "support/winheader.h"

// The native ABI of MWSE (MWSENativeApi.h, a copy of the header of MWSE): MGE XE asks MWSE for
// engine behaviour instead of writing engine code itself. Only the wgpu path uses it; the
// Direct3D 9 path keeps its own patches.
namespace MwseNative {
    // Looks for the table in MWSE.dll. True when MWSE has the native ABI.
    bool init(HMODULE mwse);

    // The UI scale through MWSE. False when MWSE's table is not in use: the caller applies the
    // scale itself.
    bool requestUIScale(float scale);

    // Calls `listener` when the engine configured its UI, after MWSE applied the UI scale.
    bool listenUIConfigured(void (*listener)());
}
