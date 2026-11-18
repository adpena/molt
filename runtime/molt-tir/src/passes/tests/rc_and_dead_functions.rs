use super::*;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

#[test]
fn constant_integer_facts_require_one_canonical_producer() {
    for kind in [
        molt_ir::OpKind::Const,
        molt_ir::OpKind::ConstInt,
        molt_ir::OpKind::LoadConst,
    ] {
        let constant = |name: &str| OpIR {
            kind,
            out: Some(name.into()),
            value: Some(7),
            ..OpIR::new(kind)
        };
        let function = FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "constant_identity".into(),
            params: vec!["parameter".into()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
            ops: vec![
                constant("stable"),
                constant("parameter"),
                constant("repeated"),
                constant("repeated"),
                constant("repeated"),
                constant("stored"),
                make_store_var("stored", "stable"),
                constant("deleted"),
                OpIR {
                    kind: molt_ir::OpKind::DeleteVar,
                    var: Some("deleted".into()),
                    ..OpIR::new(molt_ir::OpKind::DeleteVar)
                },
                constant("checked"),
                OpIR {
                    kind: molt_ir::OpKind::CheckedAdd,
                    var: Some("checked".into()),
                    out: Some("overflow".into()),
                    args: Some(vec!["stable".into(), "stable".into()]),
                    ..OpIR::new(molt_ir::OpKind::CheckedAdd)
                },
                constant("none"),
                constant(""),
                OpIR {
                    kind: molt_ir::OpKind::StoreIndex,
                    out: Some("stable".into()), // Metadata, not a producer.
                    args: Some(vec!["items".into(), "index".into(), "stable".into()]),
                    ..OpIR::new(molt_ir::OpKind::StoreIndex)
                },
            ],
        };
        assert_eq!(
            build_const_int_map(&function),
            std::collections::BTreeMap::from([("stable".into(), 7)]),
            "{kind}"
        );
    }
}

#[test]
fn protected_runtime_entrypoint_detection_is_explicit() {
    assert!(is_protected_runtime_entrypoint("molt_main"));
    assert!(is_protected_runtime_entrypoint("molt_host_init"));
    assert!(is_protected_runtime_entrypoint("molt_isolate_import"));
    assert!(is_protected_runtime_entrypoint("molt_isolate_bootstrap"));
    // Only the runtime ABI's compiled entries; no prefix admits a family.
    assert!(!is_protected_runtime_entrypoint("molt_isolate_other"));
    assert!(!is_protected_runtime_entrypoint("pyinit.math"));
    assert!(!is_protected_runtime_entrypoint("user_entry"));
}

#[test]
fn eliminate_dead_functions_retains_runtime_dispatch_closure() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
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
                name: "molt_isolate_import".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![
                    OpIR {
                        kind: molt_ir::OpKind::Call,
                        s_value: Some("molt_init_math".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::new(molt_ir::OpKind::Call)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::Ret,
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::new(molt_ir::OpKind::Ret)
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
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_math".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    ir.resolve_function_references().unwrap();
    eliminate_dead_functions(&mut ir);

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(retained.contains("molt_isolate_import"));
    assert!(retained.contains("molt_init_math"));
    let dispatch = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_isolate_import")
        .expect("runtime dispatch entrypoint must remain");
    assert!(
        dispatch.ops.iter().any(
            |op| op.callee.map(|id| ir.functions[id.index()].name.as_str())
                == Some("molt_init_math")
        ),
        "runtime dispatch body must keep its transitive module-init references",
    );
}

#[test]
fn eliminate_dead_functions_retains_molt_host_init_and_transitive_refs() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
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
                name: "molt_host_init".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: molt_ir::OpKind::Call,
                        s_value: Some("host_init_helper".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::new(molt_ir::OpKind::Call)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::Ret,
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::new(molt_ir::OpKind::Ret)
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
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "host_init_helper".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    ir.resolve_function_references().unwrap();
    eliminate_dead_functions(&mut ir);

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(retained.contains("molt_host_init"));
    assert!(retained.contains("host_init_helper"));
    let host_init = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_host_init")
        .expect("molt_host_init must remain");
    assert!(
        host_init.ops.iter().any(
            |op| op.callee.map(|id| ir.functions[id.index()].name.as_str())
                == Some("host_init_helper")
        ),
        "molt_host_init must keep its transitive references",
    );
}

#[test]
fn eliminate_dead_functions_does_not_root_stdlib_from_partition_env() {
    let prior = backend_env::var(BackendEnv::MOLT_STDLIB_MODULES).ok();
    unsafe {
        std::env::set_var("MOLT_STDLIB_MODULES", "[\"sys\"]");
    }

    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
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
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: molt_ir::OpKind::Call,
                    s_value: Some("sys__helper".to_string()),
                    out: Some("v0".to_string()),
                    ..OpIR::new(molt_ir::OpKind::Call)
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
                name: "sys__helper".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
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
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: vec![make_op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    ir.resolve_function_references().unwrap();
    eliminate_dead_functions(&mut ir);

    match prior {
        Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULES", value) },
        None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULES") },
    }

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(!retained.contains("molt_init_sys"));
    assert!(!retained.contains("sys__helper"));
    assert!(!retained.contains("molt_init_json"));
}
#[test]
fn generated_function_references_retain_exact_symbols_without_guessed_companions() {
    let reference_kinds = molt_ir::OpKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.function_reference_role() != molt_ir::FunctionReferenceRole::None);
    for kind in reference_kinds {
        let function = |name: &str, ops: Vec<OpIR>| FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: name.to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        for target in ["opaque_body", "opaque_body_poll", "undefined"] {
            let mut ir = SimpleIR {
                functions: vec![
                    function(
                        "entry",
                        vec![OpIR {
                            kind,
                            s_value: Some(target.to_string()),
                            ..OpIR::new(kind)
                        }],
                    ),
                    function("opaque_body", vec![make_op(molt_ir::OpKind::RetVoid)]),
                    function("opaque_body_poll", vec![make_op(molt_ir::OpKind::RetVoid)]),
                    function(
                        "opaque_body_poll_poll",
                        vec![make_op(molt_ir::OpKind::RetVoid)],
                    ),
                ],
                profile: None,
            };
            let decoded = ir.resolve_function_references();
            if kind.function_reference_role() == molt_ir::FunctionReferenceRole::Required
                && target == "undefined"
            {
                // A required reference to no row is a decode error (D2).
                assert!(decoded.unwrap_err().contains("undefined"), "{kind}");
                continue;
            }
            decoded.unwrap();
            eliminate_dead_functions(&mut ir);
            let retained: Vec<&str> = ir
                .functions
                .iter()
                .map(|function| function.name.as_str())
                .collect();
            let expected = if target == "undefined" {
                vec!["entry"]
            } else {
                vec!["entry", target]
            };
            assert_eq!(retained, expected, "{kind} -> {target}");
        }
    }
}

#[test]
fn non_reference_operations_do_not_retain_defined_functions() {
    {
        let kind = molt_ir::OpKind::ConstStr;
        let function = |name: &str, ops: Vec<OpIR>| FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: name.to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let mut ir = SimpleIR {
            functions: vec![
                function(
                    "entry",
                    vec![OpIR {
                        kind,
                        s_value: Some("opaque_body".to_string()),
                        ..OpIR::new(kind)
                    }],
                ),
                function("opaque_body", vec![make_op(molt_ir::OpKind::RetVoid)]),
                function("opaque_body_poll", vec![make_op(molt_ir::OpKind::RetVoid)]),
            ],
            profile: None,
        };
        eliminate_dead_functions(&mut ir);
        assert_eq!(ir.functions.len(), 1, "{kind}");
        assert_eq!(ir.functions[0].name, "entry", "{kind}");
    }
}
