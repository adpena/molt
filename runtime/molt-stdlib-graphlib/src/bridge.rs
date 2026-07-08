//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-stdlib-graphlib is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/graphlib_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_graphlib_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_graphlib_exception_pending() -> i32;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_graphlib_raise_exception(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_graphlib_exception_pending() != 0 }
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
    fn __molt_graphlib_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_graphlib_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_graphlib_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_graphlib_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_graphlib_alloc_list(elems.as_ptr(), elems.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_graphlib_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_graphlib_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_graphlib_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_graphlib_object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_graphlib_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_graphlib_dec_ref_bits(bits: u64);
    fn __molt_graphlib_inc_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_graphlib_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_graphlib_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_graphlib_to_i64(bits: u64, out: *mut i64) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_graphlib_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_graphlib_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_graphlib_dict_set_in_place(dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> i32;
    fn __molt_graphlib_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
}

/// # Safety
///
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
pub unsafe fn dict_get_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_graphlib_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

/// # Safety
///
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
pub unsafe fn dict_set_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> bool {
    unsafe { __molt_graphlib_dict_set_in_place(dict_ptr, key_bits, val_bits) != 0 }
}

/// # Safety
///
/// `ptr` must refer to a live Molt sequence object backed by `Vec<u64>`.
pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_graphlib_seq_vec_ptr(ptr) }
}
