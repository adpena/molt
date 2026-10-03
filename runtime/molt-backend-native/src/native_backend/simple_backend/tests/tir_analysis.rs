use super::*;

#[test]
fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let analysis = analyze_native_backend_ir(
        &ir,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
    );

    assert!(analysis.defined_functions.contains("molt_main"));
}

#[test]
fn native_backend_ir_analysis_collects_task_metadata_once_needed() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "func_new_closure".to_string(),
                out: Some("poll_obj".to_string()),
                s_value: Some("worker_poll".to_string()),
                task_kind: Some("coroutine".to_string()),
                task_closure_size: Some(3),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let analysis = analyze_native_backend_ir(
        &ir,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
    );

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
/// closure/task defined only in this batch; replacing the local scan
/// dropped it, so a `call_guarded` to that closure skipped env extraction and
/// the callee received a garbage closure (`'object' is not subscriptable`).

#[test]
fn effective_metadata_unions_module_context_with_local_scan() {
    // A module context that knows ONLY a stdlib closure / task.
    let mut stdlib_funcs = vec![FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "contextlib___inner".to_string(),
        params: vec![molt_ir::MOLT_CLOSURE_PARAM_NAME.to_string()],
        ops: vec![OpIR {
            kind: "func_new_closure".to_string(),
            s_value: Some("contextlib___inner".to_string()),
            out: Some("v0".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }];
    let ctx = SimpleBackend::prepare_module_context(&mut stdlib_funcs);
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

    // Same union contract for task kinds. Callback facts are finalized locally.
    let mut local_tasks = BTreeMap::new();
    local_tasks.insert("app_poll".to_string(), TrampolineKind::Coroutine);
    let merged_tasks = merge_task_kinds(Some(&ctx), local_tasks);
    assert_eq!(
        merged_tasks.get("app_poll"),
        Some(&TrampolineKind::Coroutine)
    );
}

#[test]
fn native_backend_module_context_preserves_cross_batch_function_metadata() {
    let mut functions = vec![
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "helper".to_string(),
            params: vec!["value".to_string(), "intrinsic".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["value".to_string()]),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "helper_poll".to_string(),
            params: vec!["state".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["state".to_string()]),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    let context = SimpleBackend::prepare_module_context(&mut functions);

    assert_eq!(context.function_arities.get("helper"), Some(&2));
    assert_eq!(context.function_has_ret.get("helper"), Some(&true));
    assert!(
        serde_json::to_value(&context)
            .unwrap()
            .get("leaf_functions")
            .is_none()
    );
}

#[test]
fn native_backend_module_context_preserves_cross_batch_void_return_metadata() {
    let mut functions = vec![
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "value_helper".to_string(),
            params: vec!["value".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["value".to_string()]),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "void_helper".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    let context = SimpleBackend::prepare_module_context(&mut functions);

    assert_eq!(context.function_has_ret.get("value_helper"), Some(&true));
    assert_eq!(context.function_has_ret.get("void_helper"), Some(&false));
}

// This fixture exercises the production pre-batch preparation, not a proxy
// splitter. The normal proof environment uses the default 2000-op boundary.
#[test]
fn prepare_module_context_bounds_bodies_before_freezing_added_linkage_rows() {
    let mut ops = Vec::new();
    for line in 0..1401 {
        ops.push(OpIR {
            kind: "line".into(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const".into(),
            value: Some(line),
            out: Some(format!("unused_{line}")),
            ..OpIR::default()
        });
    }
    ops.push(OpIR {
        kind: "ret_void".into(),
        ..OpIR::default()
    });
    let mut functions = vec![FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "app__large".into(),
        execution_context: crate::ir::ExecutionContextPolicy::Inherited,
        ops,
        ..FunctionIR::default()
    }];
    let context = SimpleBackend::prepare_module_context(&mut functions);
    assert!(
        functions.len() > 1,
        "production preparation must split before TIR"
    );
    assert!(functions.iter().all(|function| function.codegen_partition));
    for function in &functions {
        assert_eq!(context.original_function_name(&function.name), "app__large");
        let abi = context.function_linkage_abi(&function.name).unwrap();
        assert_eq!(abi.source_signature, function.function_signature().unwrap());
    }
    context.validate_function_linkage_abis(&functions).unwrap();
}

#[test]
fn shared_context_cannot_override_final_lifetime_callback_facts() {
    let provider = FunctionIR {
        name: "released_local".into(),
        params: vec!["owned".into()],
        return_abi: molt_ir::FunctionReturnAbi::Void,
        ops: vec![OpIR {
            kind: "ret_void".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut early = vec![provider.clone()];
    let context = SimpleBackend::prepare_module_context(&mut early);
    assert!(
        serde_json::to_value(&context)
            .unwrap()
            .get("leaf_functions")
            .is_none()
    );

    // Lifetime finalization can add this callback after the ABI context was
    // frozen. The final-body authority must observe it for batched builds too.
    let mut finalized = provider;
    finalized.ops.insert(
        0,
        OpIR {
            kind: "drop_inserted".into(),
            ..Default::default()
        },
    );
    finalized.ops.insert(
        0,
        OpIR {
            kind: "dec_ref".into(),
            args: Some(vec!["owned".into()]),
            ..Default::default()
        },
    );
    let mut pure = FunctionIR {
        name: "pure_local".into(),
        return_abi: molt_ir::FunctionReturnAbi::Void,
        ops: vec![OpIR {
            kind: "ret_void".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut unfinalized = pure.clone();
    unfinalized.name = "unfinalized_local".into();
    let mut exception_only = pure.clone();
    exception_only.name = "exception_only_local".into();
    exception_only.ops.insert(
        0,
        OpIR {
            kind: "exception_region_drops_inserted".into(),
            ..Default::default()
        },
    );
    pure.ops.insert(
        0,
        OpIR {
            kind: "drop_inserted".into(),
            ..Default::default()
        },
    );
    let ir = SimpleIR {
        functions: vec![finalized, pure, unfinalized, exception_only],
        profile: None,
    };
    let analysis = analyze_native_backend_ir(
        &ir,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
    );
    assert!(!analysis.leaf_functions.contains("released_local"));
    assert!(analysis.leaf_functions.contains("pure_local"));
    assert!(!analysis.leaf_functions.contains("unfinalized_local"));
    assert!(!analysis.leaf_functions.contains("exception_only_local"));
}
