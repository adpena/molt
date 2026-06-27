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
        name: "exception_label_forwarding_args_widen_downstream_merge_args".into(),
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
        name: "join_test".into(),
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
        name: "union_test".into(),
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
        name: "dynbox_test".into(),
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
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::Copy, vec![source], vec![result], attrs)],
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
        name: "never_bottom_late_dominator".into(),
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
