//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-regex is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/regex_bridge.rs`.

use molt_runtime_core::prelude::*;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_regex_raise_exception(type_name.as_ptr(), type_name.len(), msg.as_ptr(), msg.len())
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
    fn __molt_regex_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_regex_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_regex_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_regex_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_regex_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_regex_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_regex_alloc_list(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_regex_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_regex_alloc_bytes(data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_regex_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_regex_bytes_view(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_regex_subject_slice(subject_bits: u64, start: usize, end: usize) -> u64;
    fn __molt_regex_pattern_handle_new(pointer: *const std::ffi::c_void) -> u64;
    fn __molt_regex_pattern_handle_clone(bits: u64) -> *const std::ffi::c_void;
    fn __molt_regex_scanner_handle_new(pointer: *const std::ffi::c_void) -> u64;
    fn __molt_regex_scanner_handle_clone(bits: u64) -> *const std::ffi::c_void;
}

pub fn pattern_handle_new(pattern: Arc<crate::PatternHandle>) -> Option<u64> {
    let pointer = Arc::into_raw(pattern).cast();
    let bits = unsafe { __molt_regex_pattern_handle_new(pointer) };
    (bits != MoltObject::none().bits()).then_some(bits)
}

pub fn pattern_handle_arc(bits: u64) -> Option<Arc<crate::PatternHandle>> {
    let pointer = unsafe { __molt_regex_pattern_handle_clone(bits) };
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { Arc::from_raw(pointer.cast()) })
    }
}

pub fn scanner_handle_new(scanner: Arc<Mutex<crate::ScannerHandle>>) -> Option<u64> {
    let pointer = Arc::into_raw(scanner).cast();
    let bits = unsafe { __molt_regex_scanner_handle_new(pointer) };
    (bits != MoltObject::none().bits()).then_some(bits)
}

pub fn scanner_handle_arc(bits: u64) -> Option<Arc<Mutex<crate::ScannerHandle>>> {
    let pointer = unsafe { __molt_regex_scanner_handle_clone(bits) };
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { Arc::from_raw(pointer.cast()) })
    }
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_regex_object_type_id(ptr) }
}

/// Borrow bytes storage for the current intrinsic call.
///
/// # Safety
///
/// The caller must hold the core GIL and keep the runtime object represented
/// by `obj` valid and alive for the entire chosen lifetime `'a`. The returned
/// slice must not escape the intrinsic call. For a memoryview, its export must
/// remain live and its storage unmodified while the slice is borrowed: no
/// Python callback, GC/finalizer boundary, export release or GIL release may
/// occur before that borrow ends.
pub unsafe fn bytes_obj_to_borrowed<'a>(obj: MoltObject) -> Option<&'a [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_regex_bytes_view(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(out_ptr, out_len) })
}

pub fn subject_slice_to_object(subject_bits: u64, start: usize, end: usize) -> u64 {
    unsafe { __molt_regex_subject_slice(subject_bits, start, end) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_dec_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_regex_dec_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_to_i64(bits: u64, out: *mut i64) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_regex_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_seq_snapshot(
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_regex_dict_snapshot(
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
}

/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple for the duration of this call.
pub unsafe fn seq_snapshot(ptr: *mut u8) -> OwnedBridgeHandleSnapshot {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len = 0usize;
    let ok = unsafe { __molt_regex_seq_snapshot(ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 {
        out_ptr = std::ptr::null();
        out_len = 0;
    }
    unsafe { bridge_owned_handle_snapshot(out_ptr, out_len) }
}

/// Returns owned, pinned key/value pairs in insertion order.
/// # Safety
///
/// `ptr` must refer to a live Molt dictionary object for the duration of this
/// call.
pub unsafe fn dict_snapshot(_py: &CoreGilToken, ptr: *mut u8) -> Option<OwnedBridgeHandleSnapshot> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_regex_dict_snapshot(ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 {
        return None;
    }
    Some(unsafe { bridge_owned_handle_snapshot(out_ptr, out_len) })
}
