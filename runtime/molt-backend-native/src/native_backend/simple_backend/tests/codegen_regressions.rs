use super::*;
use cranelift_codegen::ir::InstBuilder;
use molt_ir::OpKind;

#[test]
fn native_completion_retires_dead_builder_components_without_traps() {
    let backend = SimpleBackend::new();
    let mut context = cranelift_codegen::Context::new();
    context.func.signature.call_conv = backend.module.isa().default_call_conv();
    let mut builder_context = cranelift_frontend::FunctionBuilderContext::new();
    let entry;
    let dead_header;
    let dead_body;
    {
        let mut builder =
            cranelift_frontend::FunctionBuilder::new(&mut context.func, &mut builder_context);
        entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.ins().return_(&[]);
        dead_header = builder.create_block();
        dead_body = builder.create_block();
        builder.insert_block_after(dead_header, entry);
        builder.switch_to_block(dead_body);
        builder.ins().jump(dead_header, &[]);
        builder.seal_all_blocks();
        builder.finalize(backend.module.target_config());
    }
    assert!(context.func.layout.is_block_inserted(dead_header));
    assert!(context.func.layout.is_block_inserted(dead_body));
    super::super::deferred_codegen::complete_native_control_flow(
        &mut context,
        backend.module.isa(),
        "dead_builder_loop",
    );
    assert_eq!(
        context.func.layout.blocks().collect::<Vec<_>>(),
        vec![entry]
    );
    cranelift_codegen::verify_function(&context.func, backend.module.isa())
        .expect("the upstream verifier admits completed CFG");
    context
        .compile(
            backend.module.isa(),
            &mut cranelift_codegen::control::ControlPlane::default(),
        )
        .expect("completed builder CFG compiles");
}

#[test]
#[should_panic(expected = "native codegen left REACHABLE block")]
fn native_completion_rejects_reachable_empty_builder_blocks() {
    let backend = SimpleBackend::new();
    let mut context = cranelift_codegen::Context::new();
    context.func.signature.call_conv = backend.module.isa().default_call_conv();
    let mut builder_context = cranelift_frontend::FunctionBuilderContext::new();
    {
        let mut builder =
            cranelift_frontend::FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        let empty_target = builder.create_block();
        builder.switch_to_block(entry);
        builder.ins().jump(empty_target, &[]);
        builder.insert_block_after(empty_target, entry);
        builder.seal_all_blocks();
        builder.finalize(backend.module.target_config());
    }
    super::super::deferred_codegen::complete_native_control_flow(
        &mut context,
        backend.module.isa(),
        "invalid_live_target",
    );
}

#[test]
fn annotate_function_object_compiles_without_signature_mismatch() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "_sitebuiltins____annotate__".to_string(),
                params: vec!["format".to_string()],
                ops: vec![OpIR {
                    ..OpIR::new(OpKind::RetVoid)
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
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        s_value: Some("_sitebuiltins____annotate__".to_string()),
                        value: Some(1),
                        out: Some("annotate_fn".to_string()),
                        ..OpIR::new(OpKind::FuncNew)
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
            },
        ],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn guarded_void_function_object_compiles_without_result_panic() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
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
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        s_value: Some("void_helper".to_string()),
                        value: Some(0),
                        out: Some("void_fn".to_string()),
                        ..OpIR::new(OpKind::FuncNew)
                    },
                    OpIR {
                        s_value: Some("void_helper".to_string()),
                        args: Some(vec!["void_fn".to_string()]),
                        out: Some("result".to_string()),
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
            },
        ],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn direct_imported_runtime_call_avoids_guarded_call_wrapper() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "hot_runtime_call".to_string(),
        params: vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
            "h".to_string(),
        ],
        ops: vec![
            OpIR {
                s_value: Some("molt_gpu_linear_contiguous".to_string()),
                args: Some(vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                    "e".to_string(),
                    "f".to_string(),
                    "g".to_string(),
                    "h".to_string(),
                ]),
                out: Some("out".to_string()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["out".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let clif = compile_function_to_clif_text(vec![func], "hot_runtime_call");

    assert!(
        !clif.contains("molt_guarded_call"),
        "direct imported runtime calls should not route through molt_guarded_call:\n{clif}"
    );
    assert!(
        !clif.contains("explicit_slot"),
        "direct imported runtime calls should not spill args for the guarded-call wrapper:\n{clif}"
    );
}

fn non_leaf_transport_callee(name: &str, params: Vec<String>) -> FunctionIR {
    // A defined recursive callee cannot be classified as a leaf. The caller
    // must therefore emit the actual recursion admission and error branches.
    FunctionIR {
        name: name.into(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        params: params.clone(),
        ops: vec![
            OpIR {
                s_value: Some(name.into()),
                args: Some(params),
                out: Some("recursive_result".into()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["recursive_result".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..Default::default()
    }
}

fn assert_guarded_continuation_sources(compiled: &CompiledFunctionClif, parameters: &[usize]) {
    let function = &compiled.function;
    let enter = compiled.import_ids["molt_recursion_enter_fast"];
    let exit = compiled.import_ids["molt_recursion_exit_fast"];
    assert_eq!(
        call_sites_for_import(function, enter).len(),
        1,
        "{}",
        function.display()
    );
    let exits = call_sites_for_import(function, exit);
    assert_eq!(exits.len(), 1, "{}", function.display());
    let continuation = exits[0].0;
    let entry = function.layout.entry_block().unwrap();
    let expected: BTreeSet<_> = parameters
        .iter()
        .map(|&index| {
            function
                .dfg
                .resolve_aliases(function.dfg.block_params(entry)[index])
        })
        .collect();
    let terminal = function.layout.last_inst(continuation).unwrap();
    let InstructionData::Jump { destination, .. } = &function.dfg.insts[terminal] else {
        panic!(
            "successful call arm must reach its value return: {}",
            function.display()
        );
    };
    let args: Vec<_> = destination.args(&function.dfg.value_lists).collect();
    let [BlockArg::Value(actual)] = args.as_slice() else {
        panic!(
            "successful return must carry one value: {}",
            function.display()
        );
    };
    if let [parameter] = parameters {
        assert!(
            value_originates_only_from(
                function,
                *actual,
                function.dfg.block_params(entry)[*parameter]
            ),
            "{}",
            function.display()
        );
    } else {
        assert_eq!(
            canonical_value_sources(function, *actual),
            expected,
            "{}",
            function.display()
        );
    }
    assert!(
        function.layout.blocks().any(|block| function
            .layout
            .block_insts(block)
            .any(|inst| matches!(function.dfg.insts[inst], InstructionData::Brif { .. }))),
        "recursion admission must split the actual CFG: {}",
        function.display()
    );
}

#[test]
fn direct_call_minicfg_preserves_unrelated_live_parameter() {
    let mut params = vec!["carried".to_string()];
    params.extend((0..8).map(|idx| format!("arg{idx}")));
    let callee_name = "non_leaf_transport";
    let callee = non_leaf_transport_callee(callee_name, params[1..].to_vec());
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "direct_call_live_through".into(),
        params: params.clone(),
        ops: vec![
            OpIR {
                s_value: Some(callee_name.into()),
                args: Some(params[1..].to_vec()),
                out: Some("ignored_result".into()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["carried".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..Default::default()
    };
    let compiled =
        compile_function_to_clif_with_imports(vec![func, callee], "direct_call_live_through");
    assert_guarded_continuation_sources(&compiled, &[0]);
}

#[test]
fn native_boxed_or_retains_selected_operand_result() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "boxed_or_selected_owner".to_string(),
        params: vec!["lhs".to_string(), "rhs".to_string()],
        ops: vec![
            OpIR {
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("selected".to_string()),
                ..OpIR::new(OpKind::Or)
            },
            OpIR {
                args: Some(vec!["selected".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let clif = compile_function_to_clif_text(vec![func], "boxed_or_selected_owner");
    let selected = clif
        .lines()
        .find_map(|line| line.trim().split_once(" = select ").map(|(v, _)| v.trim()))
        .unwrap_or_else(|| panic!("boxed or must emit a selected result:\n{clif}"));
    let selected_call = format!("({selected})");
    assert!(
        clif.lines().any(|line| {
            let line = line.trim();
            line.starts_with("call fn") && line.contains(&selected_call)
        }),
        "boxed or must retain the selected result before returning it:\n{clif}"
    );
}

#[test]
fn native_shift_lowering_uses_runtime_without_shift_count_proof() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "shift_runtime_contract".to_string(),
        params: vec!["lhs".to_string(), "rhs".to_string()],
        ops: vec![
            OpIR {
                value: Some(8),
                out: Some("const_lhs".to_string()),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                value: Some(1),
                out: Some("const_rhs".to_string()),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("left".to_string()),
                ..OpIR::new(OpKind::Lshift)
            },
            OpIR {
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("right".to_string()),
                ..OpIR::new(OpKind::Rshift)
            },
            OpIR {
                args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                out: Some("const_left".to_string()),
                ..OpIR::new(OpKind::Shl)
            },
            OpIR {
                args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                out: Some("const_right".to_string()),
                ..OpIR::new(OpKind::Shr)
            },
            OpIR {
                args: Some(vec!["left".to_string(), "right".to_string()]),
                out: Some("param_sum".to_string()),
                ..OpIR::new(OpKind::Add)
            },
            OpIR {
                args: Some(vec!["const_left".to_string(), "const_right".to_string()]),
                out: Some("const_sum".to_string()),
                ..OpIR::new(OpKind::Add)
            },
            OpIR {
                args: Some(vec!["param_sum".to_string(), "const_sum".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::new(OpKind::Add)
            },
            OpIR {
                args: Some(vec!["out".to_string()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let clif = compile_function_to_clif_text(vec![func], "shift_runtime_contract");
    let binary_calls = clif
        .lines()
        .filter(|line| line.contains(" = call fn"))
        .count();

    assert!(
        binary_calls >= 4,
        "native shifts over typed params and raw-primary constants must remain runtime calls until range and shift-count proof exists:\n{clif}"
    );
    assert!(
        !clif.contains("ishl.i64 v1, v2") && !clif.contains("sshr v1, v2"),
        "native shifts must not lower directly to raw Cranelift shifts without proof:\n{clif}"
    );
}

#[test]
fn nested_exception_raise_if_does_not_synthesize_zero_predecessors() {
    let function = compile_function_to_clif(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    value: Some(0),
                    out: Some("flag".to_string()),
                    ..OpIR::new(OpKind::ConstBool)
                },
                OpIR {
                    args: Some(vec!["flag".to_string()]),
                    ..OpIR::new(OpKind::If)
                },
                OpIR {
                    ..OpIR::new(OpKind::Else)
                },
                OpIR {
                    out: Some("exc".to_string()),
                    ..OpIR::new(OpKind::ExceptionLast)
                },
                OpIR {
                    out: Some("nonev".to_string()),
                    ..OpIR::new(OpKind::ConstNone)
                },
                OpIR {
                    args: Some(vec!["exc".to_string(), "nonev".to_string()]),
                    out: Some("is_none".to_string()),
                    ..OpIR::new(OpKind::Is)
                },
                OpIR {
                    args: Some(vec!["is_none".to_string()]),
                    out: Some("has_exc".to_string()),
                    ..OpIR::new(OpKind::Not)
                },
                OpIR {
                    args: Some(vec!["has_exc".to_string()]),
                    ..OpIR::new(OpKind::If)
                },
                OpIR {
                    out: Some("none".to_string()),
                    ..OpIR::new(OpKind::ExceptionStackClear)
                },
                OpIR {
                    args: Some(vec!["exc".to_string()]),
                    out: Some("none".to_string()),
                    ..OpIR::new(OpKind::Raise)
                },
                OpIR {
                    value: Some(1),
                    ..OpIR::new(OpKind::CheckException)
                },
                OpIR {
                    value: Some(1),
                    ..OpIR::new(OpKind::Jump)
                },
                OpIR {
                    ..OpIR::new(OpKind::EndIf)
                },
                OpIR {
                    value: Some(1),
                    ..OpIR::new(OpKind::CheckException)
                },
                OpIR {
                    ..OpIR::new(OpKind::EndIf)
                },
                OpIR {
                    value: Some(1),
                    ..OpIR::new(OpKind::Label)
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
        "molt_main",
    );

    // Inspect the actual integer payload, not a printed prefix: boxed None
    // legitimately starts with `0x` and is the empty ownership-token value.
    let mut suspicious = Vec::new();
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            if let InstructionData::Jump { destination, .. } = function.dfg.insts[inst] {
                for argument in destination.args(&function.dfg.value_lists) {
                    if let Some(value) = argument.as_value()
                        && constant(&function, value) == Some(0)
                    {
                        suspicious.push((inst, value));
                    }
                }
            }
        }
    }

    assert!(
        suspicious.is_empty(),
        "nested exception raise CFG synthesized zero-valued predecessors: {suspicious:?}\n{}",
        function.display()
    );
}

#[test]
fn semantic_branch_edges_preserve_actual_dominating_parameter_without_phi() {
    let function = compile_function_to_clif(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "semantic_edge_transport".to_string(),
            params: vec!["carried".to_string(), "cond".to_string()],
            ops: vec![
                OpIR {
                    args: Some(vec!["cond".to_string()]),
                    value: Some(7),
                    ..OpIR::new(OpKind::BrIf)
                },
                OpIR {
                    args: Some(vec!["carried".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    value: Some(7),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    args: Some(vec!["carried".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
            param_types: Some(vec!["dyn".to_string(), "bool".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        "semantic_edge_transport",
    );

    assert_branch_preserves_parameter_without_phi(&function, 0);
}

#[test]
fn exception_edges_preserve_actual_dominating_parameter_without_phi() {
    let function = compile_function_to_clif(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "exception_edge_transport".to_string(),
            params: vec!["carried".to_string()],
            ops: vec![
                OpIR {
                    value: Some(11),
                    ..OpIR::new(OpKind::CheckException)
                },
                OpIR {
                    args: Some(vec!["carried".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    value: Some(11),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    args: Some(vec!["carried".to_string()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
            param_types: Some(vec!["dyn".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        "exception_edge_transport",
    );

    assert_branch_preserves_parameter_without_phi(&function, 0);
}

fn assert_branch_preserves_parameter_without_phi(function: &Function, parameter: usize) {
    let entry = function.layout.entry_block().expect("entry block");
    let expected = function.dfg.block_params(entry)[parameter];
    let mut returns = 0;
    let mut branches = 0;
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            if let InstructionData::Brif { blocks, .. } = &function.dfg.insts[inst] {
                branches += 1;
                for destination in blocks {
                    assert_eq!(
                        destination.args(&function.dfg.value_lists).count(),
                        0,
                        "immutable parameter needs no incoming phi transport: {}",
                        function.display()
                    );
                }
            }
            if function.dfg.insts[inst].opcode() == Opcode::Return {
                returns += 1;
                let [actual] = function.dfg.inst_args(inst) else {
                    panic!("value return");
                };
                assert!(
                    value_originates_only_from(function, *actual, expected),
                    "every returning path must return the actual function parameter, never a placeholder: {}",
                    function.display()
                );
            }
        }
    }
    assert!(returns != 0 && branches != 0, "{}", function.display());
}

#[test]
fn redefined_value_survives_successive_exception_and_branch_fallthroughs() {
    let mut ops = vec![OpIR {
        args: Some(vec!["initial".into()]),
        out: Some("carried".into()),
        ..OpIR::new(OpKind::Copy)
    }];
    for (exception, branch, replacement) in [(10, 11, false), (12, 13, true)] {
        if replacement {
            ops.push(OpIR {
                args: Some(vec!["replacement".into()]),
                out: Some("carried".into()),
                ..OpIR::new(OpKind::Copy)
            });
        }
        ops.push(OpIR {
            value: Some(exception),
            ..OpIR::new(OpKind::CheckException)
        });
        ops.push(OpIR {
            value: Some(branch),
            args: Some(vec!["condition".into()]),
            ..OpIR::new(OpKind::BrIf)
        });
    }
    ops.push(OpIR {
        args: Some(vec!["carried".into()]),
        ..OpIR::new(OpKind::Ret)
    });
    for label in 10..=13 {
        ops.push(OpIR {
            value: Some(label),
            ..OpIR::new(OpKind::Label)
        });
        ops.push(OpIR {
            args: Some(vec!["carried".into()]),
            ..OpIR::new(OpKind::Ret)
        });
    }
    let function = compile_function_to_clif(
        vec![FunctionIR {
            name: "redefined_fallthrough_value".into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            params: vec!["initial".into(), "replacement".into(), "condition".into()],
            ops,
            ..Default::default()
        }],
        "redefined_fallthrough_value",
    );
    let entry = function.layout.entry_block().unwrap();
    let initial = function.dfg.block_params(entry)[0];
    let replacement = function.dfg.block_params(entry)[1];
    let return_block = function
        .layout
        .blocks()
        .find(|&block| {
            function
                .layout
                .block_insts(block)
                .any(|inst| function.dfg.insts[inst].opcode() == Opcode::Return)
        })
        .unwrap();
    let cfg = ControlFlowGraph::with_function(&function);
    let mut observed = [0, 0];
    for predecessor in cfg.pred_iter(return_block) {
        let destinations = function.dfg.insts[predecessor.inst]
            .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables);
        for destination in destinations
            .iter()
            .filter(|edge| edge.block(&function.dfg.value_lists) == return_block)
        {
            let args: Vec<_> = destination.args(&function.dfg.value_lists).collect();
            let [BlockArg::Value(actual)] = args.as_slice() else {
                panic!("value return");
            };
            if value_originates_only_from(&function, *actual, initial) {
                observed[0] += 1;
            } else {
                assert!(
                    value_originates_only_from(&function, *actual, replacement),
                    "no placeholder or stale source may reach a return: {}",
                    function.display()
                );
                observed[1] += 1;
            }
        }
    }
    assert_eq!(
        observed,
        [2, 3],
        "the first two exits precede redefinition; the next three follow it: {}",
        function.display()
    );
}

#[test]
fn preloop_value_survives_exception_fallthrough_with_backend_managed_index() {
    let function = compile_function_to_clif(
        vec![FunctionIR {
            name: "indexed_loop_exception_transport".into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            params: vec!["carried".into(), "start".into(), "keep_going".into()],
            param_types: Some(vec!["dyn".into(), "int".into(), "bool".into()]),
            ops: vec![
                OpIR {
                    ..OpIR::new(OpKind::LoopStart)
                },
                OpIR {
                    out: Some("index".into()),
                    args: Some(vec!["start".into()]),
                    ..OpIR::new(OpKind::LoopIndexStart)
                },
                OpIR {
                    args: Some(vec!["keep_going".into()]),
                    ..OpIR::new(OpKind::LoopBreakIfFalse)
                },
                OpIR {
                    value: Some(17),
                    ..OpIR::new(OpKind::CheckException)
                },
                OpIR {
                    args: Some(vec!["carried".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    ..OpIR::new(OpKind::LoopEnd)
                },
                OpIR {
                    args: Some(vec!["carried".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
                OpIR {
                    value: Some(17),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    args: Some(vec!["carried".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
            ..Default::default()
        }],
        "indexed_loop_exception_transport",
    );
    // loop_index_start disables canonical direct-definition binding. Both
    // normal and handler returns must resolve through actual lazy SSA.
    assert_return_parameter_sources(&function, &[0]);
    let branches = function
        .layout
        .blocks()
        .flat_map(|block| function.layout.block_insts(block))
        .filter(|&inst| matches!(function.dfg.insts[inst], InstructionData::Brif { .. }))
        .count();
    assert!(
        branches >= 2,
        "loop admission and exception split must both exist: {}",
        function.display()
    );
}

#[test]
fn int_overflow_result_does_not_unbox_merged_bigint_result() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("base".to_string()),
                    value: Some(2),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("exp".to_string()),
                    value: Some(63),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    args: Some(vec!["base".to_string(), "exp".to_string()]),
                    out: Some("powv".to_string()),
                    ..OpIR::new(OpKind::Pow)
                },
                OpIR {
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    args: Some(vec!["powv".to_string(), "one".to_string()]),
                    out: Some("maxsize".to_string()),
                    ..OpIR::new(OpKind::Sub)
                },
                OpIR {
                    args: Some(vec!["maxsize".to_string()]),
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
        "molt_main",
    );

    assert!(
        !clif.contains("block11(v43: i64):\n    v77 = iconst.i64 0x7fff_0000_0000_0000"),
        "merged overflow result must remain boxed until a real inline-int consumer proves otherwise:\n{clif}",
    );
}

#[test]
fn bool_primary_loop_compare_does_not_materialize_boxed_bool() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("init".to_string()),
                    value: Some(0),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    out: Some("limit".to_string()),
                    value: Some(10),
                    ..OpIR::new(OpKind::Const)
                },
                OpIR {
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["init".to_string()]),
                    ..OpIR::new(OpKind::StoreVar)
                },
                OpIR {
                    ..OpIR::new(OpKind::LoopStart)
                },
                OpIR {
                    out: Some("i_cur".to_string()),
                    var: Some("_bb1_arg0".to_string()),
                    ..OpIR::new(OpKind::LoadVar)
                },
                OpIR {
                    out: Some("keep_going".to_string()),
                    args: Some(vec!["i_cur".to_string(), "limit".to_string()]),
                    ..OpIR::new(OpKind::Lt)
                },
                OpIR {
                    args: Some(vec!["keep_going".to_string()]),
                    ..OpIR::new(OpKind::LoopBreakIfFalse)
                },
                OpIR {
                    out: Some("i_next".to_string()),
                    args: Some(vec!["i_cur".to_string(), "one".to_string()]),
                    ..OpIR::new(OpKind::Add)
                },
                OpIR {
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["i_next".to_string()]),
                    ..OpIR::new(OpKind::StoreVar)
                },
                OpIR {
                    ..OpIR::new(OpKind::LoopContinue)
                },
                OpIR {
                    ..OpIR::new(OpKind::LoopEnd)
                },
                OpIR {
                    args: Some(vec!["i_cur".to_string()]),
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
        "molt_main",
    );

    assert!(
        clif.contains("icmp slt"),
        "loop comparison should lower to a raw signed compare:\n{clif}"
    );
    assert!(
        !clif.contains("0x7ffa_0000_0000_0000"),
        "bool-primary loop compare should not materialize a NaN-boxed bool:\n{clif}"
    );
}

fn structured_phi_transport_fixture(name: &str, tail: Vec<OpIR>) -> FunctionIR {
    let mut ops = vec![
        OpIR {
            args: Some(vec!["cond".into()]),
            ..OpIR::new(OpKind::If)
        },
        OpIR {
            ..OpIR::new(OpKind::Else)
        },
        OpIR {
            ..OpIR::new(OpKind::EndIf)
        },
        OpIR {
            out: Some("merged".into()),
            args: Some(vec!["lhs".into(), "rhs".into()]),
            ..OpIR::new(OpKind::Phi)
        },
    ];
    ops.extend(tail);
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.into(),
        params: vec![
            "_bb1_arg0".into(),
            "lhs".into(),
            "rhs".into(),
            "cond".into(),
        ],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

fn assert_return_parameter_sources(function: &Function, parameters: &[usize]) {
    let entry = function.layout.entry_block().unwrap();
    let expected: BTreeSet<_> = parameters
        .iter()
        .map(|&index| function.dfg.block_params(entry)[index])
        .collect();
    let mut returns = 0;
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            if function.dfg.insts[inst].opcode() == Opcode::Return {
                returns += 1;
                let [value] = function.dfg.inst_args(inst) else {
                    panic!("value return");
                };
                assert_eq!(
                    canonical_value_sources(function, *value),
                    expected,
                    "return must originate from precisely the declared semantic inputs: {}",
                    function.display()
                );
            }
        }
    }
    assert!(returns != 0);
}

#[test]
fn structured_phi_actual_selected_value_survives_label_transport() {
    let name = "structured_phi_label";
    let function = compile_function_to_clif(
        vec![structured_phi_transport_fixture(
            name,
            vec![
                OpIR {
                    value: Some(7),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    args: Some(vec!["merged".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
        )],
        name,
    );
    assert_return_parameter_sources(&function, &[1, 2]);
}

#[test]
fn structured_phi_actual_selected_value_survives_internal_cfg() {
    let name = "structured_phi_live_through";
    let callee_name = "non_leaf_phi_transport";
    let compiled = compile_function_to_clif_with_imports(
        vec![
            structured_phi_transport_fixture(
                name,
                vec![
                    OpIR {
                        s_value: Some(callee_name.into()),
                        args: Some(vec![]),
                        out: Some("unrelated".into()),
                        ..OpIR::new(OpKind::Call)
                    },
                    OpIR {
                        args: Some(vec!["merged".into()]),
                        ..OpIR::new(OpKind::Ret)
                    },
                ],
            ),
            non_leaf_transport_callee(callee_name, vec![]),
        ],
        name,
    );
    assert_guarded_continuation_sources(&compiled, &[1, 2]);
}

#[test]
fn structured_phi_does_not_define_a_neighboring_load_source_by_its_name() {
    let name = "structured_phi_load_source";
    let function = compile_function_to_clif(
        vec![structured_phi_transport_fixture(
            name,
            vec![
                OpIR {
                    var: Some("_bb1_arg0".into()),
                    out: Some("result".into()),
                    ..OpIR::new(OpKind::LoadVar)
                },
                OpIR {
                    args: Some(vec!["result".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
        )],
        name,
    );
    assert_return_parameter_sources(&function, &[0]);
}

#[test]
fn structured_phi_explicit_binding_updates_its_declared_destination() {
    let name = "structured_phi_explicit_binding";
    let function = compile_function_to_clif(
        vec![structured_phi_transport_fixture(
            name,
            vec![
                OpIR {
                    var: Some("lhs".into()),
                    args: Some(vec!["merged".into()]),
                    ..OpIR::new(OpKind::StoreVar)
                },
                OpIR {
                    value: Some(7),
                    ..OpIR::new(OpKind::Label)
                },
                OpIR {
                    var: Some("lhs".into()),
                    out: Some("result".into()),
                    ..OpIR::new(OpKind::LoadVar)
                },
                OpIR {
                    args: Some(vec!["result".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
        )],
        name,
    );
    assert_return_parameter_sources(&function, &[1, 2]);
}

#[test]
fn statement_output_metadata_never_defines_or_overwrites_a_value() {
    // Real deletion IR carries an `out` diagnostic name. Neither a fresh name
    // nor a collision with a live value may manufacture a result definition.
    // Exercise the complete native statement family through actual codegen.
    let cases: &[(OpKind, &[&str])] = &[
        (OpKind::SetAttr, &["receiver", "value"]),
        (OpKind::SetAttrName, &["receiver", "key", "value"]),
        (OpKind::SetAttrGenericPtr, &["receiver", "value"]),
        (OpKind::SetAttrGenericObj, &["receiver", "value"]),
        (OpKind::DelAttr, &["receiver"]),
        (OpKind::DelAttrName, &["receiver", "key"]),
        (OpKind::DelAttrGenericPtr, &["receiver"]),
        (OpKind::DelAttrGenericObj, &["receiver"]),
        (OpKind::ModuleCacheSet, &["key", "receiver"]),
        (OpKind::ModuleCacheDel, &["key"]),
        (OpKind::ModuleSetAttr, &["receiver", "key", "value"]),
        (OpKind::ModuleDelGlobal, &["receiver", "key"]),
        (OpKind::ModuleDelGlobalIfPresent, &["receiver", "key"]),
        (OpKind::Store, &["receiver", "value"]),
        (OpKind::GuardedFieldSet, &["receiver", "version", "value"]),
        (OpKind::StoreIndex, &["receiver", "key", "value"]),
        (OpKind::DelIndex, &["receiver", "key"]),
        (OpKind::Raise, &["receiver"]),
        (OpKind::IncRef, &["receiver"]),
        (OpKind::DecRef, &["receiver"]),
    ];
    for &(kind, args) in cases {
        assert!(crate::tir::op_kinds_generated::simpleir_out_field_is_metadata(kind));
        for output in [None, Some("fresh_metadata"), Some("keep"), Some("none")] {
            let statement = OpIR {
                args: Some(args.iter().map(|name| (*name).into()).collect()),
                out: output.map(str::to_owned),
                s_value: Some("field".into()),
                value: Some(0),
                class_definition: (kind == OpKind::GuardedFieldSet).then(|| "class:m:Owner".into()),
                source_op_idx: Some(7),
                ..OpIR::new(kind)
            };
            let mut ops = vec![OpIR {
                ..OpIR::new(OpKind::DropInserted)
            }];
            if kind == OpKind::DecRef {
                ops.push(OpIR {
                    args: Some(vec!["receiver".into()]),
                    ..OpIR::new(OpKind::IncRef)
                });
            }
            ops.push(statement);
            if kind == OpKind::IncRef {
                ops.push(OpIR {
                    args: Some(vec!["receiver".into()]),
                    ..OpIR::new(OpKind::DecRef)
                });
            }
            ops.push(OpIR {
                args: Some(vec!["keep".into()]),
                ..OpIR::new(OpKind::Ret)
            });
            let name = format!("metadata_{kind}");
            let function = compile_function_to_clif(
                vec![FunctionIR {
                    return_abi: molt_ir::FunctionReturnAbi::Value,
                    name: name.clone(),
                    params: ["keep", "receiver", "key", "value", "class", "version"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    ops,
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    codegen_partition: false,
                    parameter_custody: Vec::new(),
                    execution_context: Default::default(),
                }],
                &name,
            );
            assert_return_parameter_sources(&function, &[0]);
        }
    }
}
