use crate::PyToken;
use crate::*;
use std::sync::atomic::AtomicU64;

pub(super) fn runtime_python_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    let state = runtime_state(_py);
    let guard = state.sys_version_info.lock().unwrap();
    let (runtime_major, runtime_minor) = guard
        .as_ref()
        .map(|info| (info.major, info.minor))
        .unwrap_or((3, 12));
    runtime_major > major || (runtime_major == major && runtime_minor >= minor)
}

/// Create and cache a builtin function object with no optional args.
pub(crate) fn builtin_func_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            // Cached builtin callables are runtime singletons; treat them as immortal so
            // refcount churn in compiled code cannot free them out from under the caches.
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(ptr);
            if old_bits != builtin_bits
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_IMMORTAL);
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Create and cache a builtin whose Rust ABI is not directly positional-callable.
///
/// These functions still have a fixed runtime trampoline arity, but their public
/// Python signature requires the binder to collect or normalize arguments first
/// (for example set/frozenset multi-operand methods that receive
/// `(self, others_tuple)`). Marking the function with a bind kind lets call ICs
/// keep caching the resolved method while routing every hit through the binder.
pub(crate) fn builtin_func_bits_with_bind_kind(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    bind_kind: i64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
            let bind_kind_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_bind_kind,
                b"__molt_bind_kind__",
            );
            function_set_attr_bits(
                _py,
                ptr,
                bind_kind_name,
                MoltObject::from_int(bind_kind).bits(),
            );
            crate::call::bind::refresh_function_requires_binder_flag(_py, ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(ptr);
            if old_bits != builtin_bits
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_IMMORTAL);
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Create a builtin function with a `__defaults__` tuple for optional args.
/// This is the CPython-parity approach: the defaults tuple holds the last N
/// parameter defaults (right-aligned). When called with fewer args, the
/// bind path reads missing values from the end of the tuple.
pub(crate) fn builtin_func_bits_with_defaults_tuple(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
            // Set __defaults__ tuple as a function attribute.
            let defaults_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                b"__defaults__",
            );
            let defaults_ptr = alloc_tuple(_py, defaults);
            if !defaults_ptr.is_null() {
                let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
                function_set_attr_bits(_py, ptr, defaults_name, defaults_bits);
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(ptr);
            if old_bits != builtin_bits
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_IMMORTAL);
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub(crate) fn builtin_classmethod_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    func_ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
                return MoltObject::none().bits();
            }
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let cm_ptr = alloc_classmethod_obj(_py, func_bits);
        if cm_ptr.is_null() {
            dec_ref_bits(_py, func_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, func_bits);
        MoltObject::from_ptr(cm_ptr).bits()
    })
}

/// Create a classmethod with a `__defaults__` tuple for optional args.
pub(crate) fn builtin_classmethod_bits_with_defaults_tuple(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            (*header_from_obj_ptr(func_ptr)).fetch_or_flags(crate::object::HEADER_FLAG_IMMORTAL);
            let defaults_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                b"__defaults__",
            );
            let defaults_ptr = alloc_tuple(_py, defaults);
            if !defaults_ptr.is_null() {
                let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
                function_set_attr_bits(_py, func_ptr, defaults_name, defaults_bits);
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    func_ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                (*header_from_obj_ptr(func_ptr))
                    .fetch_and_flags(!crate::object::HEADER_FLAG_IMMORTAL);
                dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
                return MoltObject::none().bits();
            }
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let cm_ptr = alloc_classmethod_obj(_py, func_bits);
        if cm_ptr.is_null() {
            dec_ref_bits(_py, func_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, func_bits);
        MoltObject::from_ptr(cm_ptr).bits()
    })
}
