#pragma once

#include "mge_render_link.h"

class ProxyDevice;
struct LightState;
struct RenderedState;
struct FragmentState;

// The host side of the render link. A client (the nirender plugin) reports the scene and
// the object of the D3D8 calls that follow, through MGE_RenderLinkConnect.
//
// At each place where MGE XE infers a fact from the D3D8 calls, it counts if the packet of
// the client says the same. The client can also tell MGE XE to take a fact from the packets
// (MgeRenderLinkHostV1::useFacts). Without a client every function here does nothing, and
// `resolve` returns the inference.
namespace RenderLink {
    // The device that makes the textures of the link. Null when the device goes away.
    void setDevice(ProxyDevice* device);

    // The D3D8 handlers of the proxy call these, so the link knows the state of the device.
    void noteRenderState(uint32_t state, uint32_t value);
    void noteStageState(uint32_t stage, uint32_t state, uint32_t value);
    void noteTexture(uint32_t stage, const void* texture);
    void noteStreamSource(const void* buffer, uint32_t stride);
    void noteIndices(const void* buffer, uint32_t baseVertexIndex);
    void noteVertexFormat(uint32_t format);
    // `material` is a D3DMATERIAL8.
    void noteMaterial(const void* material);
    // `worldIsRelative`: for a world matrix, the matrix is relative to the camera.
    void noteTransform(uint32_t state, const void* matrix, bool worldIsRelative);
    void noteLight(uint32_t index, const void* light);
    void noteLightEnable(uint32_t index, bool on);

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

    // The current draw is a shape of a moon: its face, or the shape that hides the dark part.
    // The moon mark of the proxy is in the material of the two.
    bool drawIsMoon();

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

    // BeginScene of a world view on the back buffer. `inferred` is the scene count of the
    // proxy for this scene: 0 for the first world scene of the frame. Returns the count to
    // use. From the packet, the world scene is 0 and each other scene is 1 or more.
    int resolveSceneCount(int inferred);

    // A DrawIndexedPrimitive of the world that the proxy inspects.
    void observeWorldDraw();

    // The same draw, after the proxy put the arguments of the call into `rs`. Compares the
    // state packet of the client with the state that the proxy has from the D3D8 calls.
    void observeDrawState(const RenderedState& rs, const FragmentState& frs, const LightState& lights);

    // Call after observeDrawState. When the client asked for it (the fact
    // MGE_LINK_FACT_STATE_DRAW in useFacts) and the draw has a state packet, this puts the
    // state of the packet into the three structs. Returns true when it did.
    // Not from the packet: the view transform, the place of a light in view space, the
    // radius of a light and the matrix palette of indexed skinning.
    bool applyDrawState(RenderedState& rs, FragmentState& frs, LightState& lights);

    // The game attaches a point light. `recordRadius` is the radius that the game has for
    // the light. `attachRadius` is the radius that the shim of the light fade gives back,
    // and that the game then keeps in the light object.
    void noteLightRadius(const void* light, int recordRadius, int attachRadius);

    // SetLight of a point light. `inferred` is the radius that the proxy solved from the
    // attenuation. Returns the radius to use.
    float resolveLightRadius(uint32_t deviceIndex, float inferred);

    // True when the proxy takes the water plane from the packets. Then the mark in the
    // material of the water is not necessary.
    bool usesWaterPlaneFact();

    // The proxy put its mark in the material of the water. `original` is the value that was
    // there. The host puts it back while the water plane comes from the packets.
    void noteWaterMark(float original);

    // The same for the mark in the materials of the two moons.
    void noteMoonMark(const float original[2]);
}
