//! Capsule API — PyCapsule_New, PyCapsule_GetPointer, etc.
//!
//! Capsules are opaque containers for C pointers, commonly used by extensions
//! to share C-level APIs between modules (e.g., NumPy's C API).

use crate::abi_types::PyObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Destructor type for capsules.
pub type PyCapsule_Destructor = Option<unsafe extern "C" fn(*mut PyObject)>;

// ─── In-process capsule registry ─────────────────────────────────────────
//
// Real capsules in CPython are PyObject wrappers around (void*, name, destructor).
// We simulate this with a global map: name → (pointer, destructor).
// This is sufficient for the common pattern where extension A exports a C API
// via PyCapsule_Import("module._C_API") and extension B retrieves it.

use parking_lot::Mutex;
use std::collections::HashMap;

struct CapsuleEntry {
    pointer: *mut std::ffi::c_void,
    _destructor: PyCapsule_Destructor,
}

unsafe impl Send for CapsuleEntry {}
unsafe impl Sync for CapsuleEntry {}

static CAPSULE_REGISTRY: once_cell::sync::Lazy<Mutex<HashMap<String, CapsuleEntry>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_New(
    pointer: *mut std::ffi::c_void,
    name: *const c_char,
    destructor: PyCapsule_Destructor,
) -> *mut PyObject {
    if pointer.is_null() {
        return ptr::null_mut();
    }
    let key = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() }
    };
    CAPSULE_REGISTRY.lock().insert(
        key,
        CapsuleEntry {
            pointer,
            _destructor: destructor,
        },
    );
    // Return a non-null sentinel (Py_None) — capsule objects are opaque to
    // extension code, which only passes them to PyCapsule_GetPointer.
    &raw mut crate::abi_types::Py_None
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetPointer(
    capsule: *mut PyObject,
    name: *const c_char,
) -> *mut std::ffi::c_void {
    let _ = capsule;
    let key = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() }
    };
    CAPSULE_REGISTRY
        .lock()
        .get(&key)
        .map(|e| e.pointer)
        .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetName(capsule: *mut PyObject) -> *const c_char {
    let _ = capsule;
    // Cannot reconstruct the name from just the sentinel pointer.
    ptr::null()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_IsValid(
    capsule: *mut PyObject,
    name: *const c_char,
) -> c_int {
    if capsule.is_null() {
        return 0;
    }
    let key = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() }
    };
    CAPSULE_REGISTRY.lock().contains_key(&key) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_SetPointer(
    capsule: *mut PyObject,
    pointer: *mut std::ffi::c_void,
) -> c_int {
    let _ = capsule;
    if pointer.is_null() {
        return -1;
    }
    // Without knowing which capsule this is, we cannot update the right entry.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_Import(
    name: *const c_char,
    _no_block: c_int,
) -> *mut std::ffi::c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    let key = unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() };
    CAPSULE_REGISTRY
        .lock()
        .get(&key)
        .map(|e| e.pointer)
        .unwrap_or(ptr::null_mut())
}
