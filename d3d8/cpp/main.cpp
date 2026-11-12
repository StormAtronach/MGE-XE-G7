
#include "mge/configuration.h"
#include "mge/mgeversion.h"
#include "mge/mwinitpatch.h"
#include "mge/mged3d8device.h"
#include "mge/mwbridge.h"
#include "mge/startupgen.h"
#include "mwse/mgebridge.h"
#include "support/winheader.h"
#include "support/log.h"
#include "support/crashlog.h"

#include <cstdio>
#include <cstring>
#include <string>



extern void* CreateD3DWrapper(UINT);
extern void* CreateInputWrapper(void*);

static FARPROC getProc1(const char* lib, const char* funcname);
static void setDPIScalingAware();

static const char* welcomeMessage = MGE_BUILD_ID;
static bool isMW;
// The wgpu path was turned off for this session (wgpuPathOff). A latch of its own: mge.loadConfig
// reads the settings again and sets Configuration.RendererWgpu back to the value of the file,
// and a later Direct3DCreate8 (a restart of the renderer) must not take the wgpu path then.
static bool wgpuOff = false;

// The wgpu path is off for this session: the log line with the reason, the latch, and the
// setting of the session. The callers: DllMain, when a part of the renderer is missing, and
// the game's Direct3DCreate8, when the plugin does not give the game its Direct3D 8 (the
// proxy, no distant land). A graphics device that cannot run the renderer is not a caller:
// the helper refuses it with its own box, and the game closes (the owner's D386).
static void wgpuPathOff(const char* reason) {
    LOG::logline("!! wgpu renderer: not in use, %s. MGE XE continues with Direct3D 9", reason);
    wgpuOff = true;
    Configuration.RendererWgpu = false;
    MWInitPatch::patchDirect3D9EngineChanges();
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
                wgpuPathOff((std::string("a part is missing: ") + missing).c_str());
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

// The function of the nirender plugin that keeps the options of a view (mge.camera.setViewOptions)
// by the address of the key's object, or null.
static UINT (__cdecl * viewOptionsExport)(UINT kind, void* key, UINT parts, UINT clear) = nullptr;

// The function of the nirender plugin that reads the options of a view back, or null.
static UINT (__cdecl * viewOptionsGetExport)(UINT kind, void* key, UINT* parts) = nullptr;

// Asks the plugin for the options of a view: 1 and the parts, 0 when the key has none (also when its
// object was freed), -1 when there is no plugin to ask (the Direct3D 9 renderer).
int queryViewOptions(UINT kind, void* key, UINT* parts) {
    return viewOptionsGetExport ? static_cast<int>(viewOptionsGetExport(kind, key, parts)) : -1;
}

// Gives the plugin the options of a view: the parts, or null to clear them. False only when the
// plugin refuses the key (a key kind that it does not take). Without the plugin's function (the
// Direct3D 9 renderer) the call does nothing and gives true: the caller keeps its own copy then.
bool sendViewOptions(UINT kind, void* key, const UINT* parts) {
    if (!viewOptionsExport) {
        return true;
    }
    return viewOptionsExport(kind, key, parts ? *parts : 0, parts ? 0 : 1) != 0;
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
    viewOptionsExport = (UINT (__cdecl *)(UINT, void*, UINT, UINT))GetProcAddress(plugin, "nirender_view_options");
    viewOptionsGetExport = (UINT (__cdecl *)(UINT, void*, UINT*))GetProcAddress(plugin, "nirender_view_options_get");
    // The build id of the plugin, for the log. A plugin without the export has no handshake:
    // the game goes on with its device, and the helper refuses its stream (no stream format
    // in the HEADER) with one box and the exit code 10. No Direct3D 9 fallback:
    // the plugin's live ports, installed early, leave that frame black (the owner's D10 (a),
    // 2026-10-27, after run 4c of records entry 192).
    typedef const char* (__cdecl * BuildIdProc) ();
    BuildIdProc buildId = (BuildIdProc)GetProcAddress(plugin, "nirender_build_id");
    if (!buildId) {
        LOG::logline("!! wgpu renderer: akulakhan.dll is from before the version handshake (no nirender_build_id); this d3d8.dll is " MGE_BUILD_ID "; the helper refuses its stream");
    }
    LOG::logline("-- wgpu renderer: the nirender plugin (%s) gives the game its Direct3D 8", buildId ? buildId() : "a build from before the handshake");
    // Before the stream starts: the plugin keeps the record and writes it after the header.
    // Once per process: the game asks for its Direct3D 8 again when it makes its renderer
    // again (a change of its resolution), and the stream of the first device goes on.
    static bool helloSent = false;
    if (!helloSent) {
        helloSent = true;
        Deviceless::sendHello();
    }
    void* d3d = create(version);
    // A plugin that refuses (no ports) gives the game no device: the Direct3D 9 path must not
    // send the plugin its records then.
    if (!d3d) {
        streamExtension = nullptr;
        viewOptionsExport = nullptr;
        viewOptionsGetExport = nullptr;
    }
    return d3d;
}

// The wgpu path for this session: the game is Morrowind, the path was not turned off
// (wgpuPathOff), and the setting is on. Two ask it: the plugin when it loads
// (MgeRendererWgpuActive), and the game when it asks for its Direct3D 8. No process is started
// and nothing waits here: a graphics device that cannot run the renderer gets the helper's
// refusal at its start, with a box and the code 20 (the owner's D386).
static bool wgpuPathOn() {
    return isMW && !wgpuOff && Configuration.RendererWgpu;
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
// with the scene packet of its contract (mge_render_link.h, MgeLinkSceneV2: nirender stream
// format 10).
extern "C" void __cdecl MGE_StreamSceneBegin(MgeLinkSceneV2* scene) {
    if (isMW && Configuration.RendererWgpu && scene) {
        Deviceless::sceneBegin(scene);
    }
}

extern "C" void __cdecl MGE_StreamTransform(UINT state, float* matrix, UINT targetIsBackBuffer) {
    if (isMW && Configuration.RendererWgpu) {
        Deviceless::transform(state, reinterpret_cast<D3DMATRIX*>(matrix), targetIsBackBuffer != 0);
    }
}

// The nirender plugin calls this for the radius that the helper's fade takes: the one recovery
// from a light's attenuation, which lightAttachRadius also takes, so that the fade ends inside
// the attach radius. 0 when the attenuation gives no radius: the light has no fade.
extern "C" float __cdecl MGE_LightFadeRadius(float constant, float linear, float quadratic) {
    return MWBridge::get()->pointLightRadius(constant, linear, quadratic);
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
    if (wgpuPathOn()) {
        Configuration.EnsureSettingsFile();
        if (void* d3d = createStreamedD3D(version)) {
            return d3d;
        }
        // The plugin did not serve; a caller that said why has turned the path off already.
        if (Configuration.RendererWgpu) {
            wgpuPathOff("the plugin did not give the game its Direct3D 8 (proxy-only mode)");
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
