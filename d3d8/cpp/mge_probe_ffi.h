#pragma once

#include <cstddef>

// The probe of the wgpu renderer (d3d8/src/probe.rs, the crate d3d8/crates/probe): runs
// `numidium.exe --probe` with a pipe and a time limit of `seconds`, and reads its answer.
// `exitGpu` is the exit code of a refusal (MGE_PROBE_EXIT_GPU of mgeversion.h, the twin of
// the host crate's EXIT_GPU). Returns 1 when the GPU is usable, 0 when not; `reason` gets
// the backend to use or the reason of the refusal (at most `capacity` bytes with the end).
extern "C" unsigned int __cdecl mge_probe_wgpu(const char* exe, int exitGpu, unsigned int seconds, char* reason, std::size_t capacity);
