//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-ipaddress is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/ipaddress_bridge.rs`.

use molt_runtime_core::prelude::*;
pub use molt_runtime_core::prelude::{opaque_handle_bits, opaque_handle_ptr_from_bits};
use num_bigint::{BigInt, Sign};

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_raise_exception")]
    fn __molt_ipaddr_raise_exception(
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
        __molt_ipaddr_raise_exception(
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_alloc_list")]
    fn __molt_ipaddr_alloc_list(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_alloc_string")]
    fn __molt_ipaddr_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_alloc_bytes")]
    fn __molt_ipaddr_alloc_bytes(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_ipaddr_alloc_list(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_ipaddr_alloc_string(_py.attached_context(), data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_ipaddr_alloc_bytes(_py.attached_context(), data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_ipaddr_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_ipaddr_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_dec_ref_bits")]
    fn __molt_ipaddr_dec_ref_bits(__molt_context: u64, bits: u64);
    fn __molt_ipaddr_release_ptr(ptr: *mut u8);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_ipaddr_dec_ref_bits(_py.attached_context(), bits) }
}

/// # Safety
///
/// `ptr` must have been allocated by the paired runtime bridge and must not be
/// used again after release.
pub unsafe fn release_ptr(ptr: *mut u8) {
    unsafe { __molt_ipaddr_release_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_ipaddr_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_ipaddr_to_bigint(
        bits: u64,
        out_sign: *mut i32,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_int_bits_from_i64")]
    fn __molt_ipaddr_int_bits_from_i64(__molt_context: u64, val: i64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_ipaddr_int_bits_from_bigint")]
    fn __molt_ipaddr_int_bits_from_bigint(
        __molt_context: u64,
        sign: i32,
        bytes_ptr: *const u8,
        bytes_len: usize,
    ) -> u64;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_ipaddr_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

/// Read an arbitrary Python int object as a full-precision `BigInt`.
///
/// Unlike [`to_i64`], this does not clamp to the `i64` range, so it can
/// faithfully represent the entire `0..2**128` IPv6 address space (and any
/// out-of-range value, which the caller range-checks).  Returns `None` when the
/// object is not an integer.
pub fn to_bigint(obj: MoltObject) -> Option<BigInt> {
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { __molt_ipaddr_to_bigint(obj.bits(), &mut out_sign, &mut out_ptr, &mut out_len) };
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
    let bytes = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

pub fn int_bits_from_i64(_py: &CoreGilToken, val: i64) -> u64 {
    unsafe { __molt_ipaddr_int_bits_from_i64(_py.attached_context(), val) }
}

pub fn int_bits_from_bigint(_py: &CoreGilToken, value: BigInt) -> u64 {
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe {
        __molt_ipaddr_int_bits_from_bigint(
            _py.attached_context(),
            sign_i32,
            bytes.as_ptr(),
            bytes.len(),
        )
    }
}
