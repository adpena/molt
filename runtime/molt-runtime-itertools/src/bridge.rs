//! FFI bridge to molt-runtime internal functions.
//!
//! All dispatch goes through a single `RuntimeVtable` fetched once at init,
//! plus direct `extern "C"` imports for itertools-specific helpers.

use molt_runtime_core::prelude::*;
use molt_runtime_core::RuntimeVtable;
use std::sync::OnceLock;

/// Global vtable reference, populated once at init time.
static VTABLE: OnceLock<&'static RuntimeVtable> = OnceLock::new();

/// Initialize the vtable. Called once by the runtime at startup.
pub fn init_vtable() {
    unsafe extern "C" {
        fn __molt_itertools_get_vtable() -> *const RuntimeVtable;
    }
    let ptr = unsafe { __molt_itertools_get_vtable() };
    if !ptr.is_null() {
        let vtable = unsafe { &*ptr };
        let _ = VTABLE.set(vtable);
    }
}

/// Get the vtable reference. Panics if not initialized.
#[inline(always)]
fn vt() -> &'static RuntimeVtable {
    VTABLE
        .get()
        .copied()
        .expect("molt-runtime-itertools: vtable not initialized — call bridge::init_vtable() first")
}

// ---------------------------------------------------------------------------
// Itertools-specific C API imports (from molt-runtime/itertools_bridge.rs)
// ---------------------------------------------------------------------------

unsafe extern "C" {
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
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        (vt().raise_exception)(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { (vt().exception_pending)() != 0 }
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
    unsafe { (vt().alloc_tuple)(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    unsafe { (vt().alloc_list)(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { (vt().alloc_string)(data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { (vt().object_type_id)(ptr) }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { (vt().is_truthy)(obj.bits()) != 0 }
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*(vt().seq_vec_ptr)(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { (vt().dec_ref_bits)(bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { (vt().inc_ref_bits)(bits) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

pub fn molt_iter(_py: &PyToken, bits: u64) -> u64 {
    unsafe { (vt().molt_iter)(bits) }
}

/// Call `molt_iter_next` — returns a 2-tuple (value, done_bool) or None on error.
pub fn bridge_molt_iter_next(_py: &PyToken, iter_bits: u64) -> u64 {
    unsafe { molt_iter_next(iter_bits) }
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken, bits: u64) -> T {
    let result = unsafe { (vt().raise_not_iterable)(bits) };
    T::from_bits(result)
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { (vt().index_i64_from_obj)(obj_bits, err.as_ptr(), err.len()) }
}

pub fn intern_static_name(_py: &PyToken, key: &[u8]) -> u64 {
    unsafe { (vt().intern_static_name)(key.as_ptr(), key.len()) }
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
