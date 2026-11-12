//! FFI bridge to molt-runtime internal functions.
//!
//! Uses direct `extern "C"` imports resolved by the linker at link time.
//! No vtable initialization needed — all symbols are resolved from molt-runtime.

use molt_runtime_core::ffi;
use molt_runtime_core::prelude::*;

/// No-op init for API compatibility with the vtable pattern.
/// All symbols are resolved at link time so no initialization is needed.
pub fn init_vtable() {}

// ---------------------------------------------------------------------------
// Runtime-scoped extension state
//
// The itertools class/next-fn/sentinel slots are boxed into the runtime's
// per-interpreter extension-state registry (keyed by an opaque byte string).
// The runtime calls the registered `clear` (release handles) then `drop` (free
// the box) on every interpreter teardown, so the cached object handles never
// outlive the heap that produced them — the satellite analogue of the in-tree
// `RuntimeState.itertools` field.
// ---------------------------------------------------------------------------

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_itertools_runtime_state_get_or_init")]
    fn __molt_itertools_runtime_state_get_or_init(
        __molt_context: u64,
        key_ptr: *const u8,
        key_len: usize,
        init: RuntimeExtensionStateInit,
        clear: RuntimeExtensionStateClear,
        drop: RuntimeExtensionStateDrop,
    ) -> *mut u8;

    /// Interned-aware release of a slot's cached object handle. Mirrors the
    /// in-tree `clear_atomic_bits` (which skips interned objects and otherwise
    /// performs a shutdown release).
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_release_slot_bits") ]
    fn molt_itertools_release_slot_bits(__molt_context: u64, bits: u64);
}

pub fn runtime_state_get_or_init(
    _py: &PyToken,
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    unsafe {
        __molt_itertools_runtime_state_get_or_init(
            _py.attached_context(),
            key.as_ptr(),
            key.len(),
            init,
            clear,
            drop,
        )
    }
}

/// Release a cached object handle from an itertools runtime-state slot.
///
/// # Safety
///
/// Must be called while the runtime GIL is held (the extension-state teardown
/// path holds it). `bits` must be a handle previously stored in a slot.
pub unsafe fn release_runtime_slot_bits(_py: &PyToken, bits: u64) {
    unsafe { molt_itertools_release_slot_bits(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Itertools-specific + runtime C API imports
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    // Itertools-specific helpers (from molt-runtime/itertools_bridge.rs):
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_alloc_instance_for_class") ]
    fn molt_itertools_alloc_instance_for_class(__molt_context: u64, class_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_validate_constructor_class") ]
    fn molt_itertools_validate_constructor_class(
        __molt_context: u64,
        actual: u64,
        expected: u64,
    ) -> bool;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_constructor_repr") ]
    fn molt_itertools_constructor_repr(
        __molt_context: u64,
        instance: u64,
        first: u64,
        second: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_call_callable1") ]
    fn molt_itertools_call_callable1(__molt_context: u64, call_bits: u64, arg0_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_call_callable2_bridge") ]
    fn molt_itertools_call_callable2_bridge(
        __molt_context: u64,
        call_bits: u64,
        arg0_bits: u64,
        arg1_bits: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_tuple_from_iter") ]
    fn molt_itertools_tuple_from_iter(__molt_context: u64, iter_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_itertools_alloc_class") ]
    fn molt_itertools_alloc_class(
        __molt_context: u64,
        name_ptr: *const u8,
        name_len: usize,
        layout_size: i64,
        shape_id: u16,
        iter_fn: u64,
        next_fn: u64,
        new_fn: u64,
        new_arity: u64,
        defaults_ptr: *const u64,
        defaults_len: usize,
    ) -> u64;
    fn molt_itertools_object_class_bits(ptr: *mut u8) -> u64;

    // Already-existing C API exports from molt-runtime:
    #[link_name = molt_runtime_core::attached_symbol!("molt_missing") ]
    fn molt_missing(__molt_context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_add") ]
    fn molt_add(__molt_context: u64, a: u64, b: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_eq") ]
    fn molt_eq(__molt_context: u64, a: u64, b: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_new") ]
    fn molt_callargs_new(__molt_context: u64, pos_capacity_bits: u64, kw_capacity_bits: u64)
    -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_expand_star") ]
    fn molt_callargs_expand_star(__molt_context: u64, builder_bits: u64, iterable_bits: u64)
    -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_call_bind") ]
    fn molt_call_bind(__molt_context: u64, call_bits: u64, builder_bits: u64) -> u64;

    // Runtime object system:
    #[link_name = molt_runtime_core::attached_symbol!("molt_iter")]
    fn molt_iter(context: u64, bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_raise_not_iterable") ]
    fn molt_raise_not_iterable(__molt_context: u64, bits: u64) -> u64;
    fn molt_object_type_id(ptr: *mut u8) -> u32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_seq_snapshot") ]
    fn molt_seq_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    fn molt_seq_read_len(ptr: *mut u8) -> usize;
    fn molt_seq_read_item_gil_borrowed(ptr: *mut u8, index: usize, out: *mut u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_seq_read_item_owned") ]
    fn molt_seq_read_item_owned(
        __molt_context: u64,
        ptr: *mut u8,
        index: usize,
        out: *mut u64,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_index_i64_from_obj") ]
    fn molt_index_i64_from_obj(
        __molt_context: u64,
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
    ) -> i64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_intern_static_name") ]
    fn molt_intern_static_name(__molt_context: u64, key_ptr: *const u8, key_len: usize) -> u64;
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(
    _py: &PyToken,
    type_name: impl Into<ExcKind>,
    msg: &str,
) -> T {
    T::from_bits(rt_raise_str(_py, type_name, msg))
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { ffi::molt_exception_pending(_py.attached_context()) != 0 }
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
    let bits = unsafe {
        ffi::molt_tuple_from_array(_py.attached_context(), elems.as_ptr(), elems.len() as u64)
    };
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

pub fn alloc_list(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    let bits = unsafe {
        ffi::molt_list_from_array(_py.attached_context(), elems.as_ptr(), elems.len() as u64)
    };
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { molt_object_type_id(ptr) }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { ffi::molt_is_truthy(_py.attached_context(), obj.bits()) == 1 }
}

/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple for the duration of this call.
pub unsafe fn seq_snapshot(_py: &PyToken, ptr: *mut u8) -> OwnedBridgeHandleSnapshot<'_> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len = 0usize;
    let ok = unsafe { molt_seq_snapshot(_py.attached_context(), ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 {
        out_ptr = std::ptr::null();
        out_len = 0;
    }
    unsafe { bridge_owned_handle_snapshot(_py, out_ptr, out_len) }
}

/// Return the number of elements in a live runtime sequence.
///
/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple and the caller must hold the
/// GIL for the duration of this call.
pub unsafe fn seq_read_len(ptr: *mut u8) -> usize {
    unsafe { molt_seq_read_len(ptr) }
}

/// Borrow one immutable pool item for immediate use while the GIL and owning
/// tuple remain live.
///
/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple and the caller must hold the
/// GIL. A returned handle is borrowed from that sequence and must not outlive
/// either the sequence or the current GIL-protected operation.
pub unsafe fn seq_read_item_gil_borrowed(ptr: *mut u8, index: usize) -> Option<u64> {
    let mut out = 0u64;
    if unsafe { molt_seq_read_item_gil_borrowed(ptr, index, &mut out) } != 0 {
        Some(out)
    } else {
        None
    }
}

/// Return one owned item from a mutable sequence. The caller assumes the
/// returned reference and must either return it to Python or release it.
///
/// # Safety
///
/// `ptr` must refer to a live Molt mutable sequence and the caller must hold
/// the GIL. Every returned handle transfers one owned reference to the caller.
pub unsafe fn seq_read_item_owned(_py: &PyToken, ptr: *mut u8, index: usize) -> Option<u64> {
    let mut out = 0u64;
    if unsafe { molt_seq_read_item_owned(_py.attached_context(), ptr, index, &mut out) } != 0 {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { ffi::molt_dec_ref_obj(_py.attached_context(), bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { ffi::molt_inc_ref_obj(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

pub fn molt_iter_bridge(_py: &PyToken, bits: u64) -> u64 {
    unsafe { molt_iter(_py.attached_context(), bits) }
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &PyToken, bits: u64) -> T {
    let result = unsafe { molt_raise_not_iterable(_py.attached_context(), bits) };
    T::from_bits(result)
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { molt_index_i64_from_obj(_py.attached_context(), obj_bits, err.as_ptr(), err.len()) }
}

pub fn intern_static_name(_py: &PyToken, key: &[u8]) -> u64 {
    unsafe { molt_intern_static_name(_py.attached_context(), key.as_ptr(), key.len()) }
}

// ---------------------------------------------------------------------------
// Arithmetic / comparison (direct C API)
// ---------------------------------------------------------------------------

pub fn bridge_molt_add(_py: &PyToken, a: u64, b: u64) -> u64 {
    unsafe { molt_add(_py.attached_context(), a, b) }
}

pub fn bridge_molt_eq(_py: &PyToken, a: u64, b: u64) -> u64 {
    unsafe { molt_eq(_py.attached_context(), a, b) }
}

// ---------------------------------------------------------------------------
// Itertools-specific helpers (delegate to C API in molt-runtime)
// ---------------------------------------------------------------------------

pub fn alloc_instance_for_class(_py: &PyToken, class_bits: u64) -> u64 {
    unsafe { molt_itertools_alloc_instance_for_class(_py.attached_context(), class_bits) }
}

pub fn call_callable1(_py: &PyToken, call_bits: u64, arg0_bits: u64) -> u64 {
    unsafe { molt_itertools_call_callable1(_py.attached_context(), call_bits, arg0_bits) }
}

pub fn call_callable2(_py: &PyToken, call_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    unsafe {
        molt_itertools_call_callable2_bridge(
            _py.attached_context(),
            call_bits,
            arg0_bits,
            arg1_bits,
        )
    }
}

pub fn tuple_from_iter_bits(_py: &PyToken, iter_bits: u64) -> Option<u64> {
    let result = unsafe { molt_itertools_tuple_from_iter(_py.attached_context(), iter_bits) };
    if result == 0 { None } else { Some(result) }
}

pub fn alloc_itertools_class(
    _py: &PyToken,
    name: &str,
    layout_size: i64,
    shape: molt_runtime_core::ObjectShapeId,
    iter_fn: u64,
    next_fn: u64,
    constructor: Option<(u64, u64, &[u64])>,
) -> u64 {
    let (new_fn, new_arity, defaults) = constructor.unwrap_or((0, 0, &[]));
    unsafe {
        molt_itertools_alloc_class(
            _py.attached_context(),
            name.as_ptr(),
            name.len(),
            layout_size,
            shape as u16,
            iter_fn,
            next_fn,
            new_fn,
            new_arity,
            defaults.as_ptr(),
            defaults.len(),
        )
    }
}

pub fn validate_constructor_class(_py: &PyToken, actual: u64, expected: u64) -> bool {
    unsafe { molt_itertools_validate_constructor_class(_py.attached_context(), actual, expected) }
}

pub fn constructor_repr(_py: &PyToken, instance: u64, first: u64, second: u64) -> u64 {
    unsafe { molt_itertools_constructor_repr(_py.attached_context(), instance, first, second) }
}

pub fn missing_bits(_py: &PyToken) -> u64 {
    unsafe { molt_missing(_py.attached_context()) }
}

pub fn bridge_callargs_new(_py: &PyToken, pos_cap: u64, kw_cap: u64) -> u64 {
    unsafe { molt_callargs_new(_py.attached_context(), pos_cap, kw_cap) }
}

pub fn bridge_callargs_expand_star(_py: &PyToken, builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe { molt_callargs_expand_star(_py.attached_context(), builder_bits, iterable_bits) }
}

pub fn bridge_call_bind(_py: &PyToken, call_bits: u64, builder_bits: u64) -> u64 {
    unsafe { molt_call_bind(_py.attached_context(), call_bits, builder_bits) }
}

/// Read the class bits from an object.
/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { molt_itertools_object_class_bits(ptr) }
}
