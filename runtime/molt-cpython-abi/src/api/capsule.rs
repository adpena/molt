//! Capsule API - PyCapsule_New, PyCapsule_GetPointer, etc.
//!
//! Capsules are opaque containers for C pointers, commonly used by extensions
//! to share C-level APIs between modules such as NumPy's C API.

use crate::abi_types::{PyCapsuleDestructor, PyCapsuleObject, PyObject};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;

#[allow(non_camel_case_types)]
pub type PyCapsule_Destructor = Option<PyCapsuleDestructor>;

struct CapsuleEntry {
    pointer: *mut c_void,
}

unsafe impl Send for CapsuleEntry {}
unsafe impl Sync for CapsuleEntry {}

static CAPSULE_REGISTRY: once_cell::sync::Lazy<Mutex<HashMap<String, CapsuleEntry>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

unsafe fn capsule_object(capsule: *mut PyObject) -> Option<*mut PyCapsuleObject> {
    if capsule.is_null() {
        return None;
    }
    let ob_type = unsafe { (*capsule).ob_type };
    if std::ptr::eq(ob_type, &raw mut crate::abi_types::PyCapsule_Type) {
        Some(capsule.cast::<PyCapsuleObject>())
    } else {
        None
    }
}

unsafe fn capsule_name_key(name: *const c_char) -> Option<String> {
    if name.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() })
    }
}

unsafe fn capsule_name_matches(stored: *const c_char, requested: *const c_char) -> bool {
    match (stored.is_null(), requested.is_null()) {
        (true, true) => true,
        (true, false) | (false, true) => false,
        (false, false) => unsafe { CStr::from_ptr(stored) == CStr::from_ptr(requested) },
    }
}

unsafe fn capsule_value_error(message: &'static CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_ValueError,
            message.as_ptr(),
        );
    }
}

pub unsafe extern "C" fn molt_capsule_dealloc(op: *mut PyObject) {
    let Some(capsule) = (unsafe { capsule_object(op) }) else {
        return;
    };
    let name = unsafe { (*capsule).name };
    let pointer = unsafe { (*capsule).pointer };
    if let Some(key) = unsafe { capsule_name_key(name) } {
        let mut registry = CAPSULE_REGISTRY.lock();
        if registry
            .get(&key)
            .is_some_and(|entry| entry.pointer == pointer)
        {
            registry.remove(&key);
        }
    }
    if let Some(destructor) = unsafe { (*capsule).destructor } {
        unsafe { destructor(op) };
    }
    unsafe { drop(Box::from_raw(capsule)) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_New(
    pointer: *mut c_void,
    name: *const c_char,
    destructor: PyCapsule_Destructor,
) -> *mut PyObject {
    if pointer.is_null() {
        unsafe { capsule_value_error(c"PyCapsule_New called with NULL pointer") };
        return ptr::null_mut();
    }
    let capsule = Box::new(PyCapsuleObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyCapsule_Type,
        },
        pointer,
        name,
        context: ptr::null_mut(),
        destructor,
    });
    if let Some(key) = unsafe { capsule_name_key(name) } {
        CAPSULE_REGISTRY
            .lock()
            .insert(key, CapsuleEntry { pointer });
    }
    Box::into_raw(capsule).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_CheckExact(capsule: *mut PyObject) -> c_int {
    unsafe { capsule_object(capsule).is_some() as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetPointer(
    capsule: *mut PyObject,
    name: *const c_char,
) -> *mut c_void {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_GetPointer called with invalid capsule") };
        return ptr::null_mut();
    };
    if !unsafe { capsule_name_matches((*capsule).name, name) } {
        unsafe { capsule_value_error(c"PyCapsule_GetPointer called with incorrect name") };
        return ptr::null_mut();
    }
    unsafe { (*capsule).pointer }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetName(capsule: *mut PyObject) -> *const c_char {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_GetName called with invalid capsule") };
        return ptr::null();
    };
    unsafe { (*capsule).name }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_GetContext(capsule: *mut PyObject) -> *mut c_void {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_GetContext called with invalid capsule") };
        return ptr::null_mut();
    };
    unsafe { (*capsule).context }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_IsValid(capsule: *mut PyObject, name: *const c_char) -> c_int {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        return 0;
    };
    unsafe { capsule_name_matches((*capsule).name, name) as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_SetPointer(
    capsule: *mut PyObject,
    pointer: *mut c_void,
) -> c_int {
    if pointer.is_null() {
        unsafe { capsule_value_error(c"PyCapsule_SetPointer called with NULL pointer") };
        return -1;
    }
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_SetPointer called with invalid capsule") };
        return -1;
    };
    unsafe {
        (*capsule).pointer = pointer;
        if let Some(key) = capsule_name_key((*capsule).name) {
            CAPSULE_REGISTRY
                .lock()
                .insert(key, CapsuleEntry { pointer });
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_SetContext(
    capsule: *mut PyObject,
    context: *mut c_void,
) -> c_int {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_SetContext called with invalid capsule") };
        return -1;
    };
    unsafe {
        (*capsule).context = context;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_SetName(capsule: *mut PyObject, name: *const c_char) -> c_int {
    let Some(capsule) = (unsafe { capsule_object(capsule) }) else {
        unsafe { capsule_value_error(c"PyCapsule_SetName called with invalid capsule") };
        return -1;
    };
    unsafe {
        let pointer = (*capsule).pointer;
        if let Some(old_key) = capsule_name_key((*capsule).name) {
            let mut registry = CAPSULE_REGISTRY.lock();
            if registry
                .get(&old_key)
                .is_some_and(|entry| entry.pointer == pointer)
            {
                registry.remove(&old_key);
            }
        }
        (*capsule).name = name;
        if let Some(new_key) = capsule_name_key(name) {
            CAPSULE_REGISTRY
                .lock()
                .insert(new_key, CapsuleEntry { pointer });
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCapsule_Import(name: *const c_char, _no_block: c_int) -> *mut c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    let key = unsafe { CStr::from_ptr(name).to_string_lossy().into_owned() };
    CAPSULE_REGISTRY
        .lock()
        .get(&key)
        .map(|entry| entry.pointer)
        .unwrap_or(ptr::null_mut())
}
