#pragma once

#define XE_VERSION_STRING "MGE-XE G7 Fork v0.21.0"
// The commit of the build, from d3d8/build.rs (build_support/build_id.rs). The MSBuild
// project does not set it.
#ifndef MGE_COMMIT
#define MGE_COMMIT "unknown"
#endif
// The build id of d3d8.dll: the first line of mgeXE.log, the hello record for the helper.
#define MGE_BUILD_ID XE_VERSION_STRING " " MGE_COMMIT
// The format of the extension records that d3d8.dll sends to the helper. The twin is
// EXTENSION_FORMAT in mgeHost64/src/renderer.rs (a unit test compares the two). Bump both
// with any change of a record's layout or of an id's meaning.
#define MGE_EXTENSION_FORMAT 1
// The exit codes of `numidium.exe --probe` that d3d8.dll reads: 0 a usable GPU, 20 none.
// The twin of 20 is EXIT_GPU in nirender-plugin's src/stream_format.rs (a unit test of
// mgeHost64 compares the two); any other code of the probe counts as none.
#define MGE_PROBE_EXIT_OK 0
#define MGE_PROBE_EXIT_GPU 20
#define MGE_MAJOR_VERSION 4
#define MGE_MINOR_VERSION 21
#define MGE_BUILD_VERSION 0

#define MGE_SAVE_VERSION 47
#define MGE_DL_VERSION 22
#define MGE_STATIC_MESH_SHARD_COUNT 128
#define MGE_STATIC_MESH_SHARD_ID_WIDTH 3
#define MGE_MWSE_VERSION ((MGE_MAJOR_VERSION*0x10000)+(MGE_MINOR_VERSION*0x100)+MGE_BUILD_VERSION)
