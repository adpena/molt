use super::*;

/// Lock-in for the loop-induction-variable seeding contract.
///
/// CFG:
/// ```text
/// entry:  i_init = ConstInt(0); branch header(i_init)
/// header(i: ?):  cond = ConstBool(true); cond_branch body, exit
/// body:  one = ConstInt(1); i_next = Add(i, one); branch header(i_next)
/// exit:  return
/// ```
/// Without IV seeding, `i` ends up DynBox: the body sees `i: DynBox`
/// initially, infers `Add(DynBox, I64)` as no-type, the back-edge
/// brings DynBox, and `meet(I64, DynBox) = DynBox` widens the entry.
/// With IV seeding, `i` is initialized to I64 (the entry-edge type
/// alone, since the back-edge is excluded from the seed), the body
/// then infers `Add(I64, I64) = I64`, the back-edge confirms I64,
/// and the fixpoint converges to I64.
#[test]
fn loop_iv_block_arg_seeded_to_entry_type() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let one = ValueId(3);
    let i_next = ValueId(4);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox, // intentionally pessimistic — the seeding fix narrows it
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![
            make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
            make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
        ],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);

    let mut func = TirFunction {
        name: "iv_loop".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 5,
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

    let _refined = refine_types(&mut func);

    let header_block = &func.blocks[&header_id];
    let i_arg_ty = header_block
        .args
        .iter()
        .find(|a| a.id == i)
        .map(|a| a.ty.clone())
        .expect("loop header arg `i` present");
    assert_eq!(
        i_arg_ty,
        TirType::I64,
        "loop induction variable seeded with entry-edge I64 must converge to I64, got {:?}",
        i_arg_ty
    );
}

#[test]
fn loop_iv_seed_widens_to_dynbox_when_backedge_is_dynamic() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let i_next = ValueId(3);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox,
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::Call,
            vec![i],
            vec![i_next],
            AttrDict::new(),
        )],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);

    let mut func = TirFunction {
        name: "iv_loop_dynamic_backedge".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
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

    refine_types(&mut func);
    let first = func.value_types.clone();
    assert_eq!(
        func.blocks[&header_id].args[0].ty,
        TirType::DynBox,
        "entry-edge I64 seed must widen when the reachable back-edge is dynamic"
    );
    assert_eq!(
        first.get(&i_next),
        Some(&TirType::DynBox),
        "dynamic back-edge producer must publish top, not stay at bottom"
    );

    refine_types(&mut func);
    assert_eq!(
        func.value_types, first,
        "loop-carried dynamic widening must be stable, not an oscillation fallback"
    );
}

#[test]
fn unreachable_loop_end_edge_does_not_widen_reachable_loop_arg() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);
    let dead_loop_end_id = BlockId(4);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let one = ValueId(3);
    let i_next = ValueId(4);
    let dead_none = ValueId(5);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox,
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![
            make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
            make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
        ],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    let dead_loop_end = TirBlock {
        id: dead_loop_end_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::ConstNone,
            vec![],
            vec![dead_none],
            AttrDict::new(),
        )],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![dead_none],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);
    blocks.insert(dead_loop_end_id, dead_loop_end);

    let mut func = TirFunction {
        name: "unreachable_loop_end_meet".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 6,
        next_block: 5,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::from([(dead_loop_end_id, LoopRole::LoopEnd)]),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);

    let header_block = &func.blocks[&header_id];
    let i_arg_ty = header_block
        .args
        .iter()
        .find(|a| a.id == i)
        .map(|a| a.ty.clone())
        .expect("loop header arg `i` present");
    assert_eq!(
        i_arg_ty,
        TirType::I64,
        "unreachable loop-end incoming values must not widen reachable loop-carried types"
    );
}
