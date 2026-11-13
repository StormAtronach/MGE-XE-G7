
#include "mge/configuration.h"
#include "mge/mgeversion.h"
#include "mge/mwinitpatch.h"
#include "mge/mged3d8device.h"
#include "mge/mwbridge.h"
#include "mge/session.h"
#include "mge/startupgen.h"
#include "mwse/mgebridge.h"
#include "mwse/mwsenative.h"
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

// Loads MWSE.dll, which injects by itself, unless the setting runtime.mwse_disabled is on.
// With `link`, the MWSE-MGE integration (the mge Lua API and MGE XE's MWScript instructions)
// is set up. Returns MWSE's module, or null.
static HMODULE loadMwse(bool link) {
    if (Configuration.MGEFlags & MWSE_DISABLED) {
        LOG::logline("MWSE is disabled.");
        return NULL;
    }
    HMODULE dll = LoadLibraryA("MWSE.dll");
    if (!dll) {
        LOG::logline("MWSE.dll failed to load.");
        return NULL;
    }
    if (link) {
        MWSE_MGEPlugin::init(dll);
    }
    LOG::logline("MWSE.dll injected.");
    return dll;
}

// Direct3D 9 (renderer_wgpu = false): the start as before the new renderer, a frozen path.
static void startDirect3D9() {
    Session::choose(Session::Path::Direct3D9);
    // Early startup patches
    MWInitPatch::patch();
    loadMwse(!Configuration.OnlyProxyD3D8To9);
}

// The new renderer. It needs the helper program, which draws, and MWSE with its native ABI
// and the two events of the renderer. When one of them is not there, the session is the
// plain game. Nothing of MGE XE goes into the game's code, and nothing is asked of MWSE,
// before these tests have passed.
static void startWgpu() {
    // Only the files in the folder of the game are examined. This code runs when the game
    // loads d3d8.dll. At that time a mod manager with a virtual file system (Mod Organizer)
    // does not show the files of its mods: a file below `Data Files` that is there looks
    // missing. The plugin below `Data Files` is examined later, when the game asks for its
    // Direct3D 8.
    static const char* const parts[] = {
        "numidium.exe",
        "MWSE.dll",
    };
    for (const char* part : parts) {
        if (GetFileAttributesA(part) == INVALID_FILE_ATTRIBUTES) {
            Session::toPlainGame(Session::Why::PartMissing, part);
            loadMwse(false);
            return;
        }
    }
    if (Configuration.MGEFlags & MWSE_DISABLED) {
        Session::toPlainGame(Session::Why::MwseDisabled, nullptr);
        loadMwse(false);
        return;
    }

    HMODULE dll = loadMwse(false);
    if (!dll) {
        Session::toPlainGame(Session::Why::PartMissing, "MWSE.dll (it is there, but it did not load)");
        return;
    }
    unsigned askedVersion = 0;
    if (!MwseNative::init(dll, &askedVersion)) {
        const std::string version = std::to_string(askedVersion);
        Session::toPlainGame(Session::Why::NoNativeAbi, version.c_str());
        return;
    }
    if (const char* refused = Deviceless::listenNeededEvents()) {
        Session::toPlainGame(Session::Why::EventRefused, refused);
        return;
    }

    Session::choose(Session::Path::Wgpu);
    // The wgpu renderer of the helper draws the picture, and no Direct3D device exists. The
    // parts of MGE XE that need its device stay off, as in proxy-only mode.
    LOG::logline(">> Using the wgpu renderer: no Direct3D device");
    Configuration.OnlyProxyD3D8To9 = true;
    MWInitPatch::patchWgpuStart();
    // MWSE-MGE integration
    MWSE_MGEPlugin::init(dll);
    // MWSE applies MGE XE's early engine changes on the wgpu path.
    MWInitPatch::patchWgpu();
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
    if (GetModuleHandle("Morrowind.exe")) {
        LOG::open("mgeXE.log");
        LOG::logline(welcomeMessage);
        CrashLog::install();

        setDPIScalingAware();

        if (!Configuration.LoadSettings()) {
            LOG::logline("Error: MGE XE is not configured. MGE XE will be disabled for this session.");
            // As before: this session does not load MWSE.
            Session::toPlainGame(Session::Why::NotConfigured, nullptr);
            return true;
        }

        if (Configuration.MGEFlags & MGE_DISABLED) {
            // The player's choice: the plain game, with MWSE, and nothing of MGE XE in the
            // game's code.
            Session::toPlainGame(Session::Why::Disabled, nullptr);
            loadMwse(false);
        } else if (Configuration.RendererWgpu) {
            startWgpu();
        } else {
            startDirect3D9();
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

    // Nothing of the plugin ran when it is missing, does not load, or has no create: the
    // plain game (case d1).
    HMODULE plugin = LoadLibraryA((folder + "Data Files\\MWSE\\lib\\akulakhan.dll").c_str());
    if (!plugin) {
        Session::toPlainGame(Session::Why::PluginFailed, "Data Files\\MWSE\\lib\\akulakhan.dll did not load");
        return nullptr;
    }
    typedef void* (__cdecl * CreateProc) (UINT);
    CreateProc create = (CreateProc)GetProcAddress(plugin, "nirender_direct3d_create8");
    if (!create) {
        Session::toPlainGame(Session::Why::PluginFailed, "the nirender plugin has no nirender_direct3d_create8");
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
    if (d3d) {
        return d3d;
    }
    // The plugin refused: nothing may send it records. When it changed the game's code first
    // (its detours stay), the game closes (case d2); a plugin without nirender_refused is
    // from before that answer and counts as such. Else nothing of it is in the game's code,
    // and the session is the plain game (case d1).
    streamExtension = nullptr;
    viewOptionsExport = nullptr;
    viewOptionsGetExport = nullptr;
    typedef const char* (__cdecl * RefusalProc) (UINT*);
    RefusalProc refusal = (RefusalProc)GetProcAddress(plugin, "nirender_refusal");
    if (!refusal) {
        Session::closeAfterPluginRefusal("the plugin gave no Direct3D 8 and has no nirender_refusal");
    }
    UINT codeChanged = 1;
    const char* cause = refusal(&codeChanged);
    if (!cause) {
        cause = "the plugin gave no Direct3D 8 and names no refusal";
    }
    if (codeChanged != 0) {
        Session::closeAfterPluginRefusal(cause);
    }
    Session::toPlainGame(Session::Why::PluginRefused, cause);
    return nullptr;
}

// The nirender plugin asks this when it loads: nonzero when the game gets its Direct3D 8
// from the plugin (Session::wgpu, decided in DllMain). The plugin then puts its renderer code
// on before the game makes its renderer. No process is started and nothing waits here: a
// graphics device that cannot run the renderer gets the helper's refusal at its start, with a
// box and the code 20 (the owner's D386).
extern "C" UINT _stdcall MgeRendererWgpuActive() {
    return Session::wgpu() ? 1 : 0;
}

// The nirender plugin calls these two at the points of a frame where the proxy device ran
// the logic of MGE XE that is not drawing. See Deviceless in mge/mged3d8device.cpp.
extern "C" void __cdecl MGE_StreamPresent(UINT width, UINT height) {
    if (Session::wgpu()) {
        Deviceless::present(width, height);
    }
}

// The nirender plugin calls this at the start of each camera's Click, before its transforms,
// with the scene packet of its contract (mge_render_link.h, MgeLinkSceneV2: nirender stream
// format 10).
extern "C" void __cdecl MGE_StreamSceneBegin(MgeLinkSceneV2* scene) {
    if (Session::wgpu() && scene) {
        Deviceless::sceneBegin(scene);
    }
}

extern "C" void __cdecl MGE_StreamTransform(UINT state, float* matrix, UINT targetIsBackBuffer) {
    if (Session::wgpu()) {
        Deviceless::transform(state, reinterpret_cast<D3DMATRIX*>(matrix), targetIsBackBuffer != 0);
    }
}

// The nirender plugin calls this for the radius that the helper's fade takes: the one recovery
// from a light's attenuation, which lightAttachRadius also takes, so that the fade ends inside
// the attach radius. 0 when the attenuation gives no radius: the light has no fade.
extern "C" float __cdecl MGE_LightFadeRadius(float constant, float linear, float quadratic) {
    if (!Session::wgpu()) {
        return 0.0f;
    }
    return MWBridge::get()->pointLightRadius(constant, linear, quadratic);
}

// The nirender plugin calls this for each message of the extension in the helper, at the end
// of a frame of the game. See Deviceless::message.
extern "C" void __cdecl MGE_StreamMessage(UINT id, const void* data, UINT length) {
    if (Session::wgpu() && data) {
        Deviceless::message(id, data, length);
    }
}

extern "C" void* _stdcall FakeDirect3DCreate(UINT version) {
    // The box of a switch to the plain game in DllMain.
    Session::showPendingBox();
    if (Session::wgpu()) {
        Configuration.EnsureSettingsFile();
        if (void* d3d = createStreamedD3D(version)) {
            return d3d;
        }
        // The plugin did not serve: createStreamedD3D switched to the plain game.
        Session::showPendingBox();
    }
    // Wrap Morrowind only, not TESCS
    if (Session::direct3D9()) {
        Configuration.EnsureSettingsFile();
        StartupGeneration::launchEarlyHost(true);
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
        // The plain game's input is not wrapped.
        if (Session::wgpu() || Session::direct3D9()) {
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
