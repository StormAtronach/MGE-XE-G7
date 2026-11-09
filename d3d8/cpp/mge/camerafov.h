#pragma once

// The field of view of the cameras that draw the world.
//
// Each camera has a rule. A camera that follows gets MGE XE's horizontal field of view and
// the aspect of the world camera, at each Click. A camera that is its own keeps what the
// game or a mod gave it. The world, arm and shadow cameras follow; every other camera is
// its own.
//
// The write happens where the Click hands the camera to the renderer
// (NiDX8Renderer::SetCameraData): on Direct3D 9 in the hook of that slot
// (CameraRelative::installHooks), and on the render link in Deviceless::sceneBegin, which
// the plugin calls from its own hook of the same slot. Both run before the renderer reads
// the frustum.

namespace NI {
struct Camera;
}

namespace CameraFov {

// Whether the camera follows the field of view of MGE XE. Reads the world controller and the
// shadow manager at each call, so a camera the game makes again is known at once.
bool follows(const NI::Camera* camera);

// The start of the Click of this camera: notes whether it follows, writes its frustum when
// it does and MGE XE manages the field of view (Configuration.ScreenFOV), and builds the
// culling planes again when the write changed the frustum. Call it before anything reads the
// camera.
void atClick(NI::Camera* camera);

// The same for the camera whose world translation the renderer received. NiCamera::Click passes
// &camera->worldTransform.translation, and the renderer slot has no other caller in the
// engine. The pointer is only compared until a camera of the game has matched it.
void atClickOfLocation(const float* worldLocation);

// Whether the camera of the last atClick follows. True until the first call.
bool lastClickFollows();

// The write for each camera that follows, outside a Click: at the start of a frame, so that
// the code that reads the world frustum outside a Click sees MGE XE's field of view
// (MWBridge::SetFOV).
void applyToFollowers(float screenFOV);

}  // namespace CameraFov
