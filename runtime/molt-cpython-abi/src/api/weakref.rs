//! Weak reference C-API surface.

use crate::abi_types::PyObject;
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyWeakref_Check(_op: *mut PyObject) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyWeakref_GetObject(ref_obj: *mut PyObject) -> *mut PyObject {
    if ref_obj.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"expected a weakref".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    &raw mut crate::abi_types::Py_None
}
