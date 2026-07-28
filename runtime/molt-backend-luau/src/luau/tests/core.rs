use super::exceptions::luau_tir_roundtrip_function;
use super::*;

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
fn control_flow_labels_share_the_injective_symbol_authority() {
    let mut backend = LuauBackend::new();
    for label in ["a-b", "a.b"] {
        assert!(backend.emit_control_op(&OpIR {
            kind: "label".to_string(),
            s_value: Some(label.to_string()),
            ..OpIR::default()
        }));
        assert!(backend.emit_control_op(&OpIR {
            kind: "jump".to_string(),
            s_value: Some(label.to_string()),
            ..OpIR::default()
        }));
    }
    assert!(backend.emit_control_op(&OpIR {
        kind: "label".to_string(),
        value: Some(1),
        ..OpIR::default()
    }));
    assert!(backend.emit_control_op(&OpIR {
        kind: "label".to_string(),
        s_value: Some("label_1".to_string()),
        ..OpIR::default()
    }));
    assert!(backend.output.contains("::_m_label_612d62::"));
    assert!(backend.output.contains("goto _m_label_612d62"));
    assert!(backend.output.contains("::_m_label_612e62::"));
    assert!(backend.output.contains("goto _m_label_612e62"));
    assert!(backend.output.contains("::label_1::"));
    assert!(backend.output.contains("::_m_label_6c6162656c5f31::"));
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
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("annotation-result".to_string()),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("result".to_string()),
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
fn callargs_codegen_uses_packed_positional_and_ordered_keyword_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "invoke".to_string(),
            params: vec!["func".to_string(), "value".to_string()],
            ops: vec![
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("builder".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_pos".to_string(),
                    args: Some(vec!["builder".to_string(), "value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_bind".to_string(),
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
    let source = LuauBackend::new().compile(&ir);
    assert!(source.contains("local builder: {any} = molt_callargs_new()"));
    assert!(source.contains("molt_callargs_push_pos(builder, value)"));
    assert!(source.contains("local result = molt_callargs_invoke(func, builder)"));
    assert!(source.contains("molt_call_checked = function"));
    assert!(source.contains("local function molt_callargs_expand_kwstar"));
    assert!(!source.contains("local function molt_equal"));
    assert!(!source.contains("molt_function_params"));
    assert!(source.contains(
        "local molt_function_metadata: {[any]: any} = setmetatable({}, {__mode = \"k\"})"
    ));
    assert!(source.contains(
        "local molt_func_attrs: {[any]: {[string]: any}} = setmetatable({}, {__mode = \"k\"})"
    ));
    assert!(source.contains("if value == func then molt_func_self_attr else value"));
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
            kind: "code_slot_set".to_string(),
            value: Some(3),
            args: Some(vec!["code".to_string()]),
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
    assert!(source.contains("local code = {__molt_code=true"));
    assert!(source.contains("molt_code_slots[3] = code"));
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
fn ordered_dict_runtime_executes_full_semantics_in_lune_when_available() {
    let runner = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .map(|home| {
            home.join("bin")
                .join(if cfg!(windows) { "lune.exe" } else { "lune" })
        })
        .or_else(|| {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(
                std::path::PathBuf::from(home)
                    .join(".cargo")
                    .join("bin")
                    .join(if cfg!(windows) { "lune.exe" } else { "lune" }),
            )
        });
    let Some(runner) = runner.filter(|path| path.is_file()) else {
        eprintln!("Lune unavailable; executable ordered-dict proof skipped");
        return;
    };

    let source = format!(
        "--!strict\nlocal molt_func_attrs = setmetatable({{}}, {{__mode=\"k\"}})\nlocal molt_function_metadata = setmetatable({{}}, {{__mode=\"k\"}})\nlocal molt_call_checked: (any, ...any) -> any\nlocal molt_equal: (any, any, any?) -> boolean\nlocal molt_sequence_length_key = {{}}\nlocal molt_sequence_kind_key = {{}}\nlocal function molt_sequence_len(sequence: {{any}}): number\n\tlocal packed = rawget(sequence, molt_sequence_length_key)\n\tif type(packed) == \"number\" then return packed end\n\treturn #sequence\nend\nlocal function molt_pack_sequence_kind(kind: string, ...): {{any}} local sequence = table.pack(...); rawset(sequence, molt_sequence_length_key, sequence.n); rawset(sequence, molt_sequence_kind_key, kind); rawset(sequence, \"n\", nil); return sequence end\nlocal function molt_pack_list(...): {{any}} return molt_pack_sequence_kind(\"list\", ...) end\nlocal function molt_pack_tuple(...): {{any}} return molt_pack_sequence_kind(\"tuple\", ...) end\n{}\nlocal math_floor = math.floor\n{}\n{}",
        format!(
            "local molt_binary_metadata = setmetatable({{}}, {{__mode=\"k\"}})\nlocal function molt_binary_new(kind: string, value: string): any local result = {{}}; molt_binary_metadata[result] = {{kind=kind, value=value}}; return result end\n{}{}{}",
            dict_runtime::DICT_CORE_RUNTIME,
            dict_runtime::CALLARGS_RUNTIME,
            dict_runtime::EQUALITY_REPR_RUNTIME
        ),
        include_str!("../../luau_json_prelude.luau"),
        r#"
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
assert(molt_sequence_len(molt_function_metadata[bound_defaults].defaults) == 2)
assert(molt_callargs_invoke(bound_defaults, molt_callargs_new()) == 22)
local bound_override = molt_callargs_new(); molt_callargs_push_kw(bound_override, "y", 2)
assert(molt_callargs_invoke(bound_defaults, bound_override) == 17)
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
local weak_self_sentinel = {}
local function molt_func_attr_set(func, name, value)
	local attrs = molt_func_attrs[func]
	if attrs == nil then attrs = {}; molt_func_attrs[func] = attrs end
	rawset(attrs, name, if value == func then weak_self_sentinel else value)
end
local function install_ephemeral()
	local function ephemeral(value) return value end
	molt_function_metadata[ephemeral] = {arg_names=molt_pack_tuple("value"), posonly=0, kwonly=molt_pack_tuple(), vararg=nil, varkw=nil, defaults=nil, kwdefaults=nil}
	molt_func_attr_set(ephemeral, "__self_cycle", ephemeral)
	assert(molt_func_attrs[ephemeral].__self_cycle == weak_self_sentinel)
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
"#
    );
    let runtime_bytes = dict_runtime::DICT_CORE_RUNTIME.len()
        + dict_runtime::CALLARGS_RUNTIME.len()
        + dict_runtime::EQUALITY_REPR_RUNTIME.len();
    let prelude_bytes = include_str!("../../luau_json_prelude.luau").len();
    assert!(
        runtime_bytes < 43_000,
        "Luau container/call runtime grew to {runtime_bytes} bytes"
    );
    eprintln!(
        "luau-source-size runtime_bytes={runtime_bytes} prelude_bytes={prelude_bytes} oracle_bytes={}",
        source.len()
    );
    let path = std::env::temp_dir().join(format!(
        "molt_luau_ordered_dict_{}.luau",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write executable Luau proof");
    let output = std::process::Command::new(&runner)
        .arg("run")
        .arg(&path)
        .output()
        .expect("run Lune ordered-dict proof");
    if output.status.success() {
        let _ = std::fs::remove_file(&path);
    }
    assert!(
        output.status.success(),
        "Lune ordered-dict proof failed:\nstdout:\n{}\nstderr:\n{}\nsource: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        path.display()
    );
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

        let error = compile_pipeline::validate_luau_identity_contract(&ir).unwrap_err();
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
fn test_compile_checked_lowers_call_function_alias_without_shadowing_globals() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "call_function_alias_test".to_string(),
            params: vec!["arg".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
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
fn test_simple_function() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    var: Some("out".to_string()),
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
    // The dead goto/label stripping pass removes:
    //   - label_0 (orphaned: no goto targets it)
    //   - goto label_1 + label_1 (dead: goto jumps to immediately next label)
    // This is correct — the optimiser eliminates redundant control flow.
    // Verify they are NOT emitted as comments (the old Bug 4 regression).
    assert!(
        !output.contains("-- ::label_0::"),
        "labels must not be comments"
    );
    assert!(!output.contains("-- goto"), "gotos must not be comments");
    // The function still compiles and returns.
    assert!(output.contains("return"));
}

#[test]
fn test_lower_iter_to_for_requires_exhaustion_break_condition() {
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
            args: Some(vec!["v_other_cond".to_string()]),
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

    let lowered = lower_iter_to_for(&ops);
    assert!(
        lowered.iter().any(|op| op.kind == "iter"),
        "iter op should be preserved when break guard is unrelated"
    );
    assert!(
        !lowered.iter().any(|op| op.kind == "for_iter"),
        "unsafe iterator rewrite should not fire"
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
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("sys".to_string()),
                    out: Some("sys_name".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_import".to_string(),
                    args: Some(vec!["sys_name".to_string()]),
                    out: Some("sys_module".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    args: Some(vec!["sys_module".to_string()]),
                    s_value: Some("version_info".to_string()),
                    out: Some("version_info".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    args: Some(vec!["sys_module".to_string()]),
                    s_value: Some("hexversion".to_string()),
                    out: Some("hexversion".to_string()),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("bounded sys bootstrap literals and the Luau module model are exact");
    assert!(source.contains("local major: number = 3"));
    assert!(source.contains("local minor: number = 14"));
    assert!(source.contains("molt_sys_set_version_info("));
    assert!(source.contains("local sys_module = molt_luau_import_module(sys_name)"));
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
fn test_compile_checked_accepts_label_goto_comments() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "flow_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    // Labels and gotos emit as real Luau control flow, then the dead
    // goto/label stripping pass removes unreachable ones.  The key
    // correctness property is that they are NOT emitted as comments.
    let source = backend.compile(&ir);
    assert!(
        !source.contains("-- ::label_0::"),
        "labels must not be comments"
    );
    assert!(!source.contains("-- goto"), "gotos must not be comments");
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
fn test_compile_checked_lowers_missing_singleton() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "missing_singleton_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
fn test_compile_checked_lowers_luau_process_target_facts() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "process_target_facts_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "getargv".to_string(),
                    out: Some("argv".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "sys_executable".to_string(),
                    out: Some("executable".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("depth".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "getframe".to_string(),
                    out: Some("frame".to_string()),
                    args: Some(vec!["depth".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    args: Some(vec![
                        "argv".to_string(),
                        "executable".to_string(),
                        "frame".to_string(),
                    ]),
                    out: Some("facts".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["facts".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("local argv = {}"));
    assert!(source.contains("local executable = \"\""));
    assert!(source.contains("local frame = nil"));
    assert!(!source.contains("-- [getargv]"));
    assert!(!source.contains("-- [sys_executable]"));
    assert!(!source.contains("-- [getframe]"));
}

#[test]
fn test_compile_checked_lowers_trace_markers_as_luau_noops() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "trace_marker_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "trace_enter_slot".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "trace_exit".to_string(),
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
    let source = backend.compile(&ir);

    assert!(
        source.contains("trace_marker_test"),
        "compiled trace marker function should be emitted, got:\n{source}"
    );
    assert!(
        !source.contains("[internal: trace_enter_slot]")
            && !source.contains("[internal: trace_exit]")
            && !source.contains("[unsupported op: trace_enter_slot]")
            && !source.contains("[unsupported op: trace_exit]"),
        "trace markers must not leave semantic stub markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_loop_exception_break_as_luau_noop() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "loop_exception_break_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let source = backend.compile(&ir);

    assert!(
        source.contains("loop_exception_break_test"),
        "compiled loop exception-break function should be emitted, got:\n{source}"
    );
    assert!(
        !source.contains("[loop_break_if_exception]")
            && !source.contains("[unsupported op: loop_break_if_exception]"),
        "loop exception-break markers must not leave semantic stub markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_code_slot_metadata_to_reachable_luau_state() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "code_frame_metadata_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    kind: "code_slot_set".to_string(),
                    value: Some(1),
                    args: Some(vec!["code".to_string()]),
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
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(
        source.contains("code_frame_metadata_test"),
        "compiled code/frame metadata function should be emitted, got:\n{source}"
    );
    assert!(source.contains("molt_code_slots = table.create(2)"));
    assert!(source.contains("molt_code_slots[1] = code"));
    assert!(
        !source.contains("[internal: code_slots_init]")
            && !source.contains("[internal: code_slot_set]")
            && !source.contains("[internal: frame_locals_set]")
            && !source.contains("[unsupported op: code_slots_init]")
            && !source.contains("[unsupported op: code_slot_set]")
            && !source.contains("[unsupported op: frame_locals_set]"),
        "code/frame metadata must not leave semantic stub markers, got:\n{source}"
    );
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
    let source = backend.compile(&ir);
    assert!(source.contains("\tlocal v0 = 0\n"));
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
fn test_luau_tir_roundtrip_raise_catch_fails_closed_before_source() {
    let func: FunctionIR = serde_json::from_str(
            r#"{"name":"__main____raise_catch","ops":[{"kind":"trace_enter_slot","value":1},{"kind":"exception_stack_enter","out":"v107"},{"kind":"exception_stack_depth","out":"v108"},{"kind":"missing","out":"v109"},{"args":["v109"],"kind":"store_var","var":"caught"},{"kind":"check_exception","value":3},{"kind":"missing","out":"v110"},{"args":["v110"],"kind":"store_var","var":"i"},{"kind":"check_exception","value":3},{"args":["n"],"col_offset":4,"end_col_offset":14,"kind":"store_var","var":"n"},{"col_offset":4,"end_col_offset":14,"kind":"line","value":36},{"kind":"check_exception","value":3},{"kind":"const","out":"v111","value":0},{"args":["v111"],"col_offset":4,"end_col_offset":23,"kind":"store_var","var":"caught"},{"col_offset":4,"end_col_offset":23,"kind":"line","value":37},{"kind":"check_exception","value":3},{"kind":"const","out":"v112","value":0},{"kind":"const","out":"v113","value":1},{"args":["v112","n","v113"],"kind":"range_new","out":"v114"},{"kind":"check_exception","value":3},{"kind":"const","out":"v115","value":0},{"kind":"const","out":"v116","value":1},{"args":["v114"],"kind":"len","out":"v117"},{"kind":"check_exception","value":3},{"kind":"loop_start"},{"args":["v115"],"kind":"loop_index_start","out":"v118"},{"args":["v118","v117"],"fast_int":true,"kind":"lt","out":"v119"},{"kind":"check_exception","value":3},{"args":["v119"],"kind":"loop_break_if_false","type_hint":"bool"},{"args":["v114","v118"],"kind":"index","out":"v120"},{"kind":"check_exception","value":3},{"args":["v120"],"col_offset":8,"end_col_offset":23,"kind":"store_var","var":"i"},{"col_offset":8,"end_col_offset":23,"kind":"line","value":38},{"kind":"check_exception","value":3},{"kind":"exception_push","out":"none"},{"col_offset":12,"end_col_offset":31,"kind":"try_start","value":4},{"col_offset":12,"end_col_offset":31,"kind":"line","value":39},{"kind":"load_var","out":"v121","var":"i"},{"kind":"check_exception","value":4},{"args":["v121"],"kind":"exception_new_builtin_one","out":"v122","s_value":"ValueError","value":5},{"args":["v122"],"kind":"raise","out":"none"},{"kind":"jump","value":4},{"kind":"try_end","value":4},{"kind":"jump","value":6},{"kind":"label","value":4},{"kind":"exception_last_pending","out":"v123"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"kind":"exception_match_builtin","out":"v124","s_value":"ValueError","value":5},{"args":["v124"],"kind":"if","type_hint":"bool"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"col_offset":12,"end_col_offset":23,"kind":"exception_context_set","out":"none"},{"col_offset":12,"end_col_offset":23,"kind":"line","value":41},{"kind":"load_var","out":"v125","var":"caught"},{"kind":"const","out":"v126","value":1},{"args":["v125","v126"],"fast_int":true,"kind":"inplace_add","out":"v127"},{"args":["v127"],"kind":"store_var","var":"caught"},{"kind":"const_none","out":"v128"},{"args":["v128"],"kind":"exception_context_set","out":"none"},{"kind":"else"},{"args":["v123"],"kind":"raise","out":"none"},{"kind":"end_if"},{"kind":"jump","value":7},{"kind":"label","value":6},{"kind":"exception_pop","out":"none"},{"kind":"jump","value":8},{"kind":"label","value":7},{"kind":"exception_pop","out":"none"},{"kind":"check_exception","value":3},{"kind":"label","value":8},{"kind":"check_exception","value":3},{"args":["v118","v116"],"fast_int":true,"kind":"add","out":"v129"},{"kind":"check_exception","value":3},{"args":["v129"],"kind":"loop_index_next","out":"v118"},{"kind":"loop_continue"},{"col_offset":4,"end_col_offset":17,"kind":"loop_end"},{"col_offset":4,"end_col_offset":17,"kind":"line","value":42},{"kind":"load_var","out":"v130","var":"caught"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret","var":"v130"},{"kind":"label","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret_void"}],"param_types":["i64"],"params":["n"]}"#,
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
