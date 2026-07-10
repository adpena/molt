//! Sequence abstract protocol — PySequence_* operations.
//!
//! Faithful to CPython 3.12 `Objects/abstract.c`: every function keeps a
//! zero-overhead Molt-native tier (list/tuple/str/bytes via the runtime hooks),
//! then a foreign type-slot tier (`tp_as_sequence` dispatch, mirroring the
//! `foreign_get_item` pattern in `api/object.rs`), then an iterator fallback
//! where CPython has one — and every error return carries the CPython-shaped
//! exception (the pre-sweep code returned bare `-1`/NULL sentinels and
//! fabricated empty results; see the divergence ledger rows for this file).

use crate::abi_types::{Py_ssize_t, PyObject, PySequenceMethods, PyTupleObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

/// CPython `Include/object.h` rich-comparison opcode `Py_EQ` (the flag
/// constants live only in the C header tier, per abi_types).
const PY_EQ: c_int = 2;

type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
type SsizeArgFunc = unsafe extern "C" fn(*mut PyObject, Py_ssize_t) -> *mut PyObject;
type BinaryFunc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
type SsizeObjArgProc = unsafe extern "C" fn(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int;
type ObjObjProc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> c_int;

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(op)
}

/// Helper: classify a heap-pointer handle.
fn classify(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    if !obj.is_ptr() {
        return crate::abi_types::MoltTypeTag::Other as u8;
    }
    let h = hooks_or_stubs();
    unsafe { (h.classify_heap)(bits) }
}

#[inline]
fn tag_list() -> u8 {
    crate::abi_types::MoltTypeTag::List as u8
}
#[inline]
fn tag_tuple() -> u8 {
    crate::abi_types::MoltTypeTag::Tuple as u8
}
#[inline]
fn tag_str() -> u8 {
    crate::abi_types::MoltTypeTag::Str as u8
}
#[inline]
fn tag_bytes() -> u8 {
    crate::abi_types::MoltTypeTag::Bytes as u8
}
#[inline]
fn tag_dict() -> u8 {
    crate::abi_types::MoltTypeTag::Dict as u8
}

/// Set a `TypeError` with a formatted message, unless an exception is already
/// pending (never mask the more specific inner error).
unsafe fn set_type_error(message: String) {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return;
    }
    if let Ok(cmsg) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmsg.as_ptr(),
            );
        }
    }
}

/// CPython `null_error()`: SystemError for a NULL argument to an internal
/// routine.
unsafe fn set_null_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_SystemError,
            c"null argument to internal routine".as_ptr(),
        );
    }
}

unsafe fn type_name(o: *mut PyObject) -> String {
    unsafe { crate::api::object::type_name_lossy(o) }
}

/// The object's `tp_as_sequence` slot table, if any (foreign C objects and any
/// ABI-layout type; a bridge proxy's static type usually has none).
unsafe fn seq_methods(o: *mut PyObject) -> Option<*mut PySequenceMethods> {
    if o.is_null() {
        return None;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return None;
    }
    let m = unsafe { (*tp).tp_as_sequence }.cast::<PySequenceMethods>();
    if m.is_null() { None } else { Some(m) }
}

/// Non-null `mp_subscript` presence (for CPython's "is not a sequence" vs
/// "does not support indexing" message split).
unsafe fn has_mp_subscript(o: *mut PyObject) -> bool {
    if o.is_null() {
        return false;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return false;
    }
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    !m.is_null() && !unsafe { (*m).mp_subscript }.is_null()
}

/// Foreign `sq_length`, when present: calls the slot and returns its result.
unsafe fn foreign_sq_length(o: *mut PyObject) -> Option<Py_ssize_t> {
    let m = unsafe { seq_methods(o) }?;
    let f = unsafe { (*m).sq_length };
    if f.is_null() {
        return None;
    }
    let f: LenFunc = unsafe { std::mem::transmute::<*mut c_void, LenFunc>(f) };
    Some(unsafe { f(o) })
}

/// UTF-8 bytes of a native str handle (pointer valid until the next GC cycle).
unsafe fn str_slice(bits: u64) -> Option<&'static [u8]> {
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let p = unsafe { (h.str_data)(bits, &raw mut len) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

/// Raw bytes of a native bytes handle.
unsafe fn bytes_slice(bits: u64) -> Option<&'static [u8]> {
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let p = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(p, len) })
    }
}

/// Value equality between two Molt handles, replacing the raw bits-identity
/// compare the ledger flagged for Contains/Count/Index (equal-but-distinct heap
/// strings / big ints / floats were missed).
///
/// Tiers: bits identity (identity implies equality, as CPython's
/// `PyObject_RichCompareBool` shortcut — also makes NaN-is-item work) → exact
/// int/int → mixed int/float numeric → str bytes → bytes bytes. Residual:
/// equal-but-distinct heap containers (tuple vs tuple) still miss — CPython
/// recurses via rich comparison; the runtime exposes no deep-equality hook yet.
unsafe fn bits_value_eq(a: u64, b: u64) -> bool {
    if a == b {
        return true;
    }
    let oa = MoltObject::from_bits(a);
    let ob = MoltObject::from_bits(b);
    let h = hooks_or_stubs();
    // Integer value (immediate int/bool, or heap big-int via the checked hook).
    let as_i64 = |bits: u64, o: &MoltObject| -> Option<i64> {
        if let Some(v) = o.as_int() {
            return Some(v);
        }
        if let Some(v) = o.as_bool() {
            return Some(v as i64);
        }
        if o.is_ptr() && classify(bits) == crate::abi_types::MoltTypeTag::Int as u8 {
            let mut out: i64 = 0;
            if unsafe { (h.int_as_i64_checked)(bits, &raw mut out) } == 0 {
                return Some(out);
            }
        }
        None
    };
    let ia = as_i64(a, &oa);
    let ib = as_i64(b, &ob);
    if let (Some(x), Some(y)) = (ia, ib) {
        return x == y;
    }
    // Mixed numeric (int vs float, or float bit-patterns like 0.0 vs -0.0).
    let as_f64 = |i: Option<i64>, o: &MoltObject| -> Option<f64> {
        if let Some(v) = i {
            return Some(v as f64);
        }
        o.as_float()
    };
    if let (Some(x), Some(y)) = (as_f64(ia, &oa), as_f64(ib, &ob)) {
        return x == y;
    }
    if oa.is_ptr() && ob.is_ptr() {
        let (ta, tb) = (classify(a), classify(b));
        if ta == tag_str() && tb == tag_str() {
            return unsafe { str_slice(a) } == unsafe { str_slice(b) };
        }
        if ta == tag_bytes() && tb == tag_bytes() {
            return unsafe { bytes_slice(a) } == unsafe { bytes_slice(b) };
        }
    }
    false
}

unsafe fn is_abi_tuple_object(o: *mut PyObject) -> bool {
    !o.is_null()
        && unsafe { crate::api::sequences::PyTuple_Check(o) } != 0
        && resolve_bits(o).is_none()
}

/// Append the code points of a native str handle to `out` as fresh 1-char str
/// handles (CPython `str` iteration yields 1-char strings).
unsafe fn push_str_chars(bits: u64, out: &mut Vec<u64>) -> bool {
    let Some(bytes) = (unsafe { str_slice(bits) }) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let h = hooks_or_stubs();
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let s = ch.encode_utf8(&mut buf);
        let cb = unsafe { (h.alloc_str)(s.as_ptr(), s.len()) };
        if cb == 0 {
            return false;
        }
        out.push(cb);
    }
    true
}

/// Append the elements of a native bytes handle to `out` as int handles
/// (CPython `bytes` iteration yields ints).
unsafe fn push_bytes_ints(bits: u64, out: &mut Vec<u64>) -> bool {
    let Some(bytes) = (unsafe { bytes_slice(bits) }) else {
        return false;
    };
    let h = hooks_or_stubs();
    for &b in bytes {
        let ib = unsafe { (h.int_from_i64)(b as i64) };
        if ib == 0 {
            return false;
        }
        out.push(ib);
    }
    true
}

/// Materialize any iterable into a vector of owned Molt handle bits, or None
/// with the CPython-shaped exception set.
///
/// Tiers: native list/tuple (direct index copy) → native str/bytes (element
/// semantics per CPython) → native dict (keys, via the `dict_entry` cursor) →
/// the object's own iterator protocol (`PyObject_GetIter` + `PyIter_Next`, the
/// CPython fallback for every other iterable) → TypeError.
unsafe fn materialize_iterable(o: *mut PyObject) -> Option<Vec<u64>> {
    let h = hooks_or_stubs();
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_list() {
            let len = unsafe { (h.list_len)(bits) };
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let item = unsafe { (h.list_item)(bits, i) };
                if item == 0 {
                    break;
                }
                out.push(item);
            }
            return Some(out);
        }
        if tag == tag_tuple() {
            let len = unsafe { (h.tuple_len)(bits) };
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let item = unsafe { (h.tuple_item)(bits, i) };
                if item == 0 {
                    break;
                }
                out.push(item);
            }
            return Some(out);
        }
        if tag == tag_str() {
            let mut out = Vec::new();
            if unsafe { push_str_chars(bits, &mut out) } {
                return Some(out);
            }
        }
        if tag == tag_bytes() {
            let mut out = Vec::new();
            if unsafe { push_bytes_ints(bits, &mut out) } {
                return Some(out);
            }
        }
        if tag == tag_dict() {
            // Iterating a dict yields its KEYS (CPython dict iteration order).
            let mut out = Vec::new();
            let mut idx: usize = 0;
            loop {
                let mut key: u64 = 0;
                let found =
                    unsafe { (h.dict_entry)(bits, idx, &raw mut key, ptr::null_mut()) };
                if found != 1 {
                    break;
                }
                out.push(key);
                idx += 1;
            }
            return Some(out);
        }
    }
    // Iterator-protocol fallback (foreign objects and slot-bearing types).
    let iter = unsafe { crate::api::object::PyObject_GetIter(o) };
    if iter.is_null() {
        unsafe { set_type_error(format!("'{}' object is not iterable", type_name(o))) };
        return None;
    }
    let mut out = Vec::new();
    loop {
        let item = unsafe { crate::api::object::PyIter_Next(iter) };
        if item.is_null() {
            break;
        }
        let mut bridge = GLOBAL_BRIDGE.lock();
        let item_bits = match unsafe { bridge.molt_value_for_pyobj(item) } {
            Some(b) => b,
            None => {
                drop(bridge);
                unsafe {
                    crate::api::refcount::Py_DECREF(item);
                    crate::api::refcount::Py_DECREF(iter);
                    set_type_error(
                        "iterator produced an object that cannot enter the Molt runtime"
                            .to_string(),
                    );
                }
                return None;
            }
        };
        drop(bridge);
        out.push(item_bits);
        unsafe { crate::api::refcount::Py_DECREF(item) };
    }
    unsafe { crate::api::refcount::Py_DECREF(iter) };
    // A pending exception here is a real iteration error, not exhaustion.
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return None;
    }
    Some(out)
}

unsafe fn set_sequence_fast_type_error(message: *const c_char) {
    let msg = if message.is_null() {
        c"object is not a sequence".as_ptr()
    } else {
        // Validate that the caller supplied a C string before handing it to
        // the shared error state.
        let _ = unsafe { CStr::from_ptr(message) };
        message
    };
    unsafe { crate::api::errors::PyErr_SetString(&raw mut crate::abi_types::PyExc_TypeError, msg) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Size(o: *mut PyObject) -> Py_ssize_t {
    // CPython: sq_length when present; a mapping without sq_length is "%.200s
    // is not a sequence"; anything else "object of type '%.200s' has no
    // len()". Every -1 carries an exception (sentinel sweep).
    if o.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        if tag == tag_list() {
            return unsafe { (h.list_len)(bits) as Py_ssize_t };
        }
        if tag == tag_tuple() {
            return unsafe { (h.tuple_len)(bits) as Py_ssize_t };
        }
        if tag == tag_str() {
            // len(str) counts CODE POINTS, not UTF-8 bytes.
            if let Some(bytes) = unsafe { str_slice(bits) }
                && let Ok(text) = std::str::from_utf8(bytes)
            {
                return text.chars().count() as Py_ssize_t;
            }
        }
        if tag == tag_bytes()
            && let Some(bytes) = unsafe { bytes_slice(bits) }
        {
            return bytes.len() as Py_ssize_t;
        }
        if tag == tag_dict() {
            // dict has mp_length but NO sq_length: CPython raises the
            // "is not a sequence" TypeError here, never returns the length.
            unsafe { set_type_error(format!("{} is not a sequence", type_name(o))) };
            return -1;
        }
    }
    // Foreign tier: dispatch the object's own sq_length.
    if let Some(n) = unsafe { foreign_sq_length(o) } {
        return n;
    }
    unsafe {
        if has_mp_subscript(o) {
            set_type_error(format!("{} is not a sequence", type_name(o)));
        } else {
            set_type_error(format!("object of type '{}' has no len()", type_name(o)));
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_GetItem(o: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);

        if tag == tag_list() {
            let len = unsafe { (h.list_len)(bits) };
            let actual_i = if i < 0 { len as Py_ssize_t + i } else { i };
            if actual_i < 0 || actual_i >= len as Py_ssize_t {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_IndexError,
                        c"list index out of range".as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            let item_bits = unsafe { (h.list_item)(bits, actual_i as usize) };
            if item_bits == 0 {
                return ptr::null_mut();
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
        }
        if tag == tag_tuple() {
            let len = unsafe { (h.tuple_len)(bits) };
            let actual_i = if i < 0 { len as Py_ssize_t + i } else { i };
            if actual_i < 0 || actual_i >= len as Py_ssize_t {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_IndexError,
                        c"tuple index out of range".as_ptr(),
                    );
                }
                return ptr::null_mut();
            }
            let item_bits = unsafe { (h.tuple_item)(bits, actual_i as usize) };
            if item_bits == 0 {
                return ptr::null_mut();
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
        }
        if tag == tag_str() {
            // str sq_item yields a 1-code-point str (code-point indexing).
            if let Some(bytes) = unsafe { str_slice(bits) }
                && let Ok(text) = std::str::from_utf8(bytes)
            {
                let n = text.chars().count() as Py_ssize_t;
                let actual_i = if i < 0 { n + i } else { i };
                if actual_i < 0 || actual_i >= n {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_IndexError,
                            c"string index out of range".as_ptr(),
                        );
                    }
                    return ptr::null_mut();
                }
                if let Some(ch) = text.chars().nth(actual_i as usize) {
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    let cb = unsafe { (h.alloc_str)(s.as_ptr(), s.len()) };
                    if cb != 0 {
                        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(cb) };
                    }
                }
                return ptr::null_mut();
            }
        }
        if tag == tag_bytes() {
            // bytes sq_item yields an int in [0, 256).
            if let Some(bytes) = unsafe { bytes_slice(bits) } {
                let n = bytes.len() as Py_ssize_t;
                let actual_i = if i < 0 { n + i } else { i };
                if actual_i < 0 || actual_i >= n {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_IndexError,
                            c"index out of range".as_ptr(),
                        );
                    }
                    return ptr::null_mut();
                }
                let ib = unsafe { (h.int_from_i64)(bytes[actual_i as usize] as i64) };
                if ib != 0 {
                    return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(ib) };
                }
                return ptr::null_mut();
            }
        }
    }
    // Foreign tier: sq_item with CPython's negative-index adjustment.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_item = unsafe { (*m).sq_item };
        if !sq_item.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return ptr::null_mut();
                }
                idx += l;
            }
            let f: SsizeArgFunc = unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(sq_item) };
            return unsafe { f(o, idx) };
        }
    }
    unsafe {
        if has_mp_subscript(o) {
            set_type_error(format!("{} is not a sequence", type_name(o)));
        } else {
            set_type_error(format!(
                "'{}' object does not support indexing",
                type_name(o)
            ));
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_SetItem(
    o: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    // CPython sequence_setitem: dispatch sq_ass_item with negative-index
    // adjustment; types without it (tuple, str, bytes) raise TypeError. NOTE:
    // unlike PyList_SetItem this does NOT steal the reference to v.
    if o.is_null() || v.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_list() {
            let h = hooks_or_stubs();
            let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
            let actual_i = if i < 0 { len + i } else { i };
            // PyList_SetItem steals; take an extra reference to preserve the
            // non-stealing sq_ass_item contract on success AND error paths.
            unsafe { crate::api::refcount::Py_INCREF(v) };
            return unsafe { crate::api::sequences::PyList_SetItem(o, actual_i, v) };
        }
        if tag == tag_tuple() || tag == tag_str() || tag == tag_bytes() {
            // CPython: these types have no sq_ass_item — TypeError, not the
            // previous silent tuple mutation / bare -1.
            unsafe {
                set_type_error(format!(
                    "'{}' object does not support item assignment",
                    type_name(o)
                ));
            }
            return -1;
        }
    }
    // Foreign tier: sq_ass_item(o, i, v).
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_ass = unsafe { (*m).sq_ass_item };
        if !sq_ass.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return -1;
                }
                idx += l;
            }
            let f: SsizeObjArgProc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
            return unsafe { f(o, idx, v) };
        }
    }
    unsafe {
        set_type_error(format!(
            "'{}' object does not support item assignment",
            type_name(o)
        ));
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_DelItem(o: *mut PyObject, i: Py_ssize_t) -> c_int {
    // CPython: sq_ass_item(o, i, NULL) deletes. Native list deletion routes
    // through the list_set_slice splice authority (the previous body was an
    // unconditional silent -1).
    if o.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_list() {
            let h = hooks_or_stubs();
            let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
            let actual_i = if i < 0 { len + i } else { i };
            if actual_i < 0 || actual_i >= len {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_IndexError,
                        c"list assignment index out of range".as_ptr(),
                    );
                }
                return -1;
            }
            let rc = unsafe { (h.list_set_slice)(bits, actual_i, actual_i + 1, 0) };
            if rc != 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return -1;
            }
            return 0;
        }
    }
    // Foreign tier: sq_ass_item(o, i, NULL).
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_ass = unsafe { (*m).sq_ass_item };
        if !sq_ass.is_null() {
            let mut idx = i;
            if idx < 0
                && let Some(l) = unsafe { foreign_sq_length(o) }
            {
                if l < 0 {
                    return -1;
                }
                idx += l;
            }
            let f: SsizeObjArgProc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeObjArgProc>(sq_ass) };
            return unsafe { f(o, idx, ptr::null_mut()) };
        }
    }
    unsafe {
        set_type_error(format!(
            "'{}' object doesn't support item deletion",
            type_name(o)
        ));
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Contains(o: *mut PyObject, value: *mut PyObject) -> c_int {
    // CPython: sq_contains, else iterator search with Py_EQ VALUE equality.
    // The pre-fix scan compared raw handle bits (equal-but-distinct heap
    // strings/ints always missed) and returned silent -1 for anything else.
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        let val_bits = resolve_bits(value);

        if tag == tag_list() || tag == tag_tuple() {
            if let Some(val_bits) = val_bits {
                let len = if tag == tag_list() {
                    unsafe { (h.list_len)(bits) }
                } else {
                    unsafe { (h.tuple_len)(bits) }
                };
                for idx in 0..len {
                    let item = if tag == tag_list() {
                        unsafe { (h.list_item)(bits, idx) }
                    } else {
                        unsafe { (h.tuple_item)(bits, idx) }
                    };
                    if unsafe { bits_value_eq(item, val_bits) } {
                        return 1;
                    }
                }
                return 0;
            }
            // A foreign needle can only match by identity in a Molt container;
            // its wrapper (if any) was checked above. Not present.
            return 0;
        }
        if tag == tag_str() {
            // 'in <string>' is SUBSTRING containment and requires a str operand.
            if let Some(vb) = val_bits
                && classify(vb) == tag_str()
                && let (Some(hay), Some(needle)) =
                    (unsafe { str_slice(bits) }, unsafe { str_slice(vb) })
            {
                if let (Ok(hay), Ok(needle)) =
                    (std::str::from_utf8(hay), std::str::from_utf8(needle))
                {
                    return hay.contains(needle) as c_int;
                }
                return 0;
            }
            unsafe {
                set_type_error(format!(
                    "'in <string>' requires string as left operand, not {}",
                    type_name(value)
                ));
            }
            return -1;
        }
        if tag == tag_bytes()
            && let Some(hay) = unsafe { bytes_slice(bits) }
        {
            if let Some(vb) = val_bits {
                // int in bytes: byte-value membership.
                if let Some(iv) = MoltObject::from_bits(vb).as_int() {
                    if !(0..=255).contains(&iv) {
                        unsafe {
                            crate::api::errors::PyErr_SetString(
                                &raw mut crate::abi_types::PyExc_ValueError,
                                c"byte must be in range(0, 256)".as_ptr(),
                            );
                        }
                        return -1;
                    }
                    return hay.contains(&(iv as u8)) as c_int;
                }
                // bytes in bytes: sub-slice containment.
                if classify(vb) == tag_bytes()
                    && let Some(needle) = unsafe { bytes_slice(vb) }
                {
                    if needle.is_empty() {
                        return 1;
                    }
                    return hay.windows(needle.len()).any(|w| w == needle) as c_int;
                }
            }
            unsafe {
                set_type_error(format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(value)
                ));
            }
            return -1;
        }
        if tag == tag_dict() {
            // `x in dict` is a key lookup through the runtime dict authority.
            if let Some(val_bits) = val_bits {
                let result = unsafe { (h.dict_get)(bits, val_bits) };
                return (result != 0) as c_int;
            }
            return 0;
        }
    }
    // Foreign tier: sq_contains, else the iterator search CPython falls to.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_contains = unsafe { (*m).sq_contains };
        if !sq_contains.is_null() {
            let f: ObjObjProc =
                unsafe { std::mem::transmute::<*mut c_void, ObjObjProc>(sq_contains) };
            return unsafe { f(o, value) };
        }
    }
    unsafe { iter_search(o, value, IterSearch::Contains) as c_int }
}

/// CPython `_PySequence_IterSearch` operations.
enum IterSearch {
    Contains,
    Count,
    Index,
}

/// Iterator-protocol search: drains `PyObject_GetIter(o)` comparing each item
/// to `value` with `PyObject_RichCompareBool(..., Py_EQ)`. Returns the CPython
/// result contract for each mode (-1 with an exception on error).
unsafe fn iter_search(o: *mut PyObject, value: *mut PyObject, mode: IterSearch) -> Py_ssize_t {
    let iter = unsafe { crate::api::object::PyObject_GetIter(o) };
    if iter.is_null() {
        unsafe {
            set_type_error(format!(
                "argument of type '{}' is not iterable",
                type_name(o)
            ));
        }
        return -1;
    }
    let mut count: Py_ssize_t = 0;
    let mut index: Py_ssize_t = 0;
    let mut found: Py_ssize_t = -1;
    loop {
        let item = unsafe { crate::api::object::PyIter_Next(iter) };
        if item.is_null() {
            break;
        }
        let eq = unsafe { crate::api::typeobj::PyObject_RichCompareBool(item, value, PY_EQ) };
        unsafe { crate::api::refcount::Py_DECREF(item) };
        if eq < 0 {
            unsafe { crate::api::refcount::Py_DECREF(iter) };
            return -1;
        }
        if eq > 0 {
            match mode {
                IterSearch::Contains => {
                    unsafe { crate::api::refcount::Py_DECREF(iter) };
                    return 1;
                }
                IterSearch::Count => count += 1,
                IterSearch::Index => {
                    if found < 0 {
                        found = index;
                        unsafe { crate::api::refcount::Py_DECREF(iter) };
                        return found;
                    }
                }
            }
        }
        index += 1;
    }
    unsafe { crate::api::refcount::Py_DECREF(iter) };
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return -1;
    }
    match mode {
        IterSearch::Contains => 0,
        IterSearch::Count => count,
        IterSearch::Index => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_ValueError,
                    c"sequence.index(x): x not in sequence".as_ptr(),
                );
            }
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Concat(s1: *mut PyObject, s2: *mut PyObject) -> *mut PyObject {
    if s1.is_null() || s2.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let bits1 = resolve_bits(s1);
    let bits2 = resolve_bits(s2);
    if let Some(bits1) = bits1 {
        let h = hooks_or_stubs();
        let tag1 = classify(bits1);
        let tag2 = bits2.map(classify);

        if tag1 == tag_list() {
            // CPython list_concat: the right operand must be a list.
            if tag2 != Some(tag_list()) {
                unsafe {
                    set_type_error(format!(
                        "can only concatenate list (not \"{}\") to list",
                        type_name(s2)
                    ));
                }
                return ptr::null_mut();
            }
            let bits2 = bits2.unwrap();
            let new_list = unsafe { (h.alloc_list)() };
            if new_list == 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
            let len1 = unsafe { (h.list_len)(bits1) };
            for i in 0..len1 {
                let item = unsafe { (h.list_item)(bits1, i) };
                unsafe { (h.list_append)(new_list, item) };
            }
            let len2 = unsafe { (h.list_len)(bits2) };
            for i in 0..len2 {
                let item = unsafe { (h.list_item)(bits2, i) };
                unsafe { (h.list_append)(new_list, item) };
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) };
        }
        if tag1 == tag_tuple() {
            // CPython tuple_concat: the right operand must be a tuple.
            if tag2 != Some(tag_tuple()) {
                unsafe {
                    set_type_error(format!(
                        "can only concatenate tuple (not \"{}\") to tuple",
                        type_name(s2)
                    ));
                }
                return ptr::null_mut();
            }
            let bits2 = bits2.unwrap();
            let len1 = unsafe { (h.tuple_len)(bits1) };
            let len2 = unsafe { (h.tuple_len)(bits2) };
            let new_tuple = unsafe { (h.alloc_tuple)(len1 + len2) };
            if new_tuple == 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
            for i in 0..len1 {
                let item = unsafe { (h.tuple_item)(bits1, i) };
                unsafe { (h.tuple_set)(new_tuple, i, item) };
            }
            for i in 0..len2 {
                let item = unsafe { (h.tuple_item)(bits2, i) };
                unsafe { (h.tuple_set)(new_tuple, len1 + i, item) };
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) };
        }
        if tag1 == tag_str() {
            if tag2 == Some(tag_str())
                && let (Some(a), Some(b)) = (
                    unsafe { str_slice(bits1) },
                    unsafe { str_slice(bits2.unwrap()) },
                )
            {
                let mut joined = Vec::with_capacity(a.len() + b.len());
                joined.extend_from_slice(a);
                joined.extend_from_slice(b);
                let nb = unsafe { (h.alloc_str)(joined.as_ptr(), joined.len()) };
                if nb != 0 {
                    return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(nb) };
                }
                return ptr::null_mut();
            }
            unsafe {
                set_type_error(format!(
                    "can only concatenate str (not \"{}\") to str",
                    type_name(s2)
                ));
            }
            return ptr::null_mut();
        }
        if tag1 == tag_bytes() {
            if tag2 == Some(tag_bytes())
                && let (Some(a), Some(b)) = (
                    unsafe { bytes_slice(bits1) },
                    unsafe { bytes_slice(bits2.unwrap()) },
                )
            {
                let mut joined = Vec::with_capacity(a.len() + b.len());
                joined.extend_from_slice(a);
                joined.extend_from_slice(b);
                let nb = unsafe { (h.alloc_bytes)(joined.as_ptr(), joined.len()) };
                if nb != 0 {
                    return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(nb) };
                }
                return ptr::null_mut();
            }
            unsafe {
                set_type_error(format!(
                    "can't concat {} to bytes",
                    type_name(s2)
                ));
            }
            return ptr::null_mut();
        }
    }
    // Foreign tier: sq_concat.
    if let Some(m) = unsafe { seq_methods(s1) } {
        let sq_concat = unsafe { (*m).sq_concat };
        if !sq_concat.is_null() {
            let f: BinaryFunc =
                unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(sq_concat) };
            return unsafe { f(s1, s2) };
        }
    }
    unsafe {
        set_type_error(format!(
            "'{}' object can't be concatenated",
            type_name(s1)
        ));
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Repeat(o: *mut PyObject, count: Py_ssize_t) -> *mut PyObject {
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let reps = count.max(0) as usize; // CPython clamps negative counts to 0
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);

        if tag == tag_list() {
            let len = unsafe { (h.list_len)(bits) };
            let new_list = unsafe { (h.alloc_list)() };
            if new_list == 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
            for _ in 0..reps {
                for i in 0..len {
                    let item = unsafe { (h.list_item)(bits, i) };
                    unsafe { (h.list_append)(new_list, item) };
                }
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) };
        }
        if tag == tag_tuple() {
            let len = unsafe { (h.tuple_len)(bits) };
            let new_tuple = unsafe { (h.alloc_tuple)(len * reps) };
            if new_tuple == 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
            let mut dst = 0;
            for _ in 0..reps {
                for i in 0..len {
                    let item = unsafe { (h.tuple_item)(bits, i) };
                    unsafe { (h.tuple_set)(new_tuple, dst, item) };
                    dst += 1;
                }
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) };
        }
        if tag == tag_str()
            && let Some(a) = unsafe { str_slice(bits) }
        {
            let repeated = a.repeat(reps);
            let nb = unsafe { (h.alloc_str)(repeated.as_ptr(), repeated.len()) };
            if nb != 0 {
                return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(nb) };
            }
            return ptr::null_mut();
        }
        if tag == tag_bytes()
            && let Some(a) = unsafe { bytes_slice(bits) }
        {
            let repeated = a.repeat(reps);
            let nb = unsafe { (h.alloc_bytes)(repeated.as_ptr(), repeated.len()) };
            if nb != 0 {
                return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(nb) };
            }
            return ptr::null_mut();
        }
    }
    // Foreign tier: sq_repeat.
    if let Some(m) = unsafe { seq_methods(o) } {
        let sq_repeat = unsafe { (*m).sq_repeat };
        if !sq_repeat.is_null() {
            let f: SsizeArgFunc =
                unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(sq_repeat) };
            return unsafe { f(o, count) };
        }
    }
    unsafe { set_type_error(format!("'{}' object can't be repeated", type_name(o))) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_List(o: *mut PyObject) -> *mut PyObject {
    // CPython builds an empty list and drains the iterator into it via
    // _PyList_Extend — ANY iterable converts; a non-iterable raises TypeError.
    // The previous body fabricated an EMPTY list for every non-list/tuple
    // (theater row).
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let Some(items) = (unsafe { materialize_iterable(o) }) else {
        return ptr::null_mut();
    };
    let h = hooks_or_stubs();
    let new_list = unsafe { (h.alloc_list)() };
    if new_list == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    for item in items {
        unsafe { (h.list_append)(new_list, item) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Tuple(o: *mut PyObject) -> *mut PyObject {
    // CPython: PyObject_GetIter(v) drained into a tuple (tuple('abc') ==
    // ('a','b','c'), tuple(dict) == keys); non-iterable raises TypeError. The
    // previous body fabricated an EMPTY tuple for every non-list/tuple.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    let Some(items) = (unsafe { materialize_iterable(o) }) else {
        return ptr::null_mut();
    };
    let h = hooks_or_stubs();
    let new_tuple = unsafe { (h.alloc_tuple)(items.len()) };
    if new_tuple == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    for (i, item) in items.iter().enumerate() {
        unsafe { (h.tuple_set)(new_tuple, i, *item) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Count(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        if tag == tag_list() || tag == tag_tuple() {
            let val_bits = resolve_bits(value);
            let len = if tag == tag_list() {
                unsafe { (h.list_len)(bits) }
            } else {
                unsafe { (h.tuple_len)(bits) }
            };
            let mut count: Py_ssize_t = 0;
            if let Some(val_bits) = val_bits {
                for i in 0..len {
                    let item = if tag == tag_list() {
                        unsafe { (h.list_item)(bits, i) }
                    } else {
                        unsafe { (h.tuple_item)(bits, i) }
                    };
                    // VALUE equality, not the pre-fix raw bits identity.
                    if unsafe { bits_value_eq(item, val_bits) } {
                        count += 1;
                    }
                }
            }
            return count;
        }
    }
    // CPython: _PySequence_IterSearch(COUNT) over any iterable.
    unsafe { iter_search(o, value, IterSearch::Count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Index(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let h = hooks_or_stubs();
        let tag = classify(bits);
        if tag == tag_list() || tag == tag_tuple() {
            let val_bits = resolve_bits(value);
            let len = if tag == tag_list() {
                unsafe { (h.list_len)(bits) }
            } else {
                unsafe { (h.tuple_len)(bits) }
            };
            if let Some(val_bits) = val_bits {
                for i in 0..len {
                    let item = if tag == tag_list() {
                        unsafe { (h.list_item)(bits, i) }
                    } else {
                        unsafe { (h.tuple_item)(bits, i) }
                    };
                    if unsafe { bits_value_eq(item, val_bits) } {
                        return i as Py_ssize_t;
                    }
                }
            }
            // CPython raises ValueError when absent — never a bare -1 (the
            // pre-fix silent sentinel was indistinguishable from an error).
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_ValueError,
                    c"sequence.index(x): x not in sequence".as_ptr(),
                );
            }
            return -1;
        }
    }
    unsafe { iter_search(o, value, IterSearch::Index) }
}

// ─── PySequence_Check ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Check(o: *mut PyObject) -> c_int {
    // CPython: 0 for dicts, else `tp_as_sequence && sq_item != NULL` — a
    // foreign C sequence (ndarray) must report 1 (the pre-fix hardcoded 0 was
    // the MISSING_DISPATCH row).
    if o.is_null() {
        return 0;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_dict() {
            return 0;
        }
        if tag == tag_list() || tag == tag_tuple() || tag == tag_str() || tag == tag_bytes() {
            return 1;
        }
    }
    match unsafe { seq_methods(o) } {
        Some(m) => (!unsafe { (*m).sq_item }.is_null()) as c_int,
        None => 0,
    }
}

// ─── PySequence_Fast — fast access to list/tuple items ───────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast(
    o: *mut PyObject,
    msg: *const std::os::raw::c_char,
) -> *mut PyObject {
    // CPython accepts ANY iterable (list/tuple fast path, else the iterator
    // protocol). The ABI materializes into an ABI-layout tuple so that
    // PySequence_Fast_ITEMS has a real C array to expose.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if unsafe { is_abi_tuple_object(o) } {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    let Some(items) = (unsafe { materialize_iterable(o) }) else {
        // materialize_iterable set a TypeError; override with the caller's
        // message (CPython substitutes `m` for the not-iterable TypeError).
        unsafe {
            crate::api::errors::PyErr_Clear();
            set_sequence_fast_type_error(msg);
        }
        return ptr::null_mut();
    };
    let tuple = unsafe { crate::api::sequences::PyTuple_New(items.len() as Py_ssize_t) };
    if tuple.is_null() {
        return ptr::null_mut();
    }
    for (index, item_bits) in items.iter().enumerate() {
        let item = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(*item_bits) };
        if item.is_null()
            || unsafe {
                crate::api::sequences::PyTuple_SetItem(tuple, index as Py_ssize_t, item)
            } != 0
        {
            unsafe { crate::api::refcount::Py_DECREF(tuple) };
            return ptr::null_mut();
        }
    }
    tuple
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_SIZE(o: *mut PyObject) -> Py_ssize_t {
    if unsafe { crate::api::sequences::PyTuple_Check(o) } != 0 {
        return unsafe { crate::api::sequences::PyTuple_Size(o) };
    }
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_ITEM(
    o: *mut PyObject,
    i: Py_ssize_t,
) -> *mut PyObject {
    if unsafe { crate::api::sequences::PyTuple_Check(o) } != 0 {
        return unsafe { crate::api::sequences::PyTuple_GetItem(o, i) };
    }
    unsafe { PySequence_GetItem(o, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_ITEMS(o: *mut PyObject) -> *mut *mut PyObject {
    if !unsafe { is_abi_tuple_object(o) } {
        return ptr::null_mut();
    }
    let tuple = o.cast::<PyTupleObject>();
    unsafe { (*tuple).ob_item }
}

// ─── PySequence_InPlaceConcat / InPlaceRepeat ────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceConcat(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    // CPython prefers sq_inplace_concat: `list += seq` EXTENDS IN PLACE and
    // returns a new reference to the SAME object. The previous delegation to
    // Concat allocated a fresh list, so aliases never observed the extension.
    if o1.is_null() || o2.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits1) = resolve_bits(o1)
        && classify(bits1) == tag_list()
        && let Some(bits2) = resolve_bits(o2)
    {
        let tag2 = classify(bits2);
        if tag2 == tag_list() || tag2 == tag_tuple() {
            let h = hooks_or_stubs();
            let len2 = if tag2 == tag_list() {
                unsafe { (h.list_len)(bits2) }
            } else {
                unsafe { (h.tuple_len)(bits2) }
            };
            // Snapshot first: `lst += lst` must append the ORIGINAL elements.
            let mut incoming = Vec::with_capacity(len2);
            for i in 0..len2 {
                let item = if tag2 == tag_list() {
                    unsafe { (h.list_item)(bits2, i) }
                } else {
                    unsafe { (h.tuple_item)(bits2, i) }
                };
                incoming.push(item);
            }
            {
                // The receiving list takes its own reference per element
                // (CPython list_inplace_concat → list_extend INCREFs).
                let mut bridge = GLOBAL_BRIDGE.lock();
                for &item in &incoming {
                    if item != 0 && MoltObject::from_bits(item).is_ptr() {
                        let proxy = unsafe { bridge.handle_to_borrowed_pyobj(item) };
                        unsafe { crate::api::refcount::Py_INCREF(proxy) };
                    }
                }
            }
            for item in incoming {
                unsafe { (h.list_append)(bits1, item) };
            }
            unsafe { crate::api::refcount::Py_INCREF(o1) };
            return o1;
        }
    }
    // Foreign tier: sq_inplace_concat, then sq_concat.
    if let Some(m) = unsafe { seq_methods(o1) } {
        let slot = unsafe { (*m).sq_inplace_concat };
        if !slot.is_null() {
            let f: BinaryFunc = unsafe { std::mem::transmute::<*mut c_void, BinaryFunc>(slot) };
            return unsafe { f(o1, o2) };
        }
    }
    unsafe { PySequence_Concat(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceRepeat(
    o: *mut PyObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    // CPython prefers sq_inplace_repeat: `list *= n` mutates in place and
    // returns a new reference to the SAME object.
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o)
        && classify(bits) == tag_list()
    {
        let h = hooks_or_stubs();
        let len = unsafe { (h.list_len)(bits) };
        if count <= 0 {
            // `lst *= 0` empties in place.
            let rc = unsafe { (h.list_set_slice)(bits, 0, len as Py_ssize_t, 0) };
            if rc != 0 {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
                return ptr::null_mut();
            }
        } else if count > 1 {
            let snapshot: Vec<u64> =
                (0..len).map(|i| unsafe { (h.list_item)(bits, i) }).collect();
            {
                let mut bridge = GLOBAL_BRIDGE.lock();
                for &item in &snapshot {
                    if item != 0 && MoltObject::from_bits(item).is_ptr() {
                        let proxy = unsafe { bridge.handle_to_borrowed_pyobj(item) };
                        // (count-1) extra copies each take a reference.
                        for _ in 1..count {
                            unsafe { crate::api::refcount::Py_INCREF(proxy) };
                        }
                    }
                }
            }
            for _ in 1..count {
                for &item in &snapshot {
                    unsafe { (h.list_append)(bits, item) };
                }
            }
        }
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if let Some(m) = unsafe { seq_methods(o) } {
        let slot = unsafe { (*m).sq_inplace_repeat };
        if !slot.is_null() {
            let f: SsizeArgFunc = unsafe { std::mem::transmute::<*mut c_void, SsizeArgFunc>(slot) };
            return unsafe { f(o, count) };
        }
    }
    unsafe { PySequence_Repeat(o, count) }
}
