
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

static void showMessageBox(const char* text);

// The wgpu path is off for this session: the log line with the reason, the box once when
// `box` has a text for the player (the four parts of the pattern: what, why, what the game
// does now, where the details are), and the setting of the session. The callers: the probe
// at the load of this library (all of MGE XE on Direct3D 9), and the plugin that cannot
// serve when the game asks for its Direct3D 8 (the proxy, no distant land).
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
    showMessageBox(box.c_str());
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
            // The GPU: `numidium.exe --probe` asks the backends for what the renderer needs,
            // without a window, and answers with its last line and its exit code
            // (MGE_PROBE_EXIT_OK or MGE_PROBE_EXIT_GPU). It takes about 0.3 s. Any other end
            // of the probe counts as none, so that a crash of it leaves the game on
            // Direct3D 9 with the code in the log.
            char answer[1024] = {};
            const DWORD probeStarted = GetTickCount();
            const unsigned usable = mge_probe_wgpu("numidium.exe", MGE_PROBE_EXIT_GPU, 5, answer, sizeof(answer));
            const DWORD probeMs = GetTickCount() - probeStarted;
            if (usable) {
                LOG::logline("-- wgpu renderer: the probe says %s (%lu ms)", answer, probeMs);
            } else {
                LOG::logline("-- wgpu renderer: the probe took %lu ms", probeMs);
                wgpuPathOff((std::string("the probe says none: ") + answer).c_str(),
                    std::string("The wgpu renderer of MGE XE is not in use on this machine.\n")
                        + "The graphics card or its driver lacks what it needs: " + answer + ".\n"
                        + "The game goes on with MGE XE on Direct3D 9.\n"
                        + "mgeXE.log in the folder of the game has the details.");
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
// it the box is only logged); without the switch it waits for the click.
static void showMessageBox(const char* text) {
    std::string lines(text);
    for (size_t start = 0; start <= lines.size();) {
        size_t end = lines.find('\n', start);
        if (end == std::string::npos) {
            end = lines.size();
        }
        LOG::logline("box: %s", lines.substr(start, end - start).c_str());
        start = end + 1;
    }
    char value[16] = {};
    GetEnvironmentVariableA("MGE_XE_BOX_SECONDS", value, sizeof(value));
    const unsigned seconds = unsigned(std::atoi(value));
    const UINT flags = MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST;
    const DWORD started = GetTickCount();
    if (seconds == 0) {
        MessageBoxA(NULL, text, "MGE XE", flags);
    } else {
        typedef int (WINAPI * MessageBoxTimeoutProc)(HWND, LPCSTR, LPCSTR, UINT, WORD, DWORD);
        MessageBoxTimeoutProc timed = (MessageBoxTimeoutProc)GetProcAddress(GetModuleHandleA("user32.dll"), "MessageBoxTimeoutA");
        if (!timed) {
            LOG::logline("box: not shown: this Windows has no MessageBoxTimeoutA");
            return;
        }
        timed(NULL, text, "MGE XE", flags, 0, seconds * 1000);
    }
    // The elapsed time tells a timeout from a click: MessageBoxTimeout answers IDOK on the
    // timeout on this machine, not the documented 32000 [V Rob's run, 2026-10-27].
    if (seconds != 0 && GetTickCount() - started >= seconds * 1000) {
        LOG::logline("box: closed after %u s, no click", seconds);
    } else {
        LOG::logline("box: closed by a click");
    }
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
    // handshake: the helper would refuse its stream. The player sees why once, and the game
    // runs on Direct3D 9 (the owner's choice, 2026-10-27).
    typedef const char* (__cdecl * BuildIdProc) ();
    BuildIdProc buildId = (BuildIdProc)GetProcAddress(plugin, "nirender_build_id");
    if (!buildId) {
        wgpuPathOff("akulakhan.dll is from before the version handshake (no nirender_build_id); this d3d8.dll is " MGE_BUILD_ID,
            "The parts of the wgpu renderer of MGE XE do not match.\n"
            "akulakhan.dll is from before the version handshake.\n"
            "d3d8.dll is " MGE_BUILD_ID ".\n\n"
            "Install akulakhan.dll, numidium.exe and d3d8.dll from one archive.\n"
            "The game goes on with Direct3D 9.\n"
            "mgeXE.log in the folder of the game has the details.");
        return nullptr;
    }
    LOG::logline("-- wgpu renderer: the nirender plugin (%s) gives the game its Direct3D 8", buildId());
    // Before the stream starts: the plugin keeps the record and writes it after the header.
    Deviceless::sendHello();
    return create(version);
}

// The nirender plugin asks this when it loads: nonzero when the game gets its Direct3D 8
// from the plugin. The plugin then puts its renderer code on before the game makes its renderer.
extern "C" UINT _stdcall MgeRendererWgpuActive() {
    return (isMW && Configuration.RendererWgpu) ? 1 : 0;
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

extern "C" void* _stdcall FakeDirect3DCreate(UINT version) {
    // Wrap Morrowind only, not TESCS
    if (isMW && Configuration.RendererWgpu) {
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
