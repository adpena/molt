//! Numeric type bridge — PyLong_*, PyFloat_*, PyBool_*.

use crate::abi_types::{Py_False, Py_True, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::os::raw::{c_double, c_int, c_long, c_longlong, c_ulong, c_ulonglong};

// ─── PyLong ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLong(v: c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    let bits = MoltObject::from_int(v as i64).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSsize_t(v: isize) -> *mut PyObject {
    unsafe { PyLong_FromLong(v as c_long) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLongLong(v: c_longlong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    let bits = MoltObject::from_int(v as i64).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLong(v: c_ulong) -> *mut PyObject {
    unsafe { PyLong_FromLongLong(v as c_longlong) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLongLong(v: c_ulonglong) -> *mut PyObject {
    unsafe { PyLong_FromLongLong(v as c_longlong) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLong(op: *mut PyObject) -> c_long {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    match bridge.pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_int() {
                obj.as_int().unwrap_or(-1) as c_long
            } else if obj.is_bool() {
                obj.as_bool().map(|b| b as c_long).unwrap_or(-1)
            } else {
                -1
            }
        }
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSsize_t(op: *mut PyObject) -> isize {
    unsafe { PyLong_AsLong(op) as isize }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLong(op: *mut PyObject) -> c_longlong {
    unsafe { PyLong_AsLong(op) as c_longlong }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLong(op: *mut PyObject) -> c_ulong {
    unsafe { PyLong_AsLong(op) as c_ulong }
}

// ─── PyFloat ─────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromDouble(v: c_double) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_AsDouble(op: *mut PyObject) -> c_double {
    if op.is_null() {
        return -1.0;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    match bridge.pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_float() {
                obj.as_float().unwrap_or(f64::NAN)
            } else if obj.is_int() {
                obj.as_int().map(|i| i as f64).unwrap_or(f64::NAN)
            } else {
                f64::NAN
            }
        }
        None => f64::NAN,
    }
}

// ─── PyBool ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBool_FromLong(v: c_long) -> *mut PyObject {
    if v != 0 {
        &raw mut Py_True
    } else {
        &raw mut Py_False
    }
}

// ─── Type checks (PyLong_Check etc.) ─────────────────────────────────────────

macro_rules! type_check {
    ($name:ident, $pred:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(op: *mut PyObject) -> c_int {
            if op.is_null() {
                return 0;
            }
            match GLOBAL_BRIDGE.lock().pyobj_to_handle(op) {
                Some(bits) => MoltObject::from_bits(bits).$pred() as c_int,
                None => 0,
            }
        }
    };
}

type_check!(PyLong_Check, is_int);
type_check!(PyFloat_Check, is_float);
type_check!(PyBool_Check, is_bool);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    match GLOBAL_BRIDGE.lock().pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            (obj.is_int() || obj.is_float() || obj.is_bool()) as c_int
        }
        None => 0,
    }
}
