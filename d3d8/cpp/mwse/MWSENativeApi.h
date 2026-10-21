#pragma once

//
// The native ABI of MWSE: the table that MWSE.dll gives to native modules (DLLs) of the game.
//
// MWSE is the module that changes the code of Morrowind.exe. A native module does not write
// engine bytes itself. It asks MWSE through this table:
//
// - An event tells the listeners that the engine reached a point. Many modules can listen.
// - An override replaces one engine function. Only one module can own it.
// - A request asks MWSE to change an engine behaviour, which MWSE then owns.
// - A query reads a fact through MWSE.
//
// Rules:
// - The game is 32-bit. Every function is __cdecl.
// - Include <stdint.h> before this header.
// - Each packet starts with its size. A new version adds members only at the end. A reader
//   checks structSize before it reads a member that a later version added.
// - An id is never used again for another meaning. An id that this MWSE does not have answers
//   MWSE_NATIVE_UNKNOWN_ID.
// - MWSE writes the engine bytes of an event, an override or a request at its first use, not
//   at its start. A game without native modules runs as without this ABI.
// - subscribe, unsubscribe, claim, release and request run on the main thread of the game.
//   Each event names the thread it fires on.
// - A pointer in a packet is valid only until the listener returns.
//
// To get the table: GetProcAddress(GetModuleHandleA("MWSE.dll"), "MWSE_GetNativeApi"), then
// call it with the version the module needs. MWSE.dll is in the process before any module of
// the game starts (MGE XE loads it).
//

#define MWSE_NATIVE_API_VERSION 1u

typedef uint32_t MwseNativeHandle;

typedef enum MwseNativeStatus {
	MWSE_NATIVE_OK = 0,
	// This MWSE does not have the id.
	MWSE_NATIVE_UNKNOWN_ID = 1,
	// An override has an owner. MWSE.log names it.
	MWSE_NATIVE_TAKEN = 2,
	// The engine code at the place is not the code MWSE expects. MWSE.log names the place.
	MWSE_NATIVE_BYTES_CHANGED = 3,
	// The engine passed the latest point to subscribe, claim or request.
	MWSE_NATIVE_TOO_LATE = 4,
	// The call is not on the main thread of the game.
	MWSE_NATIVE_WRONG_THREAD = 5,
	// A null pointer, or a packet with a size too small.
	MWSE_NATIVE_BAD_ARGUMENT = 6,
} MwseNativeStatus;

// A listener of an event. `packet` is the packet of the event.
typedef void (__cdecl* MwseNativeListener)(void* user, const void* packet);

//
// Events.
//

// The engine updated the transforms of the player's cameras from the "Camera" node of the
// first-person model (0x542E60 `PlayerAnimationController::updateCameraTransforms`), at each of
// its five callers and at MWSE's own calls. Main thread. It fires after the engine function
// returns. At the caller in `MobilePlayer::updateSceneGraph` it fires before the Lua event
// `cameraControl`. Latest subscribe: none.
#define MWSE_NATIVE_EVENT_CAMERA_TRANSFORMS_UPDATED 1u
typedef struct MwseNativeCameraTransformsUpdatedV1 {
	uint32_t structSize;
	// The `TES3::PlayerAnimationController`.
	void* playerAnimationController;
} MwseNativeCameraTransformsUpdatedV1;

// The engine configured the scale of its UI when it made its UI (the call of
// `WorldController::configUIScaling` in `WorldController::initUIAndAudio`). Main thread. It fires
// after MWSE applied the UI scale of a request (MWSE_NATIVE_REQUEST_UI_SCALE). Latest
// subscribe: before the game initializes (before the engine makes its renderer).
#define MWSE_NATIVE_EVENT_UI_CONFIGURED 2u
typedef struct MwseNativeUIConfiguredV1 {
	uint32_t structSize;
	// The `TES3::WorldController`.
	void* worldController;
	// The width of the window that the engine passed.
	int32_t windowWidth;
	// The size of the UI's view after the configuration.
	int32_t viewWidth;
	int32_t viewHeight;
} MwseNativeUIConfiguredV1;

//
// Overrides. The owner's function has the signature of the engine function. `engineFunction`
// of `claim` gives the engine function, to call or not.
//

// 0x4D2F40 `game_dynamicLightTest`: the test whether a point light reaches a node, at its six
// callers. The engine writes the radius into the three parts of the light's specular colour.
// Main thread. Latest claim: none. `isLand` and `highPriority` are a byte in a stack slot of
// 4 bytes: a module may take them as uint32_t and pass them on.
#define MWSE_NATIVE_OVERRIDE_DYNAMIC_LIGHT_TEST 1u
typedef void (__cdecl* MwseNativeDynamicLightTest)(void* light, void* node, int32_t radius, int32_t flags, uint8_t isLand, uint8_t highPriority);

//
// Requests.
//

// The scale of the UI. MWSE makes the UI's view smaller than the back buffer by the scale,
// configures the UI for it, sets the cursor bounds, and makes the pick ray of the renderer
// (`NiRenderer::MapWindowPointToBufferPoint`) divide by the view's size. MWSE applies the scale
// when the engine makes its UI, and at once when the UI exists. A new request changes the
// scale. Latest first request: before the game initializes.
#define MWSE_NATIVE_REQUEST_UI_SCALE 1u
typedef struct MwseNativeUIScaleRequestV1 {
	uint32_t structSize;
	// Larger than 0. 1 is the engine's own scale.
	float scale;
} MwseNativeUIScaleRequestV1;

//
// Queries.
//

// The scale of the UI and the size of its view.
#define MWSE_NATIVE_QUERY_UI_SCALE 1u
typedef struct MwseNativeUIScaleV1 {
	uint32_t structSize;
	// The scale of the last request, or 1.
	float scale;
	// The size of the UI's view now, or 0 before the engine made its UI.
	int32_t viewWidth;
	int32_t viewHeight;
	// 1 when the pick ray of the renderer divides by the view's size (a request came), else 0:
	// it divides by the size of the back buffer.
	uint32_t pickRayInViewSize;
} MwseNativeUIScaleV1;

//
// The table.
//

typedef struct MwseNativeApiV1 {
	uint32_t structSize;
	uint32_t version;
	// The version of MWSE, for the log of a module.
	const char* mwseVersion;

	// Adds a listener of an event. A listener with a lower priority runs later. `module` names
	// the module in MWSE.log.
	MwseNativeStatus (__cdecl* subscribe)(const char* module, uint32_t eventId, MwseNativeListener listener, void* user, int32_t priority, MwseNativeHandle* handle);
	// Removes a listener. The listener can still run once on another thread.
	void (__cdecl* unsubscribe)(MwseNativeHandle handle);

	// Makes the module the owner of an override. `function` replaces the engine function.
	MwseNativeStatus (__cdecl* claim)(const char* module, uint32_t overrideId, void* function, void** engineFunction, MwseNativeHandle* handle);
	// Gives an override back to the engine function.
	void (__cdecl* release)(MwseNativeHandle handle);

	// Makes a request. `packet` is the packet of the request.
	MwseNativeStatus (__cdecl* request)(const char* module, uint32_t requestId, const void* packet);
	// Reads a fact into `packet`. The module sets structSize first.
	MwseNativeStatus (__cdecl* query)(uint32_t queryId, void* packet);
} MwseNativeApiV1;

// The export of MWSE.dll. Returns null when this MWSE has no table of `version` or later.
typedef const MwseNativeApiV1* (__cdecl* MwseGetNativeApi)(uint32_t version);
