#pragma once

struct IDirect3DDevice9;
struct _D3DGAMMARAMP;

// The gamma ramp of the device (IDirect3DDevice8::SetGammaRamp). The engine sets it from its
// gamma setting (NiDX8Application::SetGamma 0x429C60: one ramp for red, green and blue).
// The ramp goes through a look-up table onto the back buffer just before Present, and
// changes no other picture of the display. A screenshot is taken before it (EndScene), as a
// Direct3D 8 device ramp is not in the back buffer either. An identity ramp (gamma 1) draws
// nothing. The nirender helper does the same on wgpu (host/src/gamma.rs).
class GammaRamp {
public:
    // Keeps the ramp for the passes that follow.
    static void set(const _D3DGAMMARAMP* ramp);
    // The ramp: the last one set, else the identity.
    static void get(_D3DGAMMARAMP* ramp);
    // Draws the back buffer through the look-up table. Call it after the last draw of the
    // frame and before Present.
    static void apply(IDirect3DDevice9* device);
    static void release();
};
