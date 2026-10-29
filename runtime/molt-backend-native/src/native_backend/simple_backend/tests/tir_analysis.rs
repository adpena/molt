use super::*;
use molt_ir::OpKind;

#[test]
fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                ..OpIR::new(OpKind::Ret)
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
        &final_functions_for_test(&ir.functions),
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
                out: Some("poll_obj".to_string()),
                s_value: Some("worker_poll".to_string()),
                task_kind: Some("coroutine".to_string()),
                task_closure_size: Some(3),
                ..OpIR::new(OpKind::FuncNewClosure)
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
        &final_functions_for_test(&ir.functions),
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
            s_value: Some("contextlib___inner".to_string()),
            out: Some("v0".to_string()),
            ..OpIR::new(OpKind::FuncNewClosure)
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
                args: Some(vec!["value".to_string()]),
                ..OpIR::new(OpKind::Ret)
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
                args: Some(vec!["state".to_string()]),
                ..OpIR::new(OpKind::Ret)
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
                args: Some(vec!["value".to_string()]),
                ..OpIR::new(OpKind::Ret)
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
                ..OpIR::new(OpKind::RetVoid)
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
            value: Some(line),
            ..OpIR::new(OpKind::Line)
        });
        ops.push(OpIR {
            value: Some(line),
            out: Some(format!("unused_{line}")),
            ..OpIR::new(OpKind::Const)
        });
    }
    ops.push(OpIR {
        ..OpIR::new(OpKind::RetVoid)
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
            ..OpIR::new(OpKind::RetVoid)
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
            ..OpIR::new(OpKind::DropInserted)
        },
    );
    finalized.ops.insert(
        0,
        OpIR {
            args: Some(vec!["owned".into()]),
            ..OpIR::new(OpKind::DecRef)
        },
    );
    let mut pure = FunctionIR {
        name: "pure_local".into(),
        return_abi: molt_ir::FunctionReturnAbi::Void,
        ops: vec![OpIR {
            ..OpIR::new(OpKind::RetVoid)
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
            ..OpIR::new(OpKind::ExceptionRegionDropsInserted)
        },
    );
    pure.ops.insert(
        0,
        OpIR {
            ..OpIR::new(OpKind::DropInserted)
        },
    );
    let ir = SimpleIR {
        functions: vec![finalized, pure, unfinalized, exception_only],
        profile: None,
    };
    let analysis = analyze_native_backend_ir(
        &final_functions_for_test(&ir.functions),
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
    );
    assert!(!analysis.leaf_functions.contains("released_local"));
    assert!(analysis.leaf_functions.contains("pure_local"));
    assert!(!analysis.leaf_functions.contains("unfinalized_local"));
    assert!(!analysis.leaf_functions.contains("exception_only_local"));
}

fn callback_op(kind: OpKind, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        args: (!args.is_empty()).then(|| args.iter().map(|name| (*name).into()).collect()),
        out: out.map(str::to_string),
        ..OpIR::new(kind)
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
        .insert(0, callback_op(OpKind::DropInserted, &[], None));
    let before = final_leaf_oracle(std::slice::from_ref(&function));
    let mut typed = crate::tir::lower_from_simple::lower_to_tir(&function);
    crate::tir::drop_phase::prepare_final_backend_lowering(
        &mut typed,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );
    crate::tir::type_refine::refine_types(&mut typed);
    let finalized = NativeFinalFunctions::from_final_tir(
        SimpleIR {
            functions: vec![function],
            profile: None,
        },
        vec![(false, typed)],
    );
    let after = final_leaf_oracle(&finalized.ir().functions);
    assert_eq!(
        before, after,
        "typed final rewrite changed callback eligibility"
    );
    let analysis = analyze_native_backend_ir(
        &finalized,
        molt_tir::trampolines::CallableMetadata::from_functions(&finalized.ir().functions),
    );
    assert_eq!(analysis.leaf_functions, after);
    finalized
        .into_local_functions()
        .next()
        .unwrap()
        .into_parts()
        .0
}

#[test]
fn final_bridge_rewrites_preserve_typed_callback_eligibility() {
    let _guard = backend_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut scalar = callback_op(OpKind::Const, &[], Some("source"));
    scalar.value = Some(7);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "scalar_alias",
        &[],
        vec![
            scalar,
            callback_op(OpKind::CopyVar, &["source"], Some("alias")),
            callback_op(OpKind::DecRef, &["alias"], None),
            callback_op(OpKind::RetVoid, &[], None),
        ],
    ));
    assert!(
        !rewritten
            .ops
            .iter()
            .any(|op| op.out.as_deref() == Some("alias"))
    );
    assert!(final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("scalar_alias"));

    let mut poll = callback_op(OpKind::AsyncWorkPoll, &[], None);
    poll.value = Some(9);
    let mut handler = callback_op(OpKind::Label, &[], None);
    handler.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "poll_callback",
        &[],
        vec![
            poll,
            callback_op(OpKind::RetVoid, &[], None),
            handler,
            callback_op(OpKind::RetVoid, &[], None),
        ],
    ));
    assert!(
        rewritten
            .ops
            .iter()
            .any(|op| op.kind == OpKind::AsyncWorkPoll)
    );
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("poll_callback"));

    // The schema's admitted non-CheckException poll carrier retains its typed
    // marker; CheckException itself back-converts to the distinct poll kind.
    let mut observer = callback_op(
        OpKind::ExceptionFinallyPendingObserver,
        &[],
        Some("pending"),
    );
    observer.async_work_poll = true;
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "observer_callback",
        &[],
        vec![observer, callback_op(OpKind::RetVoid, &[], None)],
    ));
    assert!(
        rewritten
            .ops
            .iter()
            .any(|op| op.kind == OpKind::ExceptionFinallyPendingObserver && op.is_async_work_poll())
    );
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("observer_callback"));

    let mut conditional = callback_op(OpKind::BrIf, &["condition"], None);
    conditional.value = Some(9);
    let mut handler = callback_op(OpKind::Label, &[], None);
    handler.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "truthiness_callback",
        &["condition"],
        vec![
            conditional,
            callback_op(OpKind::RetVoid, &[], None),
            handler,
            callback_op(OpKind::RetVoid, &[], None),
        ],
    ));
    assert!(!final_leaf_oracle(std::slice::from_ref(&rewritten)).contains("truthiness_callback"));
    for mutable in [false, true] {
        let mut copy = callback_op(OpKind::CopyVar, &["source"], Some("alias"));
        if mutable {
            copy.var = Some("source".into());
        }
        let mut ops = vec![copy];
        if mutable {
            let mut store = callback_op(OpKind::StoreVar, &["replacement"], None);
            store.var = Some("source".into());
            ops.push(store);
        }
        ops.extend([
            callback_op(OpKind::DecRef, &["alias"], None),
            callback_op(OpKind::BindingAlias, &["source"], Some("owned_alias")),
            callback_op(OpKind::RetVoid, &[], None),
        ]);
        let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
            "alias_callbacks",
            &["source", "replacement"],
            ops,
        ));
        // The shared SSA owner preserves a mutable load's value at the load
        // point; immutable identity copies no longer need a source nop.
        if mutable {
            assert!(rewritten.ops.iter().any(|op| op.kind == OpKind::StoreVar));
        }
        assert!(
            rewritten
                .ops
                .iter()
                .any(|op| op.kind == OpKind::BindingAlias)
        );
        assert!(rewritten.ops.iter().any(|op| op.kind == OpKind::DecRef));
    }

    for (tag, guard_output, transformed) in [
        (5, None, true),
        (1, None, false),
        (5, Some("observed"), false),
    ] {
        let mut tag_op = callback_op(OpKind::Const, &[], Some("tag"));
        tag_op.value = Some(tag);
        let mut check = callback_op(OpKind::CheckException, &[], None);
        check.value = Some(9);
        let mut label = callback_op(OpKind::Label, &[], None);
        label.value = Some(9);
        let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
            "field_callbacks",
            &["hay", "sep", "idx"],
            vec![
                tag_op,
                callback_op(
                    OpKind::StringSplitField,
                    &["hay", "sep", "idx"],
                    Some("field"),
                ),
                check,
                callback_op(OpKind::GuardType, &["field", "tag"], guard_output),
                callback_op(OpKind::Len, &["field"], Some("length")),
                callback_op(OpKind::RetVoid, &[], None),
                label,
                callback_op(OpKind::RetVoid, &[], None),
            ],
        ));
        assert_eq!(
            rewritten
                .ops
                .iter()
                .any(|op| op.kind == OpKind::StringSplitFieldStart),
            transformed
        );
        assert_eq!(
            rewritten.ops.iter().any(|op| op.kind == OpKind::GuardType),
            !transformed
        );
    }

    let mut constructor = callback_op(OpKind::ExceptionNewBuiltinEmpty, &[], Some("exc"));
    constructor.value = Some(5);
    let mut check = callback_op(OpKind::CheckException, &[], None);
    check.value = Some(9);
    let mut jump = callback_op(OpKind::Jump, &[], None);
    jump.value = Some(9);
    let mut label = callback_op(OpKind::Label, &[], None);
    label.value = Some(9);
    let rewritten = assert_callback_rewrite_preserves_oracle(callback_function(
        "raise_callbacks",
        &[],
        vec![
            constructor,
            check.clone(),
            callback_op(OpKind::Raise, &["exc"], None),
            check,
            jump,
            label,
            callback_op(OpKind::RetVoid, &[], None),
        ],
    ));
    assert!(
        !rewritten
            .ops
            .iter()
            .any(|op| op.kind == OpKind::CheckException)
    );
    assert!(rewritten.ops.iter().any(|op| op.kind == OpKind::Raise));
}

#[test]
fn native_final_custody_requires_every_local_and_rejects_wrong_identity() {
    let pure = callback_function("pure", &[], vec![callback_op(OpKind::RetVoid, &[], None)]);
    for mode in ["missing", "duplicate", "wrong_name", "wrong_params"] {
        let mut typed = crate::tir::lower_from_simple::lower_to_tir(&pure);
        if mode == "wrong_name" {
            typed.name = "other".into();
        }
        if mode == "wrong_params" {
            typed.param_names.push("other".into());
        }
        let typed = match mode {
            "missing" => vec![],
            "duplicate" => vec![(false, typed.clone()), (false, typed)],
            _ => vec![(false, typed)],
        };
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            NativeFinalFunctions::from_final_tir(
                SimpleIR {
                    functions: vec![pure.clone()],
                    profile: None,
                },
                typed,
            )
        }));
        assert!(failed.is_err(), "{mode}: invalid custody must fail loud");
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
                    callback_op(OpKind::DecRef, &["owned"], None),
                    callback_op(OpKind::RetVoid, &[], None),
                ]
            } else {
                vec![callback_op(OpKind::RetVoid, &[], None)]
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
            let finalized = prepared.final_functions.expect("native final owner");
            assert!(
                ir.functions.is_empty(),
                "source custody moved into final owner"
            );
            let expected = final_leaf_oracle(&finalized.ir().functions);
            let analysis = analyze_native_backend_ir(&finalized, prepared.source_callables);
            assert_eq!(
                analysis.leaf_functions, expected,
                "skip={skip_ir_passes}, callback={callback}"
            );
            assert_eq!(analysis.leaf_functions.contains("molt_main"), !callback);
        }
    }
}

#[test]
fn final_function_custody_retires_dead_bodies_and_preserves_externs() {
    let functions: Vec<_> = ["local", "dead", "externalized"]
        .into_iter()
        .map(|name| {
            callback_function(
                name,
                &[],
                vec![
                    callback_op(OpKind::DropInserted, &[], None),
                    callback_op(OpKind::RetVoid, &[], None),
                ],
            )
        })
        .collect();
    let mut sources = functions.clone();
    sources[2] = sources[2].extern_declaration().unwrap();
    let mut finalized = final_functions_for_test(&sources);
    finalized.retain_reachable(&BTreeSet::from(["externalized".into()]));
    let analysis = analyze_native_backend_ir(
        &finalized,
        molt_tir::trampolines::CallableMetadata::from_functions(&finalized.ir().functions),
    );
    assert_eq!(analysis.leaf_functions, BTreeSet::from(["local".into()]));
    assert_eq!(
        analysis.leaf_functions,
        final_leaf_oracle(&finalized.ir().functions)
    );
    let emitted: Vec<_> = finalized
        .into_local_functions()
        .map(|function| function.function().name.clone())
        .collect();
    assert_eq!(emitted, vec!["local"]);
}

#[test]
fn final_partition_retires_body_facts_together_and_preserves_source_order() {
    let _guard = acquire_backend_env_lock();
    let executable = std::env::current_exe().unwrap();
    let _stdlib = ScopedEnvVar::set("MOLT_STDLIB_OBJ", executable.to_str());
    let _entry = ScopedEnvVar::set("MOLT_ENTRY_MODULE", Some("application"));
    let _symbols = ScopedEnvVar::set("MOLT_STDLIB_MODULE_SYMBOLS", Some("[\"stdlib\"]"));
    let _dfe = ScopedEnvVar::set("MOLT_DISABLE_DEAD_FUNC_ELIM", None);
    let mut entry = callback_function(
        "molt_main",
        &[],
        vec![
            callback_op(OpKind::DropInserted, &[], None),
            callback_op(OpKind::CallFunc, &[], None),
            callback_op(OpKind::CallFunc, &[], None),
            callback_op(OpKind::RetVoid, &[], None),
        ],
    );
    entry.ops[1].s_value = Some("stdlib__helper".into());
    entry.ops[2].s_value = Some("application__helper".into());
    let pure = |name| {
        callback_function(
            name,
            &[],
            vec![
                callback_op(OpKind::DropInserted, &[], None),
                callback_op(OpKind::RetVoid, &[], None),
            ],
        )
    };
    let sources = vec![
        entry,
        pure("stdlib__helper"),
        pure("dead"),
        pure("application__helper"),
        pure("compiler_partition"),
    ];
    let roots = BTreeSet::from(["compiler_partition".into()]);
    let partitions = BTreeMap::from([("compiler_partition".into(), "stdlib__other".into())]);
    for needs_reachability in [false, true] {
        let mut finalized = final_functions_for_test(&sources);
        assert!(
            analyze_native_backend_ir(&finalized, Default::default())
                .leaf_functions
                .contains("stdlib__helper")
        );
        if !needs_reachability {
            finalized.retain_reachable(&roots);
        }
        let mut oracle = finalized.ir().clone();
        crate::native_backend::simple_backend::externalize_shared_stdlib_partition(
            &mut oracle,
            &roots,
            &partitions,
        );
        finalized.externalize_partition(needs_reachability, &roots, &partitions);
        let rows = |functions: &[FunctionIR]| {
            functions
                .iter()
                .map(|f| {
                    (
                        f.name.clone(),
                        f.is_extern,
                        f.return_abi,
                        f.parameter_custody.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&finalized.ir().functions), rows(&oracle.functions));
        assert_eq!(
            finalized
                .ir()
                .functions
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            [
                "molt_main",
                "application__helper",
                "stdlib__helper",
                "compiler_partition"
            ]
        );
        let analysis = analyze_native_backend_ir(&finalized, Default::default());
        assert!(!analysis.leaf_functions.contains("stdlib__helper"));
        assert!(!analysis.leaf_functions.contains("compiler_partition"));
        assert!(!analysis.leaf_functions.contains("dead"));
        assert_eq!(
            analysis.leaf_functions,
            final_leaf_oracle(&finalized.ir().functions)
        );
        let names = finalized
            .into_local_functions()
            .map(|f| f.function().name.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, ["molt_main", "application__helper"]);
    }
}

#[test]
fn final_function_custody_is_identical_after_cold_and_warm_cache_transport() {
    use crate::tir::pipeline_cache::{
        TirOwnedModulePipelineMode, TirOwnedModulePipelineOptions, TirPipelineCacheFlavor,
        TirPipelineInput, TirPipelineRunOptions, run_cached_tir_pipeline,
        run_owned_module_pipeline_from_cached_tir,
    };
    let _guard = acquire_backend_env_lock();
    let cache_dir = std::env::temp_dir().join(format!(
        "molt-native-final-custody-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let target = crate::tir::TargetInfo::native_release_fast();
    let mut seven = callback_op(OpKind::Const, &[], Some("seven"));
    seven.value = Some(7);
    let mut source = callback_function(
        "cached_final",
        &[],
        vec![seven, callback_op(OpKind::Ret, &["seven"], None)],
    );
    source.return_abi = molt_ir::FunctionReturnAbi::Value;
    let non_inlinable = std::collections::HashSet::new();
    for mode in [
        TirOwnedModulePipelineMode::ModulePhase,
        TirOwnedModulePipelineMode::TerminalDropsOnly,
    ] {
        let mut reference = None;
        for warm in [false, true] {
            let mut functions = vec![source.clone()];
            let mut run = run_cached_tir_pipeline(
                TirPipelineInput::SimpleIr(&mut functions),
                TirPipelineRunOptions {
                    target_info: target.clone(),
                    cache_flavor: TirPipelineCacheFlavor::Native,
                    cache: crate::tir::pipeline_cache::TirCacheLocation::Directory(
                        cache_dir.join(format!("{mode:?}")),
                    ),
                    process_externs: false,
                    verify_lir: true,
                    tir_dump: false,
                    tir_stats: false,
                    progress_prefix: None,
                    resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
                },
                |_| {},
            );
            assert_eq!(run.uncached_count, usize::from(!warm));
            let typed = run_owned_module_pipeline_from_cached_tir(
                &functions,
                &mut run.cached_tir,
                TirOwnedModulePipelineOptions {
                    target_info: &target,
                    module_name: "cached_final",
                    non_inlinable: &non_inlinable,
                    missing_tir_context: "native final cache test",
                    mode,
                },
            );
            let finalized = NativeFinalFunctions::from_final_tir(
                SimpleIR {
                    functions,
                    profile: None,
                },
                typed,
            );
            assert_eq!(
                analyze_native_backend_ir(&finalized, Default::default()).leaf_functions,
                BTreeSet::from(["cached_final".into()])
            );
            let (body, plan) = finalized
                .into_local_functions()
                .next()
                .unwrap()
                .into_parts();
            let literal = body
                .ops
                .iter()
                .find(|operation| operation.kind == OpKind::Const && operation.value == Some(7))
                .expect("constant return value");
            assert!(plan.is_inline_safe_int_name(literal.out.as_deref().unwrap()));
            let snapshot = (serde_json::to_vec(&body).unwrap(), plan);
            if let Some(reference) = &reference {
                assert_eq!(
                    &snapshot, reference,
                    "{mode:?}: cache transport changed final body or facts"
                );
            } else {
                reference = Some(snapshot);
            }
        }
    }
    std::fs::remove_dir_all(cache_dir).unwrap();
}
