use super::*;

#[test]
fn compute_function_has_ret_uses_actual_ir_not_name_heuristics() {
    let result = compute_function_has_ret(&[
        FunctionIR {
            name: "demo__molt_module_chunk_1".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
        FunctionIR {
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
                    var: Some("ret".to_string()),
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
        },
    ]);

    assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
    assert_eq!(result.get("demo____molt_globals_builtin__"), Some(&true));
}

#[test]
fn compute_function_has_ret_treats_extern_declarations_as_value_returning() {
    let mut func = FunctionIR {
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
        name: "stdlib_void_helper".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
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
    };
    crate::externalize_function_with_signature(&mut extern_helper);
    let caller = FunctionIR {
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
                var: Some("result".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let functions = vec![caller.clone(), extern_helper.clone()];
    let module_context = SimpleBackend::build_module_context(&functions);
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
    let mut module_known_functions = BTreeSet::from(["molt_main".to_string()]);
    module_known_functions.insert("stdlib_value_helper".to_string());
    let mut backend = SimpleBackend::new();
    backend.compile_func(
        caller,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeSet::from(["molt_main".to_string()]),
        &module_known_functions,
        &BTreeSet::new(),
        &BTreeMap::new(),
        false,
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
            name: "user_func".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        },
        FunctionIR {
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
        },
    ]);

    assert_eq!(result.get("user_func"), Some(&false));
    assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
}

#[test]
fn compute_function_has_ret_treats_state_machines_as_value_returning() {
    let result = compute_function_has_ret(&[FunctionIR {
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
