//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-collections is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/collections_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_collections_exception_pending() -> i32;

    fn __molt_collections_raise_key_error_with_key(key_bits: u64) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_collections_raise_exception(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_collections_exception_pending() != 0 }
}

pub fn raise_key_error_with_key<T: ExceptionSentinel>(_py: &CoreGilToken, key_bits: u64) -> T {
    let bits = unsafe { __molt_collections_raise_key_error_with_key(key_bits) };
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
    fn __molt_collections_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_collections_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_collections_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_collections_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_collections_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_collections_alloc_list(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_collections_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_collections_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_collections_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_collections_is_truthy(bits: u64) -> i32;
    fn __molt_collections_type_name(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_collections_object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_collections_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_collections_is_truthy(obj.bits()) != 0 }
}

pub fn type_name(_py: &CoreGilToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_collections_type_name(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "object".to_string()
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_dec_ref_bits(bits: u64);
    fn __molt_collections_inc_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_collections_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_collections_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_collections_index_i64_with_overflow(
        bits: u64,
        type_err_ptr: *const u8,
        type_err_len: usize,
        out: *mut i64,
    ) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_collections_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn index_i64_with_overflow(_py: &CoreGilToken, bits: u64, type_err: &str, _overflow_err: Option<&str>) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe {
        __molt_collections_index_i64_with_overflow(
            bits,
            type_err.as_ptr(),
            type_err.len(),
            &mut out,
        )
    };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_collections_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_collections_dict_set_in_place(dict_ptr: *mut u8, key_bits: u64, val_bits: u64) -> i32;
    fn __molt_collections_dict_del_in_place(dict_ptr: *mut u8, key_bits: u64) -> i32;
    fn __molt_collections_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
    fn __molt_collections_dict_order_clone(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize) -> i32;
}

pub unsafe fn dict_get_in_place(_py: &CoreGilToken, dict_ptr: *mut u8, key_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_collections_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub unsafe fn dict_set_in_place(_py: &CoreGilToken, dict_ptr: *mut u8, key_bits: u64, val_bits: u64) {
    unsafe { __molt_collections_dict_set_in_place(dict_ptr, key_bits, val_bits); }
}

pub unsafe fn dict_del_in_place(_py: &CoreGilToken, dict_ptr: *mut u8, key_bits: u64) -> bool {
    unsafe { __molt_collections_dict_del_in_place(dict_ptr, key_bits) != 0 }
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_collections_seq_vec_ptr(ptr) }
}

/// Returns a cloned copy of the dict's insertion order as a Vec of [k0, v0, k1, v1, ...].
pub fn dict_order_clone(_py: &CoreGilToken, ptr: *mut u8) -> Vec<u64> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_collections_dict_order_clone(ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return Vec::new();
    }
    let boxed =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u64, out_len)) };
    boxed.into_vec()
}

// ---------------------------------------------------------------------------
// Object comparison
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_obj_eq(lhs_bits: u64, rhs_bits: u64) -> i32;
}

pub fn obj_eq(_py: &CoreGilToken, lhs: MoltObject, rhs: MoltObject) -> bool {
    unsafe { __molt_collections_obj_eq(lhs.bits(), rhs.bits()) != 0 }
}

// ---------------------------------------------------------------------------
// Callable helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_collections_call_callable0(call_bits: u64) -> u64;
}

/// Call a callable with zero arguments. Returns the result bits.
pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    unsafe { __molt_collections_call_callable0(call_bits) }
}
