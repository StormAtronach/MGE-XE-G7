#include "mge/session.h"
#include "mwse/mgebridge.h"
#include "mwse/mwsenative.h"
#include "support/log.h"
#include "support/winheader.h"

#include <cstdlib>
#include <string>

namespace {
    Session::Path currentPath = Session::Path::Undecided;

    // The text of the box of a switch, shown at the game's first Direct3DCreate8; empty when
    // none waits.
    std::string pendingBox;

    const char* const BoxWhat =
        "MGE XE's renderer is not in use in this session. The game runs as it does without MGE XE: "
        "no distant land and no MGE XE graphics.";
    const char* const BoxLog = "The log of MGE XE is mgeXE.log in the Morrowind folder.";

    std::string text(const char* detail) {
        return detail ? detail : "not known";
    }

    // The log line's reason of a switch.
    std::string reasonOf(Session::Why why, const char* detail) {
        switch (why) {
        case Session::Why::NotConfigured:
            return "MGE XE is not configured";
        case Session::Why::Disabled:
            return "MGE XE is turned off (runtime.disabled)";
        case Session::Why::PartMissing:
            return "a part is missing: " + text(detail);
        case Session::Why::MwseDisabled:
            return "MWSE is turned off (runtime.mwse_disabled)";
        case Session::Why::NoNativeAbi:
            return "MWSE has no native ABI of version " + text(detail);
        case Session::Why::EventRefused:
            return "MWSE refused " + text(detail);
        case Session::Why::PluginFailed:
            return "the plugin did not start: " + text(detail);
        case Session::Why::PluginRefused:
            return "the plugin refused: " + text(detail);
        }
        return "not known";
    }

    // The box of a switch: what happened, why, and what to do. Empty for the player's choice
    // and for a session without settings.
    std::string boxOf(Session::Why why, const char* detail) {
        std::string whyAndWhat;
        switch (why) {
        case Session::Why::NotConfigured:
        case Session::Why::Disabled:
            return std::string();
        case Session::Why::PartMissing:
            whyAndWhat = "A part of MGE XE is missing: " + text(detail) + ".\n\nInstall MGE XE again. A virus scanner can remove its programs.";
            break;
        case Session::Why::MwseDisabled:
            whyAndWhat = "MWSE is turned off in MGE XE's settings.\n\nTurn it on.";
            break;
        case Session::Why::NoNativeAbi:
            whyAndWhat = "This MWSE is too old for MGE XE (it needs MWSE's native interface, version " + text(detail) + ").\n\nUpdate MWSE.";
            break;
        case Session::Why::EventRefused:
            whyAndWhat = "MWSE refused a function that the renderer needs: " + text(detail) + ".\n\nSee MWSE.log.";
            break;
        case Session::Why::PluginFailed:
            whyAndWhat = "The renderer's plugin (akulakhan.dll) did not start.\n\nInstall MGE XE again.";
            break;
        case Session::Why::PluginRefused:
            whyAndWhat = "The renderer's plugin (akulakhan.dll) refused: " + text(detail) + ".\n\nSee mgeXE.log.";
            break;
        }
        return std::string(BoxWhat) + "\n\n" + whyAndWhat + "\n\n" + BoxLog;
    }

    // MessageBoxTimeoutW of user32: a box that closes itself after the milliseconds. Exported
    // since Windows XP, not in the documented API, so it is taken by GetProcAddress, as the
    // helper and the plugin take it.
    typedef int (WINAPI * MessageBoxTimeoutProc)(HWND, LPCWSTR, LPCWSTR, UINT, WORD, DWORD);

    // MGE_XE_BOX_SECONDS=<n>: the boxes close themselves after n seconds, for runs with no
    // person. 0 when the switch is not set or is not a number.
    unsigned boxSeconds() {
        char value[16];
        const DWORD length = GetEnvironmentVariableA("MGE_XE_BOX_SECONDS", value, sizeof(value));
        if (length == 0 || length >= sizeof(value)) {
            return 0;
        }
        char* end = nullptr;
        const unsigned long seconds = std::strtoul(value, &end, 10);
        return (end && *end == '\0') ? static_cast<unsigned>(seconds) : 0;
    }

    // The box, on the calling thread. Its text goes to the log first, as "box: ".
    void showBox(const std::string& message) {
        LOG::logline("box: %s", message.c_str());
        LOG::flush();

        std::wstring wide(message.begin(), message.end());
        const UINT kind = MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST;
        const unsigned seconds = boxSeconds();
        if (seconds == 0) {
            MessageBoxW(NULL, wide.c_str(), L"MGE XE", kind);
            return;
        }
        HMODULE user32 = GetModuleHandleA("user32.dll");
        auto timed = user32 ? reinterpret_cast<MessageBoxTimeoutProc>(GetProcAddress(user32, "MessageBoxTimeoutW")) : nullptr;
        if (!timed) {
            LOG::logline("box: not shown: this Windows has no MessageBoxTimeoutW");
            return;
        }
        timed(NULL, wide.c_str(), L"MGE XE", kind, 0, seconds * 1000);
        LOG::logline("box: closed (MGE_XE_BOX_SECONDS=%u)", seconds);
    }
}

Session::Path Session::path() {
    return currentPath;
}

bool Session::wgpu() {
    return currentPath == Path::Wgpu;
}

bool Session::direct3D9() {
    return currentPath == Path::Direct3D9;
}

bool Session::plain() {
    return currentPath == Path::Plain;
}

void Session::choose(Path path) {
    currentPath = path;
}

void Session::toPlainGame(Why why, const char* detail) {
    const std::string reason = reasonOf(why, detail);
    if (why == Why::Disabled || why == Why::NotConfigured) {
        LOG::logline("-- %s: the game runs on its own Direct3D 8", reason.c_str());
    } else {
        LOG::logline("!! wgpu renderer: not in use, %s. The game runs on its own Direct3D 8", reason.c_str());
    }
    currentPath = Path::Plain;
    MwseNative::giveBack();
    MWSE_MGEPlugin::toPlainGame();
    pendingBox = boxOf(why, detail);
}

void Session::showPendingBox() {
    if (pendingBox.empty()) {
        return;
    }
    const std::string message = pendingBox;
    pendingBox.clear();
    showBox(message);
}

void Session::closeAfterPluginRefusal(const char* detail) {
    LOG::logline("!! wgpu renderer: the plugin refused after it changed the game's code (%s). MGE XE closes the game with the exit code %u",
                 text(detail).c_str(), ExitPluginRefused);
    showBox("MGE XE's renderer could not start: its plugin (akulakhan.dll) refused after it had changed the game's code. The game closes now.\n\n"
            "To play without MGE XE's renderer, turn it off in MGE XE's settings. See mgeXE.log.");
    LOG::flush();
    ExitProcess(ExitPluginRefused);
}
