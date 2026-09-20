use super::*;
use crate::runtime_import_abi::{
    MOLT_DEC_REF, MOLT_DEC_REF_OBJ, MOLT_TASK_NEW, NATIVE_RUNTIME_HELPER_IMPORTS,
};
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::InstructionData;

#[test]
fn iterator_lowering_preserves_materialized_values_and_unpack_consumers() {
    for unboxed in [false, true] {
        let mut ops = if unboxed {
            vec![OpIR {
                kind: "iter_next_unboxed".into(),
                args: Some(vec!["iterator".into()]),
                var: Some("next_value".into()),
                out: Some("done".into()),
                ..OpIR::default()
            }]
        } else {
            vec![
                OpIR {
                    kind: "iter_next".into(),
                    args: Some(vec!["iterator".into()]),
                    out: Some("pair".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "index".into(),
                    args: Some(vec!["pair".into(), "one".into()]),
                    out: Some("done".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("zero".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "index".into(),
                    args: Some(vec!["pair".into(), "zero".into()]),
                    out: Some("next_value".into()),
                    ..OpIR::default()
                },
            ]
        };
        ops.extend([
            OpIR {
                kind: "unpack_sequence".into(),
                args: Some(vec!["next_value".into(), "left".into(), "right".into()]),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec![if unboxed { "next_value" } else { "pair" }.into()]),
                ..OpIR::default()
            },
        ]);
        let compiled = compile_function_to_clif_with_imports(
            vec![FunctionIR {
                name: "observable_iterator_value".into(),
                params: vec!["iterator".into()],
                ops,
                ..FunctionIR::default()
            }],
            "observable_iterator_value",
        );
        let next = if unboxed {
            "molt_iter_next_unboxed"
        } else {
            "molt_iter_next"
        };
        for helper in [next, "molt_unpack_sequence"] {
            let id = compiled.import_ids.get(helper).unwrap_or_else(|| {
                panic!("missing {helper}: native lowering replaced an observable value")
            });
            assert_eq!(
                call_sites_for_import(&compiled.function, *id).len(),
                1,
                "{helper}"
            );
        }
        assert!(
            !compiled
                .import_ids
                .contains_key("molt_iter_next_dict_items"),
            "an unpack consumer cannot authorize replacing the observable iterator value with a key"
        );
    }
}
#[test]
fn direct_and_dynamic_calls_release_only_discarded_owned_results() {
    for (kind, argc, target, runtime_symbol, owns_result, returns_value) in [
        ("call", 1, Some("molt_classmethod_new"), None, true, true),
        (
            "call",
            1,
            Some("molt_function_closure_bits"),
            None,
            false,
            true,
        ),
        ("call", 1, Some("molt_is_truthy"), None, false, true),
        ("call", 0, Some("molt_print_newline"), None, false, false),
        ("call", 1, Some("owned_call_target"), None, true, true),
        (
            "call_internal",
            1,
            Some("owned_call_target"),
            None,
            true,
            true,
        ),
        ("call", 1, Some("external_owned_target"), None, true, true),
        (
            "call_internal",
            1,
            Some("external_owned_target"),
            None,
            true,
            true,
        ),
        (
            "call_internal",
            0,
            Some("external_void_target"),
            None,
            false,
            false,
        ),
        (
            "call_func",
            1,
            None,
            Some("molt_call_func_fast0"),
            true,
            true,
        ),
        (
            "call_func",
            5,
            None,
            Some("molt_call_func_dispatch"),
            true,
            true,
        ),
        ("call_bind", 2, None, Some("molt_call_bind_ic"), true, true),
        (
            "call_indirect",
            2,
            None,
            Some("molt_call_indirect_ic"),
            true,
            true,
        ),
        (
            "call_guarded",
            2,
            Some("owned_call_target"),
            Some("molt_call_bind_ic"),
            true,
            true,
        ),
        (
            "call_guarded",
            1,
            Some("owned_call_target"),
            Some("molt_call_bind_ic"),
            true,
            true,
        ),
        (
            "call_method",
            1,
            None,
            Some("molt_call_bind_ic"),
            true,
            true,
        ),
        (
            "call_method",
            1,
            Some("BoundMethod:str:upper"),
            Some("molt_fast_str_upper"),
            true,
            true,
        ),
        (
            "call_method_ic",
            1,
            Some("method"),
            Some("molt_call_method_ic0"),
            true,
            true,
        ),
        (
            "call_super_method_ic",
            2,
            Some("method"),
            Some("molt_call_super_method_ic0"),
            true,
            true,
        ),
        (
            "invoke_ffi",
            1,
            None,
            Some("molt_invoke_ffi_ic"),
            true,
            true,
        ),
        (
            "invoke_ffi",
            1,
            Some("molt.object_call_v1"),
            None,
            true,
            true,
        ),
        (
            "invoke_ffi",
            0,
            Some("molt.pyinit_module_v1"),
            None,
            false,
            true,
        ),
    ] {
        for bound in [false, true] {
            if target == Some("molt_print_newline") && bound {
                continue;
            }
            let native_abi = (kind == "invoke_ffi").then_some(target).flatten();
            let mut external_owned = FunctionIR {
                name: "external_owned_target".into(),
                params: vec!["arg".into()],
                ops: vec![OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["arg".into()]),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            };
            external_owned.externalize_with_signature().unwrap();
            let mut external_void = FunctionIR {
                name: "external_void_target".into(),
                ops: vec![OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            };
            external_void.externalize_with_signature().unwrap();
            let compiled = compile_function_to_clif_with_imports(
                vec![
                    FunctionIR {
                        name: "call_ownership_probe".into(),
                        params: vec!["value".into()],
                        ops: vec![
                            OpIR {
                                kind: kind.into(),
                                args: Some(vec!["value".into(); argc]),
                                out: bound.then(|| "result".into()),
                                s_value: target.map(str::to_string),
                                native_callable_export: native_abi.map(|_| "native.probe".into()),
                                native_callable_binding: native_abi.map(|_| "direct_symbol".into()),
                                native_callable_symbol: native_abi.map(|_| "native_probe".into()),
                                native_callable_abi: native_abi.map(str::to_string),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret".into(),
                                args: Some(vec![if bound { "result" } else { "value" }.into()]),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    },
                    FunctionIR {
                        name: "owned_call_target".into(),
                        params: vec!["arg".into()],
                        ops: vec![OpIR {
                            kind: "ret".into(),
                            args: Some(vec!["arg".into()]),
                            ..OpIR::default()
                        }],
                        ..FunctionIR::default()
                    },
                    external_owned,
                    external_void,
                ],
                "call_ownership_probe",
            );
            let function = &compiled.function;
            // Direct symbols are declared on the object module, while dynamic
            // helpers are registered imports. Exclude bookkeeping imports to
            // identify the sole direct target without relying on instruction order.
            let mut calls = Vec::new();
            for block in function.layout.blocks() {
                for inst in function.layout.block_insts(block) {
                    if kind == "call_func"
                        && matches!(
                            function.dfg.insts[inst],
                            InstructionData::CallIndirect { .. }
                        )
                    {
                        calls.push(inst);
                        continue;
                    }
                    let InstructionData::Call { func_ref, .. } = function.dfg.insts[inst] else {
                        continue;
                    };
                    let ExternalName::User(name) = function.dfg.ext_funcs[func_ref].name else {
                        continue;
                    };
                    let name = &function.params.user_named_funcs()[name];
                    let registered = compiled
                        .import_ids
                        .values()
                        .any(|id| name.namespace == 0 && name.index == id.as_u32());
                    let selected_runtime = runtime_symbol
                        .and_then(|symbol| compiled.import_ids.get(symbol))
                        .is_some_and(|id| name.namespace == 0 && name.index == id.as_u32());
                    if !registered || selected_runtime {
                        calls.push(inst);
                    }
                }
            }
            assert!(
                !calls.is_empty(),
                "{kind} {target:?}: {}",
                function.display()
            );
            let owners: Vec<_> = calls
                .iter()
                .flat_map(|&inst| function.dfg.inst_results(inst))
                .copied()
                .collect();
            assert_eq!(!owners.is_empty(), returns_value, "{kind} {target:?}");
            for symbol in ["molt_dec_ref_obj", "molt_inc_ref_obj"] {
                let matching = compiled.import_ids.get(symbol).map_or(0, |&import| {
                    call_sites_for_import(function, import)
                        .into_iter()
                        .filter(|(_, inst)| {
                            let args = function.dfg.inst_args(*inst);
                            args.len() == 1
                                && owners.iter().any(|owner| {
                                    canonical_value_sources(function, args[0]).contains(owner)
                                })
                        })
                        .count()
                });
                let expected = usize::from(symbol == "molt_dec_ref_obj" && owns_result && !bound);
                assert_eq!(
                    matching,
                    expected,
                    "{kind} {target:?}, bound={bound}, {symbol}: {}",
                    function.display()
                );
            }
        }
    }
}

#[test]
fn direct_calls_reject_runtime_void_outputs_and_internal_runtime_targets() {
    for (kind, target, argc) in [
        ("call", "molt_print_newline", 0),
        ("call_internal", "molt_classmethod_new", 1),
    ] {
        let outcome = std::panic::catch_unwind(|| {
            compile_function_to_clif_with_imports(
                vec![FunctionIR {
                    name: "invalid_call_probe".into(),
                    params: vec!["arg".into()],
                    ops: vec![
                        OpIR {
                            kind: kind.into(),
                            s_value: Some(target.into()),
                            args: Some(vec!["arg".into(); argc]),
                            out: Some("result".into()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".into(),
                            args: Some(vec!["result".into()]),
                            ..OpIR::default()
                        },
                    ],
                    ..FunctionIR::default()
                }],
                "invalid_call_probe",
            );
        });
        assert!(
            outcome.is_err(),
            "{kind} {target} must reject the invalid result ABI"
        );
    }
}

#[test]
fn callable_constructors_release_only_discarded_owned_results() {
    for (kind, argc) in [
        ("func_new", 0),
        ("func_new_closure", 1),
        ("builtin_func", 0),
        ("code_new", 9),
        ("callargs_new", 0),
        ("classmethod_new", 1),
        ("staticmethod_new", 1),
        ("property_new", 3),
        ("bound_method_new", 2),
        ("asyncgen_new", 1),
    ] {
        for bound in [false, true] {
            let mut constructor = OpIR {
                kind: kind.into(),
                args: Some(vec!["value".into(); argc]),
                out: bound.then(|| "created".into()),
                ..OpIR::default()
            };
            if matches!(kind, "func_new" | "func_new_closure" | "builtin_func") {
                constructor.s_value = Some("callable_result_target".into());
                constructor.value = Some(0);
            }
            let compiled = compile_function_to_clif_with_imports(
                vec![
                    FunctionIR {
                        name: "callable_result_probe".into(),
                        params: vec!["value".into()],
                        ops: vec![
                            constructor,
                            OpIR {
                                kind: "ret".into(),
                                args: Some(vec![if bound { "created" } else { "value" }.into()]),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    },
                    FunctionIR {
                        name: "callable_result_target".into(),
                        params: if kind == "func_new_closure" {
                            vec!["closure".into()]
                        } else {
                            vec![]
                        },
                        ops: vec![
                            OpIR {
                                kind: "const_none".into(),
                                out: Some("none_value".into()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret".into(),
                                args: Some(vec!["none_value".into()]),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    },
                ],
                "callable_result_probe",
            );
            let function = &compiled.function;
            let symbol = if kind == "builtin_func" {
                "molt_func_new_builtin".into()
            } else {
                format!("molt_{kind}")
            };
            let calls = call_sites_for_import(function, compiled.import_ids[symbol.as_str()]);
            assert_eq!(calls.len(), 1, "{kind}: {}", function.display());
            let owner = function.dfg.first_result(calls[0].1);
            let releases = compiled
                .import_ids
                .get("molt_dec_ref_obj")
                .map_or(0, |&import| {
                    call_sites_for_import(function, import)
                        .into_iter()
                        .filter(|(_, inst)| {
                            let args = function.dfg.inst_args(*inst);
                            args.len() == 1 && value_originates_only_from(function, args[0], owner)
                        })
                        .count()
                });
            assert_eq!(
                releases,
                usize::from(!bound),
                "{kind}, bound={bound}: {}",
                function.display()
            );
        }
    }
}

#[test]
fn closure_extraction_retains_only_bound_borrowed_results() {
    for bound in [false, true] {
        let compiled = compile_function_to_clif_with_imports(
            vec![FunctionIR {
                name: "closure_result_ownership".into(),
                params: vec!["callee".into()],
                ops: vec![
                    OpIR {
                        kind: "function_closure_bits".into(),
                        args: Some(vec!["callee".into()]),
                        out: bound.then(|| "closure".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: if bound { "ret" } else { "ret_void" }.into(),
                        args: bound.then(|| vec!["closure".into()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            }],
            "closure_result_ownership",
        );
        let function = &compiled.function;
        let extract = compiled.import_ids["molt_function_closure_bits"];
        let calls = call_sites_for_import(function, extract);
        assert_eq!(calls.len(), 1, "{}", function.display());
        let borrowed = function.dfg.first_result(calls[0].1);
        for (symbol, expected) in [
            ("molt_inc_ref_obj", usize::from(bound)),
            ("molt_dec_ref_obj", 0),
            ("molt_dec_ref", 0),
        ] {
            let matching = compiled.import_ids.get(symbol).map_or(0, |&import| {
                call_sites_for_import(function, import)
                    .into_iter()
                    .filter(|(_, inst)| {
                        let args = function.dfg.inst_args(*inst);
                        args.len() == 1 && value_originates_only_from(function, args[0], borrowed)
                    })
                    .count()
            });
            assert_eq!(
                matching,
                expected,
                "bound={bound}: {symbol} must respect the borrowed result: {}",
                function.display()
            );
        }
    }
}

#[test]
fn ordinary_task_initialization_is_guarded_before_payload_or_cancellation() {
    for (op_kind, task_kind) in [
        ("alloc_task", "generator"),
        ("alloc_task", "future"),
        ("alloc_task", "coroutine"),
        ("call_async", "future"),
    ] {
        let compiled = compile_function_to_clif_with_imports(
            vec![
                FunctionIR {
                    name: "task_allocation_probe".into(),
                    params: vec!["arg".into()],
                    ops: vec![
                        OpIR {
                            kind: op_kind.into(),
                            out: Some("task".into()),
                            args: Some(vec!["arg".into()]),
                            value: Some(64),
                            s_value: Some("task_allocation_poll".into()),
                            task_kind: (op_kind == "alloc_task").then(|| task_kind.into()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".into(),
                            args: Some(vec!["task".into()]),
                            ..OpIR::default()
                        },
                    ],
                    ..FunctionIR::default()
                },
                FunctionIR {
                    name: "task_allocation_poll".into(),
                    params: vec!["task".into()],
                    ops: vec![OpIR {
                        kind: "ret".into(),
                        args: Some(vec!["task".into()]),
                        ..OpIR::default()
                    }],
                    ..FunctionIR::default()
                },
            ],
            "task_allocation_probe",
        );
        let function = &compiled.function;
        let (_, success, _) = assert_task_allocation_admission(&compiled);
        let cfg = ControlFlowGraph::with_function(function);
        let dominators =
            cranelift_codegen::dominator_tree::DominatorTree::with_function(function, &cfg);
        let allocator =
            call_sites_for_import(function, compiled.import_ids[MOLT_TASK_NEW.name])[0].1;
        let mut stores = 0;
        for block in function.layout.blocks() {
            for inst in function.layout.block_insts(block) {
                let opcode = function.dfg.insts[inst].opcode();
                stores += usize::from(opcode.can_store());
                if inst != allocator
                    && (opcode.can_load() || opcode.can_store() || opcode.is_call())
                {
                    assert!(
                        dominators.dominates(success, inst, &function.layout),
                        "{op_kind}/{task_kind} payload, RC or cancellation bypasses allocation admission:\n{}",
                        function.display()
                    );
                }
            }
        }
        assert_eq!(stores, 1, "{}", function.display());
    }
}

#[test]
fn task_guard_carries_non_entry_owned_cleanup_to_its_merge() {
    for guarded_kind in ["alloc_task", "call_async"] {
        let guarded_is_alloc = guarded_kind == "alloc_task";
        let compiled = compile_function_to_clif_with_imports(
            vec![
                FunctionIR {
                    name: "task_guard_cleanup_probe".into(),
                    ops: vec![
                        OpIR {
                            kind: "jump".into(),
                            value: Some(1),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "label".into(),
                            value: Some(1),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "alloc_task".into(),
                            out: Some("prior_task".into()),
                            value: Some(i64::from(GENERATOR_CONTROL_BYTES)),
                            s_value: Some("task_guard_cleanup_poll".into()),
                            task_kind: Some("generator".into()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: guarded_kind.into(),
                            out: Some("result_task".into()),
                            args: Some(vec!["prior_task".into()]),
                            value: guarded_is_alloc
                                .then_some(i64::from(GENERATOR_CONTROL_BYTES) + 8),
                            s_value: Some("task_guard_cleanup_poll".into()),
                            task_kind: guarded_is_alloc.then(|| "generator".into()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".into(),
                            args: Some(vec!["result_task".into()]),
                            ..OpIR::default()
                        },
                    ],
                    ..FunctionIR::default()
                },
                FunctionIR {
                    name: "task_guard_cleanup_poll".into(),
                    params: vec!["task".into()],
                    ops: vec![OpIR {
                        kind: "ret".into(),
                        args: Some(vec!["task".into()]),
                        ..OpIR::default()
                    }],
                    ..FunctionIR::default()
                },
            ],
            "task_guard_cleanup_probe",
        );
        let function = &compiled.function;
        let task_new = compiled.import_ids[MOLT_TASK_NEW.name];
        let task_calls = call_sites_for_import(function, task_new);
        assert_eq!(
            task_calls.len(),
            2,
            "{guarded_kind} probe must allocate the prior and result tasks:\n{}",
            function.display()
        );
        let prior_task = function.dfg.first_result(task_calls[0].1);
        let dec_ref_obj = compiled.import_ids[MOLT_DEC_REF_OBJ.name];
        let releases: Vec<_> = call_sites_for_import(function, dec_ref_obj)
            .into_iter()
            .filter(|(_, inst)| {
                let args = function.dfg.inst_args(*inst);
                args.len() == 1 && value_originates_only_from(function, args[0], prior_task)
            })
            .collect();
        assert_eq!(
            releases.len(),
            1,
            "{guarded_kind} must release the exact prior-task owner once:\n{}",
            function.display()
        );
        let release_block = releases[0].0;
        let cfg = ControlFlowGraph::with_function(function);
        let predecessors: Vec<_> = cfg.pred_iter(release_block).collect();
        assert_eq!(
            predecessors.len(),
            2,
            "{guarded_kind} cleanup must execute in the merge reached by allocation failure and initialized success:\n{}",
            function.display()
        );
        assert!(
            predecessors
                .iter()
                .any(|edge| matches!(function.dfg.insts[edge.inst], InstructionData::Brif { .. }))
                && predecessors.iter().any(|edge| matches!(
                    function.dfg.insts[edge.inst],
                    InstructionData::Jump { .. }
                )),
            "{guarded_kind} cleanup merge must join the direct failure branch and initialized-success jump:\n{}",
            function.display()
        );
    }
}

#[test]
fn dynamic_br_if_releases_non_entry_owner_on_both_successors() {
    let compiled = compile_function_to_clif_with_imports(
        vec![
            FunctionIR {
                name: "dynamic_br_if_cleanup_probe".into(),
                params: vec!["condition".into()],
                ops: vec![
                    OpIR {
                        kind: "jump".into(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".into(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "alloc_task".into(),
                        out: Some("owned_task".into()),
                        value: Some(i64::from(GENERATOR_CONTROL_BYTES)),
                        s_value: Some("dynamic_br_if_cleanup_poll".into()),
                        task_kind: Some("generator".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "br_if".into(),
                        args: Some(vec!["condition".into()]),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".into(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".into(),
                        out: Some("none_value".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".into(),
                        args: Some(vec!["owned_task".into(), "none_value".into()]),
                        out: Some("task_is_none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
                param_types: Some(vec!["dyn".into()]),
                ..FunctionIR::default()
            },
            FunctionIR {
                name: "dynamic_br_if_cleanup_poll".into(),
                params: vec!["task".into()],
                ops: vec![OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["task".into()]),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            },
        ],
        "dynamic_br_if_cleanup_probe",
    );
    let function = &compiled.function;
    let task_new = compiled.import_ids[MOLT_TASK_NEW.name];
    let task_calls = call_sites_for_import(function, task_new);
    assert_eq!(task_calls.len(), 1, "{}", function.display());
    let owned_task = function.dfg.first_result(task_calls[0].1);
    let dec_ref_obj = compiled.import_ids[MOLT_DEC_REF_OBJ.name];
    let release_blocks: BTreeSet<_> = call_sites_for_import(function, dec_ref_obj)
        .into_iter()
        .filter(|(_, inst)| {
            let args = function.dfg.inst_args(*inst);
            args.len() == 1 && value_originates_only_from(function, args[0], owned_task)
        })
        .map(|(block, _)| block)
        .collect();
    assert_eq!(
        release_blocks.len(),
        2,
        "dynamic br_if must release the exact non-entry owner on both semantic successors:\n{}",
        function.display()
    );
    let branches_to_both_releases = function.layout.blocks().any(|block| {
        function.layout.block_insts(block).any(|inst| {
            let InstructionData::Brif { blocks, .. } = &function.dfg.insts[inst] else {
                return false;
            };
            BTreeSet::from([
                blocks[0].block(&function.dfg.value_lists),
                blocks[1].block(&function.dfg.value_lists),
            ]) == release_blocks
        })
    });
    assert!(
        branches_to_both_releases,
        "the semantic DynBox br_if must route directly to both exact-release blocks:\n{}",
        function.display()
    );
}

#[test]
fn native_compiles_canonical_bare_get_attr() {
    let func = FunctionIR {
        name: "bare_get_attr_repr".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "get_attr".to_string(),
                args: Some(vec!["self".to_string()]),
                s_value: Some("optional".to_string()),
                out: Some("v0".to_string()),
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
    // Must not panic at the dispatch's no-codegen catch-all; the canonical
    // `get_attr` lowers to the generic-by-name runtime attribute fetch.
    let clif = compile_function_to_clif_text(vec![func], "bare_get_attr_repr");
    assert!(
        clif.contains("call"),
        "canonical bare `get_attr` must lower to a runtime attribute-get call; \
         got CLIF:\n{clif}",
    );
}

#[test]
fn native_backend_preserves_semantic_frames_without_optional_tracing() {
    for setting in [None, Some("0"), Some("1")] {
        let bytes = compile_trace_probe_object(setting, crate::ir::ExecutionContextPolicy::Local);
        for symbol in [
            b"molt_trace_enter_slot".as_slice(),
            b"molt_trace_exit".as_slice(),
        ] {
            assert!(bytes.windows(symbol.len()).any(|window| window == symbol));
        }
    }
}

#[test]
fn static_calls_preserve_void_abi_and_callee_owned_frames() {
    let _guard = acquire_backend_env_lock();
    for setting in [None, Some("0"), Some("1")] {
        let _trace_env = ScopedEnvVar::set("MOLT_BACKEND_EMIT_TRACES", setting);
        let target = FunctionIR {
            name: "void_target".into(),
            execution_context: crate::ir::ExecutionContextPolicy::Local,
            ops: vec![
                OpIR {
                    kind: "trace_enter_slot".into(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".into(),
                    s_value: Some("molt_print_newline".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "trace_exit".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        let caller = FunctionIR {
            name: "molt_main".into(),
            ops: vec![
                OpIR {
                    kind: "call".into(),
                    s_value: Some(target.name.clone()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        let bytes = SimpleBackend::new()
            .compile(SimpleIR {
                functions: vec![caller, target],
                profile: None,
            })
            .bytes;
        for symbol in [
            "molt_print_newline",
            "molt_trace_enter_slot",
            "molt_trace_exit",
        ] {
            assert!(
                bytes
                    .windows(symbol.len())
                    .any(|window| window == symbol.as_bytes()),
                "missing {symbol}"
            );
        }
        let erased_dispatch = b"molt_guarded_call";
        assert!(
            !bytes
                .windows(erased_dispatch.len())
                .any(|window| window == erased_dispatch)
        );
    }
}

#[test]
fn static_calls_reject_argument_abi_mismatch_before_dispatch() {
    for kind in ["call", "call_internal"] {
        let failure = std::panic::catch_unwind(|| {
            compile_function_to_clif(
                vec![
                    FunctionIR {
                        name: "caller".into(),
                        ops: vec![
                            OpIR {
                                kind: kind.into(),
                                s_value: Some("callee".into()),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".into(),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    },
                    FunctionIR {
                        name: "callee".into(),
                        params: vec!["argument".into()],
                        ops: vec![OpIR {
                            kind: "ret_void".into(),
                            ..OpIR::default()
                        }],
                        ..FunctionIR::default()
                    },
                ],
                "caller",
            )
        })
        .expect_err("static call must reject incompatible argument count");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            message.contains("static call argument ABI mismatch for callee"),
            "{message}"
        );
    }
}

#[test]
fn static_calls_transport_closure_arguments_with_the_declared_abi() {
    for kind in ["call", "call_internal"] {
        for returns_value in [false, true] {
            let compiled = compile_function_to_clif_with_imports(
                vec![
                    FunctionIR {
                        name: "closure_caller".into(),
                        params: vec!["capture".into(), "argument".into()],
                        ops: vec![
                            OpIR {
                                kind: "func_new_closure".into(),
                                s_value: Some("closure_target".into()),
                                args: Some(vec!["capture".into()]),
                                out: Some("callable".into()),
                                value: Some(1),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: kind.into(),
                                s_value: Some("closure_target".into()),
                                args: Some(vec!["argument".into()]),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret".into(),
                                args: Some(vec!["callable".into()]),
                                ..OpIR::default()
                            },
                        ],
                        ..FunctionIR::default()
                    },
                    FunctionIR {
                        name: "closure_target".into(),
                        params: vec!["environment".into(), "value".into()],
                        ops: vec![OpIR {
                            kind: if returns_value { "ret" } else { "ret_void" }.into(),
                            args: returns_value.then(|| vec!["value".into()]),
                            ..OpIR::default()
                        }],
                        ..FunctionIR::default()
                    },
                ],
                "closure_caller",
            );
            assert!(!compiled.import_ids.contains_key("molt_guarded_call"));
            let extract = compiled.import_ids["molt_function_closure_bits"];
            let extracted = call_sites_for_import(&compiled.function, extract);
            assert_eq!(extracted.len(), 1);
            let environment = compiled.function.dfg.inst_results(extracted[0].1)[0];
            let calls: Vec<_> = compiled
                .function
                .layout
                .blocks()
                .flat_map(|block| compiled.function.layout.block_insts(block))
                .filter(|&inst| {
                    matches!(
                        compiled.function.dfg.insts[inst],
                        InstructionData::Call { .. }
                    ) && compiled.function.dfg.inst_args(inst).first() == Some(&environment)
                })
                .collect();
            assert_eq!(calls.len(), 1, "{}", compiled.function.display());
            assert_eq!(compiled.function.dfg.inst_args(calls[0]).len(), 2);
            assert_eq!(
                compiled.function.dfg.inst_results(calls[0]).len(),
                usize::from(returns_value)
            );
        }
    }
}

#[test]
fn native_backend_does_not_mint_frames_for_frameless_or_inherited_bodies() {
    for policy in [
        crate::ir::ExecutionContextPolicy::None,
        crate::ir::ExecutionContextPolicy::Inherited,
    ] {
        let bytes = compile_trace_probe_object(None, policy);
        for symbol in [
            b"molt_trace_enter_slot".as_slice(),
            b"molt_trace_exit".as_slice(),
        ] {
            assert!(!bytes.windows(symbol.len()).any(|window| window == symbol));
        }
    }
}

#[test]
fn native_backend_import_ids_are_cached_by_symbol() {
    let mut backend = SimpleBackend::new();

    let first = SimpleBackend::import_runtime_func_id_split(
        &mut backend.module,
        &mut backend.import_ids,
        MOLT_DEC_REF,
    );
    let second = SimpleBackend::import_runtime_func_id_split(
        &mut backend.module,
        &mut backend.import_ids,
        MOLT_DEC_REF,
    );

    assert_eq!(first, second);
    assert_eq!(backend.import_ids.len(), 1);
}

#[test]
fn marked_finally_observer_imports_only_the_fused_runtime_projection() {
    use cranelift_object::object::{Object, ObjectSymbol};

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "marked_finally_observer".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "exception_finally_pending_observer".to_string(),
                    out: Some("pending".to_string()),
                    async_work_poll: true,
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["pending".to_string()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);
    let object = cranelift_object::object::File::parse(&*output.bytes)
        .expect("parse marked-observer object");
    let undefined: BTreeSet<String> = object
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
        .collect();

    assert!(
        undefined.contains("molt_async_work_poll_and_exception_last_pending"),
        "marked observer must import the fused pending-call/exception primitive: {undefined:?}"
    );
    assert!(
        !undefined.contains("molt_exception_last_pending"),
        "marked observer must not retain the unfused exception-only import: {undefined:?}"
    );
}

#[test]
fn native_runtime_helper_import_descriptors_are_unique() {
    let names: BTreeSet<&str> = NATIVE_RUNTIME_HELPER_IMPORTS
        .iter()
        .map(|signature| signature.name)
        .collect();

    assert_eq!(names.len(), NATIVE_RUNTIME_HELPER_IMPORTS.len());
    assert!(names.contains("molt_inc_ref_obj"));
    assert!(names.contains("molt_dec_ref_obj"));
    assert!(names.contains("molt_task_new"));
    assert!(names.contains("molt_cancel_token_get_current"));
    assert!(names.contains("molt_task_register_token_owned"));
    assert!(names.contains("molt_asyncgen_new"));
}

#[test]
fn native_backend_skips_profile_store_imports_when_function_has_no_store_ops() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(
        !output
            .bytes
            .windows(b"molt_profile_struct_field_store".len())
            .any(|window| window == b"molt_profile_struct_field_store")
    );
    assert!(
        !output
            .bytes
            .windows(b"molt_profile_enabled".len())
            .any(|window| window == b"molt_profile_enabled")
    );
}

#[test]
fn native_backend_keeps_profile_store_imports_when_function_has_store_ops() {
    let _guard = acquire_backend_env_lock();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("obj".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("value".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "value".to_string()]),
                    value: Some(8),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(
        output
            .bytes
            .windows(b"molt_profile_struct_field_store".len())
            .any(|window| window == b"molt_profile_struct_field_store")
    );
    assert!(
        output
            .bytes
            .windows(b"molt_profile_enabled".len())
            .any(|window| window == b"molt_profile_enabled")
    );
}

fn compile_check_exception_target_shape(name: &str, target: Option<i64>) {
    compile_function_to_clif_text(
        vec![FunctionIR {
            name: name.to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("sentinel".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: target,
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
        name,
    );
}

#[test]
#[should_panic(
    expected = "check_exception missing target label id in function `native_check_exception_missing_target` op 1"
)]
fn check_exception_missing_target_fails_closed_at_codegen() {
    compile_check_exception_target_shape("native_check_exception_missing_target", None);
}

#[test]
#[should_panic(
    expected = "check_exception target label 7 is not present in native label map for function `native_check_exception_orphan_target` op 1"
)]
fn check_exception_orphan_target_fails_closed_at_codegen() {
    compile_check_exception_target_shape("native_check_exception_orphan_target", Some(7));
}

#[test]
fn native_backend_compiles_exception_label_guard_if_without_else() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hello_regress____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "exception_stack_enter".to_string(),
                    out: Some("v74".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("v75".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v76".to_string()),
                    s_value: Some("hello_regress".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    out: Some("v77".to_string()),
                    args: Some(vec!["v76".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v78".to_string()),
                    s_value: Some("__dict__".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    out: Some("v79".to_string()),
                    args: Some(vec!["v77".to_string(), "v78".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v79".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["v75".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".to_string(),
                    args: Some(vec!["v74".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("v80".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v81".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    out: Some("v82".to_string()),
                    args: Some(vec!["v80".to_string(), "v81".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".to_string(),
                    out: Some("v83".to_string()),
                    args: Some(vec!["v82".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["v83".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["v80".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v84".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
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
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn native_backend_compiles_tir_roundtripped_exception_label_guard_if_without_else() {
    let func = FunctionIR {
        name: "hello_regress____molt_globals_builtin__".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "exception_stack_enter".to_string(),
                out: Some("v74".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_depth".to_string(),
                out: Some("v75".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("v76".to_string()),
                s_value: Some("hello_regress".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".to_string(),
                out: Some("v77".to_string()),
                args: Some(vec!["v76".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("v78".to_string()),
                s_value: Some("__dict__".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".to_string(),
                out: Some("v79".to_string()),
                args: Some(vec!["v77".to_string(), "v78".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["v79".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_set_depth".to_string(),
                args: Some(vec!["v75".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_exit".to_string(),
                args: Some(vec!["v74".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("v80".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("v81".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "is".to_string(),
                out: Some("v82".to_string()),
                args: Some(vec!["v80".to_string(), "v81".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "not".to_string(),
                out: Some("v83".to_string()),
                args: Some(vec!["v82".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".to_string(),
                args: Some(vec!["v83".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "raise".to_string(),
                args: Some(vec!["v80".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("v84".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["v84".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
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

    let roundtripped = roundtrip_function_through_tir(&func);
    let clif = compile_function_to_clif_text(
        vec![roundtripped],
        "hello_regress____molt_globals_builtin__",
    );

    assert!(
        clif.contains("return"),
        "TIR-roundtripped exception function must compile to CLIF:\n{clif}"
    );
}

#[cfg(feature = "llvm")]
#[test]
fn native_backend_compiles_tir_roundtripped_nested_loops() {
    let func = FunctionIR {
        name: "nested_loops".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("total".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(2),
                out: Some("outer_limit".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(2),
                out: Some("inner_limit".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("one".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "lt".into(),
                args: Some(vec!["i".into(), "outer_limit".into()]),
                out: Some("outer_cond".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_false".into(),
                args: Some(vec!["outer_cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("j".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "lt".into(),
                args: Some(vec!["j".into(), "inner_limit".into()]),
                out: Some("inner_cond".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_false".into(),
                args: Some(vec!["inner_cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["total".into(), "j".into()]),
                out: Some("total".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["j".into(), "one".into()]),
                out: Some("j".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["i".into(), "one".into()]),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["total".into()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let roundtripped = roundtrip_function_through_tir(&func);
    let clif = compile_function_to_clif_text(vec![roundtripped], "nested_loops");

    assert!(
        clif.contains("return"),
        "TIR-roundtripped nested-loop function must compile to CLIF:\n{clif}"
    );
}
