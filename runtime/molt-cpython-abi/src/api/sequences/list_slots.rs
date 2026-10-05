//! Physical builtin list sequence slots. Python overrides belong to the actual
//! class's slot table; these functions always operate on the declaring list's
//! storage through the existing C-API readers, publishers and mutation owner.
use super::*;
use crate::abi_types::PySequenceMethods;
use crate::api::abstract_sequence::{MaterializedPointers, checked_py_ssize};
use crate::api::{errors, object, refcount::OwnedPyObject, typeobj};
use std::os::raw::c_void;

static mut LIST_SEQUENCE: PySequenceMethods = PySequenceMethods {
    sq_length: PyList_Size as *mut c_void,
    sq_concat: concat as *mut c_void,
    sq_repeat: repeat as *mut c_void,
    sq_item: PyList_GetItemRef as *mut c_void,
    was_sq_slice: ptr::null_mut(),
    sq_ass_item: assign_item as *mut c_void,
    was_sq_ass_slice: ptr::null_mut(),
    sq_contains: contains as *mut c_void,
    sq_inplace_concat: inplace_concat as *mut c_void,
    sq_inplace_repeat: inplace_repeat as *mut c_void,
};

/// Called once by builtin type initialization before any type is published.
pub(crate) unsafe fn initialize() {
    unsafe { crate::abi_types::PyList_Type.tp_as_sequence = (&raw mut LIST_SEQUENCE).cast() };
}

pub(crate) unsafe extern "C" fn assign_item(
    list: *mut PyObject,
    index: Py_ssize_t,
    value: *mut PyObject,
) -> c_int {
    let len = unsafe { PyList_Size(list) };
    if len < 0 {
        return -1;
    }
    if index < 0 || index >= len {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_IndexError).cast(),
                c"list assignment index out of range".as_ptr(),
            );
        }
        return -1;
    }
    if value.is_null() {
        return unsafe { PyList_SetSlice(list, index, index + 1, ptr::null_mut()) };
    }
    // sq_ass_item borrows; PyList_SetItem consumes on success and failure.
    unsafe { crate::api::refcount::Py_INCREF(value) };
    unsafe { PyList_SetItem(list, index, value) }
}

unsafe extern "C" fn contains(list: *mut PyObject, needle: *mut PyObject) -> c_int {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    let mut index = 0;
    loop {
        // Equality may clear, resize or replace the list. Own this one element
        // and acquire current storage again after each callback.
        let item = {
            let Some(read) = (unsafe { ListRead::acquire(list) }) else {
                return -1;
            };
            if index >= unsafe { read.len() } {
                return 0;
            }
            let item = unsafe { read.item(index) };
            if item.is_null() {
                return -1;
            }
            unsafe { OwnedPyObject::from_borrowed(item) }
        };
        let equal = unsafe { typeobj::PyObject_RichCompareBool(item.as_ptr(), needle, 2) };
        drop(item);
        if equal != 0 {
            return equal;
        }
        index += 1;
    }
}

unsafe extern "C" fn concat(left: *mut PyObject, right: *mut PyObject) -> *mut PyObject {
    if unsafe { PyList_Check(right) } == 0 {
        if unsafe { errors::PyErr_Occurred() }.is_null() {
            let message = format!("can only concatenate list (not \"{}\") to list", unsafe {
                object::type_name_lossy(right)
            });
            if let Ok(message) = std::ffi::CString::new(message) {
                unsafe {
                    errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                        message.as_ptr(),
                    );
                }
            }
        }
        return ptr::null_mut();
    }
    let Some(left) = (unsafe { MaterializedPointers::from_list_storage(left) }) else {
        return ptr::null_mut();
    };
    let Some(right) = (unsafe { MaterializedPointers::from_list_storage(right) }) else {
        return ptr::null_mut();
    };
    let Some(total) = left.len().checked_add(right.len()) else {
        return unsafe { errors::PyErr_NoMemory() };
    };
    let Some(total) = checked_py_ssize(total) else {
        return ptr::null_mut();
    };
    unsafe {
        list_from_borrowed_indexed(total, |index| {
            let index = index as usize;
            if index < left.len() {
                left.as_slice()[index]
            } else {
                right.as_slice()[index - left.len()]
            }
        })
    }
}

unsafe extern "C" fn repeat(list: *mut PyObject, count: Py_ssize_t) -> *mut PyObject {
    let Some(source) = (unsafe { MaterializedPointers::from_list_storage(list) }) else {
        return ptr::null_mut();
    };
    let count = count.max(0) as usize;
    let Some(total) = source.len().checked_mul(count) else {
        return unsafe { errors::PyErr_NoMemory() };
    };
    let Some(total) = checked_py_ssize(total) else {
        return ptr::null_mut();
    };
    unsafe {
        list_from_borrowed_indexed(total, |index| {
            // The publisher never asks for an item when the source is empty.
            source.as_slice()[index as usize % source.len()]
        })
    }
}

unsafe extern "C" fn inplace_concat(list: *mut PyObject, other: *mut PyObject) -> *mut PyObject {
    let result = unsafe {
        OwnedPyObject::from_owned(crate::api::abstract_sequence::_PyList_Extend(list, other))
    };
    if result.as_ptr().is_null() {
        return ptr::null_mut();
    }
    unsafe { object::Py_NewRef(list) }
}

unsafe extern "C" fn inplace_repeat(list: *mut PyObject, count: Py_ssize_t) -> *mut PyObject {
    if count == 1 {
        // A direct slot call still validates/commits its physical receiver.
        if unsafe { PyList_Size(list) } < 0 {
            return ptr::null_mut();
        }
        return unsafe { object::Py_NewRef(list) };
    }
    // Build the complete physical replacement before one splice transaction.
    // Allocation failure cannot leave a prefix of the requested repetition.
    let repeated = unsafe { OwnedPyObject::from_owned(repeat(list, count)) };
    if repeated.as_ptr().is_null() {
        return ptr::null_mut();
    }
    if unsafe { PyList_SetSlice(list, 0, Py_ssize_t::MAX, repeated.as_ptr()) } < 0 {
        return ptr::null_mut();
    }
    unsafe { object::Py_NewRef(list) }
}
