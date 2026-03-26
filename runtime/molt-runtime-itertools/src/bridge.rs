//! FFI bridge to molt-runtime internal functions.
//!
//! Uses direct `extern "C"` imports resolved by the linker at link time.
//! No vtable initialization needed — all symbols are resolved from molt-runtime.

use molt_runtime_core::prelude::*;
use molt_runtime_core::ffi;

/// No-op init for API compatibility with the vtable pattern.
/// All symbols are resolved at link time so no initialization is needed.
pub fn init_vtable() {}

// ---------------------------------------------------------------------------
// Itertools-specific + runtime C API imports
// ---------------------------------------------------------------------------

unsafe extern "C" {
    // Itertools-specific helpers (from molt-runtime/itertools_bridge.rs):
    fn molt_itertools_alloc_instance_for_class(class_bits: u64) -> u64;
    fn molt_itertools_call_callable1(call_bits: u64, arg0_bits: u64) -> u64;
    fn molt_itertools_call_callable2_bridge(call_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64;
    fn molt_itertools_tuple_from_iter(iter_bits: u64) -> u64;
    fn molt_itertools_alloc_class(
        name_ptr: *const u8,
        name_len: usize,
        layout_size: i64,
    ) -> u64;
    fn molt_itertools_class_set_iter_next(
        class_bits: u64,
        iter_fn_bits: u64,
        next_fn_bits: u64,
    );
    fn molt_itertools_alloc_function(fn_ptr: u64, arity: u64) -> u64;
    fn molt_itertools_alloc_kwd_mark() -> u64;
    fn molt_itertools_object_class_bits(ptr: *mut u8) -> u64;

    // Already-existing C API exports from molt-runtime:
    fn molt_missing() -> u64;
    fn molt_add(a: u64, b: u64) -> u64;
    fn molt_eq(a: u64, b: u64) -> u64;
    /// Raw `molt_iter_next` — returns a 2-tuple (value, done_bool).
    pub fn molt_iter_next(iter_bits: u64) -> u64;
    fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64;
    fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64;
    fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64;

    // Runtime object system:
    fn molt_iter(bits: u64) -> u64;
    fn molt_raise_not_iterable(bits: u64) -> u64;
    fn molt_object_type_id(ptr: *mut u8) -> u32;
    fn molt_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
    fn molt_index_i64_from_obj(obj_bits: u64, err_ptr: *const u8, err_len: usize) -> i64;
    fn molt_intern_static_name(key_ptr: *const u8, key_len: usize) -> u64;
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    let kind_bits = unsafe { ffi::molt_string_from(type_name.as_ptr(), type_name.len() as u64) };
    let msg_bits = unsafe { ffi::molt_string_from(msg.as_ptr(), msg.len() as u64) };
    let args_bits = unsafe { ffi::molt_tuple_from_array(&msg_bits as *const u64, 1) };
    let exc_bits = unsafe { ffi::molt_exception_new(kind_bits, args_bits) };
    unsafe { ffi::molt_dec_ref_obj(kind_bits) };
    unsafe { ffi::molt_dec_ref_obj(args_bits) };
    let result = unsafe { ffi::molt_raise(exc_bits) };
    T::from_bits(result)
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { ffi::molt_exception_pending() != 0 }
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

pub fn alloc_tuple(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    let bits = unsafe { ffi::molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) };
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

pub fn alloc_list(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    let bits = unsafe { ffi::molt_list_from_array(elems.as_ptr(), elems.len() as u64) };
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { molt_object_type_id(ptr) }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { ffi::molt_is_truthy(obj.bits()) == 1 }
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*molt_seq_vec_ptr(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { ffi::molt_dec_ref_obj(bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { ffi::molt_inc_ref_obj(bits) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

pub fn molt_iter_bridge(_py: &PyToken, bits: u64) -> u64 {
    unsafe { molt_iter(bits) }
}

/// Call `molt_iter_next` — returns a 2-tuple (value, done_bool) or None on error.
pub fn bridge_molt_iter_next(_py: &PyToken, iter_bits: u64) -> u64 {
    unsafe { molt_iter_next(iter_bits) }
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken, bits: u64) -> T {
    let result = unsafe { molt_raise_not_iterable(bits) };
    T::from_bits(result)
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { molt_index_i64_from_obj(obj_bits, err.as_ptr(), err.len()) }
}

pub fn intern_static_name(_py: &PyToken, key: &[u8]) -> u64 {
    unsafe { molt_intern_static_name(key.as_ptr(), key.len()) }
}

// ---------------------------------------------------------------------------
// Arithmetic / comparison (direct C API)
// ---------------------------------------------------------------------------

pub fn bridge_molt_add(a: u64, b: u64) -> u64 {
    unsafe { molt_add(a, b) }
}

pub fn bridge_molt_eq(a: u64, b: u64) -> u64 {
    unsafe { molt_eq(a, b) }
}

// ---------------------------------------------------------------------------
// Itertools-specific helpers (delegate to C API in molt-runtime)
// ---------------------------------------------------------------------------

pub fn alloc_instance_for_class(_py: &PyToken, class_bits: u64) -> u64 {
    unsafe { molt_itertools_alloc_instance_for_class(class_bits) }
}

pub fn call_callable1(_py: &PyToken, call_bits: u64, arg0_bits: u64) -> u64 {
    unsafe { molt_itertools_call_callable1(call_bits, arg0_bits) }
}

pub fn call_callable2(_py: &PyToken, call_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    unsafe { molt_itertools_call_callable2_bridge(call_bits, arg0_bits, arg1_bits) }
}

pub fn tuple_from_iter_bits(_py: &PyToken, iter_bits: u64) -> Option<u64> {
    let result = unsafe { molt_itertools_tuple_from_iter(iter_bits) };
    if result == 0 { None } else { Some(result) }
}

pub fn alloc_itertools_class(_py: &PyToken, name: &str, layout_size: i64) -> u64 {
    unsafe {
        molt_itertools_alloc_class(name.as_ptr(), name.len(), layout_size)
    }
}

pub fn class_set_iter_next(_py: &PyToken, class_bits: u64, iter_fn_bits: u64, next_fn_bits: u64) {
    unsafe {
        molt_itertools_class_set_iter_next(class_bits, iter_fn_bits, next_fn_bits)
    }
}

pub fn alloc_function(_py: &PyToken, fn_ptr: u64, arity: u64) -> u64 {
    unsafe { molt_itertools_alloc_function(fn_ptr, arity) }
}

pub fn alloc_kwd_mark(_py: &PyToken) -> u64 {
    unsafe { molt_itertools_alloc_kwd_mark() }
}

pub fn missing_bits(_py: &PyToken) -> u64 {
    unsafe { molt_missing() }
}

pub fn bridge_callargs_new(pos_cap: u64, kw_cap: u64) -> u64 {
    unsafe { molt_callargs_new(pos_cap, kw_cap) }
}

pub fn bridge_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe { molt_callargs_expand_star(builder_bits, iterable_bits) }
}

pub fn bridge_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    unsafe { molt_call_bind(call_bits, builder_bits) }
}

/// Read the class bits from an object.
pub unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { molt_itertools_object_class_bits(ptr) }
}
