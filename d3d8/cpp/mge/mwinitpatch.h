#pragma once

#include "support/winheader.h"

class MWInitPatch {
public:
    static void patch();
    // The engine changes of MGE XE on the wgpu path, after MWSE.dll is loaded: through MWSE's
    // native ABI when MWSE has it.
    static void patchWgpu(HMODULE mwse);

private:
    // The intro movies, the frame timer and the light particles fix.
    static void patchEarlyEngineChanges();
};
