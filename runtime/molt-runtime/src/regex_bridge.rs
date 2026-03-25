//! FFI bridge shims for `molt-runtime-regex`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The regex crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::*;
use crate::builtins::numbers::to_i64 as _to_i64;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_raise_exception(
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
pub extern "C" fn __molt_regex_exception_pending() -> i32 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) { 1 } else { 0 }
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_string_obj_to_owned(
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
pub extern "C" fn __molt_regex_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match _to_i64(obj) {
        Some(v) => {
            unsafe { *out = v; }
            1
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dec_ref_bits(bits: u64) {
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_inc_ref_bits(bits: u64) {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Sequence / collection access
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_seq_vec_ptr(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) {
    let vec = unsafe { seq_vec_ref(ptr) };
    unsafe {
        *out_ptr = vec.as_ptr();
        *out_len = vec.len();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_attr_name_bits_from_bytes(
    data_ptr: *const u8,
    data_len: usize,
    out_bits: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        match attr_name_bits_from_bytes(_py, data) {
            Some(bits) => {
                unsafe { *out_bits = bits; }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out_bits: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Some(bits) => {
                unsafe { *out_bits = bits; }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dict_set_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    crate::with_gil_entry!(_py, {
        dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_dict_order_ptr(
    dict_ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) {
    let order = unsafe { dict_order(dict_ptr) };
    unsafe {
        *out_ptr = order.as_ptr();
        *out_len = order.len();
    }
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_molt_iter(bits: u64) -> u64 {
    molt_iter(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_molt_iter_next(iter_bits: u64) -> u64 {
    molt_iter_next(iter_bits)
}

// ---------------------------------------------------------------------------
// Callable dispatch
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_regex_call_callable1(callable_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe { crate::call::dispatch::call_callable1(_py, callable_bits, arg_bits) }
    })
}
