use super::super::*;

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
fn test_len_uses_tir_container_fact_for_packed_sequence_length() {
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
        output.contains("local n = molt_sequence_len(xs)"),
        "typed list len should use packed-sequence length authority, got:\n{output}"
    );
    assert!(
        !output.contains("local n = molt_len(xs)"),
        "typed list len should not call runtime len, got:\n{output}"
    );
}

#[test]
fn test_typed_string_len_uses_unicode_codepoint_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "typed_string_len".to_string(),
            params: vec!["text".to_string()],
            param_types: Some(vec!["str".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "len".to_string(),
                    args: Some(vec!["text".to_string()]),
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

    assert!(output.contains("local n = molt_len(text)"));
    assert!(output.contains("utf8.len(obj)"));
    assert!(!output.contains("local n = #text"));
}

#[test]
fn test_typed_list_truthiness_uses_packed_sequence_length_for_not() {
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
        output.contains("local empty: boolean = not (molt_sequence_len(xs) > 0)"),
        "typed list truthiness should use packed-sequence length authority, got:\n{output}"
    );
    assert!(
        !output.contains("not molt_bool(xs)"),
        "typed list truthiness should not call runtime bool for not, got:\n{output}"
    );
}

#[test]
fn test_typed_dict_truthiness_uses_ordered_dict_size_authority() {
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
        output.contains("local selected = if (molt_dict_len(d) > 0) then d else fallback"),
        "typed dict truthiness should use canonical O(1) dict size, got:\n{output}"
    );
    assert!(
        !output.contains("molt_bool(d)"),
        "typed dict truthiness should not call runtime bool for or, got:\n{output}"
    );
}
