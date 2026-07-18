//! Narrow object-runtime ABI used by the GPU implementation.
//!
//! GPU algorithms and backend dispatch live in `molt-gpu`; the owning Python
//! runtime remains responsible for object allocation, class state, attributes,
//! calls, and refcounts.  This module is the only place where those layers
//! meet.  Shared stable operations route through `molt-runtime-core`; the
//! smaller set of private object-model operations use GPU-prefixed shims
//! implemented by `molt-runtime/src/gpu_bridge.rs`.

use molt_runtime_core::prelude::*;

pub(super) trait ExceptionSentinel {
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
    fn from_bits(_bits: u64) {}
}

#[inline]
pub(super) fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, kind: &str, message: &str) -> T {
    let bits = unsafe {
        __molt_gpu_raise_exception(kind.as_ptr(), kind.len(), message.as_ptr(), message.len())
    };
    T::from_bits(bits)
}

#[inline]
pub(super) fn exception_pending(_py: &PyToken) -> bool {
    molt_runtime_core::rt_exception_pending()
}

#[inline]
pub(super) fn molt_exception_clear() -> u64 {
    molt_runtime_core::rt_exception_clear()
}

#[inline]
pub(super) fn molt_exception_last() -> u64 {
    unsafe { molt_runtime_core::ffi::molt_exception_last() }
}

#[inline]
pub(super) fn molt_exception_kind(exc_bits: u64) -> u64 {
    unsafe { molt_runtime_core::ffi::molt_exception_kind(exc_bits) }
}

#[inline]
pub(super) fn dec_ref_bits(_py: &PyToken, bits: u64) {
    molt_runtime_core::rt_dec_ref(bits);
}

#[inline]
pub(super) fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    molt_runtime_core::rt_is_truthy(obj.bits())
}

#[inline]
fn ptr_from_owned_bits(bits: u64) -> *mut u8 {
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

#[inline]
pub(super) fn alloc_bytes(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_bytes_from(bytes))
}

#[inline]
pub(super) fn alloc_string(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_string_from_bytes(bytes))
}

#[inline]
pub(super) fn alloc_tuple(_py: &PyToken, values: &[u64]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_tuple(values))
}

#[inline]
pub(super) fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    molt_runtime_core::rt_string_as_bytes(obj.bits())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
}

unsafe extern "C" {
    fn __molt_gpu_raise_exception(
        kind_ptr: *const u8,
        kind_len: usize,
        message_ptr: *const u8,
        message_len: usize,
    ) -> u64;
    fn __molt_gpu_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_gpu_alloc_bytearray(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_gpu_bytes_view(ptr: *mut u8, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_gpu_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_gpu_to_f64(bits: u64, out: *mut f64) -> i32;
    fn __molt_gpu_attr_name_bits(data_ptr: *const u8, data_len: usize) -> u64;
    fn __molt_gpu_object_setattr_raw(
        obj_ptr: *mut u8,
        name_bits: u64,
        name_ptr: *const u8,
        name_len: usize,
        value_bits: u64,
    ) -> i64;
    fn __molt_gpu_alloc_instance_for_class(class_ptr: *mut u8) -> u64;
    fn __molt_gpu_builtin_float() -> u64;
    fn __molt_gpu_object_class_bits(ptr: *mut u8) -> u64;
    fn __molt_gpu_seq_len(ptr: *mut u8) -> usize;
    fn __molt_gpu_seq_snapshot(
        ptr: *mut u8,
        message_ptr: *const u8,
        message_len: usize,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_gpu_seq_visit(
        ptr: *mut u8,
        visitor: unsafe extern "C" fn(*const u64, usize, *mut std::ffi::c_void),
        context: *mut std::ffi::c_void,
    ) -> i32;
    fn __molt_gpu_seq_pin_item(ptr: *mut u8, index: usize, out: *mut u64) -> i32;
    fn __molt_gpu_alloc_list_owned(
        elems_ptr: *const u64,
        elems_len: usize,
        capacity: usize,
    ) -> *mut u8;
    fn __molt_gpu_callargs_positional_snapshot(
        builder_bits: u64,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_gpu_clone_callargs_builder(builder_bits: u64, out: *mut u64) -> i32;
    fn __molt_gpu_missing_bits() -> u64;
    fn __molt_gpu_call_callable1(call_bits: u64, arg_bits: u64) -> u64;

}

mod ffi {
    unsafe extern "C" {
        pub(super) fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64;
        pub(super) fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64)
        -> u64;
        pub(super) fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64;
        pub(super) fn molt_module_cache_get(name_bits: u64) -> u64;
        pub(super) fn molt_module_import(name_bits: u64) -> u64;
        pub(super) fn molt_isinstance(value_bits: u64, class_bits: u64) -> u64;
    }
}

#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    #[link_name = "molt_gpu_webgpu_dispatch_host"]
    pub(super) fn molt_gpu_webgpu_dispatch_host(
        source_ptr: u32,
        source_len: u32,
        entry_ptr: u32,
        entry_len: u32,
        bindings_ptr: u32,
        bindings_len: u32,
        grid: u32,
        workgroup_size: u32,
        err_ptr: u32,
        err_cap: u32,
        out_err_len_ptr: *mut u32,
    ) -> i32;
}

#[inline]
pub(super) unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_gpu_object_type_id(ptr) }
}

#[inline]
pub(super) fn alloc_bytearray(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    unsafe { __molt_gpu_alloc_bytearray(bytes.as_ptr(), bytes.len()) }
}

#[inline]
unsafe fn bytes_view(ptr: *mut u8) -> (*const u8, usize) {
    let mut data = std::ptr::null();
    let mut len = 0;
    let ok = unsafe { __molt_gpu_bytes_view(ptr, &mut data, &mut len) };
    if ok == 0 {
        (std::ptr::null(), 0)
    } else {
        (data, len)
    }
}

#[inline]
pub(super) unsafe fn bytes_data(ptr: *mut u8) -> *const u8 {
    unsafe { bytes_view(ptr).0 }
}

#[inline]
pub(super) unsafe fn bytes_len(ptr: *mut u8) -> usize {
    unsafe { bytes_view(ptr).1 }
}

#[inline]
pub(super) fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut value = 0;
    (unsafe { __molt_gpu_to_i64(obj.bits(), &mut value) } != 0).then_some(value)
}

#[inline]
pub(super) fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut value = 0.0;
    (unsafe { __molt_gpu_to_f64(obj.bits(), &mut value) } != 0).then_some(value)
}

#[inline]
pub(super) fn attr_name_bits_from_bytes(_py: &PyToken, name: &[u8]) -> Option<u64> {
    let bits = unsafe { __molt_gpu_attr_name_bits(name.as_ptr(), name.len()) };
    (!obj_from_bits(bits).is_none()).then_some(bits)
}

#[inline]
pub(super) fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    unsafe { ffi::molt_get_attr_name(obj_bits, name_bits) }
}

#[inline]
pub(super) unsafe fn object_setattr_raw(
    _py: &PyToken,
    obj_ptr: *mut u8,
    name_bits: u64,
    name: &str,
    value_bits: u64,
) -> i64 {
    unsafe {
        __molt_gpu_object_setattr_raw(obj_ptr, name_bits, name.as_ptr(), name.len(), value_bits)
    }
}

#[inline]
pub(super) unsafe fn alloc_instance_for_class(_py: &PyToken, class_ptr: *mut u8) -> u64 {
    unsafe { __molt_gpu_alloc_instance_for_class(class_ptr) }
}

#[inline]
pub(super) fn builtin_float(_py: &PyToken) -> u64 {
    unsafe { __molt_gpu_builtin_float() }
}

#[inline]
pub(super) unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { __molt_gpu_object_class_bits(ptr) }
}

#[inline]
pub(super) fn alloc_list_with_capacity_owned(
    _py: &PyToken,
    values: &[u64],
    capacity: usize,
) -> *mut u8 {
    unsafe { __molt_gpu_alloc_list_owned(values.as_ptr(), values.len(), capacity) }
}

#[inline]
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) unsafe fn callargs_positional_snapshot(
    _py: &PyToken,
    builder_bits: u64,
) -> Result<Vec<u64>, u64> {
    let mut ptr = std::ptr::null();
    let mut len = 0;
    if unsafe { __molt_gpu_callargs_positional_snapshot(builder_bits, &mut ptr, &mut len) } == 0 {
        return Err(MoltObject::none().bits());
    }
    Ok(unsafe { bridge_owned_u64_to_vec(ptr, len) })
}

#[inline]
pub(super) unsafe fn clone_callargs_builder_bits(
    _py: &PyToken,
    builder_bits: u64,
) -> Result<u64, u64> {
    let mut out = MoltObject::none().bits();
    if unsafe { __molt_gpu_clone_callargs_builder(builder_bits, &mut out) } == 0 {
        Err(out)
    } else {
        Ok(out)
    }
}

#[inline]
pub(super) fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    unsafe { ffi::molt_getattr_builtin(obj_bits, name_bits, default_bits) }
}

#[inline]
pub(super) fn missing_bits(_py: &PyToken) -> u64 {
    unsafe { __molt_gpu_missing_bits() }
}

#[inline]
pub(super) fn is_missing_bits(_py: &PyToken, bits: u64) -> bool {
    bits == unsafe { __molt_gpu_missing_bits() }
}

#[inline]
pub(super) fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    unsafe { ffi::molt_call_bind(call_bits, builder_bits) }
}

#[inline]
pub(super) fn molt_module_cache_get(name_bits: u64) -> u64 {
    unsafe { ffi::molt_module_cache_get(name_bits) }
}

#[inline]
pub(super) fn molt_module_import(name_bits: u64) -> u64 {
    unsafe { ffi::molt_module_import(name_bits) }
}

#[inline]
pub(super) fn molt_isinstance(value_bits: u64, class_bits: u64) -> u64 {
    unsafe { ffi::molt_isinstance(value_bits, class_bits) }
}

#[inline]
pub(super) unsafe fn call_callable1(_py: &PyToken, call_bits: u64, arg_bits: u64) -> u64 {
    unsafe { __molt_gpu_call_callable1(call_bits, arg_bits) }
}

pub(super) mod seq_access {
    use super::*;

    pub(crate) struct PinnedSequenceSnapshot(OwnedBridgeHandleSnapshot);

    impl std::ops::Deref for PinnedSequenceSnapshot {
        type Target = [u64];

        #[inline]
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    pub(crate) struct PinnedItem(u64);

    impl PinnedItem {
        #[inline]
        pub(crate) fn bits(&self) -> u64 {
            self.0
        }
    }

    impl Drop for PinnedItem {
        #[inline]
        fn drop(&mut self) {
            molt_runtime_core::rt_dec_ref(self.0);
        }
    }

    #[inline]
    pub(crate) unsafe fn len(ptr: *mut u8) -> usize {
        unsafe { __molt_gpu_seq_len(ptr) }
    }

    pub(crate) unsafe fn snapshot(
        _py: &PyToken,
        ptr: *mut u8,
        message: &str,
    ) -> Option<PinnedSequenceSnapshot> {
        let mut out_ptr = std::ptr::null();
        let mut out_len = 0;
        let ok = unsafe {
            __molt_gpu_seq_snapshot(
                ptr,
                message.as_ptr(),
                message.len(),
                &mut out_ptr,
                &mut out_len,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(PinnedSequenceSnapshot(unsafe {
            bridge_owned_handle_snapshot(out_ptr, out_len)
        }))
    }

    /// Execute one scoped read while the runtime retains sequence custody.
    ///
    /// The slice never crosses the callback boundary: the runtime invokes the
    /// visitor synchronously while its storage guard is live.  Panics are
    /// captured inside the visitor and resumed only after control returns to
    /// Rust, so no unwind can cross the C ABI.  This preserves the zero-copy
    /// hot path without weakening free-threaded synchronization.
    pub(crate) unsafe fn with_borrowed<R, F>(ptr: *mut u8, body: F) -> R
    where
        F: FnOnce(&[u64]) -> R,
    {
        struct VisitState<F, R> {
            body: Option<F>,
            result: Option<R>,
            panic: Option<Box<dyn std::any::Any + Send>>,
        }

        unsafe extern "C" fn visit<F, R>(
            values_ptr: *const u64,
            values_len: usize,
            context: *mut std::ffi::c_void,
        ) where
            F: FnOnce(&[u64]) -> R,
        {
            let state = unsafe { &mut *(context.cast::<VisitState<F, R>>()) };
            let values = if values_len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(values_ptr, values_len) }
            };
            let body = state
                .body
                .take()
                .expect("GPU sequence visitor called twice");
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(values))) {
                Ok(result) => state.result = Some(result),
                Err(payload) => state.panic = Some(payload),
            }
        }

        let mut state = VisitState {
            body: Some(body),
            result: None,
            panic: None,
        };
        let visited = unsafe {
            __molt_gpu_seq_visit(
                ptr,
                visit::<F, R>,
                (&raw mut state).cast::<std::ffi::c_void>(),
            )
        };
        if let Some(payload) = state.panic {
            std::panic::resume_unwind(payload);
        }
        if visited == 0 {
            let body = state
                .body
                .take()
                .expect("GPU sequence visitor was not invoked");
            return body(&[]);
        }
        state
            .result
            .expect("GPU sequence visitor returned no result")
    }

    pub(crate) unsafe fn pin_item(_py: &PyToken, ptr: *mut u8, index: usize) -> Option<PinnedItem> {
        let mut bits = 0;
        if unsafe { __molt_gpu_seq_pin_item(ptr, index, &mut bits) } == 0 {
            None
        } else {
            Some(PinnedItem(bits))
        }
    }

    #[inline]
    pub(crate) unsafe fn pin_tuple(py: &PyToken, ptr: *mut u8) -> Option<PinnedSequenceSnapshot> {
        unsafe { snapshot(py, ptr, "GPU tuple snapshot allocation failed") }
    }
}
