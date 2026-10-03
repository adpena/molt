//! FFI bridge to `molt-runtime` for asyncio stdlib intrinsics.
//!
//! Asyncio owns its data-structure state in this crate. The host runtime owns
//! the GIL, object heap, exception state, and interpreter-scoped extension-state
//! registry, so this bridge declares the narrow runtime primitives asyncio
//! needs and nothing else.

use molt_runtime_core::prelude::*;

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

unsafe extern "C" {
    fn __molt_asyncio_runtime_state_get_or_init(
        key_ptr: *const u8,
        key_len: usize,
        init: RuntimeExtensionStateInit,
        clear: RuntimeExtensionStateClear,
        drop: RuntimeExtensionStateDrop,
    ) -> *mut u8;
    fn __molt_asyncio_runtime_state_clear_and_drop(key_ptr: *const u8, key_len: usize) -> i32;

    fn __molt_asyncio_to_i64(bits: u64, out: *mut i64) -> i32;
}

pub fn runtime_state_get_or_init(
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    unsafe { __molt_asyncio_runtime_state_get_or_init(key.as_ptr(), key.len(), init, clear, drop) }
}

pub fn runtime_state_clear_and_drop(key: &[u8]) -> bool {
    unsafe { __molt_asyncio_runtime_state_clear_and_drop(key.as_ptr(), key.len()) != 0 }
}

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

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    T::from_bits(rt_raise_str(type_name, msg))
}

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    rt_dec_ref(bits);
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    rt_inc_ref(bits);
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    rt_is_truthy(obj.bits())
}

pub fn int_bits_from_i64(_py: &PyToken, value: i64) -> u64 {
    rt_int(value)
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out = 0_i64;
    let ok = unsafe { __molt_asyncio_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}
