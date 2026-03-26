//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-path is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/path_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_path_exception_pending() -> i32;

    fn __molt_path_raise_os_error(
        err_kind: u32,
        err_msg_ptr: *const u8,
        err_msg_len: usize,
        ctx_ptr: *const u8,
        ctx_len: usize,
    ) -> u64;

    fn __molt_path_raise_os_error_errno(
        errno: i64,
        ctx_ptr: *const u8,
        ctx_len: usize,
    ) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_path_raise_exception(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_path_exception_pending() != 0 }
}

pub fn raise_os_error<T: ExceptionSentinel>(_py: &CoreGilToken, err: std::io::Error, ctx: &str) -> T {
    let kind = err.kind() as u32;
    let msg = err.to_string();
    let bits = unsafe {
        __molt_path_raise_os_error(
            kind,
            msg.as_ptr(),
            msg.len(),
            ctx.as_ptr(),
            ctx.len(),
        )
    };
    T::from_bits(bits)
}

pub fn raise_os_error_errno<T: ExceptionSentinel>(_py: &CoreGilToken, errno: i64, ctx: &str) -> T {
    let bits = unsafe {
        __molt_path_raise_os_error_errno(
            errno,
            ctx.as_ptr(),
            ctx.len(),
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
    fn __molt_path_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_path_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_path_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_path_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_path_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_path_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_path_alloc_list(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_path_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_path_alloc_bytes(data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_path_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_path_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_path_is_truthy(bits: u64) -> i32;
    fn __molt_path_bytes_like_slice(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_path_object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_path_is_truthy(obj.bits()) != 0 }
}

pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_bytes_like_slice(ptr, &mut out_ptr, &mut out_len) };
    if ok != 0 {
        Some(unsafe { std::slice::from_raw_parts(out_ptr, out_len) })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_dec_ref_bits(bits: u64);
    fn __molt_path_inc_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_path_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_path_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_path_to_f64(bits: u64, out: *mut f64) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_path_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut out: f64 = 0.0;
    let ok = unsafe { __molt_path_to_f64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_path_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_path_seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_molt_iter(bits: u64) -> u64;
    fn __molt_path_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32;
}

pub fn molt_iter(_py: &CoreGilToken, bits: u64) -> u64 {
    unsafe { __molt_path_molt_iter(bits) }
}

pub fn molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_path_molt_iter_next(iter_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_has_capability(name_ptr: *const u8, name_len: usize) -> i32;
}

pub fn has_capability(_py: &CoreGilToken, name: &str) -> bool {
    unsafe { __molt_path_has_capability(name.as_ptr(), name.len()) != 0 }
}

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_molt_object_hash(bits: u64) -> u64;
}

pub fn molt_object_hash(bits: u64) -> u64 {
    unsafe { __molt_path_molt_object_hash(bits) }
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_path_from_bits(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;

    fn __molt_path_type_name(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn path_from_bits(_py: &CoreGilToken, bits: u64) -> Result<std::path::PathBuf, String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_path_from_bits(bits, &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        let s = String::from_utf8_lossy(&boxed).into_owned();
        Ok(std::path::PathBuf::from(s))
    } else {
        // Error message is in the returned buffer
        if !out_ptr.is_null() && out_len > 0 {
            let boxed =
                unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
            Err(String::from_utf8_lossy(&boxed).into_owned())
        } else {
            Err("path_from_bits failed".to_string())
        }
    }
}

pub fn type_name(_py: &CoreGilToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_type_name(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() && out_len > 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<unknown>".to_string()
    }
}
