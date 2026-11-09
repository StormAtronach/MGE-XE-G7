#include "camerafov.h"

#include <cmath>
#include <cstddef>
#include <cstring>

#include "configuration.h"
#include "mwbridge.h"
#include "support/log.h"
#include "tes3/nitypes.h"
#include "tes3/tes3addresses.h"
#include "tes3/tes3types.h"

namespace {

constexpr float PI = 3.14159265358979f;

// Within this distance of the left edge the frustum already has the field of view.
constexpr float FOV_TOLERANCE = 0.001f;

// The plane rebuilds that log a line even when the camera did not move. A rebuild that
// moved the camera always logs.
constexpr int LOGGED_REBUILDS = 32;

constexpr int FOLLOWER_COUNT = 4;
using Followers = NI::Camera*[FOLLOWER_COUNT];

const auto TES3_NI_Camera_updateWorldData =
    reinterpret_cast<void(__thiscall*)(NI::Camera*)>(TES3::Address::NI_Camera_updateWorldData);

// What NiCamera::UpdateWorldData can write besides the planes: the world transform it
// derives from the parent, and the axes it copies from the rotation.
struct CameraPose {
    NI::Transform world;
    NI::Point3 direction;
    NI::Point3 up;
    NI::Point3 right;
};

bool clickFollows = true;
int rebuildCount = 0;
float cachedDegrees = 0.0f;
float cachedTan = 0.0f;

// The cameras that follow: the world camera, the first person arm camera (which MGE
// historically called eSkyFOV) and the shadow camera, which draws the actors that cast
// shadows (the shadow manager keeps a pointer to it too). They are read at each call: the
// game makes its cameras again at a load. False before the game has loaded its world
// controller.
bool collectFollowers(Followers& followers) {
    if (!MWBridge::get()->IsLoaded()) {
        return false;
    }
    const TES3::WorldController* worldController = TES3::WorldController::get();
    if (!worldController) {
        return false;
    }
    const TES3::ShadowManager* shadowManager = worldController->shadowManager;
    followers[0] = worldController->worldCamera.cameraData.camera;
    followers[1] = worldController->armCamera.cameraData.camera;
    followers[2] = worldController->shadowCamera.cameraData.camera;
    followers[3] = shadowManager ? shadowManager->sgShadowCamera : nullptr;
    return true;
}

NI::Camera* cameraFromLocation(const float* worldLocation) {
    return reinterpret_cast<NI::Camera*>(
        const_cast<char*>(reinterpret_cast<const char*>(worldLocation))
        - offsetof(NI::AVObject, worldTransform) - offsetof(NI::Transform, translation));
}

bool isFollower(const Followers& followers, const NI::Camera* camera) {
    if (!camera) {
        return false;
    }
    for (const NI::Camera* follower : followers) {
        if (follower == camera) {
            return true;
        }
    }
    return false;
}

float tanOfHalfAngle(float degrees) {
    if (degrees != cachedDegrees) {
        cachedDegrees = degrees;
        cachedTan = std::tan(degrees * PI / 360.0f);
    }
    return cachedTan;
}

CameraPose readPose(const NI::Camera* camera) {
    return { camera->worldTransform, camera->worldDirection, camera->worldUp, camera->worldRight };
}

const char* nameOf(const NI::Camera* camera) {
    const TES3::WorldController* worldController = TES3::WorldController::get();
    if (camera == worldController->worldCamera.cameraData.camera) {
        return "world";
    }
    if (camera == worldController->armCamera.cameraData.camera) {
        return "arm";
    }
    return "shadow";
}

void logPose(const char* label, const CameraPose& pose) {
    const float* rotation = &pose.world.rotation.m0.x;
    LOG::logline("--   %s: translation %.9g %.9g %.9g, rotation %.9g %.9g %.9g / %.9g %.9g %.9g / %.9g %.9g %.9g, "
                 "direction %.9g %.9g %.9g, up %.9g %.9g %.9g, right %.9g %.9g %.9g",
        label, pose.world.translation.x, pose.world.translation.y, pose.world.translation.z,
        rotation[0], rotation[1], rotation[2], rotation[3], rotation[4], rotation[5], rotation[6], rotation[7], rotation[8],
        pose.direction.x, pose.direction.y, pose.direction.z, pose.up.x, pose.up.y, pose.up.z,
        pose.right.x, pose.right.y, pose.right.z);
}

// The culling planes are made at the camera's Update, from the frustum. A write after that
// leaves them at the old frustum for this Click, so the planes are made again. The engine
// function also derives the world transform from the parent again. The engine has just set
// it, so it should stay; the pose is compared before and after, and a change is logged.
void rebuildPlanes(NI::Camera* camera) {
    const CameraPose before = readPose(camera);
    camera->updateWorldData();
    const CameraPose after = readPose(camera);

    const bool moved = std::memcmp(&before, &after, sizeof(CameraPose)) != 0;
    ++rebuildCount;
    if (moved) {
        LOG::logline("!! Camera field of view: rebuilding the culling planes of the %s camera (%p) moved it.",
            nameOf(camera), static_cast<const void*>(camera));
        logPose("before", before);
        logPose("after", after);
    } else if (rebuildCount <= LOGGED_REBUILDS) {
        LOG::logline("-- Camera field of view: rebuilt the culling planes of the %s camera (%p); it did not move.",
            nameOf(camera), static_cast<const void*>(camera));
    }
}

// The frustum of a camera that follows: the same horizontal field of view, and the aspect
// of the world camera. True when the frustum changed.
bool writeFrustum(NI::Camera* camera, float fovtan, float fovtanaspect) {
    NI::Frustum& frustum = camera->viewFrustum;
    if (std::fabs(frustum.left + fovtan) <= FOV_TOLERANCE) {
        return false;
    }
    frustum.left = -fovtan;
    frustum.right = fovtan;
    frustum.top = fovtanaspect;
    frustum.bottom = -fovtanaspect;
    return true;
}

// The two numbers of the write. The aspect is that of the world camera, which holds it before
// and after its own write. False when the world camera has none.
bool frustumTerms(float screenFOV, float& fovtan, float& fovtanaspect) {
    const NI::Camera* worldCamera = TES3::WorldController::get()->worldCamera.cameraData.camera;
    if (!worldCamera) {
        return false;
    }
    const NI::Frustum& worldFrustum = worldCamera->viewFrustum;
    if (worldFrustum.right <= 0.0f || worldFrustum.top <= 0.0f) {
        return false;
    }
    fovtan = tanOfHalfAngle(screenFOV);
    fovtanaspect = fovtan / (worldFrustum.right / worldFrustum.top);
    return true;
}

}  // namespace

void NI::Camera::updateWorldData() {
    TES3_NI_Camera_updateWorldData(this);
}

namespace CameraFov {

bool follows(const NI::Camera* camera) {
    Followers followers;
    return collectFollowers(followers) && isFollower(followers, camera);
}

void atClick(NI::Camera* camera) {
    Followers followers;
    if (!collectFollowers(followers)) {
        // No world yet, so no camera of the world.
        clickFollows = false;
        return;
    }
    clickFollows = isFollower(followers, camera);
    if (!clickFollows || Configuration.ScreenFOV <= 0) {
        return;
    }

    float fovtan, fovtanaspect;
    if (frustumTerms(Configuration.ScreenFOV, fovtan, fovtanaspect) && writeFrustum(camera, fovtan, fovtanaspect)) {
        rebuildPlanes(camera);
    }
}

void atClickOfLocation(const float* worldLocation) {
    atClick(worldLocation ? cameraFromLocation(worldLocation) : nullptr);
}

bool lastClickFollows() {
    return clickFollows;
}

void applyToFollowers(float screenFOV) {
    Followers followers;
    float fovtan, fovtanaspect;
    if (!collectFollowers(followers) || !frustumTerms(screenFOV, fovtan, fovtanaspect)) {
        return;
    }
    for (NI::Camera* camera : followers) {
        if (camera) {
            writeFrustum(camera, fovtan, fovtanaspect);
        }
    }
}

}  // namespace CameraFov
