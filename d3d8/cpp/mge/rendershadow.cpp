
#include "distantland.h"
#include "distantshader.h"
#include "configuration.h"
#include "mwbridge.h"
#include "proxydx/d3d8header.h"
#include "support/log.h"

#include <algorithm>
#include <cmath>



// Cascade half-widths in world units, the last covering the distant land draw distance
static float shadowCascadeRadius(int layer) {
    static const float fixedRadius[] = { 1000.0f, 4000.0f, 16000.0f };
    if (layer < 3) {
        return fixedRadius[layer];
    }
    return Configuration.DL.DrawDist * DistantLand::kCellSize;
}

// Vertical half-extent of the box a cascade covers around the eye, so casters and
// receivers this far above or below the eye are inside it at any sun angle
static float shadowCascadeHeight(float radius) {
    return radius;
}

// How far toward the sun the light frustum reaches past the box, so a hill a couple of
// cells away still casts into the near cascades at low sun
static const float shadowCasterReach = 2.0f * DistantLand::kCellSize;

// Lowest sun elevation the fit uses, in degrees. Below it the constant receiver bias
// detaches shadows from their casters (bias / tan), the caster reach no longer covers the
// relief, and ground texels stretch to many units along the light. The azimuth is kept, so
// shadows still point away from the sun; the receivers fade them out over the same band
// (shadowElevationFade in XE Mod Shadow Data.fx, whose top must match this)
static const float shadowMinElevation = 10.0f;

// Light-space half-extent, along one unit axis of the light basis, of a cylinder of the
// cascade radius and height around the eye
static float shadowExtentAlong(const D3DXVECTOR3& axis, float radius, float height) {
    return radius * std::sqrt(axis.x * axis.x + axis.y * axis.y) + height * std::fabs(axis.z);
}

// Texel size and light depth range of each cascade as last fitted, in world units
struct ShadowFit {
    float texel;
    float depth;
};
static ShadowFit shadowFit[DistantLand::kShadowCascades];

// Real-time length of the crossfade from the current atlas to the next one
static const float shadowBlendSeconds = 0.25f;



// Uploads shadow clip transforms for both atlases, current in [0, N) and next in [N, 2N),
// premultiplied by pre when given (inverse view for view space receivers), with the blend
// factor and both atlas textures
void DistantLand::uploadShadowMatrices(const D3DXMATRIX* pre) {
    D3DXMATRIX m[2 * kShadowCascades];
    for (int i = 0; i < kShadowCascades; ++i) {
        m[i] = pre ? (*pre) * smViewproj[shadowCurrent][i] : smViewproj[shadowCurrent][i];
        m[kShadowCascades + i] = pre ? (*pre) * smViewproj[shadowBuilding][i] : smViewproj[shadowBuilding][i];
    }
    effect->SetMatrixArray(ehShadowViewproj, m, 2 * kShadowCascades);
    effect->SetFloat(ehShadowBlend, shadowBuildComplete ? shadowBlend : 0.0f);
    effect->SetTexture(ehTex3, texShadow[shadowCurrent]);
    effect->SetTexture(ehTexShadowNext, texShadow[shadowBuilding]);
}

// Builds the next shadow atlas one cascade per frame, then cross-fades it in over
// shadowBlendSeconds of real time and swaps. Shadows therefore move continuously as the
// sun does, independent of frame rate, and casters cost one cascade per frame.
// Restores render state on return.
void DistantLand::renderShadowMap() {
    const DWORD now = GetTickCount();

    if (shadowBuildComplete) {
        shadowBlend = std::min(1.0f, (now - shadowBlendStart) * (0.001f / shadowBlendSeconds));
        if (shadowBlend >= 1.0f || !shadowCurrentValid) {
            shadowCurrent = shadowBuilding;
            shadowBuilding ^= 1;
            shadowCurrentValid = true;
            shadowBuildComplete = false;
            shadowBlend = 0;
        }
    }

    if (!shadowBuildComplete) {
        // Depth-only render into the next atlas, colour writes go to the null target
        RenderTargetSwitcher rtsw(surfShadowColor, surfShadow[shadowBuilding]);
        D3DVIEWPORT9 vp;
        device->GetViewport(&vp);

        // Unbind samplers, the atlases are still bound from the last receiver pass
        effect->SetTexture(ehTex0, 0);
        effect->SetTexture(ehTex2, 0);
        effect->SetTexture(ehTex3, 0);
        effect->SetTexture(ehTexShadowNext, 0);

        if (shadowBuildLayer == 0) {
            device->Clear(0, 0, D3DCLEAR_ZBUFFER, 0, 1.0, 0);
        }

        renderShadowLayer(shadowBuildLayer, shadowCascadeRadius(shadowBuildLayer));

        if (++shadowBuildLayer == kShadowCascades) {
            shadowBuildLayer = 0;
            shadowBuildComplete = true;
            shadowBlendStart = now;
        }

        // Reset viewport and the caster pass states that the distant land passes do not set
        device->SetViewport(&vp);
        device->SetRenderState(D3DRS_COLORWRITEENABLE, D3DCOLORWRITEENABLE_RED | D3DCOLORWRITEENABLE_GREEN | D3DCOLORWRITEENABLE_BLUE | D3DCOLORWRITEENABLE_ALPHA);
        device->SetRenderState(D3DRS_SLOPESCALEDEPTHBIAS, 0);
        device->SetRenderState(D3DRS_DEPTHBIAS, 0);
    }

    // Texel size and light depth range per cascade in world units, for receiver bias;
    // a layer not fitted yet gets the values its first fit would give
    D3DXVECTOR4 cascadeParams[kShadowCascades];
    for (int layer = 0; layer < kShadowCascades; ++layer) {
        if (shadowFit[layer].depth <= 0) {
            const float radius = shadowCascadeRadius(layer);
            shadowFit[layer].texel = 2.0f * radius / Configuration.DL.ShadowResolution;
            shadowFit[layer].depth = std::max(shadowCasterReach, radius) + radius;
        }
        cascadeParams[layer] = D3DXVECTOR4(shadowFit[layer].texel, shadowFit[layer].depth, 0, 0);
    }
    effect->SetVectorArray(ehShadowCascade, cascadeParams, kShadowCascades);

    // Distant land and statics receive next; they sample from world space
    uploadShadowMatrices(nullptr);
}

// Draws one cascade's casters into its atlas strip. The whole cascade box is rendered, not
// just the camera frustum: the atlas is reused for up to shadowBlendSeconds while the
// camera turns freely, so its content must not depend on the view direction
void DistantLand::renderShadowLayerGeneric(MWBridge* mwBridge, int layer, D3DXMATRIX* view, D3DXMATRIX* proj, VisibleSet& visible_set) {
    // Clip to atlas region with viewport
    const DWORD res = Configuration.DL.ShadowResolution;
    D3DVIEWPORT9 vp = { layer * res, 0, res, res, 0.0f, 1.0f };
    device->SetViewport(&vp);

    // Render land
    effectShadow->BeginPass(PASS_RENDERSHADOWMAP);

    if (mwBridge->IsExterior()) {
        renderDistantLand(effectShadow, view, proj);
    }

    effectShadow->EndPass();

    if (staticsUploaded) {
        // Render statics with their texture-atlas UV bounds
        effectShadow->BeginPass(PASS_RENDERSTATICSHADOWMAP);
        device->SetVertexDeclaration(StaticDecl);
        visible_set.Render(device, effectShadow, effect, &ehTex0, &ehHasAlpha, &ehHasVCol, &ehWorld, &ehUvBoundPalette, SIZEOFSTATICVERT, true);
        effectShadow->EndPass();
    }
}

void DistantLand::renderShadowLayer(int layer, float radius) {
    auto mwBridge = MWBridge::get();
    D3DXVECTOR3 lookAt, shadowCameraPos;
    D3DXMATRIX* view = &smView[layer], *proj = &smProj[layer], *viewproj = &smViewproj[shadowBuilding][layer];

    // Select light vector, sunPos during daytime, sunVec during night
    D3DXVECTOR4 lightVec = (sunPos.z > 0) ? -sunPos : sunVec;

    // Centre of projection is the eye, so the atlas stays valid however the camera turns
    // while it is reused; only eye translation ages it
    lookAt.x = eyePos.x;
    lookAt.y = eyePos.y;
    lookAt.z = eyePos.z;

    // World Z keeps light-space y vertical; within 26 degrees of the zenith it nears the
    // light direction and the basis would spin with every sun step, so world Y takes over
    // there (the sun path never runs north-south). The swap is one grid rotation, which
    // the atlas crossfade absorbs.
    D3DXVECTOR3 lightDir(lightVec.x, lightVec.y, lightVec.z);
    const float minSin = std::sin(shadowMinElevation * D3DX_PI / 180.0f);
    if (-lightDir.z < minSin) {
        const float horizontal = std::sqrt(lightDir.x * lightDir.x + lightDir.y * lightDir.y);
        if (horizontal > 1.0e-4f) {
            const float scale = std::cos(shadowMinElevation * D3DX_PI / 180.0f) / horizontal;
            lightDir.x *= scale;
            lightDir.y *= scale;
            lightDir.z = -minSin;
        }
    }
    const D3DXVECTOR3 up = (std::fabs(lightDir.z) < 0.9f) ? D3DXVECTOR3(0, 0, 1) : D3DXVECTOR3(0, 1, 0);

    // Orientation first, then size the box from its axes so a cylinder of the cascade
    // radius and height around the eye fits at any sun angle
    const D3DXVECTOR3 towardSun = lookAt - lightDir;
    D3DXMatrixLookAtRH(view, &towardSun, &lookAt, &up);
    const D3DXVECTOR3 axisX(view->_11, view->_21, view->_31);
    const D3DXVECTOR3 axisY(view->_12, view->_22, view->_32);
    const D3DXVECTOR3 axisZ(view->_13, view->_23, view->_33);
    const float height = shadowCascadeHeight(radius);
    const float halfX = shadowExtentAlong(axisX, radius, height);
    const float halfY = shadowExtentAlong(axisY, radius, height);
    const float halfZ = shadowExtentAlong(axisZ, radius, height);

    // The light camera sits toward the sun far enough that casters a couple of cells out
    // still land in the map; depth spans from there to the far side of the box
    const float zSun = std::max(shadowCasterReach, halfZ);
    shadowCameraPos = lookAt - lightDir * zSun;
    D3DXMatrixLookAtRH(view, &shadowCameraPos, &lookAt, &up);
    D3DXMatrixOrthoRH(proj, 2 * halfX, 2 * halfY, 0, zSun + halfZ);
    *viewproj = (*view) * (*proj);

    shadowFit[layer].texel = 2 * halfX / Configuration.DL.ShadowResolution;
    shadowFit[layer].depth = zSun + halfZ;

    // Snap to whole texels. The translation row is where the world origin lands in clip
    // space, so rounding it locks the sampling grid to the world for a given light direction
    const double quantizer = 2.0 / Configuration.DL.ShadowResolution;
    viewproj->_41 = float(quantizer * std::floor(viewproj->_41 / quantizer));
    viewproj->_42 = float(quantizer * std::floor(viewproj->_42 / quantizer));

    // Caster passes read shadowViewProj[0]; upload this cascade before it draws
    effect->SetMatrixArray(ehShadowViewproj, viewproj, 1);
    effectShadow->CommitChanges();

    // Cull
    ViewFrustum range_frustum(viewproj);

    visExtraShared.RemoveAll();
    if (staticsUploaded) {
        // because shadow meshes don't need to be sorted, we can read and write in parallel
        ipcClient.getVisibleMeshesCoarse(visExtraSharedId, range_frustum, VIS_STATIC);
    }

    renderShadowLayerGeneric(mwBridge, layer, view, proj, visExtraShared);
}

// renderShadow - Renders shadows (using blending) over Morrowind shadow receivers
void DistantLand::renderShadow() {
    if (!shadowCurrentValid) {
        return;
    }

    // Supply view space -> shadow clip space matrices and both atlases
    D3DXMATRIX inverseView;
    D3DXMatrixInverse(&inverseView, NULL, &mwView);
    uploadShadowMatrices(&inverseView);

    // Use an alpha threshold for solidity that isn't precisely equal to a commonly used value (such as 0.5).
    // Vertex interpolators can be slightly inaccurate and cause a value that should be constant across a triangle
    // to have interpolated fragment values that vary either side of the threshold and cause noise.
    const float alphaThreshold = 0.0101f;

    // Draw shadows over recorded renders
    const auto& recordMW_const = recordMW;
    for (const auto& i : recordMW_const) {
        // Additive alphas do not receive shadows
        if (i.blendEnable && i.destBlend == D3DBLEND_ONE) {
            continue;
        }

        // Fragment colour routing
        bool alphaDependent = i.alphaTest || i.blendEnable;
        effect->SetBool(ehHasVCol, alphaDependent && (i.fvf & D3DFVF_DIFFUSE) != 0);
        effect->SetFloat(ehMaterialAlpha, alphaDependent ? i.diffuseMaterial.a : 1.0f);

        // Only bind texture for alphas
        if (alphaDependent && i.texture) {
            effect->SetTexture(ehTex0, i.texture);
            effect->SetBool(ehHasAlpha, true);
            effect->SetFloat(ehAlphaRef, i.alphaTest ? (i.alphaRef / 255.0f) : alphaThreshold);
        } else {
            effect->SetTexture(ehTex0, 0);
            effect->SetBool(ehHasAlpha, false);
            effect->SetFloat(ehAlphaRef, -1.0f);
        }

        // Skin using worldview matrices for numerical accuracy
        const bool indexedSkinning = i.skinPaletteCount != 0;
        effect->SetBool(ehHasBones, i.vertexBlendState != 0);
        effect->SetInt(ehVertexBlendState, i.vertexBlendState);
        if (indexedSkinning) {
            effect->SetMatrixArray(
                ehVertexBlendPalette,
                recordedSkinPalettes.data() + i.skinPaletteOffset,
                i.skinPaletteCount
            );
        } else {
            effect->SetMatrixArray(ehVertexBlendPalette, i.worldViewTransforms, 4);
        }

        UINT pass;
        if (indexedSkinning) {
            pass = isPPLActive ? PASS_RENDERSHADOWFFE_INDEXED : PASS_RENDERSHADOW_INDEXED;
        } else {
            pass = isPPLActive ? PASS_RENDERSHADOWFFE : PASS_RENDERSHADOW;
        }
        effect->BeginPass(pass);
        effect->CommitChanges();

        // Ignore two-sided poly (cull none) mode, shadow casters are drawn with CW culling only,
        // which causes false shadows when cast on the reverse side (wrt normals) of a two-sided poly
        DWORD cull = (i.cullMode != D3DCULL_NONE) ? i.cullMode : (DWORD)D3DCULL_CW;
        device->SetRenderState(D3DRS_CULLMODE, cull);
        device->SetStreamSource(0, i.vb, i.vbOffset, i.vbStride);
        device->SetIndices(i.ib);
        device->SetFVF(i.fvf);
        device->DrawIndexedPrimitive(i.primType, i.baseIndex, i.minIndex, i.vertCount, i.startIndex, i.primCount);

        effect->EndPass();
    }
}

// renderShadowDebug - display shadow layers
void DistantLand::renderShadowDebug() {
    UINT passes;

    // Create shadow clip space -> camera clip space matrices
    D3DXMATRIX inverseShadowViewProj, cameraViewProj, shadowToCameraProj[kShadowCascades];

    D3DXMatrixMultiply(&cameraViewProj, &mwView, &mwProj);
    for (int i = 0; i < kShadowCascades; ++i) {
        D3DXMatrixInverse(&inverseShadowViewProj, NULL, &smViewproj[shadowCurrent][i]);
        D3DXMatrixMultiply(&shadowToCameraProj[i], &inverseShadowViewProj, &cameraViewProj);
    }

    // Display shadow layers in top right corner
    effect->Begin(&passes, D3DXFX_DONOTSAVESTATE);
    effect->BeginPass(PASS_DEBUGSHADOW);
    device->SetRenderState(D3DRS_CULLMODE, D3DCULL_CW);
    effect->SetTexture(ehTex3, texShadow[shadowCurrent]);
    effect->SetMatrixArray(ehVertexBlendPalette, shadowToCameraProj, kShadowCascades);
    effect->CommitChanges();
    device->SetVertexDeclaration(WaterDecl);
    device->SetStreamSource(0, vbFullFrame, 0, 12);
    device->DrawPrimitive(D3DPT_TRIANGLESTRIP, 0, 2);
    effect->EndPass();
    effect->End();
}
