//! Module API — PyModule_New, PyModule_AddObject, PyModuleDef_Init.

use crate::abi_types::{PyModuleDef, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_New(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let _name = unsafe { CStr::from_ptr(name).to_string_lossy() };
    // Allocate a Molt module object and wrap it.
    // TODO: call molt runtime module allocator via c_api bridge.
    // For now return a placeholder non-null value so extensions don't abort.
    let bits = MoltObject::none().bits(); // placeholder
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDict(module: *mut PyObject) -> *mut PyObject {
    if module.is_null() {
        return ptr::null_mut();
    }
    // Return module's __dict__. For bridge modules, create a wrapper dict.
    module // placeholder — real impl returns the module's attribute dict
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddObject(
    module: *mut PyObject,
    name: *const c_char,
    value: *mut PyObject,
) -> c_int {
    if module.is_null() || name.is_null() || value.is_null() {
        return -1;
    }
    let _name = unsafe { CStr::from_ptr(name).to_string_lossy() };
    // TODO: set attribute on the Molt module object.
    // Py_DECREF(value) on success per CPython convention.
    unsafe { crate::api::refcount::Py_DECREF(value) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddIntConstant(
    module: *mut PyObject,
    name: *const c_char,
    value: i64,
) -> c_int {
    let obj = unsafe { crate::api::numbers::PyLong_FromLongLong(value) };
    unsafe { PyModule_AddObject(module, name, obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddStringConstant(
    module: *mut PyObject,
    name: *const c_char,
    value: *const c_char,
) -> c_int {
    let obj = unsafe { crate::api::strings::PyUnicode_FromString(value) };
    unsafe { PyModule_AddObject(module, name, obj) }
}

/// Multi-phase init entry point. Called by `PyInit_<name>()` in extensions
/// that use PEP 451 multi-phase init (most modern extensions).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModuleDef_Init(def: *mut PyModuleDef) -> *mut PyObject {
    if def.is_null() {
        return ptr::null_mut();
    }
    // Single-phase compatibility: call the m_init function if present.
    unsafe {
        let init = (*def).m_base.m_init;
        if let Some(f) = init {
            f()
        } else {
            // Multi-phase: return the def itself cast as a pseudo-module
            // (caller will call PyModule_FromDefAndSpec2).
            def.cast()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_Create2(
    def: *mut PyModuleDef,
    _module_api_version: c_int,
) -> *mut PyObject {
    if def.is_null() {
        return ptr::null_mut();
    }
    let name = if unsafe { (*def).m_name.is_null() } {
        b"<unnamed>\0".as_ptr().cast()
    } else {
        unsafe { (*def).m_name }
    };
    let module = unsafe { PyModule_New(name) };
    // Register methods from m_methods.
    // TODO: iterate PyMethodDef array and add each method to the module dict.
    module
}
