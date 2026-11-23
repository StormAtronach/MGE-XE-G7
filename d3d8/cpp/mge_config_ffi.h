#pragma once

#include <cstddef>
#include <cstdint>

struct MgeConfigDoc;

enum class MgeConfigStatus : std::uint32_t {
    Ok = 0,
    MissingDefaults = 1,
    InvalidDefaults = 2,
    InvalidArgument = 3,
    UnknownPath = 4,
    BufferTooSmall = 5,
    ReloadFailed = 6,
    SaveFailed = 7,
};

extern "C" MgeConfigStatus __cdecl mge_config_open(const char* path, MgeConfigDoc** outDocument);
extern "C" void __cdecl mge_config_close(MgeConfigDoc* document);
extern "C" MgeConfigStatus __cdecl mge_config_get_num(
    const MgeConfigDoc* document, const char* path, double* outValue);
extern "C" MgeConfigStatus __cdecl mge_config_set_num(
    MgeConfigDoc* document, const char* path, double value);
extern "C" MgeConfigStatus __cdecl mge_config_get_str(
    const MgeConfigDoc* document, const char* path, char* output, std::size_t capacity);
extern "C" MgeConfigStatus __cdecl mge_config_set_str(
    MgeConfigDoc* document, const char* path, const char* value);
extern "C" MgeConfigStatus __cdecl mge_config_get_lines(
    const MgeConfigDoc* document, const char* path, char* output, std::size_t capacity);
extern "C" MgeConfigStatus __cdecl mge_config_reload(MgeConfigDoc* document);
extern "C" MgeConfigStatus __cdecl mge_config_save(MgeConfigDoc* document);
extern "C" std::uint32_t __cdecl mge_config_needs_creation(const MgeConfigDoc* document);
extern "C" MgeConfigStatus __cdecl mge_config_last_error(
    const MgeConfigDoc* document, char* output, std::size_t capacity);

// The settings of the post shader ports and their values files (mge-config's post_values;
// the Rust side of d3d8.dll, src/lib.rs, has the contract of each function).
extern "C" std::uint32_t __cdecl mge_post_setting_count(const char* shader);
extern "C" std::uint32_t __cdecl mge_post_setting(
    const char* shader, std::uint32_t index, char* name, std::size_t capacity, float* defaultValue);
extern "C" std::uint32_t __cdecl mge_post_values_read(
    const char* folder, const char* shader, float* values, std::uint8_t* given, std::uint32_t count);
extern "C" std::uint32_t __cdecl mge_post_values_queue(
    const char* folder, const char* shader, const float* values, std::uint32_t count);
