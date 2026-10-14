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
        final_callbacks_for_test(&ir.functions),
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
        final_callbacks_for_test(&ir.functions),
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
        final_callbacks_for_test(&ir.functions),
    );
    assert!(!analysis.leaf_functions.contains("released_local"));
    assert!(analysis.leaf_functions.contains("pure_local"));
    assert!(!analysis.leaf_functions.contains("unfinalized_local"));
    assert!(!analysis.leaf_functions.contains("exception_only_local"));
}

fn callback_op(kind: &str, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        kind: kind.into(),
        args: (!args.is_empty()).then(|| args.iter().map(|name| (*name).into()).collect()),
        out: out.map(str::to_string),
        ..Default::default()
    }
}

fn callback_function(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.into(),
        params: params.iter().map(|name| (*name).into()).collect(),
        return_abi: molt_ir::FunctionReturnAbi::Void,
        ops,
        ..Default::default()
    }
}

fn assert_callback_rewrite_preserves_oracle(mut function: FunctionIR) -> FunctionIR {
    function
        .ops
        .insert(0, callback_op("drop_inserted", &[], None));
    let before = final_leaf_oracle(std::slice::from_ref(&function));
    let callbacks = final_callbacks_for_test(std::slice::from_ref(&function));
    super::super::program_pipeline::run_post_tir_simple_ir_rewrites(std::slice::from_mut(
        &mut function,
    ));
    let after = final_leaf_oracle(std::slice::from_ref(&function));
    assert_eq!(
        before, after,
        "callback projection changed for {}",
        function.name
    );
    let ir = SimpleIR {
        functions: vec![function.clone()],
        profile: None,
    };
    let analysis = analyze_native_backend_ir(
        &ir,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
        callbacks,
    );
    assert_eq!(analysis.leaf_functions, after);
    function
}

#[test]
fn final_bridge_rewrites_preserve_typed_callback_eligibility() {
    let _guard = backend_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut scalar = callback_op("const", &[], Some("source"));
    scalar.value = Some(7);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "scalar_alias",
        &[],
        vec![
            scalar,
            callback_op("copy_var", &["source"], Some("alias")),
            callback_op("dec_ref", &["alias"], None),
            callback_op("ret_void", &[], None),
        ],
    ));
    assert_eq!(rewritten.ops[2].kind, "nop");
    assert!(final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("scalar_alias"));

    let mut poll = callback_op("async_work_poll", &[], None);
    poll.value = Some(9);
    let mut handler = callback_op("label", &[], None);
    handler.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "poll_callback",
        &[],
        vec![
            poll,
            callback_op("ret_void", &[], None),
            handler,
            callback_op("ret_void", &[], None),
        ],
    ));
    assert!(rewritten.ops.iter().any(|op| op.kind == "async_work_poll"));
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("poll_callback"));

    // The schema's admitted non-CheckException poll carrier retains its typed
    // marker; CheckException itself back-converts to the distinct poll kind.
    let mut observer = callback_op("exception_finally_pending_observer", &[], Some("pending"));
    observer.async_work_poll = true;
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "observer_callback",
        &[],
        vec![observer, callback_op("ret_void", &[], None)],
    ));
    assert!(
        rewritten
            .ops
            .iter()
            .any(|op| op.kind == "exception_finally_pending_observer" && op.is_async_work_poll())
    );
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("observer_callback"));

    let mut conditional = callback_op("br_if", &["condition"], None);
    conditional.value = Some(9);
    let mut handler = callback_op("label", &[], None);
    handler.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "truthiness_callback",
        &["condition"],
        vec![
            conditional,
            callback_op("ret_void", &[], None),
            handler,
            callback_op("ret_void", &[], None),
        ],
    ));
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("truthiness_callback"));
    for mutable in [false, true] {
        let mut copy = callback_op("copy_var", &["source"], Some("alias"));
        if mutable {
            copy.var = Some("source".into());
        }
        let mut ops = vec![copy];
        if mutable {
            let mut store = callback_op("store_var", &["replacement"], None);
            store.var = Some("source".into());
            ops.push(store);
        }
        ops.extend([
            callback_op("dec_ref", &["alias"], None),
            callback_op("binding_alias", &["source"], Some("owned_alias")),
            callback_op("ret_void", &[], None),
        ]);
        let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
            "alias_callbacks",
            &["source", "replacement"],
            ops,
        ));
        assert_eq!(
            rewritten.ops[1].kind,
            if mutable { "copy_var" } else { "nop" }
        );
        assert!(rewritten.ops.iter().any(|op| op.kind == "binding_alias"));
        assert!(rewritten.ops.iter().any(|op| op.kind == "dec_ref"));
    }

    for (tag, guard_output, transformed) in [
        (5, None, true),
        (1, None, false),
        (5, Some("observed"), false),
    ] {
        let mut tag_op = callback_op("const", &[], Some("tag"));
        tag_op.value = Some(tag);
        let mut check = callback_op("check_exception", &[], None);
        check.value = Some(9);
        let mut label = callback_op("label", &[], None);
        label.value = Some(9);
        let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
            "field_callbacks",
            &["hay", "sep", "idx"],
            vec![
                tag_op,
                callback_op("string_split_field", &["hay", "sep", "idx"], Some("field")),
                check,
                callback_op("guard_type", &["field", "tag"], guard_output),
                callback_op("len", &["field"], Some("length")),
                callback_op("ret_void", &[], None),
                label,
                callback_op("ret_void", &[], None),
            ],
        ));
        assert_eq!(
            rewritten
                .ops
                .iter()
                .any(|op| op.kind == "string_split_field_start"),
            transformed
        );
        assert_eq!(
            rewritten.ops.iter().any(|op| op.kind == "guard_type"),
            !transformed
        );
    }

    let mut constructor = callback_op("exception_new_builtin", &[], Some("exc"));
    constructor.value = Some(5);
    let mut check = callback_op("check_exception", &[], None);
    check.value = Some(9);
    let mut jump = callback_op("jump", &[], None);
    jump.value = Some(9);
    let mut label = callback_op("label", &[], None);
    label.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "raise_callbacks",
        &[],
        vec![
            constructor,
            check.clone(),
            callback_op("raise", &["exc"], None),
            check,
            jump,
            label,
            callback_op("ret_void", &[], None),
        ],
    ));
    assert!(!rewritten.ops.iter().any(|op| op.kind == "check_exception"));
    assert!(rewritten.ops.iter().any(|op| op.kind == "raise"));
}

#[test]
fn native_final_callback_custody_requires_every_local_and_exact_lifetime_authority() {
    let pure = callback_function("pure", &[], vec![callback_op("ret_void", &[], None)]);
    let mut callbacks = final_callbacks_for_test(std::slice::from_ref(&pure));
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callbacks.record(&crate::tir::lower_from_simple::lower_to_tir(&pure));
    }));
    assert!(duplicate.is_err(), "duplicate producers must fail loud");

    for missing in [false, true] {
        let callbacks = if missing {
            NativeFinalCallbacks::default()
        } else {
            final_callbacks_for_test(std::slice::from_ref(&pure))
        };
        let mut final_body = pure.clone();
        final_body
            .ops
            .insert(0, callback_op("drop_inserted", &[], None));
        let ir = SimpleIR {
            functions: vec![final_body],
            profile: None,
        };
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            analyze_native_backend_ir(
                &ir,
                molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
                callbacks,
            );
        }));
        assert!(
            failed.is_err(),
            "missing={missing}: incomplete or stale custody must fail loud"
        );
    }
}

#[test]
fn native_preparation_streams_final_callbacks_for_module_and_batch_paths() {
    let _guard = backend_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for skip_ir_passes in [false, true] {
        for callback in [false, true] {
            let ops = if callback {
                vec![
                    callback_op("dec_ref", &["owned"], None),
                    callback_op("ret_void", &[], None),
                ]
            } else {
                vec![callback_op("ret_void", &[], None)]
            };
            let mut ir = SimpleIR {
                functions: vec![callback_function("molt_main", &["owned"], ops)],
                profile: None,
            };
            let mut backend = SimpleBackend::new();
            backend.skip_ir_passes = skip_ir_passes;
            backend.skip_shared_stdlib_partition = true;
            backend.emit_app_callable_resolver = false;
            let prepared = backend.prepare_program_for_codegen(
                &mut ir,
                false,
                false,
                &std::time::Instant::now(),
            );
            let expected = final_leaf_oracle(&ir.functions);
            let analysis =
                analyze_native_backend_ir(&ir, prepared.source_callables, prepared.final_callbacks);
            assert_eq!(
                analysis.leaf_functions, expected,
                "skip={skip_ir_passes}, callback={callback}"
            );
            assert_eq!(analysis.leaf_functions.contains("molt_main"), !callback);
        }
    }
}

#[test]
fn final_callback_projection_retires_dead_and_externalized_bodies() {
    let functions: Vec<_> = ["local", "dead", "externalized"]
        .into_iter()
        .map(|name| {
            callback_function(
                name,
                &[],
                vec![
                    callback_op("drop_inserted", &[], None),
                    callback_op("ret_void", &[], None),
                ],
            )
        })
        .collect();
    let callbacks = final_callbacks_for_test(&functions);
    let external = functions[2].extern_declaration().unwrap();
    let ir = SimpleIR {
        functions: vec![functions[0].clone(), external],
        profile: None,
    };
    let analysis = analyze_native_backend_ir(
        &ir,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
        callbacks,
    );
    assert_eq!(analysis.leaf_functions, BTreeSet::from(["local".into()]));
    assert_eq!(analysis.leaf_functions, final_leaf_oracle(&ir.functions));
}
