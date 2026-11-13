
#include "mge/session.h"
#include "funcgeneral.h"
#include "funchud.h"
#include "mge/mgeversion.h"
#include "mge/distantland.h"
#include "mge/userhud.h"
#include "support/log.h"

#include <string>



MWSEINSTRUCTION_DECLARE_VTABLE(mwseWipeAll)

bool mwseWipeAll::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        return true;
    }
    resetMWSEHud();
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseOutputDebugString)

bool mwseOutputDebugString::execute(mwseInstruction* _this) {
    const char* str = _this->vmPopString();
    if (Session::plain()) {
        return true;
    }
    if (!str) { return false; }

    LOG::logline("%s", str);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseOutputVersionString)

bool mwseOutputVersionString::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        return true;
    }
    LOG::logline("%s", XE_VERSION_STRING);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseGetVersion)

// GetVersion -> returns <long version>
bool mwseGetVersion::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        _this->vmPush(VMREGTYPE(0));
        return true;
    }
    VMREGTYPE ret = MGE_MWSE_VERSION;
    return _this->vmPush(ret);
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseGetScreenWidth)

// GetScreenWidth -> returns <short width>
bool mwseGetScreenWidth::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        _this->vmPush(VMREGTYPE(0));
        return true;
    }
    VMREGTYPE ret = MGEhud::getScreenWidth();
    return _this->vmPush(ret);
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseGetScreenHeight)

// GetScreenHeight -> returns <short height>
bool mwseGetScreenHeight::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        _this->vmPush(VMREGTYPE(0));
        return true;
    }
    VMREGTYPE ret = MGEhud::getScreenHeight();
    return _this->vmPush(ret);
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseGetEyeVec)

// GetEyeVec -> returns <float x, float y, float z>
bool mwseGetEyeVec::execute(mwseInstruction* _this) {
    if (Session::plain()) {
        _this->vmPush(VMFLOAT(0));
        _this->vmPush(VMFLOAT(0));
        _this->vmPush(VMFLOAT(0));
        return true;
    }
    _this->vmPush(DistantLand::eyeVec.z);
    _this->vmPush(DistantLand::eyeVec.y);
    _this->vmPush(DistantLand::eyeVec.x);
    return true;
}
