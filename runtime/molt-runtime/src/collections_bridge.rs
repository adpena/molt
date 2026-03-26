//! FFI bridge shims for `molt-runtime-collections`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The collections crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::*;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_raise_exception(
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_ptr, type_len)) };
        let msg = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, type_name, msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_exception_pending() -> i32 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_raise_key_error_with_key(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_key_error_with_key::<u64>(_py, key_bits)
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            let len = bytes.len();
            let ptr = Box::into_raw(bytes) as *const u8;
            unsafe {
                *out_ptr = ptr;
                *out_len = len;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let name = type_name(_py, obj);
        let owned: String = name.into_owned();
        let bytes = owned.into_bytes().into_boxed_slice();
        let len = bytes.len();
        let ptr = Box::into_raw(bytes) as *const u8;
        unsafe {
            *out_ptr = ptr;
            *out_len = len;
        }
        1
    })
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dec_ref_bits(bits: u64) {
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_inc_ref_bits(bits: u64) {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) => {
            unsafe { *out = v; }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_index_i64_with_overflow(
    bits: u64,
    type_err_ptr: *const u8,
    type_err_len: usize,
    out: *mut i64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let type_err = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_err_ptr, type_err_len)) };
        match index_i64_with_overflow(_py, bits, type_err, None) {
            Some(v) => {
                unsafe { *out = v; }
                1
            }
            None => 0,
        }
    })
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Some(bits) => {
                unsafe { *out = bits; }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_set_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) };
        1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_del_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if unsafe { dict_del_in_place(_py, dict_ptr, key_bits) } { 1 } else { 0 }
    })
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { seq_vec_ptr(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_dict_order_clone(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let order = unsafe { dict_order(ptr) }.clone();
        if order.is_empty() {
            return 0;
        }
        let boxed = order.into_boxed_slice();
        let len = boxed.len();
        let raw_ptr = Box::into_raw(boxed) as *const u64;
        unsafe {
            *out_ptr = raw_ptr;
            *out_len = len;
        }
        1
    })
}

// ---------------------------------------------------------------------------
// Object comparison
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_obj_eq(lhs_bits: u64, rhs_bits: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(lhs_bits);
        let rhs = obj_from_bits(rhs_bits);
        if obj_eq(_py, lhs, rhs) { 1 } else { 0 }
    })
}

// ---------------------------------------------------------------------------
// Callable helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_call_callable0(call_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe { crate::call::dispatch::call_callable0(_py, call_bits) }
    })
}
