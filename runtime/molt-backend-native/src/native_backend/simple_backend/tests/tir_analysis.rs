use super::*;

#[test]
fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let analysis = analyze_native_backend_ir(&ir, true);

    assert!(analysis.defined_functions.contains("molt_main"));
}

#[test]
fn native_backend_ir_analysis_collects_task_metadata_once_needed() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    out: Some("flag".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("closure_size".to_string()),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "func_new_closure".to_string(),
                    out: Some("poll_obj".to_string()),
                    s_value: Some("worker_poll".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".to_string(),
                    s_value: Some("__molt_is_coroutine__".to_string()),
                    args: Some(vec!["poll_obj".to_string(), "flag".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".to_string(),
                    s_value: Some("__molt_closure_size__".to_string()),
                    args: Some(vec!["poll_obj".to_string(), "closure_size".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let analysis = analyze_native_backend_ir(&ir, true);

    assert!(analysis.closure_functions.contains("worker_poll"));
    assert_eq!(
        analysis.task_kinds.get("worker_poll"),
        Some(&TrampolineKind::Coroutine)
    );
    assert_eq!(analysis.task_closure_sizes.get("worker_poll"), Some(&3));
}

/// The effective whole-program metadata for a batch is the UNION of the
/// module context (cross-batch) and the batch's LOCAL scan  never a replace
/// (design-20 finding #3C activation). A module context built from a
/// different function set (e.g. the stdlib cache) does NOT carry a
/// closure/task/leaf defined only in this batch; replacing the local scan
/// dropped it, so a `call_guarded` to that closure skipped env extraction and
/// the callee received a garbage closure (`'object' is not subscriptable`).

#[test]
fn effective_metadata_unions_module_context_with_local_scan() {
    // A module context that knows ONLY a stdlib closure / task / leaf.
    let stdlib_funcs = vec![FunctionIR {
        name: "contextlib___inner".to_string(),
        params: vec!["__molt_closure__".to_string()],
        ops: vec![OpIR {
            kind: "func_new_closure".to_string(),
            s_value: Some("contextlib___inner".to_string()),
            out: Some("v0".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    }];
    let ctx = SimpleBackend::build_module_context(&stdlib_funcs);
    assert!(ctx.closure_functions.contains("contextlib___inner"));

    // The current batch defines its OWN closure that the context never saw.
    let mut local_closures = BTreeSet::new();
    local_closures.insert("app__inner".to_string());
    let merged = merge_closure_functions(Some(&ctx), local_closures);
    assert!(
        merged.contains("app__inner"),
        "the batch's own closure must survive the merge (no replace)"
    );
    assert!(
        merged.contains("contextlib___inner"),
        "the module context's cross-batch closures must also be present"
    );

    // None context  pure local (user-only / non-batched build).
    let mut only_local = BTreeSet::new();
    only_local.insert("app__inner".to_string());
    let merged_none = merge_closure_functions(None, only_local);
    assert!(merged_none.contains("app__inner"));
    assert_eq!(merged_none.len(), 1);

    // Same union contract for task kinds and leaf functions.
    let mut local_tasks = BTreeMap::new();
    local_tasks.insert("app_poll".to_string(), TrampolineKind::Coroutine);
    let merged_tasks = merge_task_kinds(Some(&ctx), local_tasks);
    assert_eq!(
        merged_tasks.get("app_poll"),
        Some(&TrampolineKind::Coroutine)
    );
    let mut local_leaves = BTreeSet::new();
    local_leaves.insert("app_leaf".to_string());
    let merged_leaves = merge_leaf_functions(Some(&ctx), local_leaves);
    assert!(merged_leaves.contains("app_leaf"));
    assert!(merged_leaves.contains("contextlib___inner"));
}

#[test]
fn native_backend_module_context_preserves_cross_batch_alias_metadata() {
    let functions = vec![
        FunctionIR {
            name: "helper".to_string(),
            params: vec!["value".to_string(), "intrinsic".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                var: Some("value".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
        FunctionIR {
            name: "helper_poll".to_string(),
            params: vec!["state".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                var: Some("state".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
    ];

    let context = SimpleBackend::build_module_context(&functions);

    assert_eq!(context.function_arities.get("helper"), Some(&2));
    assert_eq!(context.function_has_ret.get("helper"), Some(&true));
    assert_eq!(
        context.return_alias_summaries.get("helper"),
        Some(&ReturnAliasSummary::Param(0))
    );
    assert!(context.leaf_functions.contains("helper"));
    assert!(context.leaf_functions.contains("helper_poll"));
}

#[test]
fn tir_roundtrip_preserves_store_var_return_alias_summary() {
    let func = FunctionIR {
        name: "helper".to_string(),
        params: vec!["value".to_string()],
        ops: vec![
            OpIR {
                kind: "store_var".to_string(),
                var: Some("tmp".to_string()),
                args: Some(vec!["value".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("tmp".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["str".to_string()]),
        source_file: None,
        is_extern: false,
    };

    let roundtripped = roundtrip_function_through_tir(&func);
    let summaries =
        crate::passes::compute_return_alias_summaries(std::slice::from_ref(&roundtripped));

    assert_eq!(
        summaries.get("helper"),
        Some(&ReturnAliasSummary::Param(0)),
        "roundtripped params: {:?}; ops: {:?}; summaries: {:?}",
        roundtripped.params,
        roundtripped.ops,
        summaries
    );
}

#[test]
fn native_backend_module_context_preserves_cross_batch_void_return_metadata() {
    let functions = vec![
        FunctionIR {
            name: "value_helper".to_string(),
            params: vec!["value".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                var: Some("value".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
        FunctionIR {
            name: "void_helper".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
    ];

    let context = SimpleBackend::build_module_context(&functions);

    assert_eq!(context.function_has_ret.get("value_helper"), Some(&true));
    assert_eq!(context.function_has_ret.get("void_helper"), Some(&false));
}
