#pragma once

struct IDirect3DDevice9;

// A debug log of the state of the Direct3D 9 device at each draw of the game, for a
// comparison of a draw with a capture of another renderer. The switch MGE_XE_DRAW_STATE_LOG=<n>
// turns it on (off by default): a window of n frames (1 to 600) starts at a press of the
// Pause key while the game has the focus, or when the file mge-draw-state.trigger is in the
// folder of Morrowind.exe (the log deletes the file; after a failed delete, the file starts no
// more windows). Each draw of the window writes one line to mgeXE.log; a line at each end of
// the window counts its draws. Without the switch, each call returns at once.
class DrawStateLog {
public:
    // At each Present: starts a window on a trigger, and ends it after its frames.
    static void onPresent();
    // A window is open: the draws of this frame are logged.
    static bool logging();
    // The line of one draw: the render states, the lights that are on, the material, the
    // stage states of stages 0 to 3, the vertex format and the primitives. `scene` is the
    // scene of the frame (from 0).
    static void onDraw(IDirect3DDevice9* device, int scene, unsigned primitiveType, unsigned primitiveCount);
};
