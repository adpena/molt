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
pub(super) fn raise_exception<T: ExceptionSentinel>(
    _py: &PyToken,
    kind: impl Into<ExcKind>,
    message: &str,
) -> T {
    let bits = unsafe {
        __molt_gpu_raise_exception(
            _py.attached_context(),
            kind.into().code(),
            message.as_ptr(),
            message.len(),
        )
    };
    T::from_bits(bits)
}

#[inline]
pub(super) fn exception_pending(_py: &PyToken) -> bool {
    molt_runtime_core::rt_exception_pending(_py)
}

#[inline]
pub(super) fn molt_exception_clear(_py: &PyToken) -> u64 {
    molt_runtime_core::rt_exception_clear(_py)
}

#[inline]
pub(super) fn lookup_attribute(
    _py: &PyToken,
    obj_bits: u64,
    name_bits: u64,
) -> molt_obj_model::AttributeLookup {
    unsafe { __molt_gpu_lookup_attribute(_py.attached_context(), obj_bits, name_bits) }
}

#[inline]
pub(super) fn dec_ref_bits(_py: &PyToken, bits: u64) {
    molt_runtime_core::rt_dec_ref(_py, bits);
}

#[inline]
pub(super) fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    molt_runtime_core::rt_is_truthy(_py, obj.bits())
}

#[inline]
fn ptr_from_owned_bits(bits: u64) -> *mut u8 {
    obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
}

#[inline]
pub(super) fn alloc_bytes(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_bytes_from(_py, bytes))
}

#[inline]
pub(super) fn alloc_string(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_string_from_bytes(_py, bytes))
}

#[inline]
pub(super) fn alloc_tuple(_py: &PyToken, values: &[u64]) -> *mut u8 {
    ptr_from_owned_bits(molt_runtime_core::rt_tuple(_py, values))
}

unsafe extern "C" {
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_bytearray_copy")]
    fn __molt_gpu_bytearray_copy(
        __molt_context: u64,
        bits: u64,
        bytes: *const u8,
        len: usize,
        same_size: i32,
    ) -> i32;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_commit_buffer_data")]
    fn __molt_gpu_commit_buffer_data(__molt_context: u64, object: u64, old: u64, new: u64) -> i32;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_class_binding")]
    fn __molt_gpu_class_binding(__molt_context: u64, bits: u64, name: *const u8, len: usize)
    -> u64;

    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_default_field")]
    fn __molt_gpu_default_field(__molt_context: u64, bits: u64, name: *const u8, len: usize)
    -> u64;

    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_function_binding")]
    fn __molt_gpu_function_binding(
        __molt_context: u64,
        bits: u64,
        name: *const u8,
        len: usize,
    ) -> u64;

    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_builtin_matches")]
    fn __molt_gpu_builtin_matches(
        __molt_context: u64,
        bits: u64,
        name: *const u8,
        len: usize,
    ) -> i32;

    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_kernel_descriptor")]
    fn __molt_gpu_kernel_descriptor(__molt_context: u64, callable: u64) -> u64;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_compiled_body_matches")]
    fn __molt_gpu_compiled_body_matches(
        __molt_context: u64,
        bits: u64,
        symbol: *const u8,
        symbol_len: usize,
        arity: u64,
        defaults: *const u64,
        defaults_len: usize,
    ) -> i32;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_descriptor_is_current")]
    fn __molt_gpu_descriptor_is_current(__molt_context: u64, callable: u64, descriptor: u64)
    -> i32;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_kernel_global")]
    fn __molt_gpu_kernel_global(
        __molt_context: u64,
        callable: u64,
        name: *const u8,
        len: usize,
    ) -> u64;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_module_binding")]
    fn __molt_gpu_module_binding(
        __molt_context: u64,
        module: u64,
        name: *const u8,
        len: usize,
    ) -> u64;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_intrinsic_matches")]
    fn __molt_gpu_intrinsic_matches(
        __molt_context: u64,
        bits: u64,
        name: *const u8,
        len: usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_lookup_attribute")]
    fn __molt_gpu_lookup_attribute(
        __molt_context: u64,
        obj_bits: u64,
        name_bits: u64,
    ) -> molt_obj_model::AttributeLookup;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_raise_exception")]
    fn __molt_gpu_raise_exception(
        __molt_context: u64,
        kind_code: u16,
        message_ptr: *const u8,
        message_len: usize,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_object_type_id")]
    fn __molt_gpu_object_type_id(__molt_context: u64, ptr: *mut u8) -> u32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_alloc_bytearray")]
    fn __molt_gpu_alloc_bytearray(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_bytes_view")]
    fn __molt_gpu_bytes_view(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_to_i64")]
    fn __molt_gpu_to_i64(__molt_context: u64, bits: u64, out: *mut i64) -> i32;
    #[cfg(any(
        target_arch = "wasm32",
        all(target_os = "macos", feature = "metal-backend"),
        all(not(target_arch = "wasm32"), feature = "webgpu-backend")
    ))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_exact_scalar_kind")]
    fn __molt_gpu_exact_scalar_kind(__molt_context: u64, bits: u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_to_f64")]
    fn __molt_gpu_to_f64(__molt_context: u64, bits: u64, out: *mut f64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_attr_name_bits")]
    fn __molt_gpu_attr_name_bits(__molt_context: u64, data_ptr: *const u8, data_len: usize) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_object_setattr_raw")]
    fn __molt_gpu_object_setattr_raw(
        __molt_context: u64,
        obj_ptr: *mut u8,
        name_bits: u64,
        name_ptr: *const u8,
        name_len: usize,
        value_bits: u64,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_alloc_instance_for_class")]
    fn __molt_gpu_alloc_instance_for_class(__molt_context: u64, class_ptr: *mut u8) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_builtin_float")]
    fn __molt_gpu_builtin_float(__molt_context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_object_class_bits")]
    fn __molt_gpu_object_class_bits(__molt_context: u64, ptr: *mut u8) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_seq_len")]
    fn __molt_gpu_seq_len(__molt_context: u64, ptr: *mut u8) -> usize;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_seq_snapshot")]
    fn __molt_gpu_seq_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        message_ptr: *const u8,
        message_len: usize,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_seq_visit")]
    fn __molt_gpu_seq_visit(
        __molt_context: u64,
        ptr: *mut u8,
        visitor: unsafe extern "C" fn(*const u64, usize, *mut std::ffi::c_void),
        context: *mut std::ffi::c_void,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_seq_pin_item")]
    fn __molt_gpu_seq_pin_item(
        __molt_context: u64,
        ptr: *mut u8,
        index: usize,
        out: *mut u64,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_alloc_list_owned")]
    fn __molt_gpu_alloc_list_owned(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
        capacity: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_bind_kernel_arguments")]
    fn __molt_gpu_bind_kernel_arguments(__molt_context: u64, callable: u64, builder: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_clone_callargs_builder")]
    fn __molt_gpu_clone_callargs_builder(
        __molt_context: u64,
        builder_bits: u64,
        out: *mut u64,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_missing_bits")]
    fn __molt_gpu_missing_bits(__molt_context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_gpu_call_callable1")]
    fn __molt_gpu_call_callable1(__molt_context: u64, call_bits: u64, arg_bits: u64) -> u64;

}

mod ffi {
    unsafe extern "C" {
        #[link_name = molt_runtime_core::attached_symbol!("molt_get_attr_name") ]
        pub(super) fn molt_get_attr_name(__molt_context: u64, obj_bits: u64, name_bits: u64)
        -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_getattr_builtin") ]
        pub(super) fn molt_getattr_builtin(
            __molt_context: u64,
            obj_bits: u64,
            name_bits: u64,
            default_bits: u64,
        ) -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_call_bind") ]
        pub(super) fn molt_call_bind(__molt_context: u64, call_bits: u64, builder_bits: u64)
        -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_module_cache_get") ]
        pub(super) fn molt_module_cache_get(__molt_context: u64, name_bits: u64) -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_module_import") ]
        pub(super) fn molt_module_import(__molt_context: u64, name_bits: u64) -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_isinstance") ]
        pub(super) fn molt_isinstance(__molt_context: u64, value_bits: u64, class_bits: u64)
        -> u64;
        #[cfg(any(
            target_arch = "wasm32",
            all(target_os = "macos", feature = "metal-backend"),
            all(not(target_arch = "wasm32"), feature = "webgpu-backend")
        ))]
        #[link_name = molt_runtime_core::attached_symbol!("molt_type_of") ]
        pub(super) fn molt_type_of(__molt_context: u64, value_bits: u64) -> u64;
    }
}

// The browser host provides this import in the same `env` module that
// molt-runtime declares it in. Without the module attribute the symbol is a
// plain undefined reference and the runtime cdylib link fails under
// `molt_gpu_primitives`.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
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
pub(super) unsafe fn object_type_id(_py: &PyToken, ptr: *mut u8) -> u32 {
    unsafe { __molt_gpu_object_type_id(_py.attached_context(), ptr) }
}

#[inline]
pub(super) fn alloc_bytearray(_py: &PyToken, bytes: &[u8]) -> *mut u8 {
    unsafe { __molt_gpu_alloc_bytearray(_py.attached_context(), bytes.as_ptr(), bytes.len()) }
}

#[inline]
unsafe fn bytes_view(_py: &PyToken, ptr: *mut u8) -> (*const u8, usize) {
    let mut data = std::ptr::null();
    let mut len = 0;
    let ok = unsafe { __molt_gpu_bytes_view(_py.attached_context(), ptr, &mut data, &mut len) };
    if ok == 0 {
        (std::ptr::null(), 0)
    } else {
        (data, len)
    }
}

#[inline]
pub(super) unsafe fn bytes_data(_py: &PyToken, ptr: *mut u8) -> *const u8 {
    unsafe { bytes_view(_py, ptr).0 }
}

#[inline]
pub(super) unsafe fn bytes_len(_py: &PyToken, ptr: *mut u8) -> usize {
    unsafe { bytes_view(_py, ptr).1 }
}

#[inline]
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn exact_scalar_kind(_py: &PyToken, bits: u64) -> i32 {
    unsafe { __molt_gpu_exact_scalar_kind(_py.attached_context(), bits) }
}

pub(super) fn to_i64(_py: &PyToken, obj: MoltObject) -> Option<i64> {
    let mut value = 0;
    (unsafe { __molt_gpu_to_i64(_py.attached_context(), obj.bits(), &mut value) } != 0)
        .then_some(value)
}

#[inline]
pub(super) fn to_f64(_py: &PyToken, obj: MoltObject) -> Option<f64> {
    let mut value = 0.0;
    (unsafe { __molt_gpu_to_f64(_py.attached_context(), obj.bits(), &mut value) } != 0)
        .then_some(value)
}

#[inline]
pub(super) fn attr_name_bits_from_bytes(_py: &PyToken, name: &[u8]) -> Option<u64> {
    let bits =
        unsafe { __molt_gpu_attr_name_bits(_py.attached_context(), name.as_ptr(), name.len()) };
    (!obj_from_bits(bits).is_none()).then_some(bits)
}

#[inline]
pub(super) fn molt_get_attr_name(_py: &PyToken, obj_bits: u64, name_bits: u64) -> u64 {
    unsafe { ffi::molt_get_attr_name(_py.attached_context(), obj_bits, name_bits) }
}

#[inline]
pub(super) unsafe fn object_setattr_raw(
    _py: &PyToken,
    obj_ptr: *mut u8,
    name_bits: u64,
    name: &str,
    value_bits: u64,
) -> Result<(), ()> {
    let status = unsafe {
        __molt_gpu_object_setattr_raw(
            _py.attached_context(),
            obj_ptr,
            name_bits,
            name.as_ptr(),
            name.len(),
            value_bits,
        )
    };
    if status == 0 { Ok(()) } else { Err(()) }
}

#[inline]
pub(super) unsafe fn alloc_instance_for_class(_py: &PyToken, class_ptr: *mut u8) -> u64 {
    unsafe { __molt_gpu_alloc_instance_for_class(_py.attached_context(), class_ptr) }
}

#[inline]
pub(super) fn builtin_float(_py: &PyToken) -> u64 {
    unsafe { __molt_gpu_builtin_float(_py.attached_context()) }
}

#[inline]
pub(super) unsafe fn object_class_bits(_py: &PyToken, ptr: *mut u8) -> u64 {
    unsafe { __molt_gpu_object_class_bits(_py.attached_context(), ptr) }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn type_of_owned(_py: &PyToken, bits: u64) -> u64 {
    // The semantic type authority handles implicit builtin class edges and
    // explicit subclass identities. The caller owns the returned reference.
    unsafe { ffi::molt_type_of(_py.attached_context(), bits) }
}

#[inline]
pub(super) fn alloc_list_with_capacity_owned(
    _py: &PyToken,
    values: &[u64],
    capacity: usize,
) -> *mut u8 {
    unsafe {
        __molt_gpu_alloc_list_owned(
            _py.attached_context(),
            values.as_ptr(),
            values.len(),
            capacity,
        )
    }
}

#[inline]
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) unsafe fn bound_kernel_arguments(
    py: &PyToken,
    callable: u64,
    builder: u64,
) -> Result<seq_access::PinnedSequenceSnapshot<'_>, u64> {
    let bound = unsafe {
        OwnedRuntimeValue::from_owned_bits(
            py,
            __molt_gpu_bind_kernel_arguments(py.attached_context(), callable, builder),
        )
    };
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    let Some(tuple) = obj_from_bits(bound.bits()).as_ptr() else {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::SystemError,
            "kernel binding did not return a tuple",
        ));
    };
    // The shared snapshot owns each bound value after this temporary tuple
    // retires, including defaults and the canonical *args/**kwargs slots.
    unsafe { seq_access::pin_tuple(py, tuple) }.ok_or_else(|| MoltObject::none().bits())
}

#[inline]
pub(super) unsafe fn clone_callargs_builder_bits(
    _py: &PyToken,
    builder_bits: u64,
) -> Result<u64, u64> {
    let mut out = MoltObject::none().bits();
    if unsafe { __molt_gpu_clone_callargs_builder(_py.attached_context(), builder_bits, &mut out) }
        == 0
    {
        Err(out)
    } else {
        Ok(out)
    }
}

#[inline]
pub(super) fn molt_getattr_builtin(
    _py: &PyToken,
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    unsafe { ffi::molt_getattr_builtin(_py.attached_context(), obj_bits, name_bits, default_bits) }
}

#[inline]
pub(super) fn missing_bits(_py: &PyToken) -> u64 {
    unsafe { __molt_gpu_missing_bits(_py.attached_context()) }
}

#[inline]
pub(super) fn molt_call_bind(_py: &PyToken, call_bits: u64, builder_bits: u64) -> u64 {
    unsafe { ffi::molt_call_bind(_py.attached_context(), call_bits, builder_bits) }
}

#[inline]
pub(super) fn molt_module_cache_get(_py: &PyToken, name_bits: u64) -> u64 {
    unsafe { ffi::molt_module_cache_get(_py.attached_context(), name_bits) }
}

#[inline]
pub(super) fn molt_module_import(_py: &PyToken, name_bits: u64) -> u64 {
    unsafe { ffi::molt_module_import(_py.attached_context(), name_bits) }
}

#[inline]
pub(super) fn molt_isinstance(_py: &PyToken, value_bits: u64, class_bits: u64) -> u64 {
    unsafe { ffi::molt_isinstance(_py.attached_context(), value_bits, class_bits) }
}

#[inline]
pub(super) unsafe fn call_callable1(_py: &PyToken, call_bits: u64, arg_bits: u64) -> u64 {
    unsafe { __molt_gpu_call_callable1(_py.attached_context(), call_bits, arg_bits) }
}

pub(super) mod seq_access {
    use super::*;

    pub(crate) struct PinnedSequenceSnapshot<'py>(OwnedBridgeHandleSnapshot<'py>);

    impl std::ops::Deref for PinnedSequenceSnapshot<'_> {
        type Target = [u64];

        #[inline]
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    pub(crate) struct PinnedItem<'py>(OwnedRuntimeValue<'py>);

    impl PinnedItem<'_> {
        #[inline]
        pub(crate) fn bits(&self) -> u64 {
            self.0.bits()
        }
    }

    #[inline]
    pub(crate) unsafe fn len(_py: &PyToken, ptr: *mut u8) -> usize {
        unsafe { __molt_gpu_seq_len(_py.attached_context(), ptr) }
    }

    pub(crate) unsafe fn snapshot<'py>(
        _py: &'py PyToken,
        ptr: *mut u8,
        message: &str,
    ) -> Option<PinnedSequenceSnapshot<'py>> {
        let mut out_ptr = std::ptr::null();
        let mut out_len = 0;
        let ok = unsafe {
            __molt_gpu_seq_snapshot(
                _py.attached_context(),
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
            bridge_owned_handle_snapshot(_py, out_ptr, out_len)
        }))
    }

    /// Execute one scoped read while the runtime retains sequence custody.
    ///
    /// The slice never crosses the callback boundary: the runtime invokes the
    /// visitor synchronously while its storage guard is live.  Panics are
    /// captured inside the visitor and resumed only after control returns to
    /// Rust, so no unwind can cross the C ABI.  This preserves the zero-copy
    /// hot path without weakening free-threaded synchronization.
    pub(crate) unsafe fn with_borrowed<R, F>(_py: &PyToken, ptr: *mut u8, body: F) -> R
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
                _py.attached_context(),
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

    pub(crate) unsafe fn pin_item(
        _py: &PyToken,
        ptr: *mut u8,
        index: usize,
    ) -> Option<PinnedItem<'_>> {
        let mut bits = 0;
        if unsafe { __molt_gpu_seq_pin_item(_py.attached_context(), ptr, index, &mut bits) } == 0 {
            None
        } else {
            Some(PinnedItem(unsafe {
                OwnedRuntimeValue::from_owned_bits(_py, bits)
            }))
        }
    }

    #[inline]
    pub(crate) unsafe fn pin_tuple(
        py: &PyToken,
        ptr: *mut u8,
    ) -> Option<PinnedSequenceSnapshot<'_>> {
        unsafe { snapshot(py, ptr, "GPU tuple snapshot allocation failed") }
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn kernel_descriptor(_py: &PyToken, callable: u64) -> u64 {
    unsafe { __molt_gpu_kernel_descriptor(_py.attached_context(), callable) }
}
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn descriptor_is_current(_py: &PyToken, callable: u64, descriptor: u64) -> bool {
    unsafe { __molt_gpu_descriptor_is_current(_py.attached_context(), callable, descriptor) != 0 }
}
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn kernel_global(_py: &PyToken, callable: u64, name: &str) -> u64 {
    unsafe { __molt_gpu_kernel_global(_py.attached_context(), callable, name.as_ptr(), name.len()) }
}
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn module_binding(_py: &PyToken, module: u64, name: &str) -> u64 {
    unsafe { __molt_gpu_module_binding(_py.attached_context(), module, name.as_ptr(), name.len()) }
}
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn intrinsic_matches(_py: &PyToken, bits: u64, name: &str) -> bool {
    unsafe {
        __molt_gpu_intrinsic_matches(_py.attached_context(), bits, name.as_ptr(), name.len()) != 0
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn compiled_body_matches(
    _py: &PyToken,
    bits: u64,
    body: &super::RuntimeKernelBody,
) -> bool {
    unsafe {
        __molt_gpu_compiled_body_matches(
            _py.attached_context(),
            bits,
            body.symbol.as_ptr(),
            body.symbol.len(),
            body.arity,
            body.defaults.as_ptr(),
            body.defaults.len(),
        ) != 0
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn class_binding(_py: &PyToken, bits: u64, name: &str) -> u64 {
    unsafe { __molt_gpu_class_binding(_py.attached_context(), bits, name.as_ptr(), name.len()) }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn default_field(_py: &PyToken, bits: u64, name: &str) -> u64 {
    unsafe { __molt_gpu_default_field(_py.attached_context(), bits, name.as_ptr(), name.len()) }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn function_binding(_py: &PyToken, bits: u64, name: &str) -> u64 {
    unsafe { __molt_gpu_function_binding(_py.attached_context(), bits, name.as_ptr(), name.len()) }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
pub(super) fn builtin_matches(_py: &PyToken, bits: u64, name: &str) -> bool {
    unsafe {
        __molt_gpu_builtin_matches(_py.attached_context(), bits, name.as_ptr(), name.len()) != 0
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn bytearray_copy(_py: &PyToken, bits: u64, bytes: &[u8], same_size: bool) -> bool {
    unsafe {
        __molt_gpu_bytearray_copy(
            _py.attached_context(),
            bits,
            bytes.as_ptr(),
            bytes.len(),
            i32::from(same_size),
        ) != 0
    }
}
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[inline]
pub(super) fn commit_buffer_data(_py: &PyToken, object: u64, old: u64, new: u64) -> bool {
    unsafe { __molt_gpu_commit_buffer_data(_py.attached_context(), object, old, new) != 0 }
}
