use super::*;

#[test]
fn test_lower_try_to_pcall_basic() {
    let ops = vec![
        OpIR {
            kind: "try_start".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_int".into(),
            value: Some(1),
            out: Some("v0".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_last".into(),
            out: Some("v1".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
    ];
    let (lowered, _) = lower_try_to_pcall(&ops);
    assert!(lowered.iter().any(|op| op.kind == "pcall_wrap_begin"));
    assert!(lowered.iter().any(|op| op.kind == "pcall_wrap_end"));
    assert!(!lowered.iter().any(|op| op.kind == "try_start"));
}

#[test]
fn test_lower_try_to_pcall_targets_protected_exception_handler() {
    let ops = vec![
        OpIR {
            kind: "try_start".into(),
            value: Some(5),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_new_builtin_one".into(),
            args: Some(vec!["arg".into()]),
            out: Some("exc".into()),
            s_value: Some("ValueError".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            out: Some("none".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(2),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            value: Some(5),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(3),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(2),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_last_pending".into(),
            out: Some("caught".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
    ];
    let (lowered, _) = lower_try_to_pcall(&ops);
    let failure = lowered
        .iter()
        .find(|op| op.kind == "pcall_failure_jump")
        .expect("pcall lowering should emit a failure jump");
    assert_eq!(failure.value, Some(2));
    let handler_last = lowered
        .iter()
        .find(|op| op.kind == "exception_last_pending")
        .expect("handler should keep exception_last_pending");
    assert_eq!(handler_last.value, Some(0));
    let begin_idx = lowered
        .iter()
        .position(|op| op.kind == "pcall_wrap_begin")
        .expect("pcall begin should be emitted");
    let end_idx = lowered
        .iter()
        .position(|op| op.kind == "pcall_wrap_end")
        .expect("pcall end should be emitted");
    assert!(
        !lowered[begin_idx..end_idx]
            .iter()
            .any(|op| op.kind == "jump" && op.value == Some(2)),
        "raise's static handler jump must be consumed by pcall_failure_jump: {lowered:?}"
    );
}

pub(super) fn luau_tir_roundtrip_function(mut func: FunctionIR) -> FunctionIR {
    if func.ops.iter().any(|op| op.kind == "phi") {
        crate::rewrite_phi_to_store_load(&mut func.ops);
    }
    let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(&func);
    crate::tir::type_refine::refine_types(&mut tir_func);
    let target_info = crate::tir::target_info::TargetInfo::luau_release_fast();
    let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &target_info);
    let _drop_changed =
        crate::tir::drop_phase::finalize_function_drops(&mut tir_func, &target_info);
    crate::tir::type_refine::refine_types(&mut tir_func);
    func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
    func
}

#[test]
fn test_compile_checked_structures_raise_catch_pcall_boundary() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "raise_catch_boundary_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "exception_push".into(),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_start".into(),
                    value: Some(5),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_new_builtin_one".into(),
                    args: Some(vec!["arg".into()]),
                    out: Some("exc".into()),
                    s_value: Some("ValueError".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".into(),
                    args: Some(vec!["exc".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".into(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    value: Some(5),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".into(),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last_pending".into(),
                    out: Some("caught".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_clear".into(),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_match_builtin".into(),
                    args: Some(vec!["caught".into()]),
                    out: Some("matched".into()),
                    s_value: Some("ValueError".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".into(),
                    args: Some(vec!["matched".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("handled".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "else".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".into(),
                    args: Some(vec!["caught".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("handled".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_pop".into(),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("raise/catch pcall boundary should lower to valid Luau");
    let pcall_start = source
        .find("pcall(function()")
        .expect("pcall wrapper should be emitted");
    let pcall_end = source[pcall_start..]
        .find("end)")
        .map(|offset| pcall_start + offset)
        .expect("pcall wrapper should be closed before handler dispatch");
    let handler_read = source
        .find("caught = __err_0")
        .expect("handler should read the pcall error value");
    assert!(
        pcall_start < pcall_end && pcall_end < handler_read,
        "handler must be outside pcall body, got:\n{source}"
    );
}

#[test]
fn test_lower_try_to_pcall_escape_detection() {
    let ops = vec![
        OpIR {
            kind: "try_start".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_int".into(),
            value: Some(42),
            out: Some("v0".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "call_function".into(),
            args: Some(vec!["print".into(), "v0".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
    ];
    let (_, escaped) = lower_try_to_pcall(&ops);
    assert!(
        escaped.contains("v0"),
        "v0 should escape pcall scope: {:?}",
        escaped
    );
}

#[test]
fn test_luau_exception_region_block_args_hoist_before_protected_op() {
    let ops = vec![
        OpIR {
            kind: "call".into(),
            out: Some("v_call".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb1_arg0".into()),
            args: Some(vec!["module_obj".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let hoisted = hoist_exception_edge_block_arg_stores(&ops);

    assert_eq!(hoisted[0].kind, "store_var");
    assert_eq!(hoisted[1].kind, "call");
    assert_eq!(hoisted[2].kind, "check_exception");
}

#[test]
fn test_luau_exception_region_block_args_hoist_before_raise_edge() {
    let ops = vec![
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb5_arg0".into()),
            args: Some(vec!["caught".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb5_arg1".into()),
            args: Some(vec!["limit".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let hoisted = hoist_exception_edge_block_arg_stores(&ops);

    assert_eq!(hoisted[0].kind, "store_var");
    assert_eq!(hoisted[0].var.as_deref(), Some("_bb5_arg0"));
    assert_eq!(hoisted[1].kind, "store_var");
    assert_eq!(hoisted[1].var.as_deref(), Some("_bb5_arg1"));
    assert_eq!(hoisted[2].kind, "raise");
    assert_eq!(hoisted[3].kind, "jump");
}

#[test]
fn test_luau_exception_region_block_args_do_not_hoist_dependent_result() {
    let ops = vec![
        OpIR {
            kind: "call".into(),
            out: Some("v_call".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb1_arg0".into()),
            args: Some(vec!["v_call".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(5),
            ..OpIR::default()
        },
    ];

    let hoisted = hoist_exception_edge_block_arg_stores(&ops);

    assert_eq!(hoisted[0].kind, "call");
    assert_eq!(hoisted[1].kind, "store_var");
    assert_eq!(hoisted[2].kind, "check_exception");
}

#[test]
fn test_luau_exception_region_module_global_ops_use_module_dict_helpers() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "module_global_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_str".into(),
                    out: Some("name".into()),
                    s_value: Some("exc".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".into(),
                    out: Some("module".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_global".into(),
                    args: Some(vec!["module".into(), "name".into()]),
                    out: Some("value".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_del_global_if_present".into(),
                    args: Some(vec!["module".into(), "name".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("molt_module_get_global(module, name)"),
        "module_get_global must read the supplied module dict:\n{output}"
    );
    assert!(
        output.contains("molt_module_del_global(module, name, true)"),
        "module_del_global_if_present must delete from the supplied module dict:\n{output}"
    );
    assert!(
        !output.contains("local value = molt_module_cache[name]"),
        "module_get_global must not read import cache directly:\n{output}"
    );
}

#[test]
fn test_luau_exception_region_type_of_uses_python_descriptor_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "type_descriptor_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "exception_new_builtin_empty".into(),
                    out: Some("exc".into()),
                    s_value: Some("NameError".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "type_of".into(),
                    args: Some(vec!["exc".into()]),
                    out: Some("typ".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "get_attr_generic_obj".into(),
                    args: Some(vec!["typ".into()]),
                    out: Some("name".into()),
                    s_value: Some("__name__".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);

    assert!(
        output.contains("local typ = molt_type_of(exc)")
            && output.contains("if type(x) == \"table\" and x.__type then"),
        "type_of must preserve Python exception class identity:\n{output}"
    );
}

#[test]
fn test_pcall_try_except_compile() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "try_except_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "try_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(1),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(0),
                    out: Some("v1".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "binary_op".into(),
                    s_value: Some("/".into()),
                    args: Some(vec!["v0".into(), "v1".into()]),
                    out: Some("v2".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".into(),
                    out: Some("v3".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_int".into(),
                    value: Some(42),
                    out: Some("v4".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_function".into(),
                    s_value: Some("print".into()),
                    args: Some(vec!["print".into(), "v4".into()]),
                    out: Some("v5".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(
        output.contains("pcall(function()"),
        "Expected pcall wrapper, got:\n{output}"
    );
    assert!(
        output.contains("__ok_0") && output.contains("__err_0"),
        "Expected __ok_0/__err_0, got:\n{output}"
    );
    assert!(
        !output.contains("= nil -- [exception_last]"),
        "exception_last should NOT emit nil inside pcall, got:\n{output}"
    );
}

#[test]
fn test_no_duplicate_local_declarations() {
    // When the same variable name appears as `out` in multiple ops,
    // only the first should emit `local`.  Subsequent uses should be
    // plain assignment to avoid Luau syntax errors.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dup_local_test".into(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                // First definition of v0 — should get `local v0 = 1`
                OpIR {
                    kind: "const_int".into(),
                    value: Some(1),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                // Second definition of v0 — must NOT emit `local` again
                OpIR {
                    kind: "const_int".into(),
                    value: Some(2),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_function".into(),
                    s_value: Some("print".into()),
                    args: Some(vec!["print".into(), "v0".into()]),
                    out: Some("v1".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    // Count occurrences of `local v0` — should be exactly 1.
    let local_v0_count = output.matches("local v0").count();
    assert_eq!(
        local_v0_count, 1,
        "Expected exactly 1 `local v0`, found {local_v0_count} in:\n{output}"
    );
}

#[test]
fn test_lower_try_to_pcall_nested() {
    let ops = vec![
        OpIR {
            kind: "try_start".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_int".into(),
            value: Some(1),
            out: Some("v0".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".into(),
            ..OpIR::default()
        },
    ];
    let (lowered, _) = lower_try_to_pcall(&ops);
    let begin_count = lowered
        .iter()
        .filter(|op| op.kind == "pcall_wrap_begin")
        .count();
    let end_count = lowered
        .iter()
        .filter(|op| op.kind == "pcall_wrap_end")
        .count();
    assert_eq!(begin_count, 2, "should have 2 pcall_wrap_begin");
    assert_eq!(end_count, 2, "should have 2 pcall_wrap_end");
}
