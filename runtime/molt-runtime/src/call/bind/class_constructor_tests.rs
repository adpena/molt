use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct CallbackState {
    events: Vec<String>,
    prepared: u64,
    class_cell: u64,
    namespace_cell: u64,
}
thread_local! {
    static CALLBACKS: RefCell<CallbackState> = RefCell::new(CallbackState::default());
}

unsafe fn call_type(_py: &PyToken<'_>, name: u64, bases: u64, namespace: u64) -> u64 {
    unsafe {
        let args = molt_callargs_new(3, 0);
        assert!(obj_from_bits(args).as_ptr().is_some());
        molt_callargs_push_pos(args, name);
        molt_callargs_push_pos(args, bases);
        molt_callargs_push_pos(args, namespace);
        molt_call_bind(builtin_classes(_py).type_obj, args)
    }
}

unsafe fn empty_type(_py: &PyToken<'_>, name: &[u8], meta: u64, bases: &[u64]) -> u64 {
    unsafe {
        let name_ptr = alloc_string(_py, name);
        let ns = alloc_dict_with_pairs(_py, &[]);
        let base_tuple = alloc_tuple(_py, bases);
        assert!(!name_ptr.is_null() && !ns.is_null() && !base_tuple.is_null());
        let result = molt_type_new(
            meta,
            MoltObject::from_ptr(name_ptr).bits(),
            MoltObject::from_ptr(base_tuple).bits(),
            MoltObject::from_ptr(ns).bits(),
            MoltObject::none().bits(),
        );
        for ptr in [name_ptr, ns, base_tuple] {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        }
        assert!(!exception_pending(_py));
        assert_eq!(
            crate::object_type_id(obj_from_bits(result).as_ptr().unwrap()),
            TYPE_ID_TYPE
        );
        result
    }
}

unsafe fn function_constructor_call(
    py: &PyToken<'_>,
    direct_new: bool,
    positional: &[u64],
    keywords: &[(&[u8], u64)],
) -> u64 {
    unsafe {
        let class = builtin_classes(py).function;
        let callable = if direct_new {
            let name = crate::attr_name_bits_from_bytes(py, b"__new__").unwrap();
            let result = crate::molt_get_attr_name(class, name);
            dec_ref_bits(py, name);
            assert!(!exception_pending(py));
            result
        } else {
            class
        };
        let builder = molt_callargs_new(
            positional.len() as u64 + u64::from(direct_new),
            keywords.len() as u64,
        );
        if direct_new {
            molt_callargs_push_pos(builder, class);
        }
        for &value in positional {
            molt_callargs_push_pos(builder, value);
        }
        for &(name, value) in keywords {
            let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
            molt_callargs_push_kw(builder, key, value);
            dec_ref_bits(py, key);
        }
        let result = molt_call_bind(callable, builder);
        if direct_new {
            dec_ref_bits(py, callable);
        }
        result
    }
}

#[test]
fn function_type_constructor_keywords_share_clinic_error_precedence() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let marker = MoltObject::from_int(7).bits();
            for minor in [12, 13, 14, 15] {
                transaction.with_target_python_minor(py, minor, || {
                    for direct_new in [false, true] {
                        let maximum = if minor == 12 { 5 } else { 6 };
                        for (positional, keywords, expected) in [
                            (
                                vec![],
                                vec![(b"unexpected".as_slice(), marker)],
                                "function() missing required argument 'code' (pos 1)".to_owned(),
                            ),
                            (
                                vec![marker],
                                vec![(b"code".as_slice(), marker)],
                                "function() missing required argument 'globals' (pos 2)".to_owned(),
                            ),
                            (
                                vec![marker, marker],
                                vec![(b"code".as_slice(), marker), (b"unexpected", marker)],
                                "argument for function() given by name ('code') and position (1)"
                                    .to_owned(),
                            ),
                            (
                                vec![marker; 7],
                                vec![],
                                format!("function() takes at most {maximum} arguments (7 given)"),
                            ),
                            (
                                vec![marker, marker],
                                vec![(b"unexpected".as_slice(), marker)],
                                if minor == 12 {
                                    "'unexpected' is an invalid keyword argument for function()"
                                        .to_owned()
                                } else {
                                    "function() got an unexpected keyword argument 'unexpected'"
                                        .to_owned()
                                },
                            ),
                            (
                                vec![marker, marker],
                                vec![(b"kwdefaults".as_slice(), marker)],
                                if minor == 12 {
                                    "'kwdefaults' is an invalid keyword argument for function()"
                                        .to_owned()
                                } else {
                                    "function() argument 'code' must be code, not int".to_owned()
                                },
                            ),
                        ] {
                            let result =
                                function_constructor_call(py, direct_new, &positional, &keywords);
                            assert!(obj_from_bits(result).is_none());
                            let error =
                                crate::exception_last_bits_noinc(py).expect("binding failure");
                            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                                py,
                                error,
                                "TypeError"
                            ));
                            assert_eq!(
                                crate::format_exception_message(
                                    py,
                                    obj_from_bits(error).as_ptr().unwrap()
                                ),
                                expected
                            );
                            crate::molt_exception_clear();
                            dec_ref_bits(py, result);
                        }
                    }
                });
            }
        }
    });
}

extern "C" fn function_constructor_probe() -> u64 {
    MoltObject::none().bits()
}

#[test]
fn function_type_new_descriptor_requires_positional_subtype_and_lambda_alias() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = builtin_classes(py).function;
            let new_name = crate::attr_name_bits_from_bytes(py, b"__new__").unwrap();
            let constructor = crate::molt_get_attr_name(class, new_name);
            assert!(!exception_pending(py));
            for (receiver, expected) in [
                (None, "function.__new__(): not enough arguments"),
                (
                    Some(MoltObject::from_int(7).bits()),
                    "function.__new__(X): X is not a type object (int)",
                ),
                (
                    Some(builtin_classes(py).dict),
                    "function.__new__(dict): dict is not a subtype of function",
                ),
            ] {
                let builder = molt_callargs_new(1, 0);
                if let Some(receiver) = receiver {
                    molt_callargs_push_pos(builder, receiver);
                }
                let result = molt_call_bind(constructor, builder);
                assert!(obj_from_bits(result).is_none());
                let error = crate::exception_last_bits_noinc(py).expect("receiver failure");
                assert_eq!(
                    crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                    expected
                );
                crate::molt_exception_clear();
                dec_ref_bits(py, result);
            }
            let builder = molt_callargs_new(0, 1);
            let receiver_name = crate::attr_name_bits_from_bytes(py, b"type").unwrap();
            molt_callargs_push_kw(builder, receiver_name, class);
            let result = molt_call_bind(constructor, builder);
            let error = crate::exception_last_bits_noinc(py).expect("receiver is positional-only");
            assert_eq!(
                crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                "function.__new__(): not enough arguments"
            );
            crate::molt_exception_clear();
            dec_ref_bits(py, result);
            dec_ref_bits(py, receiver_name);

            let payload = crate::builtins::types::molt_types_bootstrap();
            assert!(!exception_pending(py));
            let dictionary = obj_from_bits(payload).as_ptr().unwrap();
            for spelling in [b"FunctionType".as_slice(), b"LambdaType".as_slice()] {
                let key = crate::attr_name_bits_from_bytes(py, spelling).unwrap();
                assert_eq!(dict_get_in_place(py, dictionary, key).unwrap(), Some(class));
                dec_ref_bits(py, key);
            }
            for bits in [payload, constructor, new_name] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn function_type_constructor_binds_named_defaults_and_retains_identity_by_target() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let source = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(
                    function_constructor_probe as *const (),
                ),
                0,
            );
            assert!(!source.is_null());
            let code = crate::object::layout::ensure_function_code_bits(py, source);
            let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let name = crate::attr_name_bits_from_bytes(py, b"renamed").unwrap();
            let defaults = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let closure = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let kwdefaults = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            for minor in [12, 13, 14, 15] {
                transaction.with_target_python_minor(py, minor, || {
                    for direct_new in [false, true] {
                        let keywords = [
                            (b"code".as_slice(), code),
                            (b"globals", globals),
                            (b"name", name),
                            (b"argdefs", defaults),
                            (b"closure", closure),
                            (b"kwdefaults", kwdefaults),
                        ];
                        let count = if minor == 12 { 5 } else { 6 };
                        for positional_count in [0, 2, count] {
                            let positional: Vec<_> = keywords[..positional_count]
                                .iter()
                                .map(|(_, value)| *value)
                                .collect();
                            let result = function_constructor_call(
                                py,
                                direct_new,
                                &positional,
                                &keywords[positional_count..count],
                            );
                            assert!(!exception_pending(py));
                            let function = obj_from_bits(result).as_ptr().unwrap();
                            assert_eq!(object_type_id(function), TYPE_ID_FUNCTION);
                            assert_eq!(crate::function_code_bits(function), code);
                            assert_eq!(crate::function_globals_bits(function), globals);
                            assert_eq!(crate::function_closure_bits(function), closure);
                            use crate::object::function_metadata::FunctionMetadataField as Field;
                            assert_eq!(Field::Name.load(function), Some(name));
                            assert_eq!(Field::Defaults.load(function), Some(defaults));
                            assert_eq!(
                                Field::KeywordDefaults.load(function),
                                Some(if minor == 12 {
                                    MoltObject::none().bits()
                                } else {
                                    kwdefaults
                                })
                            );
                            assert_eq!(
                                crate::function_dict_bits(function),
                                0,
                                "typed constructor metadata must preserve the lazy public dictionary"
                            );
                            dec_ref_bits(py, result);
                        }
                        if minor >= 13 {
                            let invalid_closure = MoltObject::from_ptr(alloc_tuple(
                                py,
                                &[MoltObject::from_int(7).bits()],
                            )).bits();
                            let result = function_constructor_call(
                                py,
                                direct_new,
                                &[code, globals],
                                &[
                                    (b"closure", invalid_closure),
                                    (b"kwdefaults", MoltObject::from_int(7).bits()),
                                ],
                            );
                            assert!(obj_from_bits(result).is_none());
                            let error = crate::exception_last_bits_noinc(py).expect("invalid kwdefaults");
                            assert_eq!(
                                crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                                "arg 6 (kwdefaults) must be None or dict",
                                "kwdefaults type admission precedes closure length/cell validation"
                            );
                            crate::molt_exception_clear();
                            dec_ref_bits(py, invalid_closure);
                            dec_ref_bits(py, result);
                        }
                    }
                });
            }
            for bits in [
                MoltObject::from_ptr(source).bits(),
                globals,
                name,
                defaults,
                closure,
                kwdefaults,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn function_type_constructor_retains_dict_subclass_keyword_defaults_for_invocation() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let flag = crate::attr_name_bits_from_bytes(py, b"flag").unwrap();
            let positional_names = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let keyword_names = MoltObject::from_ptr(alloc_tuple(py, &[flag])).bits();
            let source = super::test_support::metadata_function(
                py,
                super::test_support::compiled_identity_returns_owned_arg as *const (),
                1,
                &[
                    (b"__molt_arg_names__", positional_names),
                    (b"__molt_kwonly_names__", keyword_names),
                ],
                false,
            );
            let code = crate::object::layout::ensure_function_code_bits(
                py,
                obj_from_bits(source).as_ptr().unwrap(),
            );
            let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let classes = builtin_classes(py);
            let subclass = empty_type(
                py,
                b"FunctionKeywordDefaults",
                classes.type_obj,
                &[classes.dict],
            );
            let mapping = molt_call_bind(subclass, molt_callargs_new(0, 0));
            assert!(!exception_pending(py));
            let mapping_ptr = obj_from_bits(mapping).as_ptr().unwrap();
            assert_eq!(object_type_id(mapping_ptr), TYPE_ID_OBJECT);
            let storage = crate::object::ops::dict_backing_bits(py, mapping)
                .unwrap()
                .unwrap();
            let storage_ptr = obj_from_bits(storage).as_ptr().unwrap();
            let baseline = (*crate::header_from_obj_ptr(mapping_ptr)).ref_count_snapshot();
            for minor in [13, 14, 15] {
                transaction.with_target_python_minor(py, minor, || {
                    for direct_new in [false, true] {
                        assert!(dict_set_in_place(
                            py,
                            storage_ptr,
                            flag,
                            MoltObject::from_int(73).bits()
                        ));
                        let result = function_constructor_call(
                            py,
                            direct_new,
                            &[code, globals],
                            &[(b"kwdefaults", mapping)],
                        );
                        assert!(!exception_pending(py));
                        let function = obj_from_bits(result).as_ptr().unwrap();
                        use crate::object::function_metadata::FunctionMetadataField as Field;
                        assert_eq!(Field::KeywordDefaults.load(function), Some(mapping));
                        assert_eq!(
                            (*crate::header_from_obj_ptr(mapping_ptr)).ref_count_snapshot(),
                            baseline + 1
                        );
                        let initial = molt_call_bind(result, molt_callargs_new(0, 0));
                        assert!(!exception_pending(py));
                        assert_eq!(initial, MoltObject::from_int(73).bits());
                        assert!(dict_set_in_place(
                            py,
                            storage_ptr,
                            flag,
                            MoltObject::from_int(101).bits()
                        ));
                        let changed = molt_call_bind(result, molt_callargs_new(0, 0));
                        assert!(!exception_pending(py));
                        assert_eq!(
                            changed,
                            MoltObject::from_int(101).bits(),
                            "invocation reads the retained mapping's current backing storage"
                        );
                        for bits in [initial, changed, result] {
                            dec_ref_bits(py, bits);
                        }
                        assert_eq!(
                            (*crate::header_from_obj_ptr(mapping_ptr)).ref_count_snapshot(),
                            baseline
                        );
                    }
                });
            }
            for bits in [
                flag,
                positional_names,
                keyword_names,
                source,
                globals,
                subclass,
                mapping,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn type_call_dispatch_publishes_cells_for_direct_and_metaclass_winner_paths() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for winner in [false, true] {
                let type_bits = builtin_classes(_py).type_obj;
                let meta = if winner {
                    empty_type(_py, b"CellMeta", type_bits, &[type_bits])
                } else {
                    type_bits
                };
                let base = empty_type(_py, b"CellBase", meta, &[]);
                let name = crate::attr_name_bits_from_bytes(_py, b"CellSubject").unwrap();
                let class_key = crate::attr_name_bits_from_bytes(_py, b"__classcell__").unwrap();
                let dict_key = crate::attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
                let value_key = crate::attr_name_bits_from_bytes(_py, b"value").unwrap();
                let prepared =
                    alloc_dict_with_pairs(_py, &[value_key, MoltObject::from_int(1).bits()]);
                let prepared_bits = MoltObject::from_ptr(prepared).bits();
                let class_cell = crate::object::cells::alloc_cell(_py, MoltObject::none().bits());
                let dict_cell = crate::object::cells::alloc_cell(_py, prepared_bits);
                let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
                let dict_cell_bits = MoltObject::from_ptr(dict_cell).bits();
                assert!(crate::dict_set_in_place(
                    _py,
                    prepared,
                    class_key,
                    class_cell_bits
                ));
                assert!(crate::dict_set_in_place(
                    _py,
                    prepared,
                    dict_key,
                    dict_cell_bits
                ));
                let bases = alloc_tuple(_py, &[base]);
                let bases_bits = MoltObject::from_ptr(bases).bits();
                let class_bits = call_type(_py, name, bases_bits, prepared_bits);
                assert!(!exception_pending(_py));
                let class = obj_from_bits(class_bits).as_ptr().unwrap();
                assert_eq!(crate::object_class_bits(class), meta);
                let copied_bits = crate::class_dict_bits(class);
                let copied = obj_from_bits(copied_bits).as_ptr().unwrap();
                assert_ne!(copied_bits, prepared_bits);
                assert_eq!(
                    crate::object::cells::cell_value_bits(class_cell),
                    class_bits
                );
                assert_eq!(
                    crate::object::cells::cell_value_bits(dict_cell),
                    copied_bits
                );
                for key in [class_key, dict_key] {
                    assert_eq!(
                        dict_get_in_place(_py, copied, key).expect("dictionary lookup succeeds"),
                        None
                    );
                    assert!(
                        dict_get_in_place(_py, prepared, key)
                            .expect("dictionary lookup succeeds")
                            .is_some(),
                        "type construction must not mutate the supplied namespace"
                    );
                }
                assert!(crate::dict_set_in_place(
                    _py,
                    prepared,
                    value_key,
                    MoltObject::from_int(2).bits()
                ));
                assert_eq!(
                    dict_get_in_place(_py, copied, value_key).expect("dictionary lookup succeeds"),
                    Some(MoltObject::from_int(1).bits())
                );
                assert!(crate::dict_set_in_place(
                    _py,
                    copied,
                    value_key,
                    MoltObject::from_int(3).bits()
                ));
                assert_eq!(
                    dict_get_in_place(_py, copied, value_key).expect("dictionary lookup succeeds"),
                    Some(MoltObject::from_int(3).bits())
                );
                // Drop the caller's class reference first: the class cell still owns it.
                dec_ref_bits(_py, class_bits);
                assert_eq!(crate::object_type_id(class), TYPE_ID_TYPE);
                for bits in [
                    prepared_bits,
                    class_cell_bits,
                    dict_cell_bits,
                    bases_bits,
                    name,
                    class_key,
                    dict_key,
                    value_key,
                    base,
                ] {
                    dec_ref_bits(_py, bits);
                }
                if winner {
                    dec_ref_bits(_py, meta);
                }
                assert!(!exception_pending(_py));
            }
        }
    });
}

#[test]
fn type_call_invalid_cell_releases_copied_namespace_payload() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for cell_name in [b"__classcell__".as_slice(), b"__classdictcell__".as_slice()] {
                let name = crate::attr_name_bits_from_bytes(_py, b"RejectedCell").unwrap();
                let key = crate::attr_name_bits_from_bytes(_py, cell_name).unwrap();
                let payload_key = crate::attr_name_bits_from_bytes(_py, b"payload").unwrap();
                let payload = crate::alloc_list(_py, &[]);
                let payload_bits = MoltObject::from_ptr(payload).bits();
                let ns = alloc_dict_with_pairs(
                    _py,
                    &[
                        key,
                        MoltObject::from_int(1).bits(),
                        payload_key,
                        payload_bits,
                    ],
                );
                let ns_bits = MoltObject::from_ptr(ns).bits();
                let before = (*crate::header_from_obj_ptr(payload)).ref_count_snapshot();
                let result = call_type(_py, name, MoltObject::none().bits(), ns_bits);
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(_py));
                crate::molt_exception_clear();
                assert_eq!(
                    (*crate::header_from_obj_ptr(payload)).ref_count_snapshot(),
                    before
                );
                assert_eq!(
                    dict_get_in_place(_py, ns, key).expect("dictionary lookup succeeds"),
                    Some(MoltObject::from_int(1).bits())
                );
                for bits in [ns_bits, payload_bits, name, key, payload_key] {
                    dec_ref_bits(_py, bits);
                }
            }
        }
    });
}

#[test]
fn type_invalid_qualname_precedes_cell_validation_and_publication() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for cell_name in [b"__classcell__".as_slice(), b"__classdictcell__".as_slice()] {
                for invalid_cell in [false, true] {
                    let name = crate::attr_name_bits_from_bytes(_py, b"InvalidQualname").unwrap();
                    let qualname = crate::attr_name_bits_from_bytes(_py, b"__qualname__").unwrap();
                    let cell_key = crate::attr_name_bits_from_bytes(_py, cell_name).unwrap();
                    let marker = MoltObject::from_int(7).bits();
                    let cell = crate::object::cells::alloc_cell(_py, marker);
                    assert!(!cell.is_null());
                    let cell_bits = MoltObject::from_ptr(cell).bits();
                    let ns = alloc_dict_with_pairs(
                        _py,
                        &[
                            qualname,
                            marker,
                            cell_key,
                            if invalid_cell { marker } else { cell_bits },
                        ],
                    );
                    let bases = alloc_tuple(_py, &[]);
                    assert!(!ns.is_null() && !bases.is_null());
                    let ns_bits = MoltObject::from_ptr(ns).bits();
                    let bases_bits = MoltObject::from_ptr(bases).bits();
                    let result = call_type(_py, name, bases_bits, ns_bits);
                    assert!(obj_from_bits(result).is_none());
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        error,
                        "TypeError"
                    ));
                    let message = crate::format_exception_message(
                        _py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    assert!(
                        message.contains("type __qualname__ must be a str, not int"),
                        "{message}"
                    );
                    crate::molt_exception_clear();
                    assert_eq!(crate::object::cells::cell_value_bits(cell), marker);
                    assert_eq!(
                        dict_get_in_place(_py, ns, qualname).expect("dictionary lookup succeeds"),
                        Some(marker)
                    );
                    for bits in [
                        result, error, ns_bits, bases_bits, cell_bits, name, qualname, cell_key,
                    ] {
                        dec_ref_bits(_py, bits);
                    }
                    assert!(!exception_pending(_py));
                }
            }
        }
    });
}

extern "C" fn record_set_name(_descriptor: u64, owner: u64, name: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let name_text = crate::string_obj_to_owned(obj_from_bits(name)).unwrap();
            let (prepared, class_cell, namespace_cell) = CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.events.push(format!("set:{name_text}"));
                (state.prepared, state.class_cell, state.namespace_cell)
            });
            let owner_dict = crate::class_dict_bits(obj_from_bits(owner).as_ptr().unwrap());
            assert_eq!(
                crate::object::cells::cell_value_bits(obj_from_bits(class_cell).as_ptr().unwrap()),
                owner
            );
            assert_eq!(
                crate::object::cells::cell_value_bits(
                    obj_from_bits(namespace_cell).as_ptr().unwrap()
                ),
                owner_dict
            );
            if name_text == "first" {
                let second = crate::attr_name_bits_from_bytes(_py, b"second").unwrap();
                crate::dict_del_in_place(_py, obj_from_bits(owner_dict).as_ptr().unwrap(), second)
                    .expect("owner namespace deletion succeeds");
                crate::dict_del_in_place(_py, obj_from_bits(prepared).as_ptr().unwrap(), second)
                    .expect("prepared namespace deletion succeeds");
                dec_ref_bits(_py, second);
            }
            MoltObject::none().bits()
        }
    }) as i64
}

extern "C" fn record_left_hook(_owner: u64) -> i64 {
    CALLBACKS.with(|state| state.borrow_mut().events.push("init:left".to_string()));
    MoltObject::none().bits() as i64
}

#[test]
fn object_class_hook_binds_lookup_owner_through_every_descriptor_surface() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let builtins = builtin_classes(py);
            let base = empty_type(py, b"HookBase", builtins.type_obj, &[]);
            let child = empty_type(py, b"HookChild", builtins.type_obj, &[base]);
            let instance =
                crate::alloc_instance_for_class(py, obj_from_bits(child).as_ptr().unwrap());
            let super_ptr = crate::object::builders::alloc_super_obj(py, child, child, child);
            assert!(!super_ptr.is_null());
            let super_bits = MoltObject::from_ptr(super_ptr).bits();
            let key = crate::attr_name_bits_from_bytes(py, b"__init_subclass__").unwrap();
            let descriptor = crate::builtins::methods::object_method_bits(py, "__init_subclass__")
                .expect("method lookup succeeds")
                .unwrap();
            let descriptor_ptr = obj_from_bits(descriptor).as_ptr().unwrap();
            // Native descriptor identity is its declared Python class, not a
            // managed classmethod wrapper around the portable function payload.
            assert_eq!(object_type_id(descriptor_ptr), TYPE_ID_FUNCTION);
            assert_eq!(
                crate::object_class_bits(descriptor_ptr),
                builtins.classmethod_descriptor
            );
            assert_eq!(
                crate::call::function::function_metadata_bits(py, descriptor_ptr, b"__objclass__"),
                builtins.object
            );
            assert!(
                crate::builtins::attr::descriptor_has_get(py, descriptor)
                    .expect("operation succeeds")
            );
            assert!(
                !crate::builtins::attr::descriptor_is_data(py, descriptor)
                    .expect("operation succeeds")
            );
            assert!(callable_matches_runtime_symbol(
                Some(descriptor),
                fn_key!(molt_object_init_subclass)
            ));
            // Direct invocation and materialized lookup share the same native
            // descriptor. The raw callable does not invent a missing receiver.
            for (arguments, rejects) in [
                (&[][..], true),
                (&[instance][..], true),
                (&[child][..], false),
            ] {
                let result =
                    crate::call::function::call_function_obj_vec(py, descriptor, arguments);
                assert_eq!(exception_pending(py), rejects);
                if exception_pending(py) {
                    crate::molt_exception_clear();
                } else {
                    assert!(obj_from_bits(result).is_none());
                }
                dec_ref_bits(py, result);
            }
            for (lookup, owner) in [
                (builtins.object, builtins.object),
                (base, base),
                (child, child),
                (instance, child),
                (super_bits, child),
            ] {
                let hook = crate::molt_get_attr_name(lookup, key);
                assert!(!exception_pending(py));
                let hook_ptr = obj_from_bits(hook).as_ptr().unwrap();
                assert_eq!(object_type_id(hook_ptr), TYPE_ID_BOUND_METHOD);
                assert_eq!(
                    crate::object_class_bits(hook_ptr),
                    builtins.builtin_function_or_method
                );
                assert_eq!(bound_method_self_bits(hook_ptr), owner);
                let function = bound_method_func_bits(hook_ptr);
                assert_eq!(
                    function, descriptor,
                    "lookup must retain the published descriptor"
                );
                assert!(callable_matches_runtime_symbol(
                    Some(function),
                    fn_key!(molt_object_init_subclass)
                ));
                let builder = molt_callargs_new(0, 0);
                let result = molt_call_bind(hook, builder);
                assert!(obj_from_bits(result).is_none());
                assert!(!exception_pending(py));
                dec_ref_bits(py, result);
                for keyword in [false, true] {
                    let builder = molt_callargs_new(1, 1);
                    if keyword {
                        molt_callargs_push_kw(builder, key, MoltObject::from_int(1).bits());
                    } else {
                        molt_callargs_push_pos(builder, MoltObject::from_int(1).bits());
                    }
                    let result = molt_call_bind(hook, builder);
                    assert!(
                        exception_pending(py),
                        "class hook must reject extra arguments"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, result);
                }
                dec_ref_bits(py, hook);
            }
            assert!(dispatch_init_subclass_hooks(py, child, &[], &[]));
            assert!(!dispatch_init_subclass_hooks(
                py,
                child,
                &[key],
                &[MoltObject::from_int(1).bits()]
            ));
            crate::molt_exception_clear();
            for bits in [key, super_bits, instance, child, base] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}
extern "C" fn record_right_hook(_owner: u64) -> i64 {
    CALLBACKS.with(|state| state.borrow_mut().events.push("init:right".to_string()));
    MoltObject::none().bits() as i64
}

#[test]
fn type_callbacks_use_owned_snapshot_then_one_inherited_hook() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            CALLBACKS.with(|state| *state.borrow_mut() = CallbackState::default());
            let ty = builtin_classes(_py).type_obj;
            let left = empty_type(_py, b"Left", ty, &[]);
            let right = empty_type(_py, b"Right", ty, &[]);
            let descriptor_type = empty_type(_py, b"Descriptor", ty, &[]);
            let hook_key = crate::attr_name_bits_from_bytes(_py, b"__init_subclass__").unwrap();
            let set_key = crate::attr_name_bits_from_bytes(_py, b"__set_name__").unwrap();
            for (owner, function) in [
                (left, record_left_hook as *const ()),
                (right, record_right_hook as *const ()),
            ] {
                let ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(function),
                    1,
                );
                assert!(!ptr.is_null());
                let bits = MoltObject::from_ptr(ptr).bits();
                // type.__new__ installs a plain __init_subclass__ function as a
                // classmethod. This fixture mutates an already-created base
                // dictionary directly, so preserve that namespace contract
                // explicitly before exercising class-mode super dispatch.
                let hook = crate::molt_classmethod_new(bits);
                assert!(!obj_from_bits(hook).is_none());
                let dictionary = crate::class_dict_bits(obj_from_bits(owner).as_ptr().unwrap());
                assert!(crate::dict_set_in_place(
                    _py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    hook_key,
                    hook,
                ));
                dec_ref_bits(_py, hook);
                dec_ref_bits(_py, bits);
            }
            let ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(record_set_name as *const ()),
                3,
            );
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            let dictionary =
                crate::class_dict_bits(obj_from_bits(descriptor_type).as_ptr().unwrap());
            assert!(crate::dict_set_in_place(
                _py,
                obj_from_bits(dictionary).as_ptr().unwrap(),
                set_key,
                bits,
            ));
            dec_ref_bits(_py, bits);
            let first = crate::alloc_instance_for_class(
                _py,
                obj_from_bits(descriptor_type).as_ptr().unwrap(),
            );
            let second = crate::alloc_instance_for_class(
                _py,
                obj_from_bits(descriptor_type).as_ptr().unwrap(),
            );
            let first_key = crate::attr_name_bits_from_bytes(_py, b"first").unwrap();
            let second_key = crate::attr_name_bits_from_bytes(_py, b"second").unwrap();
            let ns = alloc_dict_with_pairs(_py, &[first_key, first, second_key, second]);
            let ns_bits = MoltObject::from_ptr(ns).bits();
            // The prepared mapping is now the only owner of each descriptor.
            dec_ref_bits(_py, first);
            dec_ref_bits(_py, second);
            let class_cell = crate::object::cells::alloc_cell(_py, MoltObject::none().bits());
            let namespace_cell = crate::object::cells::alloc_cell(_py, ns_bits);
            let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
            let namespace_cell_bits = MoltObject::from_ptr(namespace_cell).bits();
            let class_key = crate::attr_name_bits_from_bytes(_py, b"__classcell__").unwrap();
            let dict_key = crate::attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
            assert!(crate::dict_set_in_place(
                _py,
                ns,
                class_key,
                class_cell_bits
            ));
            assert!(crate::dict_set_in_place(
                _py,
                ns,
                dict_key,
                namespace_cell_bits
            ));
            CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.prepared = ns_bits;
                state.class_cell = class_cell_bits;
                state.namespace_cell = namespace_cell_bits;
            });
            let name = crate::attr_name_bits_from_bytes(_py, b"Subject").unwrap();
            let bases = alloc_tuple(_py, &[left, right]);
            let bases_bits = MoltObject::from_ptr(bases).bits();
            let result = call_type(_py, name, bases_bits, ns_bits);
            assert!(!exception_pending(_py));
            CALLBACKS.with(|state| {
                assert_eq!(
                    state.borrow().events,
                    ["set:first", "set:second", "init:left"]
                );
                *state.borrow_mut() = CallbackState::default();
            });
            for bits in [
                result,
                ns_bits,
                class_cell_bits,
                namespace_cell_bits,
                name,
                bases_bits,
                first_key,
                second_key,
                class_key,
                dict_key,
                hook_key,
                set_key,
                left,
                right,
                descriptor_type,
            ] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}
