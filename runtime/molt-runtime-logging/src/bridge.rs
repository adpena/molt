//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-logging is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/logging_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Runtime-scoped extension state
// ---------------------------------------------------------------------------

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_logging_runtime_state_get_or_init")]
    fn __molt_logging_runtime_state_get_or_init(
        __molt_context: u64,
        key_ptr: *const u8,
        key_len: usize,
        init: RuntimeExtensionStateInit,
        clear: RuntimeExtensionStateClear,
        drop: RuntimeExtensionStateDrop,
    ) -> *mut u8;
}

pub fn runtime_state_get_or_init(
    _py: &PyToken,
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    unsafe {
        __molt_logging_runtime_state_get_or_init(
            _py.attached_context(),
            key.as_ptr(),
            key.len(),
            init,
            clear,
            drop,
        )
    }
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_logging_raise_exception")]
    fn __molt_logging_raise_exception(
        __molt_context: u64,
        kind_code: u16,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_logging_exception_pending")]
    fn __molt_logging_exception_pending(__molt_context: u64) -> i32;
}

pub fn raise_exception<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    type_name: impl Into<ExcKind>,
    msg: &str,
) -> T {
    let bits = unsafe {
        __molt_logging_raise_exception(
            _py.attached_context(),
            type_name.into().code(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_logging_exception_pending(_py.attached_context()) != 0 }
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_logging_alloc_string")]
    fn __molt_logging_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_logging_alloc_string(_py.attached_context(), data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_logging_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_logging_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_logging_dec_ref_bits")]
    fn __molt_logging_dec_ref_bits(__molt_context: u64, bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_logging_dec_ref_bits(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_logging_to_i64(bits: u64, out: *mut i64) -> i32;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_logging_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}
