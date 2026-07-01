//! FFI bridge for `molt-runtime-serial`.
//!
//! The serial crate dispatches through a single `RuntimeVtable` obtained via
//! `__molt_serial_get_vtable()`.  All bridge functions are private to this
//! module — no individual `#[no_mangle]` C symbols are exported.

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

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

extern "C" fn bridge_raise_exception(
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

extern "C" fn bridge_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

extern "C" fn bridge_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

extern "C" fn bridge_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

extern "C" fn bridge_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

extern "C" fn bridge_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

extern "C" fn bridge_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

extern "C" fn bridge_string_obj_to_owned(
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

extern "C" fn bridge_type_name(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let name = _type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
    })
}

extern "C" fn bridge_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

extern "C" fn bridge_ensure_hashable(bits: u64, ctx: i32) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx = match ctx {
            x if x == molt_runtime_core::HashContextCode::SetElement as i32 => {
                HashContext::SetElement
            }
            x if x == molt_runtime_core::HashContextCode::DictKey as i32 => HashContext::DictKey,
            _ => HashContext::Bare,
        };
        if ensure_hashable(_py, bits, ctx) {
            1
        } else {
            0
        }
    })
}

extern "C" fn bridge_bytes_like_slice(
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

extern "C" fn bridge_string_bytes(
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

extern "C" fn bridge_string_len(ptr: *mut u8) -> usize {
    unsafe { string_len(ptr) }
}

// ---------------------------------------------------------------------------
// Memoryview / bytes-like helpers
// ---------------------------------------------------------------------------

extern "C" fn bridge_bytes_like_slice_raw(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    match unsafe { crate::object::memoryview::bytes_like_slice_raw(ptr) } {
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

extern "C" fn bridge_memoryview_is_c_contiguous_view(ptr: *mut u8) -> i32 {
    if unsafe { crate::object::memoryview::memoryview_is_c_contiguous_view(ptr) } {
        1
    } else {
        0
    }
}

extern "C" fn bridge_memoryview_readonly(ptr: *mut u8) -> i32 {
    if unsafe { memoryview_released(ptr) || memoryview_readonly(ptr) } {
        1
    } else {
        0
    }
}

extern "C" fn bridge_memoryview_nbytes(ptr: *mut u8) -> usize {
    if unsafe { memoryview_released(ptr) } {
        return 0;
    }
    unsafe { crate::object::memoryview::memoryview_nbytes(ptr) }
}

extern "C" fn bridge_memoryview_offset(ptr: *mut u8) -> isize {
    if unsafe { memoryview_released(ptr) } {
        return 0;
    }
    unsafe { memoryview_offset(ptr) }
}

extern "C" fn bridge_memoryview_owner_bits(ptr: *mut u8) -> u64 {
    if unsafe { memoryview_released(ptr) } {
        return 0;
    }
    unsafe { memoryview_owner_bits(ptr) }
}

extern "C" fn bridge_memoryview_data(ptr: *mut u8) -> *mut u8 {
    if unsafe { memoryview_released(ptr) } {
        return std::ptr::null_mut();
    }
    unsafe { memoryview_data(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

extern "C" fn bridge_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

extern "C" fn bridge_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

extern "C" fn bridge_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

extern "C" fn bridge_to_i64(bits: u64, out: *mut i64) -> i32 {
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

extern "C" fn bridge_to_f64(bits: u64, out: *mut f64) -> i32 {
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

extern "C" fn bridge_to_bigint(
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

extern "C" fn bridge_int_bits_from_i64(val: i64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, val) })
}

extern "C" fn bridge_int_bits_from_i128(val_lo: u64, val_hi: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let val = (val_hi as i128) << 64 | (val_lo as u128 as i128);
        crate::builtins::numbers::int_bits_from_i128(_py, val)
    })
}

extern "C" fn bridge_int_bits_from_bigint(sign: i32, data_ptr: *const u8, data_len: usize) -> u64 {
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

extern "C" fn bridge_bigint_ptr_from_bits(bits: u64) -> *mut u8 {
    match builtins::numbers::bigint_ptr_from_bits(bits) {
        Some(ptr) => ptr,
        None => std::ptr::null_mut(),
    }
}

fn bigint_from_bridge_parts(sign: i32, ptr: *const u8, len: usize) -> BigInt {
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
}

extern "C" fn bridge_py_numeric_hash(
    num_sign: i32,
    num_ptr: *const u8,
    num_len: usize,
    den_sign: i32,
    den_ptr: *const u8,
    den_len: usize,
) -> i64 {
    let numer = bigint_from_bridge_parts(num_sign, num_ptr, num_len);
    let denom = bigint_from_bridge_parts(den_sign, den_ptr, den_len);
    crate::object::ops_hash::py_numeric_hash(&numer, &denom)
}

extern "C" fn bridge_py_decimal_hash(
    coeff_sign: i32,
    coeff_ptr: *const u8,
    coeff_len: usize,
    exp10: i64,
) -> i64 {
    let coefficient = bigint_from_bridge_parts(coeff_sign, coeff_ptr, coeff_len);
    crate::object::ops_hash::py_decimal_hash(&coefficient, exp10)
}

extern "C" fn bridge_py_hash_inf() -> i64 {
    crate::object::ops_hash::PY_HASH_INF
}

extern "C" fn bridge_bigint_ref(
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

extern "C" fn bridge_bigint_from_f64_trunc(
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

extern "C" fn bridge_bigint_bits(sign: i32, data_ptr: *const u8, data_len: usize) -> u64 {
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

extern "C" fn bridge_bigint_to_inline(sign: i32, data_ptr: *const u8, data_len: usize) -> u64 {
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

extern "C" fn bridge_index_i64_from_obj(obj_bits: u64, err_ptr: *const u8, err_len: usize) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        _index_i64_from_obj(_py, obj_bits, err)
    })
}

extern "C" fn bridge_index_bigint_from_obj(
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

extern "C" fn bridge_call_callable0(call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { unsafe { call_callable0(_py, call_bits) } })
}

extern "C" fn bridge_call_callable2(call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_callable2(_py, call_bits, arg0, arg1) }
    })
}

extern "C" fn bridge_attr_lookup_ptr_allow_missing(ptr: *mut u8, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits: u64 =
            unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) }.unwrap_or_default();
        bits
    })
}

fn unsupported_serial_intern_name(_py: &PyToken<'_>, key: &[u8]) -> u64 {
    let key = String::from_utf8_lossy(key);
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        &format!("molt-runtime-serial requested unsupported interned static name {key:?}"),
    )
}

extern "C" fn bridge_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::cache::intern_bridge_protocol_name(_py, key)
            .unwrap_or_else(|| unsupported_serial_intern_name(_py, key))
    })
}

extern "C" fn bridge_class_name_for_error(
    type_bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let name = _class_name_for_error(type_bits);
    let bytes = name.into_bytes().into_boxed_slice();
    crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
}

extern "C" fn bridge_type_of_bits(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { _type_of_bits(_py, val_bits) })
}

extern "C" fn bridge_maybe_ptr_from_bits(bits: u64) -> *mut u8 {
    match maybe_ptr_from_bits(bits) {
        Some(ptr) => ptr,
        None => std::ptr::null_mut(),
    }
}

extern "C" fn bridge_molt_is_callable(bits: u64) -> i32 {
    let result = builtins::callable::molt_is_callable(bits);
    let obj = MoltObject::from_bits(result);
    if obj.as_bool() == Some(true) { 1 } else { 0 }
}

extern "C" fn bridge_format_obj(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let s = _format_obj(_py, obj);
        let bytes = s.into_bytes().into_boxed_slice();
        crate::bridge_buffer::export_u8_box(bytes, out_ptr, out_len)
    })
}

extern "C" fn bridge_format_obj_str(
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
// Bytearray helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes_definitions)]
extern "C" fn bridge_bytearray_vec(ptr: *mut u8) -> *mut Vec<u8> {
    unsafe { crate::object::layout::bytearray_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

extern "C" fn bridge_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32 {
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

extern "C" fn bridge_dict_set_in_place(dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) };
        1
    })
}

extern "C" fn bridge_list_len(ptr: *mut u8) -> usize {
    unsafe { _list_len(ptr) }
}

#[allow(improper_ctypes_definitions)]
extern "C" fn bridge_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

extern "C" fn bridge_molt_iter(bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

extern "C" fn bridge_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32 {
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    let none_bits = MoltObject::none().bits();
    if result == none_bits {
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

extern "C" fn bridge_raise_not_iterable(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { raise_not_iterable::<u64>(_py, bits) })
}

extern "C" fn bridge_molt_sorted_builtin(bits: u64) -> u64 {
    let none = MoltObject::none().bits();
    let false_bits = MoltObject::from_bool(false).bits();
    crate::molt_sorted_builtin(bits, none, false_bits)
}

extern "C" fn bridge_molt_mul(a: u64, b: u64) -> u64 {
    crate::object::ops_arith::molt_mul(a, b)
}

// ---------------------------------------------------------------------------
// OS randomness
// ---------------------------------------------------------------------------

extern "C" fn bridge_fill_os_random(buf_ptr: *mut u8, buf_len: usize) -> i32 {
    let buf = unsafe { std::slice::from_raw_parts_mut(buf_ptr, buf_len) };
    match crate::randomness::fill_os_random(buf) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

#[cfg(target_arch = "wasm32")]
extern "C" fn bridge_time_local_offset_host(secs: i64) -> i64 {
    unsafe { crate::molt_time_local_offset_host(secs) }
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn bridge_time_local_offset_host(_secs: i64) -> i64 {
    0
}

// ---------------------------------------------------------------------------
// Dict helpers (configparser-specific)
// ---------------------------------------------------------------------------

extern "C" fn bridge_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

extern "C" fn bridge_dict_order_clone(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    let order = unsafe { crate::builtins::containers::dict_order(ptr) }.clone();
    let boxed = order.into_boxed_slice();
    crate::bridge_buffer::export_u64_box(boxed, out_ptr, out_len)
}

// ---------------------------------------------------------------------------
// Extended helpers (email / zipfile / decimal)
// ---------------------------------------------------------------------------

extern "C" fn bridge_alloc_list_with_capacity(
    elems_ptr: *const u64,
    elems_len: usize,
    capacity: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        crate::object::builders::alloc_list_with_capacity(_py, elems, capacity)
    })
}

extern "C" fn bridge_attr_name_bits_from_bytes(
    name_ptr: *const u8,
    name_len: usize,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
        match crate::builtins::attr::attr_name_bits_from_bytes(_py, name) {
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

extern "C" fn bridge_call_class_init_with_args(
    class_ptr: *mut u8,
    args_ptr: *const u64,
    args_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
        unsafe { crate::call::class_init::call_class_init_with_args(_py, class_ptr, args) }
    })
}

extern "C" fn bridge_missing_bits() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { crate::builtins::methods::missing_bits(_py) })
}

extern "C" fn bridge_molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::object::ops_builtins::molt_getattr_builtin(obj_bits, name_bits, default_bits)
}

extern "C" fn bridge_molt_module_import(name_bits: u64) -> u64 {
    crate::builtins::modules::molt_module_import(name_bits)
}

// ---------------------------------------------------------------------------
// RuntimeVtable — single-dispatch entry point for the serial crate
// ---------------------------------------------------------------------------

use molt_runtime_core::{
    RUNTIME_VTABLE_ABI_MAGIC, RUNTIME_VTABLE_ABI_VERSION, RuntimeExtensionStateClear,
    RuntimeExtensionStateDrop, RuntimeExtensionStateInit, RuntimeVtable, RuntimeVtableHeader,
};

extern "C" fn bridge_runtime_state_get_or_init(
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

/// The global vtable populated with pointers to the private bridge functions.
/// The serial crate fetches this once at init time via `__molt_serial_get_vtable()`.
static RUNTIME_VTABLE: RuntimeVtable = RuntimeVtable {
    header: RuntimeVtableHeader {
        abi_magic: RUNTIME_VTABLE_ABI_MAGIC,
        abi_version: RUNTIME_VTABLE_ABI_VERSION,
        abi_size: std::mem::size_of::<RuntimeVtable>(),
    },
    runtime_state_get_or_init: bridge_runtime_state_get_or_init,
    raise_exception: bridge_raise_exception,
    exception_pending: bridge_exception_pending,
    alloc_tuple: bridge_alloc_tuple,
    alloc_list: bridge_alloc_list,
    alloc_string: bridge_alloc_string,
    alloc_bytes: bridge_alloc_bytes,
    alloc_dict_with_pairs: bridge_alloc_dict_with_pairs,
    object_type_id: bridge_object_type_id,
    string_obj_to_owned: bridge_string_obj_to_owned,
    type_name: bridge_type_name,
    is_truthy: bridge_is_truthy,
    bytes_like_slice: bridge_bytes_like_slice,
    string_bytes: bridge_string_bytes,
    string_len: bridge_string_len,
    bytes_like_slice_raw: bridge_bytes_like_slice_raw,
    format_obj: bridge_format_obj,
    format_obj_str: bridge_format_obj_str,
    class_name_for_error: bridge_class_name_for_error,
    type_of_bits: bridge_type_of_bits,
    maybe_ptr_from_bits: bridge_maybe_ptr_from_bits,
    molt_is_callable: bridge_molt_is_callable,
    memoryview_is_c_contiguous_view: bridge_memoryview_is_c_contiguous_view,
    memoryview_readonly: bridge_memoryview_readonly,
    memoryview_nbytes: bridge_memoryview_nbytes,
    memoryview_offset: bridge_memoryview_offset,
    memoryview_owner_bits: bridge_memoryview_owner_bits,
    memoryview_data: bridge_memoryview_data,
    release_ptr: bridge_release_ptr,
    dec_ref_bits: bridge_dec_ref_bits,
    inc_ref_bits: bridge_inc_ref_bits,
    to_i64: bridge_to_i64,
    to_f64: bridge_to_f64,
    to_bigint: bridge_to_bigint,
    int_bits_from_i64: bridge_int_bits_from_i64,
    int_bits_from_i128: bridge_int_bits_from_i128,
    int_bits_from_bigint: bridge_int_bits_from_bigint,
    bigint_ptr_from_bits: bridge_bigint_ptr_from_bits,
    bigint_ref: bridge_bigint_ref,
    bigint_from_f64_trunc: bridge_bigint_from_f64_trunc,
    bigint_bits: bridge_bigint_bits,
    bigint_to_inline: bridge_bigint_to_inline,
    index_i64_from_obj: bridge_index_i64_from_obj,
    index_bigint_from_obj: bridge_index_bigint_from_obj,
    call_callable0: bridge_call_callable0,
    call_callable2: bridge_call_callable2,
    attr_lookup_ptr_allow_missing: bridge_attr_lookup_ptr_allow_missing,
    intern_static_name: bridge_intern_static_name,
    bytearray_vec: bridge_bytearray_vec,
    dict_get_in_place: bridge_dict_get_in_place,
    dict_set_in_place: bridge_dict_set_in_place,
    list_len: bridge_list_len,
    seq_vec_ptr: bridge_seq_vec_ptr,
    dict_order_clone: bridge_dict_order_clone,
    molt_iter: bridge_molt_iter,
    molt_iter_next: bridge_molt_iter_next,
    raise_not_iterable: bridge_raise_not_iterable,
    molt_sorted_builtin: bridge_molt_sorted_builtin,
    molt_mul: bridge_molt_mul,
    fill_os_random: bridge_fill_os_random,
    time_local_offset_host: bridge_time_local_offset_host,
    alloc_list_with_capacity: bridge_alloc_list_with_capacity,
    attr_name_bits_from_bytes: bridge_attr_name_bits_from_bytes,
    call_class_init_with_args: bridge_call_class_init_with_args,
    missing_bits: bridge_missing_bits,
    molt_getattr_builtin: bridge_molt_getattr_builtin,
    molt_module_import: bridge_molt_module_import,
    ensure_hashable: bridge_ensure_hashable,
    py_numeric_hash: bridge_py_numeric_hash,
    py_decimal_hash: bridge_py_decimal_hash,
    py_hash_inf: bridge_py_hash_inf,
};

#[unsafe(no_mangle)]
pub extern "C" fn __molt_serial_get_vtable() -> *const RuntimeVtable {
    &RUNTIME_VTABLE as *const RuntimeVtable
}

#[cfg(test)]
mod tests {
    use super::bridge_intern_static_name;
    use crate::{clear_exception, exception_pending, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    fn serial_bridge_interns_protocol_names_in_runtime_state() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"__abs__";
            let bits = bridge_intern_static_name(key.as_ptr(), key.len());
            assert!(!exception_pending(_py));
            assert_eq!(
                runtime_state(_py).interned.abs_name.load(Ordering::Acquire),
                bits
            );
        });
    }

    #[test]
    fn serial_bridge_rejects_unknown_intern_name() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_exception(_py);
            let key = b"__molt_unknown__";
            let _ = bridge_intern_static_name(key.as_ptr(), key.len());
            assert!(exception_pending(_py));
            clear_exception(_py);
        });
    }
}
