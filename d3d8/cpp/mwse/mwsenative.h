#pragma once

#include "support/winheader.h"

// The native ABI of MWSE (MWSENativeApi.h, a copy of the header of MWSE): MGE XE asks MWSE for
// engine behaviour instead of writing engine code itself. Only the wgpu path uses it; the
// Direct3D 9 path keeps its own patches. Each function returns false when MWSE's table is not in
// use or MWSE refuses. With the table, a refusal turns that feature off for the session, and
// the caller writes no engine code (available()); without it (the Direct3D 9 path) the caller
// patches the engine itself, as before.
namespace MwseNative {
    // Looks for the table in MWSE.dll (null: no MWSE). True when MWSE has the native ABI.
    // `askedVersion` gets the version asked (the switch MGE_XE_NATIVE_ABI_VERSION can raise
    // it); it may be null.
    bool init(HMODULE mwse, unsigned* askedVersion);

    // MWSE's table is in use: a refusal of MWSE ends the feature, with no engine code of MGE XE.
    bool available();

    // Gives MWSE back each subscription and each claim of MGE XE, and stops asking MWSE:
    // the switch to the plain game. MWSE's redirects of the game's code stay, with no
    // listener of MGE XE.
    void giveBack();

    // The UI scale through MWSE.
    bool requestUIScale(float scale);

    // Calls `listener` when the engine configured its UI, after MWSE applied the UI scale.
    bool listenUIConfigured(void (*listener)());

    // Calls `listener` when the engine resolved the scripts' ids at the start of a game.
    bool listenGameIdsResolved(void (__cdecl* listener)());

    // Calls `listener` when the player changed cell through the engine's cell change (a
    // teleport, or the cell of a loaded game): the cell before and after, and whether the load
    // of a saved game made the change.
    bool listenPlayerCellChanged(void (__cdecl* listener)(const void* previousCell, const void* cell, bool gameLoad));

    // The most local lights of a node.
    bool requestLightLimit(unsigned int limit);

    // The engine's ripples on or off.
    bool requestRipples(bool enabled);

    // The two intro movies skipped.
    bool requestIntroMoviesSkipped();

    // The fix of the emissive colour of a light's particles.
    bool requestLightParticlesFix();

    // The engine's sun glare off.
    bool requestSunGlareOff();

    // The radius out to which the engine attaches a point light (`function` gets the light and the
    // engine's radius, and returns the radius to use).
    bool claimLightAttachRadius(int (__cdecl* function)(const void* light, int radius));

    // The clock of the frame timer and of the music, in milliseconds.
    bool claimFrameTimer(int (__cdecl* function)());
}
