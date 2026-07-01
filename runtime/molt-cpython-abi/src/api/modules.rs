//! Module API — PyModule_New, PyModule_AddObject, PyModuleDef_Init.

use crate::abi_types::{PyModuleDef, PyObject};
use crate::bridge::{GLOBAL_BRIDGE, read_bridge_header_bits};
use crate::hooks;
use molt_lang_obj_model::MoltObject;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_long};
use std::ptr;

const PY_MOD_CREATE: c_int = 1;
const PY_MOD_EXEC: c_int = 2;
const PY_MOD_MULTIPLE_INTERPRETERS: c_int = 3;
const PY_MOD_GIL: c_int = 4;

fn set_module_system_error(message: impl AsRef<str>) {
    let message = CString::new(message.as_ref())
        .unwrap_or_else(|_| CString::new("module API error").expect("static string has no nul"));
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_SystemError,
            message.as_ptr(),
        );
    }
}

fn set_module_system_error_if_clear(message: impl AsRef<str>) {
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        set_module_system_error(message);
    }
}

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
pub unsafe extern "C" fn PyModule_NewObject(name: *mut PyObject) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
    unsafe { PyModule_New(name_ptr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_Check(module: *mut PyObject) -> c_int {
    if module.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*module).ob_type };
    if std::ptr::eq(ob_type, &raw mut crate::abi_types::PyModule_Type) {
        return 1;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(module).is_some() as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Module_SetGIL(_module: *mut PyObject, _gil: c_int) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetName(module: *mut PyObject) -> *const c_char {
    if module.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyModule_GetName called with NULL".as_ptr(),
            );
        }
        return ptr::null();
    }
    c"molt.module".as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDict(module: *mut PyObject) -> *mut PyObject {
    if module.is_null() {
        return ptr::null_mut();
    }
    let module_bits = bridge_pyobj_to_bits(module);
    let h = hooks::hooks_or_stubs();
    let dict_bits = unsafe { (h.module_get_dict)(module_bits) };
    if dict_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(dict_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetState(module: *mut PyObject) -> *mut std::ffi::c_void {
    if module.is_null() {
        return ptr::null_mut();
    }
    let module_bits = bridge_pyobj_to_bits(module);
    let h = hooks::hooks_or_stubs();
    unsafe { (h.module_capi_get_state)(module_bits).cast() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_AddModule(module: *mut PyObject, def: *mut PyModuleDef) -> c_int {
    if module.is_null() || def.is_null() {
        return -1;
    }
    let module_bits = bridge_pyobj_to_bits(module);
    let h = hooks::hooks_or_stubs();
    unsafe { (h.module_state_add)(module_bits, def as usize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_FindModule(def: *mut PyModuleDef) -> *mut PyObject {
    if def.is_null() {
        return ptr::null_mut();
    }
    let h = hooks::hooks_or_stubs();
    let module_bits = unsafe { (h.module_state_find)(def as usize) };
    if module_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(module_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_RemoveModule(def: *mut PyModuleDef) -> c_int {
    if def.is_null() {
        return -1;
    }
    let h = hooks::hooks_or_stubs();
    unsafe { (h.module_state_remove)(def as usize) }
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
pub unsafe extern "C" fn PyModule_AddObjectRef(
    module: *mut PyObject,
    name: *const c_char,
    value: *mut PyObject,
) -> c_int {
    if value.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyModule_AddObjectRef value must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    unsafe { crate::api::refcount::Py_INCREF(value) };
    let rc = unsafe { PyModule_AddObject(module, name, value) };
    if rc != 0 {
        unsafe { crate::api::refcount::Py_DECREF(value) };
    }
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddIntConstant(
    module: *mut PyObject,
    name: *const c_char,
    value: c_long,
) -> c_int {
    let obj = unsafe { crate::api::numbers::PyLong_FromLongLong(value as i64) };
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
    unsafe {
        (*def).m_base.ob_base.ob_refcnt = 1;
        (*def).m_base.ob_base.ob_type = &raw mut crate::abi_types::PyModuleDef_Type;
        def.cast()
    }
}

unsafe fn module_state_size(def: *mut PyModuleDef) -> u64 {
    let raw = unsafe { (*def).m_size };
    if raw <= 0 { 0 } else { raw as u64 }
}

unsafe fn register_module_capi(module: *mut PyObject, def: *mut PyModuleDef) -> c_int {
    if module.is_null() || def.is_null() {
        return -1;
    }
    let module_bits = bridge_pyobj_to_bits(module);
    let h = hooks::hooks_or_stubs();
    let rc = unsafe { (h.module_capi_register)(module_bits, def as usize, module_state_size(def)) };
    if rc != 0 {
        set_module_system_error_if_clear("module C-API metadata registration failed");
    }
    rc
}

unsafe fn module_from_def_and_slots(
    def: *mut PyModuleDef,
    module_api_version: c_int,
    spec: *mut PyObject,
) -> *mut PyObject {
    let slots = unsafe { (*def).m_slots };
    if slots.is_null() {
        return unsafe { PyModule_Create2(def, module_api_version) };
    }

    let mut module = ptr::null_mut();
    let mut module_capi_registered = false;
    let mut cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            let slot = &*cursor;
            if slot.slot == PY_MOD_CREATE {
                if slot.value.is_null() {
                    set_module_system_error("Py_mod_create slot is NULL");
                    return ptr::null_mut();
                }
                type CreateFn = unsafe extern "C" fn(
                    spec: *mut PyObject,
                    def: *mut PyModuleDef,
                ) -> *mut PyObject;
                let create: CreateFn = std::mem::transmute(slot.value);
                module = create(spec, def);
                if module.is_null() {
                    set_module_system_error_if_clear(
                        "Py_mod_create slot returned NULL without setting an exception",
                    );
                    return ptr::null_mut();
                }
                break;
            }
            cursor = cursor.add(1);
        }
    }

    if module.is_null() {
        module = unsafe { PyModule_Create2(def, module_api_version) };
        if module.is_null() {
            set_module_system_error_if_clear("PyModule_Create2 failed during PyModuleDef_Init");
            return ptr::null_mut();
        }
        module_capi_registered = true;
    }
    if !module_capi_registered && unsafe { register_module_capi(module, def) } != 0 {
        unsafe { crate::api::refcount::Py_DECREF(module) };
        return ptr::null_mut();
    }

    cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            let slot = &*cursor;
            match slot.slot {
                PY_MOD_CREATE => {}
                PY_MOD_EXEC => {
                    if slot.value.is_null() {
                        set_module_system_error("Py_mod_exec slot is NULL");
                        crate::api::refcount::Py_DECREF(module);
                        return ptr::null_mut();
                    }
                    type ExecFn = unsafe extern "C" fn(module: *mut PyObject) -> c_int;
                    let exec: ExecFn = std::mem::transmute(slot.value);
                    if exec(module) != 0 {
                        set_module_system_error_if_clear(
                            "Py_mod_exec slot returned non-zero without setting an exception",
                        );
                        crate::api::refcount::Py_DECREF(module);
                        return ptr::null_mut();
                    }
                }
                PY_MOD_MULTIPLE_INTERPRETERS | PY_MOD_GIL => {}
                _ => {
                    set_module_system_error(format!("unsupported PyModuleDef slot {}", slot.slot));
                    crate::api::refcount::Py_DECREF(module);
                    return ptr::null_mut();
                }
            }
            cursor = cursor.add(1);
        }
    }

    module
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_FromDefAndSpec2(
    def: *mut PyModuleDef,
    spec: *mut PyObject,
    module_api_version: c_int,
) -> *mut PyObject {
    if def.is_null() {
        return ptr::null_mut();
    }
    unsafe { module_from_def_and_slots(def, module_api_version, spec) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_FromDefAndSpec(
    def: *mut PyModuleDef,
    spec: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyModule_FromDefAndSpec2(def, spec, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_ExecDef(module: *mut PyObject, def: *mut PyModuleDef) -> c_int {
    if module.is_null() || def.is_null() {
        return -1;
    }
    let slots = unsafe { (*def).m_slots };
    if slots.is_null() {
        return 0;
    }
    let mut cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            let slot = &*cursor;
            match slot.slot {
                PY_MOD_CREATE => {}
                PY_MOD_EXEC => {
                    if slot.value.is_null() {
                        set_module_system_error("Py_mod_exec slot is NULL");
                        return -1;
                    }
                    type ExecFn = unsafe extern "C" fn(module: *mut PyObject) -> c_int;
                    let exec: ExecFn = std::mem::transmute(slot.value);
                    if exec(module) != 0 {
                        set_module_system_error_if_clear(
                            "Py_mod_exec slot returned non-zero without setting an exception",
                        );
                        return -1;
                    }
                }
                PY_MOD_MULTIPLE_INTERPRETERS | PY_MOD_GIL => {}
                _ => {
                    set_module_system_error(format!("unsupported PyModuleDef slot {}", slot.slot));
                    return -1;
                }
            }
            cursor = cursor.add(1);
        }
    }
    0
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
    if unsafe { register_module_capi(module, def) } != 0 {
        unsafe { crate::api::refcount::Py_DECREF(module) };
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
                    let mod_name = CStr::from_ptr(name).to_string_lossy();
                    let meth_name_str = std::str::from_utf8(meth_name).unwrap_or("?");
                    set_module_system_error(format!(
                        "PyModule_Create2 for {mod_name:?}: method {meth_name_str:?} has a NULL function pointer"
                    ));
                    eprintln!(
                        "molt_cpython_abi: PyModule_Create2 for {mod_name:?}: \
                         method {meth_name_str:?} has a NULL function pointer",
                    );
                    crate::api::refcount::Py_DECREF(module);
                    return ptr::null_mut();
                };
                let meth_addr = fn_ptr as *const () as usize as u64;
                let func_bits = (h.register_c_function)(
                    meth_addr,
                    entry.ml_flags,
                    module_bits,
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
                        set_module_system_error(format!(
                            "PyModule_Create2 for {mod_name:?}: failed to register method {meth_name_str:?}"
                        ));
                        eprintln!(
                            "molt_cpython_abi: PyModule_Create2 for {mod_name:?}: \
                             failed to register method {meth_name_str:?}",
                        );
                        crate::api::refcount::Py_DECREF(module);
                        return ptr::null_mut();
                    }
                } else {
                    let mod_name = CStr::from_ptr(name).to_string_lossy();
                    let meth_name_str = std::str::from_utf8(meth_name).unwrap_or("?");
                    set_module_system_error(format!(
                        "PyModule_Create2 for {mod_name:?}: runtime rejected method {meth_name_str:?} (flags 0x{:x})",
                        entry.ml_flags
                    ));
                    eprintln!(
                        "molt_cpython_abi: PyModule_Create2 for {mod_name:?}: \
                         runtime rejected method {meth_name_str:?} (flags 0x{:x})",
                        entry.ml_flags,
                    );
                    crate::api::refcount::Py_DECREF(module);
                    return ptr::null_mut();
                }
                cursor = cursor.add(1);
            }
        }
    }
    module
}
