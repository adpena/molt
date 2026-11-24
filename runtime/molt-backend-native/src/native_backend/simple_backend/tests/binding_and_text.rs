use super::*;
use crate::test_support::native_object_symbols;
use molt_ir::OpKind;

#[test]
fn native_backend_compiles_float_primary_tuple_escape_before_exception_cleanup() {
    let _runtime = super::enter_full_runtime_fixture();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "float_primary_tuple_cleanup".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("src_a".to_string()),
                    value: Some(1),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("flt_a".to_string()),
                    args: Some(vec!["src_a".to_string()]),
                    ..OpIR::new(OpKind::FloatFromObj)
                },
                OpIR {
                    out: Some("src_b".to_string()),
                    value: Some(2),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("flt_b".to_string()),
                    args: Some(vec!["src_b".to_string()]),
                    ..OpIR::new(OpKind::FloatFromObj)
                },
                OpIR {
                    out: Some("src_c".to_string()),
                    value: Some(3),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("flt_c".to_string()),
                    args: Some(vec!["src_c".to_string()]),
                    ..OpIR::new(OpKind::FloatFromObj)
                },
                OpIR {
                    out: Some("loads".to_string()),
                    args: Some(vec![
                        "flt_a".to_string(),
                        "flt_b".to_string(),
                        "flt_c".to_string(),
                    ]),
                    ..OpIR::new(OpKind::TupleNew)
                },
                OpIR {
                    value: Some(7),
                    ..OpIR::new(OpKind::CheckException)
                },
                OpIR {
                    args: Some(vec!["loads".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    value: Some(7),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    out: Some("none_ret".to_string()),
                    ..OpIR::new(OpKind::ConstNone)
                },
                OpIR {
                    args: Some(vec!["none_ret".to_string()]),
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
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

fn compile_alias_owner_transfer(alias_kind: OpKind, release_source: bool) {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: format!("{alias_kind}_owner_transfer"),
            params: vec![],
            ops: {
                let mut ops = vec![
                    OpIR {
                        out: Some("src".to_string()),
                        s_value: Some("owned".to_string()),
                        ..OpIR::new(OpKind::ConstStr)
                    },
                    OpIR {
                        args: Some(vec!["src".to_string()]),
                        out: Some("alias".to_string()),
                        ..OpIR::new(alias_kind)
                    },
                ];
                if release_source {
                    ops.push(OpIR {
                        args: Some(vec!["src".to_string()]),
                        ..OpIR::new(OpKind::DecRef)
                    });
                }
                ops.push(OpIR {
                    args: Some(vec!["alias".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                });
                ops
            },
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
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
    compile_alias_owner_transfer(OpKind::IdentityAlias, false);
}

#[test]
fn native_backend_compiles_binding_alias_after_source_dec_ref() {
    compile_alias_owner_transfer(OpKind::BindingAlias, true);
}

#[test]
fn shared_boxed_text_calls_reach_actual_native_dispatch() {
    use super::{compile_final_functions, finalize_through_tir};
    use cranelift_codegen::{settings, verifier::verify_function};

    let mut functions = Vec::new();
    let mut cases = Vec::new();
    for (kind, arity) in [
        (OpKind::StringFind, 2),
        (OpKind::StringFindSlice, 6),
        (OpKind::StringStartswith, 2),
        (OpKind::StringStartswithSlice, 6),
        (OpKind::StringEndswith, 2),
        (OpKind::StringEndswithSlice, 6),
        (OpKind::StringCount, 2),
        (OpKind::StringCountSlice, 6),
        (OpKind::BytesFind, 2),
        (OpKind::BytesFindSlice, 6),
        (OpKind::BytesStartswith, 2),
        (OpKind::BytesStartswithSlice, 6),
        (OpKind::BytesEndswith, 2),
        (OpKind::BytesEndswithSlice, 6),
        (OpKind::BytesCount, 2),
        (OpKind::BytesCountSlice, 6),
        (OpKind::BytearrayFind, 2),
        (OpKind::BytearrayFindSlice, 6),
        (OpKind::BytearrayStartswith, 2),
        (OpKind::BytearrayStartswithSlice, 6),
        (OpKind::BytearrayEndswith, 2),
        (OpKind::BytearrayEndswithSlice, 6),
        (OpKind::BytearrayCount, 2),
        (OpKind::BytearrayCountSlice, 6),
    ] {
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
                        value: Some(1_i64 << 31),
                        out: Some("limb".into()),
                        ..OpIR::new(OpKind::ConstInt)
                    },
                    OpIR {
                        args: Some(vec!["limb".into(); 2]),
                        var: Some("wide".into()),
                        out: Some("none".into()),
                        ..OpIR::new(OpKind::CheckedMul)
                    },
                    OpIR {
                        args: Some(vec!["wide".into(), "limb".into()]),
                        var: Some("wider".into()),
                        out: Some("none".into()),
                        ..OpIR::new(OpKind::CheckedAdd)
                    },
                ]);
            }
            let sources = if wide {
                ["wide", "wider"]
            } else {
                ["first", "second"]
            };
            ops.push(OpIR {
                kind,
                args: Some((0..arity).map(|index| sources[index % 2].into()).collect()),
                out: out.map(str::to_owned),
                ..OpIR::new(kind)
            });
            ops.push(OpIR {
                args: Some(vec![
                    if out == Some("result") {
                        "result"
                    } else {
                        "none"
                    }
                    .into(),
                ]),
                ..OpIR::new(OpKind::Ret)
            });
            let function = FunctionIR {
                module: None,
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
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            };
            functions.push(function);
            cases.push((name, format!("molt_{kind}"), shape, arity, wide));
        }
    }
    // One batch of production function definitions, retaining each emitted
    // body. A union of imports cannot distinguish omitted resultless calls.
    let names: Vec<_> = cases.iter().map(|case| case.0.as_str()).collect();
    let backend = compile_final_functions(finalize_through_tir(functions), &names, None);
    let ids = declared_import_ids(&backend.module);
    let import_calls = |f: &Function, ids: &BTreeMap<String, FuncId>, symbol: &str| -> Vec<Inst> {
        ids.get(symbol)
            .into_iter()
            .flat_map(|&id| {
                call_sites_for_import(f, id)
                    .into_iter()
                    .map(|(_, inst)| inst)
            })
            .collect()
    };
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
        let arguments = attached_arguments(function, calls[0]);
        assert_eq!(arguments.len(), *arity, "{name}");
        let releases: Vec<Inst> = release_call_sites(function, &ids)
            .into_iter()
            .map(|(_, inst)| inst)
            .collect();
        if *wide {
            assert_eq!(
                import_calls(function, &ids, "molt_int_from_i64").len(),
                2,
                "{name}"
            );
            assert_eq!(
                import_calls(function, &ids, "molt_exception_pending").len(),
                0,
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
                        .resolve_aliases(attached_arguments(function, release)[0])
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

#[test]
fn integer_literal_aliases_share_raw_materialization_and_discard_contracts() {
    use crate::native_backend::simple_backend::tests::{
        compile_selected_functions_direct, emit_direct_object,
    };
    for kind in [OpKind::Const, OpKind::ConstInt, OpKind::LoadConst] {
        for value in [i64::MIN, 0, i64::MAX] {
            let input = FunctionIR {
                module: None,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "integer_literal_contract".into(),
                params: vec![],
                param_types: None,
                ops: vec![
                    OpIR {
                        value: Some(value),
                        out: Some("value".into()),
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        value: Some(i64::MAX),
                        out: None,
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        value: Some(i64::MAX),
                        out: Some("none".into()),
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        args: Some(vec!["value".into()]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                frame_locals_exposed: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            };
            let backend =
                compile_selected_functions_direct(vec![input], &["integer_literal_contract"]);
            let function = &backend
                .deferred_defines
                .iter()
                .find(|deferred| deferred.name == "integer_literal_contract")
                .unwrap()
                .func;
            cranelift_codegen::verify_function(
                function,
                &cranelift_codegen::settings::Flags::new(cranelift_codegen::settings::builder()),
            )
            .unwrap_or_else(|errors| panic!("{kind}: {errors}\n{}", function.display()));
            assert!(
                backend
                    .import_ids
                    .get(molt_ir::runtime_fn_generated::RuntimeFn::BigintFromBytes)
                    .is_none(),
                "raw literals and discarded outputs need no boxed-literal constructor: {kind}"
            );
            assert!(
                native_object_symbols(&emit_direct_object(backend))
                    .defined
                    .contains(
                        &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
                            "integer_literal_contract"
                        )
                    )
            );
        }
    }
}
