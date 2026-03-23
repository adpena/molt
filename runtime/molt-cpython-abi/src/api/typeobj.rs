//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{Py_TPFLAGS_READY, Py_ssize_t, PyObject, PyTypeObject};
use std::os::raw::c_int;
use std::ptr;

/// Mark a type as ready for use.
/// In Molt's bridge, static type objects are pre-initialized; heap types
/// need basic tp_base resolution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Ready(tp: *mut PyTypeObject) -> c_int {
    if tp.is_null() {
        return -1;
    }
    unsafe {
        // Set tp_base to object if not set.
        if (*tp).tp_base.is_null() {
            // Leave null — we don't have PyBaseObject_Type in bridge.
        }
        // Mark ready.
        (*tp).tp_flags |= Py_TPFLAGS_READY;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericAlloc(
    tp: *mut PyTypeObject,
    _nitems: Py_ssize_t,
) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    // Allocate basic size + nitems * itemsize.
    // For now, allocate a minimal PyObject header.
    let layout = std::alloc::Layout::from_size_align(
        std::mem::size_of::<PyObject>(),
        std::mem::align_of::<PyObject>(),
    )
    .expect("layout");
    let raw = unsafe { std::alloc::alloc_zeroed(layout) };
    if raw.is_null() {
        return ptr::null_mut();
    }
    let obj = raw as *mut PyObject;
    unsafe {
        (*obj).ob_refcnt = 1;
        (*obj).ob_type = tp;
    }
    obj
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericNew(
    tp: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyType_GenericAlloc(tp, 0) }
}

/// Py_TYPE(op) — return ob_type pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_TYPE(op: *mut PyObject) -> *mut PyTypeObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*op).ob_type }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_TypeCheck(op: *mut PyObject, tp: *mut PyTypeObject) -> c_int {
    if op.is_null() || tp.is_null() {
        return 0;
    }
    let actual = unsafe { (*op).ob_type };
    std::ptr::eq(actual, tp) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsInstance(inst: *mut PyObject, cls: *mut PyObject) -> c_int {
    if inst.is_null() || cls.is_null() {
        return 0;
    }
    // Check whether inst's type pointer matches cls (exact type match).
    // This does not walk the MRO — full isinstance() requires the Molt runtime.
    // Returning -1 (error) would be worse than a conservative match, so we
    // check the one thing we *can* check: pointer identity of ob_type.
    let inst_type = unsafe { (*inst).ob_type };
    if inst_type.is_null() {
        return 0;
    }
    if std::ptr::eq(inst_type as *const PyObject, cls) {
        return 1;
    }
    // Cannot determine — return 0 (not an instance) rather than lying.
    // Extensions that hit this path get a false negative, which is safer than
    // a false positive.  Log via bridge tracing if available.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCallable_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    // Check if the object's type has tp_call set — the CPython definition of
    // "callable".  Without tp_call we cannot determine callability from the
    // bridge alone, but checking it is strictly better than always returning 0,
    // which caused extensions to wrongly reject callable objects.
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return 0;
    }
    if unsafe { (*tp).tp_call }.is_some() {
        return 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    // Try tp_hash first.
    let tp = unsafe { (*op).ob_type };
    if !tp.is_null() {
        if let Some(hash_fn) = unsafe { (*tp).tp_hash } {
            return unsafe { hash_fn(op) };
        }
    }
    op as isize // pointer-based hash as last resort
}

// ─── PyType subtype / flags / name ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_IsSubtype(
    a: *mut PyTypeObject,
    b: *mut PyTypeObject,
) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    if std::ptr::eq(a, b) {
        return 1;
    }
    // Walk tp_base chain.
    let mut cursor = a;
    while !cursor.is_null() {
        if std::ptr::eq(cursor, b) {
            return 1;
        }
        cursor = unsafe { (*cursor).tp_base };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetFlags(tp: *mut PyTypeObject) -> std::os::raw::c_ulong {
    if tp.is_null() {
        return 0;
    }
    unsafe { (*tp).tp_flags }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetName(tp: *mut PyTypeObject) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    let name_ptr = unsafe { (*tp).tp_name };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::strings::PyUnicode_FromString(name_ptr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetQualName(tp: *mut PyTypeObject) -> *mut PyObject {
    // For our purposes, qualname == name.
    unsafe { PyType_GetName(tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_HasFeature(
    tp: *mut PyTypeObject,
    feature: std::os::raw::c_ulong,
) -> c_int {
    if tp.is_null() {
        return 0;
    }
    (unsafe { (*tp).tp_flags } & feature != 0) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Repr(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::strings::PyUnicode_FromString(c"<molt object>".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Str(op: *mut PyObject) -> *mut PyObject {
    unsafe { PyObject_Repr(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // Try tp_richcompare on v's type first, then w's type (reflected).
    if !v.is_null() {
        let tp = unsafe { (*v).ob_type };
        if !tp.is_null() {
            if let Some(richcmp) = unsafe { (*tp).tp_richcompare } {
                let result = unsafe { richcmp(v, w, op) };
                if !result.is_null()
                    && !std::ptr::eq(result, &raw mut crate::abi_types::Py_NotImplementedSentinel)
                {
                    return result;
                }
            }
        }
    }
    // Return NotImplemented sentinel — callers must check for this.
    &raw mut crate::abi_types::Py_NotImplementedSentinel
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompareBool(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> c_int {
    let result = unsafe { PyObject_RichCompare(v, w, op) };
    if result.is_null() {
        return -1;
    }
    if std::ptr::eq(result, &raw mut crate::abi_types::Py_NotImplementedSentinel) {
        // Comparison not supported — for Py_EQ/Py_NE fall back to pointer
        // identity (CPython semantics for unsupported comparisons).
        const PY_EQ: c_int = 2;
        const PY_NE: c_int = 3;
        return match op {
            PY_EQ => std::ptr::eq(v, w) as c_int,
            PY_NE => !std::ptr::eq(v, w) as c_int,
            _ => -1, // cannot compare: error
        };
    }
    // Truthy check: Py_True → 1, Py_False → 0, Py_None → 0
    if std::ptr::eq(result, &raw mut crate::abi_types::Py_True) {
        1
    } else if std::ptr::eq(result, &raw mut crate::abi_types::Py_False) {
        0
    } else if std::ptr::eq(result, &raw mut crate::abi_types::Py_None) {
        0
    } else {
        // Non-null, non-sentinel result — treat as truthy.
        1
    }
}
