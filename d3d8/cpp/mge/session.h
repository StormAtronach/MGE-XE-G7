#pragma once

// What runs in this session of the game: the new renderer, Direct3D 9, or the plain game.
// The settings (Configuration) say what the player wants; this says what runs. Only DllMain
// and the game's Direct3DCreate8 set it, so mge.loadConfig, which reads the settings file
// again, cannot turn a part back on.
//
// The plain game is the game as without MGE XE: its own renderer on Windows' Direct3D 8, the
// input not wrapped, nothing of MGE XE in the game's code and no request of MGE XE at MWSE.
// d3d8.dll only loads MWSE, forwards Direct3D 8 and the input to Windows, and writes its logs.
class Session {
public:
    enum class Path {
        // Not Morrowind (the Construction Set), or DllMain has not decided yet.
        Undecided,
        Wgpu,
        Direct3D9,
        Plain,
    };

    // Why a session is the plain game. Each automatic case has a message box; the player's
    // choice (Disabled) and a session without settings have none.
    enum class Why {
        NotConfigured,
        Disabled,
        PartMissing,
        MwseDisabled,
        NoNativeAbi,
        EventRefused,
        // The plugin is missing, does not load, or has no create.
        PluginFailed,
        // The plugin refused before it wrote into the game's code; the detail is its cause.
        PluginRefused,
    };

    // The game's exit code when the plugin refused after it changed the game's code: the
    // table of the helper's codes in nirender src/stream_format.rs (EXIT_PLUGIN_REFUSED).
    static const unsigned ExitPluginRefused = 12;

    static Path path();
    static bool wgpu();
    static bool direct3D9();
    static bool plain();

    // Sets the path when DllMain chose the new renderer or Direct3D 9.
    static void choose(Path path);

    // The one switch to the plain game. It logs the reason, gives MWSE back what d3d8.dll
    // took (MwseNative::giveBack), makes the mge API do nothing (MWSE_MGEPlugin::toPlainGame),
    // and keeps the text of the box for the game's first Direct3DCreate8. `detail` names the
    // part, the event or the plugin's failure; it may be null.
    static void toPlainGame(Why why, const char* detail);

    // Shows the box of a switch once, at the game's first call for its Direct3D 8: not in
    // DllMain, which runs under the loader lock.
    static void showPendingBox();

    // The plugin refused after it changed the game's code: the box, then the game closes with
    // ExitPluginRefused.
    [[noreturn]] static void closeAfterPluginRefusal(const char* detail);
};
