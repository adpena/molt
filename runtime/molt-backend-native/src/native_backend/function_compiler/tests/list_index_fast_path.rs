use super::super::fc::list_index_fast_path::{ListIndexFastPathState, ListStorageField};
use super::super::scalar_carriers::ConditionalListBoolShadow;
use super::*;

fn with_list_storage_state(
    check: impl FnOnce(
        &mut FunctionBuilder<'_>,
        &mut ListIndexFastPathState,
        &NativeCleanupRoots,
        cranelift_codegen::ir::FuncRef,
    ),
) {
    let input = super::cleanup_roots::token_test_ir();
    let analysis = preanalyze_for_test(&input);
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params
        .extend([AbiParam::new(types::I8), AbiParam::new(types::I64)]);
    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let mut roots = NativeCleanupRoots::new(
            &mut builder,
            &input,
            &analysis.alias_roots,
            &ScalarRepresentationPlan::default(),
            NativeRcAuthority::NativeValueTracking,
        );
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        roots.initialize(&mut builder);
        let release = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut BTreeMap::new(),
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        let mut state = ListIndexFastPathState::new(&roots);
        check(&mut builder, &mut state, &roots, release);
        builder.ins().return_(&[]);
        builder.finalize();
    }
    verify_function(&function, &settings::Flags::new(settings::builder()))
        .unwrap_or_else(|errors| panic!("{errors}\n{}", function.display()));
}

fn cache_list_storage_field(
    builder: &mut FunctionBuilder<'_>,
    state: &mut ListIndexFastPathState,
    field: ListStorageField,
    name: &str,
) -> cranelift_frontend::Variable {
    let ty = if field == ListStorageField::IsBool {
        types::I8
    } else {
        types::I64
    };
    let value = builder.ins().iconst(ty, 1);
    let variable = builder.declare_var(ty);
    builder.def_var(variable, value);
    state.insert(field, name.into(), variable, builder);
    variable
}

#[test]
fn list_storage_observations_do_not_cross_sibling_blocks_or_merges() {
    with_list_storage_state(|builder, state, _, _| {
        let fields = [
            ListStorageField::IntData,
            ListStorageField::IntLen,
            ListStorageField::Data,
            ListStorageField::Len,
            ListStorageField::IsBool,
        ];
        let left = builder.create_block();
        let right = builder.create_block();
        let merge = builder.create_block();
        let condition = builder.block_params(builder.current_block().unwrap())[0];
        builder.ins().brif(condition, left, &[], right, &[]);
        builder.switch_to_block(left);
        builder.seal_block(left);
        for field in fields {
            let variable = cache_list_storage_field(builder, state, field, "lst");
            assert_eq!(state.get(field, "lst", builder), Some(variable));
        }
        builder.ins().jump(merge, &[]);
        builder.switch_to_block(right);
        builder.seal_block(right);
        for field in fields {
            assert!(
                state.get(field, "lst", builder).is_none(),
                "sibling definition cannot dominate"
            );
            cache_list_storage_field(builder, state, field, "lst");
        }
        builder.ins().jump(merge, &[]);
        builder.switch_to_block(merge);
        builder.seal_block(merge);
        for field in fields {
            assert!(
                state.get(field, "lst", builder).is_none(),
                "one predecessor is not a merge fact"
            );
        }
    });
}

#[test]
fn list_storage_effect_fence_covers_alias_mutation_zero_arg_calls_and_deletion() {
    for op in [
        OpIR {
            kind: "list_append".into(),
            args: Some(vec!["alias".into(), "item".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "call".into(),
            args: Some(vec![]),
            ..OpIR::default()
        },
        OpIR {
            kind: "del_index".into(),
            args: Some(vec!["alias".into(), "idx".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_index".into(),
            args: Some(vec!["alias".into(), "idx".into(), "item".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            async_work_poll: true,
            ..OpIR::default()
        },
    ] {
        with_list_storage_state(|builder, state, _, _| {
            let plan = ScalarRepresentationPlan::default();
            let variable = cache_list_storage_field(builder, state, ListStorageField::Data, "lst");
            let alias = OpIR {
                kind: "binding_alias".into(),
                args: Some(vec!["lst".into()]),
                out: Some("alias".into()),
                ..OpIR::default()
            };
            state.begin_op(0, &alias, &plan);
            assert_eq!(
                state.get(ListStorageField::Data, "lst", builder),
                Some(variable)
            );
            state.begin_op(1, &op, &plan);
            assert!(
                state.get(ListStorageField::Data, "lst", builder).is_none(),
                "{} fences the whole heap",
                op.kind
            );
        });
    }
}

#[test]
fn list_storage_definition_fence_uses_canonical_binding_roles() {
    with_list_storage_state(|builder, state, _, _| {
        let plan = ScalarRepresentationPlan::default();
        let variable = cache_list_storage_field(builder, state, ListStorageField::Data, "lst");
        let read = OpIR {
            kind: "copy_var".into(),
            var: Some("lst".into()),
            out: Some("alias".into()),
            ..OpIR::default()
        };
        state.begin_op(0, &read, &plan);
        assert_eq!(
            state.get(ListStorageField::Data, "lst", builder),
            Some(variable),
            "var is a read here"
        );
        let rebind = OpIR {
            kind: "binding_alias".into(),
            args: Some(vec!["other".into()]),
            out: Some("lst".into()),
            ..OpIR::default()
        };
        state.begin_op(1, &rebind, &plan);
        assert!(state.get(ListStorageField::Data, "lst", builder).is_none());
    });
}

#[test]
fn list_storage_certified_loop_scope_is_published_only_by_its_actual_preheader() {
    with_list_storage_state(|builder, state, roots, _| {
        let ops = typed_list_hoist_fixture();
        let plan = representation_plan_for_ops(&ops);
        let pre = collect_pre_loop_defined_names(&ops, 3);
        let entry = builder.current_block().unwrap();
        cache_list_storage_field(builder, state, ListStorageField::IntData, "lst");
        let (hoisted, _) =
            super::scan_loop_hoistable_lists(&ops, 3, &pre, &plan, state, entry, Some(roots));
        assert!(hoisted.contains("lst"), "safe loop hoisting remains active");
        assert!(
            state
                .get(ListStorageField::IntData, "lst", builder)
                .is_none(),
            "ordinary cache cannot be promoted"
        );
        let data = cache_list_storage_field(builder, state, ListStorageField::IntData, "lst");
        let body = builder.create_block();
        builder.ins().jump(body, &[]);
        builder.switch_to_block(body);
        builder.seal_block(body);
        // Still in the header's opcode, but already in an internal lowering
        // block: a new field must not inherit preheader publication privileges.
        cache_list_storage_field(builder, state, ListStorageField::IntLen, "lst");
        let next_body = builder.create_block();
        builder.ins().jump(next_body, &[]);
        builder.switch_to_block(next_body);
        builder.seal_block(next_body);
        state.begin_op(4, &ops[4], &plan);
        assert_eq!(
            state.get(ListStorageField::IntData, "lst", builder),
            Some(data)
        );
        assert!(
            state
                .get(ListStorageField::IntLen, "lst", builder)
                .is_none()
        );
        state.begin_op(6, &ops[6], &plan);
        assert_eq!(
            state.get(ListStorageField::IntData, "lst", builder),
            Some(data)
        );
        state.begin_op(7, &ops[1], &plan);
        assert!(
            state
                .get(ListStorageField::IntData, "lst", builder)
                .is_none(),
            "completed loop cannot serve its sibling"
        );
    });
}

#[test]
fn list_storage_loop_certification_uses_actual_native_owner_custody() {
    let input = FunctionIR {
        name: "generic_loop_ownership".into(),
        params: vec!["lst".into(), "idx".into()],
        param_types: Some(vec!["list".into(), "int".into()]),
        ops: vec![
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".into(),
                args: Some(vec!["lst".into(), "idx".into()]),
                out: Some("element".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
        ],
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let plan = native_representation_plan_for_test(&input);
    let analysis = preanalyze_for_test(&input);
    let pre = input.params.iter().cloned().collect();
    assert!(
        scan_loop_hoistable_lists(&input.ops, 0, &pre, &plan)
            .1
            .is_empty(),
        "missing cleanup custody cannot certify an unknown heap result"
    );
    for authority in [
        NativeRcAuthority::NativeValueTracking,
        NativeRcAuthority::TirDropInsertion,
    ] {
        let mut function = Function::new();
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let roots = NativeCleanupRoots::new(
            &mut builder,
            &input,
            &analysis.alias_roots,
            &plan,
            authority,
        );
        let mut state = ListIndexFastPathState::new(&roots);
        let preheader = builder.create_block();
        let (_, generic) = super::scan_loop_hoistable_lists(
            &input.ops,
            0,
            &pre,
            &plan,
            &mut state,
            preheader,
            Some(&roots),
        );
        assert_eq!(
            generic.contains("lst"),
            !roots.contains("element"),
            "native replacement can finalize a prior element; TIR releases remain explicit effects"
        );
    }
}

#[test]
fn list_storage_consumption_observes_same_op_owner_release_and_replacement() {
    for action in ["release", "replace", "release_all"] {
        with_list_storage_state(|builder, state, roots, release| {
            let owner = builder.block_params(builder.current_block().unwrap())[1];
            let variable = cache_list_storage_field(builder, state, ListStorageField::Data, "lst");
            roots.acquire(builder, release, "owner", owner);
            assert_eq!(
                state.get(ListStorageField::Data, "lst", builder),
                Some(variable),
                "first acquisition releases no owner"
            );
            match action {
                "release" => roots.release(builder, release, "alias"),
                "replace" => roots.acquire(builder, release, "owner", owner),
                "release_all" => roots.release_all(builder, release),
                _ => unreachable!(),
            }
            assert!(
                state.get(ListStorageField::Data, "lst", builder).is_none(),
                "{action} must fence before the next op begins"
            );
            roots.release_all(builder, release);
            let variable = cache_list_storage_field(builder, state, ListStorageField::Data, "lst");
            roots.release_all(builder, release);
            assert_eq!(
                state.get(ListStorageField::Data, "lst", builder),
                Some(variable),
                "empty tokens emit no finalizer call"
            );
        });
    }
}

#[test]
fn list_bool_shadow_snapshots_layout_but_remains_block_local() {
    with_list_storage_state(|builder, state, _, _| {
        let is_bool = builder.ins().iconst(types::I8, 1);
        let payload = builder.ins().iconst(types::I64, 0);
        state.insert_bool_shadow(
            "item".into(),
            ConditionalListBoolShadow { is_bool, payload },
            builder,
        );
        cache_list_storage_field(builder, state, ListStorageField::IsBool, "lst");
        state.begin_op(
            1,
            &OpIR {
                kind: "list_append".into(),
                args: Some(vec!["lst".into(), "other".into()]),
                ..OpIR::default()
            },
            &ScalarRepresentationPlan::default(),
        );
        assert!(
            state
                .get(ListStorageField::IsBool, "lst", builder)
                .is_none()
        );
        let shadow = state
            .bool_shadow("item", builder)
            .expect("the old element retains its own layout snapshot");
        assert_eq!(shadow.is_bool, is_bool);
        assert_eq!(shadow.payload, payload);
        let next = builder.create_block();
        builder.ins().jump(next, &[]);
        builder.switch_to_block(next);
        builder.seal_block(next);
        assert!(
            state.bool_shadow("item", builder).is_none(),
            "shadow Values cannot escape their defining block"
        );
    });
}

#[test]
fn native_sibling_loop_list_storage_compiles_through_both_loop_producers() {
    use crate::native_backend::simple_backend::tests::{
        compile_selected_functions_direct, emit_direct_object,
    };
    let mut functions = Vec::new();
    for (name, indexed, generic) in [
        ("list_cache_sibling_plain", false, false),
        ("list_cache_sibling_indexed", true, false),
        ("list_cache_sibling_generic", false, true),
    ] {
        let mut ops = vec![
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("zero".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("one".into()),
                ..OpIR::default()
            },
        ];
        if !generic {
            ops.push(OpIR {
                kind: "list_int_new".into(),
                args: Some(vec!["one".into(), "zero".into()]),
                out: Some("lst".into()),
                ..OpIR::default()
            });
        }
        ops.push(OpIR {
            kind: "if".into(),
            args: Some(vec!["condition".into()]),
            ..OpIR::default()
        });
        for branch in ["left", "right"] {
            if branch == "right" {
                ops.push(OpIR {
                    kind: "else".into(),
                    ..OpIR::default()
                });
            }
            ops.push(OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            });
            let index = if indexed {
                let index = format!("{branch}_idx");
                ops.push(OpIR {
                    kind: "loop_index_start".into(),
                    args: Some(vec!["zero".into()]),
                    out: Some(index.clone()),
                    ..OpIR::default()
                });
                index
            } else {
                "zero".into()
            };
            ops.push(OpIR {
                kind: "index".into(),
                args: Some(vec!["lst".into(), index]),
                out: Some(format!("{branch}_element")),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "loop_break".into(),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            });
        }
        ops.push(OpIR {
            kind: "end_if".into(),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "ret".into(),
            args: Some(vec!["zero".into()]),
            ..OpIR::default()
        });
        functions.push(FunctionIR {
            name: name.into(),
            params: if generic {
                vec!["condition".into(), "lst".into()]
            } else {
                vec!["condition".into()]
            },
            param_types: Some(if generic {
                vec!["bool".into(), "list".into()]
            } else {
                vec!["bool".into()]
            }),
            ops,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        });
    }
    let names = [
        "list_cache_sibling_plain",
        "list_cache_sibling_indexed",
        "list_cache_sibling_generic",
    ];
    let backend = compile_selected_functions_direct(functions, &names);
    for name in names {
        let function = &backend
            .deferred_defines
            .iter()
            .find(|deferred| deferred.name == name)
            .expect("compiled sibling-loop function")
            .func;
        verify_function(function, &settings::Flags::new(settings::builder()))
            .unwrap_or_else(|errors| panic!("{name}: {errors}\n{}", function.display()));
    }
    let symbols = native_object_symbols(&emit_direct_object(backend));
    for name in names {
        assert!(
            symbols.defined.contains(name),
            "the real loop callers must emit {name}"
        );
    }
}

fn scan_loop_hoistable_lists(
    ops: &[OpIR],
    start_idx: usize,
    pre_loop_defined: &BTreeSet<String>,
    plan: &ScalarRepresentationPlan,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut function = Function::new();
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut context);
    let preheader = builder.create_block();
    super::scan_loop_hoistable_lists(
        ops,
        start_idx,
        pre_loop_defined,
        plan,
        &mut Default::default(),
        preheader,
        None,
    )
}

#[test]
fn sum_reduction_detects_canonical_pattern() {
    // Simulates the IR for:
    //   total = 0
    //   for x in list_of_ints:
    //       total += x
    let mut ops = vec![
        list_int_new("my_list"),
        // 0: loop_start
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        // 1: loop_index_start  (idx)
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("idx".to_string()),
            args: Some(vec!["start_val".to_string()]),
            ..OpIR::default()
        },
        // 2: index  list[idx]  -> elem
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["my_list".to_string(), "idx".to_string()]),
            out: Some("elem".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // 3: add  [total, elem]  -> sum_result
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["total".to_string(), "elem".to_string()]),
            out: Some("sum_result".to_string()),
            ..OpIR::default()
        },
        // 4: store_var  total = sum_result
        OpIR {
            kind: "store_var".to_string(),
            var: Some("total".to_string()),
            args: Some(vec!["sum_result".to_string()]),
            ..OpIR::default()
        },
        // 5: loop_index_next
        OpIR {
            kind: "loop_index_next".to_string(),
            args: Some(vec!["next_idx".to_string()]),
            out: Some("idx_next".to_string()),
            ..OpIR::default()
        },
        // 6: loop_continue
        OpIR {
            kind: "loop_continue".to_string(),
            ..OpIR::default()
        },
        // 7: loop_end
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];

    let plan = representation_plan_for_ops(&ops);
    let result = scan_loop_int_sum_reduction(&ops, 2, "idx", &plan);
    assert!(result.is_some(), "canonical sum reduction must be detected");
    let candidate = result.unwrap();
    assert_eq!(candidate.list_name, "my_list");
    assert_eq!(candidate.acc_store_slot, "total");
    assert_eq!(candidate.add_out_name, "sum_result");
    assert_eq!(candidate.acc_operand_name, "total");
    assert_eq!(candidate.loop_end_idx, 8);
    // A result-carrying store cannot be erased by the reduction rewrite. The
    // sentinel and binding-only out shapes still describe the same one slot.
    ops[5].out = Some("snapshot".into());
    assert!(scan_loop_int_sum_reduction(&ops, 2, "idx", &plan).is_none());
    ops[5].out = Some("none".into());
    assert!(scan_loop_int_sum_reduction(&ops, 2, "idx", &plan).is_some());
    ops[5].var = None;
    ops[5].out = Some("total".into());
    assert!(scan_loop_int_sum_reduction(&ops, 2, "idx", &plan).is_some());
}

#[test]
fn sum_reduction_detects_reversed_add_operands() {
    // add [elem, total] instead of [total, elem]
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "inplace_add".to_string(),
            args: Some(vec!["e".to_string(), "acc".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];

    let plan = representation_plan_for_ops(&ops);
    let result = scan_loop_int_sum_reduction(&ops, 1, "i", &plan);
    assert!(
        result.is_some(),
        "reversed operand sum reduction must be detected"
    );
    let c = result.unwrap();
    assert_eq!(c.acc_operand_name, "acc");
    assert_eq!(c.list_name, "lst");
}

#[test]
fn sum_reduction_rejects_non_bce_safe() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: None, // NOT bce_safe
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "non-bce_safe index must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_call_in_body() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // Side-effecting call in loop body â€” disqualifies
        OpIR {
            kind: "call".to_string(),
            args: Some(vec!["e".to_string()]),
            out: Some("result".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "result".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "call in loop body must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_nested_loop() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // Nested loop
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "nested loop must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_wrong_index_var() {
    // Index uses a different variable than the loop induction variable
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "other_var".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "index with non-induction variable must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_non_list_int() {
    let ops = vec![
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            container_type: Some("list".to_string()), // generic list, not list_int
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 0, "i", &plan).is_none(),
        "non-list_int container must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_multiple_stores() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("other".to_string()),
            args: Some(vec!["e".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "multiple store_var ops must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_add_elem_mismatch() {
    // add operands don't include the index element
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "other_val".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "add operand mismatch must disqualify"
    );
}

// â”€â”€ scalar_slot_exclusion_unsafe tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[test]
fn scan_loop_hoistable_lists_treats_store_index_as_mutation() {
    let flat_ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_index".to_string(),
            args: Some(vec![
                "lst".to_string(),
                "idx".to_string(),
                "val".to_string(),
            ]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let flat_plan = representation_plan_for_ops(&flat_ops);
    let flat_pre_loop_defined = collect_pre_loop_defined_names(&flat_ops, 1);
    let (flat_hoist, generic_hoist) =
        scan_loop_hoistable_lists(&flat_ops, 1, &flat_pre_loop_defined, &flat_plan);
    assert!(
        !flat_hoist.contains("lst"),
        "store_index must invalidate flat-list hoisting"
    );
    assert!(
        !generic_hoist.contains("lst"),
        "store_index must not leak through the generic hoist set"
    );

    let generic_ops = vec![
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
            container_type: Some("list".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_index".to_string(),
            args: Some(vec![
                "lst".to_string(),
                "idx".to_string(),
                "val".to_string(),
            ]),
            container_type: Some("list".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let generic_plan = representation_plan_for_typed_ops(
        &["lst", "idx", "val"],
        Some(vec!["list", "int", "int"]),
        &generic_ops,
    );
    let generic_pre_loop_defined = BTreeSet::from(["lst".to_string()]);
    let (flat_hoist, generic_hoist) =
        scan_loop_hoistable_lists(&generic_ops, 0, &generic_pre_loop_defined, &generic_plan);
    assert!(
        !flat_hoist.contains("lst"),
        "generic store_index must not enter the flat hoist set"
    );
    assert!(
        !generic_hoist.contains("lst"),
        "store_index must invalidate generic-list hoisting"
    );
}

#[test]
fn scan_loop_hoistable_lists_treats_call_and_alias_escape_as_mutation() {
    // A list handed to an opaque call inside the loop can be mutated/reallocated by
    // the callee (e.g. list.append), leaving a hoisted data_ptr/len stale — a silent
    // wrong answer or use-after-free. It must NOT be hoistable.
    let call_ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "call".to_string(),
            s_value: Some("opaque_mutator".to_string()),
            args: Some(vec!["lst".to_string()]),
            out: Some("ignored".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&call_ops);
    let pre = collect_pre_loop_defined_names(&call_ops, 1);
    let (flat_hoist, _generic) = scan_loop_hoistable_lists(&call_ops, 1, &pre, &plan);
    assert!(
        !flat_hoist.contains("lst"),
        "a list passed to an opaque call must not be hoistable (callee may realloc it)"
    );

    // Aliasing: `alias = copy_var(lst); alias.append(cur)` mutates the shared
    // buffer, so `lst` (indexed hoist candidate) must be non-hoistable too.
    let alias_ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "copy_var".to_string(),
            args: Some(vec!["lst".to_string()]),
            out: Some("alias".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "list_append".to_string(),
            args: Some(vec!["alias".to_string(), "cur".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&alias_ops);
    let pre = collect_pre_loop_defined_names(&alias_ops, 1);
    let (flat_hoist, _generic) = scan_loop_hoistable_lists(&alias_ops, 1, &pre, &plan);
    assert!(
        !flat_hoist.contains("lst"),
        "mutation of an alias must invalidate hoisting of the original list buffer"
    );

    // Positive control: a list only READ (index) in the loop stays hoistable — the
    // escape checks must not over-disable the common case.
    let read_ops = typed_list_hoist_fixture();
    let plan = representation_plan_for_ops(&read_ops);
    let pre = collect_pre_loop_defined_names(&read_ops, 3);
    let (flat_hoist, _generic) = scan_loop_hoistable_lists(&read_ops, 3, &pre, &plan);
    assert!(
        flat_hoist.contains("lst"),
        "a read-only indexed list must remain hoistable (no false escape)"
    );
}

fn typed_list_hoist_fixture() -> Vec<OpIR> {
    vec![
        list_int_new("lst"),
        OpIR {
            kind: "const".into(),
            value: Some(0),
            out: Some("idx".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".into(),
            value: Some(1),
            out: Some("total".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_start".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".into(),
            args: Some(vec!["lst".into(), "idx".into()]),
            out: Some("cur".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".into(),
            args: Some(vec!["total".into(), "idx".into()]),
            out: Some("total_next".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".into(),
            ..OpIR::default()
        },
    ]
}

#[test]
fn list_hoisting_fences_mutation_through_aliases_before_and_inside_the_loop() {
    let mut aliases = vec![];
    for kind in ["copy_var", "load_var"] {
        for args in [None, Some(vec![]), Some(vec!["lst".to_string()])] {
            aliases.push(OpIR {
                kind: kind.into(),
                args,
                var: Some("lst".into()),
                out: Some("alias".into()),
                ..OpIR::default()
            });
        }
    }
    for kind in [
        "copy",
        "identity_alias",
        "binding_alias",
        "borrow",
        "box",
        "unbox",
        "and",
        "or",
    ] {
        aliases.push(OpIR {
            kind: kind.into(),
            args: Some(vec![
                "lst".into();
                if matches!(kind, "and" | "or") { 2 } else { 1 }
            ]),
            out: Some("alias".into()),
            ..OpIR::default()
        });
    }
    for alias in aliases {
        for before_loop in [false, true] {
            let mut ops = typed_list_hoist_fixture()[..3].to_vec();
            if before_loop {
                ops.push(alias.clone());
            }
            let start = ops.len();
            ops.push(OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "index".into(),
                args: Some(vec!["lst".into(), "idx".into()]),
                out: Some("cur".into()),
                ..OpIR::default()
            });
            if !before_loop {
                ops.push(alias.clone());
            }
            ops.push(OpIR {
                kind: "list_append".into(),
                args: Some(vec!["alias".into(), "cur".into()]),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            });
            let plan = representation_plan_for_ops(&ops);
            let pre = collect_pre_loop_defined_names(&ops, start);
            let (flat, generic) = scan_loop_hoistable_lists(&ops, start, &pre, &plan);
            assert!(
                !flat.contains("lst") && !generic.contains("lst"),
                "{alias:?} prefix={before_loop}"
            );
        }
    }
}

#[test]
fn list_hoisting_does_not_turn_copy_metadata_into_a_buffer_definition() {
    for kind in ["copy_var", "load_var"] {
        let mut ops = typed_list_hoist_fixture();
        ops.insert(
            4,
            OpIR {
                kind: kind.into(),
                var: Some("lst".into()),
                args: Some(vec!["idx".into()]),
                out: Some("alias".into()),
                ..OpIR::default()
            },
        );
        let plan = representation_plan_for_ops(&ops);
        let pre = collect_pre_loop_defined_names(&ops, 3);
        let (flat, generic) = scan_loop_hoistable_lists(&ops, 3, &pre, &plan);
        assert!(flat.contains("lst") || generic.contains("lst"), "{kind}");
    }
}

#[test]
fn list_hoisting_fences_nested_effects_and_pairs_indexed_preludes() {
    let fixture = typed_list_hoist_fixture();
    for (kind, args) in [
        ("call", vec![]),
        ("call_method_ic", vec!["receiver"]),
        ("call_indirect", vec!["callee", "callargs"]),
        ("call_bind", vec!["callee", "callargs"]),
        ("invoke_ffi", vec![]),
        ("list_reverse", vec!["lst"]),
        ("list_pop", vec!["lst"]),
        ("del_index", vec!["lst", "idx"]),
        ("index_set", vec!["lst", "idx", "total"]),
    ] {
        for nested in [false, true] {
            let mut ops = fixture[..4].to_vec();
            if nested {
                ops.push(OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                });
                ops.push(OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("inner_zero".into()),
                    ..OpIR::default()
                });
                ops.push(OpIR {
                    kind: "loop_index_start".into(),
                    args: Some(vec!["inner_zero".into()]),
                    out: Some("inner_idx".into()),
                    ..OpIR::default()
                });
            }
            ops.push(OpIR {
                kind: kind.into(),
                args: Some(args.iter().map(|name| name.to_string()).collect()),
                ..OpIR::default()
            });
            if nested {
                ops.push(OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                });
            }
            ops.extend_from_slice(&fixture[4..]);
            let plan = representation_plan_for_ops(&ops);
            let pre = collect_pre_loop_defined_names(&ops, 3);
            let (flat, generic) = scan_loop_hoistable_lists(&ops, 3, &pre, &plan);
            assert!(
                flat.is_empty() && generic.is_empty(),
                "{kind}, nested={nested}"
            );
        }
    }

    // One indexed prelude is one loop. Neither a nested prelude nor a mutation
    // after the matching outer end may swallow that boundary.
    let mut ops = fixture[..4].to_vec();
    ops.push(OpIR {
        kind: "loop_index_start".into(),
        args: Some(vec!["idx".into()]),
        out: Some("outer_idx".into()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "loop_start".into(),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "const".into(),
        value: Some(0),
        out: Some("inner_zero".into()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "loop_index_start".into(),
        args: Some(vec!["inner_zero".into()]),
        out: Some("inner_idx".into()),
        ..OpIR::default()
    });
    ops.push(OpIR {
        kind: "loop_end".into(),
        ..OpIR::default()
    });
    ops.extend_from_slice(&fixture[4..]);
    ops.push(OpIR {
        kind: "list_append".into(),
        args: Some(vec!["lst".into(), "total".into()]),
        ..OpIR::default()
    });
    let plan = representation_plan_for_ops(&ops);
    let pre = collect_pre_loop_defined_names(&ops, 3);
    assert!(
        scan_loop_hoistable_lists(&ops, 3, &pre, &plan)
            .0
            .contains("lst")
    );
}

#[test]
fn list_hoisting_requires_stable_names_and_callback_free_indexing() {
    for destination in ["lst", "local_total"] {
        let mut ops = typed_list_hoist_fixture();
        ops.insert(
            6,
            OpIR {
                kind: "store_var".into(),
                var: Some(destination.into()),
                args: Some(vec![if destination == "lst" {
                    "lst".into()
                } else {
                    "total".into()
                }]),
                ..OpIR::default()
            },
        );
        let plan = representation_plan_for_ops(&ops);
        let pre = collect_pre_loop_defined_names(&ops, 3);
        let (flat, generic) = scan_loop_hoistable_lists(&ops, 3, &pre, &plan);
        assert_eq!(
            flat.contains("lst") || generic.contains("lst"),
            destination != "lst",
            "{destination}"
        );
    }
    let mut ops = typed_list_hoist_fixture();
    ops[4].args = Some(vec!["lst".into(), "opaque_index".into()]);
    let plan = representation_plan_for_ops(&ops);
    let pre = collect_pre_loop_defined_names(&ops, 3);
    let (flat, generic) = scan_loop_hoistable_lists(&ops, 3, &pre, &plan);
    assert!(
        flat.is_empty() && generic.is_empty(),
        "__index__ may mutate captured lists"
    );
}

#[test]
fn list_hoisting_requires_real_preheader_and_definition_dominance() {
    let fixture = typed_list_hoist_fixture();
    let mut continued = fixture.clone();
    continued.insert(
        6,
        OpIR {
            kind: "loop_continue".into(),
            ..OpIR::default()
        },
    );
    let plan = representation_plan_for_ops(&continued);
    let pre = collect_pre_loop_defined_names(&continued, 3);
    assert!(
        scan_loop_hoistable_lists(&continued, 3, &pre, &plan)
            .0
            .contains("lst"),
        "unreachable loop_end after a real backedge must not disable hoisting"
    );

    let mut bypass = fixture[..3].to_vec();
    bypass.push(OpIR {
        kind: "br_if".into(),
        args: Some(vec!["idx".into()]),
        value: Some(90),
        ..OpIR::default()
    });
    bypass.push(fixture[3].clone());
    bypass.push(OpIR {
        kind: "label".into(),
        value: Some(90),
        ..OpIR::default()
    });
    bypass.extend_from_slice(&fixture[4..]);

    let mut partial_definition = fixture[1..3].to_vec();
    partial_definition.push(OpIR {
        kind: "br_if".into(),
        args: Some(vec!["idx".into()]),
        value: Some(91),
        ..OpIR::default()
    });
    partial_definition.push(fixture[0].clone());
    partial_definition.push(OpIR {
        kind: "label".into(),
        value: Some(91),
        ..OpIR::default()
    });
    partial_definition.extend_from_slice(&fixture[3..]);
    for (ops, start) in [(bypass, 4), (partial_definition, 5)] {
        let plan = representation_plan_for_ops(&ops);
        let pre = collect_pre_loop_defined_names(&ops, start);
        let (flat, generic) = scan_loop_hoistable_lists(&ops, start, &pre, &plan);
        assert!(
            flat.is_empty() && generic.is_empty(),
            "lexical position is not a reaching-definition proof"
        );
    }
}

#[test]
fn pre_loop_definitions_follow_result_and_binding_roles() {
    let ops = vec![
        OpIR {
            kind: "copy_var".into(),
            var: Some("metadata".into()),
            args: Some(vec!["read_only".into()]),
            out: Some("copy".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "unpack_sequence".into(),
            value: Some(2),
            args: Some(vec!["read_only".into(), "first".into(), "second".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "checked_add".into(),
            var: Some("sum".into()),
            out: Some("overflow".into()),
            args: Some(vec!["read_only".into(); 2]),
            ..OpIR::default()
        },
        OpIR {
            kind: "dec_ref".into(),
            out: Some("out_metadata".into()),
            args: Some(vec!["read_only".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("slot".into()),
            out: Some("snapshot".into()),
            args: Some(vec!["read_only".into()]),
            ..OpIR::default()
        },
    ];
    assert_eq!(
        collect_pre_loop_defined_names(&ops, ops.len()),
        [
            "copy", "first", "second", "sum", "overflow", "slot", "snapshot"
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
}
