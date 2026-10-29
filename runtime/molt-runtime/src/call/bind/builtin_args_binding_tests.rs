use super::*;
use crate::BuiltinExc;
use crate::call::bind::test_support::*;
use crate::call_function_obj_vec;
use crate::object::builders::{alloc_list, alloc_tuple};

#[test]
fn raw_function_allocation_publishes_specialized_binder_policy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let pointer = crate::object::builders::alloc_function_obj(
                py,
                fn_addr!(crate::molt_object_new_bound),
                1,
            );
            assert!(!pointer.is_null());
            assert!(frame_binding::function_requires_binder_flag(pointer));
            let function = MoltObject::from_ptr(pointer).bits();
            let result =
                crate::call::function::call_function_obj1(py, function, MoltObject::none().bits());
            assert!(
                exception_pending(py),
                "object.__new__ needs a type receiver"
            );
            crate::molt_exception_clear();
            dec_ref_bits(py, result);
            dec_ref_bits(py, function);
        }
    });
}

#[test]
fn direct_code_attachment_publishes_signature_binder_policy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = MoltObject::from_ptr(crate::alloc_string(py, b"keyword")).bits();
            let names = MoltObject::from_ptr(alloc_tuple(py, &[name])).bits();
            let positional = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let source = metadata_function(
                py,
                none_of_one as *const (),
                1,
                &[
                    (b"__molt_arg_names__", positional),
                    (b"__molt_kwonly_names__", names),
                ],
                false,
            );
            let code = crate::builtins::functions::molt_function_get_code(source);
            assert!(!exception_pending(py));
            assert!(!obj_from_bits(code).is_none());
            let pointer = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(none_of_one as *const ()),
                1,
            );
            assert!(!pointer.is_null());
            let function = MoltObject::from_ptr(pointer).bits();
            assert!(!frame_binding::function_requires_binder_flag(pointer));
            assert!(crate::object::layout::function_set_code_bits(
                py, pointer, code
            ));
            assert!(frame_binding::function_requires_binder_flag(pointer));
            let code_pointer = obj_from_bits(code).as_ptr().unwrap();
            assert_eq!(
                crate::object::layout::code_published_execution_kind(code_pointer),
                None
            );
            let none = MoltObject::none().bits();
            let zero = MoltObject::from_int(0).bits();
            // Both functions observe this code before its execution kind is
            // sealed. Neither writer may replace the shared signature.
            assert!(
                crate::object::layout::code_set_signature_bits(
                    py,
                    code_pointer,
                    names,
                    zero,
                    positional,
                    none,
                    none,
                )
                .is_err()
            );
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            let replacement = crate::object::layout::prepare_code_signature(
                py,
                [names, zero, positional, none, none],
            )
            .unwrap();
            assert!(
                crate::object::layout::prepare_function_code_bits(
                    py,
                    pointer,
                    code,
                    None,
                    Some(replacement),
                )
                .is_err()
            );
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                crate::object::layout::code_kwonly_names_bits(code_pointer),
                names
            );
            assert!(frame_binding::function_requires_binder_flag(
                obj_from_bits(source).as_ptr().unwrap(),
            ));
            assert!(frame_binding::function_requires_binder_flag(pointer));
            // This is an exact machine-arity call, but Python's published
            // signature rejects the positional argument. A stale false flag
            // would enter none_of_one and silently return None.
            let result = crate::call::function::call_function_obj1(
                py,
                function,
                MoltObject::from_int(17).bits(),
            );
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            dec_ref_bits(py, result);
            for bits in [function, code, source, positional, names, name] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn public_keyword_defaults_publish_binder_policy_before_the_next_call() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            use crate::object::function_metadata::{FunctionMetadataField as Field, write_public};
            let name = MoltObject::from_ptr(crate::alloc_string(py, b"value")).bits();
            let names = MoltObject::from_ptr(alloc_tuple(py, &[name])).bits();
            let function = metadata_function(
                py,
                none_of_one as *const (),
                1,
                &[(b"__molt_arg_names__", names)],
                false,
            );
            let pointer = obj_from_bits(function).as_ptr().unwrap();
            let defaults = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
            // Presence, including an empty mapping, requires binding. Public
            // mutation must publish both true and false before another call.
            for (replacement, requires_binding) in [(Some(defaults), true), (None, false)] {
                write_public(py, pointer, Field::KeywordDefaults, replacement).unwrap();
                assert_eq!(
                    frame_binding::function_requires_binder_flag(pointer),
                    requires_binding,
                );
                assert_eq!(
                    frame_binding::function_raw_positional_call_needs_binding(py, pointer, 1),
                    requires_binding,
                );
                let result = crate::call::function::call_function_obj1(
                    py,
                    function,
                    MoltObject::from_int(17).bits(),
                );
                assert!(!exception_pending(py));
                assert!(obj_from_bits(result).is_none());
                dec_ref_bits(py, result);
            }
            for bits in [defaults, function, names, name] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

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
        unsafe {
            use crate::object::function_metadata::{FunctionMetadataField as Field, write_public};
            let empty = MoltObject::from_ptr(alloc_tuple(_py, &[])).bits();
            let value = MoltObject::from_int(29).bits();
            let replacement = MoltObject::from_ptr(alloc_tuple(_py, &[value])).bits();
            for (defaults, needs_binding) in [
                (Some(empty), false),
                (None, false),
                (Some(replacement), true),
            ] {
                write_public(_py, func_ptr, Field::Defaults, defaults).unwrap();
                assert_eq!(
                    frame_binding::function_requires_full_binding(_py, func_ptr),
                    needs_binding,
                );
                assert_eq!(
                    frame_binding::function_raw_positional_call_needs_binding(_py, func_ptr, 1),
                    needs_binding,
                );
                let result = crate::call::function::call_function_obj1(
                    _py,
                    func_bits,
                    MoltObject::from_int(5).bits(),
                );
                if needs_binding {
                    assert!(!exception_pending(_py));
                    assert_eq!(result, value);
                } else {
                    assert!(exception_pending(_py));
                    crate::molt_exception_clear();
                }
                dec_ref_bits(_py, result);
            }
            dec_ref_bits(_py, empty);
            dec_ref_bits(_py, replacement);
        }
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
                    crate::object::layout::function_set_trampoline_ptr(py, ptr, trampoline);
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
                    assert!(
                        crate::call::bind::frame_binding::function_binding_shape(py, ptr)
                            .full_binder
                    );
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
                crate::object::layout::function_set_trampoline_ptr(py, ptr, original);
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
                    class_attr_lookup_raw_mro(py, obj_from_bits(class).as_ptr().unwrap(), key)
                        .expect("operation succeeds"),
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
                assert!(
                    crate::call::bind::frame_binding::function_binding_shape(py, pointer)
                        .full_binder
                );
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

/// Compare public call carriers against the fixed native convention, rather
/// than mirroring its implementation. These labels are intentionally unrelated
/// to any runtime builtin symbol or the eventual stdlib consumer.
#[test]
fn fixed_native_module_binding_preserves_direct_entry_and_public_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"declared_probe").unwrap();
            let qualified = crate::attr_name_bits_from_bytes(py, b"declared_qualified").unwrap();
            let module = crate::attr_name_bits_from_bytes(py, b"probe_module").unwrap();
            let keyword = crate::attr_name_bits_from_bytes(py, b"value").unwrap();
            for (arity, address) in [
                (0, none_of_none as *const ()),
                (1, none_of_one as *const ()),
            ] {
                let function = metadata_function(
                    py,
                    address,
                    arity,
                    &[
                        (b"__name__", name),
                        (b"__qualname__", qualified),
                        (b"__module__", module),
                    ],
                    false,
                );
                let pointer = obj_from_bits(function).as_ptr().unwrap();
                let original_entry = function_fn_ptr(pointer);
                let original_version = crate::function_mutation_version(pointer);
                let original_trampoline = function_trampoline_ptr(pointer);
                crate::builtins::functions::molt_function_set_fixed_builtin(
                    function,
                    MoltObject::from_int(arity as i64).bits(),
                );
                assert!(!exception_pending(py));
                assert_ne!(crate::function_mutation_version(pointer), original_version);
                assert_eq!(
                    object_class_bits(pointer),
                    builtin_classes(py).builtin_function_or_method
                );
                assert_eq!(function_fn_ptr(pointer), original_entry);
                assert_eq!(function_trampoline_ptr(pointer), original_trampoline);
                assert!(!frame_binding::function_requires_binder_flag(pointer));
                assert!(!frame_binding::function_raw_positional_call_needs_binding(
                    py,
                    pointer,
                    arity as usize
                ));
                let positional = [MoltObject::from_int(17).bits(); 2];
                let valid = call_function_obj_vec(py, function, &positional[..arity as usize]);
                assert!(obj_from_bits(valid).is_none());
                assert!(!exception_pending(py));
                if arity == 0 {
                    assert!(
                        obj_from_bits(crate::call::function::call_function_obj0(py, function))
                            .is_none()
                    );
                } else {
                    assert!(
                        obj_from_bits(crate::call::function::call_function_obj1(
                            py,
                            function,
                            positional[0]
                        ))
                        .is_none()
                    );
                }
                assert!(!exception_pending(py));
                for supplied in 0..=2 {
                    for has_keyword in [false, true] {
                        if supplied == arity as usize && !has_keyword {
                            continue;
                        }
                        let expected = if has_keyword {
                            "probe_module.declared_qualified() takes no keyword arguments"
                                .to_owned()
                        } else if arity == 0 {
                            format!(
                                "probe_module.declared_qualified() takes no arguments ({supplied} given)"
                            )
                        } else {
                            format!(
                                "probe_module.declared_qualified() takes exactly one argument ({supplied} given)"
                            )
                        };
                        // The bad trampoline is a bite check: rejected public
                        // arguments must never enter executable transport.
                        crate::object::layout::function_set_trampoline_ptr(py, pointer, 1);
                        for form in [CallForm::Stack, CallForm::Expanded] {
                            let builder = match form {
                                CallForm::Stack => arguments::molt_callargs_new(
                                    supplied as u64,
                                    u64::from(has_keyword),
                                ),
                                CallForm::Expanded => arguments::molt_callargs_new_expanded(
                                    supplied as u64,
                                    u64::from(has_keyword),
                                ),
                            };
                            for &value in &positional[..supplied] {
                                arguments::molt_callargs_push_pos(builder, value);
                            }
                            if has_keyword {
                                arguments::molt_callargs_push_kw(builder, keyword, positional[0]);
                            }
                            let result = molt_call_bind(function, builder);
                            assert!(obj_from_bits(result).is_none());
                            let error = crate::exception_last_bits_noinc(py)
                                .expect("fixed argument rejection");
                            assert_eq!(
                                crate::format_exception_message(
                                    py,
                                    obj_from_bits(error).as_ptr().unwrap()
                                ),
                                expected
                            );
                            crate::molt_exception_clear();
                        }
                        if !has_keyword {
                            call_function_obj_vec(py, function, &positional[..supplied]);
                            let error = crate::exception_last_bits_noinc(py)
                                .expect("raw fixed argument rejection");
                            assert_eq!(
                                crate::format_exception_message(
                                    py,
                                    obj_from_bits(error).as_ptr().unwrap()
                                ),
                                expected
                            );
                            crate::molt_exception_clear();
                        }
                        crate::object::layout::function_set_trampoline_ptr(
                            py,
                            pointer,
                            original_trampoline,
                        );
                    }
                }
                // __module__ is the one writable native display field. It
                // changes error display without changing convention or entry.
                let key = crate::attr_name_bits_from_bytes(py, b"__module__").unwrap();
                let builtins = crate::attr_name_bits_from_bytes(py, b"builtins").unwrap();
                crate::molt_set_attr_name(function, key, builtins);
                assert!(!exception_pending(py));
                call_function_obj_vec(py, function, &positional[..2]);
                let error = crate::exception_last_bits_noinc(py).unwrap();
                assert!(
                    crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap())
                        .starts_with("declared_qualified() takes ")
                );
                crate::molt_exception_clear();
                for bits in [key, builtins, function] {
                    dec_ref_bits(py, bits);
                }
            }
            let alias = metadata_function(
                py,
                compiled_identity_returns_owned_arg as *const (),
                1,
                &[
                    (b"__name__", name),
                    (b"__qualname__", qualified),
                    (b"__module__", module),
                ],
                false,
            );
            crate::builtins::functions::molt_function_set_fixed_builtin(
                alias,
                MoltObject::from_int(1).bits(),
            );
            assert!(!exception_pending(py));
            let value_pointer = alloc_list(py, &[MoltObject::from_int(31).bits()]);
            let value = MoltObject::from_ptr(value_pointer).bits();
            for carrier in 0..3 {
                let result = if carrier == 0 {
                    call_function_obj_vec(py, alias, &[value])
                } else {
                    let builder = if carrier == 1 {
                        arguments::molt_callargs_new(1, 0)
                    } else {
                        arguments::molt_callargs_new_expanded(1, 0)
                    };
                    arguments::molt_callargs_push_pos(builder, value);
                    molt_call_bind(alias, builder)
                };
                assert!(!exception_pending(py));
                assert_eq!(
                    result, value,
                    "a module convention never injects a receiver"
                );
                assert_eq!(
                    (*header_from_obj_ptr(value_pointer)).ref_count_snapshot(),
                    2
                );
                dec_ref_bits(py, result);
                assert_eq!(
                    (*header_from_obj_ptr(value_pointer)).ref_count_snapshot(),
                    1
                );
            }
            dec_ref_bits(py, value);
            dec_ref_bits(py, alias);
            for bits in [name, qualified, module, keyword] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn fixed_native_promotion_rejects_invalid_declarations_atomically_and_seals_public_shape() {
    use crate::object::function_metadata::FunctionMetadataField as Field;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let qualified = crate::attr_name_bits_from_bytes(py, b"sealed_probe").unwrap();
            let function = metadata_function(
                py,
                none_of_one as *const (),
                1,
                &[(b"__qualname__", qualified)],
                false,
            );
            for metadata in [MoltObject::none().bits(), MoltObject::from_int(9).bits()] {
                let invalid = metadata_function(
                    py,
                    none_of_one as *const (),
                    1,
                    &[(b"__qualname__", metadata)],
                    false,
                );
                let invalid_pointer = obj_from_bits(invalid).as_ptr().unwrap();
                let invalid_class = object_class_bits(invalid_pointer);
                crate::builtins::functions::molt_function_set_fixed_builtin(
                    invalid,
                    MoltObject::from_int(1).bits(),
                );
                assert!(exception_pending(py));
                assert_eq!(object_class_bits(invalid_pointer), invalid_class);
                assert!(
                    crate::builtins::functions::native_callable::fixed_builtin_convention(
                        invalid_pointer
                    )
                    .is_none()
                );
                crate::molt_exception_clear();
                dec_ref_bits(py, invalid);
            }
            let pointer = obj_from_bits(function).as_ptr().unwrap();
            let original_class = object_class_bits(pointer);
            let original_kind = Field::BindKind.load(pointer);
            let original_entry = function_fn_ptr(pointer);
            for count in [
                MoltObject::from_int(-1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::none().bits(),
                MoltObject::from_int(0).bits(),
            ] {
                crate::builtins::functions::molt_function_set_fixed_builtin(function, count);
                assert!(exception_pending(py));
                assert_eq!(object_class_bits(pointer), original_class);
                assert_eq!(Field::BindKind.load(pointer), original_kind);
                assert_eq!(function_fn_ptr(pointer), original_entry);
                crate::molt_exception_clear();
            }
            crate::raise_exception::<()>(py, BuiltinExc::RuntimeError, "original pending failure");
            let pending = crate::exception_last_bits_noinc(py).unwrap();
            crate::builtins::functions::molt_function_set_fixed_builtin(
                function,
                MoltObject::from_int(1).bits(),
            );
            assert_eq!(crate::exception_last_bits_noinc(py), Some(pending));
            assert_eq!(object_class_bits(pointer), original_class);
            assert_eq!(Field::BindKind.load(pointer), original_kind);
            crate::molt_exception_clear();
            let two = metadata_function(
                py,
                none_of_two as *const (),
                2,
                &[(b"__qualname__", qualified)],
                false,
            );
            let two_pointer = obj_from_bits(two).as_ptr().unwrap();
            let two_class = object_class_bits(two_pointer);
            let two_kind = Field::BindKind.load(two_pointer);
            for count in 0..=1 {
                crate::builtins::functions::molt_function_set_fixed_builtin(
                    two,
                    MoltObject::from_int(count).bits(),
                );
                assert!(exception_pending(py));
                assert_eq!(object_class_bits(two_pointer), two_class);
                assert_eq!(Field::BindKind.load(two_pointer), two_kind);
                crate::molt_exception_clear();
            }
            let two_code = crate::builtins::functions::molt_function_get_code(two);
            assert!(!exception_pending(py));
            assert!(!obj_from_bits(two_code).is_none());
            crate::builtins::functions::molt_function_set_fixed_builtin(
                function,
                MoltObject::from_int(1).bits(),
            );
            assert!(!exception_pending(py));
            let fixed_kind = Field::BindKind.load(pointer);
            let defaults =
                MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(9).bits()])).bits();
            for (field, replacement) in [
                (b"__code__".as_slice(), two_code),
                (b"__defaults__".as_slice(), defaults),
                (b"__molt_arg_names__".as_slice(), defaults),
            ] {
                let key = crate::attr_name_bits_from_bytes(py, field).unwrap();
                crate::molt_set_attr_name(function, key, replacement);
                let error = crate::exception_last_bits_noinc(py)
                    .expect("native shape is not public mutable state");
                assert!(crate::builtins::attr::exception_is_attribute_error(
                    py, error
                ));
                crate::molt_exception_clear();
                assert_eq!(function_arity(pointer), 1);
                assert_eq!(function_fn_ptr(pointer), original_entry);
                assert_eq!(Field::BindKind.load(pointer), fixed_kind);
                dec_ref_bits(py, key);
            }
            assert!(
                obj_from_bits(crate::call::function::call_function_obj1(
                    py,
                    function,
                    MoltObject::from_int(3).bits()
                ))
                .is_none()
            );
            assert!(!exception_pending(py));
            // Existing promotions retain their original interpretation and
            // cannot silently rewrite an explicitly fixed convention.
            crate::builtins::functions::molt_function_set_builtin(function);
            assert!(!exception_pending(py));
            assert_eq!(Field::BindKind.load(pointer), fixed_kind);
            for bits in [defaults, two_code, two, function, qualified] {
                dec_ref_bits(py, bits);
            }
        }
    });
}
