//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-crypto is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/crypto_bridge.rs`.

use molt_runtime_core::prelude::*;
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_crypto_exception_pending() -> i32;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_crypto_raise_exception(type_name.as_ptr(), type_name.len(), msg.as_ptr(), msg.len())
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { __molt_crypto_exception_pending() != 0 }
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

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_crypto_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
}

pub fn alloc_bytes(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_crypto_alloc_bytes(data.as_ptr(), data.len()) }
}

pub fn alloc_string(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_crypto_alloc_string(data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_crypto_bytes_like_slice(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_crypto_bytes_like_slice_raw(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_crypto_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_crypto_type_name(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_crypto_is_truthy(bits: u64) -> i32;
}

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_crypto_object_type_id(ptr) }
}

pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if __molt_crypto_bytes_like_slice(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

pub unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if __molt_crypto_bytes_like_slice_raw(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

/// Returns an owned String from a string object, or None.
pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_crypto_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        // The bridge allocates via Box, we must reconstruct and own it.
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

/// Returns a type name string. The bridge allocates via Box<[u8]>.
pub fn type_name(_py: &PyToken, obj: MoltObject) -> Cow<'static, str> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_crypto_type_name(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Cow::Owned(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        Cow::Borrowed("<unknown>")
    }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { __molt_crypto_is_truthy(obj.bits()) != 0 }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_release_ptr(ptr: *mut u8);
    fn __molt_crypto_dec_ref_bits(bits: u64);
    fn __molt_crypto_inc_ref_bits(bits: u64);
}

pub fn release_ptr(ptr: *mut u8) {
    unsafe { __molt_crypto_release_ptr(ptr) }
}

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { __molt_crypto_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { __molt_crypto_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_crypto_int_bits_from_i64(val: i64) -> u64;
    fn __molt_crypto_int_bits_from_bigint(sign: i32, data_ptr: *const u8, data_len: usize) -> u64;
    fn __molt_crypto_index_i64_from_obj(obj_bits: u64, err_ptr: *const u8, err_len: usize) -> i64;
    fn __molt_crypto_index_bigint_from_obj(
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
    let ok = unsafe { __molt_crypto_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn int_bits_from_i64(_py: &PyToken, val: i64) -> u64 {
    unsafe { __molt_crypto_int_bits_from_i64(val) }
}

pub fn int_bits_from_bigint(_py: &PyToken, value: num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { __molt_crypto_int_bits_from_bigint(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { __molt_crypto_index_i64_from_obj(obj_bits, err.as_ptr(), err.len()) }
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
        __molt_crypto_index_bigint_from_obj(
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
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_crypto_dict_len(ptr: *mut u8) -> usize;
    fn __molt_crypto_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_crypto_list_len(ptr: *mut u8) -> usize;
    fn __molt_crypto_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
}

pub unsafe fn dict_len(ptr: *mut u8) -> usize {
    unsafe { __molt_crypto_dict_len(ptr) }
}

pub unsafe fn dict_get_in_place(_py: &PyToken, dict_ptr: *mut u8, key_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_crypto_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub unsafe fn list_len(ptr: *mut u8) -> usize {
    unsafe { __molt_crypto_list_len(ptr) }
}

pub unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { __molt_crypto_seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// OS randomness
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_crypto_fill_os_random(buf_ptr: *mut u8, buf_len: usize) -> i32;
}

pub fn fill_os_random(buf: &mut [u8]) -> Result<(), ()> {
    let ok = unsafe { __molt_crypto_fill_os_random(buf.as_mut_ptr(), buf.len()) };
    if ok != 0 { Ok(()) } else { Err(()) }
}
