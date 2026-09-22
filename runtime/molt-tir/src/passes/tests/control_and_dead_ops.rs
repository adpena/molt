use super::*;

#[test]
fn dead_box_bindings_do_not_erase_materialization_failure() {
    for out in [None, Some("none"), Some("unused")] {
        let mut ir = SimpleIR {
            functions: vec![manifest_func(vec![
                make_const_int("raw", i64::MAX),
                OpIR {
                    kind: "box".into(),
                    args: Some(vec!["raw".into()]),
                    out: out.map(str::to_string),
                    ..OpIR::default()
                },
            ])],
            profile: None,
        };
        eliminate_dead_ops(&mut ir);
        assert!(
            ir.functions[0].ops.iter().any(|op| op.kind == "box"),
            "{out:?}"
        );
    }
}

#[test]
fn exception_elision_requires_clean_fallthrough_and_generated_nothrow_facts() {
    let check = || OpIR {
        kind: "check_exception".into(),
        value: Some(100),
        ..Default::default()
    };
    for kind in [
        "const_str",
        "const_bytes",
        "const_bigint",
        "const_string",
        "box",
        "box_from_raw_int",
        "call_func",
        "trace_enter_slot",
        "label",
        "state_label",
        "unknown_operation",
    ] {
        let mut func = manifest_func(vec![check(), make_op(kind), make_op("const_none"), check()]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops
                .iter()
                .filter(|op| op.kind == "check_exception")
                .count(),
            2,
            "a pure trailing operation cannot hide pending errors or a join after {kind}"
        );
    }
    for kind in [
        "const_none",
        "const_bool",
        "const_float",
        "inc_ref",
        "dec_ref",
    ] {
        let mut func = manifest_func(vec![check(), make_op(kind), check()]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            2,
            "clean {kind} fallthrough needs no repeated check"
        );
    }
    for value in [0, i64::MIN, i64::MAX] {
        let mut func = manifest_func(vec![check(), make_const_int("v", value), check()]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(func.ops.len(), if value == 0 { 2 } else { 3 });
    }
}

#[test]
fn exception_elision_preserves_polling_observers_and_unproven_entry_state() {
    for (kind, poll, out, target) in [
        ("check_exception", true, None, Some(100)),
        ("async_work_poll", false, None, Some(100)),
        ("check_exception", false, Some("observed"), Some(100)),
        ("check_exception", false, None, None),
    ] {
        let mut func = manifest_func(vec![
            OpIR {
                kind: "check_exception".into(),
                value: Some(100),
                ..Default::default()
            },
            make_op("const_none"),
            OpIR {
                kind: kind.into(),
                async_work_poll: poll,
                out: out.map(str::to_string),
                value: target,
                ..Default::default()
            },
        ]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            3,
            "observer/polling semantics are not redundant"
        );
    }
    let mut func = manifest_func(vec![
        make_op("const_none"),
        OpIR {
            kind: "check_exception".into(),
            value: Some(100),
            ..Default::default()
        },
    ]);
    elide_safe_exception_checks(&mut func);
    assert_eq!(
        func.ops.len(),
        2,
        "a constant does not establish clean entry state"
    );
}

#[test]
fn untargeted_observers_cannot_hide_pending_failures_from_later_checks() {
    for (kind, poll, literal) in [
        ("check_exception", false, Some("const_str")),
        ("check_exception", false, Some("const_bytes")),
        ("check_exception", false, Some("const_bigint")),
        ("check_exception", true, None),
        ("async_work_poll", false, None),
    ] {
        let check = || OpIR {
            kind: "check_exception".into(),
            value: Some(100),
            ..Default::default()
        };
        let mut ops = vec![check()];
        if let Some(literal) = literal {
            ops.push(make_op(literal));
        }
        ops.push(OpIR {
            kind: kind.into(),
            async_work_poll: poll,
            ..Default::default()
        });
        ops.push(check());
        let expected_len = ops.len();
        let mut func = manifest_func(ops);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            expected_len,
            "untargeted {kind} poll={poll} must preserve the final failure transfer"
        );
        assert_eq!(func.ops.last().unwrap().value, Some(100));
    }
}

#[test]
fn direct_raise_edge_canonicalization_removes_duplicate_handler_edges() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "direct_raise".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            OpIR {
                kind: "exception_new_builtin".to_string(),
                out: Some("exc".to_string()),
                value: Some(5),
                ..Default::default()
            },
            make_store_var("_bb7_arg0", "acc"),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(100),
                ..Default::default()
            },
            OpIR {
                kind: "raise".to_string(),
                args: Some(vec!["exc".to_string()]),
                ..Default::default()
            },
            make_store_var("_bb7_arg0", "acc"),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(100),
                ..Default::default()
            },
            make_store_var("_bb7_arg0", "acc"),
            OpIR {
                kind: "jump".to_string(),
                value: Some(100),
                ..Default::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(100),
                ..Default::default()
            },
        ],
    };

    canonicalize_direct_raise_edges(&mut func);

    assert!(
        !func.ops.iter().any(|op| op.kind == "check_exception"),
        "direct raise-to-handler edge must not keep redundant polls: {:?}",
        func.ops
    );
    let raise_idx = func
        .ops
        .iter()
        .position(|op| op.kind == "raise")
        .expect("raise must remain");
    assert_eq!(func.ops[raise_idx + 1].kind, "store_var");
    assert_eq!(func.ops[raise_idx + 2].kind, "jump");
    assert_eq!(func.ops[raise_idx + 2].value, Some(100));
}

#[test]
fn dead_op_elim_keeps_copy_var_when_output_is_consumed() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "param_copy".to_string(),
            params: vec!["n".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("n".to_string()),
                    out: Some("_v8".to_string()),
                    ..Default::default()
                },
                make_const_int("_v11", 1),
                make_arith("add", &["_v8", "_v11"], "_v12"),
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["_v12".to_string()]),
                    ..Default::default()
                },
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter()
            .any(|op| op.kind == "copy_var" && op.out.as_deref() == Some("_v8")),
        "dead-op elimination must preserve copy_var definitions consumed through op.out: {ops:?}"
    );
}

#[test]
fn dead_op_elim_counts_copy_var_source_as_consumed_input() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "copy_source".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                make_const_int("_v0", 40),
                make_const_int("_v1", 2),
                make_arith("add", &["_v0", "_v1"], "_sum"),
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("_sum".to_string()),
                    out: Some("_alias".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["_alias".to_string()]),
                    ..Default::default()
                },
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter()
            .any(|op| op.kind == "add" && op.out.as_deref() == Some("_sum")),
        "dead-op elimination must preserve producers consumed through copy_var.var: {ops:?}"
    );
}

#[test]
fn dead_op_elim_ignores_args_based_copy_var_metadata_var() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "copy_source_metadata".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                make_const_int("_source", 40),
                make_const_int("_metadata", 2),
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("_metadata".to_string()),
                    args: Some(vec!["_source".to_string()]),
                    out: Some("_alias".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["_alias".to_string()]),
                    ..Default::default()
                },
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter()
            .any(|op| op.kind == "const" && op.out.as_deref() == Some("_source")),
        "dead-op elimination must preserve the args[0] value source: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "const" && op.out.as_deref() == Some("_metadata")),
        "copy_var.var is metadata when args[0] is present and must not keep dead producers alive: {ops:?}"
    );
}

#[test]
fn dead_op_elim_keeps_unused_potentially_throwing_index() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unused_index".to_string(),
            params: vec!["mapping".to_string(), "key".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                OpIR {
                    kind: "index".to_string(),
                    args: Some(vec!["mapping".to_string(), "key".to_string()]),
                    out: Some("_unused".to_string()),
                    ..Default::default()
                },
                make_op("ret_void"),
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().any(|op| op.kind == "index"),
        "dead-op elimination must preserve unused index ops because __getitem__/__missing__ exceptions are observable: {ops:?}"
    );
}

#[test]
fn dead_op_elim_preserves_observable_module_lookup_chain() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "dead_static_class_guard".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("__main__".to_string()),
                    out: Some("module_name".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    args: Some(vec!["module_name".to_string()]),
                    out: Some("module".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("Point".to_string()),
                    out: Some("attr_name".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    args: Some(vec!["module".to_string(), "attr_name".to_string()]),
                    out: Some("class_ref".to_string()),
                    ..Default::default()
                },
                make_op("ret_void"),
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().any(|op| op.kind == "module_cache_get"),
        "module cache lookup must remain because lookup and exceptions are observable: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| op.kind == "module_get_attr"),
        "module attribute lookup must remain because lookup and exceptions are observable: {ops:?}"
    );
}

#[test]
fn dead_op_elim_keeps_unused_untyped_arithmetic() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unused_untyped_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                make_arith("add", &["left", "right"], "_unused"),
                make_op("ret_void"),
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().any(|op| op.kind == "add"),
        "dead-op elimination must preserve unused untyped arithmetic because protocol dispatch can raise: {ops:?}"
    );
}

#[test]
fn dead_op_elim_keeps_transport_hinted_unknown_arithmetic() {
    let mut add = make_arith("add", &["left", "right"], "_unused");
    add.fast_int = Some(true);
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unused_transport_hint_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![add, make_op("ret_void")],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().any(|op| op.kind == "add"),
        "transport hints must not prove unused arithmetic is nonthrowing without typed facts: {ops:?}"
    );
}

#[test]
fn dead_op_elim_removes_unused_typed_param_arithmetic_without_transport_hints() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unused_typed_param_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                make_arith("add", &["left", "right"], "_unused"),
                make_op("ret_void"),
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().all(|op| op.kind != "add"),
        "typed scalar facts, not transport hints, should prove unused int arithmetic removable: {ops:?}"
    );
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, "ret_void");
}

#[test]
fn dead_op_elim_removes_unused_typed_const_arithmetic_chain() {
    let add = make_arith("add", &["_v0", "_v1"], "_unused");
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unused_typed_const_add".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                make_const_int("_v0", 40),
                make_const_int("_v1", 2),
                add,
                make_op("ret_void"),
            ],
        }],
        profile: None,
    };

    eliminate_dead_ops(&mut ir);

    let ops = &ir.functions[0].ops;
    assert!(
        ops.iter().all(|op| op.kind != "add" && op.out.is_none()),
        "dead-op elimination should still remove provably nonthrowing unused typed value chains: {ops:?}"
    );
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, "ret_void");
}

// --- RC coalescing tests ---
