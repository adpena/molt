//! The public marshal ABI transports values to the runtime's one wire codec.
use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::{GLOBAL_BRIDGE, RuntimeValue};
use crate::hooks::{MarshalInputOwnership, RuntimeGilGuard, hooks_or_stubs};
use std::os::raw::{c_char, c_int};

unsafe fn unavailable() -> *mut PyObject {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
            c"marshal requires the Molt runtime codec".as_ptr(),
        );
    }
    std::ptr::null_mut()
}

/// # Safety
/// `value` is a live borrowed Python object under the caller's C ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMarshal_WriteObjectToString(
    value: *mut PyObject,
    version: c_int,
) -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    let Some(write) = hooks_or_stubs().marshal_write else {
        return unsafe { unavailable() };
    };
    let Some(value) = (unsafe { RuntimeValue::acquire(value) }) else {
        crate::api::errors::transfer_runtime_pending_to_current();
        return std::ptr::null_mut();
    };
    let bits = value.bits();
    let temporary = value.into_temporary_owner();
    let ownership = if temporary.is_some() {
        MarshalInputOwnership::TemporaryWrapper
    } else {
        MarshalInputOwnership::Borrowed
    };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(write(bits, version, ownership)) }
}

/// # Safety
/// `data..data+length` is readable for this call; NULL is valid only for zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMarshal_ReadObjectFromString(
    data: *const c_char,
    length: Py_ssize_t,
) -> *mut PyObject {
    let _gil = RuntimeGilGuard::ensure();
    if length < 0 || (data.is_null() && length != 0) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    let Some(read) = hooks_or_stubs().marshal_read else {
        return unsafe { unavailable() };
    };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(read(data.cast(), length as usize)) }
}
