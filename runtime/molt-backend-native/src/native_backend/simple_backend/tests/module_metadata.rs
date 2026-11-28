use super::*;
use molt_ir::OpKind;
use molt_ir::runtime_callable_abi_generated::compiled_execution_symbol;

#[test]
fn checked_private_entry_requires_every_incoming_reference_to_check_its_result() {
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{AttrValue, OpCode, TirOp};
    use crate::tir::types::TirType;
    use molt_ir::{FunctionId, FunctionReturnAbi};

    let make_body = |name: &str| {
        let mut body = crate::tir::TirFunction::new(
            name.into(),
            Vec::new(),
            TirType::Bool,
            FunctionReturnAbi::Value,
        );
        let handler = body.fresh_block();
        body.label_id_map.insert(handler.0, 42);
        body.has_exception_handling = true;
        body.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: Vec::new(),
                ops: Vec::new(),
                terminator: Terminator::Unwind,
            },
        );
        let result = body.fresh_value();
        let entry = body.blocks.get_mut(&body.entry_block).unwrap();
        let mut check = TirOp::new(OpCode::CheckException, vec![], vec![]);
        check.attrs.insert("value".into(), AttrValue::Int(42));
        entry.ops.push(check);
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        (body, handler, result)
    };
    let (mut callee, _, result) = make_body("private");
    let mut constant = TirOp::new(OpCode::ConstBool, vec![], vec![result]);
    constant.attrs.insert("value".into(), AttrValue::Bool(true));
    callee
        .blocks
        .get_mut(&callee.entry_block)
        .unwrap()
        .ops
        .push(constant);
    let provider = ProducerCarrierPlan::build(&callee);
    assert_eq!(provider.checked_return_type(&callee), Some(TirType::Bool));
    assert_ne!(provider.effective_return_type(&callee), TirType::Bool);

    let (mut caller, handler, result) = make_body("caller");
    let id = FunctionId(1);
    let mut call = TirOp::new(OpCode::Call, vec![], vec![result]);
    call.callee = Some(id);
    let entry = caller.entry_block;
    caller.blocks.get_mut(&entry).unwrap().ops.insert(0, call);
    let plan = |body: &crate::tir::TirFunction| {
        let mut consumer = ProducerCarrierPlan::build(body);
        // Force the same carrier in the positive and exceptional-use cases,
        // so only the checked-call fact distinguishes them here.
        consumer
            .repr_by_value
            .insert(result, molt_ir::repr::Repr::Bool);
        super::super::module_metadata::checked_private_returns(
            [Some(body), Some(&callee), Some(&callee)],
            |row| [&consumer, &provider][row],
            [id],
        )
    };
    assert_eq!(plan(&caller), BTreeMap::from([(id, TirType::Bool)]));
    let mixed = caller.blocks.get_mut(&handler).unwrap();
    mixed.terminator = Terminator::Return {
        values: vec![result],
    };
    assert!(
        plan(&caller).is_empty(),
        "handler reads the physical result"
    );
    caller.blocks.get_mut(&handler).unwrap().terminator = Terminator::Unwind;
    let mut address = TirOp::new(OpCode::CodeIs, vec![], vec![]);
    address.callee = Some(id);
    caller.blocks.get_mut(&entry).unwrap().ops.push(address);
    assert!(
        plan(&caller).is_empty(),
        "the entry's address escapes the call contract"
    );
}

#[test]
fn frozen_provider_linkage_leaves_local_carrier_analysis_unforced() {
    let id = molt_ir::FunctionId::from_index(0);
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "local".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![OpIR::new(OpKind::RetVoid)],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let provider = SimpleBackend::prepare_module_context(&mut ir.functions);
    let eager = NativeFinalFunctions::from_codegen_fixtures(ir.clone());
    let abi = eager.linkage(id, None).clone();
    assert_eq!(eager.analyzed_carrier_count(), 1);

    let mut functions = NativeFinalFunctions::from_codegen_fixtures(ir);
    assert_eq!(
        functions.linkage(id, Some(&provider)),
        provider.function_linkage_abi("local").unwrap()
    );
    assert_eq!(functions.analyzed_carrier_count(), 0);

    functions.bind_shared_bodies(&BTreeSet::from(["local".to_string()]));
    assert_eq!(functions.linkage(id, None), &abi);
    assert_eq!(functions.analyzed_carrier_count(), 0);
}

#[test]
fn object_context_closes_indirect_callable_task_and_partition_dependencies() {
    let function = FunctionIR {
        name: "caller".into(),
        ops: vec![
            OpIR {
                s_value: Some("indirect_target".into()),
                ..OpIR::new(OpKind::CallIndirect)
            },
            OpIR {
                s_value: Some("taken_target".into()),
                value: Some(0),
                task_kind: Some("generator".into()),
                task_closure_size: Some(48),
                ..OpIR::new(OpKind::FuncNewClosure)
            },
            OpIR {
                s_value: Some("task_target".into()),
                ..OpIR::new(OpKind::AllocTask)
            },
            OpIR {
                s_value: Some("unrelated".into()),
                ..OpIR::new(OpKind::ConstStr)
            },
        ],
        ..FunctionIR::default()
    };
    // The object's document declares what it references (HF-847 slice 1);
    // `call_indirect` carries no callee symbol, so it names no dependency.
    let declaration = |name: &str| FunctionIR {
        name: name.into(),
        is_extern: true,
        ..FunctionIR::default()
    };
    let mut document = vec![
        function,
        declaration("taken_target"),
        declaration("task_target"),
    ];
    molt_ir::function_table::resolve_function_references(&mut document).unwrap();
    assert_eq!(
        document[0].ops[0].callee, None,
        "call_indirect is no reference"
    );
    let names = NativeBackendModuleContext::object_dependencies(&document);
    assert_eq!(
        names,
        BTreeSet::from(["caller", "taken_target", "task_target"].map(str::to_string))
    );
    let signature = FunctionIR {
        name: "taken_target".into(),
        ..FunctionIR::default()
    }
    .function_signature()
    .unwrap();
    let linkage = molt_ir::linkage_abi::NativeFunctionLinkageAbi {
        source_signature: signature,
        parameter_custody: Vec::new(),
        param_types: Vec::new(),
        return_type: Some(crate::tir::types::TirType::DynBox),
        return_availability: Default::default(),
    };
    let mut context = NativeBackendModuleContext {
        function_linkage_abis: BTreeMap::from([
            ("taken_target".into(), linkage.clone()),
            ("unrelated".into(), linkage),
        ]),
    };
    let closed = context.project_object_dependencies(&names);
    assert_eq!(
        closed.function_linkage_abi("taken_target"),
        context.function_linkage_abi("taken_target")
    );
    let encoded = serde_json::to_value(&closed).unwrap();
    assert!(!encoded.to_string().contains("unrelated"));
    context
        .function_linkage_abis
        .get_mut("unrelated")
        .unwrap()
        .return_type = None;
    assert_eq!(
        encoded,
        serde_json::to_value(context.project_object_dependencies(&names)).unwrap()
    );
    context
        .function_linkage_abis
        .get_mut("taken_target")
        .unwrap()
        .return_type = None;
    assert_ne!(
        encoded,
        serde_json::to_value(context.project_object_dependencies(&names)).unwrap()
    );
}

#[test]
fn authored_return_abi_uses_actual_ir_not_name_heuristics() {
    let functions = [
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "demo__molt_module_chunk_1".to_string(),
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
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "demo____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("ret".to_string()),
                    ..OpIR::new(OpKind::ConstNone)
                },
                OpIR {
                    args: Some(vec!["ret".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    ..OpIR::new(OpKind::RetVoid)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    assert!(!functions[0].function_signature().unwrap().returns_value);
    assert!(functions[1].function_signature().unwrap().returns_value);
}

#[test]
fn authored_return_abi_treats_extern_declarations_as_value_returning() {
    let mut func = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "importlib__import_module".to_string(),
        params: vec!["name".to_string(), "package".to_string()],
        ops: vec![
            OpIR {
                out: Some("result".to_string()),
                ..OpIR::new(OpKind::Missing)
            },
            OpIR {
                args: Some(vec!["result".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    crate::externalize_function_with_signature(&mut func);
    let tir = crate::tir::lower_from_simple::lower_to_tir(&func);
    assert_eq!(
        tir.return_type,
        crate::tir::types::TirType::DynBox,
        "TIR declaration metadata preserves extern value signatures from the signature stub",
    );
    let signature = func.function_signature().unwrap();

    assert!(
        signature.returns_value,
        "extern declarations must preserve the source body's value-returning ABI fact",
    );
}

#[test]
fn authored_return_abi_preserves_void_extern_declaration_signature() {
    let mut func = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "stdlib_void_helper".to_string(),
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
    };
    crate::externalize_function_with_signature(&mut func);
    let tir = crate::tir::lower_from_simple::lower_to_tir(&func);
    assert_eq!(
        tir.return_type,
        crate::tir::types::TirType::None,
        "TIR declaration metadata preserves extern void signatures from the signature stub",
    );
    let signature = func.function_signature().unwrap();

    assert!(
        !signature.returns_value,
        "extern declarations must preserve the source body's void ABI fact",
    );
}

#[test]
fn cranelift_import_declaration_uses_externalized_value_return_signature() {
    let provider = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "stdlib_value_helper".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                out: Some("value".to_string()),
                ..OpIR::new(OpKind::Missing)
            },
            OpIR {
                args: Some(vec!["value".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let caller = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "molt_main".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                s_value: Some("stdlib_value_helper".to_string()),
                out: Some("result".to_string()),
                args: Some(Vec::new()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["result".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let mut backend = SimpleBackend::new();
    compile_with_provider(&mut backend, caller, provider);
    let declaration = backend
        .module
        .declarations()
        .get_functions()
        .find_map(|(_, decl)| {
            (decl.name.as_deref()
                == Some(compiled_execution_symbol("stdlib_value_helper").as_str()))
            .then_some(decl)
        })
        .expect("stdlib_value_helper import declaration");

    assert_eq!(declaration.linkage, cranelift_module::Linkage::Import);
    assert_eq!(declaration.signature.params.len(), 1);
    assert_eq!(declaration.signature.returns.len(), 1);
    assert_eq!(declaration.signature.returns[0].value_type, types::I64);
}

#[test]
fn authored_return_abi_keeps_actual_signature_for_python_callable_targets() {
    let functions = [
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "user_func".to_string(),
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
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "demo__molt_module_chunk_1".to_string(),
            params: vec![],
            ops: vec![OpIR {
                s_value: Some("user_func".to_string()),
                value: Some(0),
                ..OpIR::new(OpKind::FuncNew)
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

    assert!(
        functions
            .iter()
            .all(|f| !f.function_signature().unwrap().returns_value)
    );
}

#[test]
fn authored_return_abi_treats_state_machines_as_value_returning() {
    let functions = [FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "raises_only_coroutine_poll".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                ..OpIR::new(OpKind::StateSwitch)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
            },
        ],
        param_types: Some(vec!["i64".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }];

    assert!(
        functions[0].function_signature().unwrap().returns_value,
        "poll functions are always invoked through the i64 poll ABI even when every user path raises",
    );
}

// Entry custody is part of the frozen linkage row: a consumer declaration in
// another object must adopt exactly what the provider's entry takes over.
#[test]
fn native_linkage_abi_freezes_entry_custody_for_consumer_declarations() {
    let provider = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "owns_argument".to_string(),
        params: vec!["argument".to_string()],
        parameter_custody: vec![molt_ir::ParameterCustody::Transferred],
        ops: vec![OpIR {
            ..OpIR::new(OpKind::RetVoid)
        }],
        ..FunctionIR::default()
    };
    let mut functions = vec![provider.clone()];
    let context = SimpleBackend::prepare_module_context(&mut functions);
    let declaration = provider.extern_declaration().unwrap();
    context
        .validate_function_linkage_abis(std::slice::from_ref(&declaration))
        .expect("a declaration projected from its provider keeps the provider's custody");
    let mut drifted = declaration;
    drifted.parameter_custody.clear();
    let error = context
        .validate_function_linkage_abis(&[drifted])
        .unwrap_err();
    assert!(error.contains("parameter custody"), "{error}");
}

fn linkage_carrier_store(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "molt-linkage-carriers-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

/// One name, two bodies: a constant return freezes an exact scalar return
/// carrier, a parameter passthrough a boxed one.
fn value_function(constant: bool) -> FunctionIR {
    let ops = if constant {
        vec![
            OpIR {
                value: Some(7),
                out: Some("seven".into()),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                args: Some(vec!["seven".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ]
    } else {
        vec![OpIR {
            args: Some(vec!["value".into()]),
            ..OpIR::new(OpKind::Ret)
        }]
    };
    FunctionIR {
        name: "app__value".into(),
        params: vec!["value".into()],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops,
        ..FunctionIR::default()
    }
}

fn computed_context(functions: &[FunctionIR]) -> NativeBackendModuleContext {
    NativeBackendModuleContext::from_functions_with_carrier_cache(&mut functions.to_vec(), None)
}

#[test]
fn linkage_carriers_reuse_unchanged_bodies_and_rekey_changed_bodies() {
    let _guard = acquire_backend_env_lock();
    let constant = value_function(true);
    let passthrough = value_function(false);
    let expected = |function: &FunctionIR| {
        computed_context(std::slice::from_ref(function))
            .function_linkage_abi("app__value")
            .cloned()
            .expect("defined function has a linkage row")
    };
    let (constant_abi, passthrough_abi) = (expected(&constant), expected(&passthrough));
    // The oracle distinguishes the bodies, so a stale row would be visible.
    assert_ne!(constant_abi.return_type, passthrough_abi.return_type);

    let store = linkage_carrier_store("rekey");
    for (function, expected, disk_hits, writes) in [
        // Cold: computed and published.
        (&constant, &constant_abi, 0, 1),
        // Warm, in a new process-equivalent cache handle: served from disk.
        (&constant, &constant_abi, 1, 0),
        // Same name, changed body: a new key, never the published row.
        (&passthrough, &passthrough_abi, 0, 1),
    ] {
        let mut cache = crate::tir::cache::CompilationCache::open(store.clone());
        let context = NativeBackendModuleContext::from_functions_with_carrier_cache(
            &mut vec![function.clone()],
            Some(&mut cache),
        );
        assert_eq!(context.function_linkage_abi("app__value"), Some(expected));
        let telemetry = cache.stats().telemetry;
        assert_eq!((telemetry.disk_hits, telemetry.writes), (disk_hits, writes));
    }
    std::fs::remove_dir_all(store).expect("remove carrier store");
}

#[test]
fn reused_linkage_carriers_freeze_the_computed_module_context() {
    let _guard = acquire_backend_env_lock();
    let mut functions = vec![value_function(true), {
        let mut other = value_function(false);
        other.name = "app__other".into();
        other
    }];
    functions.push(FunctionIR {
        name: "runtime_extern".into(),
        params: vec!["value".into()],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        is_extern: true,
        ..FunctionIR::default()
    });
    let reference = serde_json::to_value(computed_context(&functions)).unwrap();
    let store = linkage_carrier_store("context");
    for warm in [false, true] {
        let mut cache = crate::tir::cache::CompilationCache::open(store.clone());
        let context = NativeBackendModuleContext::from_functions_with_carrier_cache(
            &mut functions.clone(),
            Some(&mut cache),
        );
        assert_eq!(serde_json::to_value(context).unwrap(), reference);
        // Externs freeze no carrier row; both defined bodies do.
        assert_eq!(cache.stats().telemetry.disk_hits, if warm { 2 } else { 0 });
    }
    std::fs::remove_dir_all(store).expect("remove carrier store");
}

#[test]
fn compilation_diagnostics_compute_linkage_carriers_without_reuse() {
    use molt_ir::backend_environment::{self, BackendEnv, BackendEnvironment};
    use std::sync::Arc;

    let _request = backend_environment::enter(Arc::new(BackendEnvironment::default()));
    let store = linkage_carrier_store("diagnostics");
    let function = value_function(true);
    let mut cache = crate::tir::cache::CompilationCache::open(store.clone());
    NativeBackendModuleContext::from_functions_with_carrier_cache(
        &mut vec![function.clone()],
        Some(&mut cache),
    );
    let diagnostic_settings =
        serde_json::Map::from_iter([(BackendEnv::MOLT_TIR_DUMP.name().into(), "1".into())]);
    let _dump = backend_environment::enter(Arc::new(
        BackendEnvironment::from_request(&diagnostic_settings).expect("admit TIR dump option"),
    ));
    let mut cache = crate::tir::cache::CompilationCache::open(store.clone());
    NativeBackendModuleContext::from_functions_with_carrier_cache(
        &mut vec![function],
        Some(&mut cache),
    );
    let telemetry = cache.stats().telemetry;
    assert_eq!((telemetry.disk_hits, telemetry.misses), (0, 0));
    std::fs::remove_dir_all(store).expect("remove carrier store");
}
