// A C entry for the tests of MGE XE's log (src/lib.rs).

#include "support/log.h"

#include <string>

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
