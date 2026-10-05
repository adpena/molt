use super::*;
use crate::call::bind::inline_cache::method_ic_call_plan;
use crate::call::bind::test_support::*;
use crate::object::builders::{alloc_list, alloc_tuple};

#[test]
fn call_bind_builtin_full_binding_preserves_callee_owned_alias_return() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_identity_returns_owned_arg as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let list_ptr = alloc_list(_py, &[MoltObject::from_int(13).bits()]);
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        let builder_bits = crate::call::bind::arguments::molt_callargs_new(1, 0);
        assert!(!obj_from_bits(builder_bits).is_none());
        let _ = unsafe {
            crate::call::bind::arguments::molt_callargs_push_pos(builder_bits, list_bits)
        };

        dec_ref_bits(_py, list_bits);
        let result_bits = crate::call::bind::molt_call_bind(func_bits, builder_bits);
        assert_eq!(
            result_bits, list_bits,
            "identity callable must return the argument bits unchanged"
        );
        let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live result");
        assert_eq!(result_ptr, list_ptr);
        let rc = unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
        assert_eq!(
            rc, 1,
            "CallArgs teardown must preserve the callee-owned return"
        );

        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn call_bind_builtin_default_padded_argv_preserves_callee_owned_alias_return() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_second_arg_returns_owned_arg as *const (),
            ),
            2,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let default_ptr = alloc_list(_py, &[MoltObject::from_int(17).bits()]);
        assert!(!default_ptr.is_null());
        let default_bits = MoltObject::from_ptr(default_ptr).bits();
        let defaults_ptr = alloc_tuple(_py, &[default_bits]);
        assert!(!defaults_ptr.is_null());
        let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
        let defaults_name = intern_metadata_name(_py, b"__defaults__");
        unsafe {
            assert!(crate::call::class_init::function_set_attr_bits(
                _py,
                func_ptr,
                defaults_name,
                defaults_bits,
            ));
        }
        dec_ref_bits(_py, defaults_bits);
        dec_ref_bits(_py, default_bits);

        let before_call =
            unsafe { (*crate::object::header_from_obj_ptr(default_ptr)).ref_count_snapshot() };
        assert_eq!(
            before_call, 1,
            "function __defaults__ tuple should be the only default owner before call"
        );

        let builder_bits = crate::call::bind::arguments::molt_callargs_new(1, 0);
        assert!(!obj_from_bits(builder_bits).is_none());
        let _ = unsafe {
            crate::call::bind::arguments::molt_callargs_push_pos(
                builder_bits,
                MoltObject::from_int(5).bits(),
            )
        };

        let result_bits = crate::call::bind::molt_call_bind(func_bits, builder_bits);
        assert_eq!(result_bits, default_bits);
        let result_ptr = obj_from_bits(result_bits)
            .as_ptr()
            .expect("live default result");
        assert_eq!(result_ptr, default_ptr);
        let after_call =
            unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
        assert_eq!(
            after_call, 2,
            "default cleanup must preserve the callee-owned return"
        );

        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn specialized_builtin_binding_owns_raw_admission_with_or_without_trampolines() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            // Cover ordinary arguments, direct dictionary mutation, and
            // closure-owned exception init: all three execution strategies.
            for (name, symbol, arity) in [
                (
                    "molt_object_init_subclass",
                    fn_addr!(crate::molt_object_init_subclass),
                    1,
                ),
                ("molt_object_init", fn_addr!(crate::molt_object_init), 1),
                (
                    "molt_object_new_bound",
                    fn_addr!(crate::molt_object_new_bound),
                    1,
                ),
                ("dict_update_method", fn_addr!(crate::dict_update_method), 2),
                (
                    "molt_exception_init_owned",
                    fn_addr!(crate::builtins::exceptions::molt_exception_init_owned),
                    4,
                ),
            ] {
                let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, symbol, arity);
                assert!(!ptr.is_null());
                let bits = MoltObject::from_ptr(ptr).bits();
                let original = crate::function_trampoline_ptr(ptr);
                for trampoline in [0, 1] {
                    // A non-callable trampoline is deliberate: missing
                    // receivers must be rejected before dispatch reaches it.
                    crate::object::layout::function_set_trampoline_ptr(ptr, trampoline);
                    assert!(
                        crate::call::bind::builtin_args::builtin_call_binding(py, ptr).is_some(),
                        "{name} must retain specialized binding with trampoline {trampoline}"
                    );
                    assert!(crate::call::bind::frame_binding::function_raw_positional_call_needs_binding(
                            py, ptr, 0
                        ));
                    assert!(crate::call::bind::frame_binding::function_raw_positional_call_needs_binding(
                            py,
                            ptr,
                            arity as usize
                        ));
                    assert!(method_ic_call_plan(py, bits).unwrap().needs_binder);
                    let result = crate::call::bind::molt_call_bind(
                        bits,
                        crate::call::bind::arguments::molt_callargs_new(0, 0),
                    );
                    assert!(
                        crate::exception_pending(py),
                        "missing receiver cannot be synthesized"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, result);
                    let result = crate::call::function::call_function_obj_vec(py, bits, &[]);
                    assert!(
                        crate::exception_pending(py),
                        "raw vector calls must use the same binder"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, result);
                }
                crate::object::layout::function_set_trampoline_ptr(ptr, original);
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn published_native_constructors_own_raw_admission_and_argument_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let builtins = builtin_classes(py);
            let key = crate::attr_name_bits_from_bytes(py, b"__new__").unwrap();
            let receiver_ptr = alloc_list(py, &[]);
            assert!(!receiver_ptr.is_null());
            let receiver = MoltObject::from_ptr(receiver_ptr).bits();
            let before = (*crate::object::header_from_obj_ptr(receiver_ptr)).ref_count_snapshot();
            // The published Python constructor owns signature normalization.
            // Lower-level primitives such as molt_int_new have no independent
            // Python binding contract. Exercise every owner of this authority.
            for (name, class) in [
                ("list", builtins.list),
                ("dict", builtins.dict),
                ("set", builtins.set),
                ("frozenset", builtins.frozenset),
                ("tuple", builtins.tuple),
                ("str", builtins.str),
                ("bytes", builtins.bytes),
                ("bytearray", builtins.bytearray),
                ("int", builtins.int),
                ("float", builtins.float),
                ("complex", builtins.complex),
                ("memoryview", builtins.memoryview),
            ] {
                assert!(
                    crate::builtins::types::native_constructors::owns_constructor_descriptors(
                        py, class
                    )
                );
                let function = crate::molt_get_attr_name(class, key);
                assert!(!exception_pending(py), "{name}.__new__ lookup");
                let pointer = obj_from_bits(function).as_ptr().unwrap();
                assert_eq!(object_type_id(pointer), TYPE_ID_FUNCTION);
                assert_eq!(
                    crate::object_class_bits(pointer),
                    builtins.builtin_function_or_method
                );
                assert_eq!(
                    class_attr_lookup_raw_mro(py, obj_from_bits(class).as_ptr().unwrap(), key),
                    Some(function),
                    "{name}.__new__ must use the published callable"
                );
                assert!(builtin_call_binding(py, pointer).is_none());
                assert!(
                    crate::call::bind::frame_binding::function_requires_full_binding(py, pointer)
                );
                for supplied in [0, 1, 2] {
                    assert!(crate::call::bind::frame_binding::function_raw_positional_call_needs_binding(
                        py, pointer, supplied
                    ), "{name}.__new__ raw arguments must be normalized");
                }
                assert!(method_ic_call_plan(py, function).unwrap().needs_binder);
                // Keep the real entry/trampoline: variadic packing precedes
                // the constructor's missing/non-type receiver admission.
                for arguments in [&[][..], &[receiver][..]] {
                    let builder =
                        crate::call::bind::arguments::molt_callargs_new(arguments.len() as u64, 0);
                    for &argument in arguments {
                        crate::call::bind::arguments::molt_callargs_push_pos(builder, argument);
                    }
                    let result = crate::call::bind::molt_call_bind(function, builder);
                    assert!(
                        exception_pending(py),
                        "{name}.__new__ must reject invalid receiver"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, result);
                    assert_eq!(
                        (*crate::object::header_from_obj_ptr(receiver_ptr)).ref_count_snapshot(),
                        before,
                        "{name}.__new__ builder failure must release argument packs"
                    );
                    let result =
                        crate::call::function::call_function_obj_vec(py, function, arguments);
                    assert!(
                        exception_pending(py),
                        "{name}.__new__ vector admission must agree"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, result);
                    assert_eq!(
                        (*crate::object::header_from_obj_ptr(receiver_ptr)).ref_count_snapshot(),
                        before,
                        "{name}.__new__ vector failure must preserve borrowed arguments"
                    );
                }
                dec_ref_bits(py, function);
            }
            dec_ref_bits(py, receiver);
            dec_ref_bits(py, key);
            assert!(!exception_pending(py));
        }
    });
}
