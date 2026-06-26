//! FFI bridge shims for `molt-runtime-math`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The math crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::builtins::classes::class_name_for_error as _class_name_for_error;
use crate::builtins::containers::list_len as _list_len;
use crate::builtins::numbers::{
    bigint_bits as _bigint_bits, bigint_from_f64_trunc as _bigint_from_f64_trunc,
    bigint_to_inline as _bigint_to_inline, index_bigint_from_obj as _index_bigint_from_obj,
    index_i64_from_obj as _index_i64_from_obj, to_bigint as _to_bigint, to_f64 as _to_f64,
};
use crate::builtins::type_ops::type_of_bits as _type_of_bits;
use crate::object::ops::{
    format_obj as _format_obj, format_obj_str as _format_obj_str,
    string_obj_to_owned as _string_obj_to_owned, type_name as _type_name,
};
use crate::*;
use num_bigint::{BigInt, Sign};

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_runtime_state_get_or_init(
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    })
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_raise_exception(
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_ptr, type_len))
        };
        let msg =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, type_name, msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let name = _type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bytes_like_slice(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    match unsafe { bytes_like_slice(ptr) } {
        Some(slice) => {
            unsafe {
                *out_ptr = slice.as_ptr();
                *out_len = slice.len();
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_string_bytes(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let data_ptr = unsafe { string_bytes(ptr) };
    let data_len = unsafe { string_len(ptr) };
    unsafe {
        *out_ptr = data_ptr;
        *out_len = data_len;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_string_len(ptr: *mut u8) -> usize {
    unsafe { string_len(ptr) }
}

/// Extended float extraction: returns the f64 value for both inline floats
/// AND heap-allocated NaN floats (TYPE_ID_FLOAT).  Returns 1 and writes
/// the value to *out if the input is a float; otherwise returns 0.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_as_float_extended(bits: u64, out: *mut f64) -> i32 {
    use crate::object::ops::as_float_extended;
    let obj = obj_from_bits(bits);
    match as_float_extended(obj) {
        Some(f) => {
            unsafe {
                *out = f;
            }
            1
        }
        None => 0,
    }
}

/// Produce NaN-boxed bits for a float result.  Non-NaN values are stored
/// inline; NaN values are heap-allocated.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_float_result_bits(val: f64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { crate::object::ops::float_result_bits(_py, val) })
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_to_f64(bits: u64, out: *mut f64) -> i32 {
    let obj = obj_from_bits(bits);
    match _to_f64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_to_bigint(
    bits: u64,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _to_bigint(obj) {
        Some(value) => {
            let (sign, bytes) = value.to_bytes_be();
            let sign_i32 = match sign {
                Sign::Minus => -1i32,
                Sign::NoSign => 0i32,
                Sign::Plus => 1i32,
            };
            let boxed = bytes.into_boxed_slice();
            let ok = crate::bridge_buffer::export_u8_box(boxed, out_ptr, out_len);
            if ok == 0 {
                return 0;
            }
            unsafe {
                *out_sign = sign_i32;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_int_bits_from_i64(val: i64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, val) })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_int_bits_from_bigint(
    sign: i32,
    data_ptr: *const u8,
    data_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        let sign = match sign {
            -1 => Sign::Minus,
            0 => Sign::NoSign,
            _ => Sign::Plus,
        };
        let value = BigInt::from_bytes_be(sign, bytes);
        int_bits_from_bigint(_py, value)
    })
}

/// CPython's exact modular numeric hash over a rational `numerator/denominator`
/// (the shared `object::ops_hash::py_numeric_hash`). Each BigInt is passed as
/// `(sign, big-endian magnitude bytes, len)` like `int_bits_from_bigint`. Lets
/// the math crate's Fraction hash through the single shared numeric authority so
/// a Fraction hashes equal to a numerically-equal int/float/Decimal.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_py_numeric_hash(
    num_sign: i32,
    num_ptr: *const u8,
    num_len: usize,
    den_sign: i32,
    den_ptr: *const u8,
    den_len: usize,
) -> i64 {
    let to_big = |sign: i32, ptr: *const u8, len: usize| -> BigInt {
        let sign = match sign {
            -1 => Sign::Minus,
            0 => Sign::NoSign,
            _ => Sign::Plus,
        };
        let bytes = if ptr.is_null() || len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        BigInt::from_bytes_be(sign, bytes)
    };
    let numer = to_big(num_sign, num_ptr, num_len);
    let denom = to_big(den_sign, den_ptr, den_len);
    crate::object::ops_hash::py_numeric_hash(&numer, &denom)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bigint_ptr_from_bits(bits: u64) -> *mut u8 {
    match builtins::numbers::bigint_ptr_from_bits(bits) {
        Some(ptr) => ptr,
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bigint_ref(
    ptr: *mut u8,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let big = unsafe { bigint_ref(ptr) };
    let (sign, bytes) = big.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    let boxed = bytes.into_boxed_slice();
    let ok = crate::bridge_buffer::export_u8_box(boxed, out_ptr, out_len);
    if ok == 0 {
        return 0;
    }
    unsafe {
        *out_sign = sign_i32;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bigint_from_f64_trunc(
    val: f64,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let big = _bigint_from_f64_trunc(val);
    let (sign, bytes) = big.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    let boxed = bytes.into_boxed_slice();
    let ok = crate::bridge_buffer::export_u8_box(boxed, out_ptr, out_len);
    if ok == 0 {
        return 0;
    }
    unsafe {
        *out_sign = sign_i32;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bigint_bits(sign: i32, data_ptr: *const u8, data_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        let sign = match sign {
            -1 => Sign::Minus,
            0 => Sign::NoSign,
            _ => Sign::Plus,
        };
        let value = BigInt::from_bytes_be(sign, bytes);
        _bigint_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_bigint_to_inline(
    sign: i32,
    data_ptr: *const u8,
    data_len: usize,
) -> u64 {
    let bytes = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
    let sign = match sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    let value = BigInt::from_bytes_be(sign, bytes);
    match _bigint_to_inline(&value) {
        Some(v) => MoltObject::from_int(v).bits(),
        None => 0, // signal: doesn't fit inline
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_index_i64_from_obj(
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        _index_i64_from_obj(_py, obj_bits, err)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_index_bigint_from_obj(
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        match _index_bigint_from_obj(_py, obj_bits, err) {
            Some(value) => {
                let (sign, bytes) = value.to_bytes_be();
                let sign_i32 = match sign {
                    Sign::Minus => -1i32,
                    Sign::NoSign => 0i32,
                    Sign::Plus => 1i32,
                };
                let boxed = bytes.into_boxed_slice();
                let ok = crate::bridge_buffer::export_u8_box(boxed, out_ptr, out_len);
                if ok == 0 {
                    return 0;
                }
                unsafe {
                    *out_sign = sign_i32;
                }
                1
            }
            None => 0,
        }
    })
}

// ---------------------------------------------------------------------------
// Callable / protocol helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_call_callable0(call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { unsafe { call_callable0(_py, call_bits) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_call_callable2(call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_callable2(_py, call_bits, arg0, arg1) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_attr_lookup_ptr_allow_missing(ptr: *mut u8, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits: u64 =
            unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) }.unwrap_or_default();
        bits
    })
}

fn unsupported_math_intern_name(_py: &PyToken<'_>, key: &[u8]) -> u64 {
    let key = String::from_utf8_lossy(key);
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        &format!("molt-runtime-math requested unsupported interned static name {key:?}"),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::cache::intern_bridge_protocol_name(_py, key)
            .unwrap_or_else(|| unsupported_math_intern_name(_py, key))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_class_name_for_error(
    type_bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let name = _class_name_for_error(type_bits);
    let bytes = name.into_bytes().into_boxed_slice();
    crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_type_of_bits(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { _type_of_bits(_py, val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_maybe_ptr_from_bits(bits: u64) -> *mut u8 {
    match maybe_ptr_from_bits(bits) {
        Some(ptr) => ptr,
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_is_callable(bits: u64) -> i32 {
    // molt_is_callable is already a pub extern "C" function that does its own GIL entry.
    // We call it directly and check its result (returns True/False bits).
    let result = builtins::callable::molt_is_callable(bits);
    let obj = MoltObject::from_bits(result);
    if obj.as_bool() == Some(true) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_format_obj(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let s = _format_obj(_py, obj);
        let bytes = s.into_bytes().into_boxed_slice();
        crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_format_obj_str(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let s = _format_obj_str(_py, obj);
        let bytes = s.into_bytes().into_boxed_slice();
        crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
    })
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Some(bits) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_dict_set_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) };
        1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_list_len(ptr: *mut u8) -> usize {
    unsafe { _list_len(ptr) }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_iter(bits: u64) -> u64 {
    // molt_iter is a pub extern "C" fn(u64) -> u64 that does its own GIL entry.
    crate::object::ops_iter::molt_iter(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32 {
    // molt_iter_next returns the next element or a sentinel for StopIteration.
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    // Convention: None bits (0x7FF8_0000_0000_0000) signals exhaustion.
    let none_bits = MoltObject::none().bits();
    if result == none_bits {
        // Could be actual None or exhaustion — check exception pending.
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                0 // StopIteration or error
            } else {
                // Actual None value — return it.
                unsafe {
                    *out = result;
                }
                1
            }
        })
    } else {
        unsafe {
            *out = result;
        }
        1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_raise_not_iterable(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { raise_not_iterable::<u64>(_py, bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_sorted_builtin(bits: u64) -> u64 {
    // molt_sorted_builtin takes (iter_bits, key_bits, reverse_bits)
    // We pass None for key and False for reverse (default sorted behavior).
    let none = MoltObject::none().bits();
    let false_bits = MoltObject::from_bool(false).bits();
    crate::molt_sorted_builtin(bits, none, false_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_mul(a: u64, b: u64) -> u64 {
    // molt_mul is pub extern "C" fn(u64, u64) -> u64
    crate::object::ops_arith::molt_mul(a, b)
}

// ---------------------------------------------------------------------------
// OS randomness
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_fill_os_random(buf_ptr: *mut u8, buf_len: usize) -> i32 {
    let buf = unsafe { std::slice::from_raw_parts_mut(buf_ptr, buf_len) };
    match crate::randomness::fill_os_random(buf) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

// ---------------------------------------------------------------------------
// Container / object protocol helpers (used by math + statistics + random)
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_dict_new(capacity_bits: u64) -> u64 {
    crate::builtins::containers_alloc::molt_dict_new(capacity_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_hash_builtin(val_bits: u64) -> u64 {
    crate::object::ops::molt_hash_builtin(val_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::object::ops_slice::molt_slice_new(start_bits, stop_bits, step_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::object::ops::molt_index(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        crate::alloc_bytes(_py, data)
    })
}

#[cfg(test)]
mod tests {
    use super::__molt_math_intern_static_name;
    use crate::{clear_exception, exception_pending, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    fn math_bridge_interns_protocol_names_in_runtime_state() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"__bool__";
            let bits = __molt_math_intern_static_name(key.as_ptr(), key.len());
            assert!(!exception_pending(_py));
            assert_eq!(
                runtime_state(_py)
                    .interned
                    .bool_name
                    .load(Ordering::Acquire),
                bits
            );
        });
    }

    #[test]
    fn math_bridge_rejects_unknown_intern_name() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"__molt_unknown__";
            let _ = __molt_math_intern_static_name(key.as_ptr(), key.len());
            assert!(exception_pending(_py));
            clear_exception(_py);
        });
    }
}
