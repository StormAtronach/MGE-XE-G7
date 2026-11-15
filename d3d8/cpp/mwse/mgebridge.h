#pragma once

#include "VMTYPES.H"



class MWSE_MGEPlugin {
public:
    static void init(HMODULE dll);
    // The switch to the plain game after init: MGE XE withdraws its mge API from MWSE (the
    // request MWSE_NATIVE_REQUEST_MGE_API), and MWSE's own API that does nothing takes its place.
    // MWSE cannot take back the MWScript instructions; they test Session::plain() themselves.
    static void toPlainGame();
};
