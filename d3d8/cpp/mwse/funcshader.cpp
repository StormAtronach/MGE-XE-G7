
#include "mge/session.h"
#include "funcshader.h"
#include "mge/postshaders.h"

#include <string>



MWSEINSTRUCTION_DECLARE_VTABLE(mwseSetShaderLong)

bool mwseSetShaderLong::execute(mwseInstruction* _this) {
    const char* shader = _this->vmPopString();
    if (!shader) { return false; }

    const char* varName = _this->vmPopString();
    if (!varName) { return false; }

    VMREGTYPE x;
    if (!_this->vmPop(&x)) { return false; }
    if (Session::plain()) {
        return true;
    }

    PostShaders::setShaderVar(shader, varName, (int)x);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseSetShaderFloat)

bool mwseSetShaderFloat::execute(mwseInstruction* _this) {
    const char* shader = _this->vmPopString();
    if (!shader) { return false; }

    const char* varName = _this->vmPopString();
    if (!varName) { return false; }

    VMFLOAT x;
    if (!_this->vmPop(&x)) { return false; }
    if (Session::plain()) {
        return true;
    }

    PostShaders::setShaderVar(shader, varName, x);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseSetShaderVector)

bool mwseSetShaderVector::execute(mwseInstruction* _this) {
    const char* shader = _this->vmPopString();
    if (!shader) { return false; }

    const char* varName = _this->vmPopString();
    if (!varName) { return false; }

    VMFLOAT v[4];
    if (!_this->vmPop(&v[0])) { return false; }
    if (!_this->vmPop(&v[1])) { return false; }
    if (!_this->vmPop(&v[2])) { return false; }
    if (!_this->vmPop(&v[3])) { return false; }
    if (Session::plain()) {
        return true;
    }

    PostShaders::setShaderVar(shader, varName, v);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseEnableShader)

bool mwseEnableShader::execute(mwseInstruction* _this) {
    const char* shader = _this->vmPopString();
    if (Session::plain()) {
        return true;
    }
    if (!shader) { return false; }

    PostShaders::setShaderEnable(shader, true);
    return true;
}


MWSEINSTRUCTION_DECLARE_VTABLE(mwseDisableShader)

bool mwseDisableShader::execute(mwseInstruction* _this) {
    const char* shader = _this->vmPopString();
    if (Session::plain()) {
        return true;
    }
    if (!shader) { return false; }

    PostShaders::setShaderEnable(shader, false);
    return true;
}
