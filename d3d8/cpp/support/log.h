#pragma once

#include <cstddef>


namespace LOG {
    // Opens the log at `filename` for this run; the log of the run before stays as
    // `<filename>.1`.
    bool open(const char* filename);
    std::size_t write(const char* str);
    std::size_t log(const char* fmt, ...);
    std::size_t logline(const char* fmt, ...);
    std::size_t logbinary(void* addr, std::size_t sz);
    std::size_t winerror(const char* fmt, ...);
    void flush();
    void close();
};

extern "C" void __cdecl mge_log_panic(const char* message) noexcept;
// A line of d3d8.dll's Rust side in mgeXE.log; any thread may call it.
extern "C" void __cdecl mge_log_line(const char* text);
// The plugin's lines in mgeXE.log, prefixed "nirender: "; an export of d3d8.dll (exports.def),
// declared in the contract mge_render_link.h of the nirender plugin too. The two declarations
// must be equal (a file that sees both stops at a difference), and the contract is a C
// header: no noexcept here.
extern "C" void __cdecl MGE_LogLine(const char* text);
