//! System object registry — PySys_* C API surface.

use crate::abi_types::PyObject;
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySys_GetObject(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    let bits = unsafe {
        (hooks_or_stubs().sys_get_object_borrowed)(name_bytes.as_ptr(), name_bytes.len())
    };
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_GetVersion() -> *const c_char {
    c"3.12.0 (Molt runtime)".as_ptr()
}
