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

// The header of a post shader file (the `//!` lines; mge-config's post_header, the parser
// that the helper uses too) and the values files of its settings (mge-config's post_values).
// The Rust side of d3d8.dll, src/lib.rs, has the contract of each function.
struct MgePostHeader;

struct MgePostHeaderFacts {
    std::int64_t priorityAdjust;
    std::uint32_t variableCount;
    std::uint8_t requiresHDR;
    std::uint8_t disableSunglare;
};

extern "C" std::uint32_t __cdecl mge_post_header_read(
    const char* path, MgePostHeader** outHeader, char* error, std::size_t capacity);
extern "C" void __cdecl mge_post_header_free(MgePostHeader* header);
extern "C" void __cdecl mge_post_header_facts(const MgePostHeader* header, MgePostHeaderFacts* out);
extern "C" std::uint32_t __cdecl mge_post_header_category(
    const MgePostHeader* header, char* output, std::size_t capacity);
extern "C" std::uint32_t __cdecl mge_post_header_variable(
    const MgePostHeader* header, std::uint32_t index, char* name, std::size_t capacity, char* letter,
    std::uint32_t* valueCount, std::uint8_t* setting);
extern "C" std::uint32_t __cdecl mge_post_header_values(
    const MgePostHeader* header, std::uint32_t index, float* values, std::uint32_t count);
extern "C" std::uint32_t __cdecl mge_post_values_read(
    const char* folder, const char* shader, const char* const* names, std::uint32_t count, float* values,
    std::uint8_t* given);
extern "C" std::uint32_t __cdecl mge_post_values_queue(
    const char* folder, const char* shader, const char* const* names, std::uint32_t count, const float* values);
