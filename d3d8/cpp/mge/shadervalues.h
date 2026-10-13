#pragma once

#include <string>
#include <vector>

// The variables of a post shader in the wgpu renderer mode.
//
// In that mode there is no Direct3D device, so there is no effect object that knows the
// variables of a shader. A mod loads a shader and sets its variables through the API of
// MGE XE (the MWSE functions mge.shaders.load, shader.name = value). The variables come from
// the text of the effect file; the values that the mod sets are kept here and go to the
// renderer in the helper, which runs the WGSL port of the shader.

struct ShaderVariable {
    std::string name;
    // The type letter of MGEAPI::shaderGetVariableInfo: b, i, f for one value; B, I, F for
    // an array of them; 2, 3, 4 for a vector; V for an array of vectors; m for a matrix;
    // s for a string (kept without a value).
    char type;
    // The values, as floats: the default of the file, then what the mod set. A vector of
    // an array V has 4 values.
    std::vector<float> values;
    // The mod set the variable. Only such a variable goes to the renderer: a variable
    // that no mod set keeps the default of the port.
    bool set;
};

struct ShaderValues {
    std::vector<ShaderVariable> variables;
    // The renderer does not have the state of the shader yet (its values, its enabled
    // flag or its priority changed).
    bool changed = true;
    // A mod set the priority of the shader: without that, its place in the chain is the
    // one that its effect file gives.
    bool prioritySet = false;

    // Reads the variables of an effect file. A file that cannot be read gives none.
    static ShaderValues parse(const char* path);

    ShaderVariable* find(const char* name);

    // Sets a variable from `count` floats. Returns false for a name the file does not have.
    bool set(const char* name, const float* values, size_t count);
    // Copies the values of a variable into `out`, at most `count`. Returns the number of
    // values the variable has, 0 for a name the file does not have.
    size_t get(const char* name, float* out, size_t count);
};
