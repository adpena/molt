use super::*;

#[test]
fn object_context_closes_indirect_callable_task_and_partition_dependencies() {
    let function = FunctionIR {
        name: "caller".into(),
        ops: vec![
            OpIR {
                kind: "call_indirect".into(),
                s_value: Some("indirect_target".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "func_new_closure".into(),
                s_value: Some("taken_target".into()),
                value: Some(0),
                task_kind: Some("generator".into()),
                task_closure_size: Some(48),
                ..OpIR::default()
            },
            OpIR {
                kind: "alloc_task".into(),
                s_value: Some("task_target".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("unrelated".into()),
                ..OpIR::default()
            },
        ],
        ..FunctionIR::default()
    };
    let names = NativeBackendModuleContext::object_dependencies(&[function]);
    assert_eq!(
        names,
        BTreeSet::from(
            ["caller", "indirect_target", "taken_target", "task_target"].map(str::to_string)
        )
    );
    let signature = FunctionIR {
        name: "taken_target".into(),
        ..FunctionIR::default()
    }
    .function_signature()
    .unwrap();
    let linkage = crate::NativeFunctionLinkageAbi {
        source_signature: signature,
        parameter_custody: Vec::new(),
        param_types: Vec::new(),
        return_type: Some(crate::tir::types::TirType::DynBox),
    };
    let mut context = NativeBackendModuleContext {
        partition_sources: BTreeMap::from([
            ("caller".into(), "older_part".into()),
            ("older_part".into(), "source_owner".into()),
            ("unrelated".into(), "other_owner".into()),
        ]),
        function_arities: BTreeMap::from([("taken_target".into(), 1), ("unrelated".into(), 2)]),
        function_has_ret: BTreeMap::from([
            ("taken_target".into(), true),
            ("unrelated".into(), false),
        ]),
        closure_functions: BTreeSet::from(["taken_target".into(), "unrelated".into()]),
        task_kinds: BTreeMap::from([
            ("taken_target".into(), TrampolineKind::Generator),
            ("unrelated".into(), TrampolineKind::Plain),
        ]),
        task_closure_sizes: BTreeMap::from([("taken_target".into(), 48), ("unrelated".into(), 0)]),
        function_linkage_abis: BTreeMap::from([
            ("taken_target".into(), linkage.clone()),
            ("unrelated".into(), linkage),
        ]),
    };
    let closed = context.project_object_dependencies(&names);
    assert_eq!(closed.original_function_name("caller"), "source_owner");
    assert_eq!(closed.function_arities.get("taken_target"), Some(&1));
    assert_eq!(closed.function_has_ret.get("taken_target"), Some(&true));
    assert!(closed.closure_functions.contains("taken_target"));
    assert_eq!(
        closed.task_kinds.get("taken_target"),
        Some(&TrampolineKind::Generator)
    );
    assert_eq!(closed.task_closure_sizes.get("taken_target"), Some(&48));
    assert!(closed.function_linkage_abis.contains_key("taken_target"));
    let encoded = serde_json::to_value(&closed).unwrap();
    assert!(!encoded.to_string().contains("unrelated"));
    context.function_arities.insert("unrelated".into(), 99);
    context.task_closure_sizes.insert("unrelated".into(), 999);
    assert_eq!(
        encoded,
        serde_json::to_value(context.project_object_dependencies(&names)).unwrap()
    );
    context.task_closure_sizes.insert("taken_target".into(), 56);
    assert_ne!(
        encoded,
        serde_json::to_value(context.project_object_dependencies(&names)).unwrap()
    );
}

#[test]
fn compute_function_has_ret_uses_actual_ir_not_name_heuristics() {
    let result = compute_function_has_ret(&[
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "demo__molt_module_chunk_1".to_string(),
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
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "demo____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("ret".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["ret".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ]);

    assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
    assert_eq!(result.get("demo____molt_globals_builtin__"), Some(&true));
}

#[test]
fn compute_function_has_ret_treats_extern_declarations_as_value_returning() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "importlib__import_module".to_string(),
        params: vec!["name".to_string(), "package".to_string()],
        ops: vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some("result".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["result".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
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
    let result = compute_function_has_ret(&[func]);

    assert_eq!(
        result.get("importlib__import_module"),
        Some(&true),
        "extern declarations must preserve the source body's value-returning ABI fact",
    );
}

#[test]
fn compute_function_has_ret_preserves_void_extern_declaration_signature() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "stdlib_void_helper".to_string(),
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
    };
    crate::externalize_function_with_signature(&mut func);
    let tir = crate::tir::lower_from_simple::lower_to_tir(&func);
    assert_eq!(
        tir.return_type,
        crate::tir::types::TirType::None,
        "TIR declaration metadata preserves extern void signatures from the signature stub",
    );
    let result = compute_function_has_ret(&[func]);

    assert_eq!(
        result.get("stdlib_void_helper"),
        Some(&false),
        "extern declarations must preserve the source body's void ABI fact",
    );
}

#[test]
fn cranelift_import_declaration_uses_externalized_value_return_signature() {
    let mut extern_helper = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "stdlib_value_helper".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some("value".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["value".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    crate::externalize_function_with_signature(&mut extern_helper);
    let caller = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "molt_main".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "call".to_string(),
                s_value: Some("stdlib_value_helper".to_string()),
                out: Some("result".to_string()),
                args: Some(Vec::new()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["result".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let mut functions = vec![caller.clone(), extern_helper.clone()];
    let module_context = SimpleBackend::prepare_module_context(&mut functions);
    assert_eq!(
        module_context.function_has_ret.get("stdlib_value_helper"),
        Some(&true),
        "externalized stdlib helper must keep the value-returning ABI fact in shared module metadata",
    );
    let local_function_arities = BTreeMap::from([("molt_main".to_string(), 0usize)]);
    let effective_function_arities =
        merge_function_arities(Some(&module_context), local_function_arities);
    let local_function_has_ret = compute_function_has_ret(std::slice::from_ref(&caller));
    let effective_function_has_ret =
        merge_function_has_ret(Some(&module_context), local_function_has_ret);
    let mut backend = SimpleBackend::new();
    backend.compile_func(
        caller,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeSet::from(["molt_main".to_string()]),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &effective_function_arities,
        &effective_function_has_ret,
    );
    let declaration = backend
        .module
        .declarations()
        .get_functions()
        .find_map(|(_, decl)| (decl.name.as_deref() == Some("stdlib_value_helper")).then_some(decl))
        .expect("stdlib_value_helper import declaration");

    assert_eq!(declaration.linkage, cranelift_module::Linkage::Import);
    assert_eq!(declaration.signature.params.len(), 0);
    assert_eq!(declaration.signature.returns.len(), 1);
    assert_eq!(declaration.signature.returns[0].value_type, types::I64);
}

#[test]
fn compute_function_has_ret_keeps_actual_signature_for_python_callable_targets() {
    let result = compute_function_has_ret(&[
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "user_func".to_string(),
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
        FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "demo__molt_module_chunk_1".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "func_new".to_string(),
                s_value: Some("user_func".to_string()),
                value: Some(0),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ]);

    assert_eq!(result.get("user_func"), Some(&false));
    assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
}

#[test]
fn compute_function_has_ret_treats_state_machines_as_value_returning() {
    let result = compute_function_has_ret(&[FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "raises_only_coroutine_poll".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "state_switch".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["i64".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }]);

    assert_eq!(
        result.get("raises_only_coroutine_poll"),
        Some(&true),
        "poll functions are always invoked through the i64 poll ABI even when every user path raises",
    );
}

#[test]
fn local_function_metadata_overrides_stale_module_context_after_split() {
    let context = NativeBackendModuleContext {
        function_arities: BTreeMap::from([(
            "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
            1usize,
        )]),
        function_has_ret: BTreeMap::from([(
            "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
            false,
        )]),
        ..NativeBackendModuleContext::default()
    };

    let merged_arities = merge_function_arities(
        Some(&context),
        BTreeMap::from([(
            "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
            1usize,
        )]),
    );
    let merged_has_ret = merge_function_has_ret(
        Some(&context),
        BTreeMap::from([(
            "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
            true,
        )]),
    );

    assert_eq!(
        merged_arities.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
        Some(&1usize)
    );
    assert_eq!(
        merged_has_ret.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
        Some(&true)
    );
}

// The context's serialized origin rows are compiler-produced identity, not a
// naming convention. Batch readers must retain chains for late re-partitioning.
#[test]
fn native_module_context_roundtrip_preserves_partition_sources() {
    let context = NativeBackendModuleContext {
        partition_sources: BTreeMap::from([
            ("opaque_first".into(), "app__owner".into()),
            ("opaque_second".into(), "opaque_first".into()),
        ]),
        ..NativeBackendModuleContext::default()
    };
    let bytes = serde_json::to_vec(&context).unwrap();
    let restored: NativeBackendModuleContext = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        restored.original_function_name("opaque_second"),
        "app__owner"
    );
    assert_eq!(
        restored.original_function_name("__molt_chunk_v1_user"),
        "__molt_chunk_v1_user"
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
            kind: "ret_void".to_string(),
            ..OpIR::default()
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
