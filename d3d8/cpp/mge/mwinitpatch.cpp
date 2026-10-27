
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
    // With the wgpu renderer MWSE configures the UI scale (patchUIScaleWgpu, after MWSE loads).
    if (((Configuration.MGEFlags & MGE_DISABLED) || Configuration.OnlyProxyD3D8To9) && !Configuration.RendererWgpu) {
        // User interface still needs to be scaled, normally done by the D3DDevice proxy
        // Without the proxy, patch Morrowind UI creation instead
        MWPatches::patchUIConfigure(onUIScaleInit);
    }

    // Patch texture loading to reduce process memory footprint
    // The patch gives its textures to a Direct3D device with UpdateTexture. With the wgpu
    // renderer the game has no Direct3D device, and the game code loads the textures.
    if (!Configuration.RendererWgpu) {
        MWTextureLoader::patchLoadTexture2D();
    }
    // The reader of BC7 DDS textures, for both renderers (the owner's D17: listed in the hook
    // plan for a later move into an MWSE request).
    MWTextureLoader::patchDDSReader();

    HighResolutionTimer::init();

    // With the wgpu renderer MWSE applies these (patchWgpu, after MWSE loads).
    if (!Configuration.RendererWgpu) {
        patchEarlyEngineChanges();
    }
}

void MWInitPatch::patchEarlyEngineChanges() {
    if (Configuration.MGEFlags & SKIP_INTRO) {
        // Disable intro movies on initialization as the logo appears before game init
        MWPatches::disableIntroMovies();
    }

    // Patches certain calls to timeGetTime to redirect to a better time.
    MWPatches::patchFrameTimer(&HighResolutionTimer::getMilliseconds);

    // Patch engine issue with light emissive particle material that affects particle rendering
    MWPatches::patchLightParticleMaterialModifier();
}

void MWInitPatch::patchWgpu(HMODULE mwse) {
    // MWSE applies the engine changes of MGE XE at its requests (MWPatches asks MWSE first). An
    // MWSE without its native ABI: MGE XE changes the engine code itself, as before.
    MwseNative::init(mwse);

    // MWSE sets the UI scale when the engine makes its UI and tells when it did, for the
    // borderless window.
    if (!(MwseNative::listenUIConfigured(setupBorderlessWindow) && MwseNative::requestUIScale(Configuration.UIScale))) {
        MWPatches::patchUIConfigure(onUIScaleInit);
    }

    patchEarlyEngineChanges();
}
