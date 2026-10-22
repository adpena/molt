//! FFI bridge shims for `molt-runtime-math`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The math crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::audit::AuditArgs;
use crate::builtins::classes::class_name_for_error as _class_name_for_error;
use crate::builtins::containers::list_len as _list_len;
use crate::builtins::numbers::{
    bigint_from_f64_trunc as _bigint_from_f64_trunc,
    index_bigint_from_obj as _index_bigint_from_obj, index_i64_from_obj as _index_i64_from_obj,
    to_bigint as _to_bigint, to_f64 as _to_f64,
};
use crate::builtins::type_ops::type_of_bits as _type_of_bits;
use crate::object::ops::{string_obj_to_owned as _string_obj_to_owned, type_name as _type_name};
use crate::object::ops_format::{
    format_obj_output as _format_obj, format_obj_str_output as _format_obj_str,
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
            unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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

// Conversion boundaries use the shared numeric protocol owner. The older
// to_* exports below remain pure carrier probes for their existing consumers.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_float_as_double(bits: u64, out: *mut f64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        match crate::builtins::numbers::float_as_double(py, bits) {
            Some(value) => {
                unsafe {
                    *out = value;
                }
                1
            }
            None => -1,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_exact_float_value(bits: u64, out: *mut f64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        match crate::builtins::numbers::exact_float_storage_value(py, bits) {
            Ok(Some(value)) => {
                unsafe {
                    *out = value;
                }
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_integer_storage(
    bits: u64,
    out_sign: *mut i32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let value = match crate::builtins::numbers::integer_storage_bigint(py, bits) {
            Ok(Some(value)) => value,
            Ok(None) => return 0,
            Err(()) => return -1,
        };
        let (sign, bytes) = value.to_bytes_be();
        let status = unsafe {
            crate::resource::bridge_buffer::export_u8_box(
                bytes.into_boxed_slice(),
                out_ptr,
                out_len,
            )
        };
        if status == 0 {
            raise_exception::<()>(
                py,
                "MemoryError",
                "math integer storage transport allocation failed",
            );
            return -1;
        }
        unsafe {
            *out_sign = match sign {
                Sign::Minus => -1,
                Sign::NoSign => 0,
                Sign::Plus => 1,
            };
        }
        1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_target_minor() -> i64 {
    crate::with_gil_entry_nopanic!(py, { crate::object::ops_sys::runtime_target_minor(py) })
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
            let ok =
                unsafe { crate::resource::bridge_buffer::export_u8_box(boxed, out_ptr, out_len) };
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
    let ok = unsafe { crate::resource::bridge_buffer::export_u8_box(boxed, out_ptr, out_len) };
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
    let ok = unsafe { crate::resource::bridge_buffer::export_u8_box(boxed, out_ptr, out_len) };
    if ok == 0 {
        return 0;
    }
    unsafe {
        *out_sign = sign_i32;
    }
    1
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
                let ok = unsafe {
                    crate::resource::bridge_buffer::export_u8_box(boxed, out_ptr, out_len)
                };
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
pub extern "C" fn __molt_math_lookup_special_method(
    ptr: *mut u8,
    name_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe {
            crate::builtins::attr::lookup_special_method_bits(
                _py,
                MoltObject::from_ptr(ptr).bits(),
                name_bits,
            )
        } {
            Ok(Some(bits)) => {
                unsafe { *out = bits };
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
        }
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
        match crate::state::cache::intern_bridge_protocol_name(_py, key) {
            Ok(Some(bits)) => bits,
            Ok(None) => unsupported_math_intern_name(_py, key),
            Err(()) => MoltObject::none().bits(),
        }
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
    unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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
        let bytes = match _format_obj(_py, obj).into_bytes_result(_py) {
            Ok(bytes) => bytes.into_boxed_slice(),
            Err(error) => {
                error.raise::<()>(_py);
                return 0;
            }
        };
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
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
        let bytes = match _format_obj_str(_py, obj).into_bytes_result(_py) {
            Ok(bytes) => bytes.into_boxed_slice(),
            Err(error) => {
                error.raise::<()>(_py);
                return 0;
            }
        };
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_current_int_max_str_digits() -> usize {
    crate::with_gil_entry_nopanic!(_py, {
        crate::builtins::sys_ext::current_int_max_str_digits(_py)
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
            Ok(Some(bits)) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
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
        i32::from(unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_list_len(ptr: *mut u8) -> usize {
    unsafe { _list_len(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_list_swap(list_bits: u64, left: usize, right: usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list_bits).as_ptr() else {
            return 0;
        };
        if unsafe { crate::object::list_mutation::swap_indices(_py, ptr, left, right) } {
            1
        } else {
            0
        }
    })
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
pub extern "C" fn __molt_math_require_random_entropy() -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        crate::require_operation::<u64>(_py, crate::OperationId::RandomEntropy, AuditArgs::None)
            .is_ok() as i32
    })
}

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
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"__molt_unknown__";
            let _ = __molt_math_intern_static_name(key.as_ptr(), key.len());
            assert!(exception_pending(_py));
            clear_exception(_py);
        });
    }
}

#[cfg(test)]
mod numeric_protocol_tests {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::cpython_abi_hooks::native_test_fixture::NativeType;
    use crate::object::foreign::NativeObjectOwner;
    use molt_cpython_abi::abi_types::*;
    use molt_cpython_abi::api::{errors, numbers, object, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    use std::ptr;
    use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

    static FLOAT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INDEX_RESULT: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
    static RETIRING_FLOAT_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
    static RETIRING_FLOAT_INPUT_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
    static RETIRING_FLOAT_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
    static RETIRING_FLOAT_DROPS: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct RetiringFloat {
        base: PyFloatObject,
        saved_error: *mut PyObject,
    }

    unsafe extern "C" fn retiring_float(_self: *mut PyObject) -> *mut PyObject {
        unsafe {
            let class = RETIRING_FLOAT_TYPE.load(Ordering::SeqCst);
            if class.is_null() {
                errors::PyErr_BadInternalCall();
                return ptr::null_mut();
            }
            let result = typeobj::PyType_GenericAlloc(class, 0);
            if !result.is_null() {
                let result = result.cast::<RetiringFloat>();
                (*result).base.ob_fval = 2.5;
                // A strict float result triggers warning construction, even
                // under an ignore filter. Own the prior until RC-zero instead
                // of entering that generic call with an exception selected.
                (*result).saved_error = errors::PyErr_GetRaisedException();
                RETIRING_FLOAT_INPUT_ERROR.store((*result).saved_error, Ordering::SeqCst);
            }
            result
        }
    }

    unsafe extern "C" fn retiring_float_drop(value: *mut PyObject) {
        unsafe {
            RETIRING_FLOAT_DROPS.fetch_add(1, Ordering::SeqCst);
            let saved = std::mem::replace(
                &mut (*value.cast::<RetiringFloat>()).saved_error,
                ptr::null_mut(),
            );
            if errors::raised_error_pending() {
                // Warning-as-error owns completion; retiring the saved prior
                // must not replace that newly selected failure.
                errors::with_preserved_error(|| molt_cpython_abi::api::refcount::Py_XDECREF(saved));
            } else {
                errors::PyErr_SetRaisedException(saved);
            }
            let error = errors::PyErr_GetRaisedException();
            RETIRING_FLOAT_ERROR.store(error, Ordering::SeqCst);
            errors::PyErr_SetRaisedException(error);
            molt_cpython_abi::api::memory::PyObject_Free(value.cast());
        }
    }

    unsafe extern "C" fn float_override(_self: *mut PyObject) -> *mut PyObject {
        FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe {
            let result = typeobj::PyType_GenericAlloc(&raw mut PyFloat_Type, 0);
            if !result.is_null() {
                (*result.cast::<PyFloatObject>()).ob_fval = 4.0;
            }
            result
        }
    }

    unsafe extern "C" fn index_result(_self: *mut PyObject) -> *mut PyObject {
        unsafe { object::Py_XNewRef(INDEX_RESULT.load(Ordering::SeqCst)) }
    }

    unsafe extern "C" fn floor_override(
        _self: *mut PyObject,
        _args: *mut PyObject,
    ) -> *mut PyObject {
        unsafe { numbers::PyLong_FromLong(23) }
    }

    struct CallbackState;
    impl Drop for CallbackState {
        fn drop(&mut self) {
            INDEX_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
            RETIRING_FLOAT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        }
    }

    struct WarningPolicy;
    impl Drop for WarningPolicy {
        fn drop(&mut self) {
            crate::molt_warnings_resetwarnings();
        }
    }

    #[test]
    fn native_math_consumers_use_numeric_storage_and_protocol_owners() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            FLOAT_CALLS.store(0, Ordering::SeqCst);
            let mut integer_slots: PyNumberMethods = std::mem::zeroed();
            integer_slots.nb_float = float_override as *const () as *mut _;
            let mut integer_type = NativeType::subtype(&raw mut PyLong_Type, c"math.NativeInteger");
            integer_type.tp_as_number = (&raw mut integer_slots).cast();
            assert_eq!(integer_type.ready(), 0);
            // Genuine physical 2**1200, with an override that generic float
            // conversion must invoke and the actual-integer logarithm must ignore.
            let integer = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *integer_type,
                41,
            ));
            assert!(!integer.as_ptr().is_null());
            let layout = integer.as_ptr().cast::<PyLongObject>();
            (*layout).long_value.lv_tag = 41 << 3;
            let digits = (&raw mut (*layout).long_value.ob_digit).cast::<u32>();
            for index in 0..41 {
                digits.add(index).write(u32::from(index == 40));
            }
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(integer.as_ptr())
                    .is_none()
            );
            let integer_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE
                    .molt_value_for_pyobj(integer.as_ptr())
                    .unwrap(),
            );
            assert_eq!(
                crate::object_type_id(crate::obj_from_bits(integer_bits.bits()).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );
            let result = ExceptionValue::adopt(
                py,
                molt_runtime_math::math::molt_math_log2(integer_bits.bits()),
            );
            assert_eq!(
                crate::as_float_extended(crate::obj_from_bits(result.bits())),
                Some(1200.0)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            let result = ExceptionValue::adopt(
                py,
                molt_runtime_math::math::molt_math_sqrt(integer_bits.bits()),
            );
            assert_eq!(
                crate::as_float_extended(crate::obj_from_bits(result.bits())),
                Some(2.0)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);

            let mut methods = [
                PyMethodDef {
                    ml_name: c"__floor__".as_ptr(),
                    ml_meth: Some(floor_override),
                    ml_flags: METH_NOARGS,
                    ml_doc: ptr::null(),
                },
                PyMethodDef {
                    ml_name: ptr::null(),
                    ml_meth: None,
                    ml_flags: 0,
                    ml_doc: ptr::null(),
                },
            ];
            let mut float_slots: PyNumberMethods = std::mem::zeroed();
            float_slots.nb_float = float_override as *const () as *mut _;
            let mut float_type = NativeType::subtype(&raw mut PyFloat_Type, c"math.NativeFloat");
            float_type.tp_as_number = (&raw mut float_slots).cast();
            float_type.tp_methods = methods.as_mut_ptr();
            assert_eq!(float_type.ready(), 0);
            let float = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *float_type,
                0,
            ));
            assert!(!float.as_ptr().is_null());
            (*float.as_ptr().cast::<PyFloatObject>()).ob_fval = 9.0;
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(float.as_ptr())
                    .is_none()
            );
            let float_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(float.as_ptr()).unwrap(),
            );
            let result = ExceptionValue::adopt(
                py,
                molt_runtime_math::math::molt_math_sqrt(float_bits.bits()),
            );
            assert_eq!(
                crate::as_float_extended(crate::obj_from_bits(result.bits())),
                Some(3.0)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            let result = ExceptionValue::adopt(
                py,
                molt_runtime_math::math::molt_math_floor(float_bits.bits()),
            );
            assert_eq!(
                crate::builtins::numbers::integer_storage_i64(py, result.bits()),
                Some(23)
            );

            let exact = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut PyFloat_Type,
                0,
            ));
            assert!(!exact.as_ptr().is_null());
            (*exact.as_ptr().cast::<PyFloatObject>()).ob_fval = 9.5;
            let exact_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(exact.as_ptr()).unwrap(),
            );
            let result = ExceptionValue::adopt(
                py,
                molt_runtime_math::math::molt_math_floor(exact_bits.bits()),
            );
            assert_eq!(
                crate::builtins::numbers::integer_storage_i64(py, result.bits()),
                Some(9)
            );

            assert_eq!(
                crate::object_type_id(crate::obj_from_bits(exact_bits.bits()).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN,
            );
            // Storage probes are allowed with an existing indicator. Hit/miss
            // comes from the producer, never from that unrelated old error.
            let prior = NativeObjectOwner::from_owned(object::PyObject_CallNoArgs(
                (&raw mut PyExc_LookupError).cast(),
            ));
            assert!(!prior.as_ptr().is_null());
            let prior_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(prior.as_ptr()).unwrap(),
            );
            for runtime_prior in [false, true] {
                if runtime_prior {
                    crate::record_exception(
                        py,
                        crate::obj_from_bits(prior_bits.bits()).as_ptr().unwrap(),
                    );
                } else {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                }
                let mut value = 0.0;
                assert_eq!(
                    crate::builtins::numbers::float_storage_value(py, float_bits.bits()),
                    Ok(Some(9.0)),
                );
                assert_eq!(
                    crate::builtins::numbers::float_storage_value(py, integer_bits.bits()),
                    Ok(None),
                );
                for (bits, expected) in [
                    (exact_bits.bits(), 9.5),
                    (float_bits.bits(), 9.0),
                    (integer_bits.bits(), 4.0),
                ] {
                    assert_eq!(super::__molt_math_float_as_double(bits, &mut value), 1);
                    assert_eq!(value, expected);
                }
                assert_eq!(
                    super::__molt_math_exact_float_value(exact_bits.bits(), &mut value),
                    1
                );
                assert_eq!(value, 9.5);
                assert_eq!(
                    super::__molt_math_exact_float_value(integer_bits.bits(), &mut value),
                    0
                );
                assert_eq!(
                    super::__molt_math_exact_float_value(
                        crate::MoltObject::none().bits(),
                        &mut value
                    ),
                    0
                );
                let mut sign = 0;
                let mut bytes = ptr::null();
                let mut len = 0;
                assert_eq!(
                    super::__molt_math_integer_storage(
                        integer_bits.bits(),
                        &mut sign,
                        &mut bytes,
                        &mut len
                    ),
                    1
                );
                let owned = molt_runtime_core::bridge_owned_u8_buffer(bytes, len);
                assert_eq!(sign, 1);
                assert_eq!(owned.len(), 151);
                assert_eq!(owned[0], 1);
                assert!(owned[1..].iter().all(|byte| *byte == 0));
                drop(owned);
                assert_eq!(
                    super::__molt_math_integer_storage(
                        exact_bits.bits(),
                        &mut sign,
                        &mut bytes,
                        &mut len
                    ),
                    0
                );
                assert_eq!(
                    super::__molt_math_integer_storage(
                        crate::MoltObject::none().bits(),
                        &mut sign,
                        &mut bytes,
                        &mut len
                    ),
                    0
                );
                let selected = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(selected.as_ptr(), prior.as_ptr());
                assert!(!crate::exception_pending(py));
                assert!(!errors::raised_error_pending());
            }

            // The same magnitude returned by __index__ must overflow through
            // PyFloat_AsDouble; it is not eligible for actual-integer log handling.
            let mut index_slots: PyNumberMethods = std::mem::zeroed();
            index_slots.nb_index = index_result as *const () as *mut _;
            let mut index_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"math.NativeIndex");
            index_type.tp_as_number = (&raw mut index_slots).cast();
            assert_eq!(index_type.ready(), 0);
            let index = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *index_type,
                0,
            ));
            assert!(!index.as_ptr().is_null());
            let index_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(index.as_ptr()).unwrap(),
            );
            // Use an exact native long result to isolate overflow from subtype warnings.
            let huge = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut PyLong_Type,
                41,
            ));
            assert!(!huge.as_ptr().is_null());
            let layout = huge.as_ptr().cast::<PyLongObject>();
            (*layout).long_value.lv_tag = 41 << 3;
            let digits = (&raw mut (*layout).long_value.ob_digit).cast::<u32>();
            for index in 0..41 {
                digits.add(index).write(u32::from(index == 40));
            }
            let _callbacks = CallbackState;
            INDEX_RESULT.store(huge.as_ptr(), Ordering::SeqCst);
            for operation in [
                molt_runtime_math::math::molt_math_log,
                molt_runtime_math::math::molt_math_log2,
                molt_runtime_math::math::molt_math_log10,
            ] {
                let result = ExceptionValue::adopt(py, operation(index_bits.bits()));
                assert!(crate::obj_from_bits(result.bits()).is_none());
                let error = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                assert_eq!(
                    object::Py_TYPE(error.as_ptr()),
                    &raw mut PyExc_OverflowError
                );
                assert!(!crate::exception_pending(py));
            }
            INDEX_RESULT.store(ptr::null_mut(), Ordering::SeqCst);
        });
    }

    #[test]
    fn native_scalar_float_status_distinguishes_minus_one_and_protocol_failure() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let mut slots: PyNumberMethods = std::mem::zeroed();
            slots.nb_index = index_result as *const () as *mut _;
            let mut index_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"math.MinusOneIndex");
            index_type.tp_as_number = (&raw mut slots).cast();
            assert_eq!(index_type.ready(), 0);
            let index = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *index_type,
                0,
            ));
            assert!(!index.as_ptr().is_null());
            let index_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(index.as_ptr()).unwrap(),
            );
            let integer = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut PyLong_Type,
                1,
            ));
            assert!(!integer.as_ptr().is_null());
            let integer_layout = integer.as_ptr().cast::<PyLongObject>();
            (*integer_layout).long_value.lv_tag = (1 << 3) | 2;
            (*integer_layout).long_value.ob_digit[0] = 1;
            let float = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut PyFloat_Type,
                0,
            ));
            assert!(!float.as_ptr().is_null());
            (*float.as_ptr().cast::<PyFloatObject>()).ob_fval = -1.0;
            let float_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(float.as_ptr()).unwrap(),
            );
            let plain = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut PyBaseObject_Type,
                0,
            ));
            assert!(!plain.as_ptr().is_null());
            let plain_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(plain.as_ptr()).unwrap(),
            );
            let prior = NativeObjectOwner::from_owned(object::PyObject_CallNoArgs(
                (&raw mut PyExc_LookupError).cast(),
            ));
            assert!(!prior.as_ptr().is_null());
            let prior_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(prior.as_ptr()).unwrap(),
            );
            let _callbacks = CallbackState;
            INDEX_RESULT.store(integer.as_ptr(), Ordering::SeqCst);
            for runtime_prior in [false, true] {
                if runtime_prior {
                    crate::record_exception(
                        py,
                        crate::obj_from_bits(prior_bits.bits()).as_ptr().unwrap(),
                    );
                } else {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                }
                for bits in [float_bits.bits(), index_bits.bits()] {
                    let mut output = 7.0;
                    assert_eq!(super::__molt_math_float_as_double(bits, &mut output), 1);
                    assert_eq!(output, -1.0);
                }
                assert_eq!(numbers::PyFloat_AsDouble(index.as_ptr()), -1.0);
                // CPython's object-producing PyNumber_Float checks -1 plus
                // pending only after releasing its index result. Its scalar
                // sibling returns that valid -1 directly.
                let result = NativeObjectOwner::from_owned(
                    molt_cpython_abi::api::abstract_number::PyNumber_Float(index.as_ptr()),
                );
                assert!(result.as_ptr().is_null());
                let selected = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(selected.as_ptr(), prior.as_ptr());
                assert!(!crate::exception_pending(py));
                assert!(!errors::raised_error_pending());

                if runtime_prior {
                    crate::record_exception(
                        py,
                        crate::obj_from_bits(prior_bits.bits()).as_ptr().unwrap(),
                    );
                } else {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                }
                let mut output = 7.0;
                assert_eq!(
                    super::__molt_math_float_as_double(plain_bits.bits(), &mut output),
                    -1
                );
                assert_eq!(output, 7.0, "failed conversion must not publish a scalar");
                let failure = NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                assert!(!failure.as_ptr().is_null());
                assert_ne!(failure.as_ptr(), prior.as_ptr());
                assert_eq!(object::Py_TYPE(failure.as_ptr()), &raw mut PyExc_TypeError);
                assert!(!crate::exception_pending(py));
                assert!(!errors::raised_error_pending());
            }
        });
    }

    #[test]
    fn scalar_numeric_retirement_completes_legal_native_error_channel_movement() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let _warning_policy = WarningPolicy;
            crate::molt_warnings_resetwarnings();
            let mut result_type = NativeType::subtype(&raw mut PyFloat_Type, c"math.RetiringFloat");
            result_type.tp_basicsize = std::mem::size_of::<RetiringFloat>() as Py_ssize_t;
            result_type.tp_dealloc = Some(retiring_float_drop);
            assert_eq!(result_type.ready(), 0);
            let mut slots: PyNumberMethods = std::mem::zeroed();
            slots.nb_float = retiring_float as *const () as *mut _;
            let mut input_type =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"math.FloatProducer");
            input_type.tp_as_number = (&raw mut slots).cast();
            assert_eq!(input_type.ready(), 0);
            let input = NativeObjectOwner::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *input_type,
                0,
            ));
            assert!(!input.as_ptr().is_null());
            let input_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(input.as_ptr()).unwrap(),
            );
            let prior = NativeObjectOwner::from_owned(object::PyObject_CallNoArgs(
                (&raw mut PyExc_LookupError).cast(),
            ));
            assert!(!prior.as_ptr().is_null());
            let prior_bits = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE.molt_value_for_pyobj(prior.as_ptr()).unwrap(),
            );
            let _callbacks = CallbackState;
            RETIRING_FLOAT_TYPE.store(&raw mut *result_type, Ordering::SeqCst);
            for warning_error in [false, true] {
                crate::molt_warnings_resetwarnings();
                let action: &[u8] = if warning_error { b"error" } else { b"ignore" };
                let action = ExceptionValue::adopt(
                    py,
                    crate::attr_name_bits_from_bytes(py, action).unwrap(),
                );
                let configured = ExceptionValue::adopt(
                    py,
                    crate::molt_warnings_simplefilter(
                        action.bits(),
                        crate::builtins::exceptions::exception_type_bits_from_name(
                            py,
                            "DeprecationWarning",
                        ),
                        crate::MoltObject::from_int(0).bits(),
                        crate::MoltObject::from_bool(false).bits(),
                    ),
                );
                assert!(!crate::exception_pending(py));
                drop(configured);
                for runtime_prior in [false, true] {
                    RETIRING_FLOAT_INPUT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                    RETIRING_FLOAT_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                    RETIRING_FLOAT_DROPS.store(0, Ordering::SeqCst);
                    if runtime_prior {
                        crate::record_exception(
                            py,
                            crate::obj_from_bits(prior_bits.bits()).as_ptr().unwrap(),
                        );
                    } else {
                        errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                    }
                    let mut value = 19.25;
                    let status = super::__molt_math_float_as_double(input_bits.bits(), &mut value);
                    assert_eq!(
                        RETIRING_FLOAT_INPUT_ERROR.load(Ordering::SeqCst),
                        prior.as_ptr(),
                        "the real number slot takes the exact incoming owner"
                    );
                    assert_eq!(RETIRING_FLOAT_DROPS.load(Ordering::SeqCst), 1);
                    assert!(crate::exception_pending(py));
                    let retired = RETIRING_FLOAT_ERROR.load(Ordering::SeqCst);
                    if warning_error {
                        assert_eq!(status, -1);
                        assert_eq!(value, 19.25, "failed extraction cannot publish output");
                        assert!(!retired.is_null());
                        assert_ne!(retired, prior.as_ptr());
                    } else {
                        assert_eq!(status, 1);
                        assert_eq!(value, 2.5);
                        assert_eq!(retired, prior.as_ptr());
                        assert_eq!(
                            crate::builtins::exceptions::thread_last_exception_bits_noinc(py),
                            Some(prior_bits.bits())
                        );
                    }
                    let selected =
                        NativeObjectOwner::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(
                        selected.as_ptr(),
                        retired,
                        "retirement preserves exact identity"
                    );
                    if warning_error {
                        assert_eq!(
                            (*selected.as_ptr()).ob_type,
                            &raw mut PyExc_DeprecationWarning
                        );
                    }
                    assert!(!crate::exception_pending(py));
                    assert!(!errors::raised_error_pending());
                }
            }
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_exact_builtin_sequence(bits: u64) -> bool {
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        let class = _type_of_bits(py, bits);
        class == classes.list || class == classes.tuple
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_exact_builtin_real(bits: u64) -> bool {
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        let class = _type_of_bits(py, bits);
        class == classes.int || class == classes.bool || class == classes.float
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_not_implemented_bits() -> u64 {
    crate::with_gil_entry_nopanic!(py, { not_implemented_bits(py) })
}
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_add(a: u64, b: u64) -> u64 {
    molt_add(a, b)
}
#[unsafe(no_mangle)]
pub extern "C" fn __molt_math_molt_div(a: u64, b: u64) -> u64 {
    molt_div(a, b)
}
