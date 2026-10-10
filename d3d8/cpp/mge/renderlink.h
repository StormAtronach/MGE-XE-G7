#pragma once

#include "mge_render_link.h"

struct LightState;

// The host side of the render link. A client (the nirender plugin) reports the scene and
// the object of the D3D8 calls that follow, through MGE_RenderLinkConnect.
//
// At this stage MGE XE only compares: at each place where it infers a fact from the D3D8
// calls, it counts if the packet of the client says the same. Without a client every
// function here does nothing.
namespace RenderLink {
    // True after a client connected.
    bool isConnected();

    // The scene that is open, or null.
    const MgeLinkSceneV1* scene();

    // The packet that the current D3D8 draw belongs to, or null.
    const MgeLinkDrawV1* currentDrawPacket();

    // MGE_LINK_CLASS_* of the current draw. MGE_LINK_CLASS_UNKNOWN without a packet.
    uint32_t drawClass();

    // The current draw is a part of the sky.
    bool drawIsSky();

    // The current draw is a second or later D3D8 draw of a landscape object.
    bool drawIsLandSplat();

    // Counts one comparison of a fact. Does nothing without a packet for the current draw.
    void compare(uint32_t fact, bool inferred, bool fromPacket);

    // BeginScene on the back buffer. `sceneIndex` is the scene count of the proxy for this
    // scene, when `mainView` is true.
    void observeBeginScene(bool mainView, int sceneIndex);

    // A DrawIndexedPrimitive of the world that the proxy inspects.
    void observeWorldDraw(bool waterMaterial, const LightState& lights);
}
