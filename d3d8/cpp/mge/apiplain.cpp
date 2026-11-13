#include "apiplain.h"

namespace api {
    // MWSE raises a Lua error when loadConfig answers false: true, and nothing is read.
    bool MGEAPI_Plain::loadConfig() { return true; }
    void MGEAPI_Plain::guiSetScale(float) {}
    bool MGEAPI_Plain::reloadDistantLand() { return false; }

    // No shader exists, so no handle is given; MWSE answers nil for a null handle.
    ShaderHandle MGEAPI_Plain::shaderLoad(const char*) { return nullptr; }
    bool MGEAPI_Plain::shaderReload(ShaderHandle) { return false; }
    ShaderHandle MGEAPI_Plain::shaderGetShader(const char*) { return nullptr; }
    ShaderHandle MGEAPI_Plain::shaderListShaders(size_t) { return nullptr; }
    const char* MGEAPI_Plain::shaderGetName(ShaderHandle) { return nullptr; }
    bool MGEAPI_Plain::shaderGetEnabled(ShaderHandle) { return false; }
    void MGEAPI_Plain::shaderSetEnabled(ShaderHandle, bool) {}
    int MGEAPI_Plain::shaderGetPriority(ShaderHandle) { return 0; }
    void MGEAPI_Plain::shaderSetPriority(ShaderHandle, int) {}
    bool MGEAPI_Plain::shaderGetVariableInfo(ShaderHandle, size_t, ShaderVariableInfo*) { return false; }
    bool MGEAPI_Plain::shaderGetBool(ShaderHandle, const char*, bool*) { return false; }
    bool MGEAPI_Plain::shaderGetFloat(ShaderHandle, const char*, float*) { return false; }
    bool MGEAPI_Plain::shaderGetInt(ShaderHandle, const char*, int*) { return false; }
    bool MGEAPI_Plain::shaderGetString(ShaderHandle, const char*, const char**) { return false; }
    bool MGEAPI_Plain::shaderGetFloatArray(ShaderHandle, const char*, float*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderGetVector(ShaderHandle, const char*, float*, size_t) { return false; }
    bool MGEAPI_Plain::shaderGetMatrix(ShaderHandle, const char*, float*) { return false; }
    bool MGEAPI_Plain::shaderSetBool(ShaderHandle, const char*, bool) { return false; }
    bool MGEAPI_Plain::shaderSetFloat(ShaderHandle, const char*, float) { return false; }
    bool MGEAPI_Plain::shaderSetInt(ShaderHandle, const char*, int) { return false; }
    bool MGEAPI_Plain::shaderSetString(ShaderHandle, const char*, const char*) { return false; }
    bool MGEAPI_Plain::shaderSetFloatArray(ShaderHandle, const char*, const float*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderSetVector(ShaderHandle, const char*, const float*, size_t) { return false; }
    bool MGEAPI_Plain::shaderSetMatrix(ShaderHandle, const char*, const float*) { return false; }

    void MGEAPI_Plain::saveScreenshot(const char*, bool) {}

    float MGEAPI_Plain::nearRenderDistanceGet() { return 0.0f; }
    void MGEAPI_Plain::nearRenderDistanceSet(float) {}
    bool MGEAPI_Plain::shaderGetBoolArray(ShaderHandle, const char*, int*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderGetIntArray(ShaderHandle, const char*, int*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderGetVectorArray(ShaderHandle, const char*, float*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderSetBoolArray(ShaderHandle, const char*, const int*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderSetIntArray(ShaderHandle, const char*, const int*, size_t*) { return false; }
    bool MGEAPI_Plain::shaderSetVectorArray(ShaderHandle, const char*, const float*, size_t*) { return false; }
}
