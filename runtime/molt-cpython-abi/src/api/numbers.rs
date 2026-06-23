//! Numeric type bridge — PyLong_*, PyFloat_*, PyBool_*.

use crate::abi_types::{Py_False, Py_True, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::{c_double, c_int, c_long, c_longlong, c_ulong, c_ulonglong};
use std::ptr;

// ─── PyLong ──────────────────────────────────────────────────────────────────

fn py_long_from_i64(v: i64) -> *mut PyObject {
    let bits = MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_i64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

fn py_long_from_u64(v: u64) -> *mut PyObject {
    let bits = MoltObject::try_from_uint(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_u64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

fn py_long_as_i64(op: *mut PyObject) -> i64 {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    match bridge.pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_int() {
                obj.as_int().unwrap_or(-1)
            } else if obj.is_bool() {
                obj.as_bool().map(|b| if b { 1 } else { 0 }).unwrap_or(-1)
            } else if obj.is_ptr() {
                unsafe { (hooks_or_stubs().int_as_i64)(bits) }
            } else {
                -1
            }
        }
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLong(v: c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSsize_t(v: isize) -> *mut PyObject {
    unsafe { PyLong_FromLong(v as c_long) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLongLong(v: c_longlong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLong(v: c_ulong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLongLong(v: c_ulonglong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLong(op: *mut PyObject) -> c_long {
    py_long_as_i64(op) as c_long
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSsize_t(op: *mut PyObject) -> isize {
    py_long_as_i64(op) as isize
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLong(op: *mut PyObject) -> c_longlong {
    #[allow(clippy::unnecessary_cast)]
    {
        py_long_as_i64(op) as c_longlong
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLong(op: *mut PyObject) -> c_ulong {
    py_long_as_i64(op) as c_ulong
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
