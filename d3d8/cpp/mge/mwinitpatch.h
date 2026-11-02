#pragma once

#include "support/winheader.h"

class MWInitPatch {
public:
    static void patch();
    // The engine changes of MGE XE on the wgpu path, after MWSE.dll is loaded: through MWSE's
    // native ABI when MWSE has it.
    static void patchWgpu(HMODULE mwse);
    // The engine changes that only Direct3D 9 needs. The wgpu path can turn off after patchWgpu,
    // so this also runs then. A second call writes the same byte.
    static void patchDirect3D9EngineChanges();

private:
    // The intro movies and the frame timer.
    static void patchEarlyEngineChanges();
};
