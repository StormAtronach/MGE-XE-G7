#pragma once

#include "api.h"

namespace api {
    // The mge API of the plain game (Session::toPlainGame after the MWSE-MGE link was set up:
    // the plugin did not serve). MWSE takes it in place of MGEAPI_ExportVersion and calls it for
    // every later mge Lua call. Each method that would act on the game, on MWSE, on the
    // renderer or on a shader handle does nothing and gives its "not available" answer; the
    // methods that only read or write MGE XE's settings stay as they are.
    struct MGEAPI_Plain : public MGEAPI_ExportVersion {
        bool loadConfig() override;
        void guiSetScale(float scale) override;
        bool reloadDistantLand() override;

        ShaderHandle shaderLoad(const char* id) override;
        bool shaderReload(ShaderHandle handle) override;
        ShaderHandle shaderGetShader(const char* id) override;
        ShaderHandle shaderListShaders(size_t index) override;
        const char* shaderGetName(ShaderHandle handle) override;
        bool shaderGetEnabled(ShaderHandle handle) override;
        void shaderSetEnabled(ShaderHandle handle, bool enable) override;
        int shaderGetPriority(ShaderHandle handle) override;
        void shaderSetPriority(ShaderHandle handle, int value) override;
        bool shaderGetVariableInfo(ShaderHandle handle, size_t index, ShaderVariableInfo* out_info) override;
        bool shaderGetBool(ShaderHandle handle, const char* variableName, bool* out_value) override;
        bool shaderGetFloat(ShaderHandle handle, const char* variableName, float* out_value) override;
        bool shaderGetInt(ShaderHandle handle, const char* variableName, int* out_value) override;
        bool shaderGetString(ShaderHandle handle, const char* variableName, const char** out_value) override;
        bool shaderGetFloatArray(ShaderHandle handle, const char* variableName, float* out_values, size_t* count) override;
        bool shaderGetVector(ShaderHandle handle, const char* variableName, float* out_values, size_t count) override;
        bool shaderGetMatrix(ShaderHandle handle, const char* variableName, float* out_values) override;
        bool shaderSetBool(ShaderHandle handle, const char* variableName, bool value) override;
        bool shaderSetFloat(ShaderHandle handle, const char* variableName, float value) override;
        bool shaderSetInt(ShaderHandle handle, const char* variableName, int value) override;
        bool shaderSetString(ShaderHandle handle, const char* variableName, const char* value) override;
        bool shaderSetFloatArray(ShaderHandle handle, const char* variableName, const float* values, size_t* count) override;
        bool shaderSetVector(ShaderHandle handle, const char* variableName, const float* values, size_t count) override;
        bool shaderSetMatrix(ShaderHandle handle, const char* variableName, const float* values) override;

        void saveScreenshot(const char* path, bool captureWithUI) override;

        float nearRenderDistanceGet() override;
        void nearRenderDistanceSet(float distance) override;
        bool shaderGetBoolArray(ShaderHandle handle, const char* variableName, int* values, size_t* count) override;
        bool shaderGetIntArray(ShaderHandle handle, const char* variableName, int* values, size_t* count) override;
        bool shaderGetVectorArray(ShaderHandle handle, const char* variableName, float* values, size_t* count) override;
        bool shaderSetBoolArray(ShaderHandle handle, const char* variableName, const int* values, size_t* count) override;
        bool shaderSetIntArray(ShaderHandle handle, const char* variableName, const int* values, size_t* count) override;
        bool shaderSetVectorArray(ShaderHandle handle, const char* variableName, const float* values, size_t* count) override;
    };
}
