use super::*;
use molt_ir::OpKind;

fn compile_exact_target_reference(kind: OpKind, target: &str, arity: usize, returns_value: bool) {
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
    // The exact target's own parameter declaration: every argument borrowed.
    backend.function_entry_custody.insert(
        target.to_string(),
        molt_codegen_abi::EntryCustodyDeclaration::declare(false, arity, &[]),
    );
    let args = match kind {
        OpKind::StatefulLocalsRegister => {
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
                out: Some("metadata".to_string()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                s_value: Some(target.to_string()),
                args: Some(args),
                out: matches!(kind, OpKind::FuncNew | OpKind::CallAsync)
                    .then(|| "result".to_string()),
                value: (kind == OpKind::FuncNew).then_some(arity as i64),
                ..OpIR::new(kind)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let defined = BTreeSet::from(["caller".to_string()]);
    backend.compile_func(
        final_function_for_test(caller),
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
    for kind in [OpKind::StatefulLocalsRegister] {
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
        compile_exact_target_reference(OpKind::FuncNew, "ordinary_poll", arity, true);
    }
}

#[test]
fn call_async_accepts_exact_opaque_task_target() {
    compile_exact_target_reference(OpKind::CallAsync, "opaque_body", 1, true);
}

#[test]
#[should_panic(expected = "must have ABI (i64) -> i64")]
fn call_async_rejects_poll_spelling_with_wrong_signature() {
    compile_exact_target_reference(OpKind::CallAsync, "ordinary_poll", 0, true);
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
        final_function_for_test(caller),
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
#[should_panic(expected = "builtin_func target `helper` has no runtime callable ABI")]
fn builtin_func_rejects_compiled_target_even_with_declared_machine_signature() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                value: Some(1),
                ..OpIR::new(OpKind::BuiltinFunc)
            },
            OpIR {
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                value: Some(1),
                ..OpIR::new(OpKind::FuncNew)
            },
            OpIR {
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    });
}

#[test]
#[should_panic(expected = "stateful_locals_register declaration mismatch for `helper`")]
fn stateful_locals_register_signature_mismatch_fails_closed_at_codegen() {
    compile_caller_with_incompatible_predeclared_helper(FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "caller".to_string(),
        params: Vec::new(),
        ops: vec![
            OpIR {
                out: Some("names".to_string()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                out: Some("layout".to_string()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                s_value: Some("helper".to_string()),
                args: Some(vec!["names".to_string(), "layout".to_string()]),
                value: Some(1),
                ..OpIR::new(OpKind::StatefulLocalsRegister)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
                out: Some("arg".to_string()),
                value: Some(1),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                s_value: Some("helper".to_string()),
                out: Some("result".to_string()),
                args: Some(vec!["arg".to_string()]),
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
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    });
}

fn compile_missing_static_target_symbol(kind: OpKind) {
    compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    args: Some(vec!["callee".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::new(kind)
                },
                OpIR {
                    ..OpIR::new(OpKind::RetVoid)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        "caller",
    );
}

#[test]
#[should_panic(expected = "call missing static target symbol")]
fn call_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol(OpKind::Call);
}

#[test]
#[should_panic(expected = "call_internal missing static target symbol")]
fn call_internal_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol(OpKind::CallInternal);
}

#[test]
#[should_panic(expected = "call_guarded missing static target symbol")]
fn call_guarded_missing_target_symbol_fails_closed_at_codegen() {
    compile_missing_static_target_symbol(OpKind::CallGuarded);
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
                    out: Some("missing".to_string()),
                    ..OpIR::new(OpKind::ConstStr)
                },
                OpIR {
                    ..OpIR::new(OpKind::RetVoid)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
                    out: Some("empty".to_string()),
                    s_value: Some(String::new()),
                    ..OpIR::new(OpKind::ConstStr)
                },
                OpIR {
                    args: Some(vec!["empty".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
                out: Some("callee".to_string()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                out: Some("arg".to_string()),
                value: Some(1),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                s_value: Some("helper".to_string()),
                out: Some("result".to_string()),
                args: Some(vec!["callee".to_string(), "arg".to_string()]),
                ..OpIR::new(OpKind::CallGuarded)
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
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    });
}

// A function object whose arity disagrees with its target's own declaration
// has no representable entry custody, so it fails at the `func_new`, before a
// later static call could compare the two declarations.
#[test]
#[should_panic(expected = "func_new target `helper` has no runtime entry custody")]
fn function_object_arity_mismatch_fails_closed_at_codegen() {
    compile_function_to_clif_text(
        vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "helper".to_string(),
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
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "caller".to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        out: Some("arg".to_string()),
                        value: Some(1),
                        ..OpIR::new(OpKind::Const)
                    },
                    OpIR {
                        s_value: Some("helper".to_string()),
                        out: Some("helper_obj".to_string()),
                        value: Some(0),
                        ..OpIR::new(OpKind::FuncNew)
                    },
                    OpIR {
                        s_value: Some("helper".to_string()),
                        out: Some("result".to_string()),
                        args: Some(vec!["arg".to_string()]),
                        ..OpIR::new(OpKind::CallInternal)
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
                parameter_custody: Vec::new(),
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
                out: Some("closure".to_string()),
                ..OpIR::new(OpKind::ConstNone)
            },
            OpIR {
                s_value: Some("helper".to_string()),
                out: Some("helper_obj".to_string()),
                args: Some(vec!["closure".to_string()]),
                value: Some(0),
                ..OpIR::new(OpKind::FuncNewClosure)
            },
            OpIR {
                args: Some(vec!["helper_obj".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    });
}

#[test]
fn no_codegen_markers_refuse_declared_values() {
    for kind in [
        OpKind::Nop,
        OpKind::DropInserted,
        OpKind::ExceptionRegionDropsInserted,
    ] {
        for output in [None, Some("none"), Some("fresh"), Some("keep")] {
            let name = format!("no_codegen_{kind}_{}", output.unwrap_or("absent"));
            let function = FunctionIR {
                name: name.clone(),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: vec!["keep".into()],
                param_types: Some(vec!["dyn".into()]),
                ops: vec![
                    OpIR {
                        out: output.map(str::to_owned),
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        args: Some(vec![
                            output
                                .filter(|value| *value != "none")
                                .unwrap_or("keep")
                                .into(),
                        ]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
                ..FunctionIR::default()
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compile_function_to_clif_text(vec![function], &name)
            }));
            if let Some(output) = output.filter(|value| *value != "none") {
                let refusal = result.expect_err("a marker cannot manufacture or overwrite a value");
                let message = refusal
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| refusal.downcast_ref::<&str>().copied())
                    .unwrap_or("");
                assert!(message.contains("no-codegen op kind"), "{message}");
                assert!(message.contains(kind.as_str()), "{message}");
                assert!(message.contains(output), "{message}");
            } else {
                assert!(
                    result.is_ok(),
                    "{kind}: a marker without a definition must remain valid"
                );
            }
        }
    }
}
