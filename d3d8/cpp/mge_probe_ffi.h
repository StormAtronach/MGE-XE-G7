#pragma once

#include <cstddef>

// The probe of the wgpu renderer (d3d8/src/probe.rs, the crate d3d8/crates/probe).
//
// The cached answer of the probe in the file `path` (mgeXE-renderer-probe.txt): 0 none, 1
// usable, 2 no answer to use (no cache, or a "no answer" line: the probe asks again). A file
// read only, for DllMain. The cached reason goes into `reason` (at most `capacity` bytes with
// the end).
extern "C" unsigned int __cdecl mge_probe_cached(const char* path, char* reason, std::size_t capacity);

// What a start does about the wgpu path (mge_probe::Verdict; the decision table is its test,
// and a test of the crate compares this list with it).
enum MgeProbeVerdict : unsigned int {
    // The probe said none or crashed: Direct3D 9, and the box.
    MgeProbeNone = 0,
    // The probe said usable: the wgpu path.
    MgeProbeUsable = 1,
    // The cache said none: Direct3D 9 (DllMain chose it), no box.
    MgeProbeCachedNone = 2,
    // The cache said usable: the wgpu path, no probe.
    MgeProbeCachedUsable = 3,
    // No answer within the limit, the first time for this machine: the probe was ended, and
    // the wgpu path is tried.
    MgeProbeNoAnswer = 4,
    // No answer twice in a row for this machine: none, with a box that says so.
    MgeProbeNoAnswerTwice = 5,
    // The probe did not start (a program can hold numidium.exe): Direct3D 9 for this start,
    // a box that blames no graphics card; not cached, the next start asks again.
    MgeProbeNotStarted = 6,
};

// The verdict for this machine: the cache's when its key is `key` and it holds an answer, else
// the one of `numidium.exe --probe` (`exe`, the refusal code `exitGpu` = MGE_PROBE_EXIT_GPU, a
// limit of `seconds`), whose line then goes into the cache. Returns a MgeProbeVerdict; the
// reason goes into `reason`, and the probe's own lines for mgeXE.log into `lines`, separated
// by '\n' (empty when no probe ran). Not for DllMain: the probe is a child process and a wait.
extern "C" unsigned int __cdecl mge_probe_check(const char* exe, const char* path, const char* key, int exitGpu, unsigned int seconds, char* reason, std::size_t capacity, char* lines, std::size_t linesCapacity);
