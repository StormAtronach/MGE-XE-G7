#pragma once

#include <cstddef>

// The probe of the wgpu renderer (d3d8/src/probe.rs, the crate d3d8/crates/probe).
//
// The cached answer of the probe in the file `path` (mgeXE-renderer-probe.txt): 0 none, 1
// usable, 2 no cache. A file read only, for DllMain. The cached reason goes into `reason`
// (at most `capacity` bytes with the end).
extern "C" unsigned int __cdecl mge_probe_cached(const char* path, char* reason, std::size_t capacity);

// The answer for this machine: the cache's when its key is `key`, else the answer of
// `numidium.exe --probe` (`exe`, the refusal code `exitGpu` = MGE_PROBE_EXIT_GPU, a limit of
// `seconds`), which then goes into the cache. Returns 0 a probe's none, 1 a probe's usable,
// 2 a cached none, 3 a cached usable; the reason goes into `reason`. Not for DllMain: the
// probe is a child process and a wait.
extern "C" unsigned int __cdecl mge_probe_check(const char* exe, const char* path, const char* key, int exitGpu, unsigned int seconds, char* reason, std::size_t capacity);
