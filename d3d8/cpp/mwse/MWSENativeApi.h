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

// The engine resolved the internal ids of the scripts at the start of a game: a new game, a
// load, the start in a cell (0x40FC40 `WorldController::resolveScriptInternalIDs` at its four
// callers, the quick start of 0x5FE7B0 among them). Main thread. It fires after the engine
// function returns. Latest subscribe: before the first game starts.
#define MWSE_NATIVE_EVENT_GAME_IDS_RESOLVED 3u
typedef struct MwseNativeGameIdsResolvedV1 {
	uint32_t structSize;
	// The `TES3::WorldController`.
	void* worldController;
} MwseNativeGameIdsResolvedV1;

// The engine drew the queue of an accumulator: 0x6CF0E0 `NiClusterAccumulator::FinishAccumulating`
// (the slot 0x30 of the virtual tables of NiAlphaAccumulator 0x7468D0 and NiClusterAccumulator
// 0x74FB4C) draws the queued objects in the order of its sort. `NiCamera::Click` calls it at the end
// of a Click (0x6CC895) unless its argument keeps the queue open. Main thread. It fires after the
// engine function returns. Latest subscribe: none.
#define MWSE_NATIVE_EVENT_ACCUMULATOR_FINISHED 4u
typedef struct MwseNativeAccumulatorFinishedV1 {
	uint32_t structSize;
	// The `NiAccumulator`.
	void* accumulator;
	// The camera of the accumulator before the call (the engine function clears it), or null.
	void* camera;
} MwseNativeAccumulatorFinishedV1;

// The player changed cell through the engine's cell change: 0x45CEF0 `game_cellChange` (a
// door, a travel, `coc` and the other moves of the console, Recall and the interventions, a
// script's PositionCell, the prison, the cell of a loaded game) at its thirteen callers, and
// 0x45C9B0 `game_cellChangeWithCompanion` (a door or a travel with companions) at its two, and
// at MWSE's own calls (`tes3.positionCell`). It fires after the engine function returns: the
// cells of the new place are loaded and the player is in them, and the engine has drawn no
// frame there. A walk across the border of an exterior cell does not fire it. Main thread.
// Latest subscribe: none.
#define MWSE_NATIVE_EVENT_PLAYER_CELL_CHANGED 5u
typedef struct MwseNativePlayerCellChangedV1 {
	uint32_t structSize;
	// The `TES3::Cell` of the player before the change, or null.
	void* previousCell;
	// The `TES3::Cell` of the player after the change.
	void* cell;
	// 1 when the load of a saved game made the change (0x4C4B4F, 0x4C51AE); else 0.
	uint32_t gameLoad;
} MwseNativePlayerCellChangedV1;

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

// The engine's thiscall functions below are declared as __fastcall with an unused second
// parameter: the same calling convention in C. A module may give a thiscall function.

// 0x6EB480 `NiAVObject::CullShow` of a camera's root, at its call in `NiCamera::Click`
// (0x6CC874) only: the walk of the scene of every Click. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_CAMERA_ROOT_CULL_SHOW 2u
typedef void (__fastcall* MwseNativeCameraRootCullShow)(void* root, void* unused, void* camera);

// 0x42F6F0 `WorldControllerRenderTarget::renderMap`: the read of a tile of the local map from
// its rendered texture (its caller: `drawMap`). With `skip` set the engine does nothing.
// Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_MAP_TILE_READ 3u
typedef void (__fastcall* MwseNativeMapTileRead)(void* renderTarget, void* unused, uint8_t skip);

// 0x485490 `DataHandler::purgeTextures`, at its eight callers. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_TEXTURE_PURGE 4u
typedef void (__fastcall* MwseNativeTexturePurge)(void* dataHandler, void* unused);

// 0x4C81C0 `RecordsHandler::mapDrawCell` at its call in `Cell::sub_4E3210` (0x4E32FE) only: the
// paint of a cell's tile from the cache on the world map. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_MAP_CELL_PAINT 5u
typedef void (__fastcall* MwseNativeMapCellPaint)(void* recordsHandler, void* unused, void* cell);

// 0x4B4F80 `TES3File_static::createSaveScreenshot` at its call in `TES3File::writeSaveHeader`
// (0x4B586D) only: the picture at the start of a save. Returns its pixel data. Main thread.
// Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_SAVE_PICTURE 6u
typedef void* (__cdecl* MwseNativeSavePicture)(const char* fileName);

// 0x42F3F0 `WorldControllerRenderTarget::lockRenderTarget`, at its three callers and at MWSE's
// own calls (the fog cache). `texture` is a counted pointer that the function lets go of, or
// null for the target's own texture. Returns a D3DLOCKED_RECT, or null. Main thread. Latest
// claim: none.
#define MWSE_NATIVE_OVERRIDE_RENDER_TARGET_LOCK 7u
typedef void* (__fastcall* MwseNativeRenderTargetLock)(void* renderTarget, void* unused, void* texture);

// 0x42F620 `WorldControllerRenderTarget::unlockRenderTarget`, at its three callers and at
// MWSE's own calls. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_RENDER_TARGET_UNLOCK 8u
typedef void (__fastcall* MwseNativeRenderTargetUnlock)(void* renderTarget, void* unused, void* lockedRect, int32_t copyBack);

// 0x42FC60 `WorldControllerRenderTarget::renderFogOfWar`, at its one caller: the fog of a map
// texture made again from its 16 x 16 bitmap. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_FOG_RENDER 9u
typedef void (__fastcall* MwseNativeFogRender)(void* renderTarget, void* unused, void* texture, void* bitmap, uint8_t smallBlobs);

// 0x42FA50 `WorldControllerRenderTarget::updateFogOfWarVertexBuffer`, at its seven callers: one
// blob of the fog drawn into a map texture. `texture` is a counted pointer that the function
// lets go of; x, y and size in sixteenths of the texture. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_FOG_BLOB 10u
typedef void (__fastcall* MwseNativeFogBlob)(void* renderTarget, void* unused, void* texture, int32_t x, int32_t y, int32_t size);

// The radius out to which the engine attaches a point light to the nodes near it: the radius
// that `game_updateDynamicLightingForPointLight` (0x4D2C20, four callers) and
// `game_updateLightHelper2` (0x4D32F0, two callers) pass on to the dynamic light test. The owner
// returns the radius to use; the engine function returns `radius`. Main thread. Latest claim:
// none (lights attached before keep their radius until the engine attaches them again).
#define MWSE_NATIVE_OVERRIDE_LIGHT_ATTACH_RADIUS 11u
typedef int32_t (__cdecl* MwseNativeLightAttachRadius)(const void* light, int32_t radius);

// The clock of the frame timer and of the music: the engine's `timeGetTime` at its four uses
// (0x403B50 the music, 0x4535FB, 0x453613, 0x453636 the frame timer). The owner returns
// milliseconds. The engine function is `timeGetTime`. Main thread. Latest claim: none.
#define MWSE_NATIVE_OVERRIDE_FRAME_TIMER 12u
typedef uint32_t (__stdcall* MwseNativeFrameTimer)(void);

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

// The most local lights of a node (`NiNode::PushLocalEffects`, 0x6C8FF2: the engine's 7). Later
// requests replace the limit. Latest request: none (the next update of a node uses it).
#define MWSE_NATIVE_REQUEST_LIGHT_LIMIT 2u
typedef struct MwseNativeLightLimitRequestV1 {
	uint32_t structSize;
	// 1 to 127.
	uint32_t limit;
} MwseNativeLightLimitRequestV1;

// The ripples of the engine's water (`WaterController::createRipple`, 0x51C2D4): on or off.
// Latest request: none.
#define MWSE_NATIVE_REQUEST_RIPPLES 3u
typedef struct MwseNativeRipplesRequestV1 {
	uint32_t structSize;
	// 1: the engine makes ripples (its own behaviour). 0: no ripples.
	uint32_t enabled;
} MwseNativeRipplesRequestV1;

// The two movies at the start of the game (the Bethesda logo, 0x418EF0, and the intro of the main
// menu, 0x5FC8F7). Latest request: before the game initializes (the logo plays then).
#define MWSE_NATIVE_REQUEST_INTRO_MOVIES 4u
typedef struct MwseNativeIntroMoviesRequestV1 {
	uint32_t structSize;
	// 1: the engine plays them (its own behaviour). 0: they are skipped.
	uint32_t play;
} MwseNativeIntroMoviesRequestV1;

// A fix of the engine: `sg_stopParticlesAndSetEmissiveMaterialToBlack` (0x4D2789) jumps over the
// code that changes the emissive colour of the material of a light's particles. Latest request:
// none.
#define MWSE_NATIVE_REQUEST_LIGHT_PARTICLES_FIX 5u
typedef struct MwseNativeLightParticlesFixRequestV1 {
	uint32_t structSize;
} MwseNativeLightParticlesFixRequestV1;

// The engine's sun glare (`WeatherController::sunGlareTests`, the jz at 0x4404FB): on or off. Off
// takes the occluded branch each frame, so the glare fades out and stays culled; the occlusion test
// and its result stay the same. Latest request: none.
#define MWSE_NATIVE_REQUEST_SUN_GLARE 6u
typedef struct MwseNativeSunGlareRequestV1 {
	uint32_t structSize;
	// 1: the engine shows its sun glare (its own behaviour). 0: no sun glare.
	uint32_t enabled;
} MwseNativeSunGlareRequestV1;

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
