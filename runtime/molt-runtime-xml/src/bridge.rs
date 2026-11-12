//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-xml is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/xml_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_raise_exception")]
    fn __molt_xml_raise_exception(
        __molt_context: u64,
        kind_code: u16,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    type_name: impl Into<ExcKind>,
    msg: &str,
) -> T {
    let bits = unsafe {
        __molt_xml_raise_exception(
            _py.attached_context(),
            type_name.into().code(),
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_alloc_tuple")]
    fn __molt_xml_alloc_tuple(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_alloc_list")]
    fn __molt_xml_alloc_list(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_alloc_string")]
    fn __molt_xml_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_xml_alloc_tuple(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_xml_alloc_list(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_xml_alloc_string(_py.attached_context(), data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_xml_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_is_truthy")]
    fn __molt_xml_is_truthy(__molt_context: u64, bits: u64) -> i32;
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_xml_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_xml_is_truthy(_py.attached_context(), obj.bits()) != 0 }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_inc_ref_bits")]
    fn __molt_xml_inc_ref_bits(__molt_context: u64, bits: u64);
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_xml_inc_ref_bits(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_xml_to_i64(bits: u64, out: *mut i64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_xml_int_bits_from_i64")]
    fn __molt_xml_int_bits_from_i64(__molt_context: u64, val: i64) -> u64;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_xml_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn int_bits_from_i64(_py: &CoreGilToken, val: i64) -> u64 {
    unsafe { __molt_xml_int_bits_from_i64(_py.attached_context(), val) }
}
