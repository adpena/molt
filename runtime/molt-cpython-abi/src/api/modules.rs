//! Module API — PyModule_New, PyModule_AddObject, PyModuleDef_Init.

use crate::abi_types::{PyMethodDef, PyModuleDef, PyObject};
use crate::bridge::{GLOBAL_BRIDGE, RuntimeValue};
use crate::hooks;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_long, c_void};
use std::ptr;

const PY_MOD_CREATE: c_int = 1;
const PY_MOD_EXEC: c_int = 2;
const PY_MOD_MULTIPLE_INTERPRETERS: c_int = 3;
const PY_MOD_GIL: c_int = 4;

/// `Py_MOD_GIL_NOT_USED == ((void *)1)` — the slot/SetGIL VALUE declaring the
/// module safe to run without the GIL. `Py_MOD_GIL_USED == ((void *)0)`.
/// Cross-header drift for these tokens is bound by
/// `tools/check_table_drift.py` (`_PYMOD_SLOT_VALUES`); CPython authority is
/// `Include/moduleobject.h` (3.13+).
const PY_MOD_GIL_NOT_USED_VALUE: usize = 1;

/// Record the PEP 703 free-threading declaration carried by a module
/// definition: the `{Py_mod_gil, ...}` slot when present, else the CPython
/// default (`Py_MOD_GIL_USED` — on a free-threaded interpreter that import
/// re-enables the GIL). Called at every module-definition entry point
/// (multi-phase create, exec, single-phase create); recording is idempotent
/// and an explicit slot is never downgraded by a later default pass (see
/// `crate::gil_declarations`).
///
/// # Safety
/// `def`, when non-null, must point at a live `PyModuleDef` whose `m_slots`
/// array (when non-null) is zero-terminated.
unsafe fn record_def_gil_declaration(def: *mut PyModuleDef) {
    use crate::gil_declarations::{
        ModuleGilDeclaration, record_module_gil_declaration, record_unresolved_gil_declaration,
    };
    if def.is_null() {
        return;
    }
    let mut decl = ModuleGilDeclaration::GilUsedDefault;
    let slots = unsafe { (*def).m_slots };
    if !slots.is_null() {
        let mut cursor = slots;
        unsafe {
            while (*cursor).slot != 0 {
                if (*cursor).slot == PY_MOD_GIL {
                    // CPython stores the raw slot value into md_gil and tests
                    // it against Py_MOD_GIL_NOT_USED; mirror exactly.
                    decl = if (*cursor).value as usize == PY_MOD_GIL_NOT_USED_VALUE {
                        ModuleGilDeclaration::GilNotUsed
                    } else {
                        ModuleGilDeclaration::GilUsedExplicit
                    };
                }
                cursor = cursor.add(1);
            }
        }
    }
    let name_ptr = unsafe { (*def).m_name };
    if name_ptr.is_null() {
        record_unresolved_gil_declaration();
        return;
    }
    match unsafe { CStr::from_ptr(name_ptr) }.to_str() {
        Ok(name) => record_module_gil_declaration(name, decl),
        Err(_) => record_unresolved_gil_declaration(),
    }
}

fn set_module_system_error(message: impl AsRef<str>) {
    let message = CString::new(message.as_ref())
        .unwrap_or_else(|_| CString::new("module API error").expect("static string has no nul"));
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr(),
        );
    }
}

fn set_module_system_error_if_clear(message: impl AsRef<str>) {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return;
    }
    // A pending runtime exception (for example an import failure raised
    // inside module exec through the import hook) is the real error;
    // never mask it with a synthetic "without setting an exception"
    // message the diagnostics would then surface instead.
    let h = hooks::hooks_or_stubs();
    if unsafe { (h.exception_pending)() } != 0 {
        return;
    }
    // No exception was set anywhere, so an extension returned a failure
    // sentinel without honoring the C-API contract. If a Molt C-API recorded a
    // silent failure on this thread, name it: it pinpoints the exact call the
    // extension's exec sequence tripped on instead of the opaque generic text.
    let message = match crate::capi_trace::take_last_silent_failure() {
        Some(site) => format!("{} (last silent C-API failure: {site})", message.as_ref()),
        None => message.as_ref().to_string(),
    };
    set_module_system_error(message);
}

fn module_error_pending() -> bool {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return true;
    }
    crate::api::errors::transfer_runtime_pending_to_current()
}

unsafe fn validate_module_pointer_result(result: *mut PyObject, call_name: &str) -> *mut PyObject {
    let has_error = module_error_pending();
    match (result.is_null(), has_error) {
        (false, false) => result,
        (true, true) => ptr::null_mut(),
        (true, false) => {
            set_module_system_error(format!(
                "{call_name} returned NULL without setting an exception"
            ));
            ptr::null_mut()
        }
        (false, true) => {
            let pending = crate::api::errors::take_current_error();
            unsafe { crate::api::refcount::Py_DECREF(result) };
            drop(crate::api::errors::take_current_error());
            if let Some(pending) = pending {
                crate::api::errors::restore_current_error_exact(pending);
            }
            unsafe {
                crate::api::errors::replace_current_with_system_error(&format!(
                    "{call_name} returned a result with an exception set"
                ))
            };
            ptr::null_mut()
        }
    }
}

fn validate_module_status_result(rc: c_int, call_name: &str) -> c_int {
    let has_error = module_error_pending();
    match (rc == 0, has_error) {
        (true, false) => 0,
        (false, true) => -1,
        (false, false) => {
            set_module_system_error(format!(
                "{call_name} returned non-zero without setting an exception"
            ));
            -1
        }
        (true, true) => {
            unsafe {
                crate::api::errors::replace_current_with_system_error(&format!(
                    "{call_name} returned success with an exception set"
                ))
            };
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_New(name: *const c_char) -> *mut PyObject {
    unsafe { new_module(name, hooks::hooks_or_stubs().alloc_module) }
}

unsafe fn new_module(
    name: *const c_char,
    allocate: unsafe extern "C" fn(*const u8, usize) -> u64,
) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    // SAFETY: hook is initialised by molt-runtime at startup; stubs return 0 if not.
    let bits = unsafe { allocate(name_bytes.as_ptr(), name_bytes.len()) };
    if bits == 0 {
        return ptr::null_mut();
    }
    // Physical identity and ownership belong to the same ABI image as the
    // runtime. No adjacent-memory decoding or cross-image view exists.
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
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
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(module) {
        return (value.decode().is_ptr()
            && unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Module as u8) as c_int;
    }
    let ob_type = unsafe { (*module).ob_type };
    if std::ptr::eq(ob_type, &raw mut crate::abi_types::PyModule_Type) {
        return 1;
    }
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyModule_Type)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_CheckExact(module: *mut PyObject) -> c_int {
    if let Some(value) = GLOBAL_BRIDGE.molt_handle_for_pyobj(module) {
        return (value.decode().is_ptr()
            && unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Module as u8) as c_int;
    }
    (!module.is_null()
        && std::ptr::eq(
            unsafe { (*module).ob_type },
            &raw const crate::abi_types::PyModule_Type,
        )) as c_int
}

/// CPython 3.13+ Unstable API (`Objects/moduleobject.c`):
/// `int PyUnstable_Module_SetGIL(PyObject *module, void *gil)` — the
/// single-phase-init counterpart of the `Py_mod_gil` slot. `gil` is one of the
/// `Py_MOD_GIL_*` tokens (`void*`-typed; the former `c_int` parameter here
/// deviated from the CPython prototype).
///
/// Molt records the declaration in `crate::gil_declarations` instead of
/// discarding it. Like the historical stub — and unlike CPython, which can
/// fail with `SystemError` — this always returns 0: molt's runtime GIL makes
/// the declaration behavior-free today, and a recorder must not introduce a
/// new failure path into extension init. A declaration whose module name
/// cannot be resolved is counted as unresolved rather than dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Module_SetGIL(
    module: *mut PyObject,
    gil: *mut c_void,
) -> c_int {
    use crate::gil_declarations::{
        ModuleGilDeclaration, record_module_gil_declaration, record_unresolved_gil_declaration,
    };
    let decl = if gil as usize == PY_MOD_GIL_NOT_USED_VALUE {
        ModuleGilDeclaration::GilNotUsed
    } else {
        ModuleGilDeclaration::GilUsedExplicit
    };
    if module.is_null() {
        record_unresolved_gil_declaration();
        return 0;
    }
    let name_ptr = unsafe { PyModule_GetName(module) };
    if name_ptr.is_null() {
        // PyModule_GetName sets SystemError on failure; the recorder contract
        // is side-effect-free success, so clear it and count the declaration.
        unsafe { crate::api::errors::PyErr_Clear() };
        record_unresolved_gil_declaration();
        return 0;
    }
    match unsafe { CStr::from_ptr(name_ptr) }.to_str() {
        Ok(name) => record_module_gil_declaration(name, decl),
        Err(_) => record_unresolved_gil_declaration(),
    }
    0
}

/// Borrow a module string attribute; public object getters add their own C
/// owner while legacy char-pointer getters borrow the module's storage.
unsafe fn module_string_attribute(
    module: *mut PyObject,
    key: &CStr,
    error: *mut PyObject,
    message: &CStr,
) -> *mut PyObject {
    if unsafe { PyModule_Check(module) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    // md_dict, borrowed (PyModule_GetDict returns the module's own dict).
    let dict = unsafe { PyModule_GetDict(module) };
    let name = if dict.is_null() {
        ptr::null_mut()
    } else {
        // Borrowed reference; PyDict_GetItemString suppresses errors like CPython.
        unsafe { crate::api::mapping::PyDict_GetItemString(dict, key.as_ptr()) }
    };
    if name.is_null() || unsafe { crate::api::strings::PyUnicode_Check(name) } == 0 {
        unsafe {
            if crate::api::errors::PyErr_Occurred().is_null() {
                crate::api::errors::PyErr_SetString(error, message.as_ptr());
            }
        }
        return ptr::null_mut();
    }
    name
}

unsafe fn module_get_name_object(module: *mut PyObject) -> *mut PyObject {
    unsafe {
        module_string_attribute(
            module,
            c"__name__",
            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
            c"nameless module",
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetNameObject(module: *mut PyObject) -> *mut PyObject {
    let name = unsafe { module_get_name_object(module) };
    unsafe { crate::api::refcount::Py_XINCREF(name) };
    name
}

unsafe fn module_get_filename_object(module: *mut PyObject) -> *mut PyObject {
    unsafe {
        module_string_attribute(
            module,
            c"__file__",
            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
            c"module filename missing",
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetFilenameObject(module: *mut PyObject) -> *mut PyObject {
    let filename = unsafe { module_get_filename_object(module) };
    unsafe { crate::api::refcount::Py_XINCREF(filename) };
    filename
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetFilename(module: *mut PyObject) -> *const c_char {
    let filename = unsafe { module_get_filename_object(module) };
    if filename.is_null() {
        return ptr::null();
    }
    unsafe { crate::api::strings::PyUnicode_AsUTF8(filename) }
}

/// Molt source-API convenience; returns a new reference like attribute lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetObject(
    module: *mut PyObject,
    name: *const c_char,
) -> *mut PyObject {
    if name.is_null() || unsafe { PyModule_Check(module) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    unsafe { crate::api::object::PyObject_GetAttrString(module, name) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_SetDocString(module: *mut PyObject, doc: *const c_char) -> c_int {
    if unsafe { PyModule_Check(module) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let value = if doc.is_null() {
        unsafe {
            GLOBAL_BRIDGE
                .borrowed_handle_to_new_pyobj(molt_lang_obj_model::MoltObject::none().bits())
        }
    } else {
        unsafe { crate::api::strings::PyUnicode_FromString(doc) }
    };
    if value.is_null() {
        return -1;
    }
    let rc = unsafe { PyModule_AddObjectRef(module, c"__doc__".as_ptr(), value) };
    unsafe { crate::api::errors::release_preserving_error(&[value]) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetName(module: *mut PyObject) -> *const c_char {
    if module.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyModule_GetName called with NULL".as_ptr(),
            );
        }
        return ptr::null();
    }
    // CPython moduleobject.c: PyUnicode_AsUTF8(PyModule_GetNameObject(m)). Return
    // the module's actual __name__, never a fabricated constant — collapsing every
    // module under one key (the previous hardcoded c"molt.module") broke
    // PyImport_AddModule(PyModule_GetName(m)) identity. The returned pointer aliases
    // the __name__ str's runtime UTF-8 buffer, kept alive by the module dict (the
    // borrowed reference contract CPython relies on).
    let name = unsafe { module_get_name_object(module) };
    if name.is_null() {
        return ptr::null();
    }
    unsafe { crate::api::strings::PyUnicode_AsUTF8(name) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDict(module: *mut PyObject) -> *mut PyObject {
    if module.is_null() {
        return ptr::null_mut();
    }
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return ptr::null_mut();
    };
    let module_bits = module_value.bits();
    let h = hooks::hooks_or_stubs();
    let result = unsafe { (h.module_get_dict_borrowed)(module_bits) };
    let hooks::DecodedHandleResult::Ok(dict_bits) = result.decode() else {
        crate::capi_trace::record_silent_failure("PyModule_GetDict", None);
        if !crate::api::errors::transfer_runtime_pending_to_current()
            && unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
        {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyModule_GetDict: runtime module dict hook failed".as_ptr(),
                )
            };
        }
        return ptr::null_mut();
    };
    unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(dict_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetState(module: *mut PyObject) -> *mut std::ffi::c_void {
    if module.is_null() {
        return ptr::null_mut();
    }
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return ptr::null_mut();
    };
    let module_bits = module_value.bits();
    let h = hooks::hooks_or_stubs();
    unsafe { (h.module_capi_get_state)(module_bits).cast() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_GetDef(module: *mut PyObject) -> *mut PyModuleDef {
    if unsafe { PyModule_Check(module) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return ptr::null_mut();
    };
    unsafe {
        (hooks::hooks_or_stubs().module_capi_get_def)(module_value.bits()) as *mut PyModuleDef
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_AddModule(module: *mut PyObject, def: *mut PyModuleDef) -> c_int {
    if module.is_null() || def.is_null() {
        return -1;
    }
    if !unsafe { (*def).m_slots.is_null() } {
        set_module_system_error("PyState_AddModule called on module with slots");
        return -1;
    }
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return -1;
    };
    let module_bits = module_value.bits();
    let h = hooks::hooks_or_stubs();
    let rc = unsafe { (h.module_state_add)(module_bits, def as usize) };
    validate_module_status_result(rc, "module state registration")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_FindModule(def: *mut PyModuleDef) -> *mut PyObject {
    if def.is_null() || !unsafe { (*def).m_slots.is_null() } {
        return ptr::null_mut();
    }
    let h = hooks::hooks_or_stubs();
    match unsafe { (h.module_state_find)(def as usize) }.decode() {
        hooks::DecodedHandleResult::Ok(module_bits) if module_bits != 0 => unsafe {
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(module_bits)
        },
        hooks::DecodedHandleResult::Ok(_) | hooks::DecodedHandleResult::Missing => ptr::null_mut(),
        hooks::DecodedHandleResult::Error => {
            let _ = crate::api::errors::transfer_runtime_pending_to_current();
            set_module_system_error_if_clear("module state lookup failed");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyState_RemoveModule(def: *mut PyModuleDef) -> c_int {
    if def.is_null() {
        return -1;
    }
    if !unsafe { (*def).m_slots.is_null() } {
        set_module_system_error("PyState_RemoveModule called on module with slots");
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
    let rc = unsafe { PyModule_AddObjectRef(module, name, value) };
    if rc == 0 {
        // The runtime module has acquired its own edge. Consume precisely the
        // native C reference stolen by this API, never an extra view anchor.
        unsafe { crate::api::errors::release_preserving_error(&[value]) };
    }
    rc
}

/// Unlike PyModule_AddObject, PyModule_Add consumes its value on failure too.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_Add(
    module: *mut PyObject,
    name: *const c_char,
    value: *mut PyObject,
) -> c_int {
    let rc = unsafe { PyModule_AddObjectRef(module, name, value) };
    unsafe { crate::api::errors::release_preserving_error(&[value]) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddType(
    module: *mut PyObject,
    type_obj: *mut crate::abi_types::PyTypeObject,
) -> c_int {
    if type_obj.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if unsafe { crate::api::typeobj::PyType_Ready(type_obj) } < 0 {
        return -1;
    }
    let name = unsafe { (*type_obj).tp_name };
    if name.is_null() {
        set_module_system_error("module type has no name");
        return -1;
    }
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    let offset = bytes
        .iter()
        .rposition(|byte| *byte == b'.')
        .map_or(0, |index| index + 1);
    unsafe { PyModule_AddObjectRef(module, name.add(offset), type_obj.cast()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddObjectRef(
    module: *mut PyObject,
    name: *const c_char,
    value: *mut PyObject,
) -> c_int {
    if module.is_null() || name.is_null() || value.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return -1;
    };
    let Some(value_owner) = (unsafe { RuntimeValue::acquire_edge(value) }) else {
        return -1;
    };
    let h = hooks::hooks_or_stubs();
    let rc = unsafe {
        (h.module_set_attr)(
            module_value.bits(),
            name_bytes.as_ptr(),
            name_bytes.len(),
            value_owner.bits(),
        )
    };
    validate_module_status_result(rc, "module attribute assignment")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddIntConstant(
    module: *mut PyObject,
    name: *const c_char,
    value: c_long,
) -> c_int {
    let obj = unsafe { crate::api::numbers::PyLong_FromLongLong(value as i64) };
    if obj.is_null() {
        if !module_error_pending() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
        }
        return -1;
    }
    let rc = unsafe { PyModule_AddObjectRef(module, name, obj) };
    unsafe { crate::api::errors::release_preserving_error(&[obj]) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddStringConstant(
    module: *mut PyObject,
    name: *const c_char,
    value: *const c_char,
) -> c_int {
    if value.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let obj = unsafe { crate::api::strings::PyUnicode_FromString(value) };
    if obj.is_null() {
        if !module_error_pending() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
        }
        return -1;
    }
    let rc = unsafe { PyModule_AddObjectRef(module, name, obj) };
    unsafe { crate::api::errors::release_preserving_error(&[obj]) };
    rc
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

unsafe fn register_module_capi(
    module: *mut PyObject,
    def: *mut PyModuleDef,
    attach_legacy_state: bool,
) -> c_int {
    if module.is_null() || def.is_null() {
        return -1;
    }
    let Some(module_value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return -1;
    };
    let module_bits = module_value.bits();
    let h = hooks::hooks_or_stubs();
    let rc = unsafe {
        (h.module_capi_register)(
            module_bits,
            def as usize,
            module_state_size(def),
            !attach_legacy_state,
        )
    };
    if rc != 0 {
        set_module_system_error_if_clear("module C-API metadata registration failed");
        return rc;
    }
    // PyState registration belongs to successful single-phase import, not
    // PyModule_Create. A failing initializer must not leave a strong root.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyModule_AddFunctions(
    module: *mut PyObject,
    functions: *mut PyMethodDef,
) -> c_int {
    let name = unsafe { module_get_name_object(module) };
    unsafe { add_module_functions(module, functions, name) }
}

/// CPython `PyModule_AddFunctions` (`_add_methods_to_object`): each module
/// function is the canonical `PyCFunctionObject` built by `PyCFunction_NewEx`
/// with the module as `m_self` and its `__name__` as `m_module`, published by
/// module attribute assignment. The first failure leaves its exact error.
unsafe fn add_module_functions(
    module: *mut PyObject,
    functions: *mut PyMethodDef,
    name: *mut PyObject,
) -> c_int {
    if name.is_null() {
        return -1;
    }
    if functions.is_null() {
        return 0;
    }
    // `__name__` is borrowed from the module dict; a function published under
    // that key must not release it while later functions still adopt it.
    unsafe { crate::api::refcount::Py_INCREF(name) };
    let mut rc = 0;
    let mut cursor = functions;
    unsafe {
        while !(*cursor).ml_name.is_null() {
            if (*cursor).ml_flags & (crate::abi_types::METH_CLASS | crate::abi_types::METH_STATIC)
                != 0
            {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError).cast::<PyObject>(),
                    c"module functions cannot set METH_CLASS or METH_STATIC".as_ptr(),
                );
                rc = -1;
                break;
            }
            let function = crate::api::object::PyCFunction_NewEx(cursor, module, name);
            if function.is_null() {
                // A runtime-channel error is the real failure: make it the C
                // indicator instead of masking it with a generic message.
                if !module_error_pending() {
                    set_module_system_error(format!(
                        "module function {:?} construction failed without an exception",
                        CStr::from_ptr((*cursor).ml_name)
                    ));
                }
                rc = -1;
                break;
            }
            rc = PyModule_AddObjectRef(module, (*cursor).ml_name, function);
            crate::api::errors::release_preserving_error(&[function]);
            if rc != 0 {
                break;
            }
            cursor = cursor.add(1);
        }
        crate::api::errors::release_preserving_error(&[name]);
    }
    rc
}

unsafe fn module_from_def_and_slots(
    def: *mut PyModuleDef,
    _module_api_version: c_int,
    spec: *mut PyObject,
) -> *mut PyObject {
    if unsafe { (*def).m_size } < 0 {
        set_module_system_error("m_size may not be negative for multi-phase initialization");
        return ptr::null_mut();
    }
    let name = unsafe { crate::api::object::PyObject_GetAttrString(spec, c"name".as_ptr()) };
    if name.is_null() {
        return ptr::null_mut();
    }
    // Record the module's free-threading declaration before creation, exactly
    // as CPython stamps md_gil from the slots during module_from_def_and_spec.
    unsafe { record_def_gil_declaration(def) };
    let slots = unsafe { (*def).m_slots };
    let mut module = ptr::null_mut();
    let mut cursor = slots;
    unsafe {
        while !cursor.is_null() && (*cursor).slot != 0 {
            let slot = &*cursor;
            if slot.slot == PY_MOD_CREATE {
                if slot.value.is_null() {
                    set_module_system_error("Py_mod_create slot is NULL");
                    crate::api::errors::release_preserving_error(&[name]);
                    return ptr::null_mut();
                }
                type CreateFn = unsafe extern "C" fn(
                    spec: *mut PyObject,
                    def: *mut PyModuleDef,
                ) -> *mut PyObject;
                let create: CreateFn = std::mem::transmute(slot.value);
                module = validate_module_pointer_result(create(spec, def), "Py_mod_create slot");
                if module.is_null() {
                    crate::api::errors::release_preserving_error(&[name]);
                    return ptr::null_mut();
                }
                break;
            }
            cursor = cursor.add(1);
        }
    }
    if module.is_null() {
        module = unsafe { PyModule_NewObject(name) };
    }
    if module.is_null() {
        unsafe { crate::api::errors::release_preserving_error(&[name]) };
        return ptr::null_mut();
    }
    // Both the default constructor and Py_mod_create use the qualified spec
    // name for builtin functions' m_module, before any exec slot is entered.
    // A custom creator owns its module's __name__; it need not equal spec.name.
    // CPython still gives methods the spec name, without rewriting that module.
    let result = unsafe { finish_module_creation(module, def, false, name) };
    unsafe { crate::api::errors::release_preserving_error(&[name]) };
    result
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
    // Loaders may create the module elsewhere and only route exec through
    // here; recording is idempotent (explicit slot wins over default).
    unsafe { record_def_gil_declaration(def) };
    let slots = unsafe { (*def).m_slots };
    if slots.is_null() {
        return 0;
    }
    let Some(value) = (unsafe { RuntimeValue::acquire(module) }) else {
        return -1;
    };
    match unsafe { (hooks::hooks_or_stubs().module_exec_begin)(value.bits(), def.addr()) } {
        // Direct C callers may execute slots repeatedly. Only the import
        // loader skips an already-entered module (CPython _imp.exec_dynamic).
        0 | 1 => {}
        _ => {
            set_module_system_error_if_clear("module execution admission failed");
            return -1;
        }
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
                    crate::capi_trace::clear_last_silent_failure();
                    if validate_module_status_result(exec(module), "Py_mod_exec slot") != 0 {
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
    module_api_version: c_int,
) -> *mut PyObject {
    unsafe { module_create2(def, module_api_version, true) }
}

unsafe fn module_create2(
    def: *mut PyModuleDef,
    _module_api_version: c_int,
    attach_legacy_state: bool,
) -> *mut PyObject {
    if def.is_null() {
        return ptr::null_mut();
    }
    if !unsafe { (*def).m_slots.is_null() } {
        set_module_system_error("PyModule_Create is incompatible with m_slots");
        return ptr::null_mut();
    }
    // Single-phase init (a legacy PyInit_* returning PyModule_Create(&def))
    // carries no slots: record the CPython default (GIL used). A slotted def
    // arriving here via module_from_def_and_slots was already recorded — the
    // default pass never downgrades it.
    unsafe { record_def_gil_declaration(def) };
    let name = if unsafe { (*def).m_name.is_null() } {
        c"<unnamed>".as_ptr()
    } else {
        unsafe { (*def).m_name }
    };
    let module = unsafe { new_module(name, hooks::hooks_or_stubs().alloc_extension_module) };
    if module.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { module_get_name_object(module) };
    unsafe { finish_module_creation(module, def, attach_legacy_state, name) }
}

unsafe fn finish_module_creation(
    module: *mut PyObject,
    def: *mut PyModuleDef,
    attach_legacy_state: bool,
    name: *mut PyObject,
) -> *mut PyObject {
    if unsafe { register_module_capi(module, def, attach_legacy_state) } != 0 {
        unsafe { crate::api::errors::release_preserving_error(&[module]) };
        return ptr::null_mut();
    }
    // Malformed or unsupported methods fail module creation atomically
    // instead of publishing a partial method table.
    let m_methods = unsafe { (*def).m_methods };
    if !m_methods.is_null() && unsafe { add_module_functions(module, m_methods, name) } != 0 {
        unsafe { crate::api::errors::release_preserving_error(&[module]) };
        return ptr::null_mut();
    }
    let doc = unsafe { (*def).m_doc };
    if !doc.is_null() && unsafe { PyModule_SetDocString(module, doc) } < 0 {
        unsafe { crate::api::errors::release_preserving_error(&[module]) };
        return ptr::null_mut();
    }
    module
}
