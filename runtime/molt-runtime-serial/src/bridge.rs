//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-serial is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/serial_bridge.rs`.

use molt_runtime_core::prelude::*;
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_serial_exception_pending() -> i32;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_serial_raise_exception(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { __molt_serial_exception_pending() != 0 }
}

/// Trait for exception return sentinels.
pub trait ExceptionSentinel {
    fn from_bits(bits: u64) -> Self;
}

impl ExceptionSentinel for u64 {
    #[inline]
    fn from_bits(bits: u64) -> Self {
        bits
    }
}

impl<T> ExceptionSentinel for Option<T> {
    #[inline]
    fn from_bits(_bits: u64) -> Self {
        None
    }
}

impl ExceptionSentinel for () {
    #[inline]
    fn from_bits(_bits: u64) -> Self {}
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_serial_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_serial_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_serial_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_serial_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_serial_alloc_list(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_serial_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_serial_alloc_bytes(data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_serial_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_type_name(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_is_truthy(bits: u64) -> i32;
    fn __molt_serial_bytes_like_slice(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_string_bytes(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_string_len(ptr: *mut u8) -> usize;
}

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_serial_object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn type_name(_py: &PyToken, obj: MoltObject) -> Cow<'static, str> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_type_name(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Cow::Owned(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        Cow::Borrowed("<unknown>")
    }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { __molt_serial_is_truthy(obj.bits()) != 0 }
}

pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if __molt_serial_bytes_like_slice(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

pub unsafe fn string_bytes(ptr: *mut u8) -> &'static [u8] {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        __molt_serial_string_bytes(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len)
    }
}

pub fn string_len(ptr: *mut u8) -> usize {
    unsafe { __molt_serial_string_len(ptr) }
}

// ---------------------------------------------------------------------------
// Memoryview / bytes-like helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_bytes_like_slice_raw(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_memoryview_is_c_contiguous_view(ptr: *mut u8) -> i32;
    fn __molt_serial_memoryview_readonly(ptr: *mut u8) -> i32;
    fn __molt_serial_memoryview_nbytes(ptr: *mut u8) -> usize;
    fn __molt_serial_memoryview_offset(ptr: *mut u8) -> isize;
    fn __molt_serial_memoryview_owner_bits(ptr: *mut u8) -> u64;
}

pub unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if __molt_serial_bytes_like_slice_raw(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

pub unsafe fn memoryview_is_c_contiguous_view(ptr: *mut u8) -> bool {
    unsafe { __molt_serial_memoryview_is_c_contiguous_view(ptr) != 0 }
}

pub unsafe fn memoryview_readonly(ptr: *mut u8) -> bool {
    unsafe { __molt_serial_memoryview_readonly(ptr) != 0 }
}

pub unsafe fn memoryview_nbytes(ptr: *mut u8) -> usize {
    unsafe { __molt_serial_memoryview_nbytes(ptr) }
}

pub unsafe fn memoryview_offset(ptr: *mut u8) -> isize {
    unsafe { __molt_serial_memoryview_offset(ptr) }
}

pub unsafe fn memoryview_owner_bits(ptr: *mut u8) -> u64 {
    unsafe { __molt_serial_memoryview_owner_bits(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_release_ptr(ptr: *mut u8);
    fn __molt_serial_dec_ref_bits(bits: u64);
    fn __molt_serial_inc_ref_bits(bits: u64);
}

pub fn release_ptr(ptr: *mut u8) {
    unsafe { __molt_serial_release_ptr(ptr) }
}

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { __molt_serial_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { __molt_serial_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_serial_to_f64(bits: u64, out: *mut f64) -> i32;
    fn __molt_serial_to_bigint(
        bits: u64,
        out_sign: *mut i32,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_serial_int_bits_from_i64(val: i64) -> u64;
    fn __molt_serial_int_bits_from_bigint(
        sign: i32,
        data_ptr: *const u8,
        data_len: usize,
    ) -> u64;
    fn __molt_serial_bigint_ptr_from_bits(bits: u64) -> *mut u8;
    fn __molt_serial_bigint_ref(ptr: *mut u8, out_sign: *mut i32, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_serial_bigint_from_f64_trunc(val: f64, out_sign: *mut i32, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_serial_bigint_bits(sign: i32, data_ptr: *const u8, data_len: usize) -> u64;
    fn __molt_serial_bigint_to_inline(sign: i32, data_ptr: *const u8, data_len: usize) -> u64;
    fn __molt_serial_index_i64_from_obj(
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
    ) -> i64;
    fn __molt_serial_index_bigint_from_obj(
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
        out_sign: *mut i32,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_serial_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut out: f64 = 0.0;
    let ok = unsafe { __molt_serial_to_f64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_bigint(obj: MoltObject) -> Option<num_bigint::BigInt> {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_to_bigint(obj.bits(), &mut out_sign, &mut out_ptr, &mut out_len) };
    if ok == 0 {
        return None;
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    if out_len == 0 {
        return Some(BigInt::from(0));
    }
    let bytes = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

pub fn int_bits_from_i64(_py: &PyToken, val: i64) -> u64 {
    unsafe { __molt_serial_int_bits_from_i64(val) }
}

unsafe extern "C" {
    fn __molt_serial_int_bits_from_i128(val_lo: u64, val_hi: u64) -> u64;
}

pub fn int_bits_from_i128(_py: &PyToken, val: i128) -> u64 {
    let lo = val as u64;
    let hi = (val >> 64) as u64;
    unsafe { __molt_serial_int_bits_from_i128(lo, hi) }
}

pub fn int_bits_from_bigint(_py: &PyToken, value: num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { __molt_serial_int_bits_from_bigint(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn bigint_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { __molt_serial_bigint_ptr_from_bits(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

/// Read the BigInt stored at a raw pointer. The bridge serializes it.
pub fn bigint_ref(ptr: *mut u8) -> num_bigint::BigInt {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_bigint_ref(ptr, &mut out_sign, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return BigInt::from(0);
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    let bytes = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    BigInt::from_bytes_be(sign, &bytes)
}

pub fn bigint_from_f64_trunc(val: f64) -> num_bigint::BigInt {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_bigint_from_f64_trunc(val, &mut out_sign, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return BigInt::from(0);
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    let bytes = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    BigInt::from_bytes_be(sign, &bytes)
}

pub fn bigint_bits(_py: &PyToken, value: &num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { __molt_serial_bigint_bits(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn bigint_to_inline(_py: &PyToken, value: &num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { __molt_serial_bigint_to_inline(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { __molt_serial_index_i64_from_obj(obj_bits, err.as_ptr(), err.len()) }
}

pub fn index_bigint_from_obj(
    _py: &PyToken,
    obj_bits: u64,
    err: &str,
) -> Option<num_bigint::BigInt> {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        __molt_serial_index_bigint_from_obj(
            obj_bits,
            err.as_ptr(),
            err.len(),
            &mut out_sign,
            &mut out_ptr,
            &mut out_len,
        )
    };
    if ok == 0 {
        return None;
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    if out_len == 0 {
        return Some(BigInt::from(0));
    }
    let bytes =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

// ---------------------------------------------------------------------------
// Callable / protocol helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_call_callable0(call_bits: u64) -> u64;
    fn __molt_serial_call_callable2(call_bits: u64, arg0: u64, arg1: u64) -> u64;
    fn __molt_serial_attr_lookup_ptr_allow_missing(ptr: *mut u8, name_bits: u64) -> u64;
    fn __molt_serial_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64;
    fn __molt_serial_class_name_for_error(type_bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_serial_type_of_bits(val_bits: u64) -> u64;
    fn __molt_serial_maybe_ptr_from_bits(bits: u64) -> *mut u8;
    fn __molt_serial_molt_is_callable(bits: u64) -> i32;
    fn __molt_serial_format_obj(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_serial_format_obj_str(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
}

pub fn call_callable0(_py: &PyToken, call_bits: u64) -> u64 {
    unsafe { __molt_serial_call_callable0(call_bits) }
}

pub fn call_callable2(_py: &PyToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    unsafe { __molt_serial_call_callable2(call_bits, arg0, arg1) }
}

pub fn attr_lookup_ptr_allow_missing(_py: &PyToken, ptr: *mut u8, name_bits: u64) -> Option<u64> {
    let result = unsafe { __molt_serial_attr_lookup_ptr_allow_missing(ptr, name_bits) };
    if result == 0 { None } else { Some(result) }
}

pub fn intern_static_name(_py: &PyToken, key: &[u8]) -> u64 {
    unsafe { __molt_serial_intern_static_name(key.as_ptr(), key.len()) }
}

pub fn class_name_for_error(type_bits: u64) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_class_name_for_error(type_bits, &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<unknown>".to_string()
    }
}

pub fn type_of_bits(_py: &PyToken, val_bits: u64) -> u64 {
    unsafe { __molt_serial_type_of_bits(val_bits) }
}

pub fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { __molt_serial_maybe_ptr_from_bits(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

pub fn molt_is_callable(_py: &PyToken, bits: u64) -> bool {
    unsafe { __molt_serial_molt_is_callable(bits) != 0 }
}

pub fn format_obj(_py: &PyToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_format_obj(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<?>".to_string()
    }
}

pub fn format_obj_str(_py: &PyToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_format_obj_str(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<?>".to_string()
    }
}

// ---------------------------------------------------------------------------
// Bytearray helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_serial_bytearray_vec(ptr: *mut u8) -> *mut Vec<u8>;
}

pub unsafe fn bytearray_vec(ptr: *mut u8) -> &'static mut Vec<u8> {
    unsafe { &mut *__molt_serial_bytearray_vec(ptr) }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_serial_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_serial_dict_set_in_place(dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> i32;
    fn __molt_serial_list_len(ptr: *mut u8) -> usize;
    fn __molt_serial_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
}

pub unsafe fn dict_get_in_place(_py: &PyToken, dict_ptr: *mut u8, key_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_serial_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub unsafe fn dict_set_in_place(_py: &PyToken, dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> bool {
    unsafe { __molt_serial_dict_set_in_place(dict_ptr, key_bits, val_bits) != 0 }
}

pub unsafe fn list_len(ptr: *mut u8) -> usize {
    unsafe { __molt_serial_list_len(ptr) }
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_serial_seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_molt_iter(bits: u64) -> u64;
    fn __molt_serial_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32;
    fn __molt_serial_raise_not_iterable(bits: u64) -> u64;
    fn __molt_serial_molt_sorted_builtin(bits: u64) -> u64;
    fn __molt_serial_molt_mul(a: u64, b: u64) -> u64;
}

pub fn molt_iter(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_serial_molt_iter(bits) }
}

pub fn molt_iter_next(_py: &PyToken, iter_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_serial_molt_iter_next(iter_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn raise_not_iterable(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_serial_raise_not_iterable(bits) }
}

pub fn molt_sorted_builtin(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_serial_molt_sorted_builtin(bits) }
}

pub fn molt_mul(_py: &PyToken, a: u64, b: u64) -> u64 {
    unsafe { __molt_serial_molt_mul(a, b) }
}

// ---------------------------------------------------------------------------
// OS randomness
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_fill_os_random(buf_ptr: *mut u8, buf_len: usize) -> i32;
}

pub fn fill_os_random(buf: &mut [u8]) -> Result<(), ()> {
    let ok = unsafe { __molt_serial_fill_os_random(buf.as_mut_ptr(), buf.len()) };
    if ok != 0 { Ok(()) } else { Err(()) }
}

// ---------------------------------------------------------------------------
// Dict helpers (configparser-specific)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_serial_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
    fn __molt_serial_dict_order_clone(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize) -> i32;
}

pub fn alloc_dict_with_pairs(_py: &PyToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_serial_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

/// Returns a cloned copy of the dict's insertion order as a Vec of [k0, v0, k1, v1, ...].
pub fn dict_order_clone(_py: &PyToken, ptr: *mut u8) -> Vec<u64> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_serial_dict_order_clone(ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return Vec::new();
    }
    let boxed = unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u64, out_len)) };
    boxed.into_vec()
}
