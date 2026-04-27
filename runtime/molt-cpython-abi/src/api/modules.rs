//! Module API — PyModule_New, PyModule_AddObject, PyModuleDef_Init.

use crate::abi_types::{PyModuleDef, PyObject};
use crate::bridge::{GLOBAL_BRIDGE, read_bridge_header_bits};
use crate::hooks;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Resolve a `*mut PyObject` produced by this bridge to its underlying Molt
/// handle bits.
///
/// All PyObject blocks our bridge mints carry the canonical Molt handle in a
/// trailing u64 (see `bridge::handle_to_pyobj`), so we can read it directly
/// without any per-bridge state.  Foreign pointers fall back to the in-memory
/// map maintained by this copy of the bridge.
fn bridge_pyobj_to_bits(obj: *mut PyObject) -> u64 {
    if obj.is_null() {
        return MoltObject::none().bits();
    }
    if let Some(bits) = GLOBAL_BRIDGE.lock().pyobj_to_handle(obj) {
        return bits;
    }
    // No singleton match and no entry in the local map — the pointer most
    // likely came from another copy of this bridge.  Read the trailing
    // handle bits directly.
    unsafe { read_bridge_header_bits(obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_New(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let h = hooks::hooks_or_stubs();
    // SAFETY: hook is initialised by molt-runtime at startup; stubs return 0 if not.
    let bits = unsafe { (h.alloc_module)(name_bytes.as_ptr(), name_bytes.len()) };
    if bits == 0 {
        return ptr::null_mut();
    }
    // Wrap the Molt handle in a bridge `PyObject` block so the returned
    // pointer survives the `*mut PyObject` narrowing (which only preserves
    // 48 bits of address) and so the trailing handle bits give the loader a
    // stateless way to recover the canonical handle even when called from a
    // different copy of this bridge crate.
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDict(module: *mut PyObject) -> *mut PyObject {
    if module.is_null() {
        return ptr::null_mut();
    }
    // CPython returns module.__dict__ (a borrowed reference).  The bridge
    // does not yet expose the module's underlying dict pointer through a
    // hook; PyModule_AddObject below uses module_set_attr directly to store
    // attributes on the module's __dict__, so callers that only use
    // PyModule_AddObject / PyModule_AddStringConstant / PyModule_AddIntConstant
    // never need to inspect the dict pointer.
    //
    // NOTE(c-extension-gap): Extensions that call PyDict_SetItemString on the
    // result will currently set attributes on the module pointer, which the
    // mapping bridge silently ignores.  Wire a dedicated `module_dict_bits`
    // hook before claiming PyDict_SetItemString-on-module support.
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
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let module_bits = bridge_pyobj_to_bits(module);
    let value_bits = bridge_pyobj_to_bits(value);
    let h = hooks::hooks_or_stubs();
    let rc = unsafe {
        (h.module_set_attr)(
            module_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
            value_bits,
        )
    };
    if rc != 0 {
        // CPython contract: on failure the caller still owns the value
        // reference — do NOT decref.
        return rc;
    }
    // Per CPython: PyModule_AddObject steals the reference on success.
    // The runtime hook took its own reference when storing the value, so we
    // must drop the caller's reference here to balance the count.
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
    // Iterate the NULL-terminated PyMethodDef array and register each method
    // as a callable Molt function via the runtime hook.  Methods whose flags
    // describe a calling convention the runtime does not yet support are
    // skipped with a diagnostic — the loader caller surfaces this as a load
    // error if the extension actually invokes the unsupported method.
    let m_methods = unsafe { (*def).m_methods };
    if !m_methods.is_null() {
        let h = hooks::hooks_or_stubs();
        let module_bits = bridge_pyobj_to_bits(module);
        let mut cursor = m_methods;
        unsafe {
            while !(*cursor).ml_name.is_null() {
                let entry = &*cursor;
                let meth_name = CStr::from_ptr(entry.ml_name).to_bytes();
                // PyMethodDef.ml_meth is `Option<unsafe extern "C" fn(...)>`
                // for CPython compatibility; a NULL slot signals end-of-table
                // (handled by the outer `ml_name.is_null()` check, but a
                // mid-table NULL would be malformed input — skip silently
                // rather than ferrying an invalid pointer through dispatch).
                let Some(fn_ptr) = entry.ml_meth else {
                    cursor = cursor.add(1);
                    continue;
                };
                let meth_addr = fn_ptr as *const () as usize as u64;
                let func_bits = (h.register_c_function)(
                    meth_addr,
                    entry.ml_flags,
                    meth_name.as_ptr(),
                    meth_name.len(),
                );
                if func_bits != 0 {
                    let rc = (h.module_set_attr)(
                        module_bits,
                        meth_name.as_ptr(),
                        meth_name.len(),
                        func_bits,
                    );
                    // Drop our reference to the callable — module_set_attr
                    // grabbed its own reference when storing into the dict.
                    (h.dec_ref)(func_bits);
                    if rc != 0 {
                        let mod_name = CStr::from_ptr(name).to_string_lossy();
                        let meth_name_str = std::str::from_utf8(meth_name).unwrap_or("?");
                        eprintln!(
                            "molt_cpython_abi: PyModule_Create2 for {mod_name:?}: \
                             failed to register method {meth_name_str:?}",
                        );
                    }
                } else {
                    let mod_name = CStr::from_ptr(name).to_string_lossy();
                    let meth_name_str = std::str::from_utf8(meth_name).unwrap_or("?");
                    eprintln!(
                        "molt_cpython_abi: PyModule_Create2 for {mod_name:?}: \
                         method {meth_name_str:?} (flags 0x{:x}) uses an unsupported \
                         calling convention; only METH_NOARGS is implemented today",
                        entry.ml_flags,
                    );
                }
                cursor = cursor.add(1);
            }
        }
    }
    module
}
