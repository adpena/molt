//! Import API entrypoints for C extensions.
//!
//! These are ABI-level hooks. Package custody lives in the Molt
//! runtime/import pipeline, so absolute imports route through the
//! `import_module` runtime hook (package custody, static extension
//! registry, sys.modules cache). Paths the runtime cannot own — relative
//! imports without package context, or genuinely standalone use with no
//! registered hooks — fail closed with a Python exception.

use crate::abi_types::PyObject;
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks;
use once_cell::sync::OnceCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

static MODULE_DICT: OnceCell<usize> = OnceCell::new();

unsafe fn set_import_unavailable(_name: *const c_char) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_ImportError,
            c"import API is not available in standalone molt-cpython-abi".as_ptr(),
        );
    }
}

/// Import the dotted module path in `name` through the runtime hook and
/// return an owned bridge `PyObject` for the imported module, or null on
/// failure.
///
/// With registered runtime hooks a null return leaves the real import
/// error in the runtime pending-exception state — deliberately NOT
/// mirrored into the ABI-side `PyErr`, because module-init diagnostics
/// drain the ABI error first and a synthetic message here would mask the
/// runtime's precise import failure. Standalone use (no hooks) fails
/// closed with the ABI-side unavailable error.
unsafe fn import_module_bytes(name: &[u8]) -> *mut PyObject {
    if name.is_empty() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let Some(h) = hooks::hooks() else {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    };
    let module_bits = unsafe { (h.import_module)(name.as_ptr(), name.len()) };
    if module_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(module_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModule(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    unsafe { import_module_bytes(name_bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_AddModule(name: *const c_char) -> *mut PyObject {
    // CPython's PyImport_AddModule must not run an import; the runtime does
    // not expose a create-without-import registry surface here, so this
    // path stays closed until a runtime hook owns that contract.
    unsafe { set_import_unavailable(name) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_GetModuleDict() -> *mut PyObject {
    let raw = MODULE_DICT.get_or_init(|| unsafe { crate::api::mapping::PyDict_New() as usize });
    *raw as *mut PyObject
}

unsafe fn import_module_level_bytes(
    name: &[u8],
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    if level != 0 {
        // Relative imports need package context the ABI boundary does not
        // carry; fail closed instead of guessing a package root.
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let fromlist_empty =
        if fromlist.is_null() || std::ptr::eq(fromlist, &raw mut crate::abi_types::Py_None) {
            true
        } else {
            match unsafe { crate::api::object::PyObject_IsTrue(fromlist) } {
                -1 => return ptr::null_mut(),
                0 => true,
                _ => false,
            }
        };
    // Import the full dotted chain first; __import__ with an empty fromlist
    // then binds the ROOT package, otherwise the leaf module.
    let leaf = unsafe { import_module_bytes(name) };
    if leaf.is_null() {
        return ptr::null_mut();
    }
    if fromlist_empty {
        if let Some(dot) = name.iter().position(|byte| *byte == b'.') {
            let root = unsafe { import_module_bytes(&name[..dot]) };
            unsafe { crate::api::refcount::Py_DECREF(leaf) };
            return root;
        }
    }
    leaf
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevel(
    name: *const c_char,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    unsafe { import_module_level_bytes(name_bytes, fromlist, level) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevelObject(
    name: *mut PyObject,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name_ptr).to_bytes() };
    unsafe { import_module_level_bytes(name_bytes, fromlist, level) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_Import(name: *mut PyObject) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    unsafe { PyImport_ImportModule(name_ptr) }
}
