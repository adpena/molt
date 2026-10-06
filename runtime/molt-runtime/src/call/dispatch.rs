use crate::call::ExceptionBaselineGuard;
use crate::call::type_policy::{InitArgPolicy, resolved_constructor_init_policy};
use crate::call::{StaticmethodCallTarget, require_call_attr, resolve_staticmethod_call_target};
use crate::{
    MoltObject, PtrDropGuard, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_FUNCTION,
    TYPE_ID_GENERIC_ALIAS, TYPE_ID_TYPE, call_builtin_type_if_needed, call_function_obj_vec,
    class_attr_lookup_raw_mro, class_name_for_error, dec_ref_bits, exception_pending,
    generic_alias_origin_bits, intern_static_name, molt_call_bind, obj_from_bits, object_type_id,
    ptr_from_bits, raise_exception, raise_not_callable, runtime_state,
};

#[inline]
unsafe fn with_owned_callable<T>(
    _py: &PyToken<'_>,
    callable_bits: u64,
    invoke: impl FnOnce(u64) -> T,
) -> T {
    let result = invoke(callable_bits);
    dec_ref_bits(_py, callable_bits);
    result
}

unsafe fn call_type_via_bind(_py: &PyToken<'_>, call_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        if !args.is_empty() {
            let call_obj = obj_from_bits(call_bits);
            let Some(call_ptr) = call_obj.as_ptr() else {
                return raise_not_callable(_py, call_obj);
            };
            if object_type_id(call_ptr) == TYPE_ID_TYPE {
                let new_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
                let new_bits = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
                let init_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
                let init_bits = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits);
                if matches!(
                    resolved_constructor_init_policy(new_bits, init_bits),
                    InitArgPolicy::RejectConstructorArgs
                ) {
                    let class_name = class_name_for_error(call_bits);
                    let msg = format!("{class_name}() takes no arguments");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        crate::call::bind::call_bind_borrowed(_py, call_bits, None, args, &[], &[])
    }
}

#[inline]
unsafe fn call_staticmethod_if_needed(
    py: &PyToken<'_>,
    call_bits: u64,
    invoke: impl FnOnce(u64) -> u64,
) -> Option<u64> {
    match unsafe { resolve_staticmethod_call_target(py, call_bits) } {
        StaticmethodCallTarget::NotStaticmethod => None,
        StaticmethodCallTarget::Owned(target) => Some(invoke(target.bits())),
        StaticmethodCallTarget::Raised => Some(MoltObject::none().bits()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_call_builtin(name_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            // The call consumes the builder on every path. Until binding takes
            // it, a failure releases it as the value stack of the failed call.
            let mut builder_owner = PtrDropGuard::new(ptr_from_bits(builder_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "builtin name must be str");
            };
            let name = {
                if object_type_id(name_ptr) != crate::TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "builtin name must be str");
                }
                let len = crate::string_len(name_ptr);
                let bytes = std::slice::from_raw_parts(crate::string_bytes(name_ptr), len);
                std::str::from_utf8(bytes).unwrap_or("")
            };

            if let Some(func_bits) = crate::builtins::functions::lookup_builtin_name(_py, name) {
                builder_owner.release();
                return bind_owned_callable(_py, func_bits, builder_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            // CallBuiltin names the active public namespace. A miss cannot
            // acquire an intrinsic or retry against a different module cache.
            raise_exception::<_>(_py, "NameError", &format!("name '{name}' is not defined"))
        }
    })
}

unsafe fn bind_owned_callable(_py: &PyToken<'_>, callable_bits: u64, builder_bits: u64) -> u64 {
    let result = molt_call_bind(callable_bits, builder_bits);
    dec_ref_bits(_py, callable_bits);
    result
}

unsafe fn call_generic_alias_via_bind(_py: &PyToken<'_>, alias_ptr: *mut u8, args: &[u64]) -> u64 {
    unsafe {
        let origin_bits = generic_alias_origin_bits(alias_ptr);
        call_type_via_bind(_py, origin_bits, args)
    }
}

pub(crate) unsafe fn call_callable0(_py: &PyToken<'_>, call_bits: u64) -> u64 {
    unsafe {
        let _baseline_guard = ExceptionBaselineGuard::new();
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_staticmethod_if_needed(_py, call_bits, |target_bits| {
            call_callable0(_py, target_bits)
        }) {
            return bits;
        }
        if let Some(bits) = call_builtin_type_if_needed(_py, call_bits, call_ptr, &[]) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => call_function_obj_vec(_py, call_bits, &[]),
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[]),
            TYPE_ID_GENERIC_ALIAS => call_generic_alias_via_bind(_py, call_ptr, &[]),
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                with_owned_callable(_py, call_attr_bits, |bits| call_callable0(_py, bits))
            }
        }
    }
}

pub(crate) unsafe fn call_callable1(_py: &PyToken<'_>, call_bits: u64, arg0_bits: u64) -> u64 {
    unsafe {
        let _baseline_guard = ExceptionBaselineGuard::new();
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_staticmethod_if_needed(_py, call_bits, |target_bits| {
            call_callable1(_py, target_bits, arg0_bits)
        }) {
            return bits;
        }
        if let Some(bits) = call_builtin_type_if_needed(_py, call_bits, call_ptr, &[arg0_bits]) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => call_function_obj_vec(_py, call_bits, &[arg0_bits]),
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            TYPE_ID_GENERIC_ALIAS => call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits]),
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                with_owned_callable(_py, call_attr_bits, |bits| {
                    call_callable1(_py, bits, arg0_bits)
                })
            }
        }
    }
}

pub(crate) unsafe fn call_callable2(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    unsafe {
        let _baseline_guard = ExceptionBaselineGuard::new();
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_staticmethod_if_needed(_py, call_bits, |target_bits| {
            call_callable2(_py, target_bits, arg0_bits, arg1_bits)
        }) {
            return bits;
        }
        if let Some(bits) =
            call_builtin_type_if_needed(_py, call_bits, call_ptr, &[arg0_bits, arg1_bits])
        {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => call_function_obj_vec(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_GENERIC_ALIAS => {
                call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits, arg1_bits])
            }
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                with_owned_callable(_py, call_attr_bits, |bits| {
                    call_callable2(_py, bits, arg0_bits, arg1_bits)
                })
            }
        }
    }
}

pub(crate) unsafe fn call_callable3(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    unsafe {
        let _baseline_guard = ExceptionBaselineGuard::new();
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_staticmethod_if_needed(_py, call_bits, |target_bits| {
            call_callable3(_py, target_bits, arg0_bits, arg1_bits, arg2_bits)
        }) {
            return bits;
        }
        if let Some(bits) = call_builtin_type_if_needed(
            _py,
            call_bits,
            call_ptr,
            &[arg0_bits, arg1_bits, arg2_bits],
        ) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                call_function_obj_vec(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_BOUND_METHOD => {
                call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits]),
            crate::TYPE_ID_FOREIGN => {
                call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_GENERIC_ALIAS => {
                call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits, arg1_bits, arg2_bits])
            }
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                with_owned_callable(_py, call_attr_bits, |bits| {
                    call_callable3(_py, bits, arg0_bits, arg1_bits, arg2_bits)
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{molt_callargs_new, molt_callargs_push_pos};

    extern "C" fn len_trampoline(_closure: u64, argv: u64, argc: u64) -> i64 {
        assert_eq!(argc, 1);
        crate::molt_len(unsafe { *(argv as *const u64) }) as i64
    }
    fn object_ref_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits)
            .as_ptr()
            .expect("cached builtin must be an object");
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn call_builtin_uses_published_namespace_and_releases_its_callable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _provider = crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
            let name = crate::attr_name_bits_from_bytes(py, b"len").unwrap();
            let callable = crate::builtins::functions::lookup_builtin_name(py, "len").unwrap();
            let baseline = object_ref_count(callable);
            let argument = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let builder = molt_callargs_new(1, 0);
            unsafe { molt_callargs_push_pos(builder, argument) };
            let result = molt_call_builtin(name, builder);
            assert!(!exception_pending(py));
            assert_eq!(crate::to_i64(obj_from_bits(result)), Some(0));
            assert_eq!(object_ref_count(callable), baseline);
            for bits in [name, callable, argument, result] {
                crate::dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn call_builtin_consumes_its_builder_when_resolution_fails() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let unknown_ptr = crate::alloc_string(_py, b"__molt_unresolved_builtin_probe__");
            assert!(!unknown_ptr.is_null());
            let unknown_bits = MoltObject::from_ptr(unknown_ptr).bits();
            let arg_ptr = crate::alloc_string(_py, b"call_builtin argument");
            assert!(!arg_ptr.is_null());
            let arg_bits = MoltObject::from_ptr(arg_ptr).bits();
            let baseline = object_ref_count(arg_bits);
            // An unresolvable name and a non-string name both fail before any
            // callable exists; the consumed builder must still release its
            // argument rather than leak it with the builder.
            for name_bits in [unknown_bits, MoltObject::from_int(7).bits()] {
                let builder_bits = molt_callargs_new(1, 0);
                assert!(!obj_from_bits(builder_bits).is_none());
                let pushed = unsafe { molt_callargs_push_pos(builder_bits, arg_bits) };
                assert!(obj_from_bits(pushed).is_none());
                assert_eq!(object_ref_count(arg_bits), baseline + 1);

                let result_bits = molt_call_builtin(name_bits, builder_bits);
                assert!(obj_from_bits(result_bits).is_none());
                assert!(
                    exception_pending(_py),
                    "a failed builtin resolution must raise"
                );
                let _ = crate::molt_exception_clear();
                assert_eq!(
                    object_ref_count(arg_bits),
                    baseline,
                    "molt_call_builtin must consume its builder when resolution fails"
                );
            }
            crate::dec_ref_bits(_py, arg_bits);
            crate::dec_ref_bits(_py, unknown_bits);
        });
    }

    #[test]
    fn builtin_call_consumers_respect_captured_names_and_explicit_runtime_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _provider = crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
            let canonical = crate::builtins::functions::lookup_builtin_name(py, "len").unwrap();
            let captured_ptr = crate::alloc_dict_with_pairs(py, &[]);
            let captured = MoltObject::from_ptr(captured_ptr).bits();
            let globals = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
            let argument = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let public_name = crate::attr_name_bits_from_bytes(py, b"len").unwrap();
            let custom_name = crate::attr_name_bits_from_bytes(py, b"custom_builtin").unwrap();
            let runtime_name = crate::attr_name_bits_from_bytes(py, b"molt_len").unwrap();
            unsafe {
                crate::dict_set_in_place(py, captured_ptr, public_name, canonical);
                crate::dict_set_in_place(py, captured_ptr, custom_name, canonical);
                crate::dict_set_in_place(
                    py,
                    captured_ptr,
                    runtime_name,
                    MoltObject::from_int(37).bits(),
                );
            }
            crate::inc_ref_bits(py, globals);
            crate::inc_ref_bits(py, captured);
            crate::builtins::frames::frame_stack_push_owned(py, 0, globals, captured, 0);
            for name in [public_name, custom_name] {
                let builder = molt_callargs_new(1, 0);
                unsafe {
                    molt_callargs_push_pos(builder, argument);
                }
                let value = molt_call_builtin(name, builder);
                assert!(!exception_pending(py));
                assert_eq!(crate::to_i64(obj_from_bits(value)), Some(0));
                crate::dec_ref_bits(py, value);
            }
            unsafe {
                crate::dict_del_in_place(py, captured_ptr, public_name);
            }
            let baseline = object_ref_count(argument);
            for name in [public_name, runtime_name] {
                // len is absent; molt_len is a present non-callable. Neither
                // case may select the builtin/intrinsic behind this namespace.
                let builder = molt_callargs_new(1, 0);
                unsafe {
                    molt_callargs_push_pos(builder, argument);
                }
                let result = molt_call_builtin(name, builder);
                assert!(exception_pending(py));
                let error = crate::builtins::exceptions::exception_last_bits_noinc(py).unwrap();
                let expected = if name == public_name {
                    "NameError"
                } else {
                    "TypeError"
                };
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py, error, expected
                ));
                crate::clear_exception(py);
                crate::dec_ref_bits(py, result);
                assert_eq!(object_ref_count(argument), baseline);
            }
            let missing =
                crate::molt_func_new_builtin_named(public_name, fn_addr!(crate::molt_len), 0, 1);
            assert!(exception_pending(py));
            let error = crate::builtins::exceptions::exception_last_bits_noinc(py).unwrap();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "NameError"
            ));
            crate::clear_exception(py);
            crate::dec_ref_bits(py, missing);
            // The constructor's canonical runtime symbol is declarative and
            // must not read the same-spelled active public binding (37).
            let intrinsic = crate::molt_func_new_builtin_named(
                runtime_name,
                fn_addr!(crate::molt_len),
                fn_addr!(len_trampoline),
                1,
            );
            assert!(!exception_pending(py));
            assert_ne!(intrinsic, MoltObject::from_int(37).bits());
            let result = unsafe { call_callable1(py, intrinsic, argument) };
            assert_eq!(crate::to_i64(obj_from_bits(result)), Some(0));
            assert!(!exception_pending(py));
            crate::builtins::frames::frame_stack_pop(py);
            for bits in [
                canonical,
                captured,
                globals,
                argument,
                public_name,
                custom_name,
                runtime_name,
                intrinsic,
                result,
            ] {
                crate::dec_ref_bits(py, bits);
            }
        });
    }
}
