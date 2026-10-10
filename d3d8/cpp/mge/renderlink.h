#pragma once

#include "mge_render_link.h"

struct LightState;

// The host side of the render link. A client (the nirender plugin) reports the scene and
// the object of the D3D8 calls that follow, through MGE_RenderLinkConnect.
//
// At each place where MGE XE infers a fact from the D3D8 calls, it counts if the packet of
// the client says the same. The client can also tell MGE XE to take a fact from the packets
// (MgeRenderLinkHostV1::useFacts). Without a client every function here does nothing, and
// `resolve` returns the inference.
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

    // The current draw is the water plane of the game.
    bool drawIsWaterPlane();

    // The light with this device index is a directional light of the current draw.
    bool lightIsDirectional(uint32_t deviceIndex);

    // Counts one comparison of a fact. Does nothing without a packet for the current draw.
    void compare(uint32_t fact, bool inferred, bool fromPacket);

    // Counts one comparison of a fact of the current draw and returns the answer to use:
    // `fromPacket` when the client asked for this fact and the draw has a packet, and
    // `inferred` if not.
    bool resolve(uint32_t fact, bool inferred, bool fromPacket);

    // The view transform of a scene goes to the device. `inferredMainView` is what the view
    // matrix says. Returns the answer to use for "this is a world view". Only the first view
    // of a scene can come from the packet: a later view in a scene (a screen polygon) is not
    // a fact of the scene.
    bool resolveMainView(bool inferredMainView);

    // BeginScene on the back buffer. `sceneIndex` is the scene count of the proxy for this
    // scene, when `mainView` is true.
    void observeBeginScene(bool mainView, int sceneIndex);

    // A DrawIndexedPrimitive of the world that the proxy inspects.
    void observeWorldDraw(const LightState& lights);
}
