
#include "statusoverlay.h"
#include "configuration.h"
#include "mged3d8device.h"
#include "proxydx/d3d9header.h"

#include <cstdio>
#include <cstring>
#include <vector>



namespace StatusOverlay {
    char statusText[512];
    char fpsText[16];
    DWORD statusTimeout;
    int currentPriority;
    D3DCOLOR statusColour;

    ID3DXFont* font;
    ID3DXSprite* sprite;
    RECT statusRect, shadowRect, fpsRect;
}

const D3DCOLOR colWhite = 0xffffffff, colRed = 0xffff2222, colShadow = 0xc0000000;
const char* fontFace = "Lucida Console";
const int fontHeight = 14;

bool StatusOverlay::init(IDirect3DDevice9* device) {
    D3DXCreateFont(device, fontHeight, 0, 400, 1, false, ANSI_CHARSET, OUT_DEFAULT_PRECIS,
                   DEFAULT_QUALITY, DEFAULT_PITCH | FF_DONTCARE, fontFace, &font);
    D3DXCreateSprite(device, &sprite);

    statusRect = {8, 10, 635, 25};
    shadowRect = {statusRect.left+1, statusRect.top+1, statusRect.right+1, statusRect.bottom+1};
    fpsRect = {8, 35, 160, 50};
    return true;
}

void StatusOverlay::release() {
    sprite->Release();
    font->Release();
}

void StatusOverlay::show(IDirect3DDevice9* device) {
    if (!(Configuration.MGEFlags & DISPLAY_MESSAGES)) {
        return;
    }

    if ((Configuration.MGEFlags & FPS_COUNTER) || statusTimeout) {
        sprite->Begin(D3DXSPRITE_ALPHABLEND);
        if (Configuration.MGEFlags & FPS_COUNTER) {
            font->DrawTextA(sprite, fpsText, -1, &fpsRect, DT_NOCLIP, colWhite);
        }

        if (statusText[0] != 0) {
            if (GetTickCount() < statusTimeout) {
                font->DrawTextA(sprite, statusText, -1, &shadowRect, DT_NOCLIP, colShadow);
                font->DrawTextA(sprite, statusText, -1, &statusRect, DT_NOCLIP, statusColour);
            } else {
                statusTimeout = 0;
                currentPriority = 0;
            }
        }
        sprite->End();
    }
}

// The wgpu renderer mode: the helper draws the text. A new text goes to it with the next frame.
static bool statusUnsent = false;

void StatusOverlay::setStatus(const char* s, int priority) {
    // A text that has gone away does not hold back one of a lower priority. `show` does the
    // same, but the wgpu renderer mode has no `show`.
    if (statusTimeout && GetTickCount() >= statusTimeout) {
        statusTimeout = 0;
        currentPriority = 0;
    }
    if (priority >= currentPriority) {
        std::snprintf(statusText, sizeof(statusText), "%s", s);
        statusTimeout = GetTickCount() + priority * Configuration.StatusTimeout;
        currentPriority = priority;
        statusColour = currentPriority >= Priority::PriorityWarning ? colRed : colWhite;
        statusUnsent = true;
    }
}

void StatusOverlay::setFPS(float fps) {
    std::snprintf(fpsText, sizeof(fpsText), "%4.0f", fps);
}

void StatusOverlay::showLastStatus() {
    statusTimeout = GetTickCount() + Configuration.StatusTimeout;
    statusUnsent = true;
}

void sendToRenderer(UINT id, const void* data, UINT length);

void StatusOverlay::sendToRenderer() {
    if (!statusUnsent) {
        return;
    }
    statusUnsent = false;
    const DWORD now = GetTickCount();
    if (statusText[0] == 0 || now >= statusTimeout) {
        return;
    }
    // The record 9: the version 1, 1 for a warning or an error, the time to show the text in
    // ms, the text. STATUS_ID in mgeHost64/src/renderer.rs.
    static const UINT StatusId = 9;
    const size_t length = std::strlen(statusText) + 1;
    std::vector<unsigned char> record(12 + length);
    const UINT head[3] = { 1, currentPriority >= Priority::PriorityWarning ? 1u : 0u, statusTimeout - now };
    std::memcpy(record.data(), head, sizeof(head));
    std::memcpy(record.data() + 12, statusText, length);
    ::sendToRenderer(StatusId, record.data(), static_cast<UINT>(record.size()));
}
