use super::super::*;

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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
        output.contains("molt_get_attr_checked(xs, \"custom\")") && !output.contains("xs:custom"),
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
