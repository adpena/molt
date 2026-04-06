//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-zoneinfo is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/zoneinfo_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_zoneinfo_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_zoneinfo_raise_exception(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
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
    fn __molt_zoneinfo_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_zoneinfo_alloc_set_with_entries(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_zoneinfo_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_set_with_entries(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_zoneinfo_alloc_set_with_entries(elems.as_ptr(), elems.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_zoneinfo_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_zoneinfo_object_type_id(ptr: *mut u8) -> u32;
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_zoneinfo_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(out_ptr as *mut u8, out_len))
        };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_zoneinfo_object_type_id(ptr) }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_zoneinfo_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
}

/// # Safety
///
/// `ptr` must refer to a live Molt sequence object backed by `Vec<u64>`.
pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_zoneinfo_seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_zoneinfo_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_zoneinfo_int_bits_from_i64(val: i64) -> u64;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_zoneinfo_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn int_bits_from_i64(_py: &CoreGilToken, val: i64) -> u64 {
    unsafe { __molt_zoneinfo_int_bits_from_i64(val) }
}
