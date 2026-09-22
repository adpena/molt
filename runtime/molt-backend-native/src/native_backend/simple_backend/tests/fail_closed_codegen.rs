use super::*;

fn compile_exact_target_reference(kind: &str, target: &str, arity: usize, returns_value: bool) {
    let mut backend = SimpleBackend::new();
    let mut signature = backend.module.make_signature();
    for _ in 0..arity {
        signature
            .params
            .push(cranelift_codegen::ir::AbiParam::new(types::I64));
    }
    if returns_value {
        signature
            .returns
            .push(cranelift_codegen::ir::AbiParam::new(types::I64));
    }
    backend
        .module
        .declare_function(target, cranelift_module::Linkage::Import, &signature)
        .expect("predeclare exact target ABI");
    let args = match kind {
        "gen_locals_register" | "asyncgen_locals_register" => {
            vec!["metadata".to_string(), "metadata".to_string()]
        }
        _ => Vec::new(),
    };
    let caller = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("metadata".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: kind.to_string(),
                s_value: Some(target.to_string()),
                args: Some(args),
                out: matches!(kind, "func_new" | "call_async").then(|| "result".to_string()),
                value: (kind == "func_new").then_some(arity as i64),
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
        execution_context: Default::default(),
    };
    let defined = BTreeSet::from(["caller".to_string()]);
    backend.compile_func(
        caller,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &defined,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::from([("caller".to_string(), 0), (target.to_string(), arity)]),
        &BTreeMap::from([
            ("caller".to_string(), false),
            (target.to_string(), returns_value),
        ]),
    );
}

#[test]
fn metadata_pointer_declarations_use_exact_target_abi() {
    for kind in ["gen_locals_register", "asyncgen_locals_register"] {
        for (target, arity, returns_value) in [
            ("opaque_body", 1, true),
            ("ordinary_poll", 0, true),
            ("ordinary_poll", 2, true),
            ("void_poll", 0, false),
        ] {
            compile_exact_target_reference(kind, target, arity, returns_value);
        }
    }
}

#[test]
fn ordinary_poll_named_callable_keeps_its_declared_arity() {
    for arity in [0, 2] {
        compile_exact_target_reference("func_new", "ordinary_poll", arity, true);
    }
}

#[test]
fn call_async_accepts_exact_opaque_task_target() {
    compile_exact_target_reference("call_async", "opaque_body", 1, true);
}

#[test]
#[should_panic(expected = "must have ABI (i64) -> i64")]
fn call_async_rejects_poll_spelling_with_wrong_signature() {
    compile_exact_target_reference("call_async", "ordinary_poll", 0, true);
}

fn compile_caller_with_incompatible_predeclared_helper(caller: FunctionIR) {
    let mut backend = SimpleBackend::new();
    let mut predeclared_sig = backend.module.make_signature();
    predeclared_sig
        .returns
        .push(cranelift_codegen::ir::AbiParam::new(types::I64));
    backend
        .module
        .declare_function(
            "helper",
            cranelift_module::Linkage::Import,
            &predeclared_sig,
        )
        .expect("predeclare helper");

    let defined_functions = BTreeSet::from(["caller".to_string(), "helper".to_string()]);
    let function_arities = BTreeMap::from([
        ("caller".to_string(), caller.params.len()),
        ("helper".to_string(), 1usize),
    ]);
    let function_has_ret =
        BTreeMap::from([("caller".to_string(), true), ("helper".to_string(), true)]);
    backend.compile_func(
        caller,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &defined_functions,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &function_arities,
        &function_has_ret,
    );
}

#[test]
#[should_panic(expected = "builtin_func declaration mismatch for `helper`")]
fn builtin_func_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "builtin_func".to_string(),
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "func_new declaration mismatch for `helper`")]
fn func_new_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "func_new".to_string(),
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "asyncgen_locals_register declaration mismatch for `helper`")]
fn asyncgen_locals_register_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("names".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("offsets".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "asyncgen_locals_register".to_string(),
                s_value: Some("helper".to_string()),
                args: Some(vec!["names".to_string(), "offsets".to_string()]),
                value: Some(1),
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
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "gen_locals_register declaration mismatch for `helper`")]
fn gen_locals_register_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("names".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("offsets".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "gen_locals_register".to_string(),
                s_value: Some("helper".to_string()),
                args: Some(vec!["names".to_string(), "offsets".to_string()]),
                value: Some(1),
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
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "call declaration mismatch for `helper`")]
fn call_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("arg".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                s_value: Some("helper".to_string()),
                out: Some("result".to_string()),
                args: Some(vec!["arg".to_string()]),
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
        execution_context: Default::default(),
    });
}

fn compile_missing_static_target_symbol(kind: &str) {
    compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: kind.to_string(),
                    args: Some(vec!["callee".to_string()]),
                    out: Some("result".to_string()),
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
            execution_context: Default::default(),
        }],
        "caller",
    );
}

#[test]
#[should_panic(expected = "call missing static target symbol")]
fn call_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol("call");
}

#[test]
#[should_panic(expected = "call_internal missing static target symbol")]
fn call_internal_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol("call_internal");
}

#[test]
#[should_panic(expected = "call_guarded missing static target symbol")]
fn call_guarded_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol("call_guarded");
}

#[test]
#[should_panic(expected = "const_str missing bytes or string payload for output `missing`")]
fn const_str_missing_payload_fails_closed_at_codegen() {
    compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "const_str_missing_payload".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("missing".to_string()),
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
            execution_context: Default::default(),
        }],
        "const_str_missing_payload",
    );
}

#[test]
fn const_str_empty_string_payload_still_compiles() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "const_str_empty_payload".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("empty".to_string()),
                    s_value: Some(String::new()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["empty".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        "const_str_empty_payload",
    );

    assert!(clif.contains("return"));
}

#[test]
#[should_panic(expected = "call_guarded declaration mismatch for `helper`")]
fn call_guarded_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("callee".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("arg".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_guarded".to_string(),
                s_value: Some("helper".to_string()),
                out: Some("result".to_string()),
                args: Some(vec!["callee".to_string(), "arg".to_string()]),
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
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "conflicting static call ABI for helper")]
fn call_internal_signature_mismatch_fails_closed_at_codegen() {
    compile_function_to_clif_text(
        vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "helper".to_string(),
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
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "caller".to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("arg".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new".to_string(),
                        s_value: Some("helper".to_string()),
                        out: Some("helper_obj".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("helper".to_string()),
                        out: Some("result".to_string()),
                        args: Some(vec!["arg".to_string()]),
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
                execution_context: Default::default(),
            },
        ],
        "caller",
    );
}

#[test]
#[should_panic(expected = "func_new_closure declaration mismatch for `helper`")]
fn func_new_closure_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("closure".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "func_new_closure".to_string(),
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                args: Some(vec!["closure".to_string()]),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    });
}
