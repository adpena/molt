use super::*;
use molt_ir::OpKind;

#[test]
fn batch_declarations_preserve_provider_and_local_closure_signatures() {
    let closure = |name: &str| FunctionIR {
        name: name.into(),
        params: vec![molt_ir::MOLT_CLOSURE_PARAM_NAME.into()],
        return_abi: molt_ir::FunctionReturnAbi::Void,
        ops: vec![OpIR::new(OpKind::RetVoid)],
        ..Default::default()
    };
    let provider = closure("stdlib__inner");
    let context = SimpleBackend::prepare_module_context(&mut vec![provider.clone()]);
    let functions = NativeFinalFunctions::from_codegen_fixtures(SimpleIR {
        functions: vec![
            provider.extern_declaration().unwrap(),
            closure("app__inner"),
        ],
        profile: None,
    });
    let table = molt_ir::FunctionTable::of(&functions.ir().functions);
    for id in table.ids() {
        let abi = functions.linkage(id, Some(&context));
        assert!(abi.source_signature.has_closure, "{}", table.symbol(id));
        assert_eq!(abi.param_types.len(), 1);
    }
}

#[test]
fn native_backend_module_context_preserves_cross_batch_function_metadata() {
    let mut functions = vec![
        FunctionIR {
            module: None,
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
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
        FunctionIR {
            module: None,
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
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    let context = SimpleBackend::prepare_module_context(&mut functions);

    let abi = context.function_linkage_abi("helper").unwrap();
    assert_eq!(abi.source_signature.arity, 2);
    assert_eq!(abi.param_types.len(), 2);
    assert!(abi.return_type.is_some());
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
            module: None,
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
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
        FunctionIR {
            module: None,
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
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    let context = SimpleBackend::prepare_module_context(&mut functions);

    assert!(
        context
            .function_linkage_abi("value_helper")
            .unwrap()
            .return_type
            .is_some()
    );
    assert!(
        context
            .function_linkage_abi("void_helper")
            .unwrap()
            .return_type
            .is_none()
    );
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
        module: Some("app".into()),
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
        // Every partition keeps its source's owner; ownership never needs
        // the splitter's provenance.
        assert_eq!(function.module.as_deref(), Some("app"));
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
    let leaves = terminal_leaf_oracle(&final_functions_for_test(&ir.functions));
    assert!(!leaves.contains("released_local"));
    assert!(leaves.contains("pure_local"));
    assert!(!leaves.contains("unfinalized_local"));
    assert!(!leaves.contains("exception_only_local"));
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

/// The independent whole-module graph sees the terminal bodies, including
/// operations introduced after the last point with a SimpleIR encoding.
pub(super) fn terminal_leaf_oracle(finalized: &NativeFinalFunctions) -> BTreeSet<String> {
    let table = molt_ir::FunctionTable::of(&finalized.ir().functions);
    let ids: Vec<_> = table.ids().filter(|&id| table.defines(id)).collect();
    assert_eq!(ids.len(), finalized.bodies().count());
    let bodies = ids.into_iter().zip(finalized.bodies().cloned());
    graph_leaf_oracle(table, bodies)
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
    let after = terminal_leaf_oracle(&finalized);
    assert_eq!(
        before, after,
        "typed final rewrite changed callback eligibility"
    );
    // These source rewrite fixtures intentionally run before final-only expansion.
    // Re-emit just this fixture to check its source-level rewrites below.
    let mut source = finalized.ir().functions[0].clone();
    source.ops =
        crate::tir::lower_to_simple::lower_to_simple_ir(finalized.bodies().next().unwrap());
    source
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
fn native_preparation_only_resizes_its_own_cell_storage() {
    let mut backend = SimpleBackend::new();
    backend.module_registry = Some(Default::default());
    backend.emit_app_callable_resolver = true;
    assert!(backend.mutable_cell_layout(true).is_some());
    backend.external_program_storage = true;
    assert!(backend.mutable_cell_layout(true).is_none());
    backend.external_program_storage = false;
    assert!(backend.mutable_cell_layout(false).is_none());
    backend.emit_app_callable_resolver = false;
    assert!(backend.mutable_cell_layout(true).is_none());
}

#[test]
fn native_preparation_streams_final_callbacks_for_module_and_batch_paths() {
    let _guard = backend_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    use crate::native_backend::simple_backend::backend_selection::NativeCodegenBackend;
    for selection in [
        NativeCodegenBackend::Cranelift,
        #[cfg(feature = "llvm")]
        NativeCodegenBackend::Llvm,
    ] {
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
                    selection,
                    false,
                    false,
                    &std::time::Instant::now(),
                );
                let finalized = prepared.final_functions;
                assert!(
                    ir.functions.is_empty(),
                    "source custody moved into final owner"
                );
                let leaves = terminal_leaf_oracle(&finalized);
                assert_eq!(
                    leaves.contains("molt_main"),
                    !callback,
                    "backend={selection:?} skip={skip_ir_passes}"
                );
            }
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
    let leaves = terminal_leaf_oracle(&finalized);
    assert_eq!(leaves, BTreeSet::from(["local".into()]));

    let emitted: Vec<_> = finalized
        .into_local_functions()
        .map(|(_, function)| function.tir.name)
        .collect();
    assert_eq!(emitted, vec!["local"]);
}

#[test]
fn terminal_call_retains_and_remaps_its_provider_after_source_calls_retire() {
    let _guard = acquire_backend_env_lock();
    let _dfe = ScopedEnvVar::set("MOLT_DISABLE_DEAD_FUNC_ELIM", None);
    let mut call = callback_op(OpKind::CallInternal, &[], None);
    call.callee = Some(molt_ir::FunctionId(2));
    let ret = callback_op(OpKind::RetVoid, &[], None);
    let mut ir = SimpleIR {
        functions: vec![
            callback_function("entry", &[], vec![call, ret.clone()]),
            callback_function("dead", &[], vec![ret.clone()]),
            callback_function("provider", &[], vec![ret]),
        ],
        profile: None,
    };
    let typed = ir
        .functions
        .iter()
        .map(|source| (false, crate::tir::lower_from_simple::lower_to_tir(source)))
        .collect();
    // Optimizing away the source call to "dead" must allow that row to
    // disappear even though the terminal caller still needs "provider".
    ir.functions[0].ops[0].callee = Some(molt_ir::FunctionId(1));
    let mut finalized = NativeFinalFunctions::from_final_tir(ir, typed);
    finalized.retain_reachable(&BTreeSet::new());
    let table = molt_ir::FunctionTable::of(&finalized.ir().functions);
    let rows = finalized.into_local_functions().collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].1.tir.name, "provider");
    let callees = rows[0]
        .1
        .tir
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(|op| op.callee)
        .collect::<Vec<_>>();
    assert_eq!(callees, [rows[1].0]);
    assert_eq!(table.symbol(callees[0]), "provider");
}

#[test]
fn externalized_calls_validate_the_terminal_arguments_after_source_retirement() {
    use molt_ir::tir::ops::OpCode;
    let mut call = callback_op(OpKind::CallGuarded, &["callable", "value"], Some("result"));
    call.callee = Some(molt_ir::FunctionId(1));
    let entry = callback_function(
        "entry",
        &["callable", "value"],
        vec![call, callback_op(OpKind::RetVoid, &[], None)],
    );
    let provider = callback_function("provider", &["value"], vec![])
        .extern_declaration()
        .unwrap();
    let ir = SimpleIR {
        functions: vec![entry, provider],
        profile: None,
    };
    for (opcode, operands, admitted) in [
        (OpCode::Call, 1, true), // default binding still owns the missing argument
        (OpCode::DirectCall, 1, false),
        (OpCode::DirectCall, 2, true),
    ] {
        let mut typed = ir
            .functions
            .iter()
            .map(|source| {
                (
                    source.is_extern,
                    crate::tir::lower_from_simple::lower_to_tir(source),
                )
            })
            .collect::<Vec<_>>();
        let op = typed[0]
            .1
            .blocks
            .values_mut()
            .flat_map(|block| &mut block.ops)
            .find(|op| op.callee.is_some())
            .unwrap();
        op.opcode = opcode;
        if opcode == OpCode::DirectCall {
            op.origin = None;
        }
        op.operands.truncate(operands);
        let functions = NativeFinalFunctions::from_final_tir(ir.clone(), typed);
        let result = functions.validate_extern_calls();
        assert_eq!(result.is_ok(), admitted, "{opcode:?}: {result:?}");
        if let Err(error) = result {
            assert!(
                error.contains("caller `entry` supplies 0 parameter(s) to declaration `provider`"),
                "{error}"
            );
        }
    }
}

#[test]
fn final_partition_retires_body_facts_together_and_preserves_source_order() {
    let _guard = acquire_backend_env_lock();
    let executable = std::env::current_exe().unwrap();
    let _stdlib = ScopedEnvVar::set("MOLT_STDLIB_OBJ", executable.to_str());
    let _entry = ScopedEnvVar::set("MOLT_ENTRY_MODULE", Some("application"));
    let _symbols = ScopedEnvVar::set("MOLT_STDLIB_MODULES", Some("[\"stdlib\"]"));
    let _dfe = ScopedEnvVar::set("MOLT_DISABLE_DEAD_FUNC_ELIM", None);
    let mut entry = callback_function(
        "molt_main",
        &[],
        vec![
            callback_op(OpKind::DropInserted, &[], None),
            // Direct calls are the references that retain a body (HF-847).
            callback_op(OpKind::CallInternal, &[], None),
            callback_op(OpKind::CallInternal, &[], None),
            callback_op(OpKind::RetVoid, &[], None),
        ],
    );
    entry.ops[1].s_value = Some("stdlib__helper".into());
    entry.ops[2].s_value = Some("application__helper".into());
    // Ownership is each record's module; a compiler partition inherits its
    // source function's module from the splitter.
    let pure = |name, module: &str| FunctionIR {
        module: Some(module.into()),
        ..callback_function(
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
        pure("stdlib__helper", "stdlib"),
        pure("dead", "stdlib"),
        pure("application__helper", "application"),
        pure("compiler_partition", "stdlib"),
    ];
    let roots = BTreeSet::from(["compiler_partition".into()]);
    for needs_reachability in [false, true] {
        let mut finalized = final_functions_for_test(&sources);
        assert!(terminal_leaf_oracle(&finalized).contains("stdlib__helper"));
        if !needs_reachability {
            finalized.retain_reachable(&roots);
        }
        let mut oracle = SimpleIR {
            functions: sources.clone(),
            profile: None,
        };
        oracle.resolve_function_references().unwrap();
        crate::native_backend::simple_backend::externalize_shared_stdlib_partition(
            &mut oracle,
            &roots,
        );
        finalized.externalize_partition(needs_reachability, &roots);
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
        let leaves = terminal_leaf_oracle(&finalized);
        assert!(!leaves.contains("stdlib__helper"));
        assert!(!leaves.contains("compiler_partition"));
        assert!(!leaves.contains("dead"));

        let names = finalized
            .into_local_functions()
            .map(|(_, function)| function.tir.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["molt_main", "application__helper"]);
    }
}

#[test]
fn final_function_custody_is_identical_after_cold_and_warm_cache_transport() {
    use crate::tir::pipeline_cache::{
        TirOwnedModulePipelineMode, TirOwnedModulePipelineOptions, TirPipelineCacheFlavor,
        TirPipelineRunOptions, run_cached_tir_pipeline, run_owned_module_pipeline_from_cached_tir,
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
    let non_inlinable = molt_ir::deterministic_hash::HashSet::default();
    for mode in [
        TirOwnedModulePipelineMode::ModulePhase,
        TirOwnedModulePipelineMode::TerminalDropsOnly,
    ] {
        let mut reference = None;
        for warm in [false, true] {
            let functions = vec![source.clone()];
            let mut run = run_cached_tir_pipeline(
                &functions,
                TirPipelineRunOptions {
                    target_info: target.clone(),
                    cache_flavor: TirPipelineCacheFlavor::Native,
                    cache: crate::tir::pipeline_cache::TirCacheLocation::Directory(
                        cache_dir.join(format!("{mode:?}")),
                    ),
                    process_externs: false,
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
                    cell_registry: None,
                    target_info: &target,
                    module_name: "cached_final",
                    non_inlinable: &non_inlinable,
                    missing_tir_context: "native final cache test",
                    mode,
                    linkage: None,
                },
            )
            .functions;
            let finalized = NativeFinalFunctions::from_final_tir(
                SimpleIR {
                    functions,
                    profile: None,
                },
                typed,
            );
            assert_eq!(
                terminal_leaf_oracle(&finalized),
                BTreeSet::from(["cached_final".into()])
            );
            let table = molt_ir::FunctionTable::of(&finalized.ir().functions);
            let (_, body) = finalized.into_local_functions().next().unwrap();
            let literal = body
                .tir
                .blocks
                .values()
                .flat_map(|block| &block.ops)
                .find(|op| {
                    op.opcode == crate::tir::ops::OpCode::ConstInt
                        && op.attrs.get("value") == Some(&crate::tir::ops::AttrValue::Int(7))
                })
                .expect("constant return value");
            assert!(body.carriers.is_inline_safe_int(literal.results[0]));
            let snapshot =
                crate::tir::serialize::serialize_tir_function(&body.tir, &table).unwrap();
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

#[test]
fn rewritten_dump_can_show_a_final_only_operation() {
    use crate::tir::ops::{OpCode, TirOp};
    use crate::tir::types::TirType;
    let mut function = crate::tir::TirFunction::new(
        "poll".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Void,
    );
    function
        .blocks
        .get_mut(&function.entry_block)
        .unwrap()
        .ops
        .push(TirOp::new(OpCode::RecursionRelease, vec![], vec![]));
    let text = super::super::program_pipeline::rewritten_ir_text(&function);
    assert!(text.contains("recursion_release"), "{text}");
    assert!(text.contains("func @poll"), "{text}");
}
