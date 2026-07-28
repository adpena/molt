use super::super::*;

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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
            execution_context: ExecutionContextPolicy::None,
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
