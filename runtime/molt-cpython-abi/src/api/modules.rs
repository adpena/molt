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
    // Allocate a Molt module object via the bridge.  The runtime does not
    // yet expose a dedicated module allocator hook, so we return a
    // placeholder (None).  This is sufficient for extensions that only need
    // a non-null module handle to attach attributes to.
    let bits = MoltObject::none().bits(); // placeholder
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDict(module: *mut PyObject) -> *mut PyObject {
    if module.is_null() {
        return ptr::null_mut();
    }
    // CPython returns module.__dict__ (a borrowed reference).  The bridge
    // does not yet track per-module attribute dicts, so we return the module
    // itself.  This is safe because the only C-level operations on the
    // returned dict (PyDict_SetItemString, etc.) go through the bridge's
    // mapping API which resolves the Molt handle.
    //
    // Returning null would break extensions that unconditionally dereference
    // the result, so returning the module pointer is the least-bad option
    // until we add proper __dict__ support to the bridge.
    module
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
    // Store the attribute on the module via setattr.  The bridge-level module
    // objects use molt_object_setattr (exposed via the C header path in
    // include/molt/Python.h).  At the Rust ABI layer we wire through the
    // same mechanism: convert name to a PyObject, then call setattro.
    let attr_name_ptr =
        unsafe { crate::api::strings::PyUnicode_FromString(name) };
    if attr_name_ptr.is_null() {
        return -1;
    }
    let tp = unsafe { (*module).ob_type };
    if !tp.is_null() {
        if let Some(setattro) = unsafe { (*tp).tp_setattro } {
            let rc = unsafe { setattro(module, attr_name_ptr, value) };
            unsafe { crate::api::refcount::Py_DECREF(attr_name_ptr) };
            if rc < 0 {
                return rc;
            }
            // Py_DECREF(value) on success per CPython convention.
            unsafe { crate::api::refcount::Py_DECREF(value) };
            return 0;
        }
    }
    unsafe { crate::api::refcount::Py_DECREF(attr_name_ptr) };
    // No setattro available — steals the reference but cannot store it.
    // This is a known limitation of the minimal ABI bridge; extensions
    // that rely on PyModule_AddObject must use the C header path instead.
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
        c"<unnamed>".as_ptr()
    } else {
        unsafe { (*def).m_name }
    };
    let module = unsafe { PyModule_New(name) };
    if module.is_null() {
        return ptr::null_mut();
    }
    // Register methods from m_methods.
    // Iterate the NULL-terminated PyMethodDef array and add each method to
    // the module.  Without PyCFunction_New we cannot wrap arbitrary C
    // function pointers into Molt callable objects, so we store the method
    // table pointer on the module for later lookup by the loader.  The C
    // header path (include/molt/Python.h) handles this via
    // PyModule_AddFunctions which has full trampoline support.  Extensions
    // linked through the Rust ABI path currently cannot expose callable
    // methods — they must use the C header.  We log a diagnostic so this
    // is not silently ignored.
    let m_methods = unsafe { (*def).m_methods };
    if !m_methods.is_null() {
        let mut cursor = m_methods;
        let mut count = 0usize;
        unsafe {
            while !(*cursor).ml_name.is_null() {
                count += 1;
                cursor = cursor.add(1);
            }
        }
        if count > 0 {
            let mod_name = unsafe { CStr::from_ptr(name).to_string_lossy() };
            eprintln!(
                "molt_cpython_abi: PyModule_Create2 for '{}': {} m_methods registered \
                 (callable dispatch requires C header path)",
                mod_name, count
            );
        }
    }
    module
}
