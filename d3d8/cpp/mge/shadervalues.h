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
    // s for a string.
    char type;
    // The values, as floats: the default of the file, then what the mod set. A vector of
    // an array V has 4 values.
    std::vector<float> values;
    // The value of a string: the default of the file, then what the mod set. The renderer
    // does not get it: no port reads a string.
    std::string text;
    // The mod set the variable. Only such a variable goes to the renderer: a variable
    // that no mod set keeps the default of the port.
    bool set;
    // A setting of the port (mge-config's post_values): its value is kept in the values file
    // of the shader between game sessions.
    bool setting = false;
};

struct ShaderValues {
    std::vector<ShaderVariable> variables;
    // The renderer does not have the state of the shader yet (its values, its enabled
    // flag or its priority changed).
    bool changed = true;
    // A mod set the priority of the shader: without that, its place in the chain is the
    // one that its effect file gives.
    bool prioritySet = false;
    // The number of times that a mod reloaded the shader (shader:reload() of MWSE). The
    // renderer reads the files of the shader again when the number changes.
    unsigned reloads = 0;
    // The annotations of the first technique of the file: its category (empty without one)
    // and its priority adjustment. They give the place of the shader in the chain.
    std::string category;
    int priorityAdjust = 0;
    // The annotations `requiresHDR` and `disableSunglare` of the technique.
    bool requiresHDR = false;
    bool disableSunglare = false;
    // The first technique was read. The annotations of a second one do not count, as with
    // a device (GetTechnique(0)).
    bool techniqueRead = false;
    // The shader and the folder of its values file, for the writes of the settings; empty
    // for a shader whose port has no settings.
    std::string settingsShader;
    std::string settingsFolder;

    // Reads the variables of an effect file. A file that cannot be read gives none.
    static ShaderValues parse(const char* path);
    // Reads the `//!` lines at the start of a WGSL file of a shader (mgeHost64's
    // POST_SHADERS.md, "The file"), for a shader that has no effect file: the category, the
    // priority adjustment, `requires_hdr`, `disable_sunglare`, `mgeflags` as the variable
    // `mgeflags`, and the variables of `variables` with their start values. Returns false,
    // with the reason in `error`, for a file that cannot be read or a header that the
    // renderer does not accept: then the renderer does not load the shader either.
    static bool parseWgslHeader(const char* path, ShaderValues* out, std::string* error);

    // Adds the settings of the port of `shader` that the file does not have, with their
    // defaults, and marks every setting; then takes the values of the shader's values file
    // `<shader>.values.toml` in `folder` as set. A missing or broken file leaves the defaults.
    void addPortSettings(const char* folder, const char* shader);
    // Some variable is set: the renderer must get the state.
    bool anySet() const;

    ShaderVariable* find(const char* name);

    // Sets a variable from `count` floats. Returns false for a name the file does not have.
    // A set of a setting queues a write of the values file (see addPortSettings).
    bool set(const char* name, const float* values, size_t count);
    // Copies the values of a variable into `out`, at most `count`. Returns the number of
    // values the variable has, 0 for a name the file does not have.
    size_t get(const char* name, float* out, size_t count);
    // The value of a string variable, or null for a name that is not a string of the file.
    // The text stays valid until the variable is set again.
    const char* getString(const char* name);
    // Sets a string variable. Returns false for a name that is not a string of the file.
    bool setString(const char* name, const char* value);
};
