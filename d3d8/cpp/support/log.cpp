
#include "winheader.h"
#include "log.h"

#include <cstdarg>
#include <cctype>
#include <cstring>
#include <cstdio>


namespace LOG {

    static HANDLE handle = INVALID_HANDLE_VALUE;
    // The writes of the log from more than one thread (the plugin logs from the engine's cell
    // worker too): one line is one write under the lock, so lines do not mix. SRWLOCK_INIT
    // needs no call, so the lock works from DllMain on. The lock is not re-entrant: under it
    // there are only the file writes (and the pointer steps of MGE_LogLine's split), nothing
    // that logs; a write error goes to stdout, not to this log.
    static SRWLOCK writeLock = SRWLOCK_INIT;

    // A write of `size` bytes without the lock: for a caller that holds it, and for the
    // crash path, which must not wait for a thread that crashed inside a write.
    static void writeBytes(const char* bytes, std::size_t size) {
        DWORD written;
        if (handle == INVALID_HANDLE_VALUE || size == 0) {
            return;
        }
        if (!WriteFile(handle, bytes, (DWORD)size, &written, NULL)) {
            char errormsg[512] = "\0";
            FormatMessage(FORMAT_MESSAGE_FROM_SYSTEM, 0, GetLastError(), 0, errormsg, sizeof(errormsg), 0);
            std::printf("LOG: write error: %s\n", errormsg);
        }
    }


    // Keeps the log of the run before as `<filename>.1`, in place of the one of the run before
    // that, as the logs of the plugin and of the helper do: a player who starts the game again
    // after a crash still has the log of the run that crashed. No file yet: nothing to keep.
    // Kernel32 only, with no allocation: the caller is DllMain.
    static void keepRunBefore(const char* filename) {
        char before[MAX_PATH + 3];
        if (std::snprintf(before, sizeof(before), "%s.1", filename) >= static_cast<int>(sizeof(before))) {
            return;
        }
        // A move replaces the file of the run before that. When it fails (the file is open in
        // another program), a copy keeps the log at least.
        if (!MoveFileExA(filename, before, MOVEFILE_REPLACE_EXISTING) && GetLastError() != ERROR_FILE_NOT_FOUND) {
            CopyFileA(filename, before, FALSE);
        }
    }

    bool open(const char* filename) {
        close();
        keepRunBefore(filename);
        handle = CreateFile(filename, GENERIC_WRITE, FILE_SHARE_READ, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);

        if (handle == INVALID_HANDLE_VALUE) {
            char errormsg[512] = "\0";
            FormatMessage(FORMAT_MESSAGE_FROM_SYSTEM, NULL, GetLastError(), 0, errormsg, sizeof(errormsg), NULL);
            std::printf("LOG: cannot open log file %s: %s\n", filename, errormsg);
            fflush(stdout);
        }

        return handle != INVALID_HANDLE_VALUE;
    }

    std::size_t write(const char* str) {
        if (!str) {
            return 0;
        }
        std::size_t sz = std::strlen(str);
        AcquireSRWLockExclusive(&writeLock);
        writeBytes(str, sz);
        ReleaseSRWLockExclusive(&writeLock);
        return sz;
    }

    std::size_t log(const char* fmt, ...) {
        char buf[4096] = "\0";
        std::size_t result = 0;

        va_list args;
        va_start(args, fmt);

        if (fmt) {
            result = std::vsnprintf(buf, sizeof(buf), fmt, args);
        } else {
            result = 4;
            std::strcpy(buf, "LOG::log(null)\r\n");
        }

        LOG::write(buf);

        va_end(args);
        return result;
    }

    std::size_t logline(const char* fmt, ...) {
        char buf[4096] = "\0";
        std::size_t result = 0;

        va_list args;
        va_start(args, fmt);

        if (fmt) {
            result = std::vsnprintf(buf, sizeof(buf) - 4, fmt, args);
            // vsnprintf returns the length the whole text would have (or -1): a longer text is
            // cut at sizeof(buf) - 5 characters, so the end of the line goes after what is in
            // the buffer, not after `result`.
            std::strcat(buf, "\r\n");
        } else {
            result = 4;
            std::strcpy(buf, "LOG::log(null)\r\n");
        }

        write(buf);

        va_end(args);
        return result;
    }

    std::size_t winerror(const char* fmt, ...) {
        char buf[4096] = "\0";
        std::size_t result = 0;
        auto errorCode = GetLastError();

        va_list args;
        va_start(args, fmt);

        if (fmt) {
            result = std::vsnprintf(buf, sizeof(buf) - 4, fmt, args);
        } else {
            result = 4;
            std::strcpy(buf, "LOG::winerror(null)");
        }

        std::strcat(buf + std::strlen(buf), ": ");
        auto bufUsed = std::strlen(buf);
        FormatMessage(FORMAT_MESSAGE_FROM_SYSTEM, 0, errorCode, 0, buf + bufUsed, static_cast<DWORD>(sizeof(buf) - (bufUsed + 2)), 0);
        std::strcat(buf + std::strlen(buf), "\r\n");

        write(buf);

        va_end(args);
        return result;
    }

    std::size_t logbinary(void* addr, std::size_t sz) {
        char buf[128];
        BYTE* ptr = (BYTE*)addr;

        for (std::size_t y = 0; y < sz; y += 16, ptr += 16) {
            std::size_t n = (sz - y < 16) ? (sz - y) : 16;
            char* s = buf;

            s += std::sprintf(s, "  ");
            for (std::size_t x = 0; x < n; ++x) {
                s += std::sprintf(s, "%02X ", (unsigned int)ptr[x]);
            }

            s += std::sprintf(s, "    ");
            for (std::size_t x = 0; x < n; ++x) {
                if (std::isprint(ptr[x])) {
                    *s++ = (char)ptr[x];
                } else {
                    *s++ = '.';
                }
            }

            s += std::sprintf(s, "\r\n");
            write(buf);
        }

        return sz;
    }

    void flush() {
        FlushFileBuffers(handle);
    }

    void close() {
        if (handle != INVALID_HANDLE_VALUE) {
            CloseHandle(handle);
        }

        handle = INVALID_HANDLE_VALUE;
    }

}

extern "C" void __cdecl mge_log_panic(const char* message) noexcept {
    // Without the lock: the panic can come inside a write of this thread.
    if (message) {
        LOG::writeBytes(message, std::strlen(message));
    }
    LOG::writeBytes("\r\n", 2);
    LOG::flush();
}

// The plugin's lines in mgeXE.log (the contract mge_render_link.h, the logs-and-files design of
// the Numidium notes): each line of `text` (a text can have more than one, at its '\n') is
// written as "nirender: <line>\r\n", with no limit of length, all under the lock, so that the
// lines of another thread do not come between them. Any thread may call it.
extern "C" void __cdecl MGE_LogLine(const char* text) {
    static const char prefix[] = "nirender: ";
    if (!text) {
        return;
    }
    AcquireSRWLockExclusive(&LOG::writeLock);
    const char* line = text;
    for (;;) {
        const char* end = std::strchr(line, '\n');
        std::size_t size = end ? static_cast<std::size_t>(end - line) : std::strlen(line);
        // A text that ends with its line end has no empty line after it.
        if (!end && size == 0 && line != text) {
            break;
        }
        std::size_t kept = (size > 0 && line[size - 1] == '\r') ? size - 1 : size;
        LOG::writeBytes(prefix, sizeof(prefix) - 1);
        LOG::writeBytes(line, kept);
        LOG::writeBytes("\r\n", 2);
        if (!end) {
            break;
        }
        line = end + 1;
    }
    ReleaseSRWLockExclusive(&LOG::writeLock);
}
