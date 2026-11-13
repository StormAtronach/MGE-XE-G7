#pragma once

#include "VMTYPES.H"



class MWSE_MGEPlugin {
public:
    static void init(HMODULE dll);
    // The switch to the plain game after init: MWSE gets the mge API of the plain game
    // (api::MGEAPI_Plain), whose acting methods do nothing. MWSE cannot take back the API or
    // the MWScript instructions; the instructions test Session::plain() themselves.
    static void toPlainGame();
};
