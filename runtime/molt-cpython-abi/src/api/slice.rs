//! Slice object ABI - PySlice_New, checks, and index normalization.

use crate::abi_types::{Py_None, Py_ssize_t, PyObject, PySliceObject};
use std::os::raw::c_int;

pub unsafe extern "C" fn molt_slice_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let slice = op.cast::<PySliceObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*slice).start);
        crate::api::refcount::Py_XDECREF((*slice).stop);
        crate::api::refcount::Py_XDECREF((*slice).step);
        drop(Box::from_raw(slice));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_New(
    start: *mut PyObject,
    stop: *mut PyObject,
    step: *mut PyObject,
) -> *mut PyObject {
    let start = if start.is_null() {
        &raw mut Py_None
    } else {
        start
    };
    let stop = if stop.is_null() {
        &raw mut Py_None
    } else {
        stop
    };
    let step = if step.is_null() {
        &raw mut Py_None
    } else {
        step
    };
    unsafe {
        crate::api::refcount::Py_INCREF(start);
        crate::api::refcount::Py_INCREF(stop);
        crate::api::refcount::Py_INCREF(step);
    }
    let slice = Box::new(PySliceObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PySlice_Type,
        },
        start,
        stop,
        step,
    });
    Box::into_raw(slice).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    std::ptr::eq(ob_type, &raw mut crate::abi_types::PySlice_Type) as c_int
}

/// `_PyEval_SliceIndex` (Python/ceval.c) equivalent: a non-None slice bound
/// must satisfy the `__index__` protocol — else TypeError "slice indices must
/// be integers or None or have an __index__ method" — and converts with the
/// `PyNumber_AsSsize_t(v, NULL)` contract: an out-of-`Py_ssize_t` value CLAMPS
/// to `PY_SSIZE_T_MAX`/`MIN` by its sign (`slice(0, 10**30)` is a full-range
/// slice in CPython, not an error and never a truncation). Returns `false`
/// with a pending exception on failure.
///
/// The pre-fix `slice_index_value` was a bare `PyLong_AsSsize_t`, whose result
/// was stored WITHOUT an error check: `slice('a')`/`slice(1.5)` silently
/// produced -1 bounds and `10**30` truncated (the ledger's CONFIRMED
/// `slice.rs:117` High divergence, reached via numpy's `PySlice_GetIndicesEx`).
unsafe fn slice_index(v: *mut PyObject, out: &mut Py_ssize_t) -> bool {
    // Fast path: a genuine int (inline, bool, or heap BigInt) reads directly
    // with the clamp contract — zero allocation, no `__index__` round-trip.
    if crate::api::numbers::is_int_like(v) {
        return match crate::api::numbers::py_long_as_ssize_clamped(v) {
            Some(x) => {
                *out = x;
                true
            }
            None => false,
        };
    }
    // Foreign objects: the `__index__` protocol, exactly _PyEval_SliceIndex.
    if unsafe { crate::api::abstract_number::PyIndex_Check(v) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"slice indices must be integers or None or have an __index__ method".as_ptr(),
            );
        }
        return false;
    }
    let index = unsafe { crate::api::abstract_number::PyNumber_Index(v) };
    if index.is_null() {
        return false;
    }
    let value = crate::api::numbers::py_long_as_ssize_clamped(index);
    unsafe { crate::api::refcount::Py_DECREF(index) };
    match value {
        Some(x) => {
            *out = x;
            true
        }
        None => false,
    }
}

/// CPython `PySlice_Unpack` (Objects/sliceobject.c): extract start/stop/step
/// with `_PyEval_SliceIndex` conversion per non-None field, ValueError on a
/// zero step, the `-PY_SSIZE_T_MAX` step floor, and the documented None
/// defaults. Returns -1 with a pending exception whenever a bound fails to
/// convert — the pre-fix body stored unchecked `-1` sentinels and returned 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Unpack(
    slice: *mut PyObject,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
) -> c_int {
    if slice.is_null() || start.is_null() || stop.is_null() || step.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if unsafe { PySlice_Check(slice) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"PySlice_Unpack requires a slice object".as_ptr(),
            );
        }
        return -1;
    }
    let layout = slice.cast::<PySliceObject>();

    let raw_step = unsafe { (*layout).step };
    let mut parsed_step: Py_ssize_t = 1;
    if !std::ptr::eq(raw_step, &raw mut Py_None) {
        if !unsafe { slice_index(raw_step, &mut parsed_step) } {
            return -1;
        }
        if parsed_step == 0 {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_ValueError,
                    c"slice step cannot be zero".as_ptr(),
                );
            }
            return -1;
        }
        // sliceobject.c: PY_SSIZE_T_MIN would overflow on negation; floor it.
        parsed_step = parsed_step.max(-Py_ssize_t::MAX);
    }

    let raw_start = unsafe { (*layout).start };
    let mut parsed_start: Py_ssize_t = if parsed_step < 0 { Py_ssize_t::MAX } else { 0 };
    if !std::ptr::eq(raw_start, &raw mut Py_None)
        && !unsafe { slice_index(raw_start, &mut parsed_start) }
    {
        return -1;
    }

    let raw_stop = unsafe { (*layout).stop };
    let mut parsed_stop: Py_ssize_t = if parsed_step < 0 {
        Py_ssize_t::MIN
    } else {
        Py_ssize_t::MAX
    };
    if !std::ptr::eq(raw_stop, &raw mut Py_None)
        && !unsafe { slice_index(raw_stop, &mut parsed_stop) }
    {
        return -1;
    }

    unsafe {
        *step = parsed_step;
        *start = parsed_start;
        *stop = parsed_stop;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_AdjustIndices(
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: Py_ssize_t,
) -> Py_ssize_t {
    if start.is_null() || stop.is_null() || step == 0 {
        return 0;
    }
    let length = length.max(0);
    unsafe {
        if *start < 0 {
            *start += length;
            if *start < 0 {
                *start = if step < 0 { -1 } else { 0 };
            }
        } else if *start >= length {
            *start = if step < 0 { length - 1 } else { length };
        }

        if *stop < 0 {
            *stop += length;
            if *stop < 0 {
                *stop = if step < 0 { -1 } else { 0 };
            }
        } else if *stop >= length {
            *stop = if step < 0 { length - 1 } else { length };
        }

        if step < 0 {
            if *stop < *start {
                (*start - *stop - 1) / (-step) + 1
            } else {
                0
            }
        } else if *start < *stop {
            (*stop - *start - 1) / step + 1
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndicesEx(
    slice: *mut PyObject,
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
    slicelength: *mut Py_ssize_t,
) -> c_int {
    if slicelength.is_null() {
        return -1;
    }
    if unsafe { PySlice_Unpack(slice, start, stop, step) } < 0 {
        unsafe {
            *slicelength = 0;
        }
        return -1;
    }
    unsafe {
        *slicelength = PySlice_AdjustIndices(length, start, stop, *step);
    }
    0
}

/// CPython `PySlice_GetIndices` (Objects/sliceobject.c) — the LEGACY variant,
/// deliberately distinct from `GetIndicesEx`: each non-None field must be an
/// exact int (`PyLong_Check`, no `__index__`), a negative index gets a single
/// `+= length` adjustment with NO further clamping, and out-of-range inputs
/// (`*stop > length`, `*start >= length`) or a zero step return -1 (bare, per
/// CPython's legacy contract — it sets no exception for these). The pre-fix
/// delegation to `GetIndicesEx` silently CLAMPED out-of-range indices that
/// this API must reject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndices(
    slice: *mut PyObject,
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
) -> c_int {
    if slice.is_null()
        || start.is_null()
        || stop.is_null()
        || step.is_null()
        || unsafe { PySlice_Check(slice) } == 0
    {
        return -1;
    }
    let layout = slice.cast::<PySliceObject>();

    let raw_step = unsafe { (*layout).step };
    let step_value = if std::ptr::eq(raw_step, &raw mut Py_None) {
        1
    } else {
        if unsafe { crate::api::numbers::PyLong_Check(raw_step) } == 0 {
            return -1;
        }
        unsafe { crate::api::numbers::PyLong_AsSsize_t(raw_step) }
    };

    let raw_start = unsafe { (*layout).start };
    let start_value = if std::ptr::eq(raw_start, &raw mut Py_None) {
        if step_value < 0 { length - 1 } else { 0 }
    } else {
        if unsafe { crate::api::numbers::PyLong_Check(raw_start) } == 0 {
            return -1;
        }
        let mut v = unsafe { crate::api::numbers::PyLong_AsSsize_t(raw_start) };
        if v < 0 {
            v += length;
        }
        v
    };

    let raw_stop = unsafe { (*layout).stop };
    let stop_value = if std::ptr::eq(raw_stop, &raw mut Py_None) {
        if step_value < 0 { -1 } else { length }
    } else {
        if unsafe { crate::api::numbers::PyLong_Check(raw_stop) } == 0 {
            return -1;
        }
        let mut v = unsafe { crate::api::numbers::PyLong_AsSsize_t(raw_stop) };
        if v < 0 {
            v += length;
        }
        v
    };

    if stop_value > length || start_value >= length || step_value == 0 {
        return -1;
    }
    unsafe {
        *step = step_value;
        *start = start_value;
        *stop = stop_value;
    }
    0
}
