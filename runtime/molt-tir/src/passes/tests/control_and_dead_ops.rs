use super::*;

#[test]
fn direct_raise_edge_canonicalization_removes_duplicate_handler_edges() {
    let mut func = FunctionIR {
        name: "direct_raise".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
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
fn return_alias_summary_ignores_exception_ret_void_tail() {
    let summaries = compute_return_alias_summaries(&[FunctionIR {
        name: "expect_str_like".to_string(),
        params: vec!["value".to_string(), "label".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops: vec![
            OpIR {
                kind: "ret".to_string(),
                var: Some("value".to_string()),
                ..Default::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(1),
                ..Default::default()
            },
            make_op("ret_void"),
        ],
    }]);

    assert_eq!(
        summaries.get("expect_str_like"),
        Some(&ReturnAliasSummary::Param(0))
    );
}

#[test]
fn return_alias_summary_rejects_mixed_alias_and_fresh_return() {
    let summaries = compute_return_alias_summaries(&[FunctionIR {
        name: "mixed_return".to_string(),
        params: vec!["value".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops: vec![
            OpIR {
                kind: "ret".to_string(),
                var: Some("value".to_string()),
                ..Default::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("fresh".to_string()),
                s_value: Some("fresh".to_string()),
                ..Default::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("fresh".to_string()),
                ..Default::default()
            },
        ],
    }]);

    assert_eq!(summaries.get("mixed_return"), None);
}

#[test]
fn return_alias_summary_uses_args_based_copy_var_value_source() {
    let summaries = compute_return_alias_summaries(&[FunctionIR {
        name: "copy_var_alias".to_string(),
        params: vec!["value".to_string(), "metadata_slot".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops: vec![
            OpIR {
                kind: "copy_var".to_string(),
                var: Some("metadata_slot".to_string()),
                args: Some(vec!["value".to_string()]),
                out: Some("alias".to_string()),
                ..Default::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("alias".to_string()),
                args: Some(vec!["alias".to_string()]),
                ..Default::default()
            },
        ],
    }]);

    assert_eq!(
        summaries.get("copy_var_alias"),
        Some(&ReturnAliasSummary::Param(0)),
        "args[0] is the copied value; var is only local-name transport metadata"
    );
}

#[test]
fn dead_op_elim_keeps_copy_var_when_output_is_consumed() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "param_copy".to_string(),
            params: vec!["n".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    var: Some("_v12".to_string()),
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
            name: "copy_source".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    var: Some("_alias".to_string()),
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
            name: "copy_source_metadata".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    var: Some("_alias".to_string()),
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
            name: "unused_index".to_string(),
            params: vec!["mapping".to_string(), "key".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
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
fn dead_op_elim_removes_effect_proven_static_module_class_lookup_chain() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dead_static_class_guard".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
                    effect_proof: Some(EffectProof::StaticModuleClassBinding.name().to_string()),
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
                    effect_proof: Some(EffectProof::StaticModuleClassBinding.name().to_string()),
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
        ops.iter()
            .all(|op| !matches!(op.kind.as_str(), "module_cache_get" | "module_get_attr")),
        "effect-proven dead static class guard should be removed: {ops:?}"
    );
}

#[test]
fn dead_op_elim_keeps_unused_untyped_arithmetic() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unused_untyped_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
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
            name: "unused_transport_hint_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
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
            name: "unused_typed_param_add".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
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
            name: "unused_typed_const_add".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
