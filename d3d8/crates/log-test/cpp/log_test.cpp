// A C entry for the tests of MGE XE's log (src/lib.rs).

#include "support/log.h"

#include <string>
#include <thread>

// Writes into the log at `path`: a line of 5000 characters (longer than the log's buffer of
// 4096 bytes), a short line, and 20 bytes 'A' to 'T' as binary. Returns 0, or 1 when the file
// does not open.
extern "C" int mge_log_test_write(const char* path) {
    if (!LOG::open(path)) {
        return 1;
    }
    const std::string longText(5000, 'x');
    LOG::logline("%s", longText.c_str());
    LOG::logline("short %d", 7);
    unsigned char bytes[20];
    for (int i = 0; i < 20; ++i) {
        bytes[i] = static_cast<unsigned char>('A' + i);
    }
    LOG::logbinary(bytes, sizeof(bytes));
    LOG::close();
    return 0;
}

// The plugin's `text` through MGE_LogLine into the log at `path`, between two lines of the log's
// own. Returns 0, or 1 when the file does not open.
extern "C" int mge_log_test_plugin_text(const char* path, const char* text) {
    if (!LOG::open(path)) {
        return 1;
    }
    LOG::logline("before");
    MGE_LogLine(text);
    MGE_LogLine(nullptr);
    LOG::logline("after");
    LOG::close();
    return 0;
}

// Two threads at once into the log at `path`: one through MGE_LogLine, one through the log's
// own LOG::logline, `count` lines each. Returns 0, or 1 when the file does not open.
extern "C" int mge_log_test_two_threads(const char* path, int count) {
    if (!LOG::open(path)) {
        return 1;
    }
    std::thread plugin([count] {
        for (int i = 0; i < count; ++i) {
            MGE_LogLine("plugin line of the cell worker");
        }
    });
    std::thread own([count] {
        for (int i = 0; i < count; ++i) {
            LOG::logline("own line of MGE XE");
        }
    });
    plugin.join();
    own.join();
    LOG::close();
    return 0;
}

// One run of the log at `path`: it opens (the run before goes to `<path>.1`), writes `line`
// and closes. Returns 0, or 1 when the file does not open.
extern "C" int mge_log_test_run(const char* path, const char* line) {
    if (!LOG::open(path)) {
        return 1;
    }
    LOG::logline("%s", line);
    LOG::close();
    return 0;
}
