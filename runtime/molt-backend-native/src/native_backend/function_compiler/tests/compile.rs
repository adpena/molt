use super::*;

#[test]
fn native_backend_compiles_float_primary_tuple_escape_before_exception_cleanup() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "float_primary_tuple_cleanup".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_a".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_a".to_string()),
                    args: Some(vec!["src_a".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_b".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_b".to_string()),
                    args: Some(vec!["src_b".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_c".to_string()),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_c".to_string()),
                    args: Some(vec!["src_c".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    out: Some("loads".to_string()),
                    args: Some(vec![
                        "flt_a".to_string(),
                        "flt_b".to_string(),
                        "flt_c".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["loads".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("none_ret".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["none_ret".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

fn compile_alias_owner_transfer(alias_kind: &str, release_source: bool) {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: format!("{alias_kind}_owner_transfer"),
            params: vec![],
            ops: {
                let mut ops = vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("src".to_string()),
                        s_value: Some("owned".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: alias_kind.to_string(),
                        args: Some(vec!["src".to_string()]),
                        out: Some("alias".to_string()),
                        ..OpIR::default()
                    },
                ];
                if release_source {
                    ops.push(OpIR {
                        kind: "dec_ref".to_string(),
                        args: Some(vec!["src".to_string()]),
                        ..OpIR::default()
                    });
                }
                ops.push(OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["alias".to_string()]),
                    ..OpIR::default()
                });
                ops
            },
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn native_backend_compiles_identity_alias_transferring_the_source_owner() {
    // A transparent identity has no independent retain. Its return transfers
    // the source owner; releasing that owner first would be use-after-free.
    compile_alias_owner_transfer("identity_alias", false);
}

#[test]
fn native_backend_compiles_binding_alias_after_source_dec_ref() {
    compile_alias_owner_transfer("binding_alias", true);
}

#[test]
fn shared_boxed_text_calls_reach_actual_native_dispatch() {
    use super::shared::import_calls;
    use crate::native_backend::simple_backend::tests::compile_selected_functions_direct;

    let mut functions = Vec::new();
    let mut cases = Vec::new();
    for receiver in ["string", "bytes", "bytearray"] {
        for method in ["find", "startswith", "endswith", "count"] {
            for suffix in ["", "_slice"] {
                let kind = format!("{receiver}_{method}{suffix}");
                let arity = if suffix.is_empty() { 2 } else { 6 };
                for (shape, out, wide) in [
                    ("absent", None, false),
                    ("none", Some("none"), false),
                    ("dead", Some("unused"), false),
                    ("live", Some("result"), false),
                    ("wide_live", Some("result"), true),
                ] {
                    let name = format!("test_{kind}_{shape}");
                    let mut ops = Vec::new();
                    if wide {
                        ops.extend([
                            OpIR {
                                kind: "const_int".into(),
                                value: Some(1_i64 << 31),
                                out: Some("limb".into()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "checked_mul".into(),
                                args: Some(vec!["limb".into(); 2]),
                                var: Some("wide".into()),
                                out: Some("none".into()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "checked_add".into(),
                                args: Some(vec!["wide".into(), "limb".into()]),
                                var: Some("wider".into()),
                                out: Some("none".into()),
                                ..OpIR::default()
                            },
                        ]);
                    }
                    let sources = if wide {
                        ["wide", "wider"]
                    } else {
                        ["first", "second"]
                    };
                    ops.push(OpIR {
                        kind: kind.clone(),
                        args: Some((0..arity).map(|index| sources[index % 2].into()).collect()),
                        out: out.map(str::to_owned),
                        ..OpIR::default()
                    });
                    ops.push(OpIR {
                        kind: "ret".into(),
                        args: Some(vec![
                            if out == Some("result") {
                                "result"
                            } else {
                                "none"
                            }
                            .into(),
                        ]),
                        ..OpIR::default()
                    });
                    let function = FunctionIR {
                        return_abi: molt_ir::FunctionReturnAbi::Value,
                        name: name.clone(),
                        params: if wide {
                            vec![]
                        } else {
                            vec!["first".into(), "second".into()]
                        },
                        ops,
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                        codegen_partition: false,
                        parameter_custody: Vec::new(),
                        execution_context: Default::default(),
                    };
                    if wide {
                        let plan = native_representation_plan_for_test(&function);
                        for source in sources {
                            assert!(
                                plan.is_full_deopt_int_name(source),
                                "{name}/{source}: physical box required"
                            );
                        }
                    }
                    functions.push(function);
                    cases.push((name, format!("molt_{kind}"), shape, arity, wide));
                }
            }
        }
    }
    // One batched production compile_func_inner pass, retaining each emitted
    // body. A union of imports cannot distinguish omitted resultless calls.
    let names: Vec<_> = cases.iter().map(|case| case.0.as_str()).collect();
    let backend = compile_selected_functions_direct(functions, &names);
    let ids = backend
        .import_ids
        .iter()
        .map(|(&symbol, (id, _))| (symbol, id.as_u32()))
        .collect();
    for (name, symbol, shape, arity, wide) in &cases {
        let function = &backend
            .deferred_defines
            .iter()
            .find(|body| body.name == *name)
            .expect("actual native dispatch body")
            .func;
        verify_function(function, &settings::Flags::new(settings::builder()))
            .unwrap_or_else(|errors| panic!("{name}: {errors}\n{}", function.display()));
        let calls = import_calls(function, &ids, symbol);
        assert_eq!(calls.len(), 1, "{name}: {}", function.display());
        let arguments = function.dfg.inst_args(calls[0]);
        assert_eq!(arguments.len(), *arity, "{name}");
        let releases = import_calls(function, &ids, "molt_dec_ref_obj");
        if *wide {
            assert_eq!(
                import_calls(function, &ids, "molt_int_from_i64").len(),
                2,
                "{name}"
            );
            assert_eq!(
                import_calls(function, &ids, "molt_exception_pending_fast").len(),
                2,
                "{name}"
            );
            assert_ne!(arguments[0], arguments[1], "{name}: distinct boxes");
            for (index, argument) in arguments.iter().enumerate() {
                assert_eq!(
                    *argument,
                    arguments[index % 2],
                    "{name}: repeated source reuses its box"
                );
            }
            assert_eq!(
                releases.len(),
                2,
                "{name}: only the two temporary owners retire; result transfers: {}",
                function.display()
            );
        } else {
            assert!(
                import_calls(function, &ids, "molt_int_from_i64").is_empty(),
                "{name}"
            );
            let result = function
                .dfg
                .resolve_aliases(function.dfg.inst_results(calls[0])[0]);
            let result_releases = releases
                .iter()
                .filter(|&&release| {
                    function
                        .dfg
                        .resolve_aliases(function.dfg.inst_args(release)[0])
                        == result
                })
                .count();
            assert_eq!(
                result_releases,
                usize::from(*shape != "live"),
                "{name}: absent/none/dead results retire exactly once; live result transfers: {}",
                function.display()
            );
        }
    }
}
