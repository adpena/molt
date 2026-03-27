//! Sequence abstract protocol — PySequence_* operations.
//!
//! These implement the abstract sequence operations that work on lists,
//! tuples, and other sequence-like objects.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Size(o: *mut PyObject) -> Py_ssize_t {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);
    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => unsafe {
            (h.list_len)(bits) as Py_ssize_t
        },
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => unsafe {
            (h.tuple_len)(bits) as Py_ssize_t
        },
        t if t == crate::abi_types::MoltTypeTag::Str as u8 => {
            let mut len: usize = 0;
            unsafe { (h.str_data)(bits, &raw mut len) };
            len as Py_ssize_t
        }
        t if t == crate::abi_types::MoltTypeTag::Bytes as u8 => {
            let mut len: usize = 0;
            unsafe { (h.bytes_data)(bits, &raw mut len) };
            len as Py_ssize_t
        }
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_GetItem(o: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);

    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
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
            unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
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
            unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
        }
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_SetItem(
    o: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    if o.is_null() || v.is_null() {
        return -1;
    }
    // Delegate to the list/tuple specific setitem if applicable.
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let tag = classify(bits);
    if tag == crate::abi_types::MoltTypeTag::List as u8 {
        return unsafe { crate::api::sequences::PyList_SetItem(o, i, v) };
    }
    if tag == crate::abi_types::MoltTypeTag::Tuple as u8 {
        return unsafe { crate::api::sequences::PyTuple_SetItem(o, i, v) };
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_DelItem(o: *mut PyObject, _i: Py_ssize_t) -> c_int {
    if o.is_null() {
        return -1;
    }
    // Deletion requires a list_del_item hook not yet available.
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Contains(o: *mut PyObject, value: *mut PyObject) -> c_int {
    if o.is_null() || value.is_null() {
        return -1;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let val_bits = match resolve_bits(value) {
        Some(b) => b,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);

    // Linear scan for list and tuple.
    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            let len = unsafe { (h.list_len)(bits) };
            for idx in 0..len {
                let item = unsafe { (h.list_item)(bits, idx) };
                if item == val_bits {
                    return 1;
                }
            }
            0
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            let len = unsafe { (h.tuple_len)(bits) };
            for idx in 0..len {
                let item = unsafe { (h.tuple_item)(bits, idx) };
                if item == val_bits {
                    return 1;
                }
            }
            0
        }
        t if t == crate::abi_types::MoltTypeTag::Dict as u8 => {
            let result = unsafe { (h.dict_get)(bits, val_bits) };
            (result != 0) as c_int
        }
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Concat(s1: *mut PyObject, s2: *mut PyObject) -> *mut PyObject {
    if s1.is_null() || s2.is_null() {
        return ptr::null_mut();
    }
    let bits1 = match resolve_bits(s1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let bits2 = match resolve_bits(s2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    let tag1 = classify(bits1);

    if tag1 == crate::abi_types::MoltTypeTag::List as u8 {
        let new_list = unsafe { (h.alloc_list)() };
        if new_list == 0 {
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
    // Tuple concat
    if tag1 == crate::abi_types::MoltTypeTag::Tuple as u8 {
        let len1 = unsafe { (h.tuple_len)(bits1) };
        let len2 = unsafe { (h.tuple_len)(bits2) };
        let new_tuple = unsafe { (h.alloc_tuple)(len1 + len2) };
        if new_tuple == 0 {
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
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Repeat(o: *mut PyObject, count: Py_ssize_t) -> *mut PyObject {
    if o.is_null() || count <= 0 {
        // Return empty sequence of same type.
        let bits = match resolve_bits(o) {
            Some(b) => b,
            None => return ptr::null_mut(),
        };
        let tag = classify(bits);
        let h = hooks_or_stubs();
        if tag == crate::abi_types::MoltTypeTag::List as u8 {
            let new_list = unsafe { (h.alloc_list)() };
            if new_list == 0 {
                return ptr::null_mut();
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) };
        }
        if tag == crate::abi_types::MoltTypeTag::Tuple as u8 {
            let new_tuple = unsafe { (h.alloc_tuple)(0) };
            if new_tuple == 0 {
                return ptr::null_mut();
            }
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) };
        }
        return ptr::null_mut();
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);

    if tag == crate::abi_types::MoltTypeTag::List as u8 {
        let len = unsafe { (h.list_len)(bits) };
        let new_list = unsafe { (h.alloc_list)() };
        if new_list == 0 {
            return ptr::null_mut();
        }
        for _ in 0..count {
            for i in 0..len {
                let item = unsafe { (h.list_item)(bits, i) };
                unsafe { (h.list_append)(new_list, item) };
            }
        }
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) };
    }
    if tag == crate::abi_types::MoltTypeTag::Tuple as u8 {
        let len = unsafe { (h.tuple_len)(bits) };
        let total = len * count as usize;
        let new_tuple = unsafe { (h.alloc_tuple)(total) };
        if new_tuple == 0 {
            return ptr::null_mut();
        }
        let mut dst = 0;
        for _ in 0..count {
            for i in 0..len {
                let item = unsafe { (h.tuple_item)(bits, i) };
                unsafe { (h.tuple_set)(new_tuple, dst, item) };
                dst += 1;
            }
        }
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_List(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);
    let new_list = unsafe { (h.alloc_list)() };
    if new_list == 0 {
        return ptr::null_mut();
    }

    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            let len = unsafe { (h.list_len)(bits) };
            for i in 0..len {
                let item = unsafe { (h.list_item)(bits, i) };
                unsafe { (h.list_append)(new_list, item) };
            }
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            let len = unsafe { (h.tuple_len)(bits) };
            for i in 0..len {
                let item = unsafe { (h.tuple_item)(bits, i) };
                unsafe { (h.list_append)(new_list, item) };
            }
        }
        _ => {
            // Cannot iterate — return empty list.
        }
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Tuple(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);

    match tag {
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            // Already a tuple — return a copy.
            let len = unsafe { (h.tuple_len)(bits) };
            let new_tuple = unsafe { (h.alloc_tuple)(len) };
            if new_tuple == 0 {
                return ptr::null_mut();
            }
            for i in 0..len {
                let item = unsafe { (h.tuple_item)(bits, i) };
                unsafe { (h.tuple_set)(new_tuple, i, item) };
            }
            unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) }
        }
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            let len = unsafe { (h.list_len)(bits) };
            let new_tuple = unsafe { (h.alloc_tuple)(len) };
            if new_tuple == 0 {
                return ptr::null_mut();
            }
            for i in 0..len {
                let item = unsafe { (h.list_item)(bits, i) };
                unsafe { (h.tuple_set)(new_tuple, i, item) };
            }
            unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) }
        }
        _ => {
            // Cannot iterate — return empty tuple.
            let new_tuple = unsafe { (h.alloc_tuple)(0) };
            if new_tuple == 0 {
                return ptr::null_mut();
            }
            unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Count(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        return -1;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let val_bits = match resolve_bits(value) {
        Some(b) => b,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);
    let mut count: Py_ssize_t = 0;

    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            let len = unsafe { (h.list_len)(bits) };
            for i in 0..len {
                if unsafe { (h.list_item)(bits, i) } == val_bits {
                    count += 1;
                }
            }
            count
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            let len = unsafe { (h.tuple_len)(bits) };
            for i in 0..len {
                if unsafe { (h.tuple_item)(bits, i) } == val_bits {
                    count += 1;
                }
            }
            count
        }
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Index(o: *mut PyObject, value: *mut PyObject) -> Py_ssize_t {
    if o.is_null() || value.is_null() {
        return -1;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let val_bits = match resolve_bits(value) {
        Some(b) => b,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let tag = classify(bits);

    match tag {
        t if t == crate::abi_types::MoltTypeTag::List as u8 => {
            let len = unsafe { (h.list_len)(bits) };
            for i in 0..len {
                if unsafe { (h.list_item)(bits, i) } == val_bits {
                    return i as Py_ssize_t;
                }
            }
            -1
        }
        t if t == crate::abi_types::MoltTypeTag::Tuple as u8 => {
            let len = unsafe { (h.tuple_len)(bits) };
            for i in 0..len {
                if unsafe { (h.tuple_item)(bits, i) } == val_bits {
                    return i as Py_ssize_t;
                }
            }
            -1
        }
        _ => -1,
    }
}

// ─── PySequence_Check ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Check(o: *mut PyObject) -> c_int {
    if o.is_null() {
        return 0;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return 0,
    };
    let tag = classify(bits);
    matches!(
        tag,
        t if t == crate::abi_types::MoltTypeTag::List as u8
            || t == crate::abi_types::MoltTypeTag::Tuple as u8
            || t == crate::abi_types::MoltTypeTag::Str as u8
            || t == crate::abi_types::MoltTypeTag::Bytes as u8
    ) as c_int
}

// ─── PySequence_Fast — fast access to list/tuple items ───────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast(
    o: *mut PyObject,
    _msg: *const std::os::raw::c_char,
) -> *mut PyObject {
    if o.is_null() {
        return ptr::null_mut();
    }
    // If already a list or tuple, return it (new ref).
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let tag = classify(bits);
    if tag == crate::abi_types::MoltTypeTag::List as u8
        || tag == crate::abi_types::MoltTypeTag::Tuple as u8
    {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    // Otherwise, materialize as a list.
    unsafe { PySequence_List(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_SIZE(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PySequence_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_Fast_GET_ITEM(
    o: *mut PyObject,
    i: Py_ssize_t,
) -> *mut PyObject {
    unsafe { PySequence_GetItem(o, i) }
}

// ─── PySequence_InPlaceConcat / InPlaceRepeat ────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceConcat(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PySequence_Concat(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySequence_InPlaceRepeat(
    o: *mut PyObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    unsafe { PySequence_Repeat(o, count) }
}
