//! Bridge declarations for molt-runtime helper functions.
//!
//! These functions are `pub(crate)` in molt-runtime and cannot be called directly
//! from this crate. We declare them as `extern "C"` here; matching `#[no_mangle]`
//! wrappers in `molt-runtime/src/builtins/compression_bridge.rs` provide the
//! implementations at link time.
//!
//! Convention: all bridge functions use C-ABI-safe types only (u64, i64, bool,
//! raw pointers, usize). No `&PyToken`, `&str`, `MoltObject`, or `Cow` in signatures.

extern "C" {
    // -- Numbers / ints -------------------------------------------------------
    /// `int_bits_from_i64(_py, val) -> u64`
    pub fn molt_bridge_int_bits_from_i64(val: i64) -> u64;

    /// `to_i64(obj) -> Option<i64>`. Returns i64; sets *ok = true/false.
    pub fn molt_bridge_to_i64(obj_bits: u64, ok: *mut bool) -> i64;

    /// `index_i64_from_obj(_py, bits, err) -> i64`
    /// `err_ptr`/`err_len` are a UTF-8 string slice for the error message.
    pub fn molt_bridge_index_i64_from_obj(
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
    ) -> i64;

    // -- Exceptions -----------------------------------------------------------
    /// `raise_exception::<u64>(_py, kind, msg) -> u64`
    /// Both `kind` and `msg` are passed as ptr+len UTF-8 slices.
    pub fn molt_bridge_raise_exception(
        kind_ptr: *const u8,
        kind_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    /// `exception_pending(_py) -> bool`
    pub fn molt_bridge_exception_pending() -> bool;

    // -- Bytes / memory -------------------------------------------------------
    /// `alloc_bytes(_py, data) -> *mut u8` (null on OOM)
    pub fn molt_bridge_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;

    /// `unsafe { bytes_like_slice(ptr) }` -> data ptr + len, or null if not bytes-like.
    /// Returns data pointer (null if not bytes-like); writes length to *out_len.
    pub fn molt_bridge_bytes_like_slice(ptr: *mut u8, out_len: *mut usize) -> *const u8;

    /// `bytes_data(ptr) -> *const u8` (raw data pointer of a bytes object)
    pub fn molt_bridge_bytes_data(ptr: *mut u8) -> *const u8;

    /// `bytes_len(ptr) -> usize`
    pub fn molt_bridge_bytes_len(ptr: *mut u8) -> usize;

    // -- Strings --------------------------------------------------------------
    /// `string_obj_to_owned(obj) -> Option<String>`
    /// Returns ptr to heap-allocated UTF-8 string (caller must free with molt_bridge_free_string),
    /// or null if obj is not a string. Writes length to *out_len.
    pub fn molt_bridge_string_obj_to_owned(obj_bits: u64, out_len: *mut usize) -> *mut u8;

    /// Free a string allocated by `molt_bridge_string_obj_to_owned`.
    pub fn molt_bridge_free_string(ptr: *mut u8, len: usize);

    /// `type_name(_py, obj) -> Cow<str>`
    /// Returns ptr to UTF-8 string; writes length to *out_len.
    /// The returned pointer is valid until the next call to this function (static buffer).
    pub fn molt_bridge_type_name(obj_bits: u64, out_len: *mut usize) -> *const u8;

    /// `alloc_string(_py, s) -> *mut u8`
    pub fn molt_bridge_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;

    // -- Object allocation ----------------------------------------------------
    /// `alloc_list(_py, items, len) -> *mut u8`
    pub fn molt_bridge_alloc_list(items_ptr: *const u64, items_len: usize) -> *mut u8;

    /// `alloc_tuple(_py, items, len) -> *mut u8`
    pub fn molt_bridge_alloc_tuple(items_ptr: *const u64, items_len: usize) -> *mut u8;

    // -- Object inspection ----------------------------------------------------
    /// `object_type_id(ptr) -> u32`
    pub fn molt_bridge_object_type_id(ptr: *mut u8) -> u32;

    // -- Pointer registry -----------------------------------------------------
    /// `release_ptr(ptr)`
    pub fn molt_bridge_release_ptr(ptr: *mut u8);
}

// ── Ergonomic Rust wrappers ─────────────────────────────────────────────────

use molt_runtime_core::prelude::*;

/// Raise an exception and return a sentinel u64.
#[inline]
pub fn raise_exception(_py: &PyToken, kind: &str, msg: &str) -> u64 {
    unsafe {
        molt_bridge_raise_exception(
            kind.as_ptr(),
            kind.len(),
            msg.as_ptr(),
            msg.len(),
        )
    }
}

/// Check whether an exception is currently pending.
#[inline]
pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { molt_bridge_exception_pending() }
}

/// Convert an i64 to NaN-boxed int bits.
#[inline]
pub fn int_bits_from_i64(_py: &PyToken, val: i64) -> u64 {
    unsafe { molt_bridge_int_bits_from_i64(val) }
}

/// Extract i64 from a MoltObject, raising TypeError on failure.
#[inline]
pub fn index_i64_from_obj(_py: &PyToken, bits: u64, err: &str) -> i64 {
    unsafe { molt_bridge_index_i64_from_obj(bits, err.as_ptr(), err.len()) }
}

/// Try to extract i64 from a MoltObject.
#[inline]
pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut ok = false;
    let val = unsafe { molt_bridge_to_i64(obj.bits(), &mut ok) };
    if ok { Some(val) } else { None }
}

/// Allocate a new bytes object. Returns null on OOM.
#[inline]
pub fn alloc_bytes(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { molt_bridge_alloc_bytes(data.as_ptr(), data.len()) }
}

/// Get a slice view of a bytes-like object.
#[inline]
pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut len: usize = 0;
    let data = unsafe { molt_bridge_bytes_like_slice(ptr, &mut len) };
    if data.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(data, len) })
    }
}

/// Extract an owned String from a string MoltObject.
#[inline]
pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut len: usize = 0;
    let ptr = unsafe { molt_bridge_string_obj_to_owned(obj.bits(), &mut len) };
    if ptr.is_null() {
        None
    } else {
        let s = unsafe { String::from_raw_parts(ptr, len, len) };
        Some(s)
    }
}

/// Get the type name of an object.
#[inline]
pub fn type_name(_py: &PyToken, obj: MoltObject) -> String {
    let mut len: usize = 0;
    let ptr = unsafe { molt_bridge_type_name(obj.bits(), &mut len) };
    if ptr.is_null() {
        "object".to_string()
    } else {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf8_lossy(slice).into_owned()
    }
}

/// Release a pointer from the provenance registry.
#[inline]
pub fn release_ptr(ptr: *mut u8) {
    unsafe { molt_bridge_release_ptr(ptr) }
}

/// Allocate a string object.
#[inline]
pub fn alloc_string(_py: &PyToken, s: &str) -> *mut u8 {
    unsafe { molt_bridge_alloc_string(s.as_ptr(), s.len()) }
}

/// Allocate a list object from a slice of item bits.
#[inline]
pub fn alloc_list(_py: &PyToken, items: &[u64]) -> *mut u8 {
    unsafe { molt_bridge_alloc_list(items.as_ptr(), items.len()) }
}

/// Allocate a tuple object from a slice of item bits.
#[inline]
pub fn alloc_tuple(_py: &PyToken, items: &[u64]) -> *mut u8 {
    unsafe { molt_bridge_alloc_tuple(items.as_ptr(), items.len()) }
}

/// Get the type ID of an object from its pointer.
#[inline]
pub fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { molt_bridge_object_type_id(ptr) }
}

/// Get raw bytes data pointer.
#[inline]
pub fn bytes_data(ptr: *mut u8) -> *const u8 {
    unsafe { molt_bridge_bytes_data(ptr) }
}

/// Get bytes length.
#[inline]
pub fn bytes_len(ptr: *mut u8) -> usize {
    unsafe { molt_bridge_bytes_len(ptr) }
}
