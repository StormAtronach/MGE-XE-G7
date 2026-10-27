#pragma once

#include "proxydx/d3d8interface.h"



struct Bink;

// The menu background video of MGE XE, Data Files\video\menu_background.bik: it takes the
// place of the menu background camera's Click. On Direct3D 9 a frame is copied into the back
// buffer. On the wgpu path the frame is drawn as the game draws its films: a texture of the
// game's device (the nirender plugin's) and one quad in screen coordinates, which the helper
// draws (the plugin's DRAW_UP record).
class VideoBackground {
    Bink* video;
    IDirect3DSurface9* surface;
    IDirect3DTexture8* texture;

public:
    VideoBackground() : video(0), surface(0), texture(0) {}

    bool init();
    bool begin(IDirect3DDevice9* device);
    bool beginStreamed(IDirect3DDevice8* device);
    void render(IDirect3DDevice9* device);
    void renderStreamed(IDirect3DDevice8* device, float width, float height);
    void end();
};

class VideoPatch {
    static IDirect3DDevice9* d;
    static VideoBackground v;
    static int state;
    // The wgpu path: the size of the back buffer, from the last Present of the stream.
    static float streamedWidth, streamedHeight;

    static void advance(bool (*begin)());
    static bool beginProxy();
    static bool beginStreamed();

public:
    static void start(IDirect3DDevice9* device);
    static void monitor(IDirect3DDevice9* device);
    // The wgpu path: called at each Present of the stream with the size of the back buffer.
    static void monitorStreamed(unsigned width, unsigned height);
    static void _stdcall patch(int x);
};
