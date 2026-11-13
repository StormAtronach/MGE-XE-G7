#pragma once

#include "support/winheader.h"

class MWInitPatch {
public:
    // The start of the Direct3D 9 path: its engine changes, written by MGE XE.
    static void patch();
    // The start of the wgpu path, after DllMain's tests have passed: the clock's start and the
    // BC7 reader, before the MWSE-MGE link.
    static void patchWgpuStart();
    // The engine changes of MGE XE on the wgpu path, after the MWSE-MGE link: all through
    // MWSE's native ABI, whose table DllMain found.
    static void patchWgpu();

private:
    // The intro movies and the frame timer.
    static void patchEarlyEngineChanges();
    // The engine changes that only Direct3D 9 needs.
    static void patchDirect3D9EngineChanges();
};
