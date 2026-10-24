
#include "gammaramp.h"
#include "proxydx/d3d8header.h"
#include "support/log.h"

#include <cstring>



static D3DGAMMARAMP ramp;
static bool rampSet = false;
static bool rampIdentity = true;
static bool tableStale = true;

// The first pass after a new ramp writes its effect on the centre pixel to the log.
static bool logNext = false;

static IDirect3DTexture9* texCopy = nullptr;
static IDirect3DTexture9* texTable = nullptr;
static ID3DXEffect* effect = nullptr;
static bool effectFailed = false;

// The back buffer through a table of 256 x 1: red, green and blue of each 8-bit value.
static const char* effectSource =
    "texture tex; texture table;\n"
    "sampler sPicture = sampler_state { texture = <tex>; minfilter = point; magfilter = point; mipfilter = none; addressu = clamp; addressv = clamp; };\n"
    "sampler sTable = sampler_state { texture = <table>; minfilter = point; magfilter = point; mipfilter = none; addressu = clamp; addressv = clamp; };\n"
    "float4 ps(float2 t : TEXCOORD0) : COLOR0 {\n"
    "    float4 c = tex2D(sPicture, t);\n"
    "    float3 at = (floor(saturate(c.rgb) * 255 + 0.5) + 0.5) / 256;\n"
    "    return float4(tex2D(sTable, float2(at.r, 0.5)).r, tex2D(sTable, float2(at.g, 0.5)).g, tex2D(sTable, float2(at.b, 0.5)).b, c.a);\n"
    "}\n"
    "technique T { pass P {\n"
    "    VertexShader = null; PixelShader = compile ps_2_0 ps();\n"
    "    ZEnable = false; ZWriteEnable = false; StencilEnable = false; AlphaBlendEnable = false; AlphaTestEnable = false;\n"
    "    CullMode = none; FogEnable = false; ColorWriteEnable = 15; ScissorTestEnable = false; SRGBWriteEnable = false;\n"
    "} }\n";

// True when the ramp gives each 8-bit value back. The engine's ramp for gamma 1 truncates
// i / 255 * 65535, so a value can be i * 257 - 1.
static bool isIdentity(const D3DGAMMARAMP& r) {
    for (int i = 0; i < 256; ++i) {
        const WORD* channels[3] = { r.red, r.green, r.blue };
        for (const WORD* c : channels) {
            if ((c[i] * 255u + 32767u) / 65535u != unsigned(i)) {
                return false;
            }
        }
    }
    return true;
}

void GammaRamp::set(const D3DGAMMARAMP* r) {
    if (!r) {
        return;
    }
    ramp = *r;
    rampSet = true;
    rampIdentity = isIdentity(ramp);
    tableStale = true;
    logNext = !rampIdentity;
    if (rampIdentity) {
        LOG::logline("-- Gamma ramp: the identity; no pass");
    }
}

// The centre pixel of a surface, through a render target and a system memory surface of
// 1 x 1. 0 when a read fails.
static D3DCOLOR centrePixel(IDirect3DDevice9* device, IDirect3DSurface9* surface, const D3DSURFACE_DESC& desc) {
    IDirect3DSurface9* target = nullptr;
    IDirect3DSurface9* memory = nullptr;
    D3DCOLOR colour = 0;
    if (device->CreateRenderTarget(1, 1, D3DFMT_A8R8G8B8, D3DMULTISAMPLE_NONE, 0, FALSE, &target, nullptr) == D3D_OK
        && device->CreateOffscreenPlainSurface(1, 1, D3DFMT_A8R8G8B8, D3DPOOL_SYSTEMMEM, &memory, nullptr) == D3D_OK) {
        RECT centre = { LONG(desc.Width / 2), LONG(desc.Height / 2), LONG(desc.Width / 2 + 1), LONG(desc.Height / 2 + 1) };
        D3DLOCKED_RECT locked;
        if (device->StretchRect(surface, &centre, target, nullptr, D3DTEXF_NONE) == D3D_OK
            && device->GetRenderTargetData(target, memory) == D3D_OK
            && memory->LockRect(&locked, nullptr, D3DLOCK_READONLY) == D3D_OK) {
            colour = *static_cast<const D3DCOLOR*>(locked.pBits);
            memory->UnlockRect();
        }
    }
    if (target) {
        target->Release();
    }
    if (memory) {
        memory->Release();
    }
    return colour;
}

void GammaRamp::get(D3DGAMMARAMP* r) {
    if (!r) {
        return;
    }
    if (rampSet) {
        *r = ramp;
        return;
    }
    for (int i = 0; i < 256; ++i) {
        r->red[i] = r->green[i] = r->blue[i] = WORD(i * 257);
    }
}

// Makes the effect, the table and the copy of the back buffer when they are not there.
static bool prepare(IDirect3DDevice9* device, const D3DSURFACE_DESC& back) {
    if (!effect && !effectFailed) {
        ID3DXBuffer* errors = nullptr;
        HRESULT hr = D3DXCreateEffect(device, effectSource, UINT(std::strlen(effectSource)), nullptr, nullptr, D3DXFX_LARGEADDRESSAWARE, nullptr, &effect, &errors);
        if (hr != D3D_OK) {
            LOG::logline("!! Gamma ramp: the effect does not compile: %s", errors ? reinterpret_cast<const char*>(errors->GetBufferPointer()) : "no message");
            effectFailed = true;
        }
        if (errors) {
            errors->Release();
        }
    }
    if (!effect) {
        return false;
    }
    if (!texTable && device->CreateTexture(256, 1, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &texTable, nullptr) != D3D_OK) {
        texTable = nullptr;
        return false;
    }
    if (texCopy) {
        D3DSURFACE_DESC copy;
        texCopy->GetLevelDesc(0, &copy);
        if (copy.Width != back.Width || copy.Height != back.Height || copy.Format != back.Format) {
            texCopy->Release();
            texCopy = nullptr;
        }
    }
    if (!texCopy && device->CreateTexture(back.Width, back.Height, 1, D3DUSAGE_RENDERTARGET, back.Format, D3DPOOL_DEFAULT, &texCopy, nullptr) != D3D_OK) {
        texCopy = nullptr;
        return false;
    }
    if (tableStale) {
        D3DLOCKED_RECT locked;
        if (texTable->LockRect(0, &locked, nullptr, 0) != D3D_OK) {
            return false;
        }
        auto texels = static_cast<DWORD*>(locked.pBits);
        auto eight = [](WORD v) { return DWORD((v * 255u + 32767u) / 65535u); };
        for (int i = 0; i < 256; ++i) {
            texels[i] = D3DCOLOR_ARGB(255, eight(ramp.red[i]), eight(ramp.green[i]), eight(ramp.blue[i]));
        }
        texTable->UnlockRect(0);
        tableStale = false;
    }
    return true;
}

void GammaRamp::apply(IDirect3DDevice9* device) {
    if (!rampSet || rampIdentity || !device) {
        return;
    }

    IDirect3DSurface9* back = nullptr;
    if (device->GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &back) != D3D_OK) {
        return;
    }
    D3DSURFACE_DESC desc;
    back->GetDesc(&desc);
    IDirect3DSurface9* copy = nullptr;
    if (!prepare(device, desc) || texCopy->GetSurfaceLevel(0, &copy) != D3D_OK) {
        back->Release();
        return;
    }
    // A multisampled back buffer resolves into the copy.
    HRESULT hr = device->StretchRect(back, nullptr, copy, nullptr, D3DTEXF_NONE);
    copy->Release();
    if (hr != D3D_OK) {
        back->Release();
        return;
    }

    // The engine keeps its own copy of the device state: all of it goes back after the pass.
    // A state block does not hold the targets.
    IDirect3DStateBlock9* stateSaved = nullptr;
    if (FAILED(device->CreateStateBlock(D3DSBT_ALL, &stateSaved))) {
        back->Release();
        return;
    }
    IDirect3DSurface9* targetSaved = nullptr;
    IDirect3DSurface9* depthSaved = nullptr;
    device->GetRenderTarget(0, &targetSaved);
    device->GetDepthStencilSurface(&depthSaved);

    device->SetRenderTarget(0, back);
    device->SetDepthStencilSurface(nullptr);
    D3DVIEWPORT9 vp = { 0, 0, desc.Width, desc.Height, 0.0f, 1.0f };
    device->SetViewport(&vp);

    const float w = float(desc.Width), h = float(desc.Height);
    struct { float x, y, z, rhw, u, v; } quad[4] = {
        { -0.5f, h - 0.5f, 0.0f, 1.0f, 0.0f, 1.0f },
        { -0.5f, -0.5f, 0.0f, 1.0f, 0.0f, 0.0f },
        { w - 0.5f, h - 0.5f, 0.0f, 1.0f, 1.0f, 1.0f },
        { w - 0.5f, -0.5f, 0.0f, 1.0f, 1.0f, 0.0f },
    };

    const D3DCOLOR before = logNext ? centrePixel(device, back, desc) : 0;

    if (device->BeginScene() == D3D_OK) {
        device->SetFVF(D3DFVF_XYZRHW | D3DFVF_TEX1);
        effect->SetTexture(effect->GetParameterByName(0, "tex"), texCopy);
        effect->SetTexture(effect->GetParameterByName(0, "table"), texTable);
        UINT passes;
        effect->Begin(&passes, D3DXFX_DONOTSAVESTATE);
        effect->BeginPass(0);
        device->DrawPrimitiveUP(D3DPT_TRIANGLESTRIP, 2, quad, sizeof(quad[0]));
        effect->EndPass();
        effect->End();
        device->EndScene();
    }

    device->SetRenderTarget(0, targetSaved);
    device->SetDepthStencilSurface(depthSaved);
    if (targetSaved) {
        targetSaved->Release();
    }
    if (depthSaved) {
        depthSaved->Release();
    }
    stateSaved->Apply();
    stateSaved->Release();

    if (logNext) {
        logNext = false;
        const D3DCOLOR after = centrePixel(device, back, desc);
        LOG::logline("-- Gamma ramp: a look-up table onto the back buffer; red 64 -> %u, 128 -> %u, 192 -> %u (of 65535); the centre pixel %06X -> %06X",
            ramp.red[64], ramp.red[128], ramp.red[192], unsigned(before & 0xFFFFFF), unsigned(after & 0xFFFFFF));
    }
    back->Release();
}

void GammaRamp::release() {
    if (texCopy) {
        texCopy->Release();
        texCopy = nullptr;
    }
    if (texTable) {
        texTable->Release();
        texTable = nullptr;
    }
    if (effect) {
        effect->Release();
        effect = nullptr;
    }
    effectFailed = false;
    tableStale = true;
}
