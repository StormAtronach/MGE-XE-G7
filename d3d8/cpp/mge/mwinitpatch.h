#pragma once

#include "support/winheader.h"

class MWInitPatch {
public:
    static void patch();
    // The UI scale on the wgpu path, after MWSE.dll is loaded.
    static void patchUIScaleWgpu(HMODULE mwse);
};
