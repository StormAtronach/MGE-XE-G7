#include "drawstatelog.h"
#include "proxydx/d3d8header.h"
#include "support/log.h"

#include <cstdarg>
#include <cstdio>
#include <cstdlib>



// The frames of a window: 0 when the switch is off.
static const int windowFrames = [] {
    char value[16] = {};
    const DWORD length = GetEnvironmentVariableA("MGE_XE_DRAW_STATE_LOG", value, sizeof(value));
    if (length == 0 || length >= sizeof(value)) {
        return 0;
    }
    const int frames = std::atoi(value);
    return frames < 1 ? 0 : (frames > 600 ? 600 : frames);
}();

static const char* const triggerFile = "mge-draw-state.trigger";
// The file is looked for at each 30th Present only.
static const unsigned triggerFileInterval = 30;

static unsigned presents = 0;
static int framesLeft = 0;
static unsigned windowFrame = 0;
static unsigned windowDraws = 0;
static unsigned frameDraw = 0;
static bool keyWasDown = false;

static const char* name(const char* const* names, unsigned count, DWORD value) {
    return value < count && names[value] ? names[value] : "?";
}

static const char* compareName(DWORD value) {
    static const char* const names[] = { nullptr, "never", "less", "equal", "lessequal", "greater", "notequal", "greaterequal", "always" };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* blendName(DWORD value) {
    static const char* const names[] = {
        nullptr, "zero", "one", "srccolor", "invsrccolor", "srcalpha", "invsrcalpha", "destalpha", "invdestalpha",
        "destcolor", "invdestcolor", "srcalphasat", "bothsrcalpha", "bothinvsrcalpha", "blendfactor", "invblendfactor",
    };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* blendOpName(DWORD value) {
    static const char* const names[] = { nullptr, "add", "subtract", "revsubtract", "min", "max" };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* stageOpName(DWORD value) {
    static const char* const names[] = {
        nullptr, "disable", "selectarg1", "selectarg2", "modulate", "modulate2x", "modulate4x", "add", "addsigned",
        "addsigned2x", "subtract", "addsmooth", "blenddiffusealpha", "blendtexturealpha", "blendfactoralpha",
        "blendtexturealphapm", "blendcurrentalpha", "premodulate", "modulatealpha_addcolor", "modulatecolor_addalpha",
        "modulateinvalpha_addcolor", "modulateinvcolor_addalpha", "bumpenvmap", "bumpenvmapluminance", "dotproduct3",
        "multiplyadd", "lerp",
    };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* primitiveName(DWORD value) {
    static const char* const names[] = { nullptr, "points", "lines", "linestrip", "trilist", "tristrip", "trifan" };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* fogModeName(DWORD value) {
    static const char* const names[] = { "none", "exp", "exp2", "linear" };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

static const char* colorSourceName(DWORD value) {
    static const char* const names[] = { "material", "color1", "color2" };
    return name(names, sizeof(names) / sizeof(names[0]), value);
}

// A stage argument (D3DTA_*): the source, then "~" for the complement and ".a" for the alpha
// replicate.
static const char* argumentName(DWORD value, char* text, size_t size) {
    static const char* const sources[] = { "diffuse", "current", "texture", "tfactor", "specular", "temp", "constant" };
    std::snprintf(text, size, "%s%s%s", (value & D3DTA_COMPLEMENT) ? "~" : "",
        name(sources, sizeof(sources) / sizeof(sources[0]), value & D3DTA_SELECTMASK),
        (value & D3DTA_ALPHAREPLICATE) ? ".a" : "");
    return text;
}

// A text that grows by formatted parts and stops at its end.
struct Line {
    char text[2048];
    size_t used = 0;

    void add(const char* format, ...) {
        if (used >= sizeof(text) - 1) {
            return;
        }
        va_list args;
        va_start(args, format);
        const int written = std::vsnprintf(text + used, sizeof(text) - used, format, args);
        va_end(args);
        if (written > 0) {
            used += size_t(written) < sizeof(text) - used ? size_t(written) : sizeof(text) - 1 - used;
        }
    }
};

static bool gameHasFocus() {
    DWORD process = 0;
    GetWindowThreadProcessId(GetForegroundWindow(), &process);
    return process == GetCurrentProcessId();
}

static bool triggered() {
    const bool keyDown = (GetAsyncKeyState(VK_PAUSE) & 0x8000) != 0;
    const bool keyPressed = keyDown && !keyWasDown && gameHasFocus();
    keyWasDown = keyDown;
    if (keyPressed) {
        return true;
    }
    if (presents % triggerFileInterval != 0 || GetFileAttributesA(triggerFile) == INVALID_FILE_ATTRIBUTES) {
        return false;
    }
    DeleteFileA(triggerFile);
    return true;
}

void DrawStateLog::onPresent() {
    if (windowFrames == 0) {
        return;
    }
    ++presents;
    frameDraw = 0;

    if (framesLeft > 0) {
        --framesLeft;
        ++windowFrame;
        if (framesLeft == 0) {
            LOG::logline("draw-state: the window ends: %u frames, %u draws", windowFrame, windowDraws);
        }
        return;
    }
    if (triggered()) {
        framesLeft = windowFrames;
        windowFrame = 0;
        windowDraws = 0;
        LOG::logline("draw-state: a window of %d frames starts at Present %u", windowFrames, presents);
    }
}

bool DrawStateLog::logging() {
    return framesLeft > 0;
}

void DrawStateLog::onDraw(IDirect3DDevice9* device, int scene, unsigned primitiveType, unsigned primitiveCount) {
    if (framesLeft == 0) {
        return;
    }
    const auto state = [device](D3DRENDERSTATETYPE type) {
        DWORD value = 0;
        device->GetRenderState(type, &value);
        return value;
    };

    DWORD fvf = 0;
    device->GetFVF(&fvf);
    IDirect3DVertexShader9* vertexShader = nullptr;
    IDirect3DPixelShader9* pixelShader = nullptr;
    device->GetVertexShader(&vertexShader);
    device->GetPixelShader(&pixelShader);
    const bool shaders[2] = { vertexShader != nullptr, pixelShader != nullptr };
    if (vertexShader) {
        vertexShader->Release();
    }
    if (pixelShader) {
        pixelShader->Release();
    }

    // The lights of the device that are on, as bits of their index (the expanded light limit
    // has 32).
    DWORD lights = 0;
    for (DWORD i = 0; i < 32; ++i) {
        BOOL enabled = FALSE;
        if (SUCCEEDED(device->GetLightEnable(i, &enabled)) && enabled) {
            lights |= 1u << i;
        }
    }

    Line line;
    line.add("draw-state frame %u scene %d draw %u: %s x%u fvf 0x%x vs %d ps %d",
        windowFrame, scene, frameDraw, primitiveName(primitiveType), primitiveCount, unsigned(fvf), shaders[0], shaders[1]);
    line.add(" | z %u write %u %s cull %u stencil %u",
        unsigned(state(D3DRS_ZENABLE)), unsigned(state(D3DRS_ZWRITEENABLE)), compareName(state(D3DRS_ZFUNC)),
        unsigned(state(D3DRS_CULLMODE)), unsigned(state(D3DRS_STENCILENABLE)));
    line.add(" | blend %u %s %s %s", unsigned(state(D3DRS_ALPHABLENDENABLE)), blendName(state(D3DRS_SRCBLEND)),
        blendName(state(D3DRS_DESTBLEND)), blendOpName(state(D3DRS_BLENDOP)));
    line.add(" | alphatest %u %s ref %u | mask 0x%x", unsigned(state(D3DRS_ALPHATESTENABLE)),
        compareName(state(D3DRS_ALPHAFUNC)), unsigned(state(D3DRS_ALPHAREF)), unsigned(state(D3DRS_COLORWRITEENABLE)));
    line.add(" | fog %u table %s vertex %s", unsigned(state(D3DRS_FOGENABLE)), fogModeName(state(D3DRS_FOGTABLEMODE)),
        fogModeName(state(D3DRS_FOGVERTEXMODE)));
    line.add(" | lighting %u lights 0x%x colorvertex %u diffuse %s ambient %s emissive %s",
        unsigned(state(D3DRS_LIGHTING)), unsigned(lights), unsigned(state(D3DRS_COLORVERTEX)),
        colorSourceName(state(D3DRS_DIFFUSEMATERIALSOURCE)), colorSourceName(state(D3DRS_AMBIENTMATERIALSOURCE)),
        colorSourceName(state(D3DRS_EMISSIVEMATERIALSOURCE)));
    // The normals of a draw are scaled to length 1 only with NORMALIZENORMALS: the diffuse
    // light of a normal of another length follows that length.
    const DWORD ambient = state(D3DRS_AMBIENT);
    line.add(" normalize %u specular %u ambient %u,%u,%u,%u", unsigned(state(D3DRS_NORMALIZENORMALS)),
        unsigned(state(D3DRS_SPECULARENABLE)), unsigned((ambient >> 16) & 0xFF), unsigned((ambient >> 8) & 0xFF),
        unsigned(ambient & 0xFF), unsigned(ambient >> 24));
    D3DMATERIAL9 material = {};
    device->GetMaterial(&material);
    const D3DCOLORVALUE* const colors[4] = { &material.Diffuse, &material.Ambient, &material.Specular, &material.Emissive };
    const char* const colorNames[4] = { "diffuse", "ambient", "specular", "emissive" };
    line.add(" | material");
    for (int i = 0; i < 4; ++i) {
        line.add(" %s %.3f,%.3f,%.3f,%.3f", colorNames[i], colors[i]->r, colors[i]->g, colors[i]->b, colors[i]->a);
    }
    line.add(" power %.3f", material.Power);

    for (DWORD stage = 0; stage < 4; ++stage) {
        const auto stageState = [device, stage](D3DTEXTURESTAGESTATETYPE type) {
            DWORD value = 0;
            device->GetTextureStageState(stage, type, &value);
            return value;
        };
        IDirect3DBaseTexture9* texture = nullptr;
        device->GetTexture(stage, &texture);
        const bool hasTexture = texture != nullptr;
        if (texture) {
            texture->Release();
        }
        char args[4][24];
        line.add(" | t%u tex %d color %s %s %s alpha %s %s %s tci 0x%x ttf %u", unsigned(stage), hasTexture,
            stageOpName(stageState(D3DTSS_COLOROP)), argumentName(stageState(D3DTSS_COLORARG1), args[0], sizeof(args[0])),
            argumentName(stageState(D3DTSS_COLORARG2), args[1], sizeof(args[1])), stageOpName(stageState(D3DTSS_ALPHAOP)),
            argumentName(stageState(D3DTSS_ALPHAARG1), args[2], sizeof(args[2])),
            argumentName(stageState(D3DTSS_ALPHAARG2), args[3], sizeof(args[3])),
            unsigned(stageState(D3DTSS_TEXCOORDINDEX)), unsigned(stageState(D3DTSS_TEXTURETRANSFORMFLAGS)));
    }

    LOG::logline("%s", line.text);
    ++frameDraw;
    ++windowDraws;
}
