use crate::PyToken;
use crate::*;
use std::sync::atomic::AtomicU64;

enum BuiltinFunctionMetadata<'a> {
    None,
    BindKind(i64),
    Defaults(&'a [u64]),
    Variadic,
    Signature {
        arg_names: &'a [&'a [u8]],
        has_vararg: bool,
        has_varkw: bool,
    },
}

unsafe fn set_function_metadata_attr(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    slot: &AtomicU64,
    name: &'static [u8],
    value: u64,
) -> bool {
    unsafe {
        let name_bits = intern_static_name(_py, slot, name);
        if exception_pending(_py) {
            return false;
        }
        function_set_attr_bits(_py, ptr, name_bits, value)
    }
}

/// Attach borrowed defaults through the same fallible metadata authority used
/// by ordinary and bootstrap callables. The temporary tuple is always released.
#[must_use]
pub(crate) unsafe fn set_function_defaults(
    py: &PyToken<'_>,
    ptr: *mut u8,
    defaults: &[u64],
) -> bool {
    unsafe {
        if exception_pending(py) {
            return false;
        }
        let defaults_ptr = alloc_tuple(py, defaults);
        if defaults_ptr.is_null() {
            if !exception_pending(py) {
                raise_exception::<u64>(py, "MemoryError", "function defaults allocation failed");
            }
            return false;
        }
        let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
        let configured = set_function_metadata_attr(
            py,
            ptr,
            &runtime_state(py).interned.defaults_name,
            b"__defaults__",
            defaults_bits,
        );
        dec_ref_bits(py, defaults_bits);
        configured
    }
}

/// Configure the binder metadata of an internal runtime callable. Existing
/// positional metadata remains authoritative; repeated setup preserves its
/// tuple and dictionary identities. This runs before callable publication.
#[must_use]
pub(crate) fn configure_builtin_signature(
    py: &PyToken<'_>,
    function: u64,
    arg_names: &[&[u8]],
    has_vararg: bool,
    has_varkw: bool,
) -> bool {
    if exception_pending(py) {
        return false;
    }
    let Some(ptr) = obj_from_bits(function).as_ptr() else {
        raise_exception::<u64>(py, "SystemError", "builtin signature requires a function");
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            raise_exception::<u64>(py, "SystemError", "builtin signature requires a function");
            return false;
        }
        let interned = &runtime_state(py).interned;
        let names_key = intern_static_name(py, &interned.molt_arg_names, b"__molt_arg_names__");
        if exception_pending(py) {
            return false;
        }
        let existing = function_attr_bits(py, ptr, names_key);
        if exception_pending(py) {
            return false;
        }
        if existing.is_none() {
            let mut names = Vec::new();
            if names.try_reserve_exact(arg_names.len()).is_err() {
                raise_exception::<u64>(
                    py,
                    "MemoryError",
                    "builtin argument names allocation failed",
                );
                return false;
            }
            for name in arg_names {
                let name_ptr = alloc_string(py, name);
                if name_ptr.is_null() {
                    for bits in names {
                        dec_ref_bits(py, bits);
                    }
                    if !exception_pending(py) {
                        raise_exception::<u64>(
                            py,
                            "MemoryError",
                            "builtin argument name allocation failed",
                        );
                    }
                    return false;
                }
                names.push(MoltObject::from_ptr(name_ptr).bits());
            }
            let names_ptr = alloc_tuple(py, &names);
            for bits in names {
                dec_ref_bits(py, bits);
            }
            if names_ptr.is_null() {
                if !exception_pending(py) {
                    raise_exception::<u64>(
                        py,
                        "MemoryError",
                        "builtin argument tuple allocation failed",
                    );
                }
                return false;
            }
            let names_bits = MoltObject::from_ptr(names_ptr).bits();
            let installed = function_set_attr_bits(py, ptr, names_key, names_bits);
            dec_ref_bits(py, names_bits);
            if !installed {
                return false;
            }
        }
        for (slot, name, enabled) in [
            (
                &interned.molt_vararg,
                b"__molt_vararg__".as_slice(),
                has_vararg,
            ),
            (
                &interned.molt_varkw,
                b"__molt_varkw__".as_slice(),
                has_varkw,
            ),
        ] {
            let value = if enabled {
                MoltObject::from_bool(true).bits()
            } else {
                MoltObject::none().bits()
            };
            if !set_function_metadata_attr(py, ptr, slot, name, value) {
                return false;
            }
        }
        !exception_pending(py)
    }
}

unsafe fn configure_builtin_function(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    metadata: BuiltinFunctionMetadata<'_>,
) -> bool {
    unsafe {
        match metadata {
            BuiltinFunctionMetadata::Signature {
                arg_names,
                has_vararg,
                has_varkw,
            } => configure_builtin_signature(
                _py,
                MoltObject::from_ptr(ptr).bits(),
                arg_names,
                has_vararg,
                has_varkw,
            ),
            BuiltinFunctionMetadata::None => true,
            BuiltinFunctionMetadata::BindKind(bind_kind) => {
                if !set_function_metadata_attr(
                    _py,
                    ptr,
                    &runtime_state(_py).interned.molt_bind_kind,
                    b"__molt_bind_kind__",
                    MoltObject::from_int(bind_kind).bits(),
                ) {
                    return false;
                }
                !exception_pending(_py)
            }
            BuiltinFunctionMetadata::Defaults(defaults) => {
                set_function_defaults(_py, ptr, defaults)
            }
            BuiltinFunctionMetadata::Variadic => {
                let arg_names_ptr = alloc_tuple(_py, &[]);
                if arg_names_ptr.is_null() {
                    return false;
                }
                let arg_names_bits = MoltObject::from_ptr(arg_names_ptr).bits();
                let vararg_ptr = alloc_string(_py, b"args");
                if vararg_ptr.is_null() {
                    dec_ref_bits(_py, arg_names_bits);
                    return false;
                }
                let vararg_bits = MoltObject::from_ptr(vararg_ptr).bits();
                let varkw_ptr = alloc_string(_py, b"kwargs");
                if varkw_ptr.is_null() {
                    dec_ref_bits(_py, vararg_bits);
                    dec_ref_bits(_py, arg_names_bits);
                    return false;
                }
                let varkw_bits = MoltObject::from_ptr(varkw_ptr).bits();
                let interned = &runtime_state(_py).interned;
                let configured = set_function_metadata_attr(
                    _py,
                    ptr,
                    &interned.molt_arg_names,
                    b"__molt_arg_names__",
                    arg_names_bits,
                ) && set_function_metadata_attr(
                    _py,
                    ptr,
                    &interned.molt_vararg,
                    b"__molt_vararg__",
                    vararg_bits,
                ) && set_function_metadata_attr(
                    _py,
                    ptr,
                    &interned.molt_varkw,
                    b"__molt_varkw__",
                    varkw_bits,
                );
                dec_ref_bits(_py, varkw_bits);
                dec_ref_bits(_py, vararg_bits);
                dec_ref_bits(_py, arg_names_bits);
                if !configured {
                    return false;
                }
                !exception_pending(_py)
            }
        }
    }
}

/// Return one owned callable, or zero with an exception. The cache or wrapper
/// takes this reference; borrowed cache reads never need an immortal payload.
fn alloc_builtin_function_with_metadata(
    py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u64,
    metadata: BuiltinFunctionMetadata<'_>,
) -> u64 {
    if exception_pending(py) {
        return 0;
    }
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, fn_ptr, arity);
    if ptr.is_null() {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "MemoryError", "builtin callable allocation failed");
        }
        return 0;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    unsafe {
        let initialized =
            configure_builtin_function(py, ptr, metadata) && !exception_pending(py) && {
                let class = builtin_classes(py).builtin_function_or_method;
                !exception_pending(py)
                    && (object_class_bits(ptr) == class
                        || crate::object::object_init_class_edge_unpublished(
                            py,
                            ptr,
                            class,
                            ClassEdgeOwnership::Owned,
                        ))
            };
        if !initialized {
            dec_ref_bits(py, bits);
            if !exception_pending(py) {
                raise_exception::<u64>(py, "MemoryError", "builtin callable initialization failed");
            }
            return 0;
        }
    }
    bits
}

/// Allocate one owned builtin callable without creating a cache slot.
pub(crate) fn alloc_builtin_function(py: &PyToken<'_>, fn_ptr: u64, arity: u64) -> u64 {
    alloc_builtin_function_with_metadata(py, fn_ptr, arity, BuiltinFunctionMetadata::None)
}

/// Allocate one owned builtin callable with retained positional defaults.
pub(crate) fn alloc_builtin_function_with_defaults(
    py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    alloc_builtin_function_with_metadata(
        py,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Defaults(defaults),
    )
}

fn builtin_func_bits_with_metadata(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    metadata: BuiltinFunctionMetadata<'_>,
) -> u64 {
    init_atomic_bits(py, slot, || {
        alloc_builtin_function_with_metadata(py, fn_ptr, arity, metadata)
    })
}

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
    builtin_func_bits_with_metadata(_py, slot, fn_ptr, arity, BuiltinFunctionMetadata::None)
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
    builtin_func_bits_with_metadata(
        _py,
        slot,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::BindKind(bind_kind),
    )
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
    builtin_func_bits_with_metadata(
        _py,
        slot,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Defaults(defaults),
    )
}

/// Create and cache a builtin whose Python signature is `(*args, **kwargs)`.
/// The runtime trampoline receives the binder's `(args_tuple, kwargs_dict)` ABI.
pub(crate) fn builtin_variadic_func_bits(_py: &PyToken<'_>, slot: &AtomicU64, fn_ptr: u64) -> u64 {
    builtin_func_bits_with_metadata(_py, slot, fn_ptr, 2, BuiltinFunctionMetadata::Variadic)
}

/// Cache a callable only after its complete positional/variadic signature is ready.
#[allow(clippy::too_many_arguments)]
pub(crate) fn builtin_func_bits_with_signature(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    arg_names: &[&[u8]],
    has_vararg: bool,
    has_varkw: bool,
) -> u64 {
    builtin_func_bits_with_metadata(
        py,
        slot,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Signature {
            arg_names,
            has_vararg,
            has_varkw,
        },
    )
}

fn builtin_classmethod_bits_with_metadata(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    metadata: BuiltinFunctionMetadata<'_>,
) -> u64 {
    init_atomic_bits(py, slot, || {
        let function = alloc_builtin_function_with_metadata(py, fn_ptr, arity, metadata);
        if function == 0 {
            return 0;
        }
        let wrapper = alloc_classmethod_obj(py, function);
        dec_ref_bits(py, function);
        if wrapper.is_null() {
            if !exception_pending(py) {
                raise_exception::<u64>(py, "MemoryError", "builtin classmethod allocation failed");
            }
            0
        } else {
            MoltObject::from_ptr(wrapper).bits()
        }
    })
}

pub(crate) fn builtin_classmethod_bits(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    builtin_classmethod_bits_with_metadata(py, slot, fn_ptr, arity, BuiltinFunctionMetadata::None)
}

/// Classmethod defaults use exactly the same retained metadata as plain methods.
pub(crate) fn builtin_classmethod_bits_with_defaults_tuple(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    builtin_classmethod_bits_with_metadata(
        py,
        slot,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Defaults(defaults),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    extern "C" fn identity(value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, value);
            value
        })
    }

    #[test]
    fn descriptor_callable_cache_owns_one_releasable_reference() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let slot = AtomicU64::new(0);
            let address = crate::builtins::functions::runtime_fn_addr(
                "descriptor_cache_identity",
                identity as *const (),
            );
            let function = builtin_func_bits(py, &slot, address, 1);
            assert!(!exception_pending(py));
            let ptr = obj_from_bits(function).as_ptr().unwrap();
            let header = unsafe { &*header_from_obj_ptr(ptr) };
            assert_eq!(
                header.load_metadata_flags() & crate::object::HEADER_FLAG_IMMORTAL,
                0
            );
            assert_eq!(header.ref_count_snapshot(), 1);
            assert_eq!(builtin_func_bits(py, &slot, address, 1), function);
            assert_eq!(header.ref_count_snapshot(), 1);
            inc_ref_bits(py, function);
            crate::state::cache::clear_atomic_bits(py, &slot);
            assert_eq!(slot.load(Ordering::Acquire), 0);
            assert_eq!(header.ref_count_snapshot(), 1);
            let expected = MoltObject::from_int(17).bits();
            let result = unsafe { call_callable1(py, function, expected) };
            assert_eq!(result, expected);
            assert!(!exception_pending(py));
            dec_ref_bits(py, result);
            dec_ref_bits(py, function);
        });
    }

    #[test]
    fn descriptor_classmethod_defaults_release_with_their_wrapper() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let default_ptr = alloc_string(py, b"retained-default");
            assert!(!default_ptr.is_null());
            let default = MoltObject::from_ptr(default_ptr).bits();
            let header = unsafe { &*header_from_obj_ptr(default_ptr) };
            let initial = header.ref_count_snapshot();
            let slot = AtomicU64::new(0);
            let address = crate::builtins::functions::runtime_fn_addr(
                "descriptor_defaults_identity",
                identity as *const (),
            );
            let wrapper =
                builtin_classmethod_bits_with_defaults_tuple(py, &slot, address, 1, &[default]);
            assert!(!exception_pending(py));
            assert!(obj_from_bits(wrapper).as_ptr().is_some());
            assert_eq!(header.ref_count_snapshot(), initial + 1);
            crate::state::cache::clear_atomic_bits(py, &slot);
            assert_eq!(header.ref_count_snapshot(), initial);
            dec_ref_bits(py, default);
        });
    }

    #[test]
    fn callable_and_name_allocation_failure_never_poison_atomic_caches() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _ = builtin_classes(py);
            let function_slot = AtomicU64::new(0);
            let name_slot = AtomicU64::new(0);
            let address = crate::builtins::functions::runtime_fn_addr(
                "descriptor_cache_identity",
                identity as *const (),
            );
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            let reset = RestoreTracker;
            assert_eq!(builtin_func_bits(py, &function_slot, address, 1), 0);
            assert!(exception_pending(py));
            assert_eq!(function_slot.load(Ordering::Acquire), 0);
            let _ = molt_exception_clear();
            assert_eq!(
                intern_static_name(
                    py,
                    &name_slot,
                    b"unallocated static-name failure regression payload"
                ),
                0
            );
            assert!(exception_pending(py));
            assert_eq!(name_slot.load(Ordering::Acquire), 0);
            drop(reset);
            let _ = molt_exception_clear();
            assert_ne!(builtin_func_bits(py, &function_slot, address, 1), 0);
            let name = intern_static_name(
                py,
                &name_slot,
                b"unallocated static-name failure regression payload",
            );
            assert!(obj_from_bits(name).as_ptr().is_some());
            assert!(!exception_pending(py));
            crate::state::cache::clear_atomic_bits(py, &function_slot);
            crate::state::cache::clear_atomic_bits(py, &name_slot);
        });
    }
}
