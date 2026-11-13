
#include "mge/mwinitpatch.h"
#include "mge/configuration.h"
#include "mge/mwbridge.h"
#include "mge/mwpatches.h"
#include "mge/mwtextureloader.h"
#include "mwse/mwsenative.h"
#include "support/timing.h"
#include "tes3/tes3types.h"
#include <algorithm>

// Setup borderless window, if selected
static void setupBorderlessWindow() {
    auto mwBridge = MWBridge::get();
    auto hMainWnd = mwBridge->getWindowHandle();
    auto gameOptions = static_cast<TES3::Game*>(mwBridge->getGameOptionsStruct());

    if (!gameOptions->fullscreen && Configuration.Borderless) {
        int wx = std::max(0, Configuration.WindowAlignX * (GetSystemMetrics(SM_CXSCREEN) - gameOptions->windowWidth) / 2);
        int wy = std::max(0, Configuration.WindowAlignY * (GetSystemMetrics(SM_CYSCREEN) - gameOptions->windowHeight) / 2);

        // Remove non-client window parts and move window flush to screen edge / centre if smaller than display
        SetWindowLong(hMainWnd, GWL_STYLE, WS_VISIBLE | WS_POPUP);
        SetWindowPos(hMainWnd, NULL, wx, wy, gameOptions->windowWidth, gameOptions->windowHeight, SWP_NOACTIVATE|SWP_NOCOPYBITS|SWP_NOZORDER);
    }
}

// Override UI scaling callback: set new UI scaling and mouse bounds, setup borderless window
static void _stdcall onUIScaleInit() {
    setupBorderlessWindow();
    MWBridge::get()->setUIScale(Configuration.UIScale);
}

void MWInitPatch::patch() {
    if (Configuration.OnlyProxyD3D8To9) {
        // User interface still needs to be scaled, normally done by the D3DDevice proxy
        // Without the proxy, patch Morrowind UI creation instead
        MWPatches::patchUIConfigure(onUIScaleInit);
    }

    // Patch texture loading to reduce process memory footprint
    MWTextureLoader::patchLoadTexture2D();
    // The reader of BC7 DDS textures, for both renderers (the owner's D17: listed in the hook
    // plan for a later move into an MWSE request).
    MWTextureLoader::patchDDSReader();

    HighResolutionTimer::init();

    patchEarlyEngineChanges();
    patchDirect3D9EngineChanges();
}

void MWInitPatch::patchWgpuStart() {
    // Before the frame timer's claim in patchWgpu: the clock reads its start here.
    HighResolutionTimer::init();
    // The reader of BC7 DDS textures (the owner's D17: listed in the hook plan for a later move
    // into an MWSE request). The game has no Direct3D device here, and the game code loads
    // the textures: no texture loader patch.
    MWTextureLoader::patchDDSReader();
}

void MWInitPatch::patchDirect3D9EngineChanges() {
    // Patch engine issue with light emissive particle material that affects particle rendering
    MWPatches::patchLightParticleMaterialModifier();
}

void MWInitPatch::patchEarlyEngineChanges() {
    if (Configuration.MGEFlags & SKIP_INTRO) {
        // Disable intro movies on initialization as the logo appears before game init
        MWPatches::disableIntroMovies();
    }

    // Patches certain calls to timeGetTime to redirect to a better time.
    MWPatches::patchFrameTimer(&HighResolutionTimer::getMilliseconds);
}

void MWInitPatch::patchWgpu() {
    // MWSE applies the engine changes of MGE XE at its requests (DllMain tested its native ABI
    // before). A refusal turns that feature off for the session; MGE XE writes no engine code.
    //
    // MWSE sets the UI scale when the engine makes its UI and tells when it did, for the
    // borderless window. Each is asked on its own: a refusal of one keeps the other.
    MwseNative::listenUIConfigured(setupBorderlessWindow);
    MwseNative::requestUIScale(Configuration.UIScale);

    patchEarlyEngineChanges();
}
