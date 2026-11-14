use crate::BuiltinExc;
use crate::PyToken;
use crate::builtins::functions::native_callable::{NativeCallableSpec, configure_native_callable};
use crate::*;
#[cfg(test)]
use std::sync::atomic::AtomicU64;

enum BuiltinFunctionMetadata<'a> {
    None,
    BindKind(i64),
    Defaults(&'a [u64]),
    Variadic,
    Signature {
        defaults: &'a [u64],
        positional_only: usize,
        arg_names: &'a [&'a [u8]],
        has_vararg: bool,
        has_varkw: bool,
    },
}

unsafe fn set_function_metadata_attr(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    name: crate::NameId,
    value: u64,
) -> bool {
    unsafe {
        let name_bits = name.bits(_py);
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
            return false;
        }
        let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
        let configured =
            set_function_metadata_attr(py, ptr, crate::NameId::DunderDefaults, defaults_bits);
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
        raise_exception::<u64>(
            py,
            BuiltinExc::SystemError,
            "builtin signature requires a function",
        );
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "builtin signature requires a function",
            );
            return false;
        }
        let names_key = crate::NameId::DunderMoltArgNames.bits(py);
        if exception_pending(py) {
            return false;
        }
        let existing = match function_attr_bits(py, ptr, names_key) {
            Ok(value) => value,
            Err(()) => return false,
        };
        if existing.is_none() {
            let mut names = Vec::new();
            if names.try_reserve_exact(arg_names.len()).is_err() {
                crate::record_memory_error_without_allocation(py);
                return false;
            }
            for name in arg_names {
                let name_ptr = alloc_string(py, name);
                if name_ptr.is_null() {
                    for bits in names {
                        dec_ref_bits(py, bits);
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
                return false;
            }
            let names_bits = MoltObject::from_ptr(names_ptr).bits();
            let installed = function_set_attr_bits(py, ptr, names_key, names_bits);
            dec_ref_bits(py, names_bits);
            if !installed {
                return false;
            }
        }
        for (name, enabled) in [
            (crate::NameId::DunderMoltVararg, has_vararg),
            (crate::NameId::DunderMoltVarkw, has_varkw),
        ] {
            let value = if enabled {
                MoltObject::from_bool(true).bits()
            } else {
                MoltObject::none().bits()
            };
            if !set_function_metadata_attr(py, ptr, name, value) {
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
                defaults,
                positional_only,
                arg_names,
                has_vararg,
                has_varkw,
            } => {
                (defaults.is_empty() || set_function_defaults(_py, ptr, defaults))
                    && (positional_only == 0
                        || set_function_metadata_attr(
                            _py,
                            ptr,
                            crate::NameId::DunderMoltPosonly,
                            MoltObject::from_int(positional_only as i64).bits(),
                        ))
                    && configure_builtin_signature(
                        _py,
                        MoltObject::from_ptr(ptr).bits(),
                        arg_names,
                        has_vararg,
                        has_varkw,
                    )
            }
            BuiltinFunctionMetadata::None => true,
            BuiltinFunctionMetadata::BindKind(bind_kind) => {
                if !set_function_metadata_attr(
                    _py,
                    ptr,
                    crate::NameId::DunderMoltBindKind,
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
                let configured = set_function_metadata_attr(
                    _py,
                    ptr,
                    crate::NameId::DunderMoltArgNames,
                    arg_names_bits,
                ) && set_function_metadata_attr(
                    _py,
                    ptr,
                    crate::NameId::DunderMoltVararg,
                    vararg_bits,
                ) && set_function_metadata_attr(
                    _py,
                    ptr,
                    crate::NameId::DunderMoltVarkw,
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
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
    metadata: BuiltinFunctionMetadata<'_>,
) -> u64 {
    if exception_pending(py) {
        return 0;
    }
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, fn_ptr, arity);
    if ptr.is_null() {
        return 0;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    unsafe {
        let initialized = configure_builtin_function(py, ptr, metadata)
            && !exception_pending(py)
            && configure_native_callable(py, ptr, spec);
        if !initialized {
            dec_ref_bits(py, bits);
            return 0;
        }
    }
    bits
}

/// Allocate one owned builtin callable without creating a cache slot.
pub(crate) fn alloc_builtin_function(py: &PyToken<'_>, fn_ptr: u64, arity: u64) -> u64 {
    alloc_builtin_function_with_metadata(
        py,
        NativeCallableSpec::uncached_function(),
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::None,
    )
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
        NativeCallableSpec::uncached_function(),
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Defaults(defaults),
    )
}

fn builtin_func_bits_with_metadata(
    py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
    metadata: BuiltinFunctionMetadata<'_>,
) -> u64 {
    init_native_callable(py, spec, || {
        alloc_builtin_function_with_metadata(py, spec, fn_ptr, arity, metadata)
    })
}

/// Allocate an owned standalone native callable using the ordinary binder.
pub(crate) fn alloc_builtin_function_with_signature(
    py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    signature: crate::intrinsics::public_objects_generated::PublicSignatureSpec,
    defaults: &[u64],
) -> u64 {
    alloc_builtin_function_with_metadata(
        py,
        spec,
        fn_ptr,
        signature.arity,
        BuiltinFunctionMetadata::Signature {
            defaults,
            positional_only: signature.posonly,
            arg_names: signature.positional,
            has_vararg: signature.vararg,
            has_varkw: signature.varkw,
        },
    )
}

pub(crate) fn init_native_callable(
    py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    initialize: impl FnOnce() -> u64,
) -> u64 {
    // Initialization builds unpublished runtime metadata. Its explicit outcome
    // preserves an incoming error on success and selects a real failure. Public
    // namespace lookup and publication below retain ordinary callback visibility.
    let initialize = || {
        let mut result = 0;
        crate::builtins::exceptions::with_saved_raised_exception(py, || {
            result = initialize();
            result != 0
        });
        result
    };
    if let Some(slot) = spec.cache {
        return init_atomic_bits(py, slot, initialize);
    }
    let Some(owner) = spec.owner.and_then(|bits| obj_from_bits(bits).as_ptr()) else {
        raise_exception::<u64>(py, BuiltinExc::SystemError, "cached callable has no owner");
        return 0;
    };
    let Some(name) = spec.name else {
        raise_exception::<u64>(py, BuiltinExc::SystemError, "declared callable has no name");
        return 0;
    };
    let Some(key) = name.owned_bits(py) else {
        return 0;
    };
    unsafe {
        let Some(dict) = obj_from_bits(class_dict_bits(owner)).as_ptr() else {
            dec_ref_bits(py, key);
            raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "declared callable owner has no namespace",
            );
            return 0;
        };
        match dict_get_in_place(py, dict, key) {
            Ok(Some(existing)) => {
                dec_ref_bits(py, key);
                return existing;
            }
            Err(()) => {
                dec_ref_bits(py, key);
                return 0;
            }
            Ok(None) => {}
        }
        if crate::object::class_storage::class_declares(
            owner,
            crate::object::class_storage::ClassDeclaration::NativeNamespacePublished,
        ) {
            dec_ref_bits(py, key);
            return MoltObject::none().bits();
        }
        let function = initialize();
        if function == 0 {
            dec_ref_bits(py, key);
            return 0;
        }
        let installed = dict_set_in_place(py, dict, key, function);
        dec_ref_bits(py, key);
        dec_ref_bits(py, function);
        if !installed {
            return 0;
        }
        class_bump_layout_version(owner);
        function
    }
}

pub(super) fn runtime_python_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    crate::object::ops_sys::runtime_target_at_least(_py, major, minor)
}

/// Create and cache a builtin function object with no optional args.
pub(crate) fn builtin_func_bits(
    _py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    builtin_func_bits_with_metadata(_py, spec, fn_ptr, arity, BuiltinFunctionMetadata::None)
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
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
    bind_kind: i64,
) -> u64 {
    builtin_func_bits_with_metadata(
        _py,
        spec,
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
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    builtin_func_bits_with_metadata(
        _py,
        spec,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Defaults(defaults),
    )
}

/// Create and cache a builtin whose Python signature is `(*args, **kwargs)`.
/// The runtime trampoline receives the binder's `(args_tuple, kwargs_dict)` ABI.
pub(crate) fn builtin_variadic_func_bits(
    _py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
) -> u64 {
    builtin_func_bits_with_metadata(_py, spec, fn_ptr, 2, BuiltinFunctionMetadata::Variadic)
}

/// Cache a callable only after its complete positional/variadic signature is ready.
#[allow(clippy::too_many_arguments)]
pub(crate) fn builtin_func_bits_with_signature(
    py: &PyToken<'_>,
    spec: NativeCallableSpec<'_>,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
    positional_only: usize,
    arg_names: &[&[u8]],
    has_vararg: bool,
    has_varkw: bool,
) -> u64 {
    builtin_func_bits_with_metadata(
        py,
        spec,
        fn_ptr,
        arity,
        BuiltinFunctionMetadata::Signature {
            defaults,
            positional_only,
            arg_names,
            has_vararg,
            has_varkw,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    extern "C" fn identity(__context: u64, value: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        {
            let py = __py;
            inc_ref_bits(py, value);
            value
        }
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
            let function = builtin_func_bits(py, NativeCallableSpec::function(&slot), address, 1);
            assert!(!exception_pending(py));
            let ptr = obj_from_bits(function).as_ptr().unwrap();
            let header = unsafe { &*header_from_obj_ptr(ptr) };
            assert_eq!(
                header.load_metadata_flags() & crate::object::HEADER_FLAG_IMMORTAL,
                0
            );
            assert_eq!(header.ref_count_snapshot(), 1);
            assert_eq!(
                builtin_func_bits(py, NativeCallableSpec::function(&slot), address, 1),
                function
            );
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
            let wrapper = builtin_func_bits_with_defaults_tuple(
                py,
                NativeCallableSpec::function(&slot),
                address,
                1,
                &[default],
            );
            assert!(!exception_pending(py));
            assert!(obj_from_bits(wrapper).as_ptr().is_some());
            assert_eq!(header.ref_count_snapshot(), initial + 1);
            crate::state::cache::clear_atomic_bits(py, &slot);
            assert_eq!(header.ref_count_snapshot(), initial);
            dec_ref_bits(py, default);
        });
    }

    #[test]
    fn callable_failure_retries_and_startup_names_need_no_allocation() {
        use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let _ = builtin_classes(py);
                let function_slot = AtomicU64::new(0);
                let name = crate::NameId::DunderDefaults.bits(py);
                assert_ne!(name, 0);
                let address = crate::builtins::functions::runtime_fn_addr(
                    "descriptor_cache_identity",
                    identity as *const (),
                );
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    ..Default::default()
                })));
                let reset = crate::test_support::restore_unlimited_tracker_on_drop();
                assert_eq!(
                    builtin_func_bits(py, NativeCallableSpec::function(&function_slot), address, 1),
                    0
                );
                assert!(exception_pending(py));
                assert_eq!(function_slot.load(Ordering::Acquire), 0);
                let _ = molt_exception_clear_attached_body(py);
                assert_eq!(crate::NameId::DunderDefaults.bits(py), name);
                assert!(!exception_pending(py));
                drop(reset);
                let _ = molt_exception_clear_attached_body(py);
                assert_ne!(
                    builtin_func_bits(py, NativeCallableSpec::function(&function_slot), address, 1),
                    0
                );
                assert!(obj_from_bits(name).as_ptr().is_some());
                assert!(!exception_pending(py));
                crate::state::cache::clear_atomic_bits(py, &function_slot);
            });
        });
    }
}
