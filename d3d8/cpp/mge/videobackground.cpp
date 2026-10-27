
#include "videobackground.h"
#include "mwbridge.h"
#include "mwpatches.h"
#include "support/log.h"
#include "tes3/tes3types.h"



#define BINKSURFACE32       3
#define BINKCOPYALL         0x80000000

struct Bink {
    DWORD width, height;
    DWORD frames, frameNum, lastFrameNum;
    DWORD frameRate, frameRateDiv;

    // and a lot more junk
};

typedef Bink* (_stdcall* BinkOpen_t)(const char*, DWORD);
typedef void (_stdcall* BinkClose_t)(Bink*);
typedef int (_stdcall* BinkDoFrame_t)(Bink*);
typedef int (_stdcall* BinkCopyToBuffer_t)(Bink*, void*, int, DWORD, DWORD, DWORD, DWORD);
typedef int (_stdcall* BinkWait_t)(Bink*);
typedef void (_stdcall* BinkNextFrame_t)(Bink*);

BinkOpen_t BinkOpen;
BinkClose_t BinkClose;
BinkDoFrame_t BinkDoFrame;
BinkCopyToBuffer_t BinkCopyToBuffer;
BinkWait_t BinkWait;
BinkNextFrame_t BinkNextFrame;

IDirect3DDevice9* VideoPatch::d;
VideoBackground VideoPatch::v;
int VideoPatch::state;
float VideoPatch::streamedWidth;
float VideoPatch::streamedHeight;

const char* videoPath = "Data Files\\video\\menu_background.bik";



bool VideoBackground::init() {

    HMODULE hDLL = LoadLibrary("binkw32.dll");
    if (hDLL == NULL) {
        return false;
    }

    BinkOpen = (BinkOpen_t)GetProcAddress(hDLL, "_BinkOpen@8");
    BinkClose = (BinkClose_t)GetProcAddress(hDLL, "_BinkClose@4");
    BinkDoFrame = (BinkDoFrame_t)GetProcAddress(hDLL, "_BinkDoFrame@4");
    BinkCopyToBuffer = (BinkCopyToBuffer_t)GetProcAddress(hDLL, "_BinkCopyToBuffer@28");
    BinkWait = (BinkWait_t)GetProcAddress(hDLL, "_BinkWait@4");
    BinkNextFrame = (BinkNextFrame_t)GetProcAddress(hDLL, "_BinkNextFrame@4");

    // If there is no replacer installed, signal failure but not an error
    if (GetFileAttributes(videoPath) == INVALID_FILE_ATTRIBUTES) {
        return false;
    }

    return true;
}

bool VideoBackground::begin(IDirect3DDevice9* device) {
    video = BinkOpen(videoPath, 0);
    if (!video) {
        LOG::logline("!! Video renderer: Bink failed to open video file.");
        end();
        return false;
    }

    HRESULT hr = device->CreateOffscreenPlainSurface(video->width, video->height, D3DFMT_X8R8G8B8, D3DPOOL_DEFAULT, &surface, 0);
    if (hr != D3D_OK) {
        LOG::logline("!! Video renderer: Failed to create a surface for video output.");
        end();
        return false;
    }

    return true;
}

// The wgpu path: a texture of the game's device in place of the surface. Each write of it
// goes to the helper (the plugin's TEXTURE_UPDATE record): a frame of the video's size.
bool VideoBackground::beginStreamed(IDirect3DDevice8* device) {
    video = BinkOpen(videoPath, 0);
    if (!video) {
        LOG::logline("!! Video renderer: Bink failed to open video file.");
        end();
        return false;
    }

    if (device->CreateTexture(video->width, video->height, 1, 0, D3DFMT_X8R8G8B8, D3DPOOL_MANAGED, &texture) != D3D_OK) {
        LOG::logline("!! Video renderer: no texture of %lu x %lu for the video on the wgpu path.", video->width, video->height);
        end();
        return false;
    }
    LOG::logline("-- Video renderer: the menu video (%lu x %lu) plays on the wgpu path.", video->width, video->height);
    return true;
}

// When the next frame of the video is due, Bink decodes it into the memory that the caller
// locked; the video loops by itself. `lock` returns false when there is no memory.
template <typename Lock, typename Unlock>
static void decodeDueFrame(Bink* video, Lock lock, Unlock unlock) {
    if (BinkWait(video)) {
        return;
    }
    BinkDoFrame(video);
    D3DLOCKED_RECT rect;
    if (lock(rect)) {
        BinkCopyToBuffer(video, rect.pBits, rect.Pitch, video->height, 0, 0, BINKSURFACE32 | BINKCOPYALL);
        unlock();
    }
    BinkNextFrame(video);
}

void VideoBackground::render(IDirect3DDevice9* device) {
    decodeDueFrame(video,
        [&](D3DLOCKED_RECT& rect) { return surface->LockRect(&rect, 0, D3DLOCK_DISCARD) == D3D_OK; },
        [&]() { surface->UnlockRect(); });

    IDirect3DSurface9* backbuffer;
    device->GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &backbuffer);
    device->StretchRect(surface, 0, backbuffer, 0, D3DTEXF_LINEAR);
    backbuffer->Release();
}

// The wgpu path: the frame over the whole back buffer of `width` x `height`, stretched as the
// StretchRect of Direct3D 9 does, in one draw of the kind of the game's films (pre-transformed
// vertices with a colour and a texture coordinate, FVF 0x144). The helper draws such a draw with
// its screen pipeline: no depth test or write, no blend, a linear sampler; of the device's
// state it takes the texture of stage 0 and the FVF, which this function puts back after.
void VideoBackground::renderStreamed(IDirect3DDevice8* device, float width, float height) {
    decodeDueFrame(video,
        [&](D3DLOCKED_RECT& rect) { return texture->LockRect(0, &rect, 0, 0) == D3D_OK; },
        [&]() { texture->UnlockRect(0); });

    struct ScreenVertex {
        float x, y, z, rhw;
        DWORD colour;
        float u, v;
    };
    // The centre of a pixel is at a whole number in Direct3D 8: the edges are half a pixel out.
    const float left = -0.5f, top = -0.5f, right = width - 0.5f, bottom = height - 0.5f;
    const ScreenVertex quad[4] = {
        { left, top, 0.0f, 1.0f, 0xFFFFFFFF, 0.0f, 0.0f },
        { right, top, 0.0f, 1.0f, 0xFFFFFFFF, 1.0f, 0.0f },
        { left, bottom, 0.0f, 1.0f, 0xFFFFFFFF, 0.0f, 1.0f },
        { right, bottom, 0.0f, 1.0f, 0xFFFFFFFF, 1.0f, 1.0f },
    };

    IDirect3DBaseTexture8* stageTexture = nullptr;
    DWORD vertexShader = 0;
    device->GetTexture(0, &stageTexture);
    device->GetVertexShader(&vertexShader);

    device->SetTexture(0, texture);
    device->SetVertexShader(D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1);
    device->DrawPrimitiveUP(D3DPT_TRIANGLESTRIP, 2, quad, sizeof(ScreenVertex));

    device->SetTexture(0, stageTexture);
    device->SetVertexShader(vertexShader);
    if (stageTexture) {
        stageTexture->Release();
    }
}

void VideoBackground::end() {
    if (video) {
        BinkClose(video);
        video = 0;
    }

    if (surface) {
        surface->Release();
        surface = 0;
    }

    if (texture) {
        texture->Release();
        texture = 0;
    }
}



struct VideoState {
    enum {
        PreMainScreen = 0,
        WaitForMain = 1,
        Playing = 2,
        Done = -1
    };
};

void VideoPatch::start(IDirect3DDevice9* device) {
    if (state == VideoState::PreMainScreen) {
        d = device;
        if (v.init()) {
            state = VideoState::WaitForMain;
            MWPatches::redirectMenuBackground(VideoPatch::patch);
        }
        else {
            state = VideoState::Done;
        }
    }
}

// The steps of the video, the same on both paths: it begins when the loading screen before
// the main menu is gone, and ends at the first loading bar after it (a game load or the new
// game's film). `begin` makes the video's surface or texture.
void VideoPatch::advance(bool (*begin)()) {
    auto mwBridge = MWBridge::get();

    if (state == VideoState::WaitForMain) {
        // Wait until pre-main menu loading screen is gone
        if (!mwBridge->isLoadingBar()) {
            state = begin() ? VideoState::Playing : VideoState::Done;
        }
    }
    else if (state == VideoState::Playing && mwBridge->isLoadingBar()) {
        // Release when first loading bar appears (includes new game cutscene)
        state = VideoState::Done;
        MWPatches::redirectMenuBackground(nullptr);
        v.end();
    }
}

bool VideoPatch::beginProxy() {
    return v.begin(d);
}

// The game's device on the wgpu path: the nirender plugin's, which the game's renderer holds.
static IDirect3DDevice8* gameDevice() {
    auto worldController = TES3::WorldController::get();
    return worldController && worldController->renderer ? worldController->renderer->d3dDevice : nullptr;
}

bool VideoPatch::beginStreamed() {
    auto device = gameDevice();
    return device && v.beginStreamed(device);
}

// The device of `start` is this one (DistantLand::device, the real device).
void VideoPatch::monitor(IDirect3DDevice9*) {
    advance(&VideoPatch::beginProxy);
}

void VideoPatch::monitorStreamed(unsigned width, unsigned height) {
    streamedWidth = float(width);
    streamedHeight = float(height);
    if (state == VideoState::PreMainScreen) {
        if (v.init()) {
            state = VideoState::WaitForMain;
            MWPatches::redirectMenuBackground(VideoPatch::patch);
        }
        else {
            state = VideoState::Done;
        }
    }
    advance(&VideoPatch::beginStreamed);
}

void _stdcall VideoPatch::patch(int x) {
    if (state != VideoState::Playing) {
        return;
    }
    if (d) {
        v.render(d);
    }
    else if (auto device = gameDevice()) {
        v.renderStreamed(device, streamedWidth, streamedHeight);
    }
}
