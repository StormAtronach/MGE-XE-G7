
#include "distantland.h"
#include "distantshader.h"
#include "configuration.h"
#include "doublesurface.h"
#include "mwbridge.h"
#include "postshaders.h"
#include "waterlook.h"

#include <cmath>



void DistantLand::renderWaterReflection(const D3DXMATRIX* view, const D3DXMATRIX* proj) {
    auto mwBridge = MWBridge::get();
    const bool reflectStatics = isDistantCell() && staticsUploaded && (Configuration.MGEFlags & REFLECT_NEAR);

    // The shadow pass streams through the reflected-static vector in parallel. Preserve the
    // previous complete reflection until that RPC finishes rather than clearing either resource.
    if (reflectStatics && ipcClient.pollRpcCompletion() != IPC::Complete) {
        return;
    }

    // Switch to render target
    RenderTargetSwitcher rtsw(texReflection, surfReflectionZ);
    device->Clear(0, NULL, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, horizonCol, 1.0, 0);

    // Calculate reflected view matrix, mirror plane at water mesh level
    D3DXMATRIX reflView;
    D3DXPLANE plane(0, 0, 1.0f, -(mwBridge->WaterLevel() - 1.0f));
    D3DXMatrixReflect(&reflView, &plane);
    D3DXMatrixMultiply(&reflView, &reflView, view);
    effect->SetMatrix(ehView, &reflView);

    // Calculate new projection
    D3DXMATRIX reflProj = *proj;
    editProjectionZ(&reflProj, 4.0, Configuration.DL.DrawDist * kCellSize);
    effect->SetMatrix(ehProj, &reflProj);

    // Clipping setup
    D3DXMATRIX clipMat;

    // Clip geometry on opposite side of water plane
    plane *= mwBridge->IsUnderwater(eyePos.z) ? -1.0f : 1.0f;

    // If using dynamic ripples, the water level can be lowered by up to 0.5 * waveheight
    // so move clip plane downwards at the cost of some reflection errors
    if (Configuration.MGEFlags & DYNAMIC_RIPPLES) {
        plane.d += 0.5f * Configuration.DL.WaterWaveHeight;
    }

    // Doing inverses separately is a lot more numerically stable
    D3DXMatrixInverse(&clipMat, 0, &reflView);
    D3DXMatrixTranspose(&clipMat, &clipMat);
    D3DXPlaneTransform(&plane, &plane, &clipMat);

    D3DXMatrixInverse(&clipMat, 0, &reflProj);
    D3DXMatrixTranspose(&clipMat, &clipMat);
    D3DXPlaneTransform(&plane, &plane, &clipMat);

    if (visDistantShared.Empty()) {
        // Workaround for a Direct3D bug with clipping planes, where SetClipPlane
        // has no effect on the shader pipeline if the last rendered draw call was using
        // the fixed function pipeline. This is usually covered by distant statics, but
        // not in compact interiors where all distant statics may be culled.
        // Provoking a DrawPrimitive with shader here makes the following SetClipPlane work.
        effect->BeginPass(PASS_WORKAROUND);
        device->SetVertexDeclaration(WaterDecl);
        device->SetStreamSource(0, vbFullFrame, 0, 12);
        device->DrawPrimitive(D3DPT_TRIANGLESTRIP, 0, 2);
        effect->EndPass();
    }

    device->SetClipPlane(0, plane);
    device->SetRenderState(D3DRS_CLIPPLANEENABLE, 1);

    // Rendering
    if (mwBridge->IsExterior() && (Configuration.MGEFlags & REFLECTIVE_WATER)) {
        // Draw land reflection, with opposite culling
        effect->BeginPass(PASS_RENDERLANDREFL);
        device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CCW);
        renderDistantLand(effect, &reflView, &reflProj);
        effect->EndPass();
    }

    if (reflectStatics) {
        // Draw statics reflection, with opposite culling and no dissolve
        DWORD p = (mwBridge->IntLikeExterior(true) && !mwBridge->IsUnderwater(eyePos.z)) ? PASS_RENDERSTATICSEXTERIOR : PASS_RENDERSTATICSINTERIOR;
        effect->SetFloat(ehNearViewRange, 0);
        effect->BeginPass(p);
        device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CCW);
        renderReflectedStatics(&reflView, &reflProj);
        effect->EndPass();
        effect->SetFloat(ehNearViewRange, nearViewRange);
    }

    if ((Configuration.MGEFlags & REFLECT_SKY) && !recordSky.empty() && !mwBridge->IsUnderwater(eyePos.z)) {
        // Draw sky reflection, with opposite culling
        renderReflectedSky();
    }

    // Restore view state
    device->SetRenderState(D3DRS_CLIPPLANEENABLE, 0);
    effect->SetMatrix(ehView, view);
    effect->SetMatrix(ehProj, proj);
}

void DistantLand::renderReflectedSky() {
    // Sky objects are not correctly positioned at infinity, so correction is required
    const float adjustZ = -2.0f * eyePos.z;
    D3DXMATRIX skyScale, worldTransform;
    D3DXMatrixScaling(&skyScale, 1e6, 1e6, 1e6);

    // Recorded renders
    const auto& recordSky_const = recordSky;
    const int standardCloudVerts = 65, standardCloudTris = 112;
    const int standardMoonVerts = 4, standardMoonTris = 2;

    // Render sky without clouds first
    effect->BeginPass(PASS_RENDERSKY);
    device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CCW);

    for (const auto& i : recordSky_const) {
        // Skip clouds
        if (i.texture && i.vertCount == standardCloudVerts && i.primCount == standardCloudTris) {
            continue;
        }

        // Adjust world transform, as skydome objects are positioned relative to the viewer
        worldTransform = i.worldTransforms[0];
        worldTransform._43 += adjustZ;
        if (i.texture == nullptr) {
            // Inflate sky mesh towards infinity, makes skypos in shader calculate correctly
            D3DXMatrixMultiply(&worldTransform, &skyScale, &worldTransform);
        }

        // Set variables in main effect; variables are shared via effect pool
        effect->SetTexture(ehTex0, i.texture);
        if (i.texture) {
            // Textured object; draw as normal in shader, with exceptions:
            // - Sun/moon billboards do not use mipmaps
            // - Moon shadow cutout (prevents stars shining through moons)
            //   which requires colour to be replaced with atmosphere scattering colour
            bool isBillboard = (i.vertCount == standardMoonVerts && i.primCount == standardMoonTris);
            bool isMoonShadow = i.destBlend == D3DBLEND_INVSRCALPHA && !i.useLighting;

            effect->SetBool(ehHasAlpha, true);
            effect->SetBool(ehHasBones, isBillboard);
            effect->SetBool(ehHasVCol, isMoonShadow);
            device->SetRenderState(D3DRS_ALPHABLENDENABLE, 1);
            device->SetRenderState(D3DRS_SRCBLEND, i.srcBlend);
            device->SetRenderState(D3DRS_DESTBLEND, i.destBlend);
            device->SetRenderState(D3DRS_ALPHATESTENABLE, 1);
        } else {
            // Sky; perform atmosphere scattering in shader
            effect->SetBool(ehHasAlpha, false);
            effect->SetBool(ehHasVCol, true);
            device->SetRenderState(D3DRS_ALPHABLENDENABLE, 0);
            device->SetRenderState(D3DRS_ALPHATESTENABLE, 0);
        }

        effect->SetMatrix(ehWorld, &worldTransform);
        effect->CommitChanges();

        device->SetStreamSource(0, i.vb, i.vbOffset, i.vbStride);
        device->SetIndices(i.ib);
        device->SetFVF(i.fvf);
        device->DrawIndexedPrimitive(i.primType, i.baseIndex, i.minIndex, i.vertCount, i.startIndex, i.primCount);
    }
    effect->EndPass();

    // Render clouds with a separate shader
    effect->BeginPass(PASS_RENDERCLOUDS);
    device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CCW);

    for (const auto& i : recordSky_const) {
        // Clouds only
        if (!(i.texture && i.vertCount == standardCloudVerts && i.primCount == standardCloudTris)) {
            continue;
        }

        // Adjust world transform, as skydome objects are positioned relative to the viewer
        worldTransform = i.worldTransforms[0];
        worldTransform._43 += adjustZ;

        effect->SetTexture(ehTex0, i.texture);
        effect->SetBool(ehHasAlpha, true);
        device->SetRenderState(D3DRS_ALPHABLENDENABLE, 1);
        device->SetRenderState(D3DRS_SRCBLEND, i.srcBlend);
        device->SetRenderState(D3DRS_DESTBLEND, i.destBlend);
        device->SetRenderState(D3DRS_ALPHATESTENABLE, 1);
        effect->SetMatrix(ehWorld, &worldTransform);
        effect->CommitChanges();

        device->SetStreamSource(0, i.vb, i.vbOffset, i.vbStride);
        device->SetIndices(i.ib);
        device->SetFVF(i.fvf);
        device->DrawIndexedPrimitive(i.primType, i.baseIndex, i.minIndex, i.vertCount, i.startIndex, i.primCount);
    }
    effect->EndPass();
}

void DistantLand::renderReflectedStatics(const D3DXMATRIX* view, const D3DXMATRIX* proj) {
    // Select appropriate static clipping distance
    D3DXMATRIX ds_proj = *proj, ds_viewproj;
    const float nearStaticEnd = Configuration.DL.NearStaticEnd * kCellSize;
    const float farStaticEnd = Configuration.DL.FarStaticEnd * kCellSize;
    float zn = 4.0f, zf = nearStaticEnd;

    // Don't draw beyond fully fogged distance; early out if frustum is empty
    zf = std::min(fogEnd, zf);
    if (zf <= zn) {
        return;
    }

    // Create a clipping frustum for visibility determination
    editProjectionZ(&ds_proj, zn, zf);
    ds_viewproj = (*view) * ds_proj;

    // Cull sort and draw
    ViewFrustum range_frustum(&ds_viewproj);
    // Keep the real camera eye here: the gate combines main and reflected queries, and a mirrored
    // eye would corrupt its movement and resume-probe signals.
    // zf clips view-space z, but the host tests the sphere radially. Scale by the frustum corner
    // ray so the sphere stays a conservative bound instead of cutting off edge statics.
    const float tanX = 1.0f / proj->_11;
    const float tanY = 1.0f / proj->_22;
    const float cornerRay = std::sqrt(1.0f + tanX * tanX + tanY * tanY);
    D3DXVECTOR4 viewsphere(eyePos.x, eyePos.y, eyePos.z, zf * cornerRay);

    visExtraShared.RemoveAll();
    if (!ipcClient.getVisibleMeshes(visExtraSharedId, range_frustum, viewsphere, VIS_STATIC, nearStaticEnd, farStaticEnd, VisibleSetSort::ByState)) {
        return;
    }

    device->SetVertexDeclaration(StaticDecl);

    if (ipcClient.waitForCompletion() != IPC::Complete) {
        return;
    }
    visExtraShared.Render(device, effect, effect, &ehTex0, nullptr, &ehHasVCol, &ehWorld, &ehUvBoundPalette, SIZEOFSTATICVERT);
}

void DistantLand::clearReflection() {
    auto mwBridge = MWBridge::get();
    IDirect3DSurface9* target;
    DWORD baseColour;

    texReflection->GetSurfaceLevel(0, &target);
    if (mwBridge->IntLikeExterior(true) || mwBridge->IsUnderwater(eyePos.z)) {
        // Use fog colour as reflection
        baseColour = (DWORD)horizonCol;
    } else {
        // Interior fog colour is typically too bright
        // Guess a reflection colour based on cell lighting parameters
        const BYTE* sun = mwBridge->getInteriorSun();
        RGBVECTOR c(sun[0] / 255.0f, sun[1] / 255.0f, sun[2] / 255.0f);
        c += ambCol;
        baseColour = (DWORD)c;
    }
    device->ColorFill(target, 0, baseColour);
    target->Release();
}

void DistantLand::simulateDynamicWaves() {
    auto mwBridge = MWBridge::get();

    static bool resetRippleSurface = true;
    static float remainingWaveTime = 0;
    static const float waveStep = 0.0125f;  // time per wave simulation step (1/80 sec)

    // Simulation paused in menu mode
    if (mwBridge->IsMenu()) {
        return;
    }

    device->SetFVF(fvfWave);
    device->SetStreamSource(0, vbWaveSim, 0, 32);

    // Calc number of wave iterations to run this frame
    float frameTime = std::min(mwBridge->frameTime(), 0.5f);
    remainingWaveTime += frameTime;
    int numWaveSteps = (int)(remainingWaveTime / waveStep);
    remainingWaveTime -= numWaveSteps * waveStep;

    // Preciptation (rain/snow) ripples
    if (mwBridge->IntLikeExterior(true)) {
        static float remainingRipples = 0;

        // Reset surface when not needed next time
        resetRippleSurface = true;

        // Weather types: rain = 4; thunderstorm = 5; snow = 8; blizzard = 9
        // Thunderstorm causes 50% more ripples
        int w0 = mwBridge->GetCurrentWeather(), w1 = mwBridge->GetNextWeather();
        float precipitation0 = (w0 == 4 || w0 == 5 || w0 == 8 || w0 == 9) ? 1.0f : -1.5f;
        float precipitation1 = (w1 == 4 || w1 == 5 || w1 == 8 || w1 == 9) ? 1.0f : -1.5f;
        precipitation0 += (w0 == 5) ? 0.5f : 0;
        precipitation1 += (w1 == 5) ? 0.5f : 0;

        // 150 drops per second for normal precipitation
        float precipitation = (1.0f - mwBridge->GetWeatherRatio()) * precipitation0 + mwBridge->GetWeatherRatio() * precipitation1;
        float rippleFrequency = 150.0f * precipitation;

        if (rippleFrequency > 0) {
            static double randomizer = 0.546372819;
            int ripplePos[2];
            RECT drop;

            remainingRipples += rippleFrequency * frameTime;
            int n = int(std::floor(remainingRipples));
            remainingRipples -= n;

            while (n-- > 0) {
                // Place rain ripple at random location
                for (int i = 0; i != 2; ++i) {
                    randomizer = randomizer * (1337.134511337451 + 0.0001 * rand()) + 0.12351523;
                    randomizer -= floor(randomizer);
                    ripplePos[i] = (int)(randomizer * waveTexResolution);
                }

                drop.left = ripplePos[0] - 2;
                drop.right = ripplePos[0] + 2;
                drop.top = ripplePos[1] - 1;
                drop.bottom = ripplePos[1] + 1;
                device->ColorFill(surfRain, &drop, 0x6060);

                drop.left = ripplePos[0] - 1;
                drop.right = ripplePos[0] + 1;
                drop.top = ripplePos[1] - 2;
                drop.bottom = ripplePos[1] + 2;
                device->ColorFill(surfRain, &drop, 0x6060);

                drop.left = ripplePos[0] - 1;
                drop.right = ripplePos[0] + 1;
                drop.top = ripplePos[1] - 1;
                drop.bottom = ripplePos[1] + 1;
                device->ColorFill(surfRain, &drop, 0x4040);
            }
        }

        // Apply wave equation numWaveSteps times
        // Uses double buffering to avoid reads and writes on the same target
        RenderTargetSwitcher rtsw(surfRippleBuffer, NULL);
        SurfaceDoubleBuffer doublebuffer;
        doublebuffer.init(texRain, surfRain, texRippleBuffer, surfRippleBuffer);

        effect->BeginPass(PASS_WAVESTEP);
        for (int i = 0; i != numWaveSteps; ++i) {
            device->SetRenderTarget(0, doublebuffer.sinkSurface());
            effect->SetTexture(ehTex4, doublebuffer.sourceTexture());
            effect->CommitChanges();

            device->DrawPrimitive(D3DPT_TRIANGLESTRIP, 0, 1);
            doublebuffer.cycle();
        }
        effect->EndPass();

        if (doublebuffer.sourceSurface() != surfRain) {
            device->StretchRect(surfRippleBuffer, 0, surfRain, 0, D3DTEXF_NONE);
        }
    } else if (resetRippleSurface) {
        // No weather - clear rain ripples
        device->ColorFill(surfRain, NULL, 0);
        resetRippleSurface = false;
    }

    // Player local ripples
    // Move ripple texture with player; lock to texel alignment to prevent visible jitter
    const D3DXVECTOR3* playerPos = (const D3DXVECTOR3*)mwBridge->PlayerPositionPointer();
    if (playerPos == nullptr) {
        return;
    }
    static int lastXpos = (int)floor(playerPos->x / waveTexWorldRes);
    static int lastYpos = (int)floor(playerPos->y / waveTexWorldRes);

    int newXpos = (int)floor(playerPos->x / waveTexWorldRes);
    int newYpos = (int)floor(playerPos->y / waveTexWorldRes);

    int shiftX = newXpos - lastXpos;
    int shiftY = newYpos - lastYpos;

    lastXpos = newXpos;
    lastYpos = newYpos;

    int shiftXp = (shiftX > 0) ? +shiftX : 0;
    int shiftXn = (shiftX < 0) ? -shiftX : 0;
    int shiftYp = (shiftY > 0) ? +shiftY : 0;
    int shiftYn = (shiftY < 0) ? -shiftY : 0;

    // Shift texture by (shiftX, shiftY) pixels
    RECT source;
    source.left = 1 + shiftXp;
    source.right = waveTexResolution - shiftXn;
    source.top = 1 + shiftYp;
    source.bottom = waveTexResolution - shiftYn;

    RECT target;
    target.left = 1 + shiftXn;
    target.right = waveTexResolution - shiftXp;
    target.top = 1 + shiftYn;
    target.bottom = waveTexResolution - shiftYp;

    device->ColorFill(surfRippleBuffer, 0, 0);
    device->StretchRect(surfRipples, &source, surfRippleBuffer, &target, D3DTEXF_NONE);

    // Water simulation; realigned water starts in surfRippleBuffer
    // Uses double buffering to avoid reads and writes on the same target
    RenderTargetSwitcher rtsw(surfRipples, NULL);
    SurfaceDoubleBuffer doublebuffer;
    doublebuffer.init(texRippleBuffer, surfRippleBuffer, texRipples, surfRipples);

    float rippleOrigin[2];
    float dz = playerPos->z - mwBridge->WaterLevel();
    if (dz < 0 && dz > -mwBridge->PlayerHeight()) {
        // Create waves around the player
        effect->BeginPass(PASS_PLAYERWAVE);
        for (int i = 0; i != numWaveSteps; ++i) {
            // Interpolate between starting and ending point, so that low framerates do not cause less waves
            float w = -(float)i / (float)numWaveSteps / (float)waveTexResolution;
            rippleOrigin[0] = w * shiftX + 0.5f;
            rippleOrigin[1] = w * shiftY + 0.5f;

            device->SetRenderTarget(0, doublebuffer.sinkSurface());
            effect->SetTexture(ehTex4, doublebuffer.sourceTexture());
            effect->SetFloatArray(ehRippleOrigin, rippleOrigin, 2);
            effect->CommitChanges();

            device->DrawPrimitive(D3DPT_TRIANGLESTRIP, 0, 1);
            doublebuffer.cycle();
        }
        effect->EndPass();
    }

    // Apply wave equation numWaveSteps times
    effect->BeginPass(PASS_WAVESTEP);
    for (int i = 0; i != numWaveSteps; ++i) {
        device->SetRenderTarget(0, doublebuffer.sinkSurface());
        effect->SetTexture(ehTex4, doublebuffer.sourceTexture());
        effect->CommitChanges();

        device->DrawPrimitive(D3DPT_TRIANGLESTRIP, 0, 1);
        doublebuffer.cycle();
    }
    effect->EndPass();

    if (doublebuffer.sourceSurface() != surfRipples) {
        device->StretchRect(surfRippleBuffer, 0, surfRipples, 0, D3DTEXF_NONE);
    }

    // Set wave texture world origin
    static float halfWaveTexWorldSize = 0.5f * waveTexWorldRes * waveTexResolution;
    rippleOrigin[0] = lastXpos*waveTexWorldRes - halfWaveTexWorldSize;
    rippleOrigin[1] = lastYpos*waveTexWorldRes - halfWaveTexWorldSize;
    effect->SetFloatArray(ehRippleOrigin, rippleOrigin, 2);

    // Set weather-dependent wave height
    effect->SetFloat(ehWaveHeight, (float)Configuration.DL.WaterWaveHeight);
}

bool DistantLand::waterVolumeDrawn = false;
float DistantLand::waterPlaneTint[3] = { 0.0f, 0.0f, 0.0f };
bool DistantLand::distantWaterLoaded = false;
bool DistantLand::distantWaterInView = false;

// A surface mesh of a water volume, held until the scene it was submitted in ends.
struct PendingWaterVolume {
    RenderedState rs;
    bool reflectsScene;
    // The colour of the water: the emissive colour of the material. Black for the usual colour.
    float tint[3];
    // The look slot of the material; 0 for the standard look
    unsigned int lookSlot;
};
static std::vector<PendingWaterVolume> pendingWaterVolumes;

// renderWaterVolume - Takes the surface mesh of a water volume, as the game submits it, to draw
// it with the water shading instead of its own material. The draw happens in flushWaterVolumes,
// so that every surface refracts and reflects the same frame, without the other surfaces in it.
// Returns false when the draw should go ahead unchanged.
bool DistantLand::renderWaterVolume(const RenderedState* rs, bool reflectsScene, const D3DCOLORVALUE& tint, unsigned int lookSlot) {
    if (!canRenderDistantLand() || isRenderCached) {
        return false;
    }

    rs->vb->AddRef();
    rs->ib->AddRef();
    if (rs->texture) {
        rs->texture->AddRef();
    }
    pendingWaterVolumes.push_back({ *rs, reflectsScene, { tint.r, tint.g, tint.b }, lookSlot });
    waterVolumeDrawn = true;
    return true;
}

// discardWaterVolumes - Drops the surfaces that were taken and not drawn.
void DistantLand::discardWaterVolumes() {
    for (auto& pending : pendingWaterVolumes) {
        pending.rs.vb->Release();
        pending.rs.ib->Release();
        if (pending.rs.texture) {
            pending.rs.texture->Release();
        }
    }
    pendingWaterVolumes.clear();
}

// flushWaterVolumes - Draws the surfaces taken by renderWaterVolume, all from one copy of the frame.
// withDistant: draw the water among the distant statics as well, from the same copy. It is the
// same water farther away than the game draws, so it is wanted once per frame, with the first flush.
void DistantLand::flushWaterVolumes(bool withDistant) {
    const bool distantWanted = withDistant && distantWaterInView;
    distantWaterInView = false;
    if (pendingWaterVolumes.empty() && !distantWanted) {
        return;
    }

    auto mwBridge = MWBridge::get();
    // From below, the planar reflection is the one of the volume the eye is in.
    const bool underwater = mwBridge->IsUnderwater(eyePos.z);
    const bool drawDistant = distantWanted && !underwater && canRenderDistantLand() && !isRenderCached;
    if (pendingWaterVolumes.empty() && !drawDistant) {
        return;
    }

    IDirect3DStateBlock9* stateSaved;
    UINT passes;

    // Save state block manually since we can change FVF/decl
    device->CreateStateBlock(D3DSBT_ALL, &stateSaved);
    effect->Begin(&passes, D3DXFX_DONOTSAVESTATE);

    IDirect3DTexture9* texRefract = PostShaders::borrowBuffer(0);
    effect->SetTexture(ehTex0, texReflection);
    effect->SetTexture(ehTex1, texWater);
    effect->SetTexture(ehTex2, texRefract);
    effect->SetTexture(ehTex3, texDepthFrame);
    // The mesh is not tessellated for wave displacement, which would only rock it.
    const bool ripples = (Configuration.MGEFlags & DYNAMIC_RIPPLES) != 0;
    float waveHeight = 0.0f;
    if (ripples) {
        effect->SetTexture(ehTex4, texRain);
        effect->SetTexture(ehTex5, texRipples);
        effect->GetFloat(ehWaveHeight, &waveHeight);
        effect->SetFloat(ehWaveHeight, 0.0f);
    }

    // A surface near the player is drawn at every depth.
    effect->SetFloat(ehWaterVolumeHandoff, 0.0f);

    // Each surface is drawn with the pass of its look: the water shader of a mod, or the
    // standard one. From below all surfaces have the underwater pass.
    {
        int passInUse = -1;
        for (const auto& pending : pendingWaterVolumes) {
            const RenderedState* rs = &pending.rs;
            const WaterLook& look = WaterLooks::get(pending.lookSlot);
            int pass = PASS_RENDERUNDERWATER;
            if (!underwater) {
                pass = waterShaderPass(look.shader);
                if (pass < 0) {
                    pass = PASS_RENDERWATERVOLUME;
                }
            }
            effect->SetMatrix(ehWorld, &rs->worldTransforms[0]);
            effect->SetFloatArray(ehWaterVolumeTint, pending.tint, 3);
            // The look of the surface. The vertex colour counts only when the look asks for it
            // and the mesh has one: 1 for the tint, 2 for the opacity.
            const float flow[4] = { look.flow[0], look.flow[1], look.speed, look.scale };
            float vertexUse = 0.0f;
            if ((rs->fvf & D3DFVF_DIFFUSE) != 0) {
                vertexUse += (look.flags & WATER_LOOK_TINT_FROM_VERTEX) != 0 ? 1.0f : 0.0f;
                vertexUse += (look.flags & WATER_LOOK_OPACITY_FROM_VERTEX) != 0 ? 2.0f : 0.0f;
            }
            const float mix[4] = { look.glow, look.opacity, vertexUse, pending.reflectsScene ? 1.0f : 0.0f };
            effect->SetFloatArray(ehWaterVolumeFlow, flow, 4);
            effect->SetFloatArray(ehWaterVolumeMix, mix, 4);
            if (pass >= PASS_WATERSHADER_FIRST) {
                effect->SetFloatArray(ehWaterVolumeParams, &look.params[0][0], 16);
                effect->SetTexture(ehMeshTex0, rs->texture);
            }
            if (pass != passInUse) {
                if (passInUse >= 0) {
                    effect->EndPass();
                }
                effect->BeginPass(pass);
                passInUse = pass;
            } else {
                effect->CommitChanges();
            }
            device->SetStreamSource(0, rs->vb, rs->vbOffset, rs->vbStride);
            device->SetIndices(rs->ib);
            device->SetFVF(rs->fvf);
            device->DrawIndexedPrimitive(rs->primType, rs->baseIndex, rs->minIndex, rs->vertCount, rs->startIndex, rs->primCount);
        }
        if (passInUse >= 0) {
            effect->EndPass();
        }
        effect->SetTexture(ehMeshTex0, NULL);
    }

    // The distant water comes after the surfaces near the player. The game draws a mesh that
    // reaches past its view distance in full, and the distant mesh lies a little behind it in
    // depth, so the depth test leaves the distant mesh out there and it is not shaded twice.
    if (drawDistant) {
        // Depth as the distant land has it, so that the land hides the water behind it.
        D3DXMATRIX distProj = mwProj;
        editProjectionZ(&distProj, kDistantNearPlane - 1e-2, Configuration.DL.DrawDist * kCellSize);
        effect->SetMatrix(ehProj, &distProj);
        device->SetVertexDeclaration(StaticDecl);
        // The game has the cell of the player and the eight cells around it.
        float loadedCells[4] = { 0.0f, 0.0f, 0.0f, 0.0f };
        int gridX, gridY;
        if (mwBridge->getExteriorGrid(gridX, gridY)) {
            loadedCells[0] = (gridX - 1) * kCellSize;
            loadedCells[1] = (gridY - 1) * kCellSize;
            loadedCells[2] = (gridX + 2) * kCellSize;
            loadedCells[3] = (gridY + 2) * kCellSize;
        }
        // Each mesh is drawn with the pass and the values of its look. The pass leaves a mesh
        // out nearer than the handoff depth, where the game draws the mesh itself. The game
        // does that only for a reference in a loaded cell, so any other mesh gets zero.
        int distantPassInUse = -1;
        visWaterShared.RenderWater(device, SIZEOFSTATICVERT, [&](const RenderMesh& mesh) {
            const WaterLook& look = WaterLooks::distant(mesh.water);
            const bool reflectsScene = mesh.water >= WaterLooks::firstDistantLook
                ? (look.flags & WATER_LOOK_REFLECTS_SCENE) != 0
                : mesh.water == 1;
            int pass = waterShaderPass(look.shader);
            pass = pass < 0 ? PASS_RENDERWATERVOLUME_DISTANT : pass + 1;

            const float x = mesh.transform._41, y = mesh.transform._42;
            const bool loaded = x >= loadedCells[0] && y >= loadedCells[1] && x < loadedCells[2] && y < loadedCells[3];
            const float flow[4] = { look.flow[0], look.flow[1], look.speed, look.scale };
            const float mix[4] = { look.glow, look.opacity, 0.0f, (reflectsScene ? 1.0f : 0.0f) + 2.0f };
            effect->SetFloat(ehWaterVolumeHandoff, loaded ? nearViewRange : 0.0f);
            effect->SetMatrix(ehWorld, &mesh.transform);
            effect->SetFloatArray(ehWaterVolumeFlow, flow, 4);
            effect->SetFloatArray(ehWaterVolumeMix, mix, 4);
            if (pass >= PASS_WATERSHADER_FIRST) {
                effect->SetFloatArray(ehWaterVolumeParams, &look.params[0][0], 16);
                effect->SetTexture(ehMeshTex0, mesh.tex);
            }
            if (pass != distantPassInUse) {
                if (distantPassInUse >= 0) {
                    effect->EndPass();
                }
                effect->BeginPass(pass);
                distantPassInUse = pass;
            } else {
                effect->CommitChanges();
            }
        });
        if (distantPassInUse >= 0) {
            effect->EndPass();
        }
        effect->SetMatrix(ehProj, &mwProj);
    }

    if (ripples) {
        effect->SetFloat(ehWaveHeight, waveHeight);
    }
    effect->End();
    stateSaved->Apply();
    stateSaved->Release();

    discardWaterVolumes();
}

void DistantLand::renderWaterPlane() {
    D3DXMATRIX m;
    IDirect3DTexture9* texRefract = PostShaders::borrowBuffer(0);

    D3DXMatrixTranslation(&m, eyePos.x, eyePos.y, MWBridge::get()->WaterLevel());
    effect->SetMatrix(ehWorld, &m);
    effect->SetTexture(ehTex0, texReflection);
    effect->SetTexture(ehTex1, texWater);
    effect->SetTexture(ehTex2, texRefract);
    effect->SetTexture(ehTex3, texDepthFrame);
    if (Configuration.MGEFlags & DYNAMIC_RIPPLES) {
        effect->SetTexture(ehTex4, texRain);
        effect->SetTexture(ehTex5, texRipples);
    }
    effect->CommitChanges();

    device->SetVertexDeclaration(WaterDecl);
    device->SetStreamSource(0, vbWater, 0, 12);
    device->SetIndices(ibWater);
    device->DrawIndexedPrimitive(D3DPT_TRIANGLELIST, 0, 0, numWaterVerts, 0, numWaterTris);
}
