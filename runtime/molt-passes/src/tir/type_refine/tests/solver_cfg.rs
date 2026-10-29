use super::*;

#[test]
fn exception_label_forwarding_args_widen_downstream_merge_args() {
    let normal_value = ValueId(0);
    let handler_arg = ValueId(1);
    let merge_arg = ValueId(2);
    let entry = BlockId(0);
    let handler = BlockId(1);
    let merge = BlockId(2);

    let mut blocks = HashMap::new();
    blocks.insert(
        entry,
        TirBlock {
            id: entry,
            args: vec![],
            ops: vec![
                make_op(OpCode::TryStart, vec![], vec![], int_attr(10)),
                make_op(OpCode::ConstInt, vec![], vec![normal_value], int_attr(1)),
            ],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![normal_value],
            },
        },
    );
    blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![handler_arg],
            },
        },
    );
    blocks.insert(
        merge,
        TirBlock {
            id: merge,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );
    let mut label_id_map = HashMap::new();
    label_id_map.insert(handler.0, 10);
    let mut func = TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "exception_label_forwarding_args_widen_downstream_merge_args".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry,
        next_value: 3,
        next_block: 3,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: true,
        label_id_map,
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&handler_arg), Some(&TirType::DynBox));
    assert_eq!(
        type_map.get(&merge_arg),
        Some(&TirType::DynBox),
        "merge block args must widen when an exception-label forwarding block can feed DynBox"
    );
    assert_eq!(
        func.blocks[&merge].args[0].ty,
        TirType::DynBox,
        "refine_types must write the widened merge type back into block args"
    );
}

#[test]
fn block_arg_meet_same_types() {
    // Two predecessor blocks both pass I64 to a join block's arg.
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let join_id = BlockId(3);

    let mut blocks = HashMap::new();

    // Entry: cond branch to then/else
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::Bool,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            },
        },
    );

    // Then: const int, branch to join
    blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(1)],
                int_attr(10),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(1)],
            },
        },
    );

    // Else: const int, branch to join
    blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(2)],
                int_attr(20),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(2)],
            },
        },
    );

    // Join: one block arg (starts as DynBox), return
    blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: ValueId(3),
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(3)],
            },
        },
    );

    let mut func = TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "join_test".into(),
        execution_context: Default::default(),
        param_names: vec!["p0".into()],
        param_types: vec![TirType::Bool],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 4,
        next_block: 4,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let refined = refine_types(&mut func);

    // ValueId(1), ValueId(2) (const ints) and ValueId(3) (block arg) should
    // all be refined. ValueId(3) should be meet(I64, I64) = I64.
    assert!(refined >= 3);
    assert_eq!(func.blocks[&join_id].args[0].ty, TirType::I64);
}

#[test]
fn block_arg_meet_different_types_produces_union() {
    // One branch passes I64, another passes F64 → Union(I64, F64).
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let join_id = BlockId(3);

    let mut blocks = HashMap::new();

    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::Bool,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            },
        },
    );

    blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(1)],
                int_attr(10),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(1)],
            },
        },
    );

    blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(2)],
                float_attr(PI),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(2)],
            },
        },
    );

    blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: ValueId(3),
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(3)],
            },
        },
    );

    let mut func = TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "union_test".into(),
        execution_context: Default::default(),
        param_names: vec!["p0".into()],
        param_types: vec![TirType::Bool],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 4,
        next_block: 4,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let refined = refine_types(&mut func);
    assert!(refined >= 3);

    let join_arg_ty = &func.blocks[&join_id].args[0].ty;
    // Union member order depends on HashMap iteration order; accept either.
    assert!(
        *join_arg_ty == TirType::Union(vec![TirType::I64, TirType::F64])
            || *join_arg_ty == TirType::Union(vec![TirType::F64, TirType::I64]),
        "expected Union(I64, F64) in any order, got {:?}",
        join_arg_ty
    );
}

// ---- Test 6: Fixpoint convergence ----
#[test]
fn fixpoint_converges() {
    // Chain: ConstInt → Add → Add — all should resolve in ≤2 rounds.
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::Add,
            vec![ValueId(2), ValueId(0)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 4);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 4);
}

// ---- Test 7: DynBox stays DynBox when operands are unknown ----
#[test]
fn dynbox_stays_dynbox_for_unknown_operands() {
    // Add(DynBox, DynBox) → DynBox (no refinement possible)
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![
            TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            },
            TirValue {
                id: ValueId(1),
                ty: TirType::DynBox,
            },
        ],
        ops: vec![make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        )],
        terminator: Terminator::Return {
            values: vec![ValueId(2)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    let mut func = TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "dynbox_test".into(),
        execution_context: Default::default(),
        param_names: vec!["p0".into(), "p1".into()],
        param_types: vec![TirType::DynBox, TirType::DynBox],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };
    let refined = refine_types(&mut func);
    assert_eq!(refined, 0);
}

#[test]
fn dynamic_transfer_widens_stale_precise_result_and_stays_idempotent() {
    let source = ValueId(0);
    let result = ValueId(1);
    let attrs = AttrDict::new();

    let mut func = single_block_func(
        vec![
            make_op(OpCode::Copy, vec![source], vec![result], attrs)
                .with_origin(molt_ir::OpKind::CopyVar),
        ],
        2,
    );
    func.value_types.insert(source, TirType::DynBox);
    func.value_types.insert(result, TirType::Str);

    refine_types(&mut func);
    let once = extract_type_map(&func);
    assert_eq!(
        once.get(&result),
        Some(&TirType::DynBox),
        "a dynamic producer must widen stale precise facts to top"
    );

    refine_types(&mut func);
    let twice = extract_type_map(&func);
    assert_eq!(
        twice.get(&result),
        Some(&TirType::DynBox),
        "post-refinement extraction must not re-narrow widened top facts"
    );
}

#[test]
fn never_bottom_waits_for_late_dominator_and_drops_stale_result_fact() {
    let body_id = BlockId(0);
    let entry_id = BlockId(1);
    let source = ValueId(0);
    let alias = ValueId(1);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::Copy,
            vec![source],
            vec![alias],
            AttrDict::new(),
        )],
        terminator: Terminator::Return {
            values: vec![alias],
        },
    };
    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::ConstInt,
            vec![],
            vec![source],
            int_attr(41),
        )],
        terminator: Terminator::Branch {
            target: body_id,
            args: vec![],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(body_id, body);
    blocks.insert(entry_id, entry);

    let mut func = TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "never_bottom_late_dominator".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 2,
        next_block: 2,
        attrs: AttrDict::new(),
        value_types: HashMap::from([(alias, TirType::Str)]),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);
    let once = extract_type_map(&func);
    assert_eq!(
        once.get(&alias),
        Some(&TirType::I64),
        "bottom operands must wait for the dominating producer instead of \
         publishing the stale result fact or cementing DynBox"
    );
    assert_eq!(
        func.value_types.get(&alias),
        Some(&TirType::I64),
        "refine_types must publish the converged fact, not the stale input fact"
    );

    refine_types(&mut func);
    assert_eq!(
        extract_type_map(&func),
        once,
        "Never-bottom convergence must be idempotent after publication"
    );
}

#[test]
fn dense_check_exception_results_are_top_and_idempotent() {
    let ops: Vec<TirOp> = (0..1024)
        .map(|idx| {
            make_op(
                OpCode::CheckException,
                vec![],
                vec![ValueId(idx)],
                AttrDict::new(),
            )
        })
        .collect();
    let mut func = single_block_func(ops, 1024);
    func.name = "dense_exception_poll".into();
    func.has_exception_handling = true;

    refine_types(&mut func);
    let value_types_after_first = func.value_types.clone();
    assert_eq!(value_types_after_first.len(), 1024);
    assert!(
        value_types_after_first
            .values()
            .all(|ty| matches!(ty, TirType::DynBox)),
        "check_exception result facts are dynamic top"
    );

    refine_types(&mut func);
    assert_eq!(
        func.value_types, value_types_after_first,
        "dense exception refinement must be idempotent"
    );
}

fn copy_argument_chain(
    length: u32,
    reverse_block_ids: bool,
    parameter_type: TirType,
) -> (TirFunction, Vec<ValueId>) {
    let block_id = |position| {
        BlockId(if reverse_block_ids {
            length - position
        } else {
            position
        })
    };
    let mut func = single_block_func(vec![], length * 2 + 1);
    func.name = "copy_argument_chain".into();
    func.blocks.clear();
    func.entry_block = block_id(0);
    func.next_block = length + 1;
    func.param_names = vec!["value".into()];
    func.param_types = vec![parameter_type.clone()];
    func.return_abi = molt_ir::FunctionReturnAbi::Value;
    func.blocks.insert(
        block_id(0),
        TirBlock {
            id: block_id(0),
            args: vec![TirValue {
                id: ValueId(0),
                ty: parameter_type,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: block_id(1),
                args: vec![ValueId(0)],
            },
        },
    );
    let mut values = Vec::new();
    for position in 1..=length {
        let argument = ValueId(position * 2 - 1);
        let result = ValueId(position * 2);
        values.extend([argument, result]);
        func.blocks.insert(
            block_id(position),
            TirBlock {
                id: block_id(position),
                args: vec![TirValue {
                    id: argument,
                    ty: TirType::DynBox,
                }],
                ops: vec![make_op(
                    OpCode::Copy,
                    vec![argument],
                    vec![result],
                    AttrDict::new(),
                )],
                terminator: if position == length {
                    Terminator::Return {
                        values: vec![result],
                    }
                } else {
                    Terminator::Branch {
                        target: block_id(position + 1),
                        args: vec![result],
                    }
                },
            },
        );
    }
    (func, values)
}

#[test]
fn long_block_argument_chains_reach_all_users_in_either_block_order() {
    // The old ops-then-arguments sweep moves a fact only one copy/argument
    // boundary per round. A non-scalar seed also prevents the independent exact
    // scalar analysis from resolving this chain before the refinement solver.
    let expected = TirType::List(Box::new(TirType::DynBox));
    for reverse_block_ids in [false, true] {
        let (mut func, values) = copy_argument_chain(96, reverse_block_ids, expected.clone());
        refine_types(&mut func);
        for value in values {
            assert_eq!(func.value_types[&value], expected);
        }
        let once = func.value_types.clone();
        refine_types(&mut func);
        assert_eq!(func.value_types, once);
    }
}

#[test]
fn late_dynamic_fact_widens_the_entire_argument_chain() {
    for reverse_block_ids in [false, true] {
        let (mut func, values) = copy_argument_chain(96, reverse_block_ids, TirType::DynBox);
        // Model precise argument facts left by an earlier optimization. The
        // live entry parameter is dynamic; every downstream user must widen,
        // including users that were processed before that change arrived.
        for &value in &values {
            func.value_types
                .insert(value, TirType::List(Box::new(TirType::DynBox)));
        }
        refine_types(&mut func);
        for value in values {
            assert_eq!(func.value_types[&value], TirType::DynBox);
        }
        let once = func.value_types.clone();
        refine_types(&mut func);
        assert_eq!(func.value_types, once);
    }
}

#[test]
fn recursive_constructor_facts_stabilize_without_growing_nested_bottom() {
    for constructor in [OpCode::BuildTuple, OpCode::BoxVal, OpCode::GetIter] {
        let wrap = |inner| match constructor {
            OpCode::BuildTuple => TirType::Tuple(vec![inner]),
            OpCode::BoxVal => TirType::Box(Box::new(inner)),
            OpCode::GetIter => TirType::Iterator(Box::new(inner)),
            _ => unreachable!(),
        };
        for seed_inner in [TirType::I64, TirType::Never] {
            let seed_type = wrap(seed_inner.clone());
            let entry = BlockId(2);
            let header = BlockId(0);
            let body = BlockId(1);
            let exit = BlockId(3);
            let seed = ValueId(0);
            let argument = ValueId(1);
            let result = ValueId(2);
            let condition = ValueId(3);
            let mut func = single_block_func(vec![], 5);
            func.name = "recursive_constructor".into();
            func.blocks.clear();
            func.entry_block = entry;
            func.next_block = 4;
            func.param_names = vec!["seed".into(), "condition".into()];
            func.param_types = vec![seed_type.clone(), TirType::Bool];
            func.return_abi = molt_ir::FunctionReturnAbi::Value;
            func.blocks.insert(
                entry,
                TirBlock {
                    id: entry,
                    args: vec![
                        TirValue {
                            id: seed,
                            ty: seed_type.clone(),
                        },
                        TirValue {
                            id: condition,
                            ty: TirType::Bool,
                        },
                    ],
                    ops: vec![],
                    terminator: Terminator::Branch {
                        target: header,
                        args: vec![seed],
                    },
                },
            );
            func.blocks.insert(
                header,
                TirBlock {
                    id: header,
                    args: vec![TirValue {
                        id: argument,
                        ty: TirType::DynBox,
                    }],
                    ops: vec![],
                    terminator: Terminator::CondBranch {
                        cond: condition,
                        then_block: body,
                        then_args: vec![],
                        else_block: exit,
                        else_args: vec![],
                    },
                },
            );
            // GetIter constructs Iterator(T) from a tuple containing T. This
            // exercises the same recursive backedge as Tuple(T) and Box(T).
            let body_ops = if constructor == OpCode::GetIter {
                vec![
                    make_op(
                        OpCode::BuildTuple,
                        vec![argument],
                        vec![ValueId(4)],
                        AttrDict::new(),
                    ),
                    make_op(constructor, vec![ValueId(4)], vec![result], AttrDict::new()),
                ]
            } else {
                vec![make_op(
                    constructor,
                    vec![argument],
                    vec![result],
                    AttrDict::new(),
                )]
            };
            func.blocks.insert(
                body,
                TirBlock {
                    id: body,
                    args: vec![],
                    ops: body_ops,
                    terminator: Terminator::Branch {
                        target: header,
                        args: vec![result],
                    },
                },
            );
            func.blocks.insert(
                exit,
                TirBlock {
                    id: exit,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return {
                        values: vec![argument],
                    },
                },
            );

            refine_types(&mut func);
            if seed_inner == TirType::Never {
                assert_eq!(func.value_types[&argument], seed_type);
                assert_eq!(
                    func.value_types[&result],
                    TirType::DynBox,
                    "a constructor must withhold its nested-bottom fact"
                );
            } else {
                // At runtime the loop can nest its initial value arbitrarily
                // deeply. The outer constructor is stable, its child is top.
                assert_eq!(func.value_types[&argument], wrap(TirType::DynBox));
                assert_eq!(func.value_types[&result], wrap(wrap(TirType::DynBox)));
            }
            let once = func.value_types.clone();
            refine_types(&mut func);
            assert_eq!(func.value_types, once);
        }
    }
}
