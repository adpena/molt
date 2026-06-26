use super::*;

#[test]
fn test_bool_arithmetic_coerces_bool_operands() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "bool_arithmetic".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(1),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(0),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "sub".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v3".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "mul".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v4".to_string()),
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
    assert!(
        output.contains("then 1 else 0"),
        "bool operands must be numerically coerced in arithmetic, got:\n{output}"
    );
    assert!(
        !output.contains("true + false"),
        "bool addition must not emit raw Luau boolean arithmetic, got:\n{output}"
    );
}

#[test]
fn test_result_type_hint_does_not_prove_luau_not_operand_bool() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "truthy_not".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "not".to_string(),
                    args: Some(vec!["x".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("bool".to_string()),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("not molt_bool(x)"),
        "result-side type_hint=bool must not bypass Python truthiness for not, got:\n{output}"
    );
    assert!(
        !output.contains("not x"),
        "unknown operands must not use raw Luau boolean not, got:\n{output}"
    );
}

#[test]
fn test_result_type_hint_does_not_prove_luau_and_or_operands_bool() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "truthy_and_or".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "and".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("bool".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "or".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("v1".to_string()),
                    type_hint: Some("bool".to_string()),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("if molt_bool(a) then b else a"),
        "and must preserve Python value-returning truthiness for unknown operands, got:\n{output}"
    );
    assert!(
        output.contains("if molt_bool(a) then a else b"),
        "or must preserve Python value-returning truthiness for unknown operands, got:\n{output}"
    );
    assert!(
        !output.contains("local v0 = a and b") && !output.contains("local v1 = a or b"),
        "result-side type_hint=bool must not select native Luau and/or, got:\n{output}"
    );
}

#[test]
fn test_result_type_hint_does_not_force_luau_numeric_add() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hinted_add".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("int".to_string()),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("if type(a) == \"string\" or type(b) == \"string\""),
        "unknown add operands must keep Python string-concat guard, got:\n{output}"
    );
    assert!(
        !output.contains("local v0: number ="),
        "result-side type_hint=int must not force numeric add lowering, got:\n{output}"
    );
}

#[test]
fn test_transport_hints_do_not_force_luau_numeric_add() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "transport_hinted_add".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("v0".to_string()),
                    fast_int: Some(true),
                    fast_float: Some(true),
                    type_hint: Some("int".to_string()),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("if type(a) == \"string\" or type(b) == \"string\""),
        "transport hints must not bypass unknown add guard, got:\n{output}"
    );
    assert!(
        !output.contains("local v0: number ="),
        "transport hints must not select numeric add lowering, got:\n{output}"
    );
}

#[test]
fn test_type_hint_int_does_not_force_luau_integer_index() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hinted_index".to_string(),
            params: vec!["xs".to_string(), "key".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["xs".to_string(), "key".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("int".to_string()),
                    fast_int: Some(true),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("if type(key) == \"number\""),
        "unknown key must keep dynamic key normalization, got:\n{output}"
    );
    assert!(
        !output.contains("xs[if key >= 0 then key + 1"),
        "transport hints must not select integer-only indexing, got:\n{output}"
    );
}

#[test]
fn test_container_transport_hints_do_not_force_luau_list_dispatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hinted_container_index".to_string(),
            params: vec!["xs".to_string(), "key".to_string(), "value".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["xs".to_string(), "key".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("list".to_string()),
                    container_type: Some("list".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_item".to_string(),
                    args: Some(vec![
                        "xs".to_string(),
                        "key".to_string(),
                        "value".to_string(),
                    ]),
                    type_hint: Some("list".to_string()),
                    container_type: Some("list".to_string()),
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
    let output = backend.compile(&ir);

    assert!(
        output.contains("if type(key) == \"number\""),
        "unknown container must keep dynamic key normalization, got:\n{output}"
    );
    assert!(
        !output.contains("rawget(xs") && !output.contains("rawset(xs"),
        "transport hints must not select raw list dispatch, got:\n{output}"
    );
    assert!(
        !output.contains("list index out of range")
            && !output.contains("list assignment index out of range"),
        "transport hints must not select list bounds-guard path, got:\n{output}"
    );
}

#[test]
fn test_len_transport_hint_does_not_force_luau_raw_length() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hinted_len".to_string(),
            params: vec!["xs".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "len".to_string(),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("n".to_string()),
                    type_hint: Some("list".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["n".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local n = molt_len(xs)"),
        "unknown len operand must stay on runtime len, got:\n{output}"
    );
    assert!(
        !output.contains("local n = #xs"),
        "result-side type_hint must not select raw Luau length, got:\n{output}"
    );
}

#[test]
fn test_len_uses_tir_container_fact_for_luau_raw_length() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "typed_len".to_string(),
            params: vec!["xs".to_string()],
            param_types: Some(vec!["list[int]".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "len".to_string(),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("n".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["n".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local n = #xs"),
        "typed list len should use raw Luau length, got:\n{output}"
    );
    assert!(
        !output.contains("local n = molt_len(xs)"),
        "typed list len should not call runtime len, got:\n{output}"
    );
}

#[test]
fn test_typed_list_truthiness_uses_luau_raw_length_for_not() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "typed_list_not".to_string(),
            params: vec!["xs".to_string()],
            param_types: Some(vec!["list[int]".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "not".to_string(),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("empty".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["empty".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local empty: boolean = not (#xs > 0)"),
        "typed list truthiness should use raw Luau length for not, got:\n{output}"
    );
    assert!(
        !output.contains("not molt_bool(xs)"),
        "typed list truthiness should not call runtime bool for not, got:\n{output}"
    );
}

#[test]
fn test_typed_dict_truthiness_uses_luau_next_for_or() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "typed_dict_or".to_string(),
            params: vec!["d".to_string(), "fallback".to_string()],
            param_types: Some(vec![
                "dict[str, int]".to_string(),
                "dict[str, int]".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "or".to_string(),
                    args: Some(vec!["d".to_string(), "fallback".to_string()]),
                    out: Some("selected".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["selected".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local selected = if (next(d) ~= nil) then d else fallback"),
        "typed dict truthiness should use raw Luau next() for or, got:\n{output}"
    );
    assert!(
        !output.contains("molt_bool(d)"),
        "typed dict truthiness should not call runtime bool for or, got:\n{output}"
    );
}

#[test]
fn test_list_and_string_get_item_emit_index_error_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "index_guards".to_string(),
            params: vec!["xs".to_string(), "s".to_string(), "i".to_string()],
            param_types: Some(vec![
                "list[int]".to_string(),
                "str".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["xs".to_string(), "i".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("list".to_string()),
                    fast_int: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["s".to_string(), "i".to_string()]),
                    out: Some("v1".to_string()),
                    type_hint: Some("str".to_string()),
                    fast_int: Some(true),
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
    assert!(
        output.contains("__type=\"IndexError\""),
        "list/string indexing must guard out-of-range accesses, got:\n{output}"
    );
    assert!(
        output.contains("list index out of range") && output.contains("string index out of range"),
        "expected list and string IndexError messages, got:\n{output}"
    );
}

#[test]
fn test_string_get_item_uses_utf8_codepoint_offsets() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_index".to_string(),
            params: vec!["s".to_string(), "i".to_string()],
            param_types: Some(vec!["str".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "get_item".to_string(),
                    args: Some(vec!["s".to_string(), "i".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("str".to_string()),
                    fast_int: Some(true),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("molt_str_byte_offset(s, __idx_v0)")
            && output.contains("utf8.offset(s, __idx_v0 + 1)"),
        "string indexing must translate codepoint index to byte offsets, got:\n{output}"
    );
    assert!(
        !output.contains("string.sub(s, __idx_v0, __idx_v0)"),
        "string indexing must not fall back to byte-indexed substring extraction, got:\n{output}"
    );
}

#[test]
fn test_ord_at_emits_utf8_codepoint_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "ord_at_unicode".to_string(),
            params: vec!["s".to_string(), "i".to_string()],
            param_types: Some(vec!["str".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "ord_at".to_string(),
                    args: Some(vec!["s".to_string(), "i".to_string()]),
                    out: Some("v0".to_string()),
                    type_hint: Some("int".to_string()),
                    fast_int: Some(true),
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
    let output = backend.compile(&ir);
    assert!(
        output.contains("local function molt_ord_at")
            && output.contains("molt_ord_at(s, i)")
            && output.contains("utf8.codepoint(obj, byte_idx)")
            && output.contains("molt_str_codepoint_len(obj)"),
        "ord_at must use the shared UTF-8 codepoint helper path, got:\n{output}"
    );
}

#[test]
fn test_list_set_and_delete_emit_index_error_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "mutation_index_guards".to_string(),
            params: vec!["xs".to_string(), "i".to_string(), "v".to_string()],
            param_types: Some(vec![
                "list[int]".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "set_item".to_string(),
                    args: Some(vec!["xs".to_string(), "i".to_string(), "v".to_string()]),
                    type_hint: Some("list".to_string()),
                    fast_int: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "del_item".to_string(),
                    args: Some(vec!["xs".to_string(), "i".to_string()]),
                    type_hint: Some("list".to_string()),
                    fast_int: Some(true),
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
    assert!(
        output.contains("list assignment index out of range")
            && output.contains("list deletion index out of range"),
        "list set/delete must guard out-of-range accesses, got:\n{output}"
    );
}

#[test]
fn test_list_pop_and_index_emit_python_error_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_method_guards".to_string(),
            params: vec!["xs".to_string(), "i".to_string(), "needle".to_string()],
            param_types: Some(vec![
                "list[int]".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "list_pop".to_string(),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_pop".to_string(),
                    args: Some(vec!["xs".to_string(), "i".to_string()]),
                    out: Some("v1".to_string()),
                    fast_int: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_index".to_string(),
                    args: Some(vec!["xs".to_string(), "needle".to_string()]),
                    out: Some("v2".to_string()),
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
    assert!(
        output.contains("pop from empty list")
            && output.contains("pop index out of range")
            && output.contains("is not in list"),
        "list pop/index must emit Python error guards, got:\n{output}"
    );
}

#[test]
fn test_call_method_list_pop_uses_python_error_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_call_method_pop_guards".to_string(),
            params: vec!["xs".to_string(), "i".to_string()],
            param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("pop".to_string()),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("pop".to_string()),
                    args: Some(vec!["xs".to_string(), "i".to_string()]),
                    out: Some("v1".to_string()),
                    fast_int: Some(true),
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
    assert!(
        output.contains("pop from empty list") && output.contains("pop index out of range"),
        "list method pop must share direct list_pop Python guards, got:\n{output}"
    );
}

#[test]
fn test_call_method_list_count_and_index_use_collection_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_call_method_count_index".to_string(),
            params: vec![
                "xs".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "stop".to_string(),
            ],
            param_types: Some(vec![
                "list[int]".to_string(),
                "int".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("count".to_string()),
                    args: Some(vec!["xs".to_string(), "needle".to_string()]),
                    out: Some("count".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("index".to_string()),
                    args: Some(vec!["xs".to_string(), "needle".to_string()]),
                    out: Some("idx".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("index".to_string()),
                    args: Some(vec![
                        "xs".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                    ]),
                    out: Some("start_only".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("index".to_string()),
                    args: Some(vec![
                        "xs".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "stop".to_string(),
                    ]),
                    out: Some("bounded".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("missing_stop".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_index_range".to_string(),
                    args: Some(vec![
                        "xs".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "missing_stop".to_string(),
                    ]),
                    out: Some("missing_stop_index".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("custom".to_string()),
                    args: Some(vec!["xs".to_string()]),
                    out: Some("custom_result".to_string()),
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
    assert!(
        output.contains("for _, __v in ipairs(xs)")
            && output.contains("count = count + 1")
            && output.contains("local idx = -1")
            && output.contains("local start_only = -1")
            && output.contains("local bounded = -1")
            && output.contains("local missing_stop_index = -1")
            && output.contains("__start")
            && output.contains("__stop")
            && output.contains("local __stop = __n")
            && output.contains("__raw_stop == molt_missing_sentinel then __stop = __n"),
        "list method count/index must share direct collection lowering, got:\n{output}"
    );
    assert!(
        !output.contains("xs:count")
            && !output.contains("xs:index")
            && !output.contains("molt_get_attr(xs, \"count\")")
            && !output.contains("molt_get_attr(xs, \"index\")"),
        "typed list count/index must not use generic method lookup, got:\n{output}"
    );
    assert!(
        output.contains("molt_get_attr(xs, \"custom\")") && !output.contains("xs:custom"),
        "unknown typed list methods must fall through to generic method lookup, got:\n{output}"
    );
}

#[test]
fn test_list_index_range_honors_start_stop_bounds() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_index_range_bounds".to_string(),
            params: vec![
                "xs".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "stop".to_string(),
            ],
            param_types: Some(vec![
                "list[int]".to_string(),
                "int".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "list_index_range".to_string(),
                    args: Some(vec![
                        "xs".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "stop".to_string(),
                    ]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__start")
            && output.contains("__stop")
            && output.contains("local __raw_start = start")
            && output.contains("local __raw_stop = stop")
            && output.contains("__raw_start == molt_missing_sentinel then __start = 0")
            && output.contains("__raw_stop == molt_missing_sentinel then __stop = __n")
            && output.contains("__n + __raw_start")
            && output.contains("__n + __raw_stop")
            && output.contains("for __i = __start + 1, __stop do"),
        "list.index(value, start, stop) must honor range bounds, got:\n{output}"
    );
}

#[test]
fn test_dict_popitem_emits_empty_dict_key_error_guard() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dict_popitem_guard".to_string(),
            params: vec!["d".to_string()],
            param_types: Some(vec!["dict[str, int]".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "dict_popitem".to_string(),
                    args: Some(vec!["d".to_string()]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__type=\"KeyError\"") && output.contains("popitem(): dictionary is empty"),
        "dict.popitem must guard empty dictionaries, got:\n{output}"
    );
}

#[test]
fn test_list_insert_clamps_python_index_bounds() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_insert_clamps".to_string(),
            params: vec!["xs".to_string(), "i".to_string(), "v".to_string()],
            param_types: Some(vec![
                "list[int]".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "list_insert".to_string(),
                    args: Some(vec!["xs".to_string(), "i".to_string(), "v".to_string()]),
                    fast_int: Some(true),
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
    assert!(
        output.contains("__idx < 1")
            && output.contains("__idx = 1")
            && output.contains("__idx > #xs + 1")
            && output.contains("xs[#xs + 1] = v"),
        "list.insert must clamp Python indices before mutation, got:\n{output}"
    );
}

#[test]
fn test_list_extend_uses_table_move_fast_path() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_extend_fast_path".to_string(),
            params: vec!["dst".to_string(), "src".to_string()],
            param_types: Some(vec!["list[int]".to_string(), "list[int]".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "list_extend".to_string(),
                    args: Some(vec!["dst".to_string(), "src".to_string()]),
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
    assert!(
        output.contains("table.move(src, 1, #src, #dst + 1, dst)")
            && !output.contains("for __i = 1, #src"),
        "list_extend must use Luau table.move fast path, got:\n{output}"
    );
}

#[test]
fn test_list_repeat_clamps_negative_count_to_empty() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "list_repeat_clamps".to_string(),
            params: vec!["value".to_string(), "count".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "list_repeat_range".to_string(),
                    args: Some(vec!["value".to_string(), "count".to_string()]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("math.max(0, count)"),
        "list repetition must clamp negative counts to empty list, got:\n{output}"
    );
}

#[test]
fn test_string_startswith_endswith_honor_start_end_bounds() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_prefix_suffix_bounds".to_string(),
            params: vec![
                "s".to_string(),
                "prefix".to_string(),
                "suffix".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_startswith".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "prefix".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_endswith".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "suffix".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v1".to_string()),
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
    assert!(
        output.contains("__start")
            && output.contains("__end")
            && output.contains("string.sub(s, __start + 1, __end)"),
        "startswith/endswith must normalize start/end bounds, got:\n{output}"
    );
}

#[test]
fn test_string_slice_opcode_aliases_use_range_lowering() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_slice_opcode_aliases".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_find_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_startswith_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_endswith_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v2".to_string()),
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
    assert!(
        output.contains("__start_raw")
            && !output.contains("[unsupported op: string_find_slice]")
            && !output.contains("[unsupported op: string_startswith_slice]")
            && !output.contains("[unsupported op: string_endswith_slice]"),
        "slice op aliases must use range-aware string lowering, got:\n{output}"
    );
}

#[test]
fn test_string_find_honors_start_end_bounds() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_find_bounds".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_find".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__found")
            && output.contains("__start")
            && output.contains("__end")
            && output.contains("if __found and __found <= __end then"),
        "string.find must honor normalized start/end bounds, got:\n{output}"
    );
}

#[test]
fn test_string_startswith_endswith_tuple_prefixes_lower() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_tuple_affixes".to_string(),
            params: vec!["s".to_string()],
            param_types: Some(vec!["str".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("ba".to_string()),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("na".to_string()),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("t0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_startswith".to_string(),
                    args: Some(vec!["s".to_string(), "t0".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_endswith".to_string(),
                    args: Some(vec!["s".to_string(), "t0".to_string()]),
                    out: Some("v3".to_string()),
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
    assert!(
        output.contains("for __i = 1, #t0 do")
            && output.contains("type(__cand) ~= \"string\"")
            && !output.contains("[unsupported op: string_startswith]")
            && !output.contains("[unsupported op: string_endswith]"),
        "tuple affix args must lower to candidate loop with type guard, got:\n{output}"
    );
}

#[test]
fn test_string_rfind_honors_start_end_bounds() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_rfind_bounds".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_rfind_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__last")
            && output.contains("__found")
            && !output.contains("[unsupported op: string_rfind_slice]"),
        "string_rfind_slice must lower to bounded reverse find, got:\n{output}"
    );
}

#[test]
fn test_string_index_rindex_raise_value_error_when_missing() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_index_rindex_errors".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_index_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_rindex_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v1".to_string()),
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
    assert!(
        output.contains("__type=\"ValueError\"")
            && output.contains("substring not found")
            && !output.contains("[unsupported op: string_index_slice]")
            && !output.contains("[unsupported op: string_rindex_slice]"),
        "string index/rindex must raise ValueError when missing, got:\n{output}"
    );
}

#[test]
fn test_string_partition_and_rpartition_lower_to_tuple_tables() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_partition_ops".to_string(),
            params: vec!["s".to_string(), "sep".to_string()],
            param_types: Some(vec!["str".to_string(), "str".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_partition".to_string(),
                    args: Some(vec!["s".to_string(), "sep".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_rpartition".to_string(),
                    args: Some(vec!["s".to_string(), "sep".to_string()]),
                    out: Some("v1".to_string()),
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
    assert!(
        output.contains("empty separator")
            && output.contains("{s, \"\", \"\"}")
            && output.contains("{\"\", \"\", s}")
            && output.contains("string_partition")
            && !output.contains("[unsupported op: string_partition]"),
        "string partition/rpartition must lower to Python tuple tables, got:\n{output}"
    );
}

#[test]
fn test_string_removeprefix_suffix_get_attr_indirect_path() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_remove_affix".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("foobar".to_string()),
                    out: Some("s".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("removeprefix".to_string()),
                    out: Some("m0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("foo".to_string()),
                    out: Some("p".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("a0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_pos".to_string(),
                    args: Some(vec!["a0".to_string(), "p".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["m0".to_string(), "a0".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("removesuffix".to_string()),
                    out: Some("m1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("bar".to_string()),
                    out: Some("q".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("a1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_push_pos".to_string(),
                    args: Some(vec!["a1".to_string(), "q".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["m1".to_string(), "a1".to_string()]),
                    out: Some("v1".to_string()),
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
    assert!(
        output.contains("function(__args)")
            && output.contains("string.sub(s, 1, #__prefix)")
            && output.contains("string.sub(s, -#__suffix)")
            && !output.contains("s.removeprefix")
            && !output.contains("s.removesuffix"),
        "string remove-prefix/suffix method attrs must lower to callable closures, got:\n{output}"
    );
}

#[test]
fn test_luau_repr_authority_typed_string_get_attr_dispatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "typed_string_remove_prefix_attr".to_string(),
            params: vec!["s".to_string()],
            param_types: Some(vec!["str".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("removeprefix".to_string()),
                    out: Some("method".to_string()),
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

    assert!(
        output.contains("function(__args)")
            && output.contains("string.sub(s, 1, #__prefix)")
            && !output.contains("s.removeprefix"),
        "typed str facts should authorize string removeprefix closure lowering, got:\n{output}"
    );
}

#[test]
fn test_string_ascii_predicate_get_attr_indirect_path() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_predicate_attrs".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("Abc123".to_string()),
                    out: Some("s".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("isalnum".to_string()),
                    out: Some("m0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("a0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["m0".to_string(), "a0".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("isidentifier".to_string()),
                    out: Some("m1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("a1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["m1".to_string(), "a1".to_string()]),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: Some(vec!["s".to_string()]),
                    s_value: Some("istitle".to_string()),
                    out: Some("m2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".to_string(),
                    out: Some("a2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["m2".to_string(), "a2".to_string()]),
                    out: Some("v2".to_string()),
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
    assert!(
        output.contains("function(__args)")
            && output.contains("__has_cased")
            && output.contains("__first_ok")
            && output.contains("__prev_uncased")
            && output.contains("string.byte(__s, __i)")
            && !output.contains("s.isalnum"),
        "string predicate attrs must lower to ASCII-fast closures, got:\n{output}"
    );
}

#[test]
fn test_string_splitlines_lowers_with_keepends_flag() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_splitlines_op".to_string(),
            params: vec!["s".to_string(), "keep".to_string()],
            param_types: Some(vec!["str".to_string(), "bool".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_splitlines".to_string(),
                    args: Some(vec!["s".to_string(), "keep".to_string()]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__keep")
            && output.contains("\\r")
            && output.contains("\\n")
            && output.contains("__next += 1")
            && output.contains("__line_start"),
        "string_splitlines must lower with CR/LF handling and keepends flag, got:\n{output}"
    );
}

#[test]
fn test_string_empty_needle_edge_cases_are_explicit() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_empty_needle_edges".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_find".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_startswith".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_endswith".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v2".to_string()),
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
    assert!(
        output.contains("needle == \"\"")
            && output.contains("__start_raw")
            && output.contains("__start_raw <= __n"),
        "empty substring cases must be explicit and Python-shaped, got:\n{output}"
    );
}

#[test]
fn test_string_split_rejects_empty_separator() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_split_empty_sep".to_string(),
            params: vec!["s".to_string(), "sep".to_string()],
            param_types: Some(vec!["str".to_string(), "str".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_split".to_string(),
                    args: Some(vec!["s".to_string(), "sep".to_string()]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("__type=\"ValueError\"") && output.contains("empty separator"),
        "str.split must reject empty separator instead of looping, got:\n{output}"
    );
}

#[test]
fn test_string_replace_honors_count_argument() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_replace_count".to_string(),
            params: vec![
                "s".to_string(),
                "old".to_string(),
                "new_value".to_string(),
                "count".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_replace".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "old".to_string(),
                        "new_value".to_string(),
                        "count".to_string(),
                    ]),
                    out: Some("v0".to_string()),
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
    assert!(
        output.contains("if count >= 0 then")
            && output.contains("__pattern")
            && output.contains("__replacement"),
        "str.replace(old, new, count) must pass bounded count to gsub, got:\n{output}"
    );
}

#[test]
fn test_string_count_and_count_slice_lower_to_nonoverlap_loop() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "string_count_ops".to_string(),
            params: vec![
                "s".to_string(),
                "needle".to_string(),
                "start".to_string(),
                "end_idx".to_string(),
            ],
            param_types: Some(vec![
                "str".to_string(),
                "str".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "string_count".to_string(),
                    args: Some(vec!["s".to_string(), "needle".to_string()]),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "string_count_slice".to_string(),
                    args: Some(vec![
                        "s".to_string(),
                        "needle".to_string(),
                        "start".to_string(),
                        "end_idx".to_string(),
                    ]),
                    out: Some("v1".to_string()),
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
    assert!(
        output.contains("__sub == \"\"")
            && output.contains("__count += 1")
            && output.contains("__pos = __j + 1"),
        "string_count ops must use Python non-overlapping count loop, got:\n{output}"
    );
}
