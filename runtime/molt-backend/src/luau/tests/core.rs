use super::exceptions::luau_tir_roundtrip_function;
use super::*;

#[test]
fn test_sanitize_ident() {
    assert_eq!(sanitize_ident("foo"), "foo");
    assert_eq!(sanitize_ident("my.attr"), "my_attr");
    assert_eq!(sanitize_ident("and"), "_m_and");
    assert_eq!(sanitize_ident("v0"), "v0");
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
fn test_compile_checked_materializes_sys_target_version_module() {
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
        .expect("sys target-version bootstrap must be supported");
    assert!(!source.contains("local function molt_sys_set_version_info(...) end"));
    assert!(source.contains("local function molt_sys_set_version_info("));
    assert!(source.contains("molt_module_cache[\"sys\"] ="));
    assert!(source.contains("version_info = molt_sys_version_info"));
    assert!(source.contains("version = molt_sys_version"));
    assert!(source.contains("hexversion = molt_sys_hexversion"));
    assert!(!source.contains("(molt_module_cache[sys_name] or {})"));
    assert!(source.contains("local sys_module = molt_luau_import_module(sys_name)"));
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
    let source = backend
        .compile_checked(&ir)
        .expect("label/goto source should pass validation");
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
    let source = backend
        .compile_checked(&ir)
        .expect("store_var/load_var should lower without stub markers");
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
    let source = backend
        .compile_checked(&ir)
        .expect("missing sentinel should lower without stub markers");

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
    let source = backend
        .compile_checked(&ir)
        .expect("process target facts should lower without stub markers");

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
    let source = backend
        .compile_checked(&ir)
        .expect("trace markers should lower as Luau no-ops");

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
    let source = backend
        .compile_checked(&ir)
        .expect("exception-break markers should lower as Luau no-ops");

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
fn test_compile_checked_lowers_code_and_frame_metadata_as_luau_noops() {
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
    let source = backend
        .compile_checked(&ir)
        .expect("code/frame metadata should lower as Luau no-ops");

    assert!(
        source.contains("code_frame_metadata_test"),
        "compiled code/frame metadata function should be emitted, got:\n{source}"
    );
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
fn test_compile_checked_accepts_shared_drop_artifacts_as_gc_noops() {
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
    let source = backend
        .compile_checked(&ir)
        .expect("shared drop artifacts should be explicit Luau GC no-ops");
    assert!(!source.contains("[unsupported op: drop_inserted]"));
    assert!(!source.contains("[unsupported op: exception_region_drops_inserted]"));
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
    let source = backend
        .compile_checked(&ir)
        .expect("guard_tag should lower to the shared Luau guard helper");
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
    let source = backend
        .compile_checked(&ir)
        .expect("exception stack depth bookkeeping should lower");
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
    let source = backend
        .compile_checked(&ir)
        .expect("iter_next_unboxed should lower without stub markers");
    assert!(source.contains("local __next_done = it()"));
    assert!(source.contains("local done = __next_done[2]"));
    assert!(source.contains("local value = __next_done[1]"));
    assert!(!source.contains("[unsupported op: iter_next_unboxed]"));
}

#[test]
fn test_luau_tir_roundtrip_raise_catch_closes_pcall_before_handler() {
    let func: FunctionIR = serde_json::from_str(
            r#"{"name":"__main____raise_catch","ops":[{"kind":"trace_enter_slot","value":1},{"kind":"exception_stack_enter","out":"v107"},{"kind":"exception_stack_depth","out":"v108"},{"kind":"missing","out":"v109"},{"args":["v109"],"kind":"store_var","var":"caught"},{"kind":"check_exception","value":3},{"kind":"missing","out":"v110"},{"args":["v110"],"kind":"store_var","var":"i"},{"kind":"check_exception","value":3},{"args":["n"],"col_offset":4,"end_col_offset":14,"kind":"store_var","var":"n"},{"col_offset":4,"end_col_offset":14,"kind":"line","value":36},{"kind":"check_exception","value":3},{"kind":"const","out":"v111","value":0},{"args":["v111"],"col_offset":4,"end_col_offset":23,"kind":"store_var","var":"caught"},{"col_offset":4,"end_col_offset":23,"kind":"line","value":37},{"kind":"check_exception","value":3},{"kind":"const","out":"v112","value":0},{"kind":"const","out":"v113","value":1},{"args":["v112","n","v113"],"kind":"range_new","out":"v114"},{"kind":"check_exception","value":3},{"kind":"const","out":"v115","value":0},{"kind":"const","out":"v116","value":1},{"args":["v114"],"kind":"len","out":"v117"},{"kind":"check_exception","value":3},{"kind":"loop_start"},{"args":["v115"],"kind":"loop_index_start","out":"v118"},{"args":["v118","v117"],"fast_int":true,"kind":"lt","out":"v119"},{"kind":"check_exception","value":3},{"args":["v119"],"kind":"loop_break_if_false","type_hint":"bool"},{"args":["v114","v118"],"kind":"index","out":"v120"},{"kind":"check_exception","value":3},{"args":["v120"],"col_offset":8,"end_col_offset":23,"kind":"store_var","var":"i"},{"col_offset":8,"end_col_offset":23,"kind":"line","value":38},{"kind":"check_exception","value":3},{"kind":"exception_push","out":"none"},{"col_offset":12,"end_col_offset":31,"kind":"try_start","value":4},{"col_offset":12,"end_col_offset":31,"kind":"line","value":39},{"kind":"load_var","out":"v121","var":"i"},{"kind":"check_exception","value":4},{"args":["v121"],"kind":"exception_new_builtin_one","out":"v122","s_value":"ValueError","value":5},{"args":["v122"],"kind":"raise","out":"none"},{"kind":"jump","value":4},{"kind":"try_end","value":4},{"kind":"jump","value":6},{"kind":"label","value":4},{"kind":"exception_last_pending","out":"v123"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"kind":"exception_match_builtin","out":"v124","s_value":"ValueError","value":5},{"args":["v124"],"kind":"if","type_hint":"bool"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"col_offset":12,"end_col_offset":23,"kind":"exception_context_set","out":"none"},{"col_offset":12,"end_col_offset":23,"kind":"line","value":41},{"kind":"load_var","out":"v125","var":"caught"},{"kind":"const","out":"v126","value":1},{"args":["v125","v126"],"fast_int":true,"kind":"inplace_add","out":"v127"},{"args":["v127"],"kind":"store_var","var":"caught"},{"kind":"const_none","out":"v128"},{"args":["v128"],"kind":"exception_context_set","out":"none"},{"kind":"else"},{"args":["v123"],"kind":"raise","out":"none"},{"kind":"end_if"},{"kind":"jump","value":7},{"kind":"label","value":6},{"kind":"exception_pop","out":"none"},{"kind":"jump","value":8},{"kind":"label","value":7},{"kind":"exception_pop","out":"none"},{"kind":"check_exception","value":3},{"kind":"label","value":8},{"kind":"check_exception","value":3},{"args":["v118","v116"],"fast_int":true,"kind":"add","out":"v129"},{"kind":"check_exception","value":3},{"args":["v129"],"kind":"loop_index_next","out":"v118"},{"kind":"loop_continue"},{"col_offset":4,"end_col_offset":17,"kind":"loop_end"},{"col_offset":4,"end_col_offset":17,"kind":"line","value":42},{"kind":"load_var","out":"v130","var":"caught"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret","var":"v130"},{"kind":"label","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret_void"}],"param_types":["i64"],"params":["n"]}"#,
        )
        .expect("raise_catch frontend fixture should deserialize");
    let func = luau_tir_roundtrip_function(func);
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&SimpleIR {
            functions: vec![func],
            profile: None,
        })
        .expect("TIR-roundtripped raise/catch should lower to Luau");
    let pcall_start = source
        .find("pcall(function()")
        .expect("pcall wrapper should be emitted");
    let after_pcall = &source[pcall_start..];
    let pcall_end = after_pcall
        .find("end)")
        .unwrap_or_else(|| panic!("pcall wrapper must close before handler dispatch:\n{source}"));
    let failure_dispatch = after_pcall
        .find("__err_0")
        .expect("handler dispatch should consume the pcall error value");
    assert!(
        pcall_end < failure_dispatch,
        "handler dispatch must remain outside the protected pcall body:\n{source}"
    );
}
