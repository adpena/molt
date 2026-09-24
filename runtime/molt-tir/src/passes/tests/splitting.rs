use super::*;
use crate::tir::simple_def_use::{simple_ir_return_has_value, visit_simple_ir_defined_names};

fn split_for_test(
    func: FunctionIR,
    max_ops: usize,
) -> Result<(FunctionIR, Vec<FunctionIR>), Box<FunctionIR>> {
    let mut occupied = std::collections::BTreeSet::from([func.name.clone()]);
    split_large_function(func, max_ops, &mut occupied)
}

fn adversarial_named_large_function(name: &str) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: name.to_string(),
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            make_const_int("first", 1),
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            make_const_int("second", 2),
            make_op("ret_void"),
        ],
        ..FunctionIR::default()
    }
}

#[test]
fn split_chunk_names_are_injective_and_reserved_against_the_full_module_namespace() {
    let first = adversarial_named_large_function("a-b");
    let second = adversarial_named_large_function("a_b");
    let mut occupied = BTreeSet::from([first.name.clone(), second.name.clone()]);
    let (_, first_chunks) = split_large_function(first, 2, &mut occupied).unwrap();
    let (_, second_chunks) = split_large_function(second, 2, &mut occupied).unwrap();
    let first_names = first_chunks
        .iter()
        .map(|chunk| chunk.name.as_str())
        .collect::<BTreeSet<_>>();
    let second_names = second_chunks
        .iter()
        .map(|chunk| chunk.name.as_str())
        .collect::<BTreeSet<_>>();
    assert!(first_names.is_disjoint(&second_names));

    let colliding_source = "user";
    let reserved = split_chunk_name(colliding_source, 0);
    let mut occupied = BTreeSet::from([colliding_source.to_string(), reserved.clone()]);
    let result = split_large_function(
        adversarial_named_large_function(colliding_source),
        2,
        &mut occupied,
    );
    assert!(
        result.is_err(),
        "an existing exact chunk symbol must fail the split closed"
    );
    assert!(occupied.contains(&reserved));
}

#[test]
fn split_large_function_preserves_protected_runtime_import_entrypoint() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "molt_isolate_import".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_const_int("v0", 1),
            make_const_int("v1", 2),
            make_arith("add", &["p0", "v0"], "v2"),
            make_arith("add", &["v2", "v1"], "v3"),
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["v3".to_string()]),
                ..OpIR::default()
            },
        ],
    };

    let result = split_for_test(func, 2);

    let original = result.expect_err("protected import entrypoint must not split");
    assert_eq!(original.name, "molt_isolate_import");
    assert_eq!(original.params, vec!["p0".to_string()]);
    assert_eq!(original.ops.len(), 5);
}

#[test]
fn split_large_function_preserves_protected_runtime_bootstrap_entrypoint() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "molt_isolate_bootstrap".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_op("const_none"),
            make_op("const_none"),
            make_op("const_none"),
            make_op("const_none"),
            make_op("ret_void"),
        ],
    };

    let result = split_for_test(func, 2);

    let original = result.expect_err("protected bootstrap entrypoint must not split");
    assert_eq!(original.name, "molt_isolate_bootstrap");
    assert!(original.params.is_empty());
    assert_eq!(original.ops.len(), 5);
}

#[test]
fn split_large_function_still_splits_regular_large_functions() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "user_large".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            make_const_int("v0", 1),
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            make_const_int("v1", 2),
            OpIR {
                kind: "line".to_string(),
                value: Some(3),
                ..OpIR::default()
            },
            make_arith("add", &["p0", "v0"], "v2"),
            OpIR {
                kind: "line".to_string(),
                value: Some(4),
                ..OpIR::default()
            },
            make_arith("add", &["v2", "v1"], "v3"),
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["v3".to_string()]),
                ..OpIR::default()
            },
        ],
    };

    let (stub, chunks) = split_for_test(func, 2).expect("expected split");

    assert_eq!(stub.name, "user_large");
    assert!(!chunks.is_empty());
    let stub_chunk_calls: Vec<&OpIR> = stub
        .ops
        .iter()
        .filter(|op| op.kind == "call_internal")
        .collect();
    assert_eq!(stub_chunk_calls.len(), chunks.len());
    for (call, chunk) in stub_chunk_calls.iter().zip(chunks.iter()) {
        assert_eq!(
            call.s_value.as_deref(),
            Some(chunk.name.as_str()),
            "stub call must target the matching private chunk",
        );
        assert_eq!(
            call.args.as_ref(),
            Some(&chunk.params),
            "stub call must forward the live-in chunk ABI, not just original params",
        );
    }
    assert!(
        chunks.iter().skip(1).any(|chunk| chunk
            .ops
            .iter()
            .any(|op| op.kind == "index" && op.out.as_deref() == Some("v0"))),
        "later chunks must load values defined by earlier chunks from the split frame"
    );
    assert!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.ops.iter())
            .all(|op| op.kind != "load_index"),
        "split frame reads must use the backend-canonical index op"
    );
    assert!(
        stub.ops.iter().any(|op| {
            op.kind == "list_new"
                && op
                    .out
                    .as_deref()
                    .is_some_and(|out| out.starts_with("__molt_split_frame"))
        }),
        "stub must allocate the split frame used for cross-chunk live values"
    );
    let call_results = stub_chunk_calls
        .iter()
        .filter_map(|op| op.out.as_deref())
        .collect::<BTreeSet<_>>();
    assert!(
        stub.ops.iter().any(|op| {
            op.kind == "ret"
                && op
                    .args
                    .as_ref()
                    .and_then(|args| args.first())
                    .is_some_and(|name| call_results.contains(name.as_str()))
        }),
        "split stub must return the exact collision-free chunk-call result"
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.name.starts_with("__molt_chunk_v1_"))
    );
    assert!(
        chunks.iter().any(|chunk| chunk.ops.iter().any(|op| {
            op.kind == "store_index"
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == "v0"))
        })),
        "split chunks must store cross-chunk live values into the split frame"
    );
}

#[test]
fn split_large_function_uses_generated_return_family_and_collision_free_synthetics() {
    let reserved_names = vec![
        "__molt_split_frame",
        "__molt_split_frame_init",
        "__molt_split_frame_index",
        "__molt_split_frame_store_index",
        "__molt_split_chunk_return",
        "__molt_split_chunk_discard",
        "__molt_split_chunk_continue",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "generated_return_family".to_string(),
        params: reserved_names.clone(),
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            OpIR {
                kind: "label".to_string(),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            make_const_int("value", 1),
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            make_arith("add", &["value", "value"], "result"),
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["result".to_string()]),
                ..OpIR::default()
            },
        ],
        ..FunctionIR::default()
    };

    let (stub, chunks) = split_for_test(func, 2).expect("generated return must split");

    assert!(chunks.iter().any(|chunk| {
        chunk.ops.iter().any(|op| {
            op.kind == "ret"
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args == &["result".to_string()])
        })
    }));
    let generated_stub_names = stub
        .ops
        .iter()
        .flat_map(|op| {
            let mut names = Vec::new();
            visit_simple_ir_defined_names(op, |name| names.push(name.to_string()));
            names
        })
        .collect::<Vec<_>>();
    assert!(
        generated_stub_names
            .iter()
            .all(|name| !reserved_names.contains(name))
    );
    let stub_labels = stub
        .ops
        .iter()
        .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
        .filter_map(|op| op.value)
        .collect::<BTreeSet<_>>();
    assert!(!stub_labels.contains(&0));
    assert_eq!(
        stub_labels.len(),
        stub.ops
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
            .count()
    );
    verify_split_function_def_use(&stub).expect("collision-free stub def-use must verify");
    for chunk in &chunks {
        verify_split_function_def_use(chunk).expect("return-family chunk def-use must verify");
    }
}

#[test]
fn split_large_function_preserves_drop_authority_on_chunks_only() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "drop_inserted_large".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            make_op(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("a".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "dec_ref".to_string(),
                args: Some(vec!["a".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("b".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "dec_ref".to_string(),
                args: Some(vec!["b".to_string()]),
                ..OpIR::default()
            },
            make_op("ret_void"),
        ],
    };

    let (stub, chunks) = split_for_test(func, 2).expect("expected split");

    assert!(
        !stub.ops.iter().any(is_drop_fact_marker_op),
        "synthetic split stub creates its own frame values and must not inherit full-RC authority"
    );
    let chunks_with_dec_ref = chunks
        .iter()
        .filter(|chunk| chunk.ops.iter().any(|op| op.kind == "dec_ref"))
        .count();
    assert!(
        chunks_with_dec_ref > 0,
        "test must exercise extracted chunks containing TIR-inserted drops"
    );
    for chunk in &chunks {
        assert_eq!(
            chunk.ops.first().map(|op| op.kind.as_str()),
            Some(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            "chunk {} must start with the full-RC authority marker",
            chunk.name
        );
        assert_eq!(
            chunk
                .ops
                .iter()
                .filter(|op| is_drop_fact_marker_op(op))
                .count(),
            1,
            "chunk {} must not duplicate transport markers",
            chunk.name
        );
    }
}

#[test]
fn split_large_function_threads_cross_chunk_builtin_type_tag() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "threading__molt_module_chunk_3".to_string(),
        params: vec!["__molt_module_obj__".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            make_const_int("object_type_tag", 100),
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "builtin_type".to_string(),
                args: Some(vec!["object_type_tag".to_string()]),
                out: Some("object_type".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
    };

    let (stub, chunks) = split_for_test(func, 2).expect("expected split");

    assert!(
        chunks.iter().skip(1).any(|chunk| chunk
            .ops
            .iter()
            .any(|op| { op.kind == "index" && op.out.as_deref() == Some("object_type_tag") })),
        "the builtin_type chunk must load the tag value from the split frame"
    );
    assert!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.ops.iter())
            .all(|op| op.kind != "load_index"),
        "split frame reads must not introduce non-canonical IR ops"
    );
    assert!(
        chunks.iter().any(|chunk| {
            chunk.ops.iter().any(|op| {
                op.kind == "store_index"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == "object_type_tag"))
            })
        }),
        "the defining chunk must store the tag into the split frame"
    );
    assert!(
        stub.ops.iter().any(|op| {
            op.kind == "list_new"
                && op
                    .out
                    .as_deref()
                    .is_some_and(|out| out.starts_with("__molt_split_frame"))
        }),
        "the stub must allocate frame storage for the transported tag"
    );
    for chunk in &chunks {
        verify_split_function_def_use(chunk).expect("generated chunk def-use must verify");
    }
    verify_split_function_def_use(&stub).expect("generated stub def-use must verify");
}

#[test]
fn split_generated_op_verifier_rejects_noncanonical_frame_load() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "__molt_chunk_bad_0".to_string(),
        params: vec!["__molt_split_frame".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                value: Some(0),
                out: Some("__molt_split_frame_load_index".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_index".to_string(),
                args: Some(vec![
                    "__molt_split_frame".to_string(),
                    "__molt_split_frame_load_index".to_string(),
                ]),
                out: Some("value".to_string()),
                ..OpIR::default()
            },
            make_op("ret_void"),
        ],
    };

    let err = verify_split_generated_ops(&func).expect_err("load_index must reject");
    assert!(err.contains("non-canonical generated op `load_index`"));
}

#[test]
fn split_large_function_clones_shared_suffix_exception_handler() {
    let mut ops = Vec::new();
    for i in 0..40 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(i),
            ..OpIR::default()
        });
        ops.push(make_const_int(&format!("v{i}"), i));
        ops.push(OpIR {
            kind: "check_exception".to_string(),
            value: Some(32),
            ..OpIR::default()
        });
    }
    ops.push(OpIR {
        kind: "line".to_string(),
        value: Some(99),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "module_get_attr".to_string(),
        args: Some(vec!["__molt_module_obj__".to_string(), "v0".to_string()]),
        out: Some("loaded_v0".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "jump".to_string(),
        value: Some(32),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(32),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "exception_last".to_string(),
        out: Some("exc".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "const_none".to_string(),
        out: Some("none_exc".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "is".to_string(),
        args: Some(vec!["exc".to_string(), "none_exc".to_string()]),
        out: Some("exc_is_none".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "not".to_string(),
        args: Some(vec!["exc_is_none".to_string()]),
        out: Some("exc_pending".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "jump".to_string(),
        value: Some(430),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(430),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "br_if".to_string(),
        args: Some(vec!["exc_pending".to_string()]),
        value: Some(523),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "jump".to_string(),
        value: Some(352),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(523),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "const_str".to_string(),
        s_value: Some("builtins".to_string()),
        out: Some("module_name".to_string()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "module_cache_del".to_string(),
        args: Some(vec!["module_name".to_string()]),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "jump".to_string(),
        value: Some(352),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(352),
        ..OpIR::default()
    });
    ops.push(make_op("ret_void"));

    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "builtins__molt_module_chunk_2".to_string(),
        params: vec!["__molt_module_obj__".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops,
    };

    let (stub, chunks) = split_for_test(func, 40)
        .expect("shared suffix exception handler should not block splitting");

    assert_eq!(stub.name, "builtins__molt_module_chunk_2");
    assert!(chunks.len() >= 2);
    let control_outs: std::collections::BTreeSet<String> = stub
        .ops
        .iter()
        .filter(|op| op.kind == "call_internal")
        .filter_map(|op| op.out.clone())
        .filter(|out| out.starts_with("__molt_split_chunk_continue"))
        .collect();
    assert_eq!(
        control_outs.len(),
        chunks.len(),
        "void split chunks must return an explicit continuation status"
    );
    for out in &control_outs {
        assert!(
            stub.ops.iter().any(|op| {
                op.kind == "br_if"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == out))
            }),
            "stub must branch on chunk continuation status `{out}`"
        );
    }
    let mut observed_live_out_store_before_cloned_suffix = false;
    let mut observed_cloned_suffix_stop_return = false;
    for chunk in &chunks {
        assert!(
            chunk.ops.len() <= 80,
            "cloned shared suffix must not recreate an oversized chunk: {} ops",
            chunk.ops.len()
        );
        let labels: std::collections::BTreeSet<i64> = chunk
            .ops
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
            .filter_map(|op| op.value)
            .collect();
        let source_labels = BTreeSet::from([32, 352, 430, 523]);
        let cloned_skip_labels: Vec<i64> = labels.difference(&source_labels).copied().collect();
        let cloned_handler = chunk
            .ops
            .iter()
            .position(|op| op.kind == "label" && op.value == Some(32));
        if !cloned_skip_labels.is_empty() {
            let handler_idx = cloned_handler.expect("cloned chunk must include handler label 32");
            assert!(handler_idx > 0);
            let guard = &chunk.ops[handler_idx - 1];
            assert_eq!(
                guard.kind, "jump",
                "normal chunk fallthrough must skip the cloned exception tail"
            );
            assert_ne!(guard.value, Some(32));
            observed_cloned_suffix_stop_return |=
                chunk.ops[handler_idx..].windows(2).any(|window| {
                    window[0].kind == "const_bool"
                        && window[0].value == Some(0)
                        && window[0].out.as_ref().is_some_and(|out| {
                            window[1]
                                .args
                                .as_ref()
                                .is_some_and(|args| args == std::slice::from_ref(out))
                        })
                        && window[1].kind == "ret"
                });
            for (idx, op) in chunk.ops.iter().enumerate() {
                if op.kind == "store_index"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == "v0"))
                {
                    assert!(
                        idx < handler_idx - 1,
                        "split-frame live-out stores must execute before skipping cloned tails"
                    );
                    observed_live_out_store_before_cloned_suffix = true;
                }
            }
        }
        for op in &chunk.ops {
            if matches!(op.kind.as_str(), "check_exception" | "jump" | "br_if")
                && let Some(target) = op.value
            {
                assert!(
                    labels.contains(&target),
                    "chunk `{}` retains external control-flow target {}",
                    chunk.name,
                    target
                );
            }
        }
    }
    assert!(
        observed_live_out_store_before_cloned_suffix,
        "test must cover a live-out split-frame store in a suffix-cloned chunk"
    );
    assert!(
        observed_cloned_suffix_stop_return,
        "cloned terminal suffixes must tell the stub not to run later chunks"
    );
}

#[test]
fn split_large_function_delays_suffix_clone_until_cleanup_reads_are_available() {
    let mut ops = vec![
        OpIR {
            kind: "line".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        make_const_int("early", 1),
        OpIR {
            kind: "check_exception".to_string(),
            value: Some(90),
            ..OpIR::default()
        },
    ];
    for i in 0..5 {
        ops.push(make_const_int(&format!("filler_{i}"), i));
    }
    ops.push(OpIR {
        kind: "line".to_string(),
        value: Some(2),
        ..OpIR::default()
    });
    ops.push(make_const_int("cleanup_owned", 99));
    ops.push(OpIR {
        kind: "line".to_string(),
        value: Some(3),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(90),
        ..OpIR::default()
    });
    ops.push(make_ref_op("dec_ref", "cleanup_owned"));
    ops.push(make_op("ret_void"));

    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "cleanup_suffix".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops,
    };

    let (stub, chunks) =
        split_for_test(func, 8).expect("later safe split point should carry cleanup suffix inputs");

    assert_eq!(stub.name, "cleanup_suffix");
    assert_eq!(
        chunks.len(),
        2,
        "the first eligible line boundary is unsafe, so splitting must wait for the cleanup input definition"
    );
    for chunk in &chunks {
        verify_split_function_def_use(chunk).expect("chunk def-use must verify");
    }
    verify_split_function_def_use(&stub).expect("stub def-use must verify");

    let first = &chunks[0];
    let cleanup_def = first
        .ops
        .iter()
        .position(|op| op.out.as_deref() == Some("cleanup_owned"))
        .expect("safe chunk must include the cleanup-owned definition");
    let cleanup_drop = first
        .ops
        .iter()
        .position(|op| {
            op.kind == "dec_ref"
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == "cleanup_owned"))
        })
        .expect("safe chunk must clone the cleanup drop");
    assert!(
        cleanup_def < cleanup_drop,
        "cloned cleanup suffix must not read a value before the extracted chunk defines it"
    );
}

#[test]
fn split_large_function_void_only_stub_returns_none() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "void_only".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: ExecutionContextPolicy::Inherited,
        ops: vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(3),
                ..OpIR::default()
            },
            make_op("ret_void"),
        ],
    };

    let (stub, chunks) = split_for_test(func, 2).expect("expected split");

    assert!(!chunks.is_empty());
    assert_eq!(
        stub.ops.last().map(|op| op.kind.as_str()),
        Some("ret_void"),
        "void-only split stubs must terminate explicitly with ret_void",
    );
}

fn checked_local_split_fixture(returns_value: bool) -> FunctionIR {
    let mut ops = vec![
        OpIR {
            kind: "trace_enter_slot".to_string(),
            value: Some(17),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".to_string(),
            // Deliberately collide with the first synthetic-label candidate.
            value: Some(0),
            ..OpIR::default()
        },
        OpIR {
            kind: "frame_locals_set".to_string(),
            args: Some(vec!["locals".to_string()]),
            ..OpIR::default()
        },
    ];
    for line in 1..=6 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some(format!("value_{line}")),
            ..OpIR::default()
        });
    }
    ops.extend([
        OpIR {
            kind: "trace_exit".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: if returns_value { "ret" } else { "ret_void" }.to_string(),
            // A value from the first chunk exercises heap-frame transport and
            // proves the entry guard precedes synthetic frame allocation.
            args: returns_value.then(|| vec!["value_1".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(0),
            ..OpIR::default()
        },
        make_op("trace_exit"),
        make_op("ret_void"),
    ]);
    FunctionIR {
        return_abi: if returns_value {
            molt_ir::FunctionReturnAbi::Value
        } else {
            molt_ir::FunctionReturnAbi::Void
        },
        name: "framed_large".to_string(),
        params: vec!["locals".to_string()],
        ops,
        source_file: Some("framed_large.py".to_string()),
        execution_context: ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    }
}

#[test]
fn split_local_execution_frame_keeps_lifecycle_in_stub_and_threads_inherited_chunks() {
    let original = checked_local_split_fixture(false);
    let original_entry = original.ops[..2].to_vec();
    let original_failure_tail = original.ops[original.ops.len() - 3..].to_vec();
    let (stub, chunks) = split_for_test(original, 3).expect("expected framed split");
    assert_eq!(stub.execution_context, ExecutionContextPolicy::Local);
    assert_eq!(stub.ops[..2], original_entry);
    assert_eq!(stub.ops[stub.ops.len() - 3..], original_failure_tail);
    assert_eq!(
        stub.ops
            .iter()
            .filter(|op| op.kind == "trace_enter_slot")
            .count(),
        1
    );
    for (index, op) in stub.ops.iter().enumerate() {
        if crate::tir::op_kinds_generated::simpleir_kind_is_return_terminator(op.kind.as_str()) {
            assert_eq!(stub.ops[index - 1].kind, "trace_exit");
        }
    }
    let calls = stub
        .ops
        .iter()
        .filter(|op| op.kind == "call_internal")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), chunks.len());
    assert!(calls.iter().all(|op| op.passes_execution_context));
    assert!(chunks.iter().all(|chunk| {
        chunk.execution_context == ExecutionContextPolicy::Inherited
            && chunk
                .ops
                .iter()
                .all(|op| !matches!(op.kind.as_str(), "trace_enter_slot" | "trace_exit"))
            && chunk.ops.iter().all(|op| {
                !crate::tir::op_kinds_generated::simpleir_kind_uses_function_label_id(&op.kind)
                    || op.value != Some(0)
            })
    }));
    let stub_labels = stub
        .ops
        .iter()
        .filter(|op| {
            crate::tir::op_kinds_generated::simpleir_kind_is_verifier_label_definition(&op.kind)
        })
        .filter_map(|op| op.value)
        .collect::<Vec<_>>();
    assert_eq!(
        stub_labels.len(),
        stub_labels.iter().copied().collect::<BTreeSet<_>>().len(),
        "synthetic labels must not reuse the retained entry-failure label"
    );
    assert!(
        chunks
            .iter()
            .flat_map(|chunk| &chunk.ops)
            .any(|op| matches!(op.kind.as_str(), "line" | "frame_locals_set"))
    );
    crate::validate_simple_ir(&SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    })
    .expect("the transformed SimpleIR execution-context ABI must validate");
}

#[test]
fn split_checked_entry_preserves_value_return_and_chunk_only_drop_authority() {
    let mut original = checked_local_split_fixture(true);
    let typed = crate::tir::lower_from_simple::lower_to_tir(&original);
    original.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);
    assert_eq!(typed.return_abi, original.return_abi);
    let original_entry = original.ops[..2].to_vec();
    let original_failure_tail = original.ops[original.ops.len() - 3..].to_vec();
    original.ops.splice(
        0..0,
        [
            make_op(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            make_op(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        ],
    );
    let (stub, chunks) = split_for_test(original, 3).expect("expected checked value-return split");
    assert_eq!(stub.return_abi, molt_ir::FunctionReturnAbi::Value);
    assert_eq!(stub.ops[..2], original_entry);
    assert_eq!(stub.ops[stub.ops.len() - 3..], original_failure_tail);
    let frame_allocation = stub
        .ops
        .iter()
        .position(|op| op.kind == "list_new")
        .expect("cross-chunk return value requires a split heap frame");
    let allocation_guard = &stub.ops[frame_allocation + 1];
    assert_eq!(allocation_guard.kind, "check_exception");
    let allocation_failure_label = allocation_guard.value.expect("targeted allocation guard");
    assert_ne!(allocation_failure_label, 0);
    assert!(
        stub.ops
            .iter()
            .position(|op| op.kind == "call_internal")
            .is_some_and(|index| index > frame_allocation + 1),
        "allocation failure must be observed before any chunk executes"
    );
    let allocation_cleanup = stub
        .ops
        .iter()
        .position(|op| op.kind == "label" && op.value == Some(allocation_failure_label))
        .expect("allocation failure belongs to the owner's exception return");
    assert_eq!(
        stub.ops[allocation_cleanup..stub.ops.len() - 3]
            .iter()
            .map(|op| op.kind.as_str())
            .collect::<Vec<_>>(),
        ["label", "trace_exit", "ret_void"]
    );
    assert!(chunks.iter().all(|chunk| {
        chunk.ops.iter().all(|op| {
            !crate::tir::op_kinds_generated::simpleir_kind_uses_function_label_id(&op.kind)
                || op.value != Some(allocation_failure_label)
        })
    }));
    assert!(
        stub.ops
            .iter()
            .any(crate::tir::simple_def_use::simple_ir_return_has_value)
    );
    assert!(!stub.ops.iter().any(is_drop_fact_marker_op));
    assert!(chunks.iter().all(|chunk| {
        chunk
            .ops
            .iter()
            .filter(|op| is_drop_fact_marker_op(op))
            .count()
            == 2
            && chunk.ops.iter().take(2).all(is_drop_fact_marker_op)
            && chunk.ops.iter().all(|op| {
                !matches!(op.kind.as_str(), "trace_enter_slot" | "trace_exit")
                    && (!crate::tir::op_kinds_generated::simpleir_kind_uses_function_label_id(
                        &op.kind,
                    ) || op.value != Some(0))
            })
    }));
    for (index, op) in stub.ops.iter().enumerate() {
        if crate::tir::op_kinds_generated::simpleir_kind_is_return_terminator(&op.kind) {
            assert_eq!(stub.ops[index - 1].kind, "trace_exit");
            assert_ne!(stub.ops[index - 2].kind, "trace_exit");
        }
    }
    crate::validate_simple_ir(&SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    })
    .expect("value-returning owner must retain its original void entry-failure return");
}

#[test]
fn split_chunk_protocol_owns_abi_independently_of_owner_payload_and_roundtrip() {
    use molt_ir::FunctionReturnAbi::{Value, Void};

    for (owner_abi, has_payload) in [(Void, false), (Value, false), (Value, true)] {
        for (context, roundtrip) in [
            (ExecutionContextPolicy::Local, false),
            (ExecutionContextPolicy::Local, true),
            (ExecutionContextPolicy::Inherited, false),
            (ExecutionContextPolicy::Inherited, true),
        ] {
            let mut original = checked_local_split_fixture(has_payload);
            original.return_abi = owner_abi;
            original.execution_context = context;
            if context == ExecutionContextPolicy::Inherited {
                original.ops.truncate(original.ops.len() - 3);
                original.ops.drain(..2);
                original.ops.retain(|op| op.kind != "trace_exit");
            }
            if roundtrip {
                let typed = crate::tir::lower_from_simple::lower_to_tir(&original);
                original.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);
            }
            let (stub, chunks) = split_for_test(original, 3)
                .expect("checked entry must survive both source and SSA roundtrip");
            assert_eq!(stub.return_abi, owner_abi);
            assert!(
                chunks.len() > 1,
                "exercise intermediate and terminal chunks"
            );
            assert_eq!(stub.ops.last().unwrap().kind, "ret_void");
            assert_eq!(
                stub.ops
                    .iter()
                    .filter(|op| simple_ir_return_has_value(op))
                    .count(),
                usize::from(has_payload),
                "only the original payload may return a value; no synthetic None exits"
            );
            let calls = stub
                .ops
                .iter()
                .filter(|op| op.kind == "call_internal")
                .collect::<Vec<_>>();
            assert_eq!(calls.len(), chunks.len());
            for (index, (chunk, call)) in chunks.iter().zip(calls).enumerate() {
                assert_eq!(call.s_value.as_deref(), Some(chunk.name.as_str()));
                let result = call.out.as_ref().expect("chunk call result");
                let expected_abi = if has_payload && index + 1 != chunks.len() {
                    Void
                } else {
                    Value
                };
                assert_eq!(chunk.return_abi, expected_abi);
                if !has_payload && index + 1 != chunks.len() {
                    let tail = &chunk.ops[chunk.ops.len() - 2..];
                    assert_eq!(tail[0].kind, "const_bool");
                    assert_eq!(tail[0].value, Some(1), "normal fallthrough must continue");
                    assert_eq!(tail[1].kind, "ret");
                    assert_eq!(
                        tail[1].args.as_deref(),
                        Some(std::slice::from_ref(tail[0].out.as_ref().unwrap()))
                    );
                }
                assert_eq!(
                    chunk.function_signature().unwrap().returns_value,
                    expected_abi == Value
                );
                let consumers = stub
                    .ops
                    .iter()
                    .filter(|op| op.args.as_ref().is_some_and(|args| args.contains(result)))
                    .map(|op| op.kind.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(
                    consumers,
                    if !has_payload {
                        vec!["br_if"]
                    } else if expected_abi == Value {
                        vec!["ret"]
                    } else {
                        vec![]
                    },
                    "chunk signature and caller must agree on the result protocol"
                );
            }
            crate::validate_simple_ir(&SimpleIR {
                functions: std::iter::once(stub).chain(chunks).collect(),
                profile: None,
            })
            .expect("owner ABI and private chunk protocol must validate together");
        }
    }
}

#[test]
fn split_continuation_protocol_avoids_reserved_transport_names() {
    let reserved = [
        "__molt_split_frame",
        "__molt_split_frame_init",
        "__molt_split_frame_index",
        "__molt_split_frame_store_index",
        "__molt_split_chunk_continue",
        "__molt_split_chunk_discard",
        "__molt_split_continue_true",
        "__molt_split_continue_false",
    ]
    .map(str::to_string)
    .to_vec();
    let mut source = adversarial_named_large_function("continuation_collisions");
    source.params = reserved.clone();
    source.ops[3] = make_arith("add", &["first", "__molt_split_frame_index"], "second");
    let (stub, chunks) = split_for_test(source, 2).expect("status transport must split");
    for function in std::iter::once(&stub).chain(&chunks) {
        for op in &function.ops {
            visit_simple_ir_defined_names(op, |name| {
                assert!(!reserved.iter().any(|reserved| reserved == name));
            });
        }
        verify_split_function_def_use(function).expect("transport remains collision-free");
    }
    crate::validate_simple_ir(&SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    })
    .expect("collision-free status transport must preserve its ABI");
}

#[test]
fn split_nonisolated_local_entry_refuses_without_mutating_source_or_names() {
    let mut cases = Vec::new();
    let mut missing_guard = checked_local_split_fixture(false);
    missing_guard.ops.remove(1);
    cases.push(("missing guard", missing_guard, 3));
    let mut delayed_guard = checked_local_split_fixture(false);
    delayed_guard
        .ops
        .insert(1, make_const_int("before_guard", 9));
    cases.push(("delayed guard", delayed_guard, 3));
    let mut untargeted_guard = checked_local_split_fixture(false);
    untargeted_guard.ops[1].value = None;
    cases.push(("untargeted guard", untargeted_guard, 3));
    let mut fallthrough = checked_local_split_fixture(false);
    let tail_start = fallthrough.ops.len() - 3;
    fallthrough.ops.drain(tail_start - 2..tail_start);
    cases.push(("body falls through to entry cleanup", fallthrough, 3));
    let mut nonterminal_tail = checked_local_split_fixture(false);
    nonterminal_tail.ops.push(make_const_int("after_tail", 1));
    cases.push(("nonterminal entry cleanup", nonterminal_tail, 3));
    let mut early_return = checked_local_split_fixture(false);
    early_return
        .ops
        .splice(3..3, [make_op("trace_exit"), make_op("ret_void")]);
    cases.push(("ordinary early return remains ineligible", early_return, 3));

    let mut staged_module = checked_local_split_fixture(false);
    staged_module.name = "molt_init_staged".to_string();
    staged_module.ops.splice(
        0..0,
        [
            make_const_int("module_setup", 1),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(42),
                ..OpIR::default()
            },
        ],
    );
    let tail_start = staged_module.ops.len() - 3;
    staged_module.ops.splice(
        tail_start..tail_start,
        [
            OpIR {
                kind: "label".to_string(),
                value: Some(42),
                ..OpIR::default()
            },
            make_op("ret_void"),
        ],
    );
    cases.push(("pre-entry module setup and cleanup", staged_module, 3));
    let mut shared_rollback = checked_local_split_fixture(false);
    let exit_index = shared_rollback.ops.len() - 2;
    shared_rollback.ops.insert(
        exit_index,
        OpIR {
            kind: "module_cache_del".to_string(),
            args: Some(vec!["locals".to_string()]),
            ..OpIR::default()
        },
    );
    cases.push((
        "entry cleanup also owns module rollback",
        shared_rollback,
        3,
    ));

    for kind in [
        "check_exception",
        "async_work_poll",
        "jump",
        "goto",
        "br_if",
        "try_start",
        "try_end",
        "state_transition",
        "state_yield",
        "chan_recv_yield",
        "chan_send_yield",
        "label",
        "state_label",
    ] {
        let mut shared_label = checked_local_split_fixture(false);
        shared_label.ops.insert(
            3,
            OpIR {
                kind: kind.to_string(),
                value: Some(0),
                ..OpIR::default()
            },
        );
        cases.push((kind, shared_label, 3));
    }
    cases.push(("late size refusal", checked_local_split_fixture(false), 0));
    cases.push((
        "initial size refusal",
        checked_local_split_fixture(false),
        usize::MAX,
    ));
    for (reason, function, max_ops) in cases {
        let mut occupied = BTreeSet::from([function.name.clone(), "untouched".to_string()]);
        let before_names = occupied.clone();
        let mut before = Vec::new();
        crate::write_function_ir_contract(&function, &mut before).unwrap();
        let rejected = split_large_function(function, max_ops, &mut occupied).expect_err(reason);
        let mut after = Vec::new();
        crate::write_function_ir_contract(&rejected, &mut after).unwrap();
        assert_eq!(after, before, "{reason}");
        assert_eq!(occupied, before_names, "{reason}");
    }
}

#[test]
fn split_megafunctions_splits_module_chunks_at_native_default_threshold() {
    let mut ops = Vec::new();
    for i in 0..1401 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(i),
            ..OpIR::default()
        });
        ops.push(make_const_int(&format!("v{i}"), i));
    }
    ops.push(make_op("ret_void"));

    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "builtins__molt_module_chunk_2".to_string(),
            params: vec!["__molt_module_obj__".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: ExecutionContextPolicy::Inherited,
            ops,
        }],
        profile: None,
    };

    let sources =
        super::super::megafunction_split::split_megafunctions_at_limit(&mut ir, 2000, |_| true);
    assert!(
        ir.functions
            .iter()
            .all(|function| function.codegen_partition)
    );
    for (chunk, source) in &sources {
        assert_eq!(source, "builtins__molt_module_chunk_2");
        assert!(ir.functions.iter().any(|function| &function.name == chunk));
    }
    assert_eq!(sources.len() + 1, ir.functions.len());

    let names: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(
        names.contains("builtins__molt_module_chunk_2"),
        "stub must keep the original module chunk symbol"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("__molt_chunk_v1_")),
        "module chunk should be split into backend private chunks at the native default threshold"
    );
}

// Rejection must return the exact input and must not reserve any names,
// including when failure happens after planning rather than at the size gate.
#[test]
fn split_refusal_preserves_contract_and_namespace() {
    for max_ops in [0, usize::MAX] {
        let function = adversarial_named_large_function("unchanged");
        let mut occupied = BTreeSet::from([function.name.clone()]);
        let before_names = occupied.clone();
        let mut before = Vec::new();
        crate::write_function_ir_contract(&function, &mut before).unwrap();
        let rejected = split_large_function(function, max_ops, &mut occupied).unwrap_err();
        let mut after = Vec::new();
        crate::write_function_ir_contract(&rejected, &mut after).unwrap();
        assert_eq!(after, before);
        assert_eq!(occupied, before_names);
    }
}
