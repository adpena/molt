//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::atomic::{AtomicBool, Ordering};

use molt_cpython_abi::RuntimeHooks;
use molt_cpython_abi::abi_types::MoltTypeTag;
use molt_obj_model::MoltObject;

use crate::builtins::containers::{dict_len, dict_order, list_len, tuple_len};
use crate::concurrency::gil::with_gil;
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string,
    alloc_tuple_with_capacity,
};
use crate::object::layout::seq_vec_ref;
use crate::object::type_ids::{
    TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE,
};
use crate::object::{
    MoltHeader, bits_from_ptr, bytes_data, bytes_len, dec_ref_bits, object_type_id, string_bytes,
    string_len,
};

// ─── Hook implementations ─────────────────────────────────────────────────

unsafe extern "C" fn hook_alloc_str(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = unsafe { alloc_string(&_py, bytes) };
        if ptr.is_null() { 0 } else { bits_from_ptr(ptr) }
    })
}

unsafe extern "C" fn hook_alloc_bytes(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = unsafe { alloc_bytes(&_py, bytes) };
        if ptr.is_null() { 0 } else { bits_from_ptr(ptr) }
    })
}

unsafe extern "C" fn hook_alloc_list() -> u64 {
    with_gil(|_py| {
        let ptr = unsafe { alloc_list_with_capacity(&_py, &[], 8) };
        if ptr.is_null() { 0 } else { bits_from_ptr(ptr) }
    })
}

unsafe extern "C" fn hook_list_append(list_bits: u64, item_bits: u64) {
    let obj = MoltObject::from_bits(list_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    // SAFETY: seq_vec_ref returns a shared ref; we cast to mut for the append.
    let vec = unsafe { seq_vec_ref(ptr) as *const Vec<u64> as *mut Vec<u64> };
    unsafe { (*vec).push(item_bits) };
}

unsafe extern "C" fn hook_list_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return 0;
    }
    unsafe { list_len(ptr) }
}

unsafe extern "C" fn hook_list_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe { seq_vec_ref(ptr) }.get(i).copied().unwrap_or(0)
}

unsafe extern "C" fn hook_alloc_tuple(n: usize) -> u64 {
    with_gil(|_py| {
        let ptr = unsafe { alloc_tuple_with_capacity(&_py, &[], n) };
        if ptr.is_null() { 0 } else { bits_from_ptr(ptr) }
    })
}

unsafe extern "C" fn hook_tuple_set(bits: u64, i: usize, val_bits: u64) {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    let vec = unsafe { seq_vec_ref(ptr) as *const Vec<u64> as *mut Vec<u64> };
    let v = unsafe { &mut *vec };
    if i < v.len() {
        v[i] = val_bits;
    } else {
        v.resize(i + 1, MoltObject::none().bits());
        v[i] = val_bits;
    }
}

unsafe extern "C" fn hook_tuple_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return 0;
    }
    unsafe { tuple_len(ptr) }
}

unsafe extern "C" fn hook_tuple_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe { seq_vec_ref(ptr) }.get(i).copied().unwrap_or(0)
}

unsafe extern "C" fn hook_alloc_dict() -> u64 {
    with_gil(|_py| {
        let ptr = unsafe { alloc_dict_with_pairs(&_py, &[]) };
        if ptr.is_null() { 0 } else { bits_from_ptr(ptr) }
    })
}

unsafe extern "C" fn hook_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) {
    let obj = MoltObject::from_bits(dict_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return;
    }
    let order = unsafe { dict_order(ptr) };
    let mut found = false;
    for chunk in order.chunks_mut(2) {
        if chunk[0] == key_bits {
            chunk[1] = val_bits;
            found = true;
            break;
        }
    }
    if !found {
        order.push(key_bits);
        order.push(val_bits);
    }
}

unsafe extern "C" fn hook_dict_get(dict_bits: u64, key_bits: u64) -> u64 {
    let obj = MoltObject::from_bits(dict_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    let order = unsafe { dict_order(ptr) };
    for chunk in order.chunks(2) {
        if chunk[0] == key_bits {
            return chunk[1];
        }
    }
    0
}

unsafe extern "C" fn hook_dict_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    unsafe { dict_len(ptr) }
}

unsafe extern "C" fn hook_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_STRING {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { string_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { string_bytes(ptr) }
        }
    }
}

unsafe extern "C" fn hook_bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { bytes_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { bytes_data(ptr) }
        }
    }
}

unsafe extern "C" fn hook_classify_heap(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return MoltTypeTag::Other as u8,
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_STRING => MoltTypeTag::Str as u8,
        TYPE_ID_BYTES => MoltTypeTag::Bytes as u8,
        TYPE_ID_LIST => MoltTypeTag::List as u8,
        TYPE_ID_TUPLE => MoltTypeTag::Tuple as u8,
        TYPE_ID_DICT => MoltTypeTag::Dict as u8,
        TYPE_ID_SET => MoltTypeTag::Set as u8,
        TYPE_ID_MODULE => MoltTypeTag::Module as u8,
        _ => MoltTypeTag::Other as u8,
    }
}

unsafe extern "C" fn hook_inc_ref(bits: u64) {
    let obj = MoltObject::from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        let hdr = ptr as *mut MoltHeader;
        if !hdr.is_null() {
            unsafe { (*hdr).ref_count.fetch_add(1, Ordering::Relaxed) };
        }
    }
}

unsafe extern "C" fn hook_dec_ref(bits: u64) {
    with_gil(|_py| unsafe { dec_ref_bits(&_py, bits) });
}

// ─── Registration ─────────────────────────────────────────────────────────

static HOOKS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Register the runtime hooks into `molt-lang-cpython-abi`.
/// Idempotent — safe to call multiple times (only registers once).
pub(crate) fn register_cpython_hooks() {
    if HOOKS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hooks = RuntimeHooks {
        alloc_str: hook_alloc_str,
        alloc_bytes: hook_alloc_bytes,
        alloc_list: hook_alloc_list,
        list_append: hook_list_append,
        list_len: hook_list_len,
        list_item: hook_list_item,
        alloc_tuple: hook_alloc_tuple,
        tuple_set: hook_tuple_set,
        tuple_len: hook_tuple_len,
        tuple_item: hook_tuple_item,
        alloc_dict: hook_alloc_dict,
        dict_set: hook_dict_set,
        dict_get: hook_dict_get,
        dict_len: hook_dict_len,
        str_data: hook_str_data,
        bytes_data: hook_bytes_data,
        classify_heap: hook_classify_heap,
        inc_ref: hook_inc_ref,
        dec_ref: hook_dec_ref,
    };
    // SAFETY: all fn pointers are valid for the process lifetime.
    unsafe { molt_cpython_abi::set_runtime_hooks(hooks) };
}
