
#include "mged3d8device.h"
#include "proxydx/d3d8texture.h"
#include "proxydx/d3d8surface.h"
#include "dxvk_morrowind_interop.h"

#include <algorithm>
#include <cstring>
#include "mgeversion.h"
#include "camerarelative.h"
#include "configuration.h"
#include "distantland.h"
#include "morrowindskinning.h"
#include "mwbridge.h"
#include "waterlook.h"
#include "mwpatches.h"
#include "renderlink.h"
#include "statusoverlay.h"
#include "support/log.h"
#include "userhud.h"
#include "videobackground.h"

static int sceneCount;
static bool rendertargetNormal, isHUDready;
static bool isMainView, isStencilScene, isAmbientWhite;
static DWORD stencilRef;
static bool stage0Complete, isFrameComplete, isHUDComplete;
static bool isWaterMaterial, waterDrawn, distantWater;
// The last material had the water mark. isWaterMaterial can come from a render link packet.
static bool waterMarkSeen;
// Marks the surface mesh of a water volume, set on its material by the mod that owns the mesh:
// 99998 for a surface that reflects what is on screen, 99997 for one that reflects the sky only.
// The emissive colour of that material is the colour of the water.
static bool isWaterVolumeMaterial, waterVolumeReflectsScene;
static unsigned int waterVolumeSlot;
static D3DCOLORVALUE waterVolumeTint;

static bool zoomSensSaved;
static float zoomSensX, zoomSensY;
static D3DXMATRIX camEffectsMatrix;
static float crosshairTimeout;

static RenderedState rs;
static FragmentState frs;
static LightState lightrs;

// The world matrix being set was already made camera-relative by an engine
// hook in camerarelative.cpp (exact node or bone placement).
static bool worldAlreadyRelative;

static void initOnLoad();
static bool detectMenu(const D3DMATRIX* m);
static void captureRenderState(D3DRENDERSTATETYPE a, DWORD b);
static void captureFragmentRenderState(DWORD a, D3DTEXTURESTAGESTATETYPE b, DWORD c);
static void captureTransform(D3DTRANSFORMSTATETYPE a, const D3DMATRIX* b);
static void captureLight(DWORD a, const D3DLIGHT8* b);
static void captureMaterial(const D3DMATERIAL8* a);
static float calcFPS();



MGEProxyDevice::MGEProxyDevice(IDirect3DDevice9* real, ProxyD3D* d3d) : ProxyDevice(real, d3d) {
    RenderLink::setDevice(this);
    // Initialize state here, as the device is released and recreated on fullscreen Alt-Tab
    sceneCount = -1;
    rendertargetNormal = true;
    isHUDready = false;
    isMainView = isStencilScene = isAmbientWhite = stage0Complete = isFrameComplete = isHUDComplete = false;
    stencilRef = 0;
    isWaterMaterial = waterMarkSeen = waterDrawn = false;
    D3DXMatrixIdentity(&camEffectsMatrix);

    Configuration.CameraEffects.zoom = 1.0;
    Configuration.CameraEffects.zoomRate = 0;
    Configuration.CameraEffects.zoomRateTarget = 0;

    // Initialize state recorder to D3D defaults
    memset(&rs, 0, sizeof(rs));
    FixedFunctionShader::resetSkinningTransforms();
    rs.zWrite = true;
    rs.diffuseMaterial.r = 1.0f;
    rs.diffuseMaterial.g = 1.0f;
    rs.diffuseMaterial.b = 1.0f;
    rs.diffuseMaterial.a = 1.0f;
    rs.cullMode = D3DCULL_CCW;
    rs.useLighting = true;

    rs.matSrcDiffuse = D3DMCS_COLOR1;
    rs.matSrcEmissive = D3DMCS_MATERIAL;

    memset(&frs, 0, sizeof(frs));
    for (FragmentState::Stage* s = &frs.stage[0]; s != &frs.stage[8]; ++s) {
        s->colorOp = D3DTOP_DISABLE;
        s->alphaOp = D3DTOP_DISABLE;
        s->colorArg1 = s->alphaArg1 = D3DTA_TEXTURE;
        s->colorArg2 = s->alphaArg2 = D3DTA_CURRENT;
        s->colorArg0 = s->alphaArg0 = s->resultArg = D3DTA_CURRENT;
        D3DXMatrixIdentity(&s->textureTransform);
    }
    lightrs.ambientWhite = false;
    frs.stage[0].colorOp = D3DTOP_MODULATE;
    frs.stage[0].alphaOp = D3DTOP_SELECTARG1;

    lightrs.lights.clear();
    lightrs.active.clear();

    // Probe the renderer for expanded light limit support. This asks the
    // dedicated capability rather than the packet version, because the engine
    // patch it authorizes is irreversible and the lighting mode is not: every
    // path reachable afterwards, including ordinary fixed-function, has to
    // consume the expanded limit. A renderer that only spoke the native packet
    // would not be enough. The interop object delegates reference counting to
    // the D3D9 device, so it is released immediately; only the result is kept.
    //
    // The soft light range is asked for the same way and for the same reason:
    // it describes the ordinary path. Any other renderer, native D3D9 included,
    // steps to zero at Range, and uploadLight must not hand it a finite one.
    expandedLightLimitSupported = false;
    softLightRangeSupported = false;
    {
        IDxvkMorrowindPplInterop1* pplInterop = nullptr;
        if (SUCCEEDED(realDevice->QueryInterface(
                __uuidof(IDxvkMorrowindPplInterop1),
                reinterpret_cast<void**>(&pplInterop)))) {
            const uint64_t capabilities = pplInterop->GetCapabilities();
            expandedLightLimitSupported = (capabilities & DXVK_MORROWIND_CAP_EXPANDED_LIGHT_LIMIT) != 0;
            softLightRangeSupported = (capabilities & DXVK_MORROWIND_CAP_SOFT_LIGHT_RANGE) != 0;
            pplInterop->Release();
        }
    }
    if (Configuration.PerPixelLightFade && !softLightRangeSupported) {
        LOG::logline("-- Renderer has no soft light range; fixed-function draws keep the engine's falloff");
    }

    // Store active device in distant land, occurs on startup and after fullscreen alt-tab
    DistantLand::device = realDevice;

    // Patch splash screen minor issues
    D3DVIEWPORT9 vp;
    realDevice->GetViewport(&vp);
    MWPatches::patchSplashScreen(vp.Width, vp.Height);

    // Install the indexed-skinning engine hooks here rather than at first
    // Present: Morrowind can issue draws through this device before it ever
    // presents a frame. The installer is one-shot, so recreating the device on
    // fullscreen alt-tab does not re-patch.
    MorrowindIndexedSkinning::installHooks();
    CameraRelative::installHooks();
}

HRESULT _stdcall MGEProxyDevice::QueryInterface(REFIID a, LPVOID* b) {
    if (!b) {
        return E_POINTER;
    }

    if (std::memcmp(&a, &IID_IMgeIndexedSkinningCaps, sizeof(GUID)) == 0) {
        *b = static_cast<IMgeIndexedSkinningCaps*>(this);
        AddRef();
        return S_OK;
    }

    return ProxyDevice::QueryInterface(a, b);
}

ULONG _stdcall MGEProxyDevice::AddRef() {
    return ProxyDevice::AddRef();
}

// The reported palette size is the feature's composite authorization token.
// Reporting zero is what keeps the engine hooks on their stock paths, so every
// gate the feature depends on is checked here in one place.
HRESULT _stdcall MGEProxyDevice::GetIndexedSkinningCaps(MgeIndexedSkinningCaps* caps) {
    if (!caps) {
        return E_POINTER;
    }

    caps->structVersion = MGE_INDEXED_SKINNING_CAPS_VERSION;
    caps->maxPaletteBones = 0;

    // Partial hook installation must never authorize indexed partitions.
    if (!MorrowindIndexedSkinning::hooksInstalled()) {
        return S_OK;
    }

    // Restart-required setting; it is not re-read once partitions are built.
    if (!Configuration.EnableIndexedSkinning) {
        return S_OK;
    }

    // Skinned draws can occur before the core effects are initialized. Defer
    // negotiation instead of permanently caching a false startup result.
    if (!(Configuration.MGEFlags & MGE_DISABLED) && !Configuration.OnlyProxyD3D8To9) {
        if (!DistantLand::hasCheckedIndexedSkinningShaders()) {
            return E_PENDING;
        }
        if (!DistantLand::supportsIndexedSkinningShaders()) {
            return S_OK;
        }
    }

    D3DCAPS9 deviceCaps = {};
    const HRESULT hr = realDevice->GetDeviceCaps(&deviceCaps);
    if (FAILED(hr)) {
        return hr;
    }

    caps->maxPaletteBones = std::min<std::uint32_t>(
        MGE_INDEXED_SKINNING_PALETTE_SIZE,
        deviceCaps.MaxVertexBlendMatrixIndex + 1
    );
    return S_OK;
}

// The logic of a frame that is not drawing: third-person camera, crosshair, zoom, mouse
// sensitivity, screen rotation and shake. The proxy device runs it at each Present. Without
// a device, the stream of the nirender plugin runs it (Deviceless::present).
static void updateFrameLogic(MWBridge* mwBridge) {
    if (Configuration.Force3rdPerson && DistantLand::canRenderDistantLand()) {
        // Set 3rd person camera
        D3DXVECTOR3* camera = mwBridge->PCam3Offset();
        if (camera) {
            camera->x = Configuration.Offset3rdPerson.x;
            camera->y = Configuration.Offset3rdPerson.y;
            camera->z = Configuration.Offset3rdPerson.z;
        }
    }

    if ((Configuration.MGEFlags & CROSSHAIR_AUTOHIDE) && !mwBridge->isLoadingBar()) {
        // Update crosshair visibility
        float t = mwBridge->simulationTime();

        // Turn on if Morrowind ray cast picks up a target
        if (mwBridge->getPlayerTarget()) {
            crosshairTimeout = t + 1.5f;
        }

        // Turn on short duration if the player requires aim
        if (mwBridge->isPlayerCasting() || mwBridge->isPlayerAimingWeapon()) {
            crosshairTimeout = t + 0.5f;
        }

        // Turn off in menu mode
        if (mwBridge->IsMenu()) {
            crosshairTimeout = t;
        }

        // Allow manual toggle of crosshair to work again from 0.5 seconds after timeout
        if (t < crosshairTimeout + 0.5) {
            mwBridge->SetCrosshairEnabled(t < crosshairTimeout);
        }
    }

    if (Configuration.CameraEffects.zoomRateTarget != 0 && !mwBridge->IsMenu()) {
        // Update zoom controller
        Configuration.CameraEffects.zoomRate += 0.25f * Configuration.CameraEffects.zoomRateTarget * mwBridge->frameTime();
        if (Configuration.CameraEffects.zoomRate / Configuration.CameraEffects.zoomRateTarget > 1.0) {
            Configuration.CameraEffects.zoomRate = Configuration.CameraEffects.zoomRateTarget;
        }

        Configuration.CameraEffects.zoom += Configuration.CameraEffects.zoomRate * mwBridge->frameTime();
        Configuration.CameraEffects.zoom = std::max(1.0f, Configuration.CameraEffects.zoom);
        Configuration.CameraEffects.zoom = std::min(Configuration.CameraEffects.zoom, 8.0f);
    }

    float* mwSens = mwBridge->getMouseSensitivityYX();
    if ((Configuration.MGEFlags & ZOOM_ASPECT) && !mwBridge->IsMenu()) {
        // Adjust sensitivity to accommodate zoom level
        if (!zoomSensSaved) {
            zoomSensY = mwSens[0];
            zoomSensX = mwSens[1];
            zoomSensSaved = true;
        }
        mwSens[0] = zoomSensY / Configuration.CameraEffects.zoom;
        mwSens[1] = zoomSensX / Configuration.CameraEffects.zoom;
    } else if (zoomSensSaved) {
        // Restore unzoomed sensitivity
        mwSens[0] = zoomSensY;
        mwSens[1] = zoomSensX;
        zoomSensSaved = false;
    }

    if (Configuration.CameraEffects.rotateUpdate) {
        Configuration.CameraEffects.rotation += Configuration.CameraEffects.rotationRate * mwBridge->frameTime();
        D3DXMatrixRotationZ(&camEffectsMatrix, Configuration.CameraEffects.rotation);
        if (Configuration.CameraEffects.rotationRate == 0) {
            Configuration.CameraEffects.rotateUpdate = false;
        }
    }
    if (Configuration.CameraEffects.shake) {
        // Update screen shake controller
        Configuration.CameraEffects.shakeMagnitude += Configuration.CameraEffects.shakeAccel * mwBridge->frameTime();
        Configuration.CameraEffects.shakeMagnitude = std::max(0.0f, std::min(100.0f, Configuration.CameraEffects.shakeMagnitude));
        camEffectsMatrix._41 = Configuration.CameraEffects.shakeMagnitude * sin(0.001f*GetTickCount());
    }
}

HRESULT _stdcall MGEProxyDevice::Present(const RECT* a, const RECT* b, HWND c, const RGNDATA* d) {
    CameraRelative::onPresent();

    auto mwBridge = MWBridge::get();

    // Load Morrowind's dynamic memory pointers
    if (!mwBridge->IsLoaded() && mwBridge->CanLoad()) {
        mwBridge->Load();

        // Apply patch to load distant land before the main menu, and on renderer restart
        MWPatches::patchGameLoading(&initOnLoad);
        // Patch world rendering (on a branch without the water) to split alphas to their own scene
        MWPatches::patchWorldRenderingAccumulation();
        // Disable MW screenshot function to allow MGE to use the same key
        MWPatches::disableScreenshotFunc();
        // Mark water material to allow MGEProxyDevice to detect it
        if (!RenderLink::usesWaterPlaneFact()) {
            RenderLink::noteWaterMark(mwBridge->markWaterNode(99999.0f));
        }

        // Raise the engine's per-node light limit, on request and only when the
        // renderer reports it can consume the extra lights. This is one
        // decision per device and it is not reversible: the lighting mode is
        // runtime-mutable (ToggleLightingMode, MGEAPI::lightingModeSet,
        // interiors-only stepping outdoors), but no later re-evaluation can
        // unpatch the executable. Every path the engine can reach afterwards
        // must therefore handle 32 lights, which is why the renderer probe
        // covers the ordinary fixed-function path and not just native packets.
        if (Configuration.ExpandedLightLimit && expandedLightLimitSupported) {
            MWPatches::patchExpandedLightLimit();
        }

        // Attach lights out to where the per-pixel fade ends, plus the distance an
        // actor or light moves before the engine retests it, so no light is added
        // or removed while it still reaches an object. The radius is decided per
        // call, as the lighting mode can change at runtime.
        //
        // That decision only covers attachments made from here on. An attachment
        // is sticky: the engine keeps it until the object moves 64 units, and a
        // static never moves, so turning per-pixel lighting off at runtime
        // (MGEAPI::lightingModeSet, MacroFunctions::ToggleLightingMode) leaves
        // lights attached out past where the engine would have cut them. On a
        // renderer with the soft light range they are not drawn unfaded, because
        // uploadLight carries the same cutoff to the ordinary fixed-function
        // path in D3DLIGHT Range. On any other they are, until the cell reloads:
        // dimmer than at the record radius, and with smaller steps than the
        // engine's own. What survives either way is the extra pressure on each
        // node's effect slots, since a light that fades to zero still occupies
        // one; expanded_light_limit covers that.
        if (Configuration.PerPixelLightFade) {
            MWPatches::patchLightAttachRadius(&FixedFunctionShader::lightAttachRadius);
        }

        // Start distant-land host/output preparation and upload pumping at the earliest
        // Present where Morrowind's environment pointer is known to be valid.
        if (!(Configuration.MGEFlags & MGE_DISABLED)
            && !Configuration.OnlyProxyD3D8To9
            && (Configuration.MGEFlags & USE_DISTANT_LAND)) {
            DistantLand::init();
        }
    }

    if (mwBridge->IsLoaded()) {
        updateFrameLogic(mwBridge);

        // Main menu background video
        VideoPatch::monitor(realDevice);
    }

    // Capture whether stage0 ran this frame before the per-frame reset below: menu and
    // load-screen frames never reach it, so residency eviction needs a fallback boundary.
    const bool stage0RanThisFrame = stage0Complete;

    // Reset scene identifiers
    sceneCount = -1;
    stage0Complete = false;
    waterDrawn = false;
    DistantLand::waterVolumeDrawn = false;
    DistantLand::discardWaterVolumes();
    isFrameComplete = false;
    isHUDComplete = false;
    DistantLand::beginDepthFrame();

    // Tick the frame-budgeted distant-land upload pump across idle menu/load
    // frames (load-optimization Blocker 4). Runs after EndScene, so it is a safe
    // point to create D3D resources. The budget keeps menu frames responsive.
    if (DistantLand::pumpActive && !DistantLand::pumpDraining) {
        DistantLand::pumpUploadTick(DistantLand::kUploadPumpBudgetMs);
    } else if (!DistantLand::pumpDraining) {
        DistantLand::tickResidency(stage0RanThisFrame);
    }

    return ProxyDevice::Present(a, b, c, d);
}

HRESULT _stdcall MGEProxyDevice::SetRenderTarget(IDirect3DSurface8* a, IDirect3DSurface8* b) {
    // Track whether Morrowind is rendering to the back buffer; world-scene hooks use this target.
    if (a) {
        IDirect3DSurface9* back;
        realDevice->GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &back);
        rendertargetNormal = (static_cast<ProxySurface*>(a)->realSurface == back);
        back->Release();
    }

    return ProxyDevice::SetRenderTarget(a, b);
}

// Morrowind uses multiple scenes per frame: non-alpha / 2x stencil / post-stencil redraw / alpha /
// 1st person / UI. Fog state must be set before its first world scene.
HRESULT _stdcall MGEProxyDevice::BeginScene() {
    auto mwBridge = MWBridge::get();

    HRESULT hr = ProxyDevice::BeginScene();
    if (hr != D3D_OK) {
        return hr;
    }

    if (mwBridge->IsLoaded() && rendertargetNormal) {
        if (!isHUDready) {
            StatusOverlay::init(realDevice);
            StatusOverlay::setStatus(XE_VERSION_STRING);
            MGEhud::init(realDevice);

            if (Configuration.UIScale != 1.0f) {
                mwBridge->setUIScale(Configuration.UIScale);
            }

            isHUDready = true;
        }

        if (isMainView) {
            // Track scene count here in BeginScene; isMainView is not always valid at EndScene if Morrowind draws sunglare
            sceneCount = RenderLink::resolveSceneCount(sceneCount + 1);

            if (sceneCount == 0) {
                if (Configuration.ScreenFOV > 0) {
                    mwBridge->SetFOV(Configuration.ScreenFOV);
                }
                distantWater = (Configuration.MGEFlags & USE_DISTANT_LAND) || (Configuration.MGEFlags & USE_DISTANT_WATER);
            }
        } else {
            // UI scene, apply post-process if there was anything drawn before it
            // The race menu can issue an extra scene; post-process and HUD are still one-shot.
            if (DistantLand::canRenderDistantLand() && sceneCount > 0 && !isFrameComplete) {
                DistantLand::postProcess();
            }

            // Draw the user HUD before Morrowind's HUD so the game UI remains on top.
            if (isHUDready && !isHUDComplete) {
                MGEhud::draw();
            }

            isFrameComplete = true;
        }
    }

    return D3D_OK;
}

// Multiple scenes per frame: non-alpha / 2x stencil / post-stencil redraw / alpha / 1st person / UI
// Intercepts first scene to draw distant land before it finishes; other scenes have shadows applied
HRESULT _stdcall MGEProxyDevice::EndScene() {
    if (DistantLand::canRenderDistantLand() && rendertargetNormal) {
        // The following Morrowind scenes get past the filters:
        // ~ Opaque meshes, plus alpha meshes with 'No Sorter' property (which should use alpha test)
        // ~ If stencil shadows are active, then shadow casters are deferred to be drawn in a scene after
        //    shadows are fully applied to avoid self-shadowing problems with simplified shadow meshes
        // ~ If any alpha meshes are visible, they are sorted and drawn in another scene (except those with 'No Sorter' property)
        // ~ If 1st person or sunglare is visible, they are drawn in another scene after a Z clear
        if (sceneCount == 0) {
            if (!stage0Complete) {
                // A fully culled Morrowind scene may produce no draw call, so stage 0 must also
                // be started from EndScene.
                DistantLand::renderStage0();
                stage0Complete = true;
            }

            DistantLand::renderStage1();
            DistantLand::renderStageBlend();
            DistantLand::flushWaterVolumes(true);
        } else if (!isFrameComplete) {
            DistantLand::renderStage2();
            DistantLand::flushWaterVolumes(false);

            if (distantWater && !waterDrawn && !isStencilScene) {
                // The Morrowind water grid can be out of view, or scene/stencil order can be
                // non-standard; draw replacement water once when that happens.
                DistantLand::renderStageWater();
                waterDrawn = true;
            }
        }
    }

    if (isFrameComplete && isHUDready && !isHUDComplete) {
        DistantLand::checkCaptureScreenshot(true);

        StatusOverlay::setFPS(calcFPS());
        StatusOverlay::show(realDevice);

        isHUDComplete = true;
    }

    return ProxyDevice::EndScene();
}

// Clear occurs at start of frame, and as a z-clear before rendering 1st person and sunglare
// Skybox mesh doesn't extend over whole background; cleared background colour is visible at horizon
HRESULT _stdcall MGEProxyDevice::Clear(DWORD a, const D3DRECT* b, DWORD c, D3DCOLOR d, float e, DWORD f) {
    DistantLand::setHorizonColour(d);
    const HRESULT hr = ProxyDevice::Clear(a, b, c, d, e, f);
    if (SUCCEEDED(hr) && (c & D3DCLEAR_ZBUFFER)) {
        DistantLand::depthBufferCleared();
    }
    return hr;
}

HRESULT _stdcall MGEProxyDevice::SetTransform(D3DTRANSFORMSTATETYPE a, const D3DMATRIX* b) {
    worldAlreadyRelative = CameraRelative::takeWorldRelative();
    RenderLink::noteTransform(a, b, worldAlreadyRelative);

    if (a == D3DTS_VIEW) {
        // Decide the space of this scene before the recorder sees the view, so every
        // world matrix captured afterwards is combined with the same view.
        CameraRelative::onViewTransform(b, rendertargetNormal);
        // Lights the device still holds from the previous scene may be in the other space.
        refreshActiveLights();
    }

    captureTransform(a, b);

    if (rendertargetNormal) {
        if (a == D3DTS_VIEW) {
            isMainView = RenderLink::resolveMainView(!detectMenu(b));

            if (CameraRelative::active()) {
                // The scene was chosen by camera identity; the device view has to
                // match the camera-relative world matrices it is about to receive.
                D3DXMATRIX view;
                CameraRelative::setCameraEffects(&camEffectsMatrix);
                CameraRelative::deviceView(&camEffectsMatrix, &view);
                return ProxyDevice::SetTransform(a, &view);
            }
            if (isMainView) {
                D3DXMATRIX view = *b;
                view *= camEffectsMatrix;
                return ProxyDevice::SetTransform(a, &view);
            }
        } else if (a == D3DTS_PROJECTION) {
            if (isMainView) {
                // Expand only the world projection; UI and load-bar projections must stay intact.
                D3DXMATRIX proj = *b;
                DistantLand::setProjection(&proj);

                if (Configuration.MGEFlags & ZOOM_ASPECT) {
                    proj._11 *= Configuration.CameraEffects.zoom;
                    proj._22 *= Configuration.CameraEffects.zoom;
                }

                DistantLand::trackDepthProjection(&proj);
                return ProxyDevice::SetTransform(a, &proj);
            }
        }
    }

    if (a == D3DTS_PROJECTION) {
        DistantLand::trackDepthProjection(b);
    }

    if (CameraRelative::active() && !worldAlreadyRelative
        && a >= D3DTS_WORLDMATRIX(0) && a < D3DTS_WORLDMATRIX(MGE_INDEXED_SKINNING_PALETTE_SIZE)) {
        // The real device only ever sees camera-relative world matrices in this
        // space, so its own fixed-function world-view product is small-number math.
        D3DXMATRIX world;
        CameraRelative::relativeWorld(b, &world);
        return ProxyDevice::SetTransform(a, &world);
    }
    return ProxyDevice::SetTransform(a, b);
}

HRESULT _stdcall MGEProxyDevice::SetMaterial(const D3DMATERIAL8* a) {
    RenderLink::noteMaterial(a);
    captureMaterial(a);
    waterMarkSeen = (a->Power == 99999.0f);
    isWaterMaterial = RenderLink::resolve(MGE_LINK_FACT_WATER_PLANE, waterMarkSeen, RenderLink::drawIsWaterPlane());
    if (isWaterMaterial) {
        DistantLand::waterPlaneTint[0] = a->Emissive.r;
        DistantLand::waterPlaneTint[1] = a->Emissive.g;
        DistantLand::waterPlaneTint[2] = a->Emissive.b;
    }
    // A surface of a water volume: the two old markers, or a look slot
    waterVolumeReflectsScene = (a->Power == 99998.0f);
    isWaterVolumeMaterial = waterVolumeReflectsScene || (a->Power == 99997.0f);
    waterVolumeSlot = 0;
    if (a->Power >= WaterLooks::slotMarkerBase && a->Power < WaterLooks::slotMarkerBase + WaterLooks::maxSlots) {
        waterVolumeSlot = static_cast<unsigned int>(a->Power - WaterLooks::slotMarkerBase);
        waterVolumeReflectsScene = (WaterLooks::get(waterVolumeSlot).flags & WATER_LOOK_REFLECTS_SCENE) != 0;
        isWaterVolumeMaterial = true;
    }
    waterVolumeTint = a->Emissive;

    return ProxyDevice::SetMaterial(a);
}

HRESULT _stdcall MGEProxyDevice::SetLight(DWORD a, const D3DLIGHT8* b) {
    RenderLink::noteLight(a, b);
    captureLight(a, b);

    // Exterior sunlight/interior "sun" appears to always be light 6
    if (RenderLink::resolve(MGE_LINK_FACT_SUN_LIGHT, a == 6, RenderLink::lightIsSun(a)) && DistantLand::canRenderDistantLand()) {
        DistantLand::setSunLight(b);
    }

    return uploadLight(a, b);
}

HRESULT MGEProxyDevice::uploadLight(DWORD a, const D3DLIGHT8* absolute) {
    // The ordinary fixed-function path takes no draw packet, so the fade rides
    // in Range, which Morrowind leaves effectively infinite. Without this a
    // light the engine attached out at the widened radius is drawn unfaded by
    // any draw the per-pixel shaders did not take, which is every draw once
    // per-pixel lighting is switched off at runtime.
    //
    // Only for a renderer that fades over Range. D3D9 specifies a step there,
    // and the step is reachable: the engine attaches by an object's bounds, so
    // a room shell or a terrain patch attached at the record radius has
    // vertices well past the cutoff. A step across those is worse than the
    // unfaded light it would replace.
    float fadeRange = 0.0f;
    if (softLightRangeSupported && Configuration.PerPixelLightFade && absolute->Type == D3DLIGHT_POINT) {
        auto iLight = lightrs.lights.find(a);
        if (iLight != lightrs.lights.end()) {
            fadeRange = FixedFunctionShader::lightFadeCutoff(iLight->second.radius);
        }
    }

    const bool cameraRelative = CameraRelative::installed()
        && CameraRelative::active()
        && absolute->Type != D3DLIGHT_DIRECTIONAL;

    HRESULT hr;
    if (fadeRange > 0 || cameraRelative) {
        D3DLIGHT8 light = *absolute;
        if (fadeRange > 0) {
            light.Range = fadeRange;
        }
        if (cameraRelative) {
            // Keep the fixed-function path's lights in the same space as its geometry.
            CameraRelative::relativePosition(&absolute->Position, &light.Position);
        }
        hr = ProxyDevice::SetLight(a, &light);
    } else {
        hr = ProxyDevice::SetLight(a, absolute);
    }

    if (!CameraRelative::installed()) {
        // No scene can become active, so nothing would ever read the record.
        return hr;
    }

    // Recorded after the device call, so a rejected positional light stays
    // stale and the next view or LightEnable tries it again.
    CameraRelative::recordLightUpload(a, absolute, SUCCEEDED(hr));
    return hr;
}

void MGEProxyDevice::refreshLight(DWORD a) {
    if (!CameraRelative::installed()) {
        return;
    }
    D3DLIGHT8 absolute;
    if (CameraRelative::lightUploadStale(a, &absolute)) {
        uploadLight(a, &absolute);
    }
}

void MGEProxyDevice::refreshActiveLights() {
    if (!CameraRelative::installed()) {
        return;
    }
    for (DWORD index : lightrs.active) {
        refreshLight(index);
    }
}

HRESULT _stdcall MGEProxyDevice::SetRenderState(D3DRENDERSTATETYPE a, DWORD b) {
    RenderLink::noteRenderState(a, b);
    captureRenderState(a, b);

    if (a == D3DRS_FOGVERTEXMODE || a == D3DRS_FOGTABLEMODE) {
        return D3D_OK;
    }
    if ((Configuration.MGEFlags & USE_DISTANT_LAND) && (a == D3DRS_FOGSTART || a == D3DRS_FOGEND)) {
        return D3D_OK;
    }
    if (a == D3DRS_STENCILENABLE) {
        isStencilScene = b;
    }
    else if (a == D3DRS_STENCILREF) {
        stencilRef = b;
    }

    if (a == D3DRS_AMBIENT) {
        // Pure white ambient occurs with skydome and menu mode rendering
        // Ambient is also never set properly when high enough outside that Morrowind renders nothing
        isAmbientWhite = (b == 0xffffffff);
        lightrs.ambientWhite = isAmbientWhite;

        if (!isAmbientWhite) {
            // Preserve the last real ambient value for frames where Morrowind emits no draw calls.
            RGBVECTOR amb = D3DCOLOR(b);
            DistantLand::setAmbientColour(amb);
            lightrs.globalAmbient.r = amb.r;
            lightrs.globalAmbient.g = amb.g;
            lightrs.globalAmbient.b = amb.b;
        }
    }

    return ProxyDevice::SetRenderState(a, b);
}

HRESULT _stdcall MGEProxyDevice::SetTextureStageState(DWORD a, D3DTEXTURESTAGESTATETYPE b, DWORD c) {
    RenderLink::noteStageState(a, b, c);
    captureFragmentRenderState(a, b, c);

    // Sampler overrides to ensure trilinear/anisotropic filtering works
    // Note that DX8 had sampling state bound to texture stages instead of samplers
    if (b == D3DTSS_MINFILTER) {
        DWORD filter = (c != D3DTEXF_NONE) ? Configuration.ScaleFilter : D3DTEXF_NONE;
        return realDevice->SetSamplerState(a, D3DSAMP_MINFILTER, filter);
    } else if (b == D3DTSS_MIPFILTER) {
        DWORD filter = (c != D3DTEXF_NONE) ? D3DTEXF_LINEAR : D3DTEXF_NONE;
        return realDevice->SetSamplerState(a, D3DSAMP_MIPFILTER, filter);
    }

    return ProxyDevice::SetTextureStageState(a, b, c);
}

HRESULT _stdcall MGEProxyDevice::DrawIndexedPrimitive(D3DPRIMITIVETYPE a, UINT b, UINT c, UINT d, UINT e) {
    const bool indexedSkinning =
        rs.vertexBlendState != D3DVBF_DISABLE &&
        (rs.fvf & D3DFVF_LASTBETA_UBYTE4) != 0;

    bool isShadowStencil = isStencilScene && stencilRef <= 1;
    if (DistantLand::canRenderDistantLand() && rendertargetNormal && isMainView && !isShadowStencil) {
        rs.primType = a;
        rs.baseIndex = baseVertexIndex;
        rs.minIndex = b;
        rs.vertCount = c;
        rs.startIndex = d;
        rs.primCount = e;

        RenderLink::observeWorldDraw();
        RenderLink::observeDrawState(rs, frs, lightrs);
        RenderLink::applyDrawState(rs, frs, lightrs);

        if (!stage0Complete && !isAmbientWhite) {
            // In an exterior this is normally the first world draw after the sky; interiors may
            // have no preceding draw, so stage 0 is guarded here as well as in EndScene.
            DistantLand::renderStage0();
            stage0Complete = true;
        }

        if (RenderLink::resolve(MGE_LINK_FACT_WATER_PLANE, waterMarkSeen, RenderLink::drawIsWaterPlane())) {
            if (distantWater) {
                // Replacement water suppresses the original Morrowind grid.
                if (!waterDrawn) {
                    DistantLand::renderStageWater();
                    waterDrawn = true;
                }
                RenderLink::census(RenderLink::CensusWaterReplaced);
                return D3D_OK;
            }
        } else if (isWaterVolumeMaterial && distantWater && DistantLand::renderWaterVolume(&rs, waterVolumeReflectsScene, waterVolumeTint, waterVolumeSlot, &lightrs)) {
            // The surface of a water volume was drawn with the water shading.
            RenderLink::census(RenderLink::CensusWaterReplaced);
            return D3D_OK;
        } else {
            // DistantLand may record the draw for a later pass and signal that the original is
            // already represented; in that case suppress the game draw.
            if (!DistantLand::inspectIndexedPrimitive(sceneCount, &rs, &frs, &lightrs)) {
                return D3D_OK;
            }
        }
        RenderLink::census(RenderLink::CensusWorldFixed);
    } else {
        RenderLink::census(!DistantLand::canRenderDistantLand() ? RenderLink::CensusNotReady
            : !rendertargetNormal ? RenderLink::CensusOtherTarget
            : isShadowStencil ? RenderLink::CensusShadowStencil : RenderLink::CensusOtherView);
    }

    if (!indexedSkinning) {
        return ProxyDevice::DrawIndexedPrimitive(a, b, c, d, e);
    }

    realDevice->SetRenderState(D3DRS_INDEXEDVERTEXBLENDENABLE, TRUE);
    const HRESULT hr = ProxyDevice::DrawIndexedPrimitive(a, b, c, d, e);
    realDevice->SetRenderState(D3DRS_INDEXEDVERTEXBLENDENABLE, FALSE);
    return hr;
}

ULONG _stdcall MGEProxyDevice::Release() {
    ULONG r = ProxyDevice::Release();

    if (r == 0) {
        DistantLand::release();
        MGEhud::release();
        StatusOverlay::release();
        // Drops the negotiated device and the fallback cache's engine
        // references. The executable patches stay installed; they are
        // process-lifetime and the next device renegotiates against them.
        MorrowindIndexedSkinning::onDeviceReleased();
        CameraRelative::onDeviceReleased();
    }

    return r;
}

void initOnLoad() {
    auto mwBridge = MWBridge::get();
    auto device = DistantLand::device;

    // Compose loading message from translated string
    char buffer[64];
    const char* loadingMessage = *(const char**)mwBridge->getGMSTPointer(602);
    int firstWordLength = 0;

    for (const char *c = loadingMessage; *c; ++c) {
        if (*c == ' ') { break; }
        ++firstWordLength;
    }

    std::snprintf(buffer, sizeof(buffer), "%.*s MGE XE...", firstWordLength, loadingMessage);
    mwBridge->showLoadingBar(buffer, 95.0);

    // Initialize distant land
    if (DistantLand::init()) {
        // Initially force view distance to max, required for full extent shadows and grass
        if (Configuration.MGEFlags & USE_DISTANT_LAND) {
            mwBridge->SetViewDistance(7168.0);
        }
    } else {
        Configuration.MGEFlags &= ~USE_DISTANT_LAND;
        StatusOverlay::setStatus("MGE XE serious error condition. Exit Morrowind and check mgeXE.log for details.", StatusOverlay::PriorityError);
    }

    // Clean up loading bar menu, otherwise it persists in the background
    mwBridge->destroyLoadingBar();

    VideoPatch::start(device);
}

// detectMenu
// detects if view matrix is for UI / load bars
// the projection matrix is never set to ortho, making it unusable for detection
bool detectMenu(const D3DMATRIX* m) {
    if (m->_41 != 0.0f || !(m->_42 == 0.0f || m->_42 == -600.0f) || m->_43 != 0.0f) {
        return false;
    }

    if ((m->_11 == 0.0f || m->_11 == 1.0f) && m->_12 == 0.0f && (m->_13 == 0.0f || m->_13 == 1.0f) &&
            m->_21 == 0.0f && (m->_22 == 0.0f || m->_22 == 1.0f) && (m->_23 == 0.0f || m->_23 == 1.0f) &&
            (m->_31 == 0.0f || m->_31 == 1.0f) && (m->_32 == 0.0f || m->_32 == 1.0f) && m->_33 == 0.0f) {
        return true;
    }

    return false;
}

// --------------------------------------------------------
// State recording

HRESULT _stdcall MGEProxyDevice::SetTexture(DWORD a, IDirect3DBaseTexture8* b) {
    RenderLink::noteTexture(a, b);
    if (a < 8) {
        IDirect3DTexture9* tex = b ? static_cast<ProxyTexture*>(b)->realTexture : NULL;
        frs.stage[a].texture = tex;
        if (a == 0) {
            rs.texture = tex;
        }
    }
    return ProxyDevice::SetTexture(a, b);
}

HRESULT _stdcall MGEProxyDevice::SetVertexShader(DWORD a) {
    RenderLink::noteVertexFormat(a);
    rs.fvf = a;
    return ProxyDevice::SetVertexShader(a);
}

HRESULT _stdcall MGEProxyDevice::SetStreamSource(UINT a, IDirect3DVertexBuffer8* b, UINT c) {
    if (a == 0) {
        RenderLink::noteStreamSource(b, c);
        rs.vb = (IDirect3DVertexBuffer9*)b;
        rs.vbOffset = 0;
        rs.vbStride = c;
    }
    return ProxyDevice::SetStreamSource(a, b, c);
}

HRESULT _stdcall MGEProxyDevice::SetIndices(IDirect3DIndexBuffer8* a, UINT b) {
    RenderLink::noteIndices(a, b);
    rs.ib = (IDirect3DIndexBuffer9*)a;
    return ProxyDevice::SetIndices(a, b);
}

HRESULT _stdcall MGEProxyDevice::LightEnable(DWORD a, BOOL b) {
    RenderLink::noteLightEnable(a, b != FALSE);
    if (b) {
        if (std::find(lightrs.active.begin(), lightrs.active.end(), a) == lightrs.active.end()) {
            lightrs.active.push_back(a);
        }
        // The engine re-enables a light without re-uploading it; the device
        // copy may date from another scene or camera position.
        refreshLight(a);
    } else {
        if (std::remove(lightrs.active.begin(), lightrs.active.end(), a) != lightrs.active.end()) {
            lightrs.active.pop_back();
        }
    }
    return ProxyDevice::LightEnable(a, b);
}

void captureRenderState(D3DRENDERSTATETYPE a, DWORD b) {
    switch (a) {
    case D3DRS_VERTEXBLEND:
        rs.vertexBlendState = b;
        break;
    case D3DRS_ZWRITEENABLE:
        rs.zWrite = b;
        break;
    case D3DRS_CULLMODE:
        rs.cullMode = b;
        break;
    case D3DRS_ALPHABLENDENABLE:
        rs.blendEnable = (BYTE)b;
        break;
    case D3DRS_SRCBLEND:
        rs.srcBlend = (BYTE)b;
        break;
    case D3DRS_DESTBLEND:
        rs.destBlend = (BYTE)b;
        break;
    case D3DRS_ALPHATESTENABLE:
        rs.alphaTest = (BYTE)b;
        break;
    case D3DRS_ALPHAFUNC:
        rs.alphaFunc = (BYTE)b;
        break;
    case D3DRS_ALPHAREF:
        rs.alphaRef = (BYTE)b;
        break;
    case D3DRS_LIGHTING:
        rs.useLighting = (BYTE)b;
        break;
    case D3DRS_FOGENABLE:
        rs.useFog = (BYTE)b;
        break;
    case D3DRS_DIFFUSEMATERIALSOURCE:
        rs.matSrcDiffuse = (BYTE)b;
        break;
    case D3DRS_EMISSIVEMATERIALSOURCE:
        rs.matSrcEmissive = (BYTE)b;
        break;
    }
}

void captureFragmentRenderState(DWORD a, D3DTEXTURESTAGESTATETYPE b, DWORD c) {
    FragmentState::Stage* s = &frs.stage[a];

    switch (b) {
    case D3DTSS_COLOROP:
        s->colorOp = (BYTE)c;
        break;
    case D3DTSS_COLORARG1:
        s->colorArg1 = (BYTE)c;
        break;
    case D3DTSS_COLORARG2:
        s->colorArg2 = (BYTE)c;
        break;
    case D3DTSS_ALPHAOP:
        s->alphaOp = (BYTE)c;
        break;
    case D3DTSS_ALPHAARG1:
        s->alphaArg1 = (BYTE)c;
        break;
    case D3DTSS_ALPHAARG2:
        s->alphaArg2 = (BYTE)c;
        break;
    case D3DTSS_BUMPENVMAT00:
        s->bumpEnvMat[0][0] = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_BUMPENVMAT01:
        s->bumpEnvMat[0][1] = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_BUMPENVMAT10:
        s->bumpEnvMat[1][0] = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_BUMPENVMAT11:
        s->bumpEnvMat[1][1] = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_TEXCOORDINDEX:
        s->texcoordIndex = c;
        break;
    case D3DTSS_BUMPENVLSCALE:
        s->bumpLumiScale = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_BUMPENVLOFFSET:
        s->bumpLumiBias = reinterpret_cast<float&>(c);
        break;
    case D3DTSS_TEXTURETRANSFORMFLAGS:
        s->texTransformFlags = c;
        break;
    case D3DTSS_COLORARG0:
        s->colorArg0 = (BYTE)c;
        break;
    case D3DTSS_ALPHAARG0:
        s->alphaArg0 = (BYTE)c;
        break;
    case D3DTSS_RESULTARG:
        s->resultArg = (BYTE)c;
        break;
    }
}

void captureTransform(D3DTRANSFORMSTATETYPE a, const D3DMATRIX* b) {
    if (a >= D3DTS_WORLDMATRIX(0) && a < D3DTS_WORLDMATRIX(MGE_INDEXED_SKINNING_PALETTE_SIZE)) {
        const UINT index = a - D3DTS_WORLDMATRIX(0);

        if (CameraRelative::active()) {
            // The engine's world translation is exact; the rounding happens in the
            // world-view product. Subtract the camera first, then multiply in double.
            // A matrix an engine hook already placed relative to the camera is used
            // as is, and the recorder's absolute copy is rebuilt from it.
            D3DXMATRIX world;
            D3DXMATRIX absolute;
            if (worldAlreadyRelative) {
                world = *b;
                CameraRelative::absoluteFromRelative(b, &absolute);
            } else {
                CameraRelative::relativeWorld(b, &world);
                absolute = *b;
            }
            FixedFunctionShader::setSkinningWorldTransform(index, &world, &rs.viewTransform);

            if (index < 4) {
                rs.worldTransforms[index] = absolute;
                CameraRelative::multiplyWorldView(&world, &rs.viewTransform, &rs.worldViewTransforms[index]);
            }
            return;
        }

        const auto world = static_cast<const D3DXMATRIX*>(b);
        FixedFunctionShader::setSkinningWorldTransform(index, world, &rs.viewTransform);

        if (index < 4) {
            rs.worldTransforms[index] = *b;
            D3DXMatrixMultiply(&rs.worldViewTransforms[index], world, &rs.viewTransform);
        }
        return;
    }

    if (a == D3DTS_VIEW) {
        // While camera-relative space is active the recorder works against a
        // rotation-only view; world matrices carry the camera offset instead.
        rs.viewTransform = CameraRelative::active() ? *CameraRelative::recorderView() : *static_cast<const D3DXMATRIX*>(b);
        FixedFunctionShader::setSkinningViewTransform(&rs.viewTransform);
        lightrs.lightsTransformed.clear();
        return;
    }

    if (a >= D3DTS_TEXTURE0 && a <= D3DTS_TEXTURE7) {
        frs.stage[a - D3DTS_TEXTURE0].textureTransform = *b;
    }
}

void captureLight(DWORD a, const D3DLIGHT8* b) {
    // Morrowind uses non-contigous light IDs up to a large number (>512)
    auto iLight = lightrs.lights.find(a);
    const bool existed = iLight != lightrs.lights.end();
    LightState::Light* light = existed ? &iLight->second : &lightrs.lights[a];

    // Copy values relevant to Morrowind
    // i.e. Morrowind has no spotlights and always sets range to FLT_MAX
    // The only light source with ambient is sunlight
    const D3DLIGHTTYPE previousType = light->type;
    const D3DVECTOR previousPosition = light->position;

    light->type = b->Type;
    light->diffuse = b->Diffuse;

    if (b->Type == D3DLIGHT_POINT) {
        const bool falloffChanged = previousType != D3DLIGHT_POINT
            || light->falloff.x != b->Attenuation0
            || light->falloff.y != b->Attenuation1
            || light->falloff.z != b->Attenuation2;

        light->position = b->Position;
        light->falloff.x = b->Attenuation0;
        light->falloff.y = b->Attenuation1;
        light->falloff.z = b->Attenuation2;

        // Morrowind resubmits unchanged lights for every object.
        if (falloffChanged) {
            light->inferredRadius = MWBridge::get()->pointLightRadius(
                b->Attenuation0, b->Attenuation1, b->Attenuation2);
        }
        light->radius = RenderLink::resolveLightRadius(a, light->inferredRadius);
    } else {
        D3DXVec3Normalize((D3DXVECTOR3*)&light->position, (D3DXVECTOR3*)&b->Direction);
        light->ambient.x = b->Ambient.r;
        light->ambient.y = b->Ambient.g;
        light->ambient.z = b->Ambient.b;
    }

    // The cached view-space position is only stale if the source-space position
    // or direction moved, or the light changed type. Morrowind resubmits
    // unchanged lights for every object, so invalidating on every SetLight
    // would defeat the cross-object cache; not invalidating at all leaves a
    // light that moves twice under one view transform with a stale position.
    if (existed
     && (previousType != light->type
      || previousPosition.x != light->position.x
      || previousPosition.y != light->position.y
      || previousPosition.z != light->position.z)) {
        lightrs.lightsTransformed.erase(a);
    }
}

void captureMaterial(const D3DMATERIAL8* a) {
    // Morrowind does not use specular lighting
    rs.diffuseMaterial = a->Diffuse;
    frs.material.diffuse = a->Diffuse;
    frs.material.ambient = a->Ambient;
    frs.material.emissive = a->Emissive;
    frs.material.emissive.a = a->Power;
}

// --------------------------------------------------------
// FPS meter - Updates every 500ms. Morrowind's internal meter changes too fast and falsely clamps the fps.

float calcFPS() {
    static int lastMillis, framesSinceUpdate;
    static float fps = 0.0f;

    ++framesSinceUpdate;
    int millis = MWBridge::get()->getFrameBeginMillis();
    int diff = millis - lastMillis;

    if (diff >= 500) {
        fps = 1000.0f * framesSinceUpdate / diff;
        lastMillis = millis;
        framesSinceUpdate = 0;
    } else if (diff < 0) {
        lastMillis = millis;
    }

    return fps;
}

//-----------------------------------------------------------------------------
// Without a device: the wgpu renderer of the helper draws the picture, and the nirender
// plugin gives the game its Direct3D 8 (runtime.renderer_wgpu). No proxy device exists, so
// the plugin calls these functions at the same points of a frame.

// In main.cpp: sends data to the wgpu renderer in the helper.
void sendToRenderer(UINT id, const void* data, UINT length);

namespace Deviceless {
    static bool ready = false;
    static bool mainView = false;
    // The position of the camera of the world, from its last view matrix.
    static float eye[3] = { 0, 0, 0 };
    // The direction that the camera of the world looks in.
    static float look[3] = { 0, 1, 0 };
    // The fog colour that the scattering gives the near cells, from the last environment,
    // and true when that frame had the scattering. The game writes the fog colour of the
    // weather into its scene graph before each frame, so the colour goes in when the game
    // sets the view of the world: after that write and before the draws.
    static DWORD scatterFogColour = 0;
    static bool scatterFogValid = false;

    // The state of the game that the features of MGE XE in the helper need for a frame.
    // mgeHost64/src/renderer.rs has the same layout: 4 bytes for each member.
    struct Environment {
        UINT version;
        // 1: exponential fog. 2: the camera is under water. 4: an exterior, or an interior
        // that is like one.
        UINT flags;
        float nearViewRange;
        float fogStart, fogEnd;
        float fogExpStart, fogExpDivisor;
        float fogNearStart, fogNearEnd;
        float niceWeather;
        float lightSunMult, lightAmbMult;
        float windScaling;
        float eye[3];
        // Version 2. Flag 8: the scattering of the atmosphere is on for this frame. The
        // direction to the sun, turned at the horizon when the sun is not visible, as the
        // sunPos of the shaders of the Direct3D 9 path is.
        float sunPos[3];
        float skyCol[3];
        // The fog colour of the weather, before the scattering changed it.
        float fogColFar[3];
        float outscatter[3];
        float inscatter[3];
        float skyScatterFar[4];
        // Version 3, for the grass. The wind as the grass shader takes it: the wind of the
        // game times the value of the weather, made smooth over the frames.
        float wind[2];
        // The position of the player, and 1 when it is known.
        float footPos[3];
        UINT footValid;
        // The time of the simulation of the game, in seconds. It stops in a menu.
        float time;
        // Version 4, for the water: the level of the water of the cell, the visibility of
        // the sun (0 to 1), and 1 when the cell has water.
        float waterLevel;
        float sunVis;
        UINT hasWater;
        // Version 5: the name of the interior cell as the game has it, padded with zeros
        // (the key of its world space in usage.data); all zeros in an exterior.
        char interiorName[64];
    };
    static const UINT EnvironmentId = 1;

    static float mix(float a, float b, float t) {
        return a + (b - a) * t;
    }

    // The fog distances and the weather values of a frame, as DistantLand::adjustFog makes
    // them for the Direct3D 9 path, without its device calls.
    static void sendEnvironment(MWBridge* mwBridge) {
        Environment e = {};
        e.version = 5;
        e.nearViewRange = mwBridge->GetViewDistance();
        e.lightSunMult = 1.0f;
        e.lightAmbMult = 1.0f;
        std::memcpy(e.eye, eye, sizeof(eye));

        const bool underwater = mwBridge->IsUnderwater(eye[2]);
        const bool exterior = mwBridge->IntLikeExterior(true);
        const bool expFog = (Configuration.MGEFlags & USE_DISTANT_LAND) && (Configuration.MGEFlags & EXP_FOG);
        float fogStart, fogEnd;

        if (underwater) {
            fogStart = Configuration.DL.BelowWaterFogStart;
            fogEnd = Configuration.DL.BelowWaterFogEnd;
        } else if (exterior) {
            int wthr1 = mwBridge->GetCurrentWeather(), wthr2 = mwBridge->GetNextWeather();
            float ratio = mwBridge->GetWeatherRatio(), ff = 1.0f, fo = 0.0f, ws = 0.0f;

            if (ratio != 0 && wthr1 >= 0 && wthr1 <= 9 && wthr2 >= 0 && wthr2 <= 9) {
                ff = mix(Configuration.DL.FogD[wthr1], Configuration.DL.FogD[wthr2], ratio);
                fo = 0.01f * mix(Configuration.DL.FgOD[wthr1], Configuration.DL.FgOD[wthr2], ratio);
                ws = mix(Configuration.DL.Wind[wthr1], Configuration.DL.Wind[wthr2], ratio);
                e.niceWeather = mix((wthr1 <= 1) ? 1.0f : 0.0f, (wthr2 <= 1) ? 1.0f : 0.0f, ratio);
                e.niceWeather *= e.niceWeather;
                e.lightSunMult = mix(Configuration.Lighting.SunMult[wthr1], Configuration.Lighting.SunMult[wthr2], ratio);
                e.lightAmbMult = mix(Configuration.Lighting.AmbMult[wthr1], Configuration.Lighting.AmbMult[wthr2], ratio);
            } else if (wthr1 >= 0 && wthr1 <= 9) {
                ff = Configuration.DL.FogD[wthr1];
                fo = Configuration.DL.FgOD[wthr1] / 100.0f;
                ws = Configuration.DL.Wind[wthr1];
                e.niceWeather = (wthr1 <= 1) ? 1.0f : 0.0f;
                e.lightSunMult = Configuration.Lighting.SunMult[wthr1];
                e.lightAmbMult = Configuration.Lighting.AmbMult[wthr1];
            }

            fogEnd = std::max(0.875f, ff * Configuration.DL.AboveWaterFogEnd);
            fogStart = ff * Configuration.DL.AboveWaterFogStart - fo * fogEnd;
            e.windScaling = ws;
            if (expFog) {
                float lg = log(1.0f - 0.25f * fo);
                float expCorrection = lg / (1 + lg);
                fogStart = ff * Configuration.DL.AboveWaterFogStart + expCorrection * fogEnd;
            }
        } else {
            float density = std::max(0.01f, mwBridge->getInteriorFogDens());
            fogStart = mix(Configuration.DL.InteriorFogEnd, Configuration.DL.InteriorFogStart, density);
            fogEnd = Configuration.DL.InteriorFogEnd;
        }

        const float cellSize = 8192.0f;
        fogStart *= cellSize;
        fogEnd *= cellSize;
        e.fogStart = fogStart;
        e.fogEnd = fogEnd;
        e.fogNearStart = fogStart;
        e.fogNearEnd = fogEnd;

        const float expFogDistScale = 4.4f;
        e.fogExpStart = fogStart / expFogDistScale;
        e.fogExpDivisor = (fogEnd - e.fogExpStart) / expFogDistScale;
        if (expFog && !underwater && exterior) {
            // The linear fog of the near cells, as near to the exponential curve as it can be.
            float farIntercept = std::min(fogEnd, e.nearViewRange);
            float expFogNear = exp(-(1280.0f - e.fogExpStart) / e.fogExpDivisor);
            float expFogFar = exp(-(farIntercept - e.fogExpStart) / e.fogExpDivisor);
            if (expFogFar != expFogNear) {
                e.fogNearStart = 1280.0f + (farIntercept - 1280.0f) * (1.0f - expFogNear) / (expFogFar - expFogNear);
                e.fogNearEnd = 1280.0f + (farIntercept - 1280.0f) * -expFogNear / (expFogFar - expFogNear);
            }
        }
        e.flags = (expFog ? 1u : 0u) | (underwater ? 2u : 0u) | (exterior ? 4u : 0u);

        // The scattering of the atmosphere, as in DistantLand::adjustFog: the fog colour of
        // the near cells becomes the colour that the scattering has at the view range of the
        // game, and the game gets it through its scene graph. The shaders of the helper get
        // the values for the same sum in each pixel.
        const bool scatter = (Configuration.MGEFlags & USE_DISTANT_LAND) && (Configuration.MGEFlags & USE_ATM_SCATTER) && exterior && !underwater;
        scatterFogValid = false;
        e.sunVis = exterior ? mwBridge->GetSunVis() / 255.0f : 0.0f;
        if (exterior) {
            const RGBVECTOR fogCol = *mwBridge->getCurrentWeatherFogCol();
            const RGBVECTOR* skyCol = mwBridge->getCurrentWeatherSkyCol();
            float sun[3];
            mwBridge->GetSunDir(sun[0], sun[1], sun[2]);
            const float sunLength = sqrt(sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2]);
            for (int i = 0; i < 3; ++i) {
                e.sunPos[i] = sunLength > 0 ? sun[i] / sunLength : 0.0f;
            }
            // The sun "bounces" at the horizon when it is not visible, as DistantLand::setView
            // has it: the light of the night comes from the turned direction.
            if (e.sunVis == 0) {
                e.sunPos[2] = -e.sunPos[2];
            }
            e.skyCol[0] = skyCol->r; e.skyCol[1] = skyCol->g; e.skyCol[2] = skyCol->b;
            e.fogColFar[0] = fogCol.r; e.fogColFar[1] = fogCol.g; e.fogColFar[2] = fogCol.b;
            const RGBVECTOR& out = DistantLand::atmOutscatter;
            const RGBVECTOR& in = DistantLand::atmInscatter;
            const D3DXVECTOR4& far4 = DistantLand::atmSkylightScatter;
            e.outscatter[0] = out.r; e.outscatter[1] = out.g; e.outscatter[2] = out.b;
            e.inscatter[0] = in.r; e.inscatter[1] = in.g; e.inscatter[2] = in.b;
            e.skyScatterFar[0] = far4.x; e.skyScatterFar[1] = far4.y; e.skyScatterFar[2] = far4.z; e.skyScatterFar[3] = far4.w;

            if (scatter) {
                e.flags |= 8u;
                const float newSky[3] = {
                    mix(skyCol->r, far4.x, far4.w), mix(skyCol->g, far4.y, far4.w), mix(skyCol->b, far4.z, far4.w)
                };
                const float sunaltitude = powf(1 + e.sunPos[2], 10);
                const float sunaltitude_a = 2.8f + 4.3f / sunaltitude;
                const float sunaltitude_b = std::min(1.0f, std::max(0.0f, 1.0f - exp2f(-1.9f * sunaltitude)));
                const float sunaltitude_c = std::min(1.0f, std::max(0.0f, expf(-4.0f * e.sunPos[2]))) * std::min(1.0f, std::max(0.0f, sunaltitude));

                float fogdist = (e.nearViewRange - e.fogExpStart) / e.fogExpDivisor;
                const float fog = std::min(1.0f, std::max(0.0f, expf(-fogdist)));
                fogdist = std::min(1.0f, std::max(0.0f, 0.224f * fogdist));

                const float lookLength = sqrt(look[0] * look[0] + look[1] * look[1]);
                const float dirX = lookLength > 0 ? look[0] / lookLength : 0.0f;
                const float dirY = lookLength > 0 ? look[1] / lookLength : 1.0f;
                const float suncos = dirX * e.sunPos[0] + dirY * e.sunPos[1];
                const float mie = (1.58f / (1.24f - suncos)) * sunaltitude_c;
                const float rayl = 1.0f - 0.09f * mie;
                const float atmdep = 1.33f;
                const float scatterT = 0.5f * (1 + suncos);
                float c1[3];
                for (int i = 0; i < 3; ++i) {
                    const float scatterI = mix(e.inscatter[i], e.outscatter[i], scatterT);
                    float att = atmdep * scatterI * (sunaltitude_a + 0.7f * mie);
                    att = (1 - expf(-fogdist * att)) / att;
                    const float k0 = mie * 0.125f + rayl * newSky[i];
                    const float k1 = att * (1.17f * atmdep + 0.89f) * sunaltitude_b;
                    // From the added light of the scattering to the fog model of Direct3D.
                    c1[i] = k0 * k1 / std::max(0.02f, 1.0f - fog);
                }
                RGBVECTOR nearFogCol(
                    (1.0f - e.niceWeather) * fogCol.r + e.niceWeather * c1[0],
                    (1.0f - e.niceWeather) * fogCol.g + e.niceWeather * c1[1],
                    (1.0f - e.niceWeather) * fogCol.b + e.niceWeather * c1[2]);
                scatterFogColour = (DWORD)nearFogCol;
                scatterFogValid = true;
            }
        }

        // The wind, as in DistantLand::setupCommonEffect: it is very noisy, so each frame
        // goes a small step to the wind of the game. Not in a menu.
        static float smoothWind[2];
        if (!mwBridge->IsMenu()) {
            const float f = 0.02f;
            float targetWind[2];
            if (exterior) {
                const float* wind = mwBridge->GetWindVector();
                targetWind[0] = e.windScaling * wind[0];
                targetWind[1] = e.windScaling * wind[1];
            } else {
                const float diagonal = 0.70710678f;
                targetWind[0] = Configuration.Grass.InteriorWind * diagonal;
                targetWind[1] = Configuration.Grass.InteriorWind * diagonal;
            }
            smoothWind[0] += f * (targetWind[0] - smoothWind[0]);
            smoothWind[1] += f * (targetWind[1] - smoothWind[1]);
        }
        e.wind[0] = smoothWind[0];
        e.wind[1] = smoothWind[1];
        if (auto footPos = (const float*)mwBridge->PlayerPositionPointer()) {
            std::memcpy(e.footPos, footPos, sizeof(e.footPos));
            e.footValid = 1;
        }
        e.time = mwBridge->simulationTime();
        e.waterLevel = mwBridge->WaterLevel();
        e.hasWater = mwBridge->CellHasWater() ? 1u : 0u;
        if (!mwBridge->IsExterior()) {
            if (const char* name = mwBridge->getInteriorName()) {
                std::strncpy(e.interiorName, name, sizeof(e.interiorName));
            }
        }
        sendToRenderer(EnvironmentId, &e, sizeof(e));
    }

    static void prepare() {
        if (!ready) {
            D3DXMatrixIdentity(&camEffectsMatrix);
            ready = true;
        }
    }

    // The game showed a frame. The size is that of its back buffer.
    void present(unsigned width, unsigned height) {
        prepare();
        MGEhud::setScreenSize(width, height);

        auto mwBridge = MWBridge::get();
        if (!mwBridge->IsLoaded() && mwBridge->CanLoad()) {
            // The patches of the first Present of the proxy device prepare its drawing
            // (distant land, alpha scenes, water mark, light limit). None is for this mode.
            mwBridge->Load();
            LOG::logline("-- wgpu renderer: the game is loaded, the frame logic of MGE XE runs from the stream");
            // The wgpu renderer takes all lights of a node: the patch of the engine to 32
            // needs no capability of the device here.
            if (Configuration.ExpandedLightLimit) {
                MWPatches::patchExpandedLightLimit();
            }
        }
        if (mwBridge->IsLoaded()) {
            updateFrameLogic(mwBridge);
            if (Configuration.ScreenFOV > 0) {
                mwBridge->SetFOV(Configuration.ScreenFOV);
            }
            sendEnvironment(mwBridge);
        }
    }

    // The game sets its view or its projection. The matrix is changed in place, as the
    // proxy device changes it in SetTransform: the camera effects on the view of the world,
    // and the zoom on the projection of the world.
    void transform(unsigned state, D3DMATRIX* matrix, bool targetIsBackBuffer) {
        prepare();
        if (!targetIsBackBuffer || !matrix) {
            return;
        }
        if (state == D3DTS_VIEW) {
            mainView = !detectMenu(matrix);
            if (mainView) {
                // The camera is at -translation * transposed rotation of its view matrix.
                const D3DMATRIX& m = *matrix;
                eye[0] = -(m._41 * m._11 + m._42 * m._12 + m._43 * m._13);
                eye[1] = -(m._41 * m._21 + m._42 * m._22 + m._43 * m._23);
                eye[2] = -(m._41 * m._31 + m._42 * m._32 + m._43 * m._33);
                // The camera looks along the third axis of its view.
                look[0] = m._13;
                look[1] = m._23;
                look[2] = m._33;
                D3DXMATRIX view = *matrix;
                view *= camEffectsMatrix;
                *matrix = view;
                // The fog colour of the scattering for the draws of this view, as
                // DistantLand::adjustFog gives it to the game at the start of a scene.
                auto mwBridge = MWBridge::get();
                if (scatterFogValid && mwBridge->IsLoaded()) {
                    mwBridge->setScenegraphFogCol(scatterFogColour);
                }
            }
        } else if (state == D3DTS_PROJECTION && mainView && (Configuration.MGEFlags & ZOOM_ASPECT)) {
            matrix->_11 *= Configuration.CameraEffects.zoom;
            matrix->_22 *= Configuration.CameraEffects.zoom;
        }
    }
}
