use super::*;
use crate::call::bind::inline_cache::{
    CALL_BIND_IC_KIND_DIRECT_FUNC, CallBindIcEntry, try_call_bind_ic_fast,
};
use crate::call::bind::test_support::*;
use crate::object::builders::{alloc_list, alloc_tuple};

#[test]
fn callargs_pending_error_stops_mutation_and_consuming_dispatch() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let item = alloc_list(_py, &[]);
            let item_bits = MoltObject::from_ptr(item).bits();
            let baseline = (*crate::header_from_obj_ptr(item)).ref_count_snapshot();
            // No callback or invalid-callee TypeError may replace the
            // original exception, including cached and indirect entries.
            for dispatch in 0..3 {
                let builder = crate::call::bind::arguments::molt_callargs_new(1, 0);
                let builder_ptr = crate::call::bind::ptr_from_bits(builder);
                crate::call::bind::arguments::molt_callargs_push_pos(builder, item_bits);
                assert_eq!(
                    (*crate::header_from_obj_ptr(item)).ref_count_snapshot(),
                    baseline + 1
                );
                crate::raise_exception::<()>(_py, "ValueError", "argument failure");
                let original = crate::exception_last_bits_noinc(_py).unwrap();
                assert_eq!(crate::call::bind::arguments::molt_callargs_new(0, 0), 0);
                crate::call::bind::arguments::molt_callargs_push_pos(builder, item_bits);
                crate::call::bind::arguments::molt_callargs_push_kw(builder, item_bits, item_bits);
                // Invalid iterables/mappings would raise fresh exceptions
                // if the failed transaction reached protocol dispatch.
                crate::call::bind::arguments::molt_callargs_expand_star(
                    builder,
                    MoltObject::none().bits(),
                );
                crate::call::bind::arguments::molt_callargs_expand_kwstar(
                    builder,
                    MoltObject::none().bits(),
                );
                assert_eq!(
                    (*crate::call::bind::arguments::callargs_ptr(builder_ptr)).pos,
                    [item_bits]
                );
                assert!(
                    obj_from_bits(
                        (*crate::call::bind::arguments::callargs_ptr(builder_ptr)).keywords
                    )
                    .is_none()
                );
                let invalid_callee = MoltObject::from_int(42).bits();
                let result = match dispatch {
                    0 => crate::call::bind::molt_call_bind(invalid_callee, builder),
                    1 => crate::call::bind::inline_cache::molt_call_bind_ic(
                        23,
                        invalid_callee,
                        builder,
                    ),
                    _ => crate::call::bind::inline_cache::molt_call_indirect_ic(
                        29,
                        invalid_callee,
                        builder,
                    ),
                };
                assert!(obj_from_bits(result).is_none());
                assert_eq!(crate::exception_last_bits_noinc(_py), Some(original));
                assert!(!crate::call::bind::arguments::callargs_builder_is_live(
                    _py,
                    builder_ptr
                ));
                assert_eq!(
                    (*crate::header_from_obj_ptr(item)).ref_count_snapshot(),
                    baseline
                );
                crate::molt_exception_clear();
            }
            dec_ref_bits(_py, item_bits);
        }
    });
}

#[test]
fn callargs_expansion_has_one_keyword_owner_and_releases_iterator() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            // Ownership deltas require mortal payloads, not the canonical
            // immortal identifier strings returned by alloc_string.
            let key = crate::object::builders::alloc_string_nointern(_py, b"key");
            let value = crate::object::builders::alloc_string_nointern(_py, b"value");
            let key_bits = MoltObject::from_ptr(key).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let list = crate::alloc_list(_py, &[value_bits]);
            let list_bits = MoltObject::from_ptr(list).bits();
            let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
            let key_before = refs(key);
            let value_before = refs(value);
            let list_before = refs(list);
            let builder = crate::call::bind::arguments::molt_callargs_new(0, 0);
            crate::call::bind::arguments::molt_callargs_push_kw(builder, key_bits, value_bits);
            assert!(!crate::exception_pending(_py));
            assert_eq!(
                refs(key),
                key_before + 1,
                "expansion must retain keywords only through their dictionary"
            );
            assert_eq!(refs(value), value_before + 1);
            crate::call::bind::arguments::molt_callargs_expand_star(builder, list_bits);
            assert!(!crate::exception_pending(_py));
            assert_eq!(
                refs(list),
                list_before,
                "star expansion must release its iterator"
            );
            assert_eq!(refs(value), value_before + 2);
            crate::dec_ref_bits(_py, builder);
            assert_eq!(refs(key), key_before);
            assert_eq!(refs(value), value_before);
            crate::dec_ref_bits(_py, list_bits);
            crate::dec_ref_bits(_py, key_bits);
            crate::dec_ref_bits(_py, value_bits);
        }
    });
}

#[test]
fn keyword_admission_reads_replacements_and_retains_a_shared_mapping() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let key = crate::object::builders::alloc_string_nointern(_py, b"key");
            let old = alloc_list(_py, &[]);
            let replacement = alloc_list(_py, &[MoltObject::from_int(9).bits()]);
            let later = alloc_list(_py, &[]);
            assert!(!key.is_null() && !old.is_null());
            assert!(!replacement.is_null() && !later.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let old_bits = MoltObject::from_ptr(old).bits();
            let replacement_bits = MoltObject::from_ptr(replacement).bits();
            let later_bits = MoltObject::from_ptr(later).bits();
            let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
            let key_before = refs(key);
            let old_before = refs(old);
            let replacement_before = refs(replacement);
            let builder = crate::call::bind::arguments::molt_callargs_new(0, 1);
            assert_ne!(builder, 0);
            crate::call::bind::arguments::molt_callargs_push_kw(builder, key_bits, old_bits);
            let dict_bits =
                (*crate::call::bind::arguments::callargs_ptr(ptr_from_bits(builder))).keywords;
            let dict = obj_from_bits(dict_bits).as_ptr().unwrap();
            // Both a stateful __eq__ during insertion and foreign mutation
            // can replace an existing value without growing dictionary order.
            crate::dict_set_in_place(_py, dict, key_bits, replacement_bits);
            assert!(!crate::exception_pending(_py));
            assert_eq!(
                refs(old),
                old_before,
                "no stale builder edge may retain the old value"
            );
            // Another owner can still reach the mapping, as a C view or a
            // GC referrer could. Admission must not clear it.
            crate::inc_ref_bits(_py, dict_bits);
            let mut arguments = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                ptr_from_bits(builder),
            )
            .unwrap();
            dec_ref_bits(_py, builder);
            let view = arguments.unpacked_view().unwrap();
            assert_eq!(view.kw_names, [key_bits]);
            assert_eq!(view.kw_values, [replacement_bits]);
            assert_eq!(
                crate::dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .len(),
                2,
                "a shared mapping stays intact"
            );
            assert_eq!(refs(key), key_before + 2);
            assert_eq!(refs(replacement), replacement_before + 2);
            // The other owner's later mutation cannot change what binding reads.
            crate::dict_set_in_place(_py, dict, key_bits, later_bits);
            crate::dict_clear_in_place(_py, dict);
            assert!(!crate::exception_pending(_py));
            let view = arguments.unpacked_view().unwrap();
            assert_eq!(view.kw_values, [replacement_bits]);
            assert_eq!(refs(key), key_before + 1);
            assert_eq!(refs(replacement), replacement_before + 1);
            drop(arguments);
            assert_eq!(refs(key), key_before);
            assert_eq!(refs(replacement), replacement_before);
            dec_ref_bits(_py, dict_bits);
            for bits in [key_bits, old_bits, replacement_bits, later_bits] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn callargs_clone_and_ic_read_the_live_keyword_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let key = crate::alloc_string(_py, b"key");
            assert!(!key.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let builder = crate::call::bind::arguments::molt_callargs_new(1, 1);
            assert_ne!(builder, 0);
            let value = MoltObject::from_int(17).bits();
            crate::call::bind::arguments::molt_callargs_push_pos(builder, value);
            crate::call::bind::arguments::molt_callargs_push_kw(
                builder,
                key_bits,
                MoltObject::from_int(1).bits(),
            );
            let args = crate::call::bind::arguments::callargs_ptr(ptr_from_bits(builder));
            let dict = obj_from_bits((*args).keywords).as_ptr().unwrap();
            crate::dict_set_in_place(_py, dict, key_bits, MoltObject::from_int(2).bits());
            let clone =
                crate::call::bind::arguments::clone_callargs_builder_bits(_py, builder).unwrap();
            let cloned_args = crate::call::bind::arguments::callargs_ptr(ptr_from_bits(clone));
            assert_ne!((*args).keywords, (*cloned_args).keywords);
            let mut cloned = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                ptr_from_bits(clone),
            )
            .unwrap();
            assert_eq!(
                cloned.unpacked_view().unwrap().kw_values,
                [MoltObject::from_int(2).bits()]
            );
            crate::dict_clear_in_place(_py, dict);
            assert_eq!((*args).keyword_count(), 0);
            assert_eq!(cloned.keyword_count(), 1);
            assert_eq!(
                crate::call::bind::arguments::callargs_positional_snapshot(_py, builder).unwrap(),
                [value]
            );
            let mut original = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                ptr_from_bits(builder),
            )
            .unwrap();
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let entry = CallBindIcEntry {
                fn_ptr: crate::function_fn_ptr(func_ptr),
                target_bits: 0,
                class_bits: 0,
                class_version: 0,
                type_version: crate::global_type_version(),
                function_version: 0,
                cached_alloc_size: 0,
                arity: 1,
                kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
            };
            assert_eq!(
                try_call_bind_ic_fast(_py, entry, func_bits, &mut original),
                Some(value)
            );
            assert_eq!(
                try_call_bind_ic_fast(_py, entry, func_bits, &mut cloned),
                None
            );
            drop(original);
            drop(cloned);
            for bits in [builder, clone, key_bits, func_bits] {
                dec_ref_bits(_py, bits);
            }
            assert!(!crate::exception_pending(_py));
        }
    });
}

#[test]
fn callargs_nonstring_keywords_are_rejected_at_call_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builder = crate::call::bind::arguments::molt_callargs_new(0, 0);
            crate::call::bind::arguments::molt_callargs_push_kw(
                builder,
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            );
            assert!(!crate::exception_pending(_py));
            let arguments = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                crate::call::bind::ptr_from_bits(builder),
            )
            .unwrap();
            crate::dec_ref_bits(_py, builder);
            assert!(!arguments.validate_keywords());
            assert!(crate::exception_pending(_py));
            crate::molt_exception_clear();
            // Names already unpacked from a mapping face the same check.
            let unpacked = crate::call::bind::arguments::CallArguments::retained(
                _py,
                None,
                &[],
                &[MoltObject::from_int(3).bits()],
                &[MoltObject::from_int(4).bits()],
            )
            .unwrap();
            assert!(!unpacked.validate_keywords());
            assert!(crate::exception_pending(_py));
            crate::molt_exception_clear();
        }
    });
}

#[test]
fn consuming_entry_moves_builder_edges_into_the_call_argument_vector() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let key = crate::object::builders::alloc_string_nointern(_py, b"key");
            let value = alloc_list(_py, &[]);
            let item = alloc_list(_py, &[]);
            assert!(!key.is_null() && !value.is_null() && !item.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let item_bits = MoltObject::from_ptr(item).bits();
            let refs = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
            let builder = crate::call::bind::arguments::molt_callargs_new(1, 1);
            crate::call::bind::arguments::molt_callargs_push_pos(builder, item_bits);
            crate::call::bind::arguments::molt_callargs_push_kw(builder, key_bits, value_bits);
            assert!(!crate::exception_pending(_py));
            let counts = (refs(item), refs(key), refs(value));
            let builder_ptr = ptr_from_bits(builder);
            let mut arguments =
                crate::call::bind::arguments::CallArguments::from_builder(_py, builder_ptr)
                    .unwrap();
            // T1 moved both edges: the builder is empty and no count changed.
            assert!(
                (*crate::call::bind::arguments::callargs_ptr(builder_ptr))
                    .pos
                    .is_empty()
            );
            assert!(
                obj_from_bits((*crate::call::bind::arguments::callargs_ptr(builder_ptr)).keywords)
                    .is_none()
            );
            assert_eq!(arguments.positional(), [item_bits]);
            assert_eq!((refs(item), refs(key), refs(value)), counts);
            dec_ref_bits(_py, builder);
            assert_eq!(refs(item), counts.0, "an emptied builder releases nothing");
            // This call holds the mapping's only reference: its entries move too.
            let view = arguments.unpacked_view().unwrap();
            assert_eq!(view.kw_names, [key_bits]);
            assert_eq!(view.kw_values, [value_bits]);
            assert_eq!((refs(item), refs(key), refs(value)), counts);
            drop(arguments);
            assert_eq!(
                (refs(item), refs(key), refs(value)),
                (counts.0 - 1, counts.1 - 1, counts.2 - 1),
                "the argument vector released exactly the edges it received"
            );
            for bits in [item_bits, key_bits, value_bits] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn a_shared_builder_keeps_its_edges_and_the_call_retains_its_own() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let item = alloc_list(_py, &[]);
            assert!(!item.is_null());
            let item_bits = MoltObject::from_ptr(item).bits();
            let refs = || (*crate::header_from_obj_ptr(item)).ref_count_snapshot();
            let builder = crate::call::bind::arguments::molt_callargs_new(1, 0);
            crate::call::bind::arguments::molt_callargs_push_pos(builder, item_bits);
            let before = refs();
            // Another owner can still reach the builder: T1 must not drain it.
            crate::inc_ref_bits(_py, builder);
            let arguments = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                ptr_from_bits(builder),
            )
            .unwrap();
            assert_eq!(
                (*crate::call::bind::arguments::callargs_ptr(ptr_from_bits(builder))).pos,
                [item_bits]
            );
            assert_eq!(refs(), before + 1);
            drop(arguments);
            assert_eq!(refs(), before);
            dec_ref_bits(_py, builder);
            dec_ref_bits(_py, builder);
            assert_eq!(refs(), before - 1);
            dec_ref_bits(_py, item_bits);
        }
    });
}

#[test]
fn extension_callees_receive_the_whole_mapping_or_a_fresh_one() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let key = crate::object::builders::alloc_string_nointern(_py, b"key");
            let value = alloc_list(_py, &[]);
            assert!(!key.is_null() && !value.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let builder = crate::call::bind::arguments::molt_callargs_new(0, 1);
            crate::call::bind::arguments::molt_callargs_push_kw(builder, key_bits, value_bits);
            let dict_bits =
                (*crate::call::bind::arguments::callargs_ptr(ptr_from_bits(builder))).keywords;
            let mut arguments = crate::call::bind::arguments::CallArguments::from_builder(
                _py,
                ptr_from_bits(builder),
            )
            .unwrap();
            dec_ref_bits(_py, builder);
            // While whole, the builder's own dictionary is lent without copying.
            let whole = arguments.keyword_mapping().unwrap();
            assert_eq!(whole, dict_bits);
            dec_ref_bits(_py, whole);
            // Once unpacked, a C callee still receives a mapping it may keep.
            let _ = arguments.unpacked_view().unwrap();
            let fresh = arguments.keyword_mapping().unwrap();
            let fresh_ptr = obj_from_bits(fresh).as_ptr().unwrap();
            assert_eq!(
                crate::dict_live_entries(fresh_ptr)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                [key_bits, value_bits]
            );
            drop(arguments);
            assert_eq!(
                (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                2,
                "the fresh mapping and this test own the value"
            );
            dec_ref_bits(_py, fresh);
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, value_bits);
        }
    });
}

#[test]
fn borrowing_callees_leave_the_last_release_to_the_call_instruction() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let probes = ReleaseProbes::new(_py);
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let [a, b, args, kwargs] = [&b"a"[..], b"b", b"args", b"kwargs"].map(string);
            let no_names = MoltObject::from_ptr(alloc_tuple(_py, &[])).bits();
            // A native `(*args, **kwargs)` callable, like `"".format`: it
            // binds its own references and never inlines a frame.
            let variadic = metadata_function(
                _py,
                none_of_two as *const (),
                2,
                &[
                    (b"__molt_arg_names__", no_names),
                    (b"__molt_vararg__", args),
                    (b"__molt_varkw__", kwargs),
                ],
                true,
            );
            // CALL cleanup is DECREF_INPUTS over the stack; CALL_FUNCTION_EX
            // releases its tuple and mapping. 3.14 reversed both.
            for (minor, stack, expanded) in [
                (12, [0, 1, 2, 3], [1, 0, 2, 3]),
                (13, [0, 1, 2, 3], [1, 0, 2, 3]),
                (14, [3, 2, 1, 0], [2, 3, 1, 0]),
            ] {
                with_target_minor(_py, minor, || {
                    for (form, expected) in [
                        (crate::call::bind::arguments::CallForm::Stack, stack),
                        (crate::call::bind::arguments::CallForm::Expanded, expanded),
                    ] {
                        let values = probes.instances(_py, 4);
                        let builder = last_owner_call(
                            _py,
                            form,
                            &values[..2],
                            &[(a, values[2]), (b, values[3])],
                        );
                        let result = crate::call::bind::molt_call_bind(variadic, builder);
                        assert!(obj_from_bits(result).is_none());
                        assert!(!crate::exception_pending(_py));
                        assert_eq!(
                            ReleaseProbes::released(&values),
                            expected,
                            "3.{minor} {form:?}"
                        );
                    }
                });
            }
            for bits in [variadic, no_names, a, b, args, kwargs] {
                dec_ref_bits(_py, bits);
            }
            probes.release(_py);
        }
    });
}

#[test]
fn keyword_mapping_keeps_the_exception_a_name_callback_raised() {
    extern "C" fn raising_hash(_self_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            crate::raise_exception::<u64>(_py, "ValueError", "keyword name hash")
        })
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builtins = crate::builtin_classes(_py);
            let hash = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(raising_hash as *const ()),
                1,
            );
            assert!(!hash.is_null());
            let hash_bits = MoltObject::from_ptr(hash).bits();
            let hash_name = MoltObject::from_ptr(crate::alloc_string(_py, b"__hash__")).bits();
            let namespace = crate::alloc_dict_with_pairs(_py, &[hash_name, hash_bits]);
            assert!(!namespace.is_null());
            let namespace_bits = MoltObject::from_ptr(namespace).bits();
            let class_name = MoltObject::from_ptr(crate::alloc_string(_py, b"Name")).bits();
            // `class Name(str): __hash__ = raising_hash`
            let class_bits = crate::builtins::types::molt_type_new(
                builtins.type_obj,
                class_name,
                builtins.str,
                namespace_bits,
                MoltObject::none().bits(),
            );
            assert!(!obj_from_bits(class_bits).is_none() && !crate::exception_pending(_py));
            // A `Name("key")` instance: string storage of the subclass.
            let text = b"key";
            let key = crate::object::builders::alloc_native_inline_bytes(
                _py,
                class_bits,
                crate::object::native_instance::NativePayload::String,
                text,
            );
            assert!(!key.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let arguments = crate::call::bind::arguments::CallArguments::retained(
                _py,
                None,
                &[],
                &[key_bits],
                &[MoltObject::from_int(1).bits()],
            )
            .unwrap();
            assert!(
                arguments.validate_keywords(),
                "a str subclass names a keyword"
            );
            // Building the fresh mapping hashes the name; its callback raises.
            let error = arguments.keyword_mapping().unwrap_err();
            assert!(obj_from_bits(error).is_none());
            let pending = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(
                crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    pending,
                    "ValueError"
                ),
                "the callback's exception reaches the caller, not a MemoryError"
            );
            let _ = crate::molt_exception_clear();
            dec_ref_bits(_py, pending);
            drop(arguments);
            for bits in [
                key_bits,
                class_bits,
                class_name,
                namespace_bits,
                hash_name,
                hash_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn callargs_registries_are_runtime_scoped() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        {
            let mut guard = state.call_bind.lock().unwrap();
            guard.callargs_builder_map.clear();
            guard.callargs_storage_registry.clear();
        }

        let builder_bits = crate::call::bind::arguments::molt_callargs_new(1, 0);
        assert!(!obj_from_bits(builder_bits).is_none());
        let builder_ptr = ptr_from_bits(builder_bits);
        assert!(!builder_ptr.is_null());
        let args_ptr = unsafe { crate::call::bind::arguments::callargs_ptr(builder_ptr) };
        assert!(!args_ptr.is_null());
        {
            let guard = state.call_bind.lock().unwrap();
            assert_eq!(guard.callargs_builder_map.len(), 1);
            assert_eq!(guard.callargs_storage_registry.len(), 1);
            assert!(
                guard
                    .callargs_builder_map
                    .contains_key(&(builder_ptr as usize))
            );
            assert!(
                guard
                    .callargs_storage_registry
                    .contains(&(args_ptr as usize))
            );
        }

        dec_ref_bits(_py, builder_bits);
        {
            let guard = state.call_bind.lock().unwrap();
            assert!(guard.callargs_builder_map.is_empty());
            assert!(guard.callargs_storage_registry.is_empty());
        }
    });
}

#[test]
fn capi_arguments_keep_forward_value_release_across_target_versions_and_binding_errors() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let probes = ReleaseProbes::new(_py);
            let names: Vec<_> = [b"a", b"b", b"c", b"d"]
                .map(|name| MoltObject::from_ptr(crate::alloc_string(_py, name)).bits())
                .into();
            let parameter_names = MoltObject::from_ptr(alloc_tuple(_py, &names)).bits();
            let function = metadata_function(
                _py,
                none_of_four as *const (),
                4,
                &[(b"__molt_arg_names__", parameter_names)],
                false,
            );
            let finalizer = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(record_release as *const ()),
                1,
            );
            let finalizer_bits = MoltObject::from_ptr(finalizer).bits();
            let del_name = MoltObject::from_ptr(crate::alloc_string(_py, b"__del__")).bits();
            let class_name =
                MoltObject::from_ptr(crate::alloc_string(_py, b"KeywordReleaseProbe")).bits();
            let namespace = alloc_dict_with_pairs(_py, &[del_name, finalizer_bits]);
            let namespace_bits = MoltObject::from_ptr(namespace).bits();
            let builtins = crate::builtin_classes(_py);
            let key_class = crate::builtins::types::molt_type_new(
                builtins.type_obj,
                class_name,
                builtins.str,
                namespace_bits,
                MoltObject::none().bits(),
            );
            assert!(!crate::exception_pending(_py));
            for minor in [12, 13, 14] {
                transaction.with_target_python_minor(_py, minor, || {
                    for fail_binding in [false, true] {
                        RELEASED.lock().unwrap().clear();
                        let values = probes.instances(_py, 4);
                        let keys = [if fail_binding { b"a" } else { b"c" }, b"d"].map(|text| {
                            let key = crate::object::builders::alloc_native_inline_bytes(
                                _py,
                                key_class,
                                crate::object::native_instance::NativePayload::String,
                                text,
                            );
                            assert!(!key.is_null());
                            MoltObject::from_ptr(key).bits()
                        });
                        let mapping =
                            alloc_dict_with_pairs(_py, &[keys[0], values[2], keys[1], values[3]]);
                        assert!(!mapping.is_null());
                        let mapping_bits = MoltObject::from_ptr(mapping).bits();
                        let arguments =
                            CallArguments::capi(_py, Some(values[0]), &values[1..2], mapping_bits)
                                .unwrap();
                        // Make the real dispatch owner the last holder, then run
                        // binding: the success and duplicate-value error paths
                        // must both follow _PyStack_UnpackDict_Free's order.
                        for &bits in &values {
                            dec_ref_bits(_py, bits);
                        }
                        for &bits in &keys {
                            dec_ref_bits(_py, bits);
                        }
                        dec_ref_bits(_py, mapping_bits);
                        let result = call_bind_with_arguments(_py, function, arguments);
                        assert!(obj_from_bits(result).is_none());
                        assert_eq!(crate::exception_pending(_py), fail_binding);
                        assert_eq!(
                            ReleaseProbes::released(&[values.as_slice(), &keys].concat()),
                            [0, 1, 2, 3, 5, 4],
                            "3.{minor} failure={fail_binding}"
                        );
                        if fail_binding {
                            crate::molt_exception_clear();
                        }
                    }
                });
            }
            for bits in names.into_iter().chain([parameter_names, function]) {
                dec_ref_bits(_py, bits);
            }
            for bits in [
                key_class,
                namespace_bits,
                class_name,
                del_name,
                finalizer_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
            probes.release(_py);
        }
    });
}

#[test]
fn capi_borrowed_spans_keep_caller_ownership_and_promote_only_receiver_span() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let item = alloc_list(py, &[]);
            let value = alloc_list(py, &[]);
            let name = crate::alloc_string(py, b"payload");
            let item_bits = MoltObject::from_ptr(item).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let name_bits = MoltObject::from_ptr(name).bits();
            let count = |ptr| (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot();
            let before = [count(item), count(value), count(name)];
            let positional = [item_bits];
            let names = [name_bits];
            let values = [value_bits];
            {
                let mut arguments =
                    CallArguments::capi_vector(py, &positional, &names, &values).unwrap();
                assert!(matches!(arguments.admission(), Admission::Copy));
                assert_eq!([count(item), count(value), count(name)], before);
                let view = arguments.unpacked_view().unwrap();
                assert_eq!(view.pos.as_ptr(), positional.as_ptr());
                assert_eq!(view.kw_names.as_ptr(), names.as_ptr());
                assert_eq!(view.kw_values.as_ptr(), values.as_ptr());
                arguments.prepend_positional(item_bits).unwrap();
                assert_eq!(arguments.positional(), &[item_bits, item_bits]);
                assert_eq!(count(item), before[0] + 2);
                assert_eq!([count(value), count(name)], [before[1], before[2]]);
            }
            assert_eq!([count(item), count(value), count(name)], before);
            let mapping = alloc_dict_with_pairs(py, &[name_bits, value_bits]);
            let mapping_bits = MoltObject::from_ptr(mapping).bits();
            let parent = CallArguments::capi(py, None, &positional, mapping_bits).unwrap();
            let mapping_count = count(mapping);
            {
                let child = parent.constructor_child(None, parent.positional()).unwrap();
                assert_eq!(child.positional().as_ptr(), positional.as_ptr());
                assert_eq!(child.capi_mapping(), Some(mapping_bits));
                assert!(matches!(child.admission(), Admission::Copy));
                assert_eq!(count(item), before[0]);
                assert_eq!(count(mapping), mapping_count + 1);
            }
            assert_eq!(count(mapping), mapping_count);
            drop(parent);
            for bits in [mapping_bits, item_bits, value_bits, name_bits] {
                dec_ref_bits(py, bits);
            }
            assert!(!crate::exception_pending(py));
        }
    });
}
