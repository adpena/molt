use super::exceptions::luau_tir_roundtrip_function;
use super::*;

#[test]
fn compile_checked_rejects_canonical_void_and_value_externs_before_emission() {
    let declarations = [
        FunctionIR {
            name: "stdlib_void_helper".to_string(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            is_extern: true,
            ..FunctionIR::default()
        },
        FunctionIR {
            name: "stdlib_value_helper".to_string(),
            ops: vec![
                OpIR {
                    kind: "missing".to_string(),
                    out: Some(crate::ir::EXTERN_SIGNATURE_RETURN_VALUE.to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec![crate::ir::EXTERN_SIGNATURE_RETURN_VALUE.to_string()]),
                    ..OpIR::default()
                },
            ],
            is_extern: true,
            ..FunctionIR::default()
        },
    ];

    for declaration in declarations {
        declaration
            .extern_signature()
            .expect("test input must be a canonical extern declaration");
        let expected = format!(
            "luau backend cannot compile extern function `{}`: the luau target has no extern provider/linkage ABI",
            declaration.name
        );
        let mut backend = LuauBackend::new();
        let error = backend
            .compile_checked(&SimpleIR {
                functions: vec![declaration.clone()],
                profile: None,
            })
            .expect_err("Luau must reject externs without a provider/linkage ABI");

        assert_eq!(error, expected);
        assert!(
            backend.output.is_empty(),
            "extern rejection must happen before any Luau source is assembled"
        );
        assert!(backend.unsupported_ops.is_empty());

        let mut pipeline_backend = LuauBackend::new();
        let pipeline_error = pipeline_backend
            .compile_via_ir(&SimpleIR {
                functions: vec![declaration],
                profile: None,
            })
            .expect_err("the Luau IR pipeline must use the checked compile boundary");
        assert_eq!(pipeline_error, expected);
        assert!(
            pipeline_backend.output.is_empty(),
            "the Luau IR pipeline must reject externs before source assembly"
        );
        assert!(pipeline_backend.unsupported_ops.is_empty());
    }
}

#[test]
fn test_sanitize_ident() {
    assert_eq!(sanitize_ident("foo"), "foo");
    assert_eq!(sanitize_ident("my.attr"), "_m_user_6d792e61747472");
    assert_eq!(sanitize_ident("and"), "_m_user_616e64");
    assert_eq!(sanitize_ident("v0"), "v0");
    assert_eq!(sanitize_ident("molt_equal"), "_m_user_6d6f6c745f657175616c");
    assert_eq!(
        sanitize_ident("_m_user_616e64"),
        "_m_user_5f6d5f757365725f363136653634"
    );
    let collision_family = ["a.b", "a-b", "a b", "a_b"];
    let mapped = collision_family.map(sanitize_ident);
    assert_eq!(mapped[3], "a_b");
    assert_eq!(
        mapped
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    let closure_family = ["$molt_closure", "_molt_closure", "__molt_closure"];
    assert_eq!(
        closure_family
            .map(sanitize_ident)
            .into_iter()
            .collect::<BTreeSet<_>>()
            .len(),
        closure_family.len()
    );
}

#[test]
fn definitions_and_references_share_injective_user_and_helper_namespaces() {
    let adversarial_params = [
        "a-b",
        "a_b",
        "and",
        "__molt_frame_context",
        "molt_bool",
        "_m_user_612d62",
    ];
    let mut framed_ops = vec![OpIR {
        kind: "trace_enter_slot".to_string(),
        value: Some(1),
        ..OpIR::default()
    }];
    framed_ops.push(OpIR {
        kind: "tuple_new".to_string(),
        args: Some(
            adversarial_params
                .iter()
                .map(|name| name.to_string())
                .collect(),
        ),
        out: Some("result".to_string()),
        ..OpIR::default()
    });
    framed_ops.extend([
        OpIR {
            kind: "trace_exit".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret".to_string(),
            args: Some(vec!["result".to_string()]),
            ..OpIR::default()
        },
    ]);
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "a-b".to_string(),
                params: adversarial_params
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
                execution_context: ExecutionContextPolicy::Local,
                ops: framed_ops,
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "a_b".to_string(),
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    let mapped = adversarial_params.map(sanitize_ident);
    assert_eq!(mapped.iter().collect::<BTreeSet<_>>().len(), mapped.len());
    for ident in mapped {
        assert!(
            source.contains(&format!("{ident}: any")),
            "{ident}: {source}"
        );
        assert!(source.contains(&ident), "{ident}: {source}");
    }
    assert!(source.contains("local _m_user_612d62"), "{source}");
    assert!(source.contains("local a_b"), "{source}");
    assert!(source.contains("local __molt_frame_context"), "{source}");
}

#[test]
fn direct_caller_observes_returned_tuple_as_one_object() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(1),
                        out: Some("left".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(2),
                        out: Some("right".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("make_pair".to_string()),
                        args: Some(vec!["left".to_string(), "right".to_string()]),
                        out: Some("pair".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "make_pair".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                ops: vec![
                    OpIR {
                        kind: "tuple_new".to_string(),
                        args: Some(vec!["left".to_string(), "right".to_string()]),
                        out: Some("pair".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["pair".to_string()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };

    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    assert!(source.contains("return pair"), "{source}");
    assert!(
        source.contains("local pair = make_pair(left, right)"),
        "{source}"
    );
    assert!(!source.contains("return table.unpack(pair"), "{source}");
}

#[test]
fn compiler_entrypoint_is_an_explicit_abi_symbol_kind() {
    assert_eq!(
        classify_function_symbol("molt_main"),
        LuauFunctionSymbol::CompilerEntrypoint
    );
    assert_eq!(
        classify_function_symbol("__main____molt_main"),
        LuauFunctionSymbol::User("__main____molt_main")
    );

    let invalid = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["user_arg".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let error = LuauBackend::new().compile_checked(&invalid).unwrap_err();
    assert!(error.contains("compiler ABI entrypoint"), "{error}");

    let valid = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
            FunctionIR {
                name: "__main____molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            },
        ],
        profile: None,
    };
    let source = LuauBackend::new().compile_checked(&valid).unwrap();
    assert!(source.contains("local molt_main"), "{source}");
    assert!(
        source.contains("local _m_user_5f5f6d61696e5f5f5f5f6d6f6c745f6d61696e"),
        "{source}"
    );
}

#[test]
fn noncanonical_string_labels_cannot_bypass_logical_label_validation() {
    for label in ["a-b", "a.b", "label_1"] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_label".into(),
                ops: vec![
                    OpIR {
                        kind: "jump".into(),
                        s_value: Some(label.into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".into(),
                        s_value: Some(label.into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let error = backend
            .compile_checked(&ir)
            .expect_err("string aliases cannot create a second label namespace");
        assert!(
            error.contains("malformed-label-reference") && error.contains("integer value"),
            "{error}"
        );
        assert!(
            backend.output.is_empty(),
            "graph rejection must precede source emission"
        );
    }
}

#[test]
fn test_escape_luau_string() {
    assert_eq!(escape_luau_string("hello"), "hello");
    assert_eq!(escape_luau_string("say \"hi\""), "say \\\"hi\\\"");
    assert_eq!(escape_luau_string("a\nb"), "a\\nb");
}

#[test]
fn test_empty_ir() {
    let ir = SimpleIR {
        functions: vec![],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(output.contains("--!strict"));
    assert!(output.contains("molt_main"));
}

#[test]
fn deferred_annotation_functions_are_emitted_with_their_real_body() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "module__C____annotate__".to_string(),
            params: vec!["format".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("annotation-result".to_string()),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile(&ir);

    assert!(source.contains("module__C____annotate__"));
    assert!(source.contains("annotation-result"));
}

#[test]
fn unpack_sequence_uses_exact_arity_runtime_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unpack_exact".to_string(),
            params: vec!["seq".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    value: Some(2),
                    args: Some(vec![
                        "seq".to_string(),
                        "left".to_string(),
                        "right".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["left".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);

    assert!(source.contains("local function molt_unpack_sequence"));
    assert!(source.contains("if actual < expected then"));
    assert!(source.contains("if actual > expected then break end"));
    assert!(source.contains("molt_unpack_sequence(seq, 2, \"auto\")"));
    assert!(source.contains("while actual <= expected do"));
    assert!(source.contains("for _, codepoint in utf8.codes(obj) do"));
    assert!(source.contains("local actual = molt_dict_len(mapping)"));
    assert!(source.contains("molt_dict_view_snapshot(molt_dict_keys(mapping))"));
    assert!(!source.contains("for key in pairs(mapping) do"));
    assert!(source.contains("for value in iterable do"));
    assert!(source.contains("local packed = rawget(sequence, molt_sequence_length_key)"));
    assert!(!source.contains("local actual = #obj"));
    assert!(!source.contains("local left = seq[1]"));
}

#[test]
fn unpack_sequence_preserves_none_holes_with_packed_sequence_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unpack_none".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("none_value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(1),
                    out: Some("seven".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "build_list".to_string(),
                    args: Some(vec!["none_value".to_string(), "seven".to_string()]),
                    out: Some("seq".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    value: Some(2),
                    args: Some(vec![
                        "seq".to_string(),
                        "left".to_string(),
                        "right".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);

    assert!(source.contains("molt_pack_list(none_value, seven)"));
    assert!(source.contains("molt_unpack_sequence(seq, 2, \"sequence\")"));
    assert!(source.contains("rawget(sequence, i)"));
    assert!(source.contains("rawset(items, molt_sequence_length_key, actual)"));
}

#[test]
fn unpack_mapping_keeps_user_n_key_distinct_from_sequence_metadata() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unpack_mapping_n".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("n".to_string()),
                    out: Some("key".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "build_dict".to_string(),
                    args: Some(vec!["key".to_string(), "value".to_string()]),
                    out: Some("mapping".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    value: Some(1),
                    args: Some(vec!["mapping".to_string(), "only".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);

    assert!(source.contains("molt_unpack_sequence(mapping, 1, \"mapping\")"));
    assert!(source.contains("local mapping: {[any]: any} = molt_dict_new()"));
    assert!(source.contains("molt_dict_set(mapping, key, value)"));
    assert!(source.contains("molt_dict_view_snapshot(molt_dict_keys(mapping))"));
    assert!(!source.contains("for key in pairs(mapping) do"));
    assert!(!source.contains("if key ~= \"n\""));
    assert!(source.contains("local molt_sequence_length_key = {}"));
    assert!(source.contains("local molt_dict_metadata = setmetatable({}, {__mode = \"k\"})"));
    assert!(!source.contains("rawget(obj, \"n\")"));
}

#[test]
fn ordered_dict_authority_is_complete_deterministic_and_collision_free() {
    let ops = vec![
        OpIR {
            kind: "dict_new".to_string(),
            out: Some("d".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_set".to_string(),
            args: Some(vec![
                "d".to_string(),
                "key".to_string(),
                "value".to_string(),
            ]),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_setdefault".to_string(),
            args: Some(vec![
                "d".to_string(),
                "other".to_string(),
                "value".to_string(),
            ]),
            out: Some("defaulted".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_pop".to_string(),
            args: Some(vec![
                "d".to_string(),
                "other".to_string(),
                "none".to_string(),
            ]),
            out: Some("popped".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_copy".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("copy".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_update".to_string(),
            args: Some(vec!["copy".to_string(), "d".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "eq".to_string(),
            args: Some(vec!["copy".to_string(), "d".to_string()]),
            out: Some("equal".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "repr_from_obj".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("rendered".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_keys".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("keys".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_values".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("values".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_items".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("items".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_popitem".to_string(),
            args: Some(vec!["d".to_string()]),
            out: Some("last".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
    ];
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "ordered_dict_surface".to_string(),
            params: vec![
                "key".to_string(),
                "other".to_string(),
                "value".to_string(),
                "none".to_string(),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops,
        }],
        profile: None,
    };

    let first = LuauBackend::new().compile(&ir);
    let second = LuauBackend::new().compile(&ir);
    assert_eq!(
        first, second,
        "identical IR must compile byte-for-byte deterministically"
    );
    for required in [
        "local molt_dict_none_key = {}",
        "local molt_dict_none_value = {}",
        "local molt_dict_metadata = setmetatable({}, {__mode = \"k\"})",
        "local function molt_hashed_index_new",
        "molt_dict_set(d, key, value)",
        "molt_dict_setdefault(d, other, value)",
        "molt_dict_pop(d, other, true, none)",
        "molt_dict_copy(d)",
        "molt_dict_update(copy, d)",
        "molt_equal(copy, d)",
        "molt_repr(d)",
        "molt_dict_keys(d)",
        "molt_dict_values(d)",
        "molt_dict_items(d)",
        "molt_dict_popitem(d)",
    ] {
        assert!(
            first.contains(required),
            "missing ordered-dict authority `{required}`:\n{first}"
        );
    }
    assert!(first.contains("if kind == \"boolean\" then value = if value then 1 else 0"));
    assert!(first.contains("if value ~= value then"));
    assert!(first.contains("unhashable container type"));
    assert!(first.contains("if molt_dict_is_ordered(x) then return \"{...}\" end"));
    assert!(first.contains("local entry_id = molt_hashed_index_find(metadata, key)"));
    assert!(first.contains("local slot = molt_hashed_index_delete(metadata, entry_id)"));
    assert!(!first.contains("for __k, __v in pairs(d)"));
}

#[test]
fn dict_runtime_dependency_slices_do_not_ship_unreferenced_call_or_repr_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dict_only".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("mapping".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);
    assert!(source.contains("local function molt_dict_new"));
    assert!(!source.contains("local function molt_callargs_new"));
    assert!(!source.contains("local function molt_equal"));
    assert!(!source.contains("local function molt_repr_string"));
    assert!(dict_runtime::DICT_CORE_RUNTIME.len() < source.len());
    assert!(
        dict_runtime::CALLARGS_RUNTIME.len() + dict_runtime::EQUALITY_REPR_RUNTIME.len() > 4_000,
        "dependency slicing must avoid a material amount of unreferenced source"
    );
}

#[test]
fn compile_checked_callargs_family_uses_one_builder_invocation_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "invoke".to_string(),
            params: vec![
                "func".to_string(),
                "value".to_string(),
                "star".to_string(),
                "key".to_string(),
                "kwvalue".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("bind_builder".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_pos".to_string(),
                    args: Some(vec!["bind_builder".to_string(), "value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_bind".to_string(),
                    args: Some(vec!["func".to_string(), "bind_builder".to_string()]),
                    out: Some("bound_result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("indirect_builder".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_pos".to_string(),
                    args: Some(vec![
                        "indirect_builder".to_string(),
                        "bound_result".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_expand_star".to_string(),
                    args: Some(vec!["indirect_builder".to_string(), "star".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_kw".to_string(),
                    args: Some(vec![
                        "indirect_builder".to_string(),
                        "key".to_string(),
                        "kwvalue".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("kwstar".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_expand_kwstar".to_string(),
                    args: Some(vec!["indirect_builder".to_string(), "kwstar".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["func".to_string(), "indirect_builder".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("the complete CallArgs family must pass checked Luau admission");
    assert!(source.contains("local bind_builder: {any} = molt_callargs_new()"));
    assert!(source.contains("molt_callargs_push_pos(bind_builder, value)"));
    assert!(source.contains("local bound_result = molt_callargs_invoke(func, bind_builder)"));
    assert!(source.contains("molt_callargs_expand_star(indirect_builder, star)"));
    assert!(source.contains("molt_callargs_push_kw(indirect_builder, key, kwvalue)"));
    assert!(source.contains("molt_callargs_expand_kwstar(indirect_builder, kwstar)"));
    assert!(source.contains("local result = molt_callargs_invoke(func, indirect_builder)"));
    assert!(!source.contains("molt_call_checked(func, indirect_builder)"));
    assert!(source.contains("molt_call_checked = function"));
    assert!(source.contains("local function molt_callargs_expand_kwstar"));
    assert!(!source.contains("local function molt_function_init_metadata_packed"));
    assert!(!source.contains("local function molt_equal"));
    assert!(!source.contains("molt_function_params"));
    assert!(source.contains(
        "local molt_function_metadata: {[any]: any} = setmetatable({}, {__mode = \"k\"})"
    ));
    assert!(source.contains(
        "local molt_func_attrs: {[any]: {[any]: any}} = setmetatable({}, {__mode = \"k\"})"
    ));
    assert!(source.contains("if value == func then molt_func_self_attr else value"));
}

#[test]
fn function_value_runtime_helper_selects_its_complete_helper_group() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("A".to_string()),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("molt_ord".to_string()),
                    args: Some(vec!["value".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("function-value helper dependencies must pass checked Luau admission");
    assert!(source.contains("local result = molt_call_checked(molt_ord, value)"));
    assert!(source.contains("local function molt_ord(ch: any): number"));
    assert!(source.contains("local function molt_str_codepoint_len(s: string): number"));
}

#[test]
fn callable_and_frame_runtime_fragments_pass_shared_source_validation() {
    for (name, source) in [
        ("frames", frame_runtime::FRAME_RUNTIME),
        ("callable frames", frame_runtime::CALLABLE_FRAME_RUNTIME),
        (
            "callable metadata",
            frame_runtime::CALLABLE_METADATA_RUNTIME,
        ),
        ("call arguments", dict_runtime::CALLARGS_RUNTIME),
    ] {
        validate_luau_source(source)
            .unwrap_or_else(|error| panic!("{name} runtime is structurally invalid: {error}"));
    }
}

#[test]
fn compile_checked_rejects_kwstar_without_canonical_ordered_mapping_provenance() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unknown_mapping".to_string(),
            params: vec!["func".to_string(), "mapping".to_string()],
            ops: vec![
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("builder".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_expand_kwstar".to_string(),
                    args: Some(vec!["builder".to_string(), "mapping".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["func".to_string(), "builder".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("generic Python mapping protocol is not implemented by Luau");
    assert!(
        error.contains("`callargs_expand_kwstar`")
            && error.contains("canonical ordered Molt dict")
            && error.contains("keys/getitem mapping protocol")
    );
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn checked_callargs_execute_mixed_arguments_live_defaults_and_bound_closures() {
    let callargs_new = |out: &str| OpIR {
        kind: "callargs_new".to_string(),
        out: Some(out.to_string()),
        ..OpIR::default()
    };
    let call = |kind: &str, func: &str, builder: &str, out: &str| OpIR {
        kind: kind.to_string(),
        args: Some(vec![func.to_string(), builder.to_string()]),
        out: Some(out.to_string()),
        ..OpIR::default()
    };
    let ret = |value: &str| OpIR {
        kind: "ret".to_string(),
        args: Some(vec![value.to_string()]),
        ..OpIR::default()
    };
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "mixed_call".to_string(),
                params: vec![
                    "func".to_string(),
                    "first".to_string(),
                    "star".to_string(),
                    "key".to_string(),
                    "kwvalue".to_string(),
                    "kwstar_value".to_string(),
                ],
                ops: vec![
                    callargs_new("builder"),
                    OpIR {
                        kind: "callargs_push_pos".to_string(),
                        args: Some(vec!["builder".to_string(), "first".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_expand_star".to_string(),
                        args: Some(vec!["builder".to_string(), "star".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_push_kw".to_string(),
                        args: Some(vec![
                            "builder".to_string(),
                            "key".to_string(),
                            "kwvalue".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("e".to_string()),
                        out: Some("kwstar_key".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_new".to_string(),
                        out: Some("kwstar".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_set".to_string(),
                        args: Some(vec![
                            "kwstar".to_string(),
                            "kwstar_key".to_string(),
                            "kwstar_value".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_expand_kwstar".to_string(),
                        args: Some(vec!["builder".to_string(), "kwstar".to_string()]),
                        ..OpIR::default()
                    },
                    call("call_indirect", "func", "builder", "result"),
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "duplicate_keyword_call".to_string(),
                params: vec![
                    "func".to_string(),
                    "key".to_string(),
                    "value".to_string(),
                    "kwstar_value".to_string(),
                ],
                ops: vec![
                    callargs_new("builder"),
                    OpIR {
                        kind: "callargs_push_kw".to_string(),
                        args: Some(vec![
                            "builder".to_string(),
                            "key".to_string(),
                            "value".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_new".to_string(),
                        out: Some("kwstar".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_set".to_string(),
                        args: Some(vec![
                            "kwstar".to_string(),
                            "key".to_string(),
                            "kwstar_value".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_expand_kwstar".to_string(),
                        args: Some(vec!["builder".to_string(), "kwstar".to_string()]),
                        ..OpIR::default()
                    },
                    call("call_indirect", "func", "builder", "result"),
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "unsigned_builtin_direct".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                ops: vec![
                    OpIR {
                        kind: "builtin_func".to_string(),
                        s_value: Some("molt_max_builtin".to_string()),
                        out: Some("builtin".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_func".to_string(),
                        args: Some(vec![
                            "builtin".to_string(),
                            "left".to_string(),
                            "right".to_string(),
                        ]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "unsigned_builtin_indirect".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                ops: vec![
                    OpIR {
                        kind: "builtin_func".to_string(),
                        s_value: Some("molt_max_builtin".to_string()),
                        out: Some("builtin".to_string()),
                        ..OpIR::default()
                    },
                    callargs_new("builder"),
                    OpIR {
                        kind: "callargs_push_pos".to_string(),
                        args: Some(vec!["builder".to_string(), "left".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_push_pos".to_string(),
                        args: Some(vec!["builder".to_string(), "right".to_string()]),
                        ..OpIR::default()
                    },
                    call("call_indirect", "builtin", "builder", "result"),
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "unsigned_builtin_keyword".to_string(),
                params: vec!["key".to_string(), "value".to_string()],
                ops: vec![
                    OpIR {
                        kind: "builtin_func".to_string(),
                        s_value: Some("molt_max_builtin".to_string()),
                        out: Some("builtin".to_string()),
                        ..OpIR::default()
                    },
                    callargs_new("builder"),
                    OpIR {
                        kind: "callargs_push_kw".to_string(),
                        args: Some(vec![
                            "builder".to_string(),
                            "key".to_string(),
                            "value".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    call("call_indirect", "builtin", "builder", "result"),
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "direct_call".to_string(),
                params: vec!["func".to_string()],
                ops: vec![
                    OpIR {
                        kind: "call_func".to_string(),
                        args: Some(vec!["func".to_string()]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "builder_call".to_string(),
                params: vec!["func".to_string()],
                ops: vec![
                    callargs_new("builder"),
                    call("call_bind", "func", "builder", "result"),
                    ret("result"),
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "set_function_defaults".to_string(),
                params: vec!["func".to_string(), "defaults".to_string()],
                ops: vec![
                    OpIR {
                        kind: "set_attr".to_string(),
                        args: Some(vec!["func".to_string(), "defaults".to_string()]),
                        s_value: Some("__defaults__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "set_function_kwdefaults".to_string(),
                params: vec!["func".to_string(), "kwdefaults".to_string()],
                ops: vec![
                    OpIR {
                        kind: "set_attr".to_string(),
                        args: Some(vec!["func".to_string(), "kwdefaults".to_string()]),
                        s_value: Some("__kwdefaults__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let compiled = LuauBackend::new()
        .compile_checked(&ir)
        .expect("checked CallArgs execution fixture must pass target admission");
    assert!(compiled.contains("molt_function_set_builtin(builtin)"));
    assert!(compiled.contains("molt_call_checked(builtin, left, right)"));
    assert!(compiled.contains("molt_callargs_invoke(builtin, builder)"));
    assert_eq!(
        compiled
            .matches("local function molt_function_attr_set")
            .count(),
        1,
        "metadata-aware function mutation must have one emitted authority"
    );
    assert!(!compiled.contains("pcall(molt_iterator_new"));
    let oracle = r#"
local function mixed_target(a, b, c, d, e) return a + b + c + d + e end
molt_function_metadata[mixed_target] = {arg_names=molt_pack_tuple("a", "b", "c", "d", "e"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
local star = molt_pack_list(2, 3)
assert(mixed_call(mixed_target, 1, star, "d", 4, 5) == 15)

local duplicate_ok, duplicate_error = pcall(function()
	return duplicate_keyword_call(mixed_target, "d", 4, 9)
end)
assert(not duplicate_ok and duplicate_error.__type == "TypeError")

local star_builder = molt_callargs_new()
local star_ok, star_error = pcall(function() molt_callargs_expand_star(star_builder, 42) end)
assert(not star_ok and star_error.__type == "TypeError" and star_error.__msg == "object is not a deterministic Python iterable")

assert(unsigned_builtin_direct(6, 7) == 7)
assert(unsigned_builtin_indirect(6, 7) == 7)
local builtin_keyword_ok, builtin_keyword_error = pcall(function()
	return unsigned_builtin_keyword("value", 7)
end)
assert(not builtin_keyword_ok and builtin_keyword_error.__type == "TypeError" and builtin_keyword_error.__msg == "callable does not accept keyword arguments")

local packed_builtin = function(args)
	return rawget(args, 1) + rawget(args, 2)
end
molt_function_metadata[packed_builtin] = {arg_names=molt_pack_tuple("left", "right"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil, is_builtin=true}
assert(molt_call_checked(packed_builtin, 6, 7) == 13)
local builtin_builder = molt_callargs_new()
molt_callargs_push_pos(builtin_builder, 6); molt_callargs_push_pos(builtin_builder, 7)
assert(molt_callargs_invoke(packed_builtin, builtin_builder) == 13)

local zero_arity_builtin = function(args) return molt_sequence_len(args) end
molt_function_metadata[zero_arity_builtin] = {arg_names=molt_pack_tuple(), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil, is_builtin=true}
assert(molt_call_checked(zero_arity_builtin) == 0)
local zero_arity_ok, zero_arity_error = pcall(function() return molt_call_checked(zero_arity_builtin, 1) end)
assert(not zero_arity_ok and zero_arity_error.__type == "TypeError")

local function defaulted(value) return value end
molt_function_metadata[defaulted] = {arg_names=molt_pack_tuple("value"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=molt_pack_tuple(10), kwdefaults=nil}
set_function_defaults(defaulted, molt_pack_tuple(20))
assert(molt_func_attr_get(defaulted, "__defaults__") == molt_function_metadata[defaulted].defaults)
assert(direct_call(defaulted) == 20 and builder_call(defaulted) == 20)
local invalid_defaults_ok, invalid_defaults_error = pcall(function()
	set_function_defaults(defaulted, 1)
end)
assert(not invalid_defaults_ok and invalid_defaults_error.__type == "TypeError")

local function keyword_only(option) return option end
local first_kwdefaults = molt_dict_new(); molt_dict_set(first_kwdefaults, "option", 11)
molt_function_metadata[keyword_only] = {arg_names=molt_pack_tuple(), posonly=0, kwonly=molt_pack_tuple("option"), vararg=nil, varkw=nil, defaults=nil, kwdefaults=first_kwdefaults}
local second_kwdefaults = molt_dict_new(); molt_dict_set(second_kwdefaults, "option", 12)
set_function_kwdefaults(keyword_only, second_kwdefaults)
assert(molt_func_attr_get(keyword_only, "__kwdefaults__") == molt_function_metadata[keyword_only].kwdefaults)
assert(direct_call(keyword_only) == 12 and builder_call(keyword_only) == 12)

local function method(self_value, value) return self_value + value end
molt_function_metadata[method] = {arg_names=molt_pack_tuple("self", "value"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=molt_pack_tuple(7), kwdefaults=nil}
local bound = molt_bound_method_new(method, 30)
assert(molt_function_metadata[bound].bound_func == method and molt_function_metadata[bound].bound_self == 30)
assert(direct_call(bound) == 37 and builder_call(bound) == 37)
set_function_defaults(method, molt_pack_tuple(9))
assert(direct_call(bound) == 39 and builder_call(bound) == 39)
print("luau-callargs-checked-execution-ok")
"#;
    let source = format!("{compiled}\n{oracle}");
    validate_luau_source(&source).expect("checked CallArgs execution source must validate");
    let output = execute_lune_oracle("checked_callargs", &source);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("luau-callargs-checked-execution-ok"),
        "{stdout}"
    );
}

#[test]
fn checked_frontend_callable_metadata_and_code_slots_are_reachable() {
    let const_str = |out: &str, value: &str| OpIR {
        kind: "const_str".to_string(),
        out: Some(out.to_string()),
        s_value: Some(value.to_string()),
        ..OpIR::default()
    };
    let none = |out: &str| OpIR {
        kind: "const_none".to_string(),
        out: Some(out.to_string()),
        ..OpIR::default()
    };
    let ops = vec![
        OpIR {
            kind: "func_new".to_string(),
            s_value: Some("target".to_string()),
            value: Some(2),
            out: Some("function_value".to_string()),
            ..OpIR::default()
        },
        const_str("name", "target"),
        const_str("qualname", "target"),
        const_str("module", "sample"),
        const_str("arg_a", "a"),
        const_str("arg_b", "b"),
        OpIR {
            kind: "tuple_new".to_string(),
            args: Some(vec!["arg_a".to_string(), "arg_b".to_string()]),
            out: Some("arg_names".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_float".to_string(),
            f_value: Some(0.0),
            out: Some("posonly".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "tuple_new".to_string(),
            args: Some(vec![]),
            out: Some("kwonly".to_string()),
            ..OpIR::default()
        },
        none("vararg"),
        none("varkw"),
        none("defaults"),
        none("kwdefaults"),
        none("doc"),
        none("bind_kind"),
        const_str("filename", "sample.py"),
        OpIR {
            kind: "const_float".to_string(),
            f_value: Some(1.0),
            out: Some("first_line".to_string()),
            ..OpIR::default()
        },
        none("linetable"),
        OpIR {
            kind: "tuple_new".to_string(),
            args: Some(vec![]),
            out: Some("names".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "code_new".to_string(),
            args: Some(vec![
                "filename".to_string(),
                "name".to_string(),
                "first_line".to_string(),
                "linetable".to_string(),
                "arg_names".to_string(),
                "names".to_string(),
                "posonly".to_string(),
                "posonly".to_string(),
                "posonly".to_string(),
            ]),
            out: Some("code".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_new".to_string(),
            args: Some(vec![]),
            out: Some("globals".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "code_slot_set".to_string(),
            value: Some(3),
            args: Some(vec!["code".to_string(), "globals".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "tuple_new".to_string(),
            args: Some(vec![
                "name".to_string(),
                "qualname".to_string(),
                "module".to_string(),
                "arg_names".to_string(),
                "posonly".to_string(),
                "kwonly".to_string(),
                "vararg".to_string(),
                "varkw".to_string(),
                "defaults".to_string(),
                "kwdefaults".to_string(),
                "doc".to_string(),
                "posonly".to_string(),
                "kwonly".to_string(),
                "kwonly".to_string(),
            ]),
            out: Some("metadata".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "call".to_string(),
            s_value: Some("molt_function_init_metadata_packed".to_string()),
            args: Some(vec![
                "function_value".to_string(),
                "metadata".to_string(),
                "code".to_string(),
                "bind_kind".to_string(),
            ]),
            out: Some("initialized".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "call".to_string(),
            s_value: Some("molt_function_set_defaults".to_string()),
            args: Some(vec![
                "function_value".to_string(),
                "defaults".to_string(),
                "kwdefaults".to_string(),
            ]),
            out: Some("defaults_set".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
    ];
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                ops,
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "target".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["a".to_string()]),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_via_ir(&ir)
        .expect("frontend-shaped callable metadata must pass checked Luau admission");
    assert!(source.contains("local function molt_function_init_metadata_packed"));
    assert!(
        source.contains(
            "molt_call_checked(molt_function_init_metadata_packed, function_value, metadata, code, bind_kind)"
        ),
        "packed metadata call must remain reachable:\n{source}"
    );
    assert!(source.contains(
        "molt_call_checked(molt_function_set_defaults, function_value, defaults, kwdefaults)"
    ));
    assert!(source.contains("local code = {__molt_code=true"));
    assert!(source.contains("molt_code_slots[3] = molt_frame_bind_code(3, code, globals)"));
    assert!(source.contains("local function_value = molt_frame_function_new(target)"));
    assert!(source.contains("molt_frame_function_capture(func, rawget(metadata, 3))"));
    assert!(source.contains("pending.slot == slot.id and pending.code ~= nil"));
    assert!(source.contains("setmetatable({value=captured}, {__mode=\"v\"})"));
    assert!(!source.contains("local function_value = target"));
    assert!(!source.contains("molt_function_params"));
    assert!(!source.contains("[unsupported op:"));
}

#[test]
fn canonical_set_codegen_has_one_deterministic_side_metadata_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "set_surface".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            ops: vec![
                OpIR {
                    kind: "set_new".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("set_value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "frozenset_new".to_string(),
                    args: Some(vec!["right".to_string(), "left".to_string()]),
                    out: Some("frozen".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "contains".to_string(),
                    args: Some(vec!["set_value".to_string(), "left".to_string()]),
                    out: Some("present".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "eq".to_string(),
                    args: Some(vec!["set_value".to_string(), "frozen".to_string()]),
                    out: Some("equal".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "repr_from_obj".to_string(),
                    args: Some(vec!["set_value".to_string()]),
                    out: Some("rendered".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);
    for required in [
        "local molt_set_metadata = setmetatable({}, {__mode = \"k\"})",
        "local set_value = molt_set_new(\"set\")",
        "local frozen = molt_set_new(\"frozenset\")",
        "molt_set_freeze(frozen)",
        "molt_set_contains(set_value, left)",
        "molt_equal(set_value, frozen)",
        "molt_repr(set_value)",
    ] {
        assert!(
            source.contains(required),
            "missing canonical set authority `{required}`:\n{source}"
        );
    }
    assert!(!source.contains("[left] = true"));
    assert!(!source.contains("for value in pairs(set_value)"));
    assert!(!source.contains("table.clear(set_value)"));
}

#[test]
fn checked_dict_codegen_preserves_distinct_str_and_bytes_key_representations() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("a".to_string()),
                    out: Some("text_key".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bytes".to_string(),
                    bytes: Some(vec![b'a']),
                    out: Some("bytes_key".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    args: Some(vec![
                        "text_key".to_string(),
                        "text_key".to_string(),
                        "bytes_key".to_string(),
                        "text_key".to_string(),
                    ]),
                    out: Some("mapping".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_via_ir(&ir)
        .expect("tagged bytes keys must pass checked Luau admission");
    assert!(source.contains("local text_key: string = \"a\""));
    assert!(source.contains("local bytes_key = molt_binary_new(\"bytes\", \"\\x61\")"));
    assert!(source.contains("return molt_hash_string(668265263, binary.value)"));
    assert!(source.contains("molt_dict_set(mapping, text_key, text_key)"));
    assert!(source.contains("molt_dict_set(mapping, bytes_key, text_key)"));
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn execution_frame_runtime_is_coroutine_local_fail_closed_and_allocation_stable() {
    let source = format!(
        "--!strict\n{}\n{}",
        frame_runtime::FRAME_RUNTIME,
        r#"
local outer_code = {co_filename="frame_oracle.py", co_name="outer", co_firstlineno=10}
local inner_code = {co_filename="frame_oracle.py", co_name="inner", co_firstlineno=20}
local module_code = {co_filename="module_oracle.py", co_name="<module>", co_firstlineno=1}
local outer_globals = {__name__="outer_module"}
local inner_globals = {__name__="inner_module"}
local module_globals = {__name__="module_oracle"}
local outer_slot = {code=outer_code, globals=outer_globals}
local inner_slot = {code=inner_code, globals=inner_globals}
local module_slot = {code=module_code, globals=module_globals}

local module_context, module_depth, module_identity, module_owner = molt_frame_enter(module_slot)
molt_frame_locals_set(module_context, module_globals)
assert(module_context.globals[module_depth] == module_globals)
assert(module_context.locals[module_depth] == module_globals)
molt_frame_exit(module_context, module_depth, module_identity, module_owner)

local outer_context, outer_depth, outer_identity, outer_owner = molt_frame_enter(outer_slot)
molt_frame_set_line(outer_context, 11, 2, 9)
local inner_context, inner_depth, inner_identity, _inner_owner = molt_frame_enter(inner_slot)
assert(inner_context == outer_context and inner_depth == 2)
assert(inner_context.globals[outer_depth] == outer_globals)
assert(inner_context.globals[inner_depth] == inner_globals)
molt_frame_set_line(inner_context, 23, 4, 17)
local exception = molt_exception_attach_traceback(inner_context, {__type="ValueError", __msg="boom"})
assert(exception.__traceback__ == nil)
assert(#exception.__molt_traceback_locations == 2)
assert(exception.__molt_traceback_locations[1].name == "outer")
assert(exception.__molt_traceback_locations[1].line == 11)
assert(exception.__molt_traceback_locations[2].name == "inner")
assert(exception.__molt_traceback_locations[2].line == 23)
assert(exception.__molt_traceback_locations[2].col_offset == 4)
molt_frame_restore_depth(inner_context, outer_depth)
assert(inner_context.depth == 1)

local mismatch_ok, mismatch_error = pcall(function()
	molt_frame_exit(outer_context, outer_depth, inner_code, outer_owner)
end)
assert(not mismatch_ok and mismatch_error.__type == "RuntimeError" and outer_context.depth == 1)
molt_frame_exit(outer_context, outer_depth, outer_identity, outer_owner)
local empty_ok, empty_error = pcall(function()
	molt_frame_exit(outer_context, outer_depth, outer_identity, outer_owner)
end)
assert(not empty_ok and empty_error.__type == "RuntimeError")

local contexts = {}
local function coroutine_body(code, line)
	local context, depth, identity, owner = molt_frame_enter(code)
	molt_frame_set_line(context, line, 0, 1)
	coroutine.yield(context)
	assert(context.depth == 1 and context.lines[1] == line)
	molt_frame_exit(context, depth, identity, owner)
	return context
end
local first = coroutine.create(function() return coroutine_body(outer_slot, 31) end)
local second = coroutine.create(function() return coroutine_body(inner_slot, 41) end)
local ok_first, first_context = coroutine.resume(first)
local ok_second, second_context = coroutine.resume(second)
assert(ok_first and ok_second and first_context ~= second_context)
assert(first_context.depth == 1 and second_context.depth == 1)
local done_first, final_first = coroutine.resume(first)
local done_second, final_second = coroutine.resume(second)
assert(done_first and done_second and final_first.depth == 0 and final_second.depth == 0)

local failing_context: any = nil
local failing_owner: any = nil
local failing = molt_coroutine_execution_wrap(function(code)
	local context, _depth, _identity, owner = molt_frame_enter(code)
	failing_context = context
	failing_owner = owner
	molt_frame_set_line(context, 57, 6, 14)
	coroutine.yield("suspended")
	error({__type="ValueError", __msg="inside coroutine"}, 0)
end)
assert(failing(inner_slot) == "suspended")
assert(failing_context.depth == 1)
local failing_ok, failing_error = pcall(failing)
assert(not failing_ok and failing_error.__type == "ValueError")
assert(#failing_error.__molt_traceback_locations == 1)
assert(failing_error.__molt_traceback_locations[1].name == "inner")
assert(failing_error.__molt_traceback_locations[1].line == 57)
assert(failing_context.depth == 0)
assert(molt_frame_contexts[failing_owner] == nil)

local attachment_failure_context: any = nil
local attachment_failure = molt_coroutine_execution_wrap(function(code)
	local context = molt_frame_enter(code)
	attachment_failure_context = context
	context.codes[context.depth] = nil
	error({__type="ValueError", __msg="traceback attachment failure"}, 0)
end)
local attachment_ok, attachment_error = pcall(attachment_failure, inner_slot)
assert(not attachment_ok and attachment_error.__type == "RuntimeError")
assert(attachment_error.__cause__.__type == "ValueError")
assert(attachment_error.__molt_traceback_attachment_error ~= nil)
assert(attachment_failure_context.depth == 0)

local hostile_context: any = nil
local hostile = molt_coroutine_execution_wrap(function(code)
	local context = molt_frame_enter(code)
	hostile_context = context
	local exception = setmetatable({__type="ValueError", __msg="hostile"}, {
		__newindex=function() error("hostile newindex", 0) end,
	})
	error(exception, 0)
end)
local hostile_ok, hostile_error = pcall(hostile, inner_slot)
assert(not hostile_ok and hostile_error.__type == "ValueError")
assert(rawget(hostile_error, "__molt_traceback_locations") ~= nil)
assert(hostile_context.depth == 0)

local frozen_context: any = nil
local frozen = molt_coroutine_execution_wrap(function(code)
	local context = molt_frame_enter(code)
	frozen_context = context
	local exception = table.freeze({__type="ValueError", __msg="frozen"})
	error(exception, 0)
end)
local frozen_ok, frozen_error = pcall(frozen, inner_slot)
assert(not frozen_ok and frozen_error.__type == "RuntimeError")
assert(frozen_error.__cause__.__type == "ValueError")
assert(frozen_error.__molt_traceback_attachment_error ~= nil)
assert(frozen_context.depth == 0)

local restoration_failure_context: any = nil
local restoration_failure_owner: any = nil
local restoration_failure = molt_coroutine_execution_wrap(function(code)
	local context, _depth, _identity, owner = molt_frame_enter(code)
	restoration_failure_context = context
	restoration_failure_owner = owner
	table.freeze(context.codes)
	return "cannot complete with a poisoned frame stack"
end)
local restoration_ok, restoration_error = pcall(restoration_failure, inner_slot)
assert(not restoration_ok and restoration_error.__type == "RuntimeError")
assert(restoration_error.__msg == "execution-frame restoration failed")
assert(restoration_error.__molt_frame_restoration_error ~= nil)
assert(molt_frame_contexts[restoration_failure_owner] == nil)
assert(restoration_failure_context.depth == 1)

local completing_context: any = nil
local completing = molt_coroutine_execution_wrap(function(code)
	local context = molt_frame_enter(code)
	completing_context = context
	return "complete"
end)
assert(completing(outer_slot) == "complete")
assert(completing_context.depth == 0)

local close_context: any = nil
local close_owner: any = nil
local abandoned, close_abandoned = molt_coroutine_execution_wrap(function(code)
	local context, _depth, _identity, owner = molt_frame_enter(code)
	close_context = context
	close_owner = owner
	coroutine.yield("open")
end)
assert(abandoned(outer_slot) == "open" and close_context.depth == 1)
close_abandoned()
close_abandoned()
assert(close_context.depth == 0)
assert(molt_frame_contexts[close_owner] == nil)

local function context_count(): number
	local count = 0
	for _key, _context in molt_frame_contexts do count += 1 end
	return count
end
local context_baseline = context_count()
for _index = 1, 2000 do
	local resume = molt_coroutine_execution_wrap(function(code)
		local context, depth = molt_frame_enter(code)
		context.globals[depth] = {owner=coroutine.running()}
		coroutine.yield("abandoned")
	end)
	assert(resume(inner_slot) == "abandoned")
	resume = nil
end
for _round = 1, 8 do
	local pressure = table.create(250000, _round)
	assert(pressure[250000] == _round)
	pressure = nil
end
local contexts_after_abandonment = context_count()
for _sweep = 1, 32 do
	if contexts_after_abandonment <= context_baseline + 2 then break end
	for _round = 1, 8 do
		local pressure = table.create(250000, _round)
		assert(pressure[250000] == _round)
		pressure = nil
	end
	contexts_after_abandonment = context_count()
end
assert(
	contexts_after_abandonment <= context_baseline + 2,
	string.format("abandoned_context_leak baseline=%d after=%d", context_baseline, contexts_after_abandonment)
)

local completed_wrappers = table.create(2000)
local completed_closers = table.create(2000)
for index = 1, 2000 do
	local completed_owner: any = nil
	local resume, close = molt_coroutine_execution_wrap(function(code)
		completed_owner = coroutine.running()
		molt_frame_enter(code)
		return index
	end)
	assert(resume(outer_slot) == index)
	assert(completed_owner ~= nil and molt_frame_contexts[completed_owner] == nil,
		"completed wrapper retained its execution-context index")
	completed_wrappers[index] = resume
	completed_closers[index] = close
end
local contexts_after_completed = context_count()
assert(
	contexts_after_completed <= context_baseline + 2,
	string.format("completed_context_leak baseline=%d after=%d", context_baseline, contexts_after_completed)
)
local finalized_ok, finalized_error = pcall(completed_wrappers[1])
assert(not finalized_ok and finalized_error.__type == "RuntimeError")
completed_closers[1]()

local live_identity: any = nil
local live = coroutine.create(function()
	for _iteration = 1, 256 do
		local context = molt_frame_context()
		if live_identity == nil then live_identity = context end
		assert(context == live_identity)
		coroutine.yield(context)
	end
end)
local live_ok, live_context = coroutine.resume(live)
assert(live_ok and live_context == live_identity)
local allocations_after_live_warm = molt_frame_context_allocations
for iteration = 2, 256 do
	local pressure = table.create(20000, iteration)
	assert(pressure[20000] == iteration)
	pressure = nil
	local resumed, context = coroutine.resume(live)
	assert(resumed and context == live_identity)
end
assert(
	molt_frame_context_allocations == allocations_after_live_warm,
	"bare_live_coroutine_allocated_after_warm"
)
coroutine.close(live)

local wrapped_identity: any = nil
local wrapped_resume, wrapped_close = molt_coroutine_execution_wrap(function()
	for _iteration = 1, 256 do
		local context = molt_frame_context()
		if wrapped_identity == nil then wrapped_identity = context end
		assert(context == wrapped_identity)
		coroutine.yield(context)
	end
end)
assert(wrapped_resume() == wrapped_identity)
local allocations_after_wrapped_warm = molt_frame_context_allocations
for iteration = 2, 256 do
	local pressure = table.create(20000, iteration)
	assert(pressure[20000] == iteration)
	pressure = nil
	assert(wrapped_resume() == wrapped_identity)
end
assert(
	molt_frame_context_allocations == allocations_after_wrapped_warm,
	"wrapped_live_coroutine_allocated_after_warm"
)
wrapped_resume()
wrapped_close()

local warm_context, warm_depth, warm_identity, warm_owner = molt_frame_enter(outer_slot)
molt_frame_exit(warm_context, warm_depth, warm_identity, warm_owner)
local allocations_after_main_warm = molt_frame_context_allocations
local baseline_total = 0
local baseline_started = os.clock()
for index = 1, 100000 do baseline_total += index end
local baseline_elapsed = os.clock() - baseline_started
local heap_before = gcinfo()
local started = os.clock()
for index = 1, 100000 do
	local context, depth, identity, owner = molt_frame_enter(outer_slot)
	molt_frame_set_line(context, 12, 1, 3)
	molt_frame_exit(context, depth, identity, owner)
end
local elapsed = os.clock() - started
local heap_delta_kib = gcinfo() - heap_before
assert(baseline_total > 0 and elapsed < 5 and heap_delta_kib < 64)
assert(
	molt_frame_context_allocations == allocations_after_main_warm,
	"main_context_allocated_after_warm"
)
local weak_mode = getmetatable(molt_frame_contexts).__mode
assert(weak_mode == "kv", "frame_registry_is_not_non_owning_kv")
print(string.format("luau-execution-frame-ok calls=100000 abandoned=2000 completed_held=2000 live_allocations_after_warm=0 contexts_baseline=%d contexts_after_abandonment=%d contexts_after_completed=%d baseline_elapsed=%.6f framed_elapsed=%.6f added_elapsed=%.6f heap_delta_kib=%.1f", context_baseline, contexts_after_abandonment, contexts_after_completed, baseline_elapsed, elapsed, elapsed - baseline_elapsed, heap_delta_kib))
"#
    );
    // The shared context also owns explicit exception state; keep its complete
    // runtime bounded without excluding that protocol from the size budget.
    assert!(frame_runtime::FRAME_RUNTIME.len() < 19_000);
    assert!(!frame_runtime::FRAME_RUNTIME.contains("owner = key"));
    assert!(!frame_runtime::FRAME_RUNTIME.contains("context.owner"));
    assert!(frame_runtime::FRAME_RUNTIME.contains("{__mode = \"kv\"}"));
    assert!(frame_runtime::FRAME_RUNTIME.contains("molt_frame_owned_context(owner) ~= context"));
    assert!(
        !frame_runtime::FRAME_RUNTIME
            .contains("owner ~= current_owner or molt_frame_contexts[owner]")
    );
    assert_eq!(
        frame_runtime::FRAME_RUNTIME
            .matches("molt_frame_contexts[owner]")
            .count(),
        2,
        "direct owner lookup/removal belongs only to the ownership helper pair"
    );
    validate_luau_source(&source).expect("execution-frame oracle must pass source validation");
    let output = execute_lune_oracle("execution_frame", &source);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("luau-execution-frame-ok calls=100000 abandoned=2000 completed_held=2000 live_allocations_after_warm=0"),
        "{stdout}"
    );
    eprintln!(
        "luau-execution-frame-source-bytes={} {stdout}",
        frame_runtime::FRAME_RUNTIME.len()
    );
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn callable_frames_keep_exact_definition_context_across_rebinding_and_call_shapes() {
    let oracle = r#"
local first_builtins = molt_dict_new(); molt_dict_set(first_builtins, "builtin_value", 41)
local second_builtins = molt_dict_new(); molt_dict_set(second_builtins, "builtin_value", 99)
local first_globals = molt_dict_new(); molt_dict_set(first_globals, "value", "first")
molt_dict_set(first_globals, "__builtins__", first_builtins)
local second_globals = molt_dict_new(); molt_dict_set(second_globals, "value", "second")
molt_dict_set(second_globals, "__builtins__", second_builtins)
local old_code = {co_name="old", co_filename="same.py", co_firstlineno=1}
local new_code = {co_name="new", co_filename="same.py", co_firstlineno=2}
local module_code = {co_name="<module>", co_filename="same.py", co_firstlineno=1}
molt_code_slots[1] = molt_frame_bind_code(1, module_code, first_globals)
molt_code_slots[7] = molt_frame_bind_code(7, old_code, first_globals)
local target
target = function(mode)
	local context, depth, code, owner = molt_frame_enter_slot(molt_code_slots[7])
	local result
	if mode == "recurse" then result = target("inspect")
	else
		result = {code, context.globals[depth], context.builtins[depth], depth,
			molt_module_get_global(second_globals, "value"),
			molt_module_get_global(second_globals, "builtin_value")}
	end
	molt_frame_exit(context, depth, code, owner)
	return result
end
molt_function_register_signature(target, molt_pack_tuple("mode"))
local module_context, module_depth, module_identity, module_owner = molt_frame_enter_slot(molt_code_slots[1])
local metadata = molt_pack_tuple("target", "target", "sample", molt_pack_tuple("mode"), 0, molt_pack_tuple(), nil, nil, nil, nil, nil, 0, molt_pack_tuple(), molt_pack_tuple())
local first = molt_frame_function_new(target)
molt_function_init_metadata_packed(first, metadata, old_code, nil)
local second = molt_frame_function_new(target)
-- func_new is staging: default expressions can mutate builtins before metadata.
molt_dict_set(first_globals, "__builtins__", second_builtins)
molt_function_init_metadata_packed(second, metadata, new_code, nil)
assert(first ~= second and molt_func_attr_get(first, "__code__") == old_code)
assert(module_context.builtins[module_depth] == first_builtins)
molt_frame_exit(module_context, module_depth, module_identity, module_owner)
molt_code_slots[7] = molt_frame_bind_code(7, new_code, second_globals)

local function check(result, code, globals, builtins, value, builtin_value, depth)
	assert(result[1] == code and result[2] == globals and result[3] == builtins)
	assert(result[4] == depth and result[5] == value and result[6] == builtin_value)
end
check(molt_call_checked(first, "inspect"), old_code, first_globals, first_builtins, "first", 41, 1)
check(molt_call_checked(second, "inspect"), new_code, first_globals, second_builtins, "first", 99, 1)
local keywords = molt_callargs_new(); molt_callargs_push_kw(keywords, "mode", "inspect")
check(molt_callargs_invoke(first, keywords), old_code, first_globals, first_builtins, "first", 41, 1)
local bound = molt_bound_method_new(first, "inspect")
check(molt_call_checked(bound), old_code, first_globals, first_builtins, "first", 41, 1)
-- Static recursion does not reuse a consumed dynamic handoff.
check(molt_call_checked(first, "recurse"), new_code, second_globals, second_builtins, "second", 99, 2)
check(target("inspect"), new_code, second_globals, second_builtins, "second", 99, 1)

local context = molt_frame_context()
assert(context.depth == 0 and #context.invocations == 0)
local mismatch = molt_frame_invoke(function()
	local unrelated, depth, code, owner = molt_frame_enter_slot(molt_code_slots[1])
	assert(code == module_code and unrelated.depth == 1)
	molt_frame_exit(unrelated, depth, code, owner)
	return target("inspect")
end, old_code, first_globals, first_builtins)
check(mismatch, old_code, first_globals, first_builtins, "first", 41, 1)
local failure = {__type="ValueError", __msg="unconsumed handoff"}
local ok, error_value = pcall(function()
	molt_frame_invoke(function() error(failure, 0) end, old_code, first_globals, first_builtins)
end)
assert(not ok and error_value == failure and #context.invocations == 0)
-- A captured miss never switches to later cached or globals-selected builtins.
molt_module_cache["builtins"] = {builtin_value=123}
molt_dict_delete(first_builtins, "builtin_value", false)
local missing_ok, missing_error = pcall(function() return molt_call_checked(first, "inspect") end)
assert(not missing_ok and missing_error.__type == "NameError")
assert(#context.invocations == 0)
molt_frame_restore_depth(context, 0)
molt_dict_set(first_builtins, "builtin_value", 41)

-- Replacing a globals entry affects new definitions, not existing functions;
-- an absent entry instead inherits the defining activation's captured builtins.
local defining, depth, code, owner = molt_frame_enter(molt_code_slots[1], first_builtins)
molt_dict_delete(first_globals, "__builtins__", false)
local inherited = molt_frame_function_new(target)
molt_function_init_metadata_packed(inherited, metadata, old_code, nil)
molt_frame_exit(defining, depth, code, owner)
check(molt_call_checked(inherited, "inspect"), old_code, first_globals, first_builtins, "first", 41, 1)
assert(context.depth == 0 and #context.invocations == 0)
molt_dict_set(first_globals, "remove", 1); molt_dict_set(second_globals, "remove", 2)
molt_frame_invoke(function()
	local active, depth, code, owner = molt_frame_enter_slot(molt_code_slots[7])
	assert(molt_globals_builtin() == first_globals)
	molt_dict_set(molt_globals_builtin(), "written", 3)
	molt_module_del_global(second_globals, "remove", false)
	molt_module_del_global(second_globals, "remove", true)
	molt_frame_exit(active, depth, code, owner)
end, old_code, first_globals, first_builtins)
assert(not molt_dict_contains(first_globals, "remove"))
assert(molt_dict_getitem(second_globals, "remove") == 2)
assert(molt_dict_getitem(first_globals, "written") == 3)
print("luau-callable-frame-context-ok")
"#;
    let mut backend = LuauBackend::new();
    backend.emit_prelude_conditional(oracle);
    let source = format!("{}\n{oracle}", backend.output);
    validate_luau_source(&source).expect("callable-context oracle must pass source validation");
    let output = execute_lune_oracle("callable_frame_context", &source);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("luau-callable-frame-context-ok"),
        "{stdout}"
    );
}

#[test]
fn compile_checked_rejects_alloc_task_without_scheduler_authority() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "sample__genexpr_1".to_string(),
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![
                    OpIR {
                        kind: "alloc_task".to_string(),
                        s_value: Some("sample__genexpr_1".to_string()),
                        out: Some("items".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };

    let error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("alloc_task requires the unavailable Molt scheduler model");
    assert!(error.contains("`alloc_task`") && error.contains("exact async scheduler"));
}

#[test]
fn module_chunks_receive_one_strong_caller_frame_context() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "sample__molt_module_chunk_1".to_string(),
                params: vec!["module".to_string()],
                execution_context: ExecutionContextPolicy::Inherited,
                ops: vec![
                    OpIR {
                        kind: "line".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("sample__molt_module_chunk_1".to_string()),
                        args: Some(vec!["module".to_string()]),
                        out: Some("result".to_string()),
                        passes_execution_context: true,
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };

    let source = LuauBackend::new().emit_source(&ir);
    assert!(source.contains(
        "sample__molt_module_chunk_1 = function(module: any, __molt_frame_context: any)"
    ));
    assert!(source.contains("sample__molt_module_chunk_1(module, __molt_frame_context)"));
    let chunk = source
        .split("sample__molt_module_chunk_1 = function")
        .nth(1)
        .expect("chunk body")
        .split("\nend")
        .next()
        .expect("chunk terminator");
    assert!(!chunk.contains("molt_frame_context()"));
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn ordered_dict_runtime_executes_full_semantics_in_lune() {
    let oracle = r#"
local function run_authority_oracle()
local d = molt_dict_new()
assert(next(d) == nil)
molt_dict_set(d, "a", 1)
molt_dict_set(d, "b", nil)
molt_dict_set(d, "n", 3)
molt_dict_set(d, "b", 20)
assert(molt_dict_len(d) == 3)
local keys = molt_dict_keys(d)
local key_snapshot = molt_dict_view_snapshot(keys)
assert(rawget(key_snapshot, 1) == "a" and rawget(key_snapshot, 2) == "b" and rawget(key_snapshot, 3) == "n")
local values = molt_dict_values(d)
local value_snapshot = molt_dict_view_snapshot(values)
assert(rawget(value_snapshot, 1) == 1 and rawget(value_snapshot, 2) == 20 and rawget(value_snapshot, 3) == 3)
local items = molt_dict_items(d)
local item_snapshot = molt_dict_view_snapshot(items)
assert(rawget(rawget(item_snapshot, 2), 1) == "b" and rawget(rawget(item_snapshot, 2), 2) == 20)
assert(molt_dict_setdefault(d, "b", 99) == 20)
assert(molt_dict_setdefault(d, "c", 5) == 5)
assert(molt_dict_pop(d, "c", false, nil) == 5)
molt_dict_delete(d, "a", false)
molt_dict_set(d, "a", 4)
keys = molt_dict_keys(d)
key_snapshot = molt_dict_view_snapshot(keys)
assert(rawget(key_snapshot, 1) == "b" and rawget(key_snapshot, 2) == "n" and rawget(key_snapshot, 3) == "a")
local popped = molt_dict_popitem(d)
assert(rawget(popped, 1) == "a" and rawget(popped, 2) == 4)
keys = molt_dict_keys(d)
key_snapshot = molt_dict_view_snapshot(keys)
assert(molt_dict_view_len(keys) == 2 and rawget(key_snapshot, 1) == "b" and rawget(key_snapshot, 2) == "n")
molt_dict_set(d, nil, nil)
assert(molt_dict_contains(d, nil) and molt_dict_getitem(d, nil) == nil)
assert(molt_repr(d) == "{'b': 20, 'n': 3, None: None}")
assert(molt_json_dumps(d) == '{"b": 20, "n": 3, "null": null}')
assert(molt_json_dumps(molt_pack_list("a\n")) == '["a\\n"]')
local nonfinite_ok, nonfinite_error = pcall(function() molt_json_dumps(math.huge) end)
assert(not nonfinite_ok and nonfinite_error.__type == "ValueError")

local aliases = molt_dict_new()
molt_dict_set(aliases, true, "true-first")
molt_dict_set(aliases, 1, "one-replaces")
molt_dict_set(aliases, false, "false-first")
molt_dict_set(aliases, 0, "zero-replaces")
local alias_keys = molt_dict_keys(aliases)
local alias_snapshot = molt_dict_view_snapshot(alias_keys)
assert(molt_dict_len(aliases) == 2)
assert(rawget(alias_snapshot, 1) == true and rawget(alias_snapshot, 2) == false)
assert(molt_dict_getitem(aliases, true) == "one-replaces")
assert(molt_dict_getitem(aliases, 0) == "zero-replaces")
local text_key = "a"
local bytes_key = molt_binary_new("bytes", "a")
local equal_bytes_key = molt_binary_new("bytes", "a")
local binary_keys = molt_dict_new(); molt_dict_set(binary_keys, text_key, "text"); molt_dict_set(binary_keys, bytes_key, "bytes")
assert(molt_dict_len(binary_keys) == 2 and molt_dict_getitem(binary_keys, text_key) == "text" and molt_dict_getitem(binary_keys, equal_bytes_key) == "bytes")
local bytearray_key_ok, bytearray_key_error = pcall(function() molt_dict_set(binary_keys, molt_binary_new("bytearray", "a"), 1) end)
assert(not bytearray_key_ok and bytearray_key_error.__type == "TypeError")
local range_key_ok, range_key_error = pcall(function() molt_dict_set(binary_keys, molt_pack_sequence_kind("range", 0, 1), 1) end)
assert(not range_key_ok and range_key_error.__type == "TypeError")

local compacted = molt_dict_new()
for index = 0, 79 do molt_dict_set(compacted, index, index) end
for index = 0, 59 do molt_dict_delete(compacted, index, false) end
for index = 0, 9 do molt_dict_set(compacted, index, index) end
local compacted_keys = molt_dict_keys(compacted)
local compacted_snapshot = molt_dict_view_snapshot(compacted_keys)
assert(molt_dict_len(compacted) == 30 and molt_dict_view_len(compacted_keys) == 30)
for index = 1, 20 do assert(rawget(compacted_snapshot, index) == index + 59) end
for index = 21, 30 do assert(rawget(compacted_snapshot, index) == index - 21) end
molt_dict_delete(compacted, 60, false)
molt_dict_set(compacted, 60, 600)
compacted_snapshot = molt_dict_view_snapshot(molt_dict_keys(compacted))
assert(rawget(compacted_snapshot, 30) == 60 and molt_dict_getitem(compacted, 60) == 600)
local compacted_last = molt_dict_popitem(compacted)
assert(rawget(compacted_last, 1) == 60 and rawget(compacted_last, 2) == 600)

local left = molt_dict_new()
local right = molt_dict_new()
molt_dict_set(left, "a", molt_pack_list(1, true))
molt_dict_set(left, "b", 2)
molt_dict_set(right, "b", 2)
molt_dict_set(right, "a", molt_pack_list(1, 1))
assert(molt_equal(left, right))
molt_dict_set(right, "b", 3)
assert(not molt_equal(left, right))
assert(not molt_equal(molt_pack_list(1), molt_pack_tuple(1)))
assert(not molt_equal({1}, {1}))

local live = molt_dict_keys(d)
local before = molt_dict_view_len(live)
molt_dict_set(d, "live", 9)
assert(molt_dict_view_len(live) == before + 1)
local iterator = molt_dict_iterator_new(d, "keys")
molt_dict_iterator_next(iterator)
molt_dict_set(d, "mutated", 10)
local mutation_ok, mutation_error = pcall(function() molt_dict_iterator_next(iterator) end)
assert(not mutation_ok and mutation_error.__type == "RuntimeError")

local kwargs = molt_callargs_new()
molt_callargs_push_kw(kwargs, "x", 1)
local duplicate_ok, duplicate_error = pcall(function() molt_callargs_push_kw(kwargs, "x", 2) end)
assert(not duplicate_ok and duplicate_error.__type == "TypeError")
local function add(a, b) return a + b end
molt_function_metadata[add] = {arg_names=molt_pack_tuple("a", "b"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
local invoke_args = molt_callargs_new()
molt_callargs_push_pos(invoke_args, 4)
molt_callargs_push_kw(invoke_args, "b", 5)
assert(molt_callargs_invoke(add, invoke_args) == 9)
local positional_duplicate = molt_callargs_new()
molt_callargs_push_pos(positional_duplicate, 4)
molt_callargs_push_kw(positional_duplicate, "a", 5)
local positional_duplicate_ok, positional_duplicate_error = pcall(function() molt_callargs_invoke(add, positional_duplicate) end)
assert(not positional_duplicate_ok and positional_duplicate_error.__type == "TypeError")

local kwonly_defaults = molt_dict_new()
molt_dict_set(kwonly_defaults, "c", nil)
local function exact(a, b, c, rest, extras) return molt_pack_tuple(a, b, c, rest, extras) end
molt_function_metadata[exact] = {arg_names=molt_pack_tuple("a", "b"), posonly=1, kwonly=molt_pack_tuple("c"), vararg="rest", varkw="extras", defaults=molt_pack_tuple(nil), kwdefaults=kwonly_defaults}
local exact_args = molt_callargs_new()
molt_callargs_push_pos(exact_args, 7)
local exact_result = molt_callargs_invoke(exact, exact_args)
assert(molt_sequence_len(exact_result) == 5 and rawget(exact_result, 1) == 7 and rawget(exact_result, 2) == nil and rawget(exact_result, 3) == nil)
assert(molt_sequence_len(rawget(exact_result, 4)) == 0 and molt_dict_len(rawget(exact_result, 5)) == 0)
local rich_args = molt_callargs_new()
molt_callargs_push_pos(rich_args, 1)
molt_callargs_push_pos(rich_args, nil)
molt_callargs_push_pos(rich_args, 30)
molt_callargs_push_pos(rich_args, nil)
molt_callargs_push_kw(rich_args, "c", 9)
molt_callargs_push_kw(rich_args, "a", 44)
molt_callargs_push_kw(rich_args, "other", nil)
local rich_result = molt_callargs_invoke(exact, rich_args)
assert(rawget(rich_result, 1) == 1 and rawget(rich_result, 2) == nil and rawget(rich_result, 3) == 9)
local rich_rest = rawget(rich_result, 4)
assert(molt_sequence_len(rich_rest) == 2 and rawget(rich_rest, 1) == 30 and rawget(rich_rest, 2) == nil)
local rich_extras = rawget(rich_result, 5)
assert(molt_dict_getitem(rich_extras, "a") == 44 and molt_dict_contains(rich_extras, "other") and molt_dict_getitem(rich_extras, "other") == nil)
local function strict(a, b) return a + b end
molt_function_metadata[strict] = {arg_names=molt_pack_tuple("a", "b"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
local extra_pos = molt_callargs_new(); molt_callargs_push_pos(extra_pos, 1); molt_callargs_push_pos(extra_pos, 2); molt_callargs_push_pos(extra_pos, 3)
local extra_pos_ok, extra_pos_error = pcall(function() molt_callargs_invoke(strict, extra_pos) end)
assert(not extra_pos_ok and extra_pos_error.__type == "TypeError")
local unexpected = molt_callargs_new(); molt_callargs_push_pos(unexpected, 1); molt_callargs_push_kw(unexpected, "other", 2)
local unexpected_ok, unexpected_error = pcall(function() molt_callargs_invoke(strict, unexpected) end)
assert(not unexpected_ok and unexpected_error.__type == "TypeError")
local missing_call = molt_callargs_new(); molt_callargs_push_pos(missing_call, 1)
local missing_ok_call, missing_error_call = pcall(function() molt_callargs_invoke(strict, missing_call) end)
assert(not missing_ok_call and missing_error_call.__type == "TypeError")
local function method(self, x) return self + x end
molt_function_metadata[method] = {arg_names=molt_pack_tuple("self", "x"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
local bound = molt_bound_method_new(method, 10)
local bound_args = molt_callargs_new(); molt_callargs_push_kw(bound_args, "x", 5)
assert(molt_callargs_invoke(bound, bound_args) == 15)
local function method_with_defaults(self, x, y) return self + x + y end
molt_function_metadata[method_with_defaults] = {arg_names=molt_pack_tuple("self", "x", "y"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=molt_pack_tuple(99, 5, 7), kwdefaults=nil}
local bound_defaults = molt_bound_method_new(method_with_defaults, 10)
assert(molt_function_metadata[bound_defaults].bound_func == method_with_defaults and molt_function_metadata[bound_defaults].bound_self == 10)
assert(molt_callargs_invoke(bound_defaults, molt_callargs_new()) == 22)
local bound_override = molt_callargs_new(); molt_callargs_push_kw(bound_override, "y", 2)
assert(molt_callargs_invoke(bound_defaults, bound_override) == 17)
molt_function_attr_set(method_with_defaults, "__defaults__", molt_pack_tuple(99, 8, 9))
assert(molt_call_checked(bound_defaults) == 27 and molt_callargs_invoke(bound_defaults, molt_callargs_new()) == 27)
local function capture3(a, b, c) return molt_pack_tuple(a, b, c) end
molt_function_metadata[capture3] = {arg_names=molt_pack_tuple("a", "b", "c"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
local string_star = molt_callargs_new(); molt_callargs_expand_star(string_star, "ab"); molt_callargs_push_pos(string_star, "c")
local string_star_result = molt_callargs_invoke(capture3, string_star)
assert(rawget(string_star_result, 1) == "a" and rawget(string_star_result, 2) == "b" and rawget(string_star_result, 3) == "c")
local set_star_value = molt_set_new("set"); molt_set_add(set_star_value, "x"); molt_set_add(set_star_value, "y")
local set_star = molt_callargs_new(); molt_callargs_expand_star(set_star, set_star_value); molt_callargs_push_pos(set_star, "z")
local set_star_result = molt_callargs_invoke(capture3, set_star)
assert(rawget(set_star_result, 1) == "x" and rawget(set_star_result, 2) == "y" and rawget(set_star_result, 3) == "z")
local dict_star_value = molt_dict_new(); molt_dict_set(dict_star_value, "k1", 1); molt_dict_set(dict_star_value, "k2", 2)
local dict_star = molt_callargs_new(); molt_callargs_expand_star(dict_star, dict_star_value); molt_callargs_push_pos(dict_star, "tail")
local dict_star_result = molt_callargs_invoke(capture3, dict_star)
assert(rawget(dict_star_result, 1) == "k1" and rawget(dict_star_result, 2) == "k2" and rawget(dict_star_result, 3) == "tail")
local bad_star_ok, bad_star_error = pcall(function() molt_callargs_expand_star(molt_callargs_new(), 42) end)
assert(not bad_star_ok and bad_star_error.__type == "TypeError")

local set_value = molt_set_new("set")
molt_set_add(set_value, nil); molt_set_add(set_value, true); molt_set_add(set_value, 1); molt_set_add(set_value, "x")
assert(molt_set_len(set_value) == 3 and molt_set_contains(set_value, nil) and molt_set_contains(set_value, 1))
assert(molt_repr(set_value) == "{None, True, 'x'}")
local frozen = molt_set_new("frozenset")
molt_frozenset_build_add(frozen, "x"); molt_frozenset_build_add(frozen, nil); molt_frozenset_build_add(frozen, 1); molt_set_freeze(frozen)
assert(molt_equal(set_value, frozen) and molt_repr(frozen) == "frozenset({'x', None, 1})")
local frozen_ok, frozen_error = pcall(function() molt_set_add(frozen, 2) end)
assert(not frozen_ok and frozen_error.__type == "AttributeError")
local set_iterator = molt_iterator_new(set_value)
local set_first = set_iterator()
assert(rawget(set_first, 1) == nil and rawget(set_first, 2) == false)

local view_dict = molt_dict_new()
molt_dict_set(view_dict, "a", 1); molt_dict_set(view_dict, "b", nil)
local view_keys = molt_dict_keys(view_dict)
local view_values = molt_dict_values(view_dict)
local view_items = molt_dict_items(view_dict)
assert(molt_dict_view_contains(view_keys, "a"))
assert(molt_dict_view_contains(view_values, nil))
assert(molt_dict_view_contains(view_items, molt_pack_tuple("b", nil)))
local key_set = molt_set_new("set"); molt_set_add(key_set, "b"); molt_set_add(key_set, "a")
assert(molt_equal(view_keys, key_set))
assert(not molt_equal(view_values, molt_dict_values(view_dict)))
assert(molt_repr(view_keys) == "dict_keys(['a', 'b'])")
assert(molt_repr(view_values) == "dict_values([1, None])")
assert(molt_repr(view_items) == "dict_items([('a', 1), ('b', None)])")

local weak_function = setmetatable({}, {__mode="v"})
local function install_ephemeral()
	local function ephemeral(value) return value end
	molt_function_metadata[ephemeral] = {arg_names=molt_pack_tuple("value"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
	molt_func_attr_set(ephemeral, "__self_cycle", ephemeral)
	assert(molt_func_attrs[ephemeral].__self_cycle == molt_func_self_attr)
	weak_function[1] = ephemeral
end
install_ephemeral()
assert(getmetatable(molt_function_metadata).__mode == "k" and getmetatable(molt_func_attrs).__mode == "k")

local missing = {}
local missing_target = molt_dict_new()
molt_dict_update_missing(missing_target, "x", 1, missing)
assert(molt_dict_getitem(missing_target, "x") == 1)
molt_dict_update_missing(missing_target, "x", missing, missing)
assert(not molt_dict_contains(missing_target, "x"))
local kwstar_source = molt_dict_new()
molt_dict_set(kwstar_source, "good", 1)
molt_dict_update_kwstar(missing_target, kwstar_source)
assert(molt_dict_getitem(missing_target, "good") == 1)
local bad_kwstar = molt_dict_new()
molt_dict_set(bad_kwstar, 1, 2)
local kwstar_ok, kwstar_error = pcall(function() molt_dict_update_kwstar(missing_target, bad_kwstar) end)
assert(not kwstar_ok and kwstar_error.__type == "TypeError")

local recursive = molt_dict_new()
molt_dict_set(recursive, "self", recursive)
assert(molt_repr(recursive) == "{'self': {...}}")
local escaped_repr = molt_repr("a\n'b\\c")
assert(escaped_repr == "'a\\n\\'b\\\\c'", escaped_repr)

local nan_ok, nan_error = pcall(function() molt_dict_set(d, 0 / 0, 1) end)
assert(not nan_ok and type(nan_error) == "table" and nan_error.__type == "TypeError")
local object_key = {}
molt_dict_set(d, object_key, "identity")
assert(molt_dict_getitem(d, object_key) == "identity" and not molt_dict_contains(d, {}))
local negative_zero_dict = molt_dict_new(); molt_dict_set(negative_zero_dict, -0.0, "zero")
assert(molt_dict_getitem(negative_zero_dict, 0) == "zero" and molt_dict_len(negative_zero_dict) == 1)
local custom_key_class = {__eq__=function(left, right) return left == right end}; custom_key_class.__index = custom_key_class
local custom_key = {}; setmetatable(custom_key, custom_key_class)
local custom_key_ok, custom_key_error = pcall(function() molt_dict_set(d, custom_key, 1) end)
assert(not custom_key_ok and custom_key_error.__type == "TypeError")
local tuple_key = molt_pack_tuple("tuple", nil, 3)
local equal_tuple_key = molt_pack_tuple("tuple", nil, 3)
molt_dict_set(d, tuple_key, "tuple-value")
assert(molt_dict_getitem(d, equal_tuple_key) == "tuple-value")
local ordered_tuple_key = molt_pack_tuple(3, nil, "tuple")
assert(not molt_dict_contains(d, ordered_tuple_key))
local frozen_key = molt_set_new("frozenset"); molt_frozenset_build_add(frozen_key, "a"); molt_frozenset_build_add(frozen_key, 2); molt_set_freeze(frozen_key)
local equal_frozen_key = molt_set_new("frozenset"); molt_frozenset_build_add(equal_frozen_key, 2); molt_frozenset_build_add(equal_frozen_key, "a"); molt_set_freeze(equal_frozen_key)
molt_dict_set(d, frozen_key, "frozen-value")
assert(molt_dict_getitem(d, equal_frozen_key) == "frozen-value")
local tuple_members = molt_set_new("set")
molt_set_add(tuple_members, tuple_key); molt_set_add(tuple_members, equal_tuple_key)
assert(molt_set_len(tuple_members) == 1)
local collision_left = {}; local collision_right = {}
molt_identity_hashes[collision_left] = 1234567; molt_identity_hashes[collision_right] = 1234567
local collision_dict = molt_dict_new()
molt_dict_set(collision_dict, collision_left, "left"); molt_dict_set(collision_dict, collision_right, "right")
assert(molt_dict_getitem(collision_dict, collision_left) == "left" and molt_dict_getitem(collision_dict, collision_right) == "right")

local copied = molt_dict_copy(d)
assert(molt_equal(copied, d))
local converted = molt_dict_from_obj(d)
assert(molt_equal(converted, d))
local foreign_ok = pcall(function() molt_dict_from_obj({a = 1}) end)
assert(not foreign_ok)
local cycle = molt_pack_list()
rawset(cycle, 1, cycle); rawset(cycle, molt_sequence_length_key, 1)
local cycle_ok, cycle_error = pcall(function() molt_json_dumps(cycle) end)
assert(not cycle_ok and cycle_error.__type == "ValueError")
local foreign_json_ok = pcall(function() molt_json_dumps({1, 2}) end)
assert(not foreign_json_ok)
molt_dict_update(copied, aliases)
assert(molt_dict_contains(copied, true) and molt_dict_contains(copied, false))
molt_dict_clear(copied)
assert(molt_dict_len(copied) == 0 and molt_repr(copied) == "{}")
local memory_before = gcinfo()
local bench_start = os.clock()
local retained = molt_pack_list()
for index = 1, 5000 do
	local entry = molt_dict_new()
	molt_dict_set(entry, "a", index)
	molt_dict_set(entry, "b", index + 1)
	molt_dict_set(entry, "c", index + 2)
	molt_dict_set(entry, "d", index + 3)
	rawset(retained, index, entry)
end
rawset(retained, molt_sequence_length_key, 5000)
local bench_elapsed = os.clock() - bench_start
local dict_retained_kib = gcinfo() - memory_before
local call_memory_before = gcinfo()
local call_bench_start = os.clock()
local call_total = 0
for _index = 1, 100000 do call_total += molt_call_checked(strict, 1, 2) end
local call_bench_elapsed = os.clock() - call_bench_start
local call_heap_delta_kib = gcinfo() - call_memory_before
assert(call_total == 300000 and call_bench_elapsed < 5)
local function measure_set_scale(count)
	local memory_before_scale = gcinfo()
	local start = os.clock()
	local value = molt_set_new("set")
	for index = 1, count do molt_set_add(value, index) end
	local elapsed = os.clock() - start
	local heap_delta_kib = gcinfo() - memory_before_scale
	assert(molt_set_len(value) == count and molt_set_contains(value, count - 1) and elapsed < 5)
	return value, elapsed, heap_delta_kib, heap_delta_kib * 1024 / count
end
local set_1k, set_1k_elapsed, set_1k_heap_kib, set_1k_bytes = measure_set_scale(1000)
local set_10k, set_10k_elapsed, set_10k_heap_kib, set_10k_bytes = measure_set_scale(10000)
local set_100k, set_100k_elapsed, set_100k_heap_kib, set_100k_bytes = measure_set_scale(100000)
assert(molt_set_len(set_1k) + molt_set_len(set_10k) + molt_set_len(set_100k) == 111000)
assert(set_100k_bytes < 512 and set_10k_bytes < 512)
local tuple_bench_memory_before = gcinfo()
local tuple_bench_start = os.clock()
local tuple_dict = molt_dict_new()
local tuple_keys = molt_pack_list()
for index = 1, 10000 do
	local key = molt_pack_tuple(index, "key")
	rawset(tuple_keys, index, key)
	molt_dict_set(tuple_dict, key, index)
end
rawset(tuple_keys, molt_sequence_length_key, 10000)
for operation = 1, 100000 do
	local index = ((operation - 1) % 10000) + 1
	local key = rawget(tuple_keys, index)
	if operation % 2 == 0 then molt_dict_delete(tuple_dict, key, false); molt_dict_set(tuple_dict, key, index)
	else assert(molt_dict_getitem(tuple_dict, key) == index) end
end
local tuple_bench_elapsed = os.clock() - tuple_bench_start
local tuple_peak_heap_delta_kib = gcinfo() - tuple_bench_memory_before
assert(molt_dict_len(tuple_dict) == 10000 and #molt_dict_metadata[tuple_dict].order <= 20032 and tuple_bench_elapsed < 5)
local collision_bench_start = os.clock()
local collision_bench_dict = molt_dict_new()
local collision_keys = molt_pack_list()
for index = 1, 10000 do
	local key = {}
	molt_identity_hashes[key] = index % 1000
	rawset(collision_keys, index, key)
	molt_dict_set(collision_bench_dict, key, index)
end
rawset(collision_keys, molt_sequence_length_key, 10000)
for index = 1, 10000 do assert(molt_dict_getitem(collision_bench_dict, rawget(collision_keys, index)) == index) end
local collision_bench_elapsed = os.clock() - collision_bench_start
assert(molt_dict_len(collision_bench_dict) == 10000 and collision_bench_elapsed < 5)
local churn_memory_before = gcinfo()
local churn_start = os.clock()
local churn_dict = molt_dict_new()
for index = 1, 100000 do molt_dict_set(churn_dict, index, index) end
local churn_metadata = molt_dict_metadata[churn_dict]
local churn_high_water_records = churn_metadata.records
for index = 1, 99900 do molt_dict_delete(churn_dict, index, false) end
local churn_elapsed = os.clock() - churn_start
local churn_heap_delta_kib = gcinfo() - churn_memory_before
assert(molt_dict_len(churn_dict) == 100 and churn_metadata.records ~= churn_high_water_records)
assert(churn_metadata.next_id <= churn_metadata.size * 2 + 32 and #churn_metadata.records <= churn_metadata.next_id * 7)
for index = 99901, 100000 do assert(molt_dict_getitem(churn_dict, index) == index) end
local frozen_bench = molt_set_new("frozenset")
for index = 1, 10000 do molt_frozenset_build_add(frozen_bench, index) end
molt_set_freeze(frozen_bench)
local frozen_bench_dict = molt_dict_new(); molt_dict_set(frozen_bench_dict, frozen_bench, "cached")
local frozen_lookup_start = os.clock()
for _index = 1, 100000 do assert(molt_dict_getitem(frozen_bench_dict, frozen_bench) == "cached") end
local frozen_lookup_elapsed = os.clock() - frozen_lookup_start
assert(molt_set_metadata[frozen_bench].cached_hash ~= nil and molt_set_metadata[frozen_bench].hash_locked == true)
assert(churn_elapsed < 5 and frozen_lookup_elapsed < 5)
assert(bench_elapsed < 5 and dict_retained_kib > 0 and set_10k_heap_kib > 0 and call_heap_delta_kib < 64)
print(string.format("luau-authority-ok dict5k_elapsed=%.6f dict_heap_kib=%.1f dict_bytes_per_map=%.1f call100k_elapsed=%.6f call_heap_delta_kib=%.1f set1k_elapsed=%.6f set1k_heap_kib=%.1f set1k_bytes=%.1f set10k_elapsed=%.6f set10k_heap_kib=%.1f set10k_bytes=%.1f set100k_elapsed=%.6f set100k_heap_kib=%.1f set100k_bytes=%.1f tuple10k_mixed100k_elapsed=%.6f tuple_peak_heap_delta_kib=%.1f collision10k_elapsed=%.6f churn100k_delete99900_elapsed=%.6f churn_allocator_delta_kib=%.1f churn_capacity=%d frozen10k_lookup100k_elapsed=%.6f", bench_elapsed, dict_retained_kib, dict_retained_kib * 1024 / 5000, call_bench_elapsed, call_heap_delta_kib, set_1k_elapsed, set_1k_heap_kib, set_1k_bytes, set_10k_elapsed, set_10k_heap_kib, set_10k_bytes, set_100k_elapsed, set_100k_heap_kib, set_100k_bytes, tuple_bench_elapsed, tuple_peak_heap_delta_kib, collision_bench_elapsed, churn_elapsed, churn_heap_delta_kib, churn_metadata.next_id, frozen_lookup_elapsed))
end
run_authority_oracle()
"#;
    let mut backend = LuauBackend::new();
    backend.emit_prelude_conditional(oracle);
    let source = format!("{}\n{oracle}", backend.output);
    assert_eq!(
        source
            .matches("local function molt_function_attr_set")
            .count(),
        1,
        "metadata-aware function mutation must have one emitted authority"
    );
    assert!(
        source
            .find("local function molt_function_attr_set")
            .expect("metadata-aware function mutation helper")
            < source
                .find("local function run_authority_oracle")
                .expect("ordered-dict executable oracle"),
        "runtime helpers must be declared before the executable oracle"
    );
    let runtime_bytes = dict_runtime::DICT_CORE_RUNTIME.len()
        + dict_runtime::CALLARGS_RUNTIME.len()
        + dict_runtime::EQUALITY_REPR_RUNTIME.len();
    let callable_metadata_bytes = frame_runtime::CALLABLE_METADATA_RUNTIME.len();
    let prelude_bytes = include_str!("../../luau_json_prelude.luau").len();
    assert!(
        runtime_bytes < 43_000,
        "Luau container/call runtime grew to {runtime_bytes} bytes"
    );
    assert!(
        callable_metadata_bytes < 5_000,
        "Luau callable metadata runtime grew to {callable_metadata_bytes} bytes"
    );
    eprintln!(
        "luau-source-size runtime_bytes={runtime_bytes} callable_metadata_bytes={callable_metadata_bytes} prelude_bytes={prelude_bytes} oracle_bytes={}",
        source.len()
    );
    let output = execute_lune_oracle("ordered_dict", &source);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .trim()
            .starts_with("luau-authority-ok dict5k_elapsed=")
    );
    eprintln!("{}", stdout.trim());
}

#[test]
fn proven_scalar_equality_does_not_pay_container_runtime_cost() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "scalar_equal".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("left".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("right".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "eq".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("equal".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["equal".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);
    assert!(source.contains("local equal: boolean = (left == right)"));
    assert!(!source.contains("local molt_dict_metadata_key = {}"));
    assert!(!source.contains("molt_equal(left, right)"));
}

#[test]
fn scalar_identity_preserves_source_kind_and_covers_both_polarities() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "scalar_identity".to_string(),
            params: vec![
                "integer".to_string(),
                "float".to_string(),
                "boolean".to_string(),
            ],
            param_types: Some(vec![
                "int".to_string(),
                "float".to_string(),
                "bool".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["integer".to_string(), "float".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is_not".to_string(),
                    args: Some(vec!["integer".to_string(), "float".to_string()]),
                    out: Some("different".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["integer".to_string(), "boolean".to_string()]),
                    out: Some("int_is_bool".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    args: Some(vec![
                        "same".to_string(),
                        "different".to_string(),
                        "int_is_bool".to_string(),
                    ]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    assert!(source.contains("local same: boolean = false"), "{source}");
    assert!(
        source.contains("local different: boolean = true"),
        "{source}"
    );
    assert!(
        source.contains("local int_is_bool: boolean = false"),
        "{source}"
    );
    assert!(!source.contains("[unsupported op: is_not]"), "{source}");
}

#[test]
fn dynamic_numeric_identity_fails_closed_before_luau_erases_provenance() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dynamic_identity".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["same".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let error = LuauBackend::new().compile_checked(&ir).unwrap_err();
    assert!(
        error.contains("identity needs alias/reference/singleton provenance"),
        "{error}"
    );
}

#[test]
fn distinct_same_kind_value_scalars_never_lower_to_luau_value_equality() {
    for scalar_kind in ["const", "const_float", "const_str"] {
        let make_const = |out: &str| match scalar_kind {
            "const" => OpIR {
                kind: scalar_kind.to_string(),
                value: Some(1),
                out: Some(out.to_string()),
                ..OpIR::default()
            },
            "const_float" => OpIR {
                kind: scalar_kind.to_string(),
                f_value: Some(1.0),
                out: Some(out.to_string()),
                ..OpIR::default()
            },
            "const_str" => OpIR {
                kind: scalar_kind.to_string(),
                s_value: Some("equal".to_string()),
                out: Some(out.to_string()),
                ..OpIR::default()
            },
            _ => unreachable!(),
        };
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: format!("{scalar_kind}_identity"),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![
                    make_const("left"),
                    make_const("right"),
                    OpIR {
                        kind: "is".to_string(),
                        args: Some(vec!["left".to_string(), "right".to_string()]),
                        out: Some("same".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["same".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let function = &ir.functions[0];
        let plan = ScalarRepresentationPlan::for_function_ir_for_target(
            function,
            &crate::tir::target_info::TargetInfo::luau_release_fast(),
        );
        let error = compile_pipeline::validate_luau_identity_contract(function, &plan).unwrap_err();
        assert!(
            error.contains("identity needs alias/reference/singleton provenance"),
            "{scalar_kind}: {error}"
        );
    }
}

#[test]
fn singleton_reference_and_unknown_identity_classes_lower_only_exact_cases() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "exact_identity_classes".to_string(),
            params: vec!["unknown".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(1),
                    out: Some("truth_a".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(1),
                    out: Some("truth_b".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("none_a".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("none_b".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_new".to_string(),
                    args: Some(vec![]),
                    out: Some("left_ref".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_new".to_string(),
                    args: Some(vec![]),
                    out: Some("right_ref".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["truth_a".to_string(), "truth_b".to_string()]),
                    out: Some("bool_same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["none_a".to_string(), "none_b".to_string()]),
                    out: Some("none_same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["left_ref".to_string(), "right_ref".to_string()]),
                    out: Some("refs_same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is_not".to_string(),
                    args: Some(vec!["left_ref".to_string(), "left_ref".to_string()]),
                    out: Some("alias_different".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["unknown".to_string(), "left_ref".to_string()]),
                    out: Some("unknown_is_ref".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    args: Some(vec![
                        "bool_same".to_string(),
                        "none_same".to_string(),
                        "refs_same".to_string(),
                        "alias_different".to_string(),
                        "unknown_is_ref".to_string(),
                    ]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    assert!(
        source.contains("local bool_same: boolean = molt_rawequal(truth_a, truth_b)"),
        "{source}"
    );
    assert!(
        source.contains("local none_same: boolean = molt_rawequal(none_a, none_b)"),
        "{source}"
    );
    assert!(
        source.contains("local refs_same: boolean = molt_rawequal(left_ref, right_ref)"),
        "{source}"
    );
    assert!(
        source.contains("local alias_different: boolean = false"),
        "{source}"
    );
    assert!(
        source.contains("local unknown_is_ref: boolean = molt_rawequal(unknown, left_ref)"),
        "{source}"
    );
    for forbidden in [
        "truth_a == truth_b",
        "none_a == none_b",
        "left_ref == right_ref",
        "unknown == left_ref",
        "left_ref ~= left_ref",
    ] {
        assert!(
            !source.contains(forbidden),
            "identity must bypass __eq metamethod dispatch: {forbidden}\n{source}"
        );
    }
}

#[test]
fn identity_primitive_and_runtime_helpers_cannot_be_shadowed_by_user_symbols() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "shadow_helpers".to_string(),
            params: vec![
                "rawequal".to_string(),
                "molt_rawequal".to_string(),
                "molt_equal".to_string(),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "list_new".to_string(),
                    args: Some(vec![]),
                    out: Some("left".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["left".to_string(), "left".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["same".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    assert!(
        source.contains("local molt_rawequal = rawequal"),
        "{source}"
    );
    assert!(source.contains("rawequal: any"), "{source}");
    assert!(
        source.contains("_m_user_6d6f6c745f726177657175616c: any"),
        "{source}"
    );
    assert!(
        source.contains("_m_user_6d6f6c745f657175616c: any"),
        "{source}"
    );
    assert!(!source.contains("molt_rawequal: any"), "{source}");
    assert!(!source.contains("molt_equal: any"), "{source}");
}

#[test]
fn compiler_temporary_namespace_cannot_be_shadowed_by_user_symbols() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "temporary_collision".to_string(),
            params: vec![
                "__ok_1".to_string(),
                "__err_1".to_string(),
                "__idx_item".to_string(),
                "__closure_slot".to_string(),
                "xs".to_string(),
                "idx".to_string(),
                "slot".to_string(),
            ],
            param_types: Some(vec![
                "any".to_string(),
                "any".to_string(),
                "any".to_string(),
                "any".to_string(),
                "list".to_string(),
                "int".to_string(),
                "any".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(1),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "closure_store".to_string(),
                    args: Some(vec!["slot".to_string(), "value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "closure_load".to_string(),
                    args: Some(vec!["slot".to_string()]),
                    out: Some("loaded".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "pcall_wrap_begin".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["xs".to_string(), "idx".to_string()]),
                    out: Some("item".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "pcall_wrap_end".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["loaded".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile(&ir);
    for raw in ["__ok_1", "__err_1", "__idx_item", "__closure_slot"] {
        let encoded = sanitize_ident(raw);
        assert!(source.contains(&format!("{encoded}: any")), "{source}");
    }
    assert!(source.contains("local __ok_1, __err_1"), "{source}");
    assert!(source.contains("local __closure_slot"), "{source}");
    assert!(source.contains("local __idx_item"), "{source}");
    assert!(!source.contains("(__ok_1: any"), "{source}");
}

#[test]
fn identity_provenance_matrix_matches_the_formal_admission_table() {
    use IdentityLowering::{Constant, Direct, Reject};
    use IdentityProvenance::{Reference, Singleton, Unknown, ValueScalar};

    let provenances = [
        Singleton(ScalarKind::Bool),
        Singleton(ScalarKind::NoneValue),
        ValueScalar(ScalarKind::Int),
        ValueScalar(ScalarKind::Float),
        ValueScalar(ScalarKind::Str),
        Reference,
        Unknown,
    ];
    let expected = [
        [
            Direct,
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Direct,
        ],
        [
            Constant(false),
            Direct,
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Direct,
        ],
        [
            Constant(false),
            Constant(false),
            Reject,
            Constant(false),
            Constant(false),
            Constant(false),
            Reject,
        ],
        [
            Constant(false),
            Constant(false),
            Constant(false),
            Reject,
            Constant(false),
            Constant(false),
            Reject,
        ],
        [
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Reject,
            Constant(false),
            Reject,
        ],
        [
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Constant(false),
            Direct,
            Direct,
        ],
        [Direct, Direct, Reject, Reject, Reject, Direct, Reject],
    ];

    for (lhs_index, lhs) in provenances.iter().copied().enumerate() {
        for (rhs_index, rhs) in provenances.iter().copied().enumerate() {
            assert_eq!(
                identity_lowering_for_provenance(false, lhs, rhs),
                expected[lhs_index][rhs_index],
                "identity provenance cell ({lhs_index}, {rhs_index}) drifted"
            );
            assert_eq!(
                identity_lowering_for_provenance(true, lhs, rhs),
                Constant(true),
                "same-SSA identity must dominate provenance"
            );
        }
    }
}

#[test]
fn value_scalar_plus_unknown_identity_is_rejected() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "float_unknown_identity".to_string(),
            params: vec!["unknown".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_float".to_string(),
                    f_value: Some(1.0),
                    out: Some("float".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["float".to_string(), "unknown".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["same".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let error = LuauBackend::new().compile_checked(&ir).unwrap_err();
    assert!(error.contains("identity needs alias/reference/singleton provenance"));
}

#[test]
fn same_ssa_value_identity_is_constant_true_even_for_value_scalars() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "same_float_alias".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_float".to_string(),
                    f_value: Some(1.0),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["value".to_string(), "value".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["same".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile_checked(&ir).unwrap();
    assert!(source.contains("local same: boolean = true"), "{source}");
}

#[test]
fn representation_conversions_preserve_only_declared_results() {
    for kind in ["box", "unbox"] {
        for input in ["source_value", "none"] {
            for output in [None, Some("none"), Some("converted")] {
                let ir = SimpleIR {
                    functions: vec![FunctionIR {
                        name: "representation_conversion".to_string(),
                        ops: vec![
                            OpIR {
                                kind: "const_bool".to_string(),
                                value: Some(1),
                                out: Some("source_value".to_string()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: kind.to_string(),
                                args: Some(vec![input.to_string()]),
                                out: output.map(str::to_string),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".to_string(),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    }],
                    profile: None,
                };
                let source = LuauBackend::new()
                    .compile_checked(&ir)
                    .unwrap_or_else(|error| {
                        panic!("{kind} input={input} output={output:?}: {error}")
                    });
                if output == Some("converted") {
                    let value = if input == "none" {
                        "nil"
                    } else {
                        "source_value"
                    };
                    assert!(
                        source.contains(&format!("local converted = {value}")),
                        "{kind} input={input}: {source}"
                    );
                } else {
                    assert!(!source.contains("local converted"), "{kind}: {source}");
                }
                assert!(!source.contains("local none ="), "{kind}: {source}");
                assert!(!source.contains("local _ ="), "{kind}: {source}");
            }
        }
    }
}

#[test]
fn raw_integer_representation_handlers_preserve_declared_results_before_admission() {
    for kind in ["box_from_raw_int", "unbox_to_raw_int"] {
        for input in ["source_value", "none"] {
            for output in [None, Some("none"), Some("converted")] {
                let mut backend = LuauBackend::new();
                backend.emit_op(&OpIR {
                    kind: kind.to_string(),
                    args: Some(vec![input.to_string()]),
                    out: output.map(str::to_string),
                    ..OpIR::default()
                });
                assert!(backend.unsupported_ops.is_empty(), "{kind} {input}");
                if output == Some("converted") {
                    let value = if input == "none" {
                        "nil"
                    } else {
                        "source_value"
                    };
                    assert!(
                        backend
                            .output
                            .contains(&format!("local converted = {value}")),
                        "{kind} input={input}: {}",
                        backend.output
                    );
                } else {
                    assert!(backend.output.is_empty(), "{kind}: {}", backend.output);
                }
            }
        }
    }
}

#[test]
fn raw_integer_representation_conversions_require_arbitrary_precision_authority() {
    for kind in ["box_from_raw_int", "unbox_to_raw_int"] {
        for output in [None, Some("none"), Some("converted")] {
            let mut backend = LuauBackend::new();
            let error = backend
                .compile_checked(&SimpleIR {
                    functions: vec![FunctionIR {
                        name: "representation_conversion".to_string(),
                        ops: vec![
                            OpIR {
                                kind: "const_bool".to_string(),
                                value: Some(1),
                                out: Some("source_value".to_string()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: kind.to_string(),
                                args: Some(vec!["source_value".to_string()]),
                                out: output.map(str::to_string),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".to_string(),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    }],
                    profile: None,
                })
                .expect_err("raw integer bridges must not bypass source-target admission");
            assert!(error.contains(kind), "{kind}: {error}");
            assert!(
                error.contains("canonical arbitrary-precision value authority"),
                "{kind}: {error}"
            );
            assert!(backend.output.is_empty());
        }
    }
}

#[test]
fn representation_conversions_reject_malformed_operands_even_without_results() {
    for kind in ["box", "box_from_raw_int", "unbox", "unbox_to_raw_int"] {
        for args in [
            None,
            Some(vec![]),
            Some(vec!["source".into(), "extra".into()]),
        ] {
            for output in [None, Some("none"), Some("converted")] {
                let mut backend = LuauBackend::new();
                backend.emit_op(&OpIR {
                    kind: kind.to_string(),
                    args: args.clone(),
                    out: output.map(str::to_string),
                    ..OpIR::default()
                });
                assert!(backend.output.is_empty(), "{kind} {args:?} {output:?}");
                assert_eq!(
                    backend.unsupported_ops.len(),
                    1,
                    "{kind} {args:?} {output:?}"
                );
                assert!(backend.unsupported_ops[0].contains("requires exactly one operand"));
            }
        }
    }
}

#[test]
fn local_copy_sources_follow_canonical_field_roles() {
    for kind in ["load_var", "copy_var"] {
        for args in [None, Some(vec![])] {
            let mut backend = LuauBackend::new();
            backend.emit_op(&OpIR {
                kind: kind.to_string(),
                args,
                var: Some("source".to_string()),
                out: Some("result".to_string()),
                ..OpIR::default()
            });
            assert!(backend.unsupported_ops.is_empty(), "{kind}");
            assert!(
                backend.output.contains("local result = source"),
                "{kind}: {}",
                backend.output
            );
        }

        for metadata in [None, Some("unread_metadata".to_string())] {
            let mut backend = LuauBackend::new();
            backend.emit_op(&OpIR {
                kind: kind.to_string(),
                args: Some(vec!["source".to_string()]),
                var: metadata,
                out: Some("result".to_string()),
                ..OpIR::default()
            });
            assert!(backend.unsupported_ops.is_empty(), "{kind}");
            assert!(
                backend.output.contains("local result = source"),
                "{kind}: {}",
                backend.output
            );
            assert!(!backend.output.contains("unread_metadata"), "{kind}");
        }

        for (args, metadata) in [
            (None, None),
            (Some(vec![]), None),
            (Some(vec!["first".to_string(), "second".to_string()]), None),
            (
                Some(vec!["first".to_string(), "second".to_string()]),
                Some("unread_metadata".to_string()),
            ),
        ] {
            let mut backend = LuauBackend::new();
            backend.emit_op(&OpIR {
                kind: kind.to_string(),
                args,
                var: metadata,
                out: Some("result".to_string()),
                ..OpIR::default()
            });
            assert!(backend.output.is_empty(), "{kind}: {}", backend.output);
            assert_eq!(backend.unsupported_ops.len(), 1, "{kind}");
            assert!(
                backend.unsupported_ops[0].contains("exactly one source operand"),
                "{kind}: {:?}",
                backend.unsupported_ops
            );
        }
    }
}

#[test]
fn module_cache_effects_ignore_out_metadata() {
    for (kind, args, emitted_effect) in [
        (
            "module_cache_set",
            vec!["cache_key".to_string(), "module".to_string()],
            "molt_module_cache[cache_key] = module",
        ),
        (
            "module_cache_del",
            vec!["cache_key".to_string()],
            "molt_module_cache[cache_key] = nil",
        ),
    ] {
        for metadata in [None, Some("none".to_string()), Some("metadata".to_string())] {
            let mut backend = LuauBackend::new();
            backend.emit_op(&OpIR {
                kind: kind.to_string(),
                args: Some(args.clone()),
                out: metadata,
                ..OpIR::default()
            });
            assert!(backend.unsupported_ops.is_empty(), "{kind}");
            assert!(
                backend.output.contains(emitted_effect),
                "{kind}: {}",
                backend.output
            );
            assert!(
                !backend.output.contains("local none ="),
                "{kind}: {}",
                backend.output
            );
            assert!(
                !backend.output.contains("local metadata ="),
                "{kind}: {}",
                backend.output
            );
        }
    }
}

#[test]
fn structured_hoisting_ignores_local_copy_metadata() {
    for kind in ["load_var", "copy_var"] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: format!("{kind}_metadata_scope"),
                params: vec!["condition".to_string(), "actual".to_string()],
                ops: vec![
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["condition".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_int".to_string(),
                        value: Some(7),
                        out: Some("metadata_collision".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: kind.to_string(),
                        args: Some(vec!["actual".to_string()]),
                        var: Some("metadata_collision".to_string()),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["result".to_string()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            }],
            profile: None,
        };

        let source = LuauBackend::new().compile(&ir);
        assert!(
            source.contains("local metadata_collision: number = 7"),
            "{kind}: {source}"
        );
        assert!(source.contains("local result = actual"), "{kind}: {source}");
        assert!(
            !source.contains("local result = metadata_collision"),
            "{kind}: {source}"
        );
        assert!(
            !source.contains("\tlocal metadata_collision\n"),
            "{kind}: {source}"
        );
    }
}

#[test]
fn structured_hoisting_tracks_canonical_secondary_results() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "secondary_result_scope".to_string(),
            params: vec![
                "condition".to_string(),
                "iterator".to_string(),
                "sequence".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["condition".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "iter_next_unboxed".to_string(),
                    args: Some(vec!["iterator".to_string()]),
                    var: Some("var_result".to_string()),
                    out: Some("out_result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    args: Some(vec!["sequence".to_string(), "trailing_result".to_string()]),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy_var".to_string(),
                    args: Some(vec!["var_result".to_string()]),
                    out: Some("copied_var_result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy_var".to_string(),
                    args: Some(vec!["trailing_result".to_string()]),
                    out: Some("copied_trailing_result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile(&ir);
    assert!(source.contains("\tlocal trailing_result\n"), "{source}");
    assert!(source.contains("\tlocal var_result\n"), "{source}");
    assert!(
        source.contains("\t\ttrailing_result = __molt_unpacked_"),
        "{source}"
    );
    assert!(
        source.contains("\t\tvar_result = __next_out_result[1]"),
        "{source}"
    );
    assert!(!source.contains("\t\tlocal trailing_result ="), "{source}");
    assert!(!source.contains("\t\tlocal var_result ="), "{source}");
}

#[test]
fn structured_hoisting_ignores_out_metadata_collisions() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "out_metadata_scope".to_string(),
            params: ["condition", "container", "key", "value"]
                .map(str::to_string)
                .to_vec(),
            ops: vec![
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["condition".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_index".to_string(),
                    args: Some(["container", "key", "value"].map(str::to_string).to_vec()),
                    out: Some("metadata_collision".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".to_string(),
                    value: Some(7),
                    out: Some("metadata_collision".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("metadata_collision".to_string()),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };

    let source = LuauBackend::new().compile(&ir);
    assert!(
        source.contains("\tlocal metadata_collision: number = 7\n"),
        "{source}"
    );
    assert!(!source.contains("\tlocal metadata_collision\n"), "{source}");
    assert!(
        source.contains("local result = metadata_collision"),
        "{source}"
    );
}

#[test]
fn test_compile_checked_lowers_call_function_alias_without_shadowing_globals() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "call_function_alias_test".to_string(),
            params: vec!["arg".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "call_function".to_string(),
                    args: Some(vec!["print".to_string(), "arg".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(
        source.contains("molt_call_checked(print, arg)"),
        "call_function should call its first operand as the callable, got:\n{source}"
    );
    assert!(
        !source.contains("local print") && !source.contains("[unsupported op: call_function]"),
        "call_function must not shadow Luau globals or leave markers, got:\n{source}"
    );
}

#[test]
fn checked_guarded_call_uses_callable_identity_not_the_lexical_target_hint() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "target".to_string(),
                params: vec!["value".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "dispatch".to_string(),
                params: vec!["selected".to_string(), "value".to_string()],
                ops: vec![
                    OpIR {
                        kind: "call_guarded".to_string(),
                        s_value: Some("target".to_string()),
                        args: Some(vec!["selected".to_string(), "value".to_string()]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["result".to_string()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("checked guarded call");
    assert!(source.contains("molt_call_checked(selected, value)"));
    assert!(!source.contains("molt_call_checked(target, selected"));
}

#[test]
fn test_simple_function() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(42),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(output.contains("function molt_main()"));
    // v0 is a single-use constant inlined into the print call.
    assert!(output.contains("print(42)"));
}

#[test]
fn test_int_from_str_of_obj_preserves_base_operand() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![
                "value".to_string(),
                "base".to_string(),
                "has_base".to_string(),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "int_from_str_of_obj".to_string(),
                    args: Some(vec![
                        "value".to_string(),
                        "base".to_string(),
                        "has_base".to_string(),
                    ]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["out".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(output.contains("molt_bool(has_base)"));
    assert!(output.contains("tonumber(molt_str(value), molt_int(base))"));
}

#[test]
fn test_real_ir_ops() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "test_func".to_string(),
            params: vec!["p0".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_float".to_string(),
                    f_value: Some(std::f64::consts::PI),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("hello".to_string()),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["p0".to_string(), "v0".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".to_string(),
                    args: Some(vec!["v2".to_string(), "p0".to_string()]),
                    out: Some("v3".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v3".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(output.contains("local function test_func(p0: any)"));
    // v0 (3.14) is single-use, inlined into the add expression.
    // add emits a type-aware string/number ternary.
    assert!(
        output.contains("p0 + 3.14") || output.contains("3.14"),
        "Expected 3.14 inlined somewhere, got:\n{output}"
    );
    // After sink pass, v2 is inlined into the lt expression.
    assert!(
        output.contains("v2 < p0") || output.contains("< p0"),
        "Expected lt comparison with p0, got:\n{output}"
    );
    assert!(output.contains("return"));
}

#[test]
fn test_control_flow() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "flow_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "label".to_string(),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    // Numeric labels are transported by the shared logical graph, never
    // emitted as unsupported Luau gotos or deleted by source heuristics.
    assert!(output.contains("__molt_block"));
    assert!(
        !output.contains("-- ::label_0::"),
        "labels must not be comments"
    );
    assert!(!output.contains("-- goto"), "gotos must not be comments");
    // The function still compiles and returns.
    assert!(output.contains("return"));
}

#[test]
fn iterator_loop_preserves_explicit_pending_observer_and_handled_state() {
    let ops = vec![
        OpIR {
            kind: "iter".to_string(),
            out: Some("v_it".to_string()),
            args: Some(vec!["v_src".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_context_set".to_string(),
            args: Some(vec!["handled".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "iter_next".to_string(),
            out: Some("v_next".to_string()),
            args: Some(vec!["v_it".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            out: Some("v_exhausted".to_string()),
            args: Some(vec!["v_next".to_string(), "v_idx1".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_break_if_true".to_string(),
            args: Some(vec!["v_exhausted".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_break_if_exception".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            out: Some("v_value".to_string()),
            args: Some(vec!["v_next".to_string(), "v_idx0".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_local".to_string(),
            args: Some(vec!["v_sink".to_string(), "v_value".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "iterator_cleanup".into(),
            params: vec![
                "v_src".into(),
                "v_idx0".into(),
                "v_idx1".into(),
                "v_sink".into(),
                "handled".into(),
            ],
            ops,
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new().compile(&ir);
    assert!(source.contains("molt_iterator_new(v_src)"), "{source}");
    assert!(
        source.contains("molt_exception_context_set(handled)"),
        "{source}"
    );
    assert!(
        source.contains("if molt_exception_pending() then break end"),
        "{source}"
    );
    assert!(
        source.contains("molt_exception_capture(__molt_pcall_frame_context_"),
        "{source}"
    );
}

#[test]
fn test_compile_checked_accepts_sys_bootstrap_with_exact_integer_literals() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(3),
                    out: Some("major".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(14),
                    out: Some("minor".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(0),
                    out: Some("micro".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("final".to_string()),
                    out: Some("releaselevel".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(0),
                    out: Some("serial".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("3.14.0 (molt)".to_string()),
                    out: Some("version".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_internal".to_string(),
                    s_value: Some("molt_sys_set_version_info".to_string()),
                    args: Some(vec![
                        "major".to_string(),
                        "minor".to_string(),
                        "micro".to_string(),
                        "releaselevel".to_string(),
                        "serial".to_string(),
                        "version".to_string(),
                    ]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("bounded sys version bootstrap does not require Python import dispatch");
    assert!(source.contains("local major: number = 3"));
    assert!(source.contains("local minor: number = 14"));
    assert!(source.contains("local function molt_sys_set_version_info("));
    assert!(source.contains("local function molt_sys_seed_module()"));
}

#[test]
fn compile_checked_rejects_module_import_before_source_emission() {
    for (kind, literal, args) in [
        ("module_import", Some("sys"), vec![]),
        ("module_import", None, vec!["module_name"]),
        ("module_import_from", Some("path"), vec!["module"]),
        ("module_import_from", None, vec!["module", "member"]),
        ("module_import_star", None, vec!["module", "namespace"]),
    ] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "import_probe".to_string(),
                params: ["module_name", "module", "member", "namespace"]
                    .map(str::to_string)
                    .to_vec(),
                ops: vec![
                    OpIR {
                        kind: kind.to_string(),
                        s_value: literal.map(str::to_string),
                        args: Some(args.into_iter().map(str::to_string).collect()),
                        out: Some("import_result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["import_result".to_string()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let error = backend
            .compile_checked(&ir)
            .expect_err("Luau has no Python import protocol, including for sys");
        assert!(
            error.contains("rejected before source generation")
                && error.contains("module_import")
                && error.contains("Python import dispatch"),
            "{error}"
        );
        assert!(
            backend.output.is_empty(),
            "import refusal emitted partial source"
        );
        assert!(backend.unsupported_ops.is_empty());
    }
}

#[test]
fn compile_checked_materializes_all_exact_integer_literal_siblings_and_rejects_overflow() {
    let exact_ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(42),
                    out: Some("plain".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".to_string(),
                    value: Some(-(1_i64 << 53)),
                    out: Some("typed".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some((1_u64 << 53).to_string()),
                    out: Some("decimal".to_string()),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&exact_ir)
        .expect("every concrete literal spelling must share exact bounded admission");
    assert!(source.contains("local plain: number = 42"));
    assert!(source.contains("local typed: number = -9007199254740992"));
    assert!(source.contains("local decimal: number = 9007199254740992"));

    for payload in ["9007199254740993", "-9007199254740993"] {
        let overflow_ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some(payload.to_string()),
                    out: Some("overflow".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let error = LuauBackend::new()
            .compile_checked(&overflow_ir)
            .expect_err("Luau must not round an unsafe bigint literal");
        assert!(error.contains("exact concrete value authority"), "{error}");
    }
}

#[test]
fn test_compile_checked_rejects_undefined_label_targets() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "flow_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "label".to_string(),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("undefined labels must fail before source emission");
    assert!(
        error.contains("invalid-jump-target") && error.contains("undefined label 1"),
        "{error}"
    );
    assert!(
        backend.output.is_empty(),
        "graph rejection must precede source emission"
    );
}

#[test]
fn test_compile_checked_lowers_store_var_and_load_var() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "slot_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_int".to_string(),
                    out: Some("v0".to_string()),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("slot".to_string()),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    out: Some("v1".to_string()),
                    var: Some("slot".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v1".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(source.contains("\tlocal slot\n"));
    assert!(source.contains("\tslot = "));
    assert!(source.contains("return slot") || source.contains("local v1 = slot"));
    assert!(!source.contains("[unsupported op: store_var]"));
    assert!(!source.contains("[unsupported op: load_var]"));
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn store_var_result_snapshots_execute_in_structured_and_labelled_flow() {
    let float = |name: &str, value: f64| OpIR {
        kind: "const_float".to_string(),
        out: Some(name.to_string()),
        f_value: Some(value),
        ..OpIR::default()
    };
    let store = |destination: Option<&str>, result: Option<&str>, source: &str| OpIR {
        kind: "store_var".to_string(),
        var: destination.map(str::to_string),
        out: result.map(str::to_string),
        args: Some(vec![source.to_string()]),
        ..OpIR::default()
    };
    let op = |kind: &str, args: &[&str], out: Option<&str>| OpIR {
        kind: kind.to_string(),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| (*arg).to_string()).collect()),
        out: out.map(str::to_string),
        ..OpIR::default()
    };
    let body = |labelled: bool| {
        let mut ops = vec![
            float("first", 5.0),
            store(Some("slot"), Some("snapshot"), "first"),
            float("same_source", 11.0),
            store(Some("same"), Some("same"), "same_source"),
            float("binding_source", 13.0),
            store(Some("binding_slot"), Some("binding_only"), "binding_source"),
        ];
        if labelled {
            ops.push(OpIR {
                kind: "jump".to_string(),
                value: Some(1),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "label".to_string(),
                value: Some(1),
                ..OpIR::default()
            });
        }
        ops.extend([
            float("rebound", 7.0),
            store(Some("slot"), Some("none"), "rebound"),
            OpIR {
                kind: "load_var".to_string(),
                var: Some("slot".to_string()),
                out: Some("current".to_string()),
                ..OpIR::default()
            },
            op("add", &["snapshot", "current"], Some("sum_one")),
            op("add", &["sum_one", "same"], Some("sum_two")),
            op("add", &["sum_two", "binding_only"], Some("total")),
            op("ret", &["total"], None),
        ]);
        ops
    };
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                ops: vec![op("ret_void", &[], None)],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "structured_store_results".to_string(),
                ops: body(false),
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "labelled_store_results".to_string(),
                ops: body(true),
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "tuple_snapshot_after_rebind".to_string(),
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("text".to_string()),
                        s_value: Some("alpha".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("prefix".to_string()),
                        s_value: Some("al".to_string()),
                        ..OpIR::default()
                    },
                    op("tuple_new", &["prefix"], Some("prefixes")),
                    store(Some("tuple_slot"), Some("tuple_snapshot"), "prefixes"),
                    store(Some("tuple_slot"), Some("none"), "prefix"),
                    op(
                        "string_startswith",
                        &["text", "tuple_snapshot"],
                        Some("matched"),
                    ),
                    op("ret", &["matched"], None),
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let compiled = LuauBackend::new().compile(&ir);
    let destination_assignment = compiled.find("slot = ").expect("destination assignment");
    let snapshot_assignment = compiled
        .find("snapshot = slot")
        .expect("ordered result snapshot");
    assert!(destination_assignment < snapshot_assignment);
    assert!(!compiled.contains("local none ="));
    assert!(!compiled.contains("[unsupported op: store_var]"));
    let source = format!(
        "{compiled}\nassert(structured_store_results() == 36)\nassert(labelled_store_results() == 36)\nassert(tuple_snapshot_after_rebind() == true)\nprint(\"luau-store-var-results-ok\")\n"
    );
    validate_luau_source(&source).expect("result-carrying store_var source must validate");
    let output = execute_lune_oracle("store_var_results", &source);
    assert!(String::from_utf8_lossy(&output.stdout).contains("luau-store-var-results-ok"));
}

#[test]
fn store_var_rejects_reserved_destinations_without_admitting_store_fast() {
    for destination in ["", "none"] {
        let mut backend = LuauBackend::new();
        backend.emit_op(&OpIR {
            kind: "store_var".to_string(),
            var: Some(destination.to_string()),
            args: Some(vec!["source".to_string()]),
            ..OpIR::default()
        });
        assert!(
            backend
                .unsupported_ops
                .iter()
                .any(|failure| failure.contains("non-empty, non-reserved destination")),
            "{destination:?}: {:?}",
            backend.unsupported_ops
        );
    }

    let mut backend = LuauBackend::new();
    backend.emit_op(&OpIR {
        kind: "store_fast".to_string(),
        out: Some("binding_only".to_string()),
        args: Some(vec!["source".to_string()]),
        ..OpIR::default()
    });
    assert_eq!(backend.unsupported_ops, ["`store_fast` (luau backend)"]);
}

#[test]
fn test_compile_checked_lowers_missing_singleton() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "missing_singleton_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("first".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("second".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["first".to_string(), "second".to_string()]),
                    out: Some("same".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["same".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("local molt_missing_sentinel = {}"));
    assert!(source.contains("local first = molt_missing_sentinel"));
    assert!(source.contains("local second = molt_missing_sentinel"));
    assert!(!source.contains("-- [missing]"));
}

#[test]
fn test_compile_checked_rejects_python_frame_introspection_target_fact() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "frame_introspection_test".to_string(),
            params: vec!["depth".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "getframe".to_string(),
                out: Some("frame".to_string()),
                args: Some(vec!["depth".to_string()]),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("getframe requires Python-visible frame introspection");
    assert!(error.contains("Python-visible frame objects"), "{error}");
}

#[test]
fn compile_checked_rejects_every_python_frame_and_trace_intrinsic() {
    for symbol in [
        "molt_getframe",
        "molt_inspect_currentframe",
        "molt_sys_settrace",
        "molt_sys_gettrace",
        "molt_sys_setprofile",
        "molt_sys_getprofile",
    ] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: ExecutionContextPolicy::None,
                ops: vec![OpIR {
                    kind: "call_internal".to_string(),
                    s_value: Some(symbol.to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let error = LuauBackend::new()
            .compile_checked(&ir)
            .expect_err("frame/trace intrinsics require whole-program locals authority");
        assert!(
            error.contains("exact Python-visible frame objects"),
            "{symbol}: {error}"
        );
    }
}

#[test]
fn execution_frame_siblings_have_real_luau_lowering() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "local_frame".to_string(),
                params: vec!["locals".to_string()],
                execution_context: ExecutionContextPolicy::Local,
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("inherited_frame".to_string()),
                        args: Some(vec!["locals".to_string()]),
                        out: Some("none".to_string()),
                        passes_execution_context: true,
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "inherited_frame".to_string(),
                params: vec!["locals".to_string()],
                execution_context: ExecutionContextPolicy::Inherited,
                ops: vec![
                    OpIR {
                        kind: "line".to_string(),
                        value: Some(8),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "frame_locals_set".to_string(),
                        args: Some(vec!["locals".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };

    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("the complete Local/Inherited frame ABI must lower");
    assert!(source.contains("molt_frame_enter_slot(molt_code_slots[7])"));
    assert!(source.contains("molt_frame_exit(__molt_frame_context"));
    assert!(source.contains("inherited_frame(locals, __molt_frame_context)"));
    assert!(source.contains("molt_frame_set_line(__molt_frame_context, 8"));
    assert!(source.contains("molt_frame_locals_set(__molt_frame_context, locals)"));
    assert!(!source.contains("molt_frame_set_line(molt_frame_context()"));
    assert!(!source.contains("molt_frame_locals_set(molt_frame_context()"));
}

#[test]
fn luau_compiles_megafunction_chunks_with_one_local_frame_owner() {
    let mut ops = vec![OpIR {
        kind: "trace_enter_slot".to_string(),
        value: Some(3),
        ..OpIR::default()
    }];
    for line in 1..=6 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some(format!("v{line}")),
            ..OpIR::default()
        });
    }
    ops.extend([
        OpIR {
            kind: "trace_exit".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
    ]);
    let original = FunctionIR {
        name: "luau_framed_large".to_string(),
        execution_context: ExecutionContextPolicy::Local,
        ops,
        ..FunctionIR::default()
    };
    let mut occupied = std::collections::BTreeSet::from([original.name.clone()]);
    let (stub, chunks) = molt_tir::passes::split_large_function(original, 3, &mut occupied)
        .expect("expected Luau framed megafunction split");
    let chunk_names = chunks
        .iter()
        .map(|chunk| chunk.name.clone())
        .collect::<Vec<_>>();
    let source = LuauBackend::new()
        .compile_checked(&SimpleIR {
            functions: std::iter::once(stub).chain(chunks).collect(),
            profile: None,
        })
        .expect("Luau must compile the validated split execution-context ABI");
    for name in chunk_names {
        let emitted = emit_function_ident(&name);
        assert!(
            source.contains(&format!("{emitted} = function(__molt_frame_context: any)"))
                || source.contains(&format!(
                    "local function {emitted}(__molt_frame_context: any)"
                )),
            "{name}: {source}"
        );
    }
    assert_eq!(
        source
            .matches("molt_frame_enter_slot(molt_code_slots[3])")
            .count(),
        1
    );
}

#[test]
fn test_compile_checked_lowers_loop_exception_break_as_pending_observer() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "loop_exception_break_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_exception".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("ok".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["ok".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("structured loop observer must be admitted");

    assert!(
        source.contains("loop_exception_break_test"),
        "compiled loop exception-break function should be emitted, got:\n{source}"
    );
    assert!(
        source.contains("if molt_exception_pending() then break end"),
        "{source}"
    );
    assert!(
        !source.contains("[loop_break_if_exception]")
            && !source.contains("[unsupported op: loop_break_if_exception]"),
        "loop exception-break markers must not leave semantic stub markers, got:\n{source}"
    );
}

#[test]
fn unchecked_luau_code_slot_metadata_cannot_restore_an_ambient_frame_fallback() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "code_frame_metadata_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "code_slots_init".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    args: Some(vec![]),
                    out: Some("globals".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(1),
                    args: Some(vec!["code".to_string(), "globals".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("locals".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "frame_locals_set".to_string(),
                    args: Some(vec!["locals".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("ok".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["ok".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let checked_error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("None policy must reject frame_locals_set before source generation");
    assert!(checked_error.contains("without an execution context"));

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LuauBackend::new().compile(&ir)
    }))
    .expect_err("unchecked lowering must fail closed instead of inventing ambient frame state");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic");
    assert!(message.contains("without a Local or Inherited context"));
}

#[test]
fn compile_checked_accepts_terminal_drop_phase_markers_as_nonsemantic_artifacts() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "drop_artifact_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "drop_inserted".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_region_drops_inserted".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("phase-completion markers carry no deterministic lifetime operation");
    assert!(!source.contains("[unsupported op: drop_inserted]"));
    assert!(!source.contains("[unsupported op: exception_region_drops_inserted]"));
}

#[test]
fn checked_luau_rejects_real_rc_operations_but_dispatch_consumes_legacy_artifacts() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "drop_operation_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v0".to_string()),
                    s_value: Some("owned".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "inc_ref".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dec_ref".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "release".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("explicit RC operations require deterministic lifetime semantics");
    assert!(error.contains("deterministic Python lifetime/finalizer semantics"));

    let source = backend.compile(&ir);
    assert!(!source.contains("[unsupported op: inc_ref]"));
    assert!(!source.contains("[unsupported op: dec_ref]"));
    assert!(!source.contains("[unsupported op: release]"));
}

#[test]
fn test_compile_checked_lowers_shared_guard_tag_fact() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "guard_tag_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(7),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("int_tag".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "guard_tag".to_string(),
                    args: Some(vec!["value".to_string(), "int_tag".to_string()]),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(source.contains("local function molt_guard_type"));
    assert!(source.contains("molt_guard_type(value, int_tag)"));
    assert!(!source.contains("[unsupported op: guard_tag]"));
}

#[test]
fn test_compile_checked_lowers_exception_stack_depth_to_value() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "exception_depth_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("stack depth must lower to the coroutine-owned exception state");
    let function = source
        .split("local function exception_depth_test()")
        .nth(1)
        .expect("fixture function must be emitted");
    assert!(
        function.contains("molt_exception_stack_depth()"),
        "{function}"
    );
    assert!(
        function.contains("molt_exception_stack_set_depth("),
        "{function}"
    );
    assert!(!function.contains("local v0 = 0"), "{function}");
    assert!(!source.contains("[exception_stack_depth]"));
}

#[test]
fn test_compile_checked_lowers_iter_next_unboxed() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "iter_unboxed_test".to_string(),
            params: vec!["xs".to_string()],
            param_types: Some(vec!["list[int]".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "iter".to_string(),
                    out: Some("it".to_string()),
                    args: Some(vec!["xs".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "iter_next_unboxed".to_string(),
                    args: Some(vec!["it".to_string()]),
                    var: Some("value".to_string()),
                    out: Some("done".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(source.contains("local __next_done = it()"));
    assert!(source.contains("local done = __next_done[2]"));
    assert!(source.contains("local value = __next_done[1]"));
    assert!(!source.contains("[unsupported op: iter_next_unboxed]"));
}

#[test]
fn test_iter_next_unboxed_preserves_discarded_result_positions() {
    for discarded in [None, Some("none")] {
        let compile = |var: Option<&str>, out: Option<&str>| {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "iter_unboxed_discarded_result_test".to_string(),
                    params: vec!["xs".to_string()],
                    param_types: Some(vec!["list[int]".to_string()]),
                    source_file: None,
                    is_extern: false,
                    codegen_partition: false,
                    execution_context: ExecutionContextPolicy::None,
                    ops: vec![
                        OpIR {
                            kind: "iter".to_string(),
                            out: Some("it".to_string()),
                            args: Some(vec!["xs".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "iter_next_unboxed".to_string(),
                            args: Some(vec!["it".to_string()]),
                            var: var.map(str::to_string),
                            out: out.map(str::to_string),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                }],
                profile: None,
            };
            LuauBackend::new().compile(&ir)
        };

        let source = compile(discarded, Some("done"));
        assert!(source.contains("local __next_done = it()"));
        assert!(source.contains("local done = __next_done[2]"));
        assert!(!source.contains("local done = __next_done[1]"));

        let source = compile(Some("value"), discarded);
        assert!(source.contains("local __next_value = it()"));
        assert!(source.contains("local value = __next_value[1]"));
        assert!(!source.contains("local value = __next_value[2]"));
    }
}

#[test]
fn test_luau_tir_roundtrip_raise_catch_fails_closed_before_source() {
    let func: FunctionIR = serde_json::from_str(
            r#"{"name":"__main____raise_catch","ops":[{"kind":"trace_enter_slot","value":1},{"kind":"exception_stack_enter","out":"v107"},{"kind":"exception_stack_depth","out":"v108"},{"kind":"missing","out":"v109"},{"args":["v109"],"kind":"store_var","var":"caught"},{"kind":"check_exception","value":3},{"kind":"missing","out":"v110"},{"args":["v110"],"kind":"store_var","var":"i"},{"kind":"check_exception","value":3},{"args":["n"],"col_offset":4,"end_col_offset":14,"kind":"store_var","var":"n"},{"col_offset":4,"end_col_offset":14,"kind":"line","value":36},{"kind":"check_exception","value":3},{"kind":"const","out":"v111","value":0},{"args":["v111"],"col_offset":4,"end_col_offset":23,"kind":"store_var","var":"caught"},{"col_offset":4,"end_col_offset":23,"kind":"line","value":37},{"kind":"check_exception","value":3},{"kind":"const","out":"v112","value":0},{"kind":"const","out":"v113","value":1},{"args":["v112","n","v113"],"kind":"range_new","out":"v114"},{"kind":"check_exception","value":3},{"kind":"const","out":"v115","value":0},{"kind":"const","out":"v116","value":1},{"args":["v114"],"kind":"len","out":"v117"},{"kind":"check_exception","value":3},{"kind":"loop_start"},{"args":["v115"],"kind":"loop_index_start","out":"v118"},{"args":["v118","v117"],"fast_int":true,"kind":"lt","out":"v119"},{"kind":"check_exception","value":3},{"args":["v119"],"kind":"loop_break_if_false","type_hint":"bool"},{"args":["v114","v118"],"kind":"index","out":"v120"},{"kind":"check_exception","value":3},{"args":["v120"],"col_offset":8,"end_col_offset":23,"kind":"store_var","var":"i"},{"col_offset":8,"end_col_offset":23,"kind":"line","value":38},{"kind":"check_exception","value":3},{"kind":"exception_push","out":"none"},{"col_offset":12,"end_col_offset":31,"kind":"try_start","value":4},{"col_offset":12,"end_col_offset":31,"kind":"line","value":39},{"kind":"load_var","out":"v121","var":"i"},{"kind":"check_exception","value":4},{"args":["v121"],"kind":"exception_new_builtin_one","out":"v122","s_value":"ValueError","value":5},{"args":["v122"],"kind":"raise","out":"none"},{"kind":"jump","value":4},{"kind":"try_end","value":4},{"kind":"jump","value":6},{"kind":"label","value":4},{"kind":"exception_last_pending","out":"v123"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"kind":"exception_match_builtin","out":"v124","s_value":"ValueError","value":5},{"args":["v124"],"kind":"if","type_hint":"bool"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"col_offset":12,"end_col_offset":23,"kind":"exception_context_set","out":"none"},{"col_offset":12,"end_col_offset":23,"kind":"line","value":41},{"kind":"load_var","out":"v125","var":"caught"},{"kind":"const","out":"v126","value":1},{"args":["v125","v126"],"fast_int":true,"kind":"inplace_add","out":"v127"},{"args":["v127"],"kind":"store_var","var":"caught"},{"kind":"const_none","out":"v128"},{"args":["v128"],"kind":"exception_context_set","out":"none"},{"kind":"else"},{"args":["v123"],"kind":"raise","out":"none"},{"kind":"end_if"},{"kind":"jump","value":7},{"kind":"label","value":6},{"kind":"exception_pop","out":"none"},{"kind":"jump","value":8},{"kind":"label","value":7},{"kind":"exception_pop","out":"none"},{"kind":"check_exception","value":3},{"kind":"label","value":8},{"kind":"check_exception","value":3},{"args":["v118","v116"],"fast_int":true,"kind":"add","out":"v129"},{"kind":"check_exception","value":3},{"args":["v129"],"kind":"loop_index_next","out":"v118"},{"kind":"loop_continue"},{"col_offset":4,"end_col_offset":17,"kind":"loop_end"},{"col_offset":4,"end_col_offset":17,"kind":"line","value":42},{"kind":"load_var","out":"v130","var":"caught"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"args":["v130"],"kind":"ret"},{"kind":"label","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret_void"}],"param_types":["i64"],"params":["n"]}"#,
        )
        .expect("raise_catch frontend fixture should deserialize");
    let func = luau_tir_roundtrip_function(func);
    let mut backend = LuauBackend::new();
    let error = backend
        .compile_checked(&SimpleIR {
            functions: vec![func],
            profile: None,
        })
        .expect_err("Luau has no certified structured Python exception model");
    assert!(
        error.contains("SimpleIR validation failed")
            || error.contains("rejected before source generation"),
        "exception CFG must fail before source publication, got: {error}"
    );
}
