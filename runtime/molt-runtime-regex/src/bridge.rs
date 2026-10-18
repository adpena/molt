//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-regex is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/regex_bridge.rs`.

use molt_runtime_core::prelude::*;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Runtime-scoped extension state
// ---------------------------------------------------------------------------

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

unsafe extern "C" {
    fn __molt_regex_runtime_state_get_or_init(
        key_ptr: *const u8,
        key_len: usize,
        init: RuntimeExtensionStateInit,
        clear: RuntimeExtensionStateClear,
        drop: RuntimeExtensionStateDrop,
    ) -> *mut u8;
}

pub fn runtime_state_get_or_init(
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    unsafe { __molt_regex_runtime_state_get_or_init(key.as_ptr(), key.len(), init, clear, drop) }
}

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

    fn __molt_regex_exception_pending() -> i32;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_regex_raise_exception(type_name.as_ptr(), type_name.len(), msg.as_ptr(), msg.len())
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_regex_exception_pending() != 0 }
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
    fn __molt_regex_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_regex_bytes_pattern_to_string(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_regex_bytes_view(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_regex_subject_slice(subject_bits: u64, start: usize, end: usize) -> u64;
    fn __molt_regex_is_truthy(bits: u64) -> i32;
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

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_regex_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn bytes_pattern_to_string(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { __molt_regex_bytes_pattern_to_string(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

/// Borrow exact bytes storage for the current intrinsic call. The caller must
/// keep the Python argument live and perform no callback before it is done.
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

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_regex_is_truthy(obj.bits()) != 0 }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_dec_ref_bits(bits: u64);
    fn __molt_regex_inc_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_regex_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_regex_inc_ref_bits(bits) }
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

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_regex_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_regex_dict_set_in_place(dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> i32;
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
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
pub unsafe fn dict_get_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Result<Option<u64>, molt_runtime_core::ErrorIndicatorSet> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_regex_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    match ok {
        1 => Ok(Some(out)),
        0 => Ok(None),
        -1 => Err(molt_runtime_core::ErrorIndicatorSet),
        _ => unreachable!("invalid dictionary lookup status"),
    }
}

/// # Safety
///
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
#[must_use]
pub unsafe fn dict_set_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> bool {
    match unsafe { __molt_regex_dict_set_in_place(dict_ptr, key_bits, val_bits) } {
        1 => true,
        0 => false,
        _ => unreachable!("invalid dictionary mutation status"),
    }
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

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_molt_iter(bits: u64) -> u64;
    fn __molt_regex_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32;
}

pub fn molt_iter(_py: &CoreGilToken, bits: u64) -> u64 {
    unsafe { __molt_regex_molt_iter(bits) }
}

pub fn molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_regex_molt_iter_next(iter_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Attribute / callable helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_regex_attr_name_bits_from_bytes(key_ptr: *const u8, key_len: usize) -> u64;
    fn __molt_regex_call_callable1(call_bits: u64, arg0: u64) -> u64;
}

/// Look up or intern an attribute name from raw bytes.
/// Returns Some(bits) on success, None if the name is invalid.
pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    let result = unsafe { __molt_regex_attr_name_bits_from_bytes(name.as_ptr(), name.len()) };
    // Convention: 0 signals failure.
    if result == 0 { None } else { Some(result) }
}

/// Call a callable with one argument. Returns the result bits.
pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    unsafe { __molt_regex_call_callable1(call_bits, arg0) }
}
