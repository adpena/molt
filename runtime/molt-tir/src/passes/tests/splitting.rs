use super::*;

#[test]
fn split_large_function_preserves_protected_runtime_import_entrypoint() {
    let func = FunctionIR {
        name: "molt_isolate_import".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
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

    let result = split_large_function(func, 2);

    let original = result.expect_err("protected import entrypoint must not split");
    assert_eq!(original.name, "molt_isolate_import");
    assert_eq!(original.params, vec!["p0".to_string()]);
    assert_eq!(original.ops.len(), 5);
}

#[test]
fn split_large_function_preserves_protected_runtime_bootstrap_entrypoint() {
    let func = FunctionIR {
        name: "molt_isolate_bootstrap".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops: vec![
            make_op("const_none"),
            make_op("const_none"),
            make_op("const_none"),
            make_op("const_none"),
            make_op("ret_void"),
        ],
    };

    let result = split_large_function(func, 2);

    let original = result.expect_err("protected bootstrap entrypoint must not split");
    assert_eq!(original.name, "molt_isolate_bootstrap");
    assert!(original.params.is_empty());
    assert_eq!(original.ops.len(), 5);
}

#[test]
fn split_large_function_still_splits_regular_large_functions() {
    let func = FunctionIR {
        name: "user_large".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
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

    let (stub, chunks) = split_large_function(func, 2).expect("expected split");

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
    assert!(
        stub.ops
            .iter()
            .any(|op| op.kind == "ret" && op.var.as_deref() == Some("__chunk_ret")),
        "split stub must return the named propagated chunk result",
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.name.starts_with("__molt_chunk_user_large_"))
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
fn split_large_function_preserves_drop_authority_on_chunks_only() {
    let func = FunctionIR {
        name: "drop_inserted_large".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
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

    let (stub, chunks) = split_large_function(func, 2).expect("expected split");

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
        name: "threading__molt_module_chunk_3".to_string(),
        params: vec!["__molt_module_obj__".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
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

    let (stub, chunks) = split_large_function(func, 2).expect("expected split");

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
        name: "__molt_chunk_bad_0".to_string(),
        params: vec!["__molt_split_frame".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
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
        name: "builtins__molt_module_chunk_2".to_string(),
        params: vec!["__molt_module_obj__".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops,
    };

    let (stub, chunks) = split_large_function(func, 40)
        .expect("shared suffix exception handler should not block splitting");

    assert_eq!(stub.name, "builtins__molt_module_chunk_2");
    assert!(chunks.len() >= 2);
    let control_outs: std::collections::BTreeSet<String> = stub
        .ops
        .iter()
        .filter(|op| op.kind == "call_internal")
        .filter_map(|op| op.out.clone())
        .filter(|out| out.starts_with("__chunk_continue_"))
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
        let cloned_skip_labels: Vec<i64> = labels.iter().copied().filter(|id| *id > 523).collect();
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
                        && window[0]
                            .out
                            .as_ref()
                            .is_some_and(|out| window[1].var.as_ref() == Some(out))
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
        name: "cleanup_suffix".to_string(),
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops,
    };

    let (stub, chunks) = split_large_function(func, 8)
        .expect("later safe split point should carry cleanup suffix inputs");

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
        name: "void_only".to_string(),
        params: vec!["p0".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
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

    let (stub, chunks) = split_large_function(func, 2).expect("expected split");

    assert!(!chunks.is_empty());
    assert_eq!(
        stub.ops.last().map(|op| op.kind.as_str()),
        Some("ret_void"),
        "void-only split stubs must terminate explicitly with ret_void",
    );
}

#[test]
fn split_megafunctions_splits_module_chunks_at_native_default_threshold() {
    let previous = std::env::var("MOLT_MAX_FUNCTION_OPS").ok();
    unsafe {
        std::env::remove_var("MOLT_MAX_FUNCTION_OPS");
    }

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
            name: "builtins__molt_module_chunk_2".to_string(),
            params: vec!["__molt_module_obj__".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops,
        }],
        profile: None,
    };

    split_megafunctions(&mut ir);

    let names: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(
        names.contains("builtins__molt_module_chunk_2"),
        "stub must keep the original module chunk symbol"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("__molt_chunk_builtins__molt_module_chunk_2_")),
        "module chunk should be split into backend private chunks at the native default threshold"
    );

    match previous {
        Some(value) => unsafe { std::env::set_var("MOLT_MAX_FUNCTION_OPS", value) },
        None => unsafe { std::env::remove_var("MOLT_MAX_FUNCTION_OPS") },
    }
}
