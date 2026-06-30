//! CPython context variable C-API surface.

use crate::abi_types::{PyContextVarObject, PyObject};
use std::ffi::CStr;
use std::os::raw::c_int;
use std::ptr;

unsafe fn is_contextvar(var: *mut PyObject) -> bool {
    !var.is_null()
        && std::ptr::eq(
            unsafe { (*var).ob_type },
            &raw mut crate::abi_types::PyContextVar_Type,
        )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_New(
    name: *const std::os::raw::c_char,
    default_value: *mut PyObject,
) -> *mut PyObject {
    if name.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"context variable name must not be NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    if unsafe { CStr::from_ptr(name) }.to_bytes().is_empty() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_ValueError,
                c"context variable name must not be empty".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(name) };
    if name_obj.is_null() {
        return ptr::null_mut();
    }
    if !default_value.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(default_value) };
    }
    let obj = Box::new(PyContextVarObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyContextVar_Type,
        },
        name: name_obj,
        default_value,
        current_value: ptr::null_mut(),
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Get(
    var: *mut PyObject,
    default_value: *mut PyObject,
    value: *mut *mut PyObject,
) -> c_int {
    if var.is_null() || value.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"var and value pointer must not be NULL".as_ptr(),
            );
        }
        return -1;
    }

    if unsafe { is_contextvar(var) } {
        let context_var = var.cast::<PyContextVarObject>();
        let candidate = unsafe {
            if !(*context_var).current_value.is_null() {
                (*context_var).current_value
            } else if !(*context_var).default_value.is_null() {
                (*context_var).default_value
            } else {
                default_value
            }
        };
        if candidate.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_LookupError,
                    c"context variable has no value".as_ptr(),
                );
            }
            return -1;
        }
        unsafe {
            crate::api::refcount::Py_INCREF(candidate);
            *value = candidate;
        }
        return 0;
    }

    unsafe {
        *value = ptr::null_mut();
    }
    let get_fn = unsafe { crate::api::object::PyObject_GetAttrString(var, c"get".as_ptr()) };
    if get_fn.is_null() {
        return -1;
    }

    let args = if default_value.is_null() {
        unsafe { crate::api::sequences::PyTuple_New(0) }
    } else {
        let tuple = unsafe { crate::api::sequences::PyTuple_New(1) };
        if !tuple.is_null() {
            unsafe { crate::api::refcount::Py_INCREF(default_value) };
            if unsafe { crate::api::sequences::PyTuple_SetItem(tuple, 0, default_value) } != 0 {
                unsafe {
                    crate::api::refcount::Py_DECREF(default_value);
                    crate::api::refcount::Py_DECREF(tuple);
                    crate::api::refcount::Py_DECREF(get_fn);
                }
                return -1;
            }
        }
        tuple
    };
    if args.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(get_fn) };
        return -1;
    }

    let result = unsafe { crate::api::object::PyObject_CallObject(get_fn, args) };
    unsafe {
        crate::api::refcount::Py_DECREF(args);
        crate::api::refcount::Py_DECREF(get_fn);
    }
    if result.is_null() {
        if !default_value.is_null() {
            unsafe {
                crate::api::errors::PyErr_Clear();
                crate::api::refcount::Py_INCREF(default_value);
                *value = default_value;
            }
            return 0;
        }
        return -1;
    }

    unsafe {
        *value = result;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Set(
    var: *mut PyObject,
    value: *mut PyObject,
) -> *mut PyObject {
    if var.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"context var must not be NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    if unsafe { is_contextvar(var) } {
        if value.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    c"context variable value must not be NULL".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        let context_var = var.cast::<PyContextVarObject>();
        unsafe {
            crate::api::refcount::Py_INCREF(value);
            let previous = (*context_var).current_value;
            (*context_var).current_value = value;
            if previous.is_null() {
                crate::api::refcount::Py_INCREF(&raw mut crate::abi_types::Py_None);
                &raw mut crate::abi_types::Py_None
            } else {
                previous
            }
        }
    } else {
        let set_fn = unsafe { crate::api::object::PyObject_GetAttrString(var, c"set".as_ptr()) };
        if set_fn.is_null() {
            return ptr::null_mut();
        }
        let args = unsafe { crate::api::sequences::PyTuple_New(1) };
        if args.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(set_fn) };
            return ptr::null_mut();
        }
        let arg = if value.is_null() {
            &raw mut crate::abi_types::Py_None
        } else {
            value
        };
        unsafe { crate::api::refcount::Py_INCREF(arg) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 0, arg) } != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(arg);
                crate::api::refcount::Py_DECREF(args);
                crate::api::refcount::Py_DECREF(set_fn);
            }
            return ptr::null_mut();
        }

        let result = unsafe { crate::api::object::PyObject_CallObject(set_fn, args) };
        unsafe {
            crate::api::refcount::Py_DECREF(args);
            crate::api::refcount::Py_DECREF(set_fn);
        }
        result
    }
}

pub unsafe extern "C" fn molt_contextvar_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let obj = op.cast::<PyContextVarObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*obj).name);
        crate::api::refcount::Py_XDECREF((*obj).default_value);
        crate::api::refcount::Py_XDECREF((*obj).current_value);
        drop(Box::from_raw(obj));
    }
}
