#pragma once

// The render link: the contract between nirender.dll (the client) and the d3d8.dll of
// MGE XE (the host).
//
// The client knows what the game draws. It tells the host the scene and the object of the
// D3D8 calls that follow. The host does not have to infer these facts from the calls.
//
// This file is the same, byte for byte, in the two repos:
//   nirender-plugin/contract/mge_render_link.h   (the source)
//   MGE-XE/d3d8/cpp/mge/mge_render_link.h        (the copy)
// `tools/check_contract.py` of nirender-plugin compares them. `src/link.rs` has the same
// structs for Rust.
//
// Rules:
// - The two DLLs are 32-bit. A pointer and a `uint32_t` have the same size.
// - An address of a game object is a `uint32_t`. The host must not keep it after the scene.
// - Each struct starts with its size. A new version adds members at the end.
// - Every function is `__cdecl` and runs on the render thread of the game.
// - A pointer in a packet is valid only until the function returns.

#include <stdint.h>

#define MGE_RENDER_LINK_VERSION 1u

// Capability bits of the host.
// The host compares each fact of the packets with the fact that it infers, and counts.
#define MGE_RENDER_LINK_CAP_COMPARE 0x00000001u
// The host can use the facts of the packets as its source.
#define MGE_RENDER_LINK_CAP_USE_FACTS 0x00000002u
// The host takes a state packet for each D3D8 draw call (`drawState`).
#define MGE_RENDER_LINK_CAP_DRAW_STATE 0x00000004u

// Scene kinds. A scene is the time between BeginPaint and EndPaint of the renderer.
#define MGE_LINK_SCENE_OTHER 0u
// The world camera on the world scene graph: the sky, then the opaque world.
#define MGE_LINK_SCENE_WORLD 1u
// The world camera with a culled root: only the sorted queue.
#define MGE_LINK_SCENE_SORTED_ALPHA 2u
// The world camera on the water plane, with the sorted queue.
#define MGE_LINK_SCENE_WATER 3u
// The shadow camera: a pass of the stencil volumes.
#define MGE_LINK_SCENE_SHADOW_VOLUME 4u
// The shadow camera: the redraw of the shadow casters.
#define MGE_LINK_SCENE_SHADOW_CASTERS 5u
// The arm camera: first person and sun glare.
#define MGE_LINK_SCENE_FIRST_PERSON 6u
#define MGE_LINK_SCENE_UI 7u
#define MGE_LINK_SCENE_RACE_MENU_HEAD 8u
// The splash camera: load screens and the main menu background.
#define MGE_LINK_SCENE_SPLASH 9u
#define MGE_LINK_SCENE_CHARACTER_PREVIEW 10u
#define MGE_LINK_SCENE_LOCAL_MAP 11u
#define MGE_LINK_SCENE_WATER_REFLECTION 12u

// Draw kinds.
#define MGE_LINK_DRAW_SHAPE 1u
#define MGE_LINK_DRAW_STRIPS 2u
#define MGE_LINK_DRAW_POINTS 3u
#define MGE_LINK_DRAW_LINES 4u
#define MGE_LINK_DRAW_SCREEN_POLYGON 5u

// Classes of the object of a draw.
#define MGE_LINK_CLASS_UNKNOWN 0u
#define MGE_LINK_CLASS_SKY_DOME 1u
#define MGE_LINK_CLASS_SKY_NIGHT 2u
#define MGE_LINK_CLASS_SKY_CLOUDS 3u
#define MGE_LINK_CLASS_SKY_SUN 4u
#define MGE_LINK_CLASS_SKY_MOON 5u
#define MGE_LINK_CLASS_SKY_MOON_SHADOW 6u
// Below the sky root, but none of the known parts.
#define MGE_LINK_CLASS_SKY_OTHER 7u
#define MGE_LINK_CLASS_SUN_GLARE 8u
#define MGE_LINK_CLASS_WEATHER_PARTICLES 9u
#define MGE_LINK_CLASS_WATER_PLANE 10u
#define MGE_LINK_CLASS_LANDSCAPE 11u
#define MGE_LINK_CLASS_OBJECT 12u
#define MGE_LINK_CLASS_SHADOW_VOLUME 13u
#define MGE_LINK_CLASS_SHADOW_OVERLAY 14u
#define MGE_LINK_CLASS_FIRST_PERSON 15u
#define MGE_LINK_CLASS_UI 16u
// A ripple shape of the game. The ripple node is below the water plane node.
#define MGE_LINK_CLASS_WATER_RIPPLE 17u

// Flags of a state packet.
// The draw is a partition of a skinned mesh. `world` is not valid.
#define MGE_LINK_STATE_SKINNED 0x00000001u

// Flags of a draw.
// The draw comes from the sorted queue of the accumulator.
#define MGE_LINK_DRAW_FROM_SORTED_QUEUE 0x00000001u
#define MGE_LINK_DRAW_SKINNED 0x00000002u

// Light types, as NiLight::GetType gives them.
#define MGE_LINK_LIGHT_AMBIENT 0u
#define MGE_LINK_LIGHT_DIRECTIONAL 1u
#define MGE_LINK_LIGHT_POINT 2u
#define MGE_LINK_LIGHT_SPOT 3u

// Frame events. `a` and `b` are the arguments of `frameEvent`.
// ClearBuffer. a: flags (1 colour, 2 stencil, 4 depth). b: 0.
#define MGE_LINK_EVENT_CLEAR 1u
// SetRenderTarget. a: the NiRenderedTexture, or 0 for the back buffer. b: 0.
#define MGE_LINK_EVENT_SET_TARGET 2u
// SwapBuffers: the end of the frame. a: 0. b: 0.
#define MGE_LINK_EVENT_SWAP 3u

// Facts that the host can compare. They are the indexes of `MgeLinkCountersV1::facts`.
#define MGE_LINK_FACT_LIGHT_RADIUS 0u
#define MGE_LINK_FACT_SUN_LIGHT 1u
// The draw is a shape of a moon: its face or the shape that hides the dark part.
#define MGE_LINK_FACT_MOON_SHADOW 2u
#define MGE_LINK_FACT_WATER_PLANE 3u
#define MGE_LINK_FACT_UI_SCENE 4u
#define MGE_LINK_FACT_SCENE_KIND 5u
#define MGE_LINK_FACT_SKY_DRAW 6u
#define MGE_LINK_FACT_LAND_SPLAT 7u
// A view transform that is not the first one of its scene is a UI view. The packet side is
// the kind of the scene. The host compares this fact and does not use it.
#define MGE_LINK_FACT_LATER_VIEW 8u
// The groups of the state packet. For these, "both yes" is the number of D3D8 draws where
// the group of the packet was equal to the state that the host has from the D3D8 calls,
// "host only" the number where it was different, and "both no" the number of draws without
// a state packet.
#define MGE_LINK_FACT_STATE_DRAW 9u
#define MGE_LINK_FACT_STATE_BUFFERS 10u
#define MGE_LINK_FACT_STATE_TRANSFORM 11u
#define MGE_LINK_FACT_COUNT 16u

typedef struct MgeLinkSceneV1 {
    uint32_t structSize;
    // MGE_LINK_SCENE_*
    uint32_t kind;
    // The NiCamera of BeginPaint.
    uint32_t camera;
    // The scene graph root of the camera.
    uint32_t sceneRoot;
    // The NiRenderedTexture that is the target, or 0 for the back buffer.
    uint32_t renderTarget;
    uint32_t reserved;
} MgeLinkSceneV1;

typedef struct MgeLinkLightV1 {
    // The NiLight.
    uint32_t light;
    // The index of the light on the device.
    uint32_t deviceIndex;
    // MGE_LINK_LIGHT_*
    uint32_t type;
    // The radius of the light record, for a point light. 0 when the object has no radius.
    float radius;
} MgeLinkLightV1;

typedef struct MgeLinkDrawV1 {
    uint32_t structSize;
    // MGE_LINK_DRAW_*
    uint32_t kind;
    // MGE_LINK_CLASS_*
    uint32_t objectClass;
    // MGE_LINK_DRAW_FROM_SORTED_QUEUE, MGE_LINK_DRAW_SKINNED
    uint32_t flags;
    // The NiGeometry, or 0 when the draw has none (a screen polygon).
    uint32_t geometry;
    // The NiGeometryData.
    uint32_t geometryData;
    // The NiSkinInstance, or 0.
    uint32_t skinInstance;
    // The NiPropertyState and the NiDynamicEffectState of the renderer at the draw.
    uint32_t propertyState;
    uint32_t effectState;
    // The shininess of the material property. Mods put marks in this value.
    float materialShininess;
    // The enabled lights of the effect state.
    uint32_t lightCount;
    const MgeLinkLightV1* lights;
} MgeLinkDrawV1;

// The comparisons of one fact. The fact is a yes or no answer for a draw or a scene.
// The state of one D3D8 draw call. The client sends it before the call. A handle is the
// value that identifies the resource for the host. In this version it is the D3D8 interface
// pointer that the game holds.
typedef struct MgeLinkDrawStateV1 {
    uint32_t structSize;
    // MGE_LINK_STATE_*
    uint32_t flags;
    // The arguments of the draw call.
    uint32_t primitiveType;
    uint32_t baseVertexIndex;
    uint32_t minIndex;
    uint32_t vertexCount;
    uint32_t startIndex;
    uint32_t primitiveCount;
    // The buffers.
    uint32_t vertexBuffer;
    uint32_t vertexStride;
    uint32_t vertexFormat;
    uint32_t indexBuffer;
    // The world matrix, as the renderer of the game has it.
    float world[16];
} MgeLinkDrawStateV1;

typedef struct MgeLinkFactCountV1 {
    // The packet and the inference of the host said yes.
    uint32_t bothYes;
    // The two said no.
    uint32_t bothNo;
    // Only the inference of the host said yes.
    uint32_t hostOnly;
    // Only the packet said yes.
    uint32_t packetOnly;
} MgeLinkFactCountV1;

typedef struct MgeLinkCountersV1 {
    uint32_t structSize;
    uint32_t scenes;
    uint32_t draws;
    // Draws of the host that had no packet.
    uint32_t drawsWithoutPacket;
    MgeLinkFactCountV1 facts[MGE_LINK_FACT_COUNT];
} MgeLinkCountersV1;

typedef struct MgeRenderLinkHostV1 {
    uint32_t structSize;
    // MGE_RENDER_LINK_VERSION of the host.
    uint32_t version;
    // MGE_RENDER_LINK_CAP_*
    uint32_t capabilities;
    uint32_t reserved;
    // A scene starts. The call comes before the view transform of the scene goes to the
    // device. The D3D8 calls that follow belong to this scene.
    void (__cdecl* sceneBegin)(const MgeLinkSceneV1* scene);
    // EndPaint starts.
    void (__cdecl* sceneEnd)(void);
    // MGE_LINK_EVENT_*. The call comes before the D3D8 calls of the event.
    void (__cdecl* frameEvent)(uint32_t kind, uint32_t a, uint32_t b);
    // The D3D8 calls that follow, until the next `draw` or `sceneEnd`, belong to this draw.
    void (__cdecl* draw)(const MgeLinkDrawV1* draw);
    // Copies the counters. `reset` not 0 sets them to 0 after the copy.
    void (__cdecl* readCounters)(MgeLinkCountersV1* out, uint32_t reset);
    // Tells the host which facts it must take from the packets and not from its inference.
    // Bit n of `factMask` is the fact MGE_LINK_FACT_ n. Returns the mask that the host uses:
    // a host does not use a fact that it cannot take from a packet.
    // Only a host with MGE_RENDER_LINK_CAP_USE_FACTS has this member.
    uint32_t (__cdecl* useFacts)(uint32_t factMask);
    // The D3D8 draw call that follows has this state.
    // Only a host with MGE_RENDER_LINK_CAP_DRAW_STATE has this member.
    void (__cdecl* drawState)(const MgeLinkDrawStateV1* state);
} MgeRenderLinkHostV1;

#ifdef __cplusplus
static_assert(sizeof(void*) == 4, "the render link is for 32-bit code");
static_assert(sizeof(MgeLinkSceneV1) == 24, "MgeLinkSceneV1 size");
static_assert(sizeof(MgeLinkLightV1) == 16, "MgeLinkLightV1 size");
static_assert(sizeof(MgeLinkDrawV1) == 48, "MgeLinkDrawV1 size");
static_assert(sizeof(MgeLinkDrawStateV1) == 112, "MgeLinkDrawStateV1 size");
static_assert(sizeof(MgeLinkFactCountV1) == 16, "MgeLinkFactCountV1 size");
static_assert(sizeof(MgeLinkCountersV1) == 272, "MgeLinkCountersV1 size");
static_assert(sizeof(MgeRenderLinkHostV1) == 44, "MgeRenderLinkHostV1 size");

extern "C" {
#endif

// The one export of the host. The client calls it one time.
// Returns the table of the host, or null when the host cannot work with `clientVersion`.
// The table stays valid until the process ends.
const MgeRenderLinkHostV1* __cdecl MGE_RenderLinkConnect(uint32_t clientVersion);

#ifdef __cplusplus
}
#endif
