
#include "mge/configuration.h"
#include "mge/mgeversion.h"
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

static const char* welcomeMessage = XE_VERSION_STRING;
static bool isMW;



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
            static const char* const parts[] = {
                "mgeHost64.exe",
                "MWSE.dll",
                "Data Files\\MWSE\\lib\\nirender.dll",
                "Data Files\\MWSE\\mods\\nirender\\main.lua",
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
                LOG::logline("!! wgpu renderer: not in use, a part is missing: %s. MGE XE continues with Direct3D 9", missing);
                Configuration.RendererWgpu = false;
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
// sends the draws of the game as a stream, and mgeHost64.exe draws the stream with wgpu.
// Returns null when the plugin is not there.
// The function of the nirender plugin that puts data of MGE XE into its stream, or null.
static void (__cdecl * streamExtension)(UINT id, const void* data, UINT length) = nullptr;

// Sends data to the wgpu renderer in the helper. It arrives with the frame in progress.
void sendToRenderer(UINT id, const void* data, UINT length) {
    if (streamExtension) {
        streamExtension(id, data, length);
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
    SetEnvironmentVariableA("NIRENDER_HOST_EXE", (folder + "mgeHost64.exe").c_str());

    HMODULE plugin = LoadLibraryA((folder + "Data Files\\MWSE\\lib\\nirender.dll").c_str());
    if (!plugin) {
        LOG::logline("!! wgpu renderer: Data Files\\MWSE\\lib\\nirender.dll did not load");
        return nullptr;
    }
    typedef void* (__cdecl * CreateProc) (UINT);
    CreateProc create = (CreateProc)GetProcAddress(plugin, "nirender_direct3d_create8");
    if (!create) {
        LOG::logline("!! wgpu renderer: the nirender plugin has no nirender_direct3d_create8");
        return nullptr;
    }
    streamExtension = (void (__cdecl *)(UINT, const void*, UINT))GetProcAddress(plugin, "nirender_stream_extension");
    LOG::logline("-- wgpu renderer: the nirender plugin gives the game its Direct3D 8");
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

extern "C" void __cdecl MGE_StreamTransform(UINT state, float* matrix, UINT targetIsBackBuffer) {
    if (isMW && Configuration.RendererWgpu) {
        Deviceless::transform(state, reinterpret_cast<D3DMATRIX*>(matrix), targetIsBackBuffer != 0);
    }
}

extern "C" void* _stdcall FakeDirect3DCreate(UINT version) {
    // Wrap Morrowind only, not TESCS
    if (isMW && Configuration.RendererWgpu) {
        Configuration.EnsureSettingsFile();
        if (void* d3d = createStreamedD3D(version)) {
            return d3d;
        }
        LOG::logline("!! wgpu renderer: not available, MGE XE continues with Direct3D 9 in proxy-only mode");
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
