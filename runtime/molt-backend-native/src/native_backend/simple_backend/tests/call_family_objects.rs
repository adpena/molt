//! Object-level call-family coverage through the production Cranelift pipeline.

use super::*;
use crate::test_support::native_object_symbols;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;

fn const_int(out: &str, v: i64) -> OpIR {
    OpIR {
        out: Some(out.to_string()),
        value: Some(v),
        ..OpIR::new(OpKind::Const)
    }
}

fn const_str(out: &str, s: &str) -> OpIR {
    OpIR {
        out: Some(out.to_string()),
        s_value: Some(s.to_string()),
        ..OpIR::new(OpKind::ConstStr)
    }
}

fn ret(name: &str) -> OpIR {
    OpIR {
        args: Some(vec![name.to_string()]),
        ..OpIR::new(OpKind::Ret)
    }
}

fn func(name: &str, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: vec![],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

/// Programs exercising the `handle_call_op` arms. Each returns a
/// `(name, SimpleIR)` pair. The function set is compiled together so that
/// intra-module known-function facts (direct/guarded dispatch) are realistic.
#[allow(clippy::vec_init_then_push)]
fn call_family_programs() -> Vec<(&'static str, SimpleIR)> {
    let mut progs: Vec<(&'static str, SimpleIR)> = Vec::new();

    // ----- plain `call` to a statically-known defined function (fast/direct + guarded paths).
    progs.push((
        "call_direct_and_guarded",
        SimpleIR {
            functions: vec![
                func(
                    "caller_direct",
                    vec![
                        const_int("a", 1),
                        OpIR {
                            s_value: Some("callee_leaf".to_string()),
                            args: Some(vec!["a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::new(OpKind::Call)
                        },
                        ret("r"),
                    ],
                ),
                FunctionIR {
                    params: vec!["value".into()],
                    // Keep the call boundary under test through the module inliner.
                    codegen_partition: true,
                    frame_locals_exposed: false,
                    parameter_custody: Vec::new(),
                    ..func("callee_leaf", vec![ret("value")])
                },
            ],
            profile: None,
        },
    ));

    // ----- `call` to an imported (not module-defined) symbol -> outlined guarded call.
    progs.push((
        "call_imported_outlined",
        SimpleIR {
            functions: vec![
                func(
                    "caller_import",
                    vec![
                        const_int("a", 5),
                        const_int("b", 6),
                        OpIR {
                            s_value: Some("some_external_symbol".to_string()),
                            args: Some(vec!["a".to_string(), "b".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::new(OpKind::Call)
                        },
                        ret("r"),
                    ],
                ),
                FunctionIR {
                    params: vec!["left".into(), "right".into()],
                    codegen_partition: true,
                    ..func("some_external_symbol", vec![ret("right")])
                },
            ],
            profile: None,
        },
    ));

    // ----- `call_internal` (direct internal dispatch).
    progs.push((
        "call_internal",
        SimpleIR {
            functions: vec![
                func(
                    "caller_internal",
                    vec![
                        const_int("a", 2),
                        OpIR {
                            s_value: Some("internal_target".to_string()),
                            args: Some(vec!["a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::new(OpKind::CallInternal)
                        },
                        ret("r"),
                    ],
                ),
                FunctionIR {
                    params: vec!["value".into()],
                    codegen_partition: true,
                    frame_locals_exposed: false,
                    parameter_custody: Vec::new(),
                    ..func("internal_target", vec![ret("value")])
                },
            ],
            profile: None,
        },
    ));

    // ----- `call_guarded` (dynamic callee value + guard/merge control flow).
    progs.push((
        "call_guarded",
        SimpleIR {
            functions: vec![
                func(
                    "caller_guarded",
                    vec![
                        OpIR {
                            s_value: Some("guarded_target".into()),
                            out: Some("callee".into()),
                            value: Some(0),
                            ..OpIR::new(OpKind::FuncNew)
                        },
                        const_int("a", 7),
                        OpIR {
                            s_value: Some("guarded_target".to_string()),
                            args: Some(vec!["callee".to_string(), "a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::new(OpKind::CallGuarded)
                        },
                        ret("r"),
                    ],
                ),
                func("guarded_target", vec![const_int("z", 4), ret("z")]),
            ],
            profile: None,
        },
    ));

    // ----- `call_func` (inline probe fast-path: <=3 args, code_id 0).
    progs.push((
        "call_func_inline_probe",
        SimpleIR {
            functions: vec![func(
                "caller_call_func",
                vec![
                    const_str("f", "target_fn"),
                    const_int("a", 1),
                    const_int("b", 2),
                    OpIR {
                        args: Some(vec!["f".to_string(), "a".to_string(), "b".to_string()]),
                        out: Some("r".to_string()),
                        value: Some(0),
                        ..OpIR::new(OpKind::CallFunc)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_func` fallback dispatch (>3 args -> molt_call_func_dispatch).
    progs.push((
        "call_func_dispatch",
        SimpleIR {
            functions: vec![func(
                "caller_call_func_many",
                vec![
                    const_str("f", "target_fn"),
                    const_int("a", 1),
                    const_int("b", 2),
                    const_int("c", 3),
                    const_int("d", 4),
                    OpIR {
                        args: Some(vec![
                            "f".to_string(),
                            "a".to_string(),
                            "b".to_string(),
                            "c".to_string(),
                            "d".to_string(),
                        ]),
                        out: Some("r".to_string()),
                        value: Some(0),
                        ..OpIR::new(OpKind::CallFunc)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_bind` (consumes a callargs builder; tracked-root scrub path).
    progs.push((
        "call_bind",
        SimpleIR {
            functions: vec![func(
                "caller_call_bind",
                vec![
                    const_str("f", "bound_target"),
                    OpIR {
                        out: Some("cargs".to_string()),
                        ..OpIR::new(OpKind::CallargsNew)
                    },
                    OpIR {
                        args: Some(vec!["f".to_string(), "cargs".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::new(OpKind::CallBind)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_indirect`.
    progs.push((
        "call_indirect",
        SimpleIR {
            functions: vec![func(
                "caller_call_indirect",
                vec![
                    const_str("f", "indirect_target"),
                    OpIR {
                        out: Some("cargs".to_string()),
                        ..OpIR::new(OpKind::CallargsNew)
                    },
                    OpIR {
                        args: Some(vec!["f".to_string(), "cargs".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::new(OpKind::CallIndirect)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `load_method` / `load_super_method` and the loaded call
    // (CPython's method-form LOAD_ATTR / LOAD_SUPER_ATTR, then CALL).
    let loaded_call = |args: Vec<&str>| OpIR {
        argument_custody: Some(vec![molt_ir::ParameterCustody::Transferred; args.len()]),
        args: Some(args.into_iter().map(str::to_string).collect()),
        out: Some("r".to_string()),
        ..OpIR::new(OpKind::CallLoadedMethod)
    };
    progs.push((
        "load_method",
        SimpleIR {
            functions: vec![func(
                "caller_load_method",
                vec![
                    const_str("recv", "obj"),
                    OpIR {
                        s_value: Some("do_thing".to_string()),
                        args: Some(vec!["recv".to_string()]),
                        var: Some("callable".to_string()),
                        out: Some("self_or_null".to_string()),
                        ..OpIR::new(OpKind::LoadMethod)
                    },
                    const_int("a", 11),
                    loaded_call(vec!["callable", "self_or_null", "a"]),
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));
    progs.push((
        "load_super_method",
        SimpleIR {
            functions: vec![func(
                "caller_load_super_method",
                vec![
                    const_str("cls", "C"),
                    const_str("selfv", "inst"),
                    OpIR {
                        s_value: Some("do_thing".to_string()),
                        args: Some(vec!["cls".to_string(), "selfv".to_string()]),
                        var: Some("callable".to_string()),
                        out: Some("self_or_null".to_string()),
                        ..OpIR::new(OpKind::LoadSuperMethod)
                    },
                    const_int("a", 13),
                    loaded_call(vec!["callable", "self_or_null", "a"]),
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_method` generic IC path.
    progs.push((
        "call_method_generic",
        SimpleIR {
            functions: vec![func(
                "caller_call_method",
                vec![
                    const_str("m", "bound"),
                    const_int("a", 15),
                    OpIR {
                        args: Some(vec!["m".to_string(), "a".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::new(OpKind::CallMethod)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_method` fast bound-method dispatch (list.append).
    progs.push((
        "call_method_fast_list_append",
        SimpleIR {
            functions: vec![func(
                "caller_list_append",
                vec![
                    const_str("m", "bound"),
                    const_int("a", 17),
                    OpIR {
                        s_value: Some("BoundMethod:list:append".to_string()),
                        args: Some(vec!["m".to_string(), "a".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::new(OpKind::CallMethod)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `getframe`.
    progs.push((
        "getframe",
        SimpleIR {
            functions: vec![func(
                "caller_getframe",
                vec![
                    const_int("depth", 0),
                    OpIR {
                        args: Some(vec!["depth".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::new(OpKind::Getframe)
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    progs
}

#[test]
fn compiled_call_families_keep_their_providers_and_imports() {
    let _runtime = super::enter_full_runtime_fixture();
    for (name, mut ir) in call_family_programs() {
        // The first function is the canonical entry root. An object containing
        // only the callee is not a call-family consumer proof.
        let entry = ir.functions[0].name.clone();
        if ir.functions.len() > 1 {
            ir.profile = Some(crate::PgoProfileIR {
                hot_functions: ir
                    .functions
                    .iter()
                    .skip(1)
                    .map(|func| func.name.clone())
                    .collect(),
                ..crate::PgoProfileIR::default()
            });
        }
        let mut backend = SimpleBackend::new();
        if name == "call_imported_outlined" {
            let context = SimpleBackend::prepare_module_context(&mut ir.functions);
            ir.functions[1].externalize_with_signature().unwrap();
            backend.set_module_context(context);
        }
        let output = backend.compile(ir);
        assert!(
            !output.bytes.is_empty(),
            "program {name} produced no object bytes"
        );
        let symbols = native_object_symbols(&output.bytes);
        assert!(
            symbols.defined.contains(
                &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(&entry)
            ),
            "program {name} discarded caller {entry}"
        );
        for target in match name {
            "call_direct_and_guarded" => &["callee_leaf"][..],
            "call_internal" => &["internal_target"][..],
            _ => &[],
        } {
            assert!(
                symbols.defined.contains(
                    &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(target)
                ),
                "program {name} lost call boundary {target}"
            );
        }
        if name == "call_guarded" {
            assert!(
                symbols
                    .undefined
                    .contains(RuntimeFn::CallFuncFast1.linker_symbol()),
                "non-exact guarded ABI must bind the callable and one argument: {:?}",
                symbols.undefined
            );
        }
        if name == "call_func_inline_probe" {
            assert!(
                symbols
                    .undefined
                    .contains(RuntimeFn::CallFuncFast2.linker_symbol()),
                "dynamic call must pass its callable and two positional arguments: {:?}",
                symbols.undefined
            );
            assert!(!symbols.undefined.contains("molt_thread_record"));
            // The handoff is a store into the thread record, not a runtime call.
            for removed in ["molt_frame_invocation_enter", "molt_frame_invocation_exit"] {
                assert!(
                    !symbols.undefined.contains(removed),
                    "inline calls must not enter the runtime through {removed}"
                );
            }
        }
    }
}

#[test]
fn generated_runtime_call_agreement_imports_declared_native_function() {
    let _runtime = super::enter_full_runtime_fixture();
    use molt_ir::runtime_calls::RuntimeCall;
    for (&call, &kind) in RuntimeCall::ALL
        .iter()
        .flat_map(|call| call.op_kinds().iter().map(move |kind| (call, kind)))
    {
        let abi = call.boxed_abi();
        let params = (0..abi.arity())
            .map(|i| format!("arg{i}"))
            .collect::<Vec<_>>();
        for bound in [false, true] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: format!("contract_{}_{}", kind, bound),
                    params: params.clone(),
                    ops: vec![
                        OpIR {
                            args: Some(params.clone()),
                            out: (bound || kind == molt_ir::OpKind::GetIter)
                                .then(|| "result".into()),
                            ..OpIR::new(kind)
                        },
                        if bound {
                            OpIR {
                                args: Some(vec!["result".into()]),
                                ..OpIR::new(OpKind::Ret)
                            }
                        } else {
                            OpIR::new(OpKind::RetVoid)
                        },
                    ],
                    return_abi: if bound {
                        molt_ir::FunctionReturnAbi::Value
                    } else {
                        molt_ir::FunctionReturnAbi::Void
                    },
                    ..FunctionIR::default()
                }],
                profile: None,
            };
            let object = SimpleBackend::new().compile(ir);
            let symbols = native_object_symbols(&object.bytes).undefined;
            assert!(
                symbols.contains(physical_runtime_symbol(abi.symbol())),
                "{call:?}: {symbols:?}"
            );
            assert!(
                !symbols.contains(physical_runtime_symbol("molt_store_index")),
                "dict storage must bypass the subclass protocol"
            );
            assert!(
                !symbols.contains(physical_runtime_symbol("molt_iter")),
                "iteration must retain the checked error protocol"
            );
        }
    }
}

#[cfg(feature = "jit")]
#[test]
fn generated_runtime_call_agreement_jit_admits_declared_import_before_relocation() {
    let _runtime = super::enter_full_runtime_fixture();
    use molt_ir::runtime_calls::RuntimeCall;
    for (&call, &kind) in RuntimeCall::ALL
        .iter()
        .flat_map(|call| call.op_kinds().iter().map(move |kind| (call, kind)))
    {
        let abi = call.boxed_abi();
        let args = (0..abi.arity())
            .map(|i| format!("arg{i}"))
            .collect::<Vec<_>>();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: format!("contract_jit_{kind}"),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: args.clone(),
                ops: vec![
                    OpIR {
                        args: Some(args),
                        out: Some("result".into()),
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        args: Some(vec!["result".into()]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
                ..FunctionIR::default()
            }],
            profile: None,
        };
        // Deliberately absent provider: the first-call compiler must check
        // the generated declarations before it relocates the body.
        static ABI_WITNESS: u8 = 0;
        let result = SimpleBackend::new_jit(std::sync::Arc::new(|name| {
            (name == molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL)
                .then_some(std::ptr::from_ref(&ABI_WITNESS))
        }))
        .compile_jit(ir)
        .and_then(|program| program.validate_body_relocations());
        let Err(crate::native_backend::simple_backend::JitCompileError::UnresolvedImports(symbols)) =
            result
        else {
            panic!("JIT must report unresolved imports before relocation");
        };
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol == physical_runtime_symbol(abi.symbol())),
            "{call:?}: {symbols:?}"
        );
        assert!(
            !symbols
                .iter()
                .any(|symbol| symbol == "molt_store_index" || symbol == "molt_iter")
        );
    }
}
