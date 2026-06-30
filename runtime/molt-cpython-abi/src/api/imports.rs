//! Import API entrypoints for C extensions.
//!
//! These are ABI-level hooks. The standalone ABI crate cannot import Python
//! modules by itself because package custody lives in the Molt runtime/import
//! pipeline, so unsupported paths fail closed with a Python exception.

use crate::abi_types::PyObject;
use once_cell::sync::OnceCell;
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModule(name: *const c_char) -> *mut PyObject {
    unsafe { set_import_unavailable(name) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_AddModule(name: *const c_char) -> *mut PyObject {
    unsafe { set_import_unavailable(name) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_GetModuleDict() -> *mut PyObject {
    let raw = MODULE_DICT.get_or_init(|| unsafe { crate::api::mapping::PyDict_New() as usize });
    *raw as *mut PyObject
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevel(
    name: *const c_char,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
    _fromlist: *mut PyObject,
    _level: c_int,
) -> *mut PyObject {
    unsafe { set_import_unavailable(name) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevelObject(
    name: *mut PyObject,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
    _fromlist: *mut PyObject,
    _level: c_int,
) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
    } else {
        let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
        unsafe { set_import_unavailable(name_ptr) };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_Import(name: *mut PyObject) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
    } else {
        let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
        unsafe { set_import_unavailable(name_ptr) };
    }
    ptr::null_mut()
}
