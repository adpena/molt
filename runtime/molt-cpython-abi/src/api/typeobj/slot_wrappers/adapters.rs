//! CPython typeobject.c wrapper adapters. The declaration selects the exact
//! callback signature; names never select an ABI or reverse operands at call time.

use super::super::descriptors::{checked_status, new_reference, pending, type_error};
use crate::abi_types::*;
use crate::api::{abstract_number, errors, memory, numbers, sequences};
use std::ffi::{c_int, c_void};
use std::ptr;

// Every non-keyword slot adapter accepts at most two positional operands.
// Missing optional operands have CPython's None default; the input tuple owns
// all supplied operands for the duration of raw_call.
unsafe fn arguments(args: *mut PyObject, min: usize, max: usize) -> Option<[*mut PyObject; 2]> {
    debug_assert!(max <= 2);
    let count = unsafe { sequences::PyTuple_Size(args) };
    if count < 0 {
        return None;
    }
    if (count as usize) < min || (count as usize) > max {
        let expected = if min == max {
            min.to_string()
        } else {
            format!("{min} to {max}")
        };
        unsafe {
            type_error(&format!(
                "expected {expected} argument{}, got {count}",
                if max == 1 { "" } else { "s" }
            ))
        };
        return None;
    }
    let mut values = [&raw mut Py_None; 2];
    for index in 0..count {
        let value = unsafe { sequences::PyTuple_GetItem(args, index) };
        if value.is_null() {
            return None;
        }
        values[index as usize] = value;
    }
    Some(values)
}

unsafe fn status_result(status: c_int) -> *mut PyObject {
    if unsafe { checked_status(status) } < 0 {
        ptr::null_mut()
    } else {
        unsafe { new_reference(&raw mut Py_None) }
    }
}

pub(super) unsafe extern "C" fn unary(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if unsafe { arguments(args, 0, 0) }.is_none() {
        return ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { call(self_) }
}

pub(super) unsafe extern "C" fn next(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if unsafe { arguments(args, 0, 0) }.is_none() {
        return ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    let result = unsafe { call(self_) };
    if result.is_null() && !pending() {
        unsafe { errors::PyErr_SetNone((&raw mut PyExc_StopIteration).cast()) };
    }
    result
}

pub(super) unsafe extern "C" fn binary<const REVERSE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    if REVERSE {
        unsafe { call(values[0], self_) }
    } else {
        unsafe { call(self_, values[0]) }
    }
}

pub(super) unsafe extern "C" fn ternary<const REVERSE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 2) }) else {
        return ptr::null_mut();
    };
    let third = values.get(1).copied().unwrap_or(&raw mut Py_None);
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    if REVERSE {
        unsafe { call(values[0], self_, third) }
    } else {
        unsafe { call(self_, values[0], third) }
    }
}

pub(super) unsafe extern "C" fn size(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if unsafe { arguments(args, 0, 0) }.is_none() {
        return ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t =
        unsafe { std::mem::transmute(wrapped) };
    let result = unsafe { call(self_) };
    if result == -1 && pending() {
        return ptr::null_mut();
    }
    unsafe { numbers::PyLong_FromSsize_t(result) }
}

pub(super) unsafe extern "C" fn inquiry(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if unsafe { arguments(args, 0, 0) }.is_none() {
        return ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    let result = unsafe { call(self_) };
    if result == -1 && pending() {
        return ptr::null_mut();
    }
    unsafe { numbers::PyBool_FromLong(result.into()) }
}

pub(super) unsafe extern "C" fn contains(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    let result = unsafe { call(self_, values[0]) };
    if result == -1 && pending() {
        return ptr::null_mut();
    }
    unsafe { numbers::PyBool_FromLong(result.into()) }
}

unsafe fn index(self_: *mut PyObject, object: *mut PyObject, adjust: bool) -> Option<Py_ssize_t> {
    let mut value = unsafe {
        abstract_number::PyNumber_AsSsize_t(object, (&raw mut PyExc_OverflowError).cast())
    };
    if pending() {
        return None;
    }
    if adjust && value < 0 {
        let ty = unsafe { crate::bridge::semantic_type(self_) };
        if !ty.is_null() {
            let table = unsafe { (*ty).tp_as_sequence }.cast::<PySequenceMethods>();
            if !table.is_null() && !unsafe { (*table).sq_length }.is_null() {
                let length: unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t =
                    unsafe { std::mem::transmute((*table).sq_length) };
                let length = unsafe { length(self_) };
                if unsafe {
                    errors::check_native_status(
                        if length < 0 { -1 } else { 0 },
                        "native sequence length",
                    )
                } < 0
                {
                    return None;
                }
                value += length;
            }
        }
    }
    Some(value)
}

pub(super) unsafe extern "C" fn index_arg<const ADJUST: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    let Some(index) = (unsafe { index(self_, values[0], ADJUST) }) else {
        return ptr::null_mut();
    };
    let call: unsafe extern "C" fn(*mut PyObject, Py_ssize_t) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { call(self_, index) }
}

pub(super) unsafe extern "C" fn sequence_set<const DELETE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let count = if DELETE { 1 } else { 2 };
    let Some(values) = (unsafe { arguments(args, count, count) }) else {
        return ptr::null_mut();
    };
    let Some(index) = (unsafe { index(self_, values[0], true) }) else {
        return ptr::null_mut();
    };
    let value = if DELETE { ptr::null_mut() } else { values[1] };
    let call: unsafe extern "C" fn(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { status_result(call(self_, index, value)) }
}

pub(super) unsafe extern "C" fn object_set<const DELETE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let count = if DELETE { 1 } else { 2 };
    let Some(values) = (unsafe { arguments(args, count, count) }) else {
        return ptr::null_mut();
    };
    let value = if DELETE { ptr::null_mut() } else { values[1] };
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { status_result(call(self_, values[0], value)) }
}

/// Reject an explicit base setter that jumps over a native layout override.
/// Runtime-owned classes have no C override; their shared generic setter remains
/// the runtime authority after receiver projection.
unsafe fn setter_admitted(self_: *mut PyObject, wrapped: *mut c_void) -> bool {
    let ty = unsafe { crate::bridge::semantic_type(self_) };
    if ty.is_null() {
        return false;
    }
    let _type_owner = unsafe { crate::api::refcount::OwnedPyObject::from_borrowed(ty.cast()) };
    let current = unsafe { (*ty).tp_setattro }.map(|f| f as *const () as *mut c_void);
    let mut defining = ty;
    let mro = unsafe { (*ty).tp_mro };
    if mro.is_null() {
        return true;
    }
    let _mro_owner = unsafe { crate::api::refcount::OwnedPyObject::from_borrowed(mro) };
    if !mro.is_null() {
        let count = unsafe { sequences::PyTuple_Size(mro) };
        if count < 0 {
            return false;
        }
        for index in (0..count).rev() {
            let base = unsafe { sequences::PyTuple_GetItem(mro, index) }.cast::<PyTypeObject>();
            if base.is_null() {
                return false;
            }
            if crate::bridge::GLOBAL_BRIDGE
                .managed_handle_for_pyobj(base.cast())
                .is_some()
            {
                continue;
            }
            if unsafe { (*base).tp_setattro }.map(|f| f as *const () as *mut c_void) == current {
                defining = base;
                break;
            }
        }
    }
    while !defining.is_null() {
        let slot = unsafe { (*defining).tp_setattro }.map(|f| f as *const () as *mut c_void);
        if slot == Some(wrapped) {
            return true;
        }
        if crate::bridge::GLOBAL_BRIDGE
            .managed_handle_for_pyobj(defining.cast())
            .is_none()
        {
            unsafe { type_error("can't apply this attribute setter across a native override") };
            return false;
        }
        defining = unsafe { (*defining).tp_base };
    }
    true
}

pub(super) unsafe extern "C" fn attribute_set<const DELETE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let count = if DELETE { 1 } else { 2 };
    let Some(values) = (unsafe { arguments(args, count, count) }) else {
        return ptr::null_mut();
    };
    if !unsafe { setter_admitted(self_, wrapped) } {
        return ptr::null_mut();
    }
    let value = if DELETE { ptr::null_mut() } else { values[1] };
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { status_result(call(self_, values[0], value)) }
}

pub(super) unsafe extern "C" fn compare<const OP: c_int>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, c_int) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { call(self_, values[0], OP) }
}

pub(super) unsafe extern "C" fn descr_get(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 2) }) else {
        return ptr::null_mut();
    };
    let object = if values[0] == &raw mut Py_None {
        ptr::null_mut()
    } else {
        values[0]
    };
    let owner = values
        .get(1)
        .copied()
        .filter(|value| *value != &raw mut Py_None)
        .unwrap_or(ptr::null_mut());
    if object.is_null() && owner.is_null() {
        return unsafe { type_error("__get__(None, None) is invalid") };
    }
    let call: PyDescrGetFunc = unsafe { std::mem::transmute(wrapped) };
    unsafe { crate::api::descriptor::invoke_get(call, self_, object, owner) }
}

pub(super) unsafe extern "C" fn descr_set<const DELETE: bool>(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let count = if DELETE { 1 } else { 2 };
    let Some(values) = (unsafe { arguments(args, count, count) }) else {
        return ptr::null_mut();
    };
    let value = if DELETE { ptr::null_mut() } else { values[1] };
    let call: PyDescrSetFunc = unsafe { std::mem::transmute(wrapped) };
    unsafe {
        status_result(crate::api::descriptor::invoke_set(
            call, self_, values[0], value,
        ))
    }
}

pub(super) unsafe extern "C" fn call(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { call(self_, args, kwargs) }
}

pub(super) unsafe extern "C" fn init(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { status_result(call(self_, args, kwargs)) }
}

pub(super) unsafe extern "C" fn finalize(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if unsafe { arguments(args, 0, 0) }.is_none() {
        return ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut PyObject) = unsafe { std::mem::transmute(wrapped) };
    unsafe {
        call(self_);
        new_reference(&raw mut Py_None)
    }
}

pub(super) unsafe extern "C" fn buffer(
    self_: *mut PyObject,
    args: *mut PyObject,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    let Some(flags) = (unsafe { index(self_, values[0], false) }) else {
        return ptr::null_mut();
    };
    let Ok(flags) = c_int::try_from(flags) else {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_OverflowError).cast(),
                c"buffer flags out of range".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let get: unsafe extern "C" fn(*mut PyObject, *mut Py_buffer, c_int) -> c_int =
        unsafe { std::mem::transmute(wrapped) };
    unsafe { memory::memoryview_from_buffer_proc(self_, flags, get) }
}

pub(super) unsafe extern "C" fn release_buffer(
    self_: *mut PyObject,
    args: *mut PyObject,
    _wrapped: *mut c_void,
) -> *mut PyObject {
    let Some(values) = (unsafe { arguments(args, 1, 1) }) else {
        return ptr::null_mut();
    };
    if unsafe { memory::PyMemoryView_Check(values[0]) } == 0 {
        return unsafe { type_error("expected a memoryview object") };
    }
    let view = unsafe { memory::PyMemoryView_GET_BUFFER(values[0]) };
    if view.is_null() {
        return ptr::null_mut();
    }
    if unsafe { (*view).obj.is_null() } {
        return unsafe { new_reference(&raw mut Py_None) };
    }
    if unsafe { (*view).obj } != self_ {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"memoryview's buffer is not this object".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    // CPython releases the view, which invokes its actual exporter's release
    // authority exactly once; it does not invoke the declaring pointer directly.
    unsafe { memory::memoryview_release(values[0]) }
}
