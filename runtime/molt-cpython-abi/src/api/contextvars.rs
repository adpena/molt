//! Public Context C API. The runtime owns every object, binding and token.
//! A C view is the bridge's canonical projection, never a second semantic map.
use crate::abi_types::{PyObject, PyTypeObject};
use crate::bridge::{GLOBAL_BRIDGE, RuntimeValue};
use crate::hooks::{DecodedHandleResult, RuntimeGilGuard, hooks_or_stubs};
use std::os::raw::{c_char, c_int};
use std::ptr;
unsafe fn input(value: *mut PyObject) -> Option<RuntimeValue> {
    unsafe { RuntimeValue::acquire_edge(value) }
}
fn status_error() -> c_int {
    crate::api::errors::transfer_runtime_pending_to_current();
    -1
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_New() -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj((hooks_or_stubs().context_new)()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_CopyCurrent() -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj((hooks_or_stubs().context_copy_current)()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_Copy(ctx: *mut PyObject) -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    let Some(ctx) = (unsafe { input(ctx) }) else {
        return ptr::null_mut();
    };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj((hooks_or_stubs().context_copy)(ctx.bits())) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_Enter(ctx: *mut PyObject) -> c_int {
    let _gil = RuntimeGilGuard::ensure();
    let Some(ctx) = (unsafe { input(ctx) }) else {
        return -1;
    };
    let status = unsafe { (hooks_or_stubs().context_enter)(ctx.bits()) };
    if status < 0 { status_error() } else { status }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_Exit(ctx: *mut PyObject) -> c_int {
    let _gil = RuntimeGilGuard::ensure();
    let Some(ctx) = (unsafe { input(ctx) }) else {
        return -1;
    };
    let status = unsafe { (hooks_or_stubs().context_exit)(ctx.bits()) };
    if status < 0 { status_error() } else { status }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_New(
    name: *const c_char,
    default: *mut PyObject,
) -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    if name.is_null() {
        unsafe {
            crate::api::errors::PyErr_BadInternalCall();
        }
        return ptr::null_mut();
    }
    let name = unsafe { crate::api::strings::PyUnicode_FromString(name) };
    if name.is_null() {
        return ptr::null_mut();
    }
    let Some(name_value) = (unsafe { input(name) }) else {
        unsafe {
            crate::api::errors::release_preserving_error(&[name]);
        }
        return ptr::null_mut();
    };
    let default_value = if default.is_null() {
        None
    } else {
        let Some(value) = (unsafe { input(default) }) else {
            unsafe {
                crate::api::errors::release_preserving_error(&[name]);
            }
            return ptr::null_mut();
        };
        Some(value)
    };
    let result = unsafe {
        (hooks_or_stubs().context_var_new)(
            name_value.bits(),
            default_value.as_ref().map_or(0, |v| v.bits()),
            c_int::from(default_value.is_some()),
        )
    };
    let out = unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
    unsafe {
        crate::api::errors::release_preserving_error(&[name]);
    }
    out
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Get(
    var: *mut PyObject,
    default: *mut PyObject,
    value: *mut *mut PyObject,
) -> c_int {
    let _gil = RuntimeGilGuard::ensure();
    if value.is_null() {
        unsafe {
            crate::api::errors::PyErr_BadInternalCall();
        }
        return -1;
    }
    unsafe {
        *value = ptr::null_mut();
    }
    let Some(var) = (unsafe { input(var) }) else {
        return -1;
    };
    let default = if default.is_null() {
        None
    } else {
        let Some(v) = (unsafe { input(default) }) else {
            return -1;
        };
        Some(v)
    };
    let result = unsafe {
        (hooks_or_stubs().context_var_get)(
            var.bits(),
            default.as_ref().map_or(0, |v| v.bits()),
            c_int::from(default.is_some()),
        )
    };
    if matches!(result.decode(), DecodedHandleResult::Missing) {
        return 0;
    }
    let out = unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
    if out.is_null() {
        return -1;
    }
    unsafe {
        *value = out;
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Set(
    var: *mut PyObject,
    value: *mut PyObject,
) -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    let Some(var) = (unsafe { input(var) }) else {
        return ptr::null_mut();
    };
    let Some(value) = (unsafe { input(value) }) else {
        return ptr::null_mut();
    };
    unsafe {
        GLOBAL_BRIDGE
            .owned_result_to_pyobj((hooks_or_stubs().context_var_set)(var.bits(), value.bits()))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Reset(var: *mut PyObject, token: *mut PyObject) -> c_int {
    let _gil = RuntimeGilGuard::ensure();
    let Some(var) = (unsafe { input(var) }) else {
        return -1;
    };
    let Some(token) = (unsafe { input(token) }) else {
        return -1;
    };
    let status = unsafe { (hooks_or_stubs().context_var_reset)(var.bits(), token.bits()) };
    if status < 0 { status_error() } else { status }
}
unsafe fn exact(value: *mut PyObject, ty: *mut PyTypeObject) -> c_int {
    // Public CheckExact follows Py_TYPE identity. The generic bridge carrier
    // describes physical storage and is not the Context object's Python class.
    unsafe { crate::bridge::is_exact_semantic_type(value, ty) as c_int }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContext_CheckExact(value: *mut PyObject) -> c_int {
    unsafe { exact(value, &raw mut crate::abi_types::PyContext_Type) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_CheckExact(value: *mut PyObject) -> c_int {
    unsafe { exact(value, &raw mut crate::abi_types::PyContextVar_Type) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextToken_CheckExact(value: *mut PyObject) -> c_int {
    unsafe { exact(value, &raw mut crate::abi_types::PyContextToken_Type) }
}
