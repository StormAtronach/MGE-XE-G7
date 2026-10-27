
#include "mge/configuration.h"
#include "mge/mgeversion.h"
#include "mge_probe_ffi.h"
#include "mge/mwinitpatch.h"
#include "mge/mged3d8device.h"
#include "mge/startupgen.h"
#include "mwse/mgebridge.h"
#include "support/winheader.h"
#include "support/log.h"
#include "support/crashlog.h"

#include <cstdio>
#include <string>



extern void* CreateD3DWrapper(UINT);
extern void* CreateInputWrapper(void*);

static FARPROC getProc1(const char* lib, const char* funcname);
static void setDPIScalingAware();

static const char* welcomeMessage = MGE_BUILD_ID;
static bool isMW;
// The wgpu path is wanted for this session: the setting is on and its files are there. The
// probe's key is checked at the first ask (wgpuPathOn) on both paths, so that a cached none
// is probed again when the graphics card, its driver or numidium.exe change.
static bool wgpuWanted = false;
// The cache of the probe's last answer, beside mgeXE.log (the failure-path design, section 6).
static const char* const kProbeCache = "mgeXE-renderer-probe.txt";

static void showMessageBox(const char* text, bool endGame);

// The wgpu path is off for this session: the log line with the reason, the box once when
// `box` has a text for the player (the four parts of the pattern: what, why, what the game
// does now, where the details are), and the setting of the session. The callers: the probe
// (through wgpuPathOn, at the plugin's ask or at the game's Direct3DCreate8) and the plugin
// that cannot serve when the game asks for its Direct3D 8 (the proxy, no distant land). The
// box runs on its own thread (showMessageBox); the game goes on at once.
static void wgpuPathOff(const char* reason, const std::string& box) {
    LOG::logline("!! wgpu renderer: not in use, %s. MGE XE continues with Direct3D 9", reason);
    Configuration.RendererWgpu = false;
    if (box.empty()) {
        return;
    }
    static bool shown = false;
    if (shown) {
        return;
    }
    shown = true;
    // Not a stop box: the game goes on, on Direct3D 9.
    showMessageBox(box.c_str(), false);
}



bool g_isProcessExiting = false;

extern "C" BOOL _stdcall DllMain(HANDLE hModule, DWORD reason, void* lpvReserved) {
    if (reason == DLL_PROCESS_DETACH) {
        if (lpvReserved != NULL) {
            g_isProcessExiting = true;
        }
        return true;
    }

    if (reason != DLL_PROCESS_ATTACH) {
        return true;
    }

    // Check if MW is in-process, avoid hooking the CS
    isMW = bool(GetModuleHandle("Morrowind.exe"));

    if (isMW) {
        LOG::open("mgeXE.log");
        LOG::logline(welcomeMessage);
        CrashLog::install();

        setDPIScalingAware();

        if (!Configuration.LoadSettings()) {
            LOG::logline("Error: MGE XE is not configured. MGE XE will be disabled for this session.");
            isMW = false;
            return true;
        }

        if (Configuration.MGEFlags & MGE_DISABLED) {
            // Signal that DirectX proxies should not load
            isMW = false;
        }

        if (Configuration.RendererWgpu) {
            // The wgpu renderer is on by default. It needs the helper program, which draws,
            // and the nirender plugin with MWSE, which gives the game its Direct3D 8. When a
            // part is missing, MGE XE runs as without the switch: all of MGE XE on
            // Direct3D 9. A program that is missing was taken away (a virus scanner) or was
            // not installed. The decision is made here, before the plugin asks, so the
            // plugin stays idle.
            //
            // Only the files in the folder of the game are examined. This code runs when
            // the game loads d3d8.dll. At that time a mod manager with a virtual file
            // system (Mod Organizer) does not show the files of its mods: a file below
            // `Data Files` that is there looks missing. The plugin below `Data Files` is
            // examined later, when the game asks for its Direct3D 8.
            static const char* const parts[] = {
                "numidium.exe",
                "MWSE.dll",
            };
            const char* missing = nullptr;
            for (const char* part : parts) {
                if (GetFileAttributesA(part) == INVALID_FILE_ATTRIBUTES) {
                    missing = part;
                    break;
                }
            }
            if (!missing && (Configuration.MGEFlags & MWSE_DISABLED)) {
                missing = "MWSE (the setting runtime.mwse_disabled is on)";
            }
            if (missing) {
                wgpuPathOff((std::string("a part is missing: ") + missing).c_str(), "");
            }
        }

        if (Configuration.RendererWgpu) {
            wgpuWanted = true;
            // The probe's last answer, from its cache file: a file read only, since this is
            // DllMain (no child process and no wait under the loader lock). A cached none
            // takes all of MGE XE on Direct3D 9 for this start, with no box; the key is
            // checked later (wgpuPathOn).
            char reason[512] = {};
            if (mge_probe_cached(kProbeCache, reason, sizeof(reason)) == 0) {
                wgpuPathOff((std::string("the last probe said none (") + kProbeCache + "): " + reason
                    + "; the probe runs again when the graphics card, its driver or numidium.exe change").c_str(), "");
            }
        }

        if (Configuration.RendererWgpu) {
            // The wgpu renderer of the helper draws the picture, and no Direct3D device
            // exists. The parts of MGE XE that need its device stay off, as in proxy-only mode.
            LOG::logline(">> Using the wgpu renderer: no Direct3D device");
            Configuration.OnlyProxyD3D8To9 = true;
        }

        // Early startup patches
        MWInitPatch::patch();

        if (~Configuration.MGEFlags & MWSE_DISABLED) {
            // Load MWSE dll, it injects by itself
            HMODULE dll = LoadLibraryA("MWSE.dll");

            if (dll) {
                if ((~Configuration.MGEFlags & MGE_DISABLED) && (!Configuration.OnlyProxyD3D8To9 || Configuration.RendererWgpu)) {
                    // MWSE-MGE integration
                    MWSE_MGEPlugin::init(dll);
                }
                LOG::logline("MWSE.dll injected.");
            } else {
                LOG::logline("MWSE.dll failed to load.");
            }
            if (Configuration.RendererWgpu) {
                // MWSE applies MGE XE's early engine changes on the wgpu path (MWInitPatch::patch
                // left them out).
                MWInitPatch::patchWgpu(dll);
            }
        } else {
            LOG::logline("MWSE is disabled.");
        }

        CrashLog::installUnhandledFilter();
    }

    // Load extender for CS, if Construction Set detected
    bool isCS = bool(GetModuleHandle("TES Construction Set.exe"));
    if (isCS) {
        // Load CSSE dll, it injects by itself
        LoadLibraryA("CSSE.dll");
    }

    return true;
}



// The Direct3D 8 of the nirender plugin: an object that makes no Direct3D device. The plugin
// sends the draws of the game as a stream, and numidium.exe draws the stream with wgpu.
// Returns null when the plugin is not there.
// The function of the nirender plugin that puts data of MGE XE into its stream, or null.
static void (__cdecl * streamExtension)(UINT id, const void* data, UINT length) = nullptr;

// Sends data to the wgpu renderer in the helper. It arrives with the frame in progress.
void sendToRenderer(UINT id, const void* data, UINT length) {
    if (streamExtension) {
        streamExtension(id, data, length);
    }
}

// A message box of MGE XE with the caption "MGE XE". The rule of every box of the three parts
// (the failure-path design of the Numidium docs, sections 5 and 5.1; the plugin's and the
// helper's boxes follow it too): each line goes to mgeXE.log first as "box: "; with
// MGE_XE_BOX_SECONDS=<n> in the environment the box closes itself after n seconds, through
// MessageBoxTimeoutA of user32 (not in the documented API, so taken by GetProcAddress; without
// it the box is only logged); without the switch it waits for the click. The old-plugin case
// only logs since D10 (a); the box of the hardware fallback (C3) is the caller.
//
// The box runs on a thread of its own, with no owner window, and nothing waits for it (the
// thread rule of the design, section 5). A box on the game's main thread pumps the game's
// window messages while it is up; during the start of the game the window procedure then
// restarts a renderer that does not exist yet, and the game crashed (run 1c of the probe,
// 2026-10-27: Tes3Game::restartRenderer, NiAVObject::UpdateEffects). The "box:" lines are
// written on the calling thread first; the box's thread writes how it closed.
//
// `endGame` is the close rule of a stop box (the design, section 5): when the box closes, its
// thread posts WM_CLOSE to the game's window, since with the box on its own thread a first
// Alt+F4 closes the box and not the game. A box after which the game goes on passes false.
static DWORD WINAPI messageBoxThread(LPVOID parameter);

// What the thread of a box takes: its text and whether the game ends after it.
struct MessageBoxJob {
    std::string text;
    bool endGame;
};

static void showMessageBox(const char* text, bool endGame) {
    std::string lines(text);
    for (size_t start = 0; start <= lines.size();) {
        size_t end = lines.find('\n', start);
        if (end == std::string::npos) {
            end = lines.size();
        }
        LOG::logline("box: %s", lines.substr(start, end - start).c_str());
        start = end + 1;
    }
    // The thread owns the job and frees it.
    MessageBoxJob* job = new MessageBoxJob{ text, endGame };
    HANDLE thread = CreateThread(nullptr, 0, messageBoxThread, job, 0, nullptr);
    if (thread) {
        CloseHandle(thread);
    } else {
        LOG::logline("box: not shown, no thread for it (error %lu)", GetLastError());
        delete job;
    }
}

// The top-level window of this process that has no owner and is visible: the game's window.
static BOOL CALLBACK findGameWindow(HWND window, LPARAM found) {
    DWORD process = 0;
    GetWindowThreadProcessId(window, &process);
    if (process == GetCurrentProcessId() && IsWindowVisible(window) && !GetWindow(window, GW_OWNER)) {
        *reinterpret_cast<HWND*>(found) = window;
        return FALSE;
    }
    return TRUE;
}

// Ends the game after a stop box: WM_CLOSE to the game's window, as Alt+F4 does.
static void closeGame() {
    HWND game = NULL;
    EnumWindows(findGameWindow, reinterpret_cast<LPARAM>(&game));
    if (game) {
        LOG::logline("box: the game closes");
        PostMessageA(game, WM_CLOSE, 0, 0);
    } else {
        LOG::logline("box: no window of the game to close");
    }
}

static DWORD WINAPI messageBoxThread(LPVOID parameter) {
    MessageBoxJob* job = static_cast<MessageBoxJob*>(parameter);
    const char* text = job->text.c_str();
    char value[16] = {};
    GetEnvironmentVariableA("MGE_XE_BOX_SECONDS", value, sizeof(value));
    // 0 to 3600: a negative value is 0, and the timeout in ms must not overflow.
    const int asked = std::atoi(value);
    const unsigned seconds = asked <= 0 ? 0u : (asked > 3600 ? 3600u : unsigned(asked));
    const UINT flags = MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST;
    const DWORD started = GetTickCount();
    if (seconds == 0) {
        MessageBoxA(NULL, text, "MGE XE", flags);
    } else {
        typedef int (WINAPI * MessageBoxTimeoutProc)(HWND, LPCSTR, LPCSTR, UINT, WORD, DWORD);
        MessageBoxTimeoutProc timed = (MessageBoxTimeoutProc)GetProcAddress(GetModuleHandleA("user32.dll"), "MessageBoxTimeoutA");
        if (!timed) {
            LOG::logline("box: not shown: this Windows has no MessageBoxTimeoutA");
        } else {
            timed(NULL, text, "MGE XE", flags, 0, seconds * 1000);
        }
    }
    // The elapsed time tells a timeout from a click: MessageBoxTimeout answers IDOK on the
    // timeout on this machine, not the documented 32000 [V Rob's run, 2026-10-27].
    if (seconds != 0 && GetTickCount() - started >= seconds * 1000) {
        LOG::logline("box: closed after %u s, no click", seconds);
    } else {
        LOG::logline("box: closed by a click");
    }
    if (job->endGame) {
        closeGame();
    }
    delete job;
    return 0;
}

static void* createStreamedD3D(UINT version) {
    char path[MAX_PATH];
    DWORD length = GetModuleFileNameA(NULL, path, MAX_PATH);
    while (length > 0 && path[length - 1] != '\\') {
        --length;
    }
    std::string folder(path, length);

    // The plugin starts the program that this variable names as its renderer.
    SetEnvironmentVariableA("NIRENDER_HOST_EXE", (folder + "numidium.exe").c_str());

    HMODULE plugin = LoadLibraryA((folder + "Data Files\\MWSE\\lib\\akulakhan.dll").c_str());
    if (!plugin) {
        LOG::logline("!! wgpu renderer: Data Files\\MWSE\\lib\\akulakhan.dll did not load");
        return nullptr;
    }
    typedef void* (__cdecl * CreateProc) (UINT);
    CreateProc create = (CreateProc)GetProcAddress(plugin, "nirender_direct3d_create8");
    if (!create) {
        LOG::logline("!! wgpu renderer: the nirender plugin has no nirender_direct3d_create8");
        return nullptr;
    }
    streamExtension = (void (__cdecl *)(UINT, const void*, UINT))GetProcAddress(plugin, "nirender_stream_extension");
    // The build id of the plugin, for the log. A plugin without the export is from before the
    // handshake: the game goes on with its device, and the helper refuses its stream (no
    // stream format in the HEADER) with one box and the exit code 10. No Direct3D 9 fallback:
    // the plugin's live ports, installed early, leave that frame black (the owner's D10 (a),
    // 2026-10-27, after run 4c of records entry 192).
    typedef const char* (__cdecl * BuildIdProc) ();
    BuildIdProc buildId = (BuildIdProc)GetProcAddress(plugin, "nirender_build_id");
    if (!buildId) {
        LOG::logline("!! wgpu renderer: akulakhan.dll is from before the version handshake (no nirender_build_id); this d3d8.dll is " MGE_BUILD_ID "; the helper refuses its stream");
    }
    LOG::logline("-- wgpu renderer: the nirender plugin (%s) gives the game its Direct3D 8", buildId ? buildId() : "a build from before the handshake");
    // Before the stream starts: the plugin keeps the record and writes it after the header.
    Deviceless::sendHello();
    void* d3d = create(version);
    // A plugin that refuses (no ports) gives the game no device: the Direct3D 9 path must not
    // send the plugin its records then.
    if (!d3d) {
        streamExtension = nullptr;
    }
    return d3d;
}

static bool probeSaysUsable();

// The wgpu path for this session: the setting, and the probe's answer, asked once. The first
// of the two that need it asks the probe: the plugin when it loads (MgeRendererWgpuActive),
// or the game when it asks for its Direct3D 8. The plugin asks first in a game with MWSE: it
// must hear the answer before it puts its renderer code on, since its code stays on and a
// Direct3D 9 frame under it is black.
static bool wgpuPathOn() {
    if (!isMW || !wgpuWanted) {
        return false;
    }
    static const bool usable = probeSaysUsable();
    return usable && Configuration.RendererWgpu;
}

// The nirender plugin asks this when it loads: nonzero when the game gets its Direct3D 8
// from the plugin. The plugin then puts its renderer code on before the game makes its renderer.
extern "C" UINT _stdcall MgeRendererWgpuActive() {
    return wgpuPathOn() ? 1 : 0;
}

// The nirender plugin calls these two at the points of a frame where the proxy device ran
// the logic of MGE XE that is not drawing. See Deviceless in mge/mged3d8device.cpp.
extern "C" void __cdecl MGE_StreamPresent(UINT width, UINT height) {
    if (isMW && Configuration.RendererWgpu) {
        Deviceless::present(width, height);
    }
}

// The nirender plugin calls this at the start of each camera's Click, before its transforms,
// with the scene packet of its contract (mge_render_link.h).
extern "C" void __cdecl MGE_StreamSceneBegin(const MgeLinkSceneV1* scene) {
    if (isMW && Configuration.RendererWgpu && scene) {
        Deviceless::sceneBegin(scene);
    }
}

extern "C" void __cdecl MGE_StreamTransform(UINT state, float* matrix, UINT targetIsBackBuffer) {
    if (isMW && Configuration.RendererWgpu) {
        Deviceless::transform(state, reinterpret_cast<D3DMATRIX*>(matrix), targetIsBackBuffer != 0);
    }
}

// The nirender plugin calls this for each message of the extension in the helper, at the end
// of a frame of the game. See Deviceless::message.
extern "C" void __cdecl MGE_StreamMessage(UINT id, const void* data, UINT length) {
    if (isMW && Configuration.RendererWgpu && data) {
        Deviceless::message(id, data, length);
    }
}

// The GPU: `numidium.exe --probe` asks the backends for what the renderer needs, without a
// window, and answers with its last line and its exit code (MGE_PROBE_EXIT_OK or
// MGE_PROBE_EXIT_GPU); about 0.3 s. Any other end of the probe counts as none, so that a
// crash of it leaves the game on Direct3D 9 with the code in the log. It runs once, through
// wgpuPathOn, and never in DllMain: a child process and a wait under the loader lock hung
// the game (the first live run, 2026-10-27).
// The key of this machine for the probe's cache: each display adapter of the registry (its
// name and its driver version) and the size and the write time of numidium.exe. A new card,
// a new driver or a new build of the helper changes it.
static std::string probeKey() {
    std::string key;
    HKEY adapters;
    const char* displayClass = "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}";
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE, displayClass, 0, KEY_READ, &adapters) == ERROR_SUCCESS) {
        char name[256];
        for (DWORD index = 0;; ++index) {
            DWORD nameLength = sizeof(name);
            if (RegEnumKeyExA(adapters, index, name, &nameLength, nullptr, nullptr, nullptr, nullptr) != ERROR_SUCCESS) {
                break;
            }
            char description[256] = {};
            char version[64] = {};
            DWORD size = sizeof(description);
            // A subkey without a driver description is not an adapter (the "Properties" key).
            if (RegGetValueA(adapters, name, "DriverDesc", RRF_RT_REG_SZ, nullptr, description, &size) != ERROR_SUCCESS) {
                continue;
            }
            size = sizeof(version);
            RegGetValueA(adapters, name, "DriverVersion", RRF_RT_REG_SZ, nullptr, version, &size);
            key += std::string(description) + " " + version + "; ";
        }
        RegCloseKey(adapters);
    }
    WIN32_FILE_ATTRIBUTE_DATA helper;
    if (GetFileAttributesExA("numidium.exe", GetFileExInfoStandard, &helper)) {
        char text[96];
        std::snprintf(text, sizeof(text), "numidium.exe %lu %08lx%08lx", helper.nFileSizeLow, helper.ftLastWriteTime.dwHighDateTime, helper.ftLastWriteTime.dwLowDateTime);
        key += text;
    }
    return key;
}

static bool probeSaysUsable() {
    const std::string key = probeKey();
    char answer[1024] = {};
    const DWORD started = GetTickCount();
    const unsigned result = mge_probe_check("numidium.exe", kProbeCache, key.c_str(), MGE_PROBE_EXIT_GPU, 5, answer, sizeof(answer));
    const DWORD ms = GetTickCount() - started;
    const bool usable = (result & 1) != 0;
    if (result >= 2) {
        // The cache had this key: no probe ran. A cached none turned the path off in DllMain.
        LOG::logline("-- wgpu renderer: the cached answer for this machine: %s %s", usable ? "usable," : "none:", answer);
        return usable;
    }
    if (usable) {
        LOG::logline("-- wgpu renderer: the probe says %s (%lu ms)", answer, ms);
        if (!Configuration.RendererWgpu) {
            LOG::logline("-- wgpu renderer: usable again on this machine; this start stays on Direct3D 9, the next uses the wgpu renderer");
        }
        return true;
    }
    LOG::logline("-- wgpu renderer: the probe took %lu ms", ms);
    if (!Configuration.RendererWgpu) {
        // A cached none, a new key, and still none: the log line only.
        LOG::logline("-- wgpu renderer: the probe says none again: %s", answer);
        return false;
    }
    wgpuPathOff((std::string("the probe says none: ") + answer).c_str(),
        std::string("The wgpu renderer of MGE XE is not in use on this machine.\n")
            + "The graphics card or its driver lacks what it needs: " + answer + ".\n"
            + "For this start the game runs on Direct3D 9 without the distant land; from the next start MGE XE runs on Direct3D 9 with it.\n"
            + "mgeXE.log in the folder of the game has the details.");
    return false;
}

extern "C" void* _stdcall FakeDirect3DCreate(UINT version) {
    // Wrap Morrowind only, not TESCS
    if (wgpuPathOn()) {
        Configuration.EnsureSettingsFile();
        if (void* d3d = createStreamedD3D(version)) {
            return d3d;
        }
        // The plugin did not serve; a caller that said why has turned the path off already.
        if (Configuration.RendererWgpu) {
            wgpuPathOff("the plugin did not give the game its Direct3D 8 (proxy-only mode)", "");
        }
    }
    if (isMW) {
        Configuration.EnsureSettingsFile();
        StartupGeneration::launchEarlyHost(isMW);
        return CreateD3DWrapper(version);
    } else {
        // Use system D3D8
        typedef void* (_stdcall * D3DProc) (UINT);
        D3DProc func = (D3DProc)getProc1("d3d8.dll", "Direct3DCreate8");
        return (func)(version);
    }
}

extern "C" HRESULT _stdcall FakeDirectInputCreate(HINSTANCE a, DWORD b, REFIID c, void** d, void* e) {
    typedef HRESULT (_stdcall * DInputProc) (HINSTANCE, DWORD, REFIID, void**, void*);
    DInputProc func = (DInputProc)getProc1("dinput8.dll", "DirectInput8Create");

    void* dinput = 0;
    HRESULT hr = (func)(a, b, c, &dinput, e);

    if (hr == S_OK) {
        if (isMW) {
            *d = CreateInputWrapper(dinput);
        } else {
            *d = dinput;
        }
    }

    return hr;
}


FARPROC getProc1(const char* lib, const char* funcname) {
    char syspath[MAX_PATH], path[MAX_PATH];
    GetSystemDirectoryA(syspath, sizeof(syspath));

    int str_sz = std::snprintf(path, sizeof(path), "%s\\%s", syspath, lib);
    if (str_sz >= sizeof(path)) {
        return NULL;
    }

    HMODULE dll = LoadLibraryA(path);
    if (dll == NULL) {
        return NULL;
    }

    return GetProcAddress(dll, funcname);
}

void setDPIScalingAware() {
    // Prevent DPI scaling from affecting chosen window size
    typedef BOOL (WINAPI * dpiProc)();
    dpiProc SetProcessDPIAware = (dpiProc)getProc1("user32.dll", "SetProcessDPIAware");

    if (SetProcessDPIAware) {
        SetProcessDPIAware();
    }
}
