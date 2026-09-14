use molt_passes::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_passes::tir::function::TirFunction;
use molt_passes::tir::op_kinds_generated::opcode_refcount_balance_role_table;
use molt_passes::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use molt_passes::tir::passes::refcount_elim::{run, run_post_drop};
use molt_passes::tir::types::TirType;
use molt_passes::tir::values::{TirValue, ValueId};

fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_func() -> TirFunction {
    TirFunction::new("f".into(), vec![], TirType::None)
}

fn add_block(func: &mut TirFunction, ops: Vec<TirOp>, terminator: Terminator) -> BlockId {
    let bid = func.fresh_block();
    let block = TirBlock {
        id: bid,
        args: vec![],
        ops,
        terminator,
    };
    func.blocks.insert(bid, block);
    bid
}

#[test]
fn adjacent_incref_decref_removed() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

#[test]
fn reversed_decref_incref_preserves_zero_transition() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 2);
}

#[test]
fn stackalloc_incref_decref_removed() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::StackAlloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    assert_eq!(
        func.blocks[&func.entry_block].ops[0].opcode,
        OpCode::StackAlloc
    );
}

#[test]
fn call_barrier_preserves_unpassed_value_refs() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

#[test]
fn no_incref_decref_no_changes() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
}

#[test]
fn different_values_do_not_form_pair() {
    let mut func = make_func();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v1], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v2], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 2);
}

#[test]
fn cross_block_incref_decref_sole_pred() {
    let mut func = make_func();
    let v = func.fresh_value();

    let succ_bid = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::Branch {
        target: succ_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
    assert!(func.blocks[&succ_bid].ops.is_empty());
}

#[test]
fn cross_block_multiple_predecessors_preserve_refs() {
    let mut func = make_func();
    let v = func.fresh_value();

    let succ_bid = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );

    let other_pred = add_block(
        &mut func,
        vec![],
        Terminator::Branch {
            target: succ_bid,
            args: vec![],
        },
    );

    let cond = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::ConstBool, vec![], vec![cond]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::CondBranch {
        cond,
        then_block: succ_bid,
        then_args: vec![],
        else_block: other_pred,
        else_args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn cross_block_trailing_callback_preserves_refs() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();

    let succ_bid = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
    entry.terminator = Terminator::Branch {
        target: succ_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn loop_invariant_incref_decref_eliminated() {
    let mut func = make_func();
    let v = func.fresh_value();
    let cond = func.fresh_value();

    let exit_bid = add_block(&mut func, vec![], Terminator::Return { values: vec![] });

    let header_bid = add_block(
        &mut func,
        vec![
            make_op(OpCode::IncRef, vec![v], vec![]),
            make_op(OpCode::ConstBool, vec![], vec![cond]),
            make_op(OpCode::DecRef, vec![v], vec![]),
        ],
        Terminator::CondBranch {
            cond,
            then_block: BlockId(0),
            then_args: vec![],
            else_block: exit_bid,
            else_args: vec![],
        },
    );

    func.blocks.get_mut(&header_bid).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: header_bid,
        then_args: vec![],
        else_block: exit_bid,
        else_args: vec![],
    };

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.terminator = Terminator::Branch {
        target: header_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&header_bid].ops.len(), 1);
    assert_eq!(func.blocks[&header_bid].ops[0].opcode, OpCode::ConstBool);
}

#[test]
fn local_pair_inside_loop_header_is_eliminated() {
    let mut func = make_func();
    let v = func.fresh_value();
    let cond = func.fresh_value();

    let exit_bid = add_block(&mut func, vec![], Terminator::Return { values: vec![] });

    let header_bid = add_block(
        &mut func,
        vec![
            make_op(OpCode::Alloc, vec![], vec![v]),
            make_op(OpCode::IncRef, vec![v], vec![]),
            make_op(OpCode::ConstBool, vec![], vec![cond]),
            make_op(OpCode::DecRef, vec![v], vec![]),
        ],
        Terminator::CondBranch {
            cond,
            then_block: BlockId(0),
            then_args: vec![],
            else_block: exit_bid,
            else_args: vec![],
        },
    );

    func.blocks.get_mut(&header_bid).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: header_bid,
        then_args: vec![],
        else_block: exit_bid,
        else_args: vec![],
    };

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Branch {
        target: header_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&header_bid].ops.len(), 2);
}

#[test]
fn cross_block_reversed_decref_incref() {
    let mut func = make_func();
    let v = func.fresh_value();

    let succ_bid = add_block(
        &mut func,
        vec![make_op(OpCode::IncRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Branch {
        target: succ_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    assert_eq!(func.blocks[&succ_bid].ops.len(), 1);
}

#[test]
fn cross_block_leading_callback_preserves_refs() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();

    let succ_bid = add_block(
        &mut func,
        vec![
            make_op(OpCode::Call, vec![callee], vec![call_result]),
            make_op(OpCode::DecRef, vec![v], vec![]),
        ],
        Terminator::Return { values: vec![] },
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::Branch {
        target: succ_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn generic_operator_preserves_refs() {
    let mut func = make_func();
    let v = func.fresh_value();
    let result = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Add, vec![v, v], vec![result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
    entry.terminator = Terminator::Return {
        values: vec![const_none],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 5);
}

#[test]
fn returned_value_release_is_kept() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 4);
}

#[test]
fn heap_store_retention_is_kept() {
    let mut func = make_func();
    let target = func.fresh_value();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::StoreAttr, vec![target, v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

#[test]
fn unpassed_local_preserves_callback_boundary_refs() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
    entry.terminator = Terminator::Return {
        values: vec![const_none],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 5);
}

#[test]
fn call_argument_retention_is_kept() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee, v], vec![call_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
    entry.terminator = Terminator::Return {
        values: vec![const_none],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 4);
}

#[test]
fn callback_barrier_preserves_every_value_ref() {
    let mut func = make_func();
    let local_v = func.fresh_value();
    let heap_v = func.fresh_value();
    let target = func.fresh_value();
    let add_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![local_v]));
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![local_v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![heap_v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::StoreAttr, vec![target, heap_v], vec![]));
    entry.ops.push(make_op(
        OpCode::Add,
        vec![local_v, local_v],
        vec![add_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![heap_v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![local_v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 7);
    let remaining_refs: Vec<_> = entry
        .ops
        .iter()
        .filter(|op| opcode_refcount_balance_role_table(op.opcode).is_refcount_balance())
        .collect();
    assert_eq!(remaining_refs.len(), 4);
}

#[test]
fn exception_region_drop_marker_protects_lone_decref_without_full_drop_gate() {
    let mut func = make_func();
    let v = func.fresh_value();

    func.attrs.insert(
        molt_passes::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
        molt_passes::tir::ops::AttrValue::Bool(true),
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );

    assert_eq!(
        stats.ops_removed, 0,
        "exception-only pre-bail drops must receive post-drop protection in refcount_elim"
    );
    assert!(
        !func
            .attrs
            .contains_key(molt_passes::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
        "exception-only protection must not set native's full drop_inserted gate"
    );
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    assert_eq!(func.blocks[&func.entry_block].ops[0].opcode, OpCode::DecRef);
}

#[test]
fn post_drop_keeps_check_exception_edge_payload_retain_release() {
    let mut func = make_func();
    let payload = func.fresh_value();
    let handler = func.fresh_block();
    let handler_arg = func.fresh_value();
    let label = 77;

    func.has_exception_handling = true;
    func.label_id_map.insert(handler.0, label);

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![payload], vec![]));
    entry.ops.push(make_op_with_attr(
        OpCode::CheckException,
        vec![payload],
        vec![],
        "value",
        molt_passes::tir::ops::AttrValue::Int(label),
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![payload], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run_post_drop(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );

    assert_eq!(
        stats.ops_removed, 0,
        "post-drop cleanup must preserve the retain consumed by the handler edge"
    );
    assert_eq!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .map(|op| op.opcode)
            .collect::<Vec<_>>(),
        vec![OpCode::IncRef, OpCode::CheckException, OpCode::DecRef]
    );
}

#[test]
fn post_drop_keeps_try_start_edge_payload_retain_release() {
    let mut func = make_func();
    let payload = func.fresh_value();
    let handler = func.fresh_block();
    let handler_arg = func.fresh_value();
    let label = 88;

    func.has_exception_handling = true;
    func.label_id_map.insert(handler.0, label);

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![payload], vec![]));
    entry.ops.push(make_op_with_attr(
        OpCode::TryStart,
        vec![payload],
        vec![],
        "value",
        molt_passes::tir::ops::AttrValue::Int(label),
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![payload], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run_post_drop(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );

    assert_eq!(
        stats.ops_removed, 0,
        "post-drop cleanup must preserve the retain consumed by the try handler edge"
    );
    assert_eq!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .map(|op| op.opcode)
            .collect::<Vec<_>>(),
        vec![OpCode::IncRef, OpCode::TryStart, OpCode::DecRef]
    );
}

#[test]
fn post_drop_keeps_raise_boundary_retain_release() {
    let mut func = make_func();
    let payload = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![payload], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Raise, vec![payload], vec![]));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![payload], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run_post_drop(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );

    assert_eq!(
        stats.ops_removed, 0,
        "post-drop cleanup must not pair across a no-fallthrough raise"
    );
    assert_eq!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .map(|op| op.opcode)
            .collect::<Vec<_>>(),
        vec![OpCode::IncRef, OpCode::Raise, OpCode::DecRef]
    );
}

#[test]
fn closure_store_retention_is_kept() {
    let mut func = make_func();
    let v = func.fresh_value();
    let cell = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::ClosureStore, vec![cell, v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn container_element_retention_is_kept() {
    let mut func = make_func();
    let elem = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();
    let list_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![elem], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
    entry
        .ops
        .push(make_op(OpCode::BuildList, vec![elem], vec![list_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![elem], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

fn make_op_with_attr(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    key: &str,
    value: molt_passes::tir::ops::AttrValue,
) -> TirOp {
    let mut op = make_op(opcode, operands, results);
    op.attrs.insert(key.to_string(), value);
    op
}

#[test]
fn finalizer_release_is_never_replaced_with_free() {
    use molt_passes::tir::ops::AttrValue;
    let mut func = make_func();
    let del_obj = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op_with_attr(
        OpCode::ObjectNewBound,
        vec![],
        vec![del_obj],
        "defines_del",
        AttrValue::Bool(true),
    ));
    entry.ops.push(make_op(
        OpCode::Call,
        vec![callee, del_obj],
        vec![call_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![del_obj], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );

    let ops = &func.blocks[&func.entry_block].ops;
    assert!(
        ops.iter()
            .any(|op| op.opcode == OpCode::DecRef && op.operands.first() == Some(&del_obj)),
        "finalizer DecRef must survive as a DecRef"
    );
    assert!(
        !ops.iter().any(|op| op.opcode == OpCode::Free),
        "a finalizer-bearing DecRef must NEVER be promoted to Free"
    );
}

#[test]
fn alias_exposure_never_rewrites_release_to_free() {
    let mut func = make_func();
    let root = func.fresh_value();
    let alias = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![root]));
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![root], vec![alias]));
    entry
        .ops
        .push(make_op(OpCode::CallBuiltin, vec![alias], vec![result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![root], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops.last().unwrap().opcode,
        OpCode::DecRef
    );
}

#[test]
fn cfg_forwarded_capture_keeps_original_owner_release() {
    for shape in 0..4 {
        let mut func = make_func();
        let root = func.fresh_value();
        let parameter = func.fresh_value();
        let control = func.fresh_value();
        let call_result = func.fresh_value();
        let destination = add_block(
            &mut func,
            vec![
                make_op(OpCode::CallBuiltin, vec![parameter], vec![call_result]),
                make_op(OpCode::DecRef, vec![root], vec![]),
            ],
            Terminator::Return { values: vec![] },
        );
        func.blocks
            .get_mut(&destination)
            .unwrap()
            .args
            .push(TirValue {
                id: parameter,
                ty: TirType::DynBox,
            });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::Alloc, vec![], vec![root]));
        entry
            .ops
            .push(make_op(OpCode::ConstBool, vec![], vec![control]));
        entry.terminator = match shape {
            0 => Terminator::Branch {
                target: destination,
                args: vec![root],
            },
            1 => Terminator::CondBranch {
                cond: control,
                then_block: destination,
                then_args: vec![root],
                else_block: destination,
                else_args: vec![root],
            },
            2 => Terminator::Switch {
                value: control,
                cases: vec![(1, destination, vec![root])],
                default: destination,
                default_args: vec![root],
            },
            _ => Terminator::StateDispatch {
                cases: vec![(1, destination, vec![root])],
                default: destination,
                default_args: vec![root],
            },
        };
        let stats = run(
            &mut func,
            &mut molt_passes::tir::analysis::AnalysisManager::new(),
        );
        assert_eq!(stats.ops_removed, 0, "shape {shape}");
        assert_eq!(stats.values_changed, 0, "shape {shape}");
    }
}

#[test]
fn mixed_cfg_value_never_inherits_stack_rc_elision() {
    let mut func = TirFunction::new("mixed".into(), vec![TirType::DynBox], TirType::None);
    let stack = func.fresh_value();
    let parameter = func.fresh_value();
    let control = func.fresh_value();
    let destination = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![parameter], vec![])],
        Terminator::Return { values: vec![] },
    );
    func.blocks
        .get_mut(&destination)
        .unwrap()
        .args
        .push(TirValue {
            id: parameter,
            ty: TirType::DynBox,
        });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::StackAlloc, vec![], vec![stack]));
    entry
        .ops
        .push(make_op(OpCode::ConstBool, vec![], vec![control]));
    entry.terminator = Terminator::CondBranch {
        cond: control,
        then_block: destination,
        then_args: vec![stack],
        else_block: destination,
        else_args: vec![ValueId(0)],
    };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn unsupported_stack_alias_does_not_mint_rc_inert_representation() {
    let mut func = make_func();
    let stack = func.fresh_value();
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::StackAlloc, vec![], vec![stack]));
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![stack], vec![alias]));
    entry.ops.push(make_op(OpCode::DecRef, vec![alias], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn local_heap_and_unpromotable_layout_keep_destruction() {
    for opcode in [
        OpCode::Alloc,
        OpCode::ObjectNewBound,
        OpCode::BuildList,
        OpCode::BuildDict,
        OpCode::BuildTuple,
        OpCode::BuildSet,
        OpCode::AllocTask,
    ] {
        let mut func = make_func();
        let root = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(opcode, vec![], vec![root]));
        entry.ops.push(make_op(OpCode::DecRef, vec![root], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };
        assert_eq!(
            molt_passes::tir::passes::escape_analysis::analyze(&func)[&root],
            molt_passes::tir::passes::escape_analysis::EscapeState::NoEscape
        );
        let stats = run(
            &mut func,
            &mut molt_passes::tir::analysis::AnalysisManager::new(),
        );
        assert_eq!(stats.ops_removed, 0, "{opcode:?}");
        assert_eq!(stats.values_changed, 0, "{opcode:?}");
        assert_eq!(
            func.blocks[&func.entry_block].ops.last().unwrap().opcode,
            OpCode::DecRef
        );
    }
}

#[test]
fn rc_pairs_do_not_cross_python_callbacks_or_unrelated_finalizers() {
    for opcode in [
        OpCode::Add,
        OpCode::Bool,
        OpCode::LoadAttr,
        OpCode::Index,
        OpCode::GetIter,
        OpCode::IterNext,
        OpCode::Copy,
        OpCode::DecRef,
        OpCode::Free,
        OpCode::DeleteVar,
        OpCode::DelBoundary,
    ] {
        let mut func = make_func();
        let root = func.fresh_value();
        let other = func.fresh_value();
        let result = func.fresh_value();
        let mut boundary = make_op(opcode, vec![other], vec![result]);
        if opcode == OpCode::Copy {
            boundary.attrs.insert(
                "_original_kind".into(),
                molt_passes::tir::ops::AttrValue::Str("opaque_future_callback".into()),
            );
        }
        if matches!(
            opcode,
            OpCode::DecRef | OpCode::Free | OpCode::DeleteVar | OpCode::DelBoundary
        ) {
            boundary.results.clear();
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![root], vec![]));
        entry.ops.push(boundary);
        entry.ops.push(make_op(OpCode::DecRef, vec![root], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };
        let stats = run(
            &mut func,
            &mut molt_passes::tir::analysis::AnalysisManager::new(),
        );
        assert_eq!(stats.ops_removed, 0, "{opcode:?}");
    }
}

#[test]
fn cross_block_pair_cannot_remove_retain_from_other_successor() {
    let mut func = make_func();
    let root = func.fresh_value();
    let control = func.fresh_value();
    let drop_path = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![root], vec![])],
        Terminator::Return { values: vec![] },
    );
    let retained_path = add_block(&mut func, vec![], Terminator::Return { values: vec![root] });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![root], vec![]));
    entry.terminator = Terminator::CondBranch {
        cond: control,
        then_block: drop_path,
        then_args: vec![],
        else_block: retained_path,
        else_args: vec![],
    };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn cross_block_chain_removes_only_proved_pair_endpoints() {
    let mut func = make_func();
    let root = func.fresh_value();
    let other = func.fresh_value();
    let last = add_block(
        &mut func,
        vec![
            make_op(OpCode::DecRef, vec![root], vec![]),
            make_op(OpCode::DecRef, vec![other], vec![]),
        ],
        Terminator::Return { values: vec![] },
    );
    let middle = add_block(
        &mut func,
        vec![
            make_op(OpCode::DecRef, vec![root], vec![]),
            make_op(OpCode::IncRef, vec![root], vec![]),
        ],
        Terminator::Branch {
            target: last,
            args: vec![],
        },
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![root], vec![]));
    entry.terminator = Terminator::Branch {
        target: middle,
        args: vec![],
    };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 4);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
    assert!(func.blocks[&middle].ops.is_empty());
    assert_eq!(func.blocks[&last].ops.len(), 1);
    assert_eq!(func.blocks[&last].ops[0].operands, vec![other]);
}

#[test]
fn local_pairing_handles_nested_retains_and_exact_aliases() {
    let mut func = make_func();
    let root = func.fresh_value();
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Copy, vec![root], vec![alias]));
    entry.ops.push(make_op(OpCode::IncRef, vec![root], vec![]));
    entry.ops.push(make_op(OpCode::IncRef, vec![alias], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![root], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![alias], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![root], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 4);
    assert_eq!(
        func.blocks[&func.entry_block].ops.last().unwrap().opcode,
        OpCode::DecRef
    );
}

#[test]
fn pre_and_post_drop_paths_share_release_preserving_authority() {
    let mut before = make_func();
    let value = before.fresh_value();
    before.blocks.get_mut(&before.entry_block).unwrap().ops = vec![
        make_op(OpCode::Alloc, vec![], vec![value]),
        make_op(OpCode::DecRef, vec![value], vec![]),
    ];
    let mut after = before.clone();
    let first = run(
        &mut before,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    let second = run_post_drop(
        &mut after,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(first.ops_removed, second.ops_removed);
    assert_eq!(first.values_changed, second.values_changed);
    let opcodes = |func: &TirFunction| {
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .map(|op| op.opcode)
            .collect::<Vec<_>>()
    };
    assert_eq!(opcodes(&before), opcodes(&after));
}

#[test]
fn proven_nonheap_carrier_release_is_elided_without_lifetime_inference() {
    let mut func = make_func();
    let value = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op_with_attr(
        OpCode::ConstBool,
        vec![],
        vec![value],
        "value",
        molt_passes::tir::ops::AttrValue::Bool(true),
    ));
    entry.ops.push(make_op(OpCode::DecRef, vec![value], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 1);
}

#[test]
fn nominal_float_annotation_is_not_nonheap_release_permission() {
    let mut func = TirFunction::new("annotation".into(), vec![TirType::F64], TirType::None);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![ValueId(0)], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
}

#[test]
fn malformed_local_refcount_op_is_a_barrier() {
    let mut func = make_func();
    let v = func.fresh_value();
    let invalid_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![v], vec![invalid_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

#[test]
fn malformed_cross_block_retain_is_not_canceled() {
    let mut func = make_func();
    let v = func.fresh_value();
    let invalid_result = func.fresh_value();
    let succ_bid = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::IncRef, vec![v], vec![invalid_result]));
    entry.terminator = Terminator::Branch {
        target: succ_bid,
        args: vec![],
    };

    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    assert_eq!(func.blocks[&succ_bid].ops.len(), 1);
}

#[test]
fn cross_block_same_predecessor_exception_entry_preserves_refs() {
    // Both EH opcodes project through the shared edge authority. The coarse
    // has_exception_handling hint must not be required to notice a real edge.
    for edge in [OpCode::TryStart, OpCode::CheckException] {
        for has_exception_handling in [false, true] {
            let mut func = make_func();
            let v = func.fresh_value();
            let callee = func.fresh_value();
            let call_result = func.fresh_value();
            let succ = add_block(
                &mut func,
                vec![make_op(OpCode::DecRef, vec![v], vec![])],
                Terminator::Return { values: vec![] },
            );
            let label = 91;
            func.label_id_map.insert(succ.0, label);
            func.has_exception_handling = has_exception_handling;
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_op_with_attr(
                edge,
                vec![],
                vec![],
                "value",
                molt_passes::tir::ops::AttrValue::Int(label),
            ));
            entry
                .ops
                .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
            entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
            entry.terminator = Terminator::Branch {
                target: succ,
                args: vec![],
            };
            assert_eq!(
                molt_passes::tir::dominators::build_pred_map(&func)[&succ],
                vec![func.entry_block],
                "the predecessor set deliberately hides edge multiplicity"
            );
            let stats = run(
                &mut func,
                &mut molt_passes::tir::analysis::AnalysisManager::new(),
            );
            assert_eq!(
                stats.ops_removed, 0,
                "{edge:?}, EH hint={has_exception_handling}"
            );
            assert_eq!(func.blocks[&succ].ops[0].opcode, OpCode::DecRef);
            assert_eq!(
                func.blocks[&func.entry_block].ops.last().unwrap().opcode,
                OpCode::IncRef
            );
        }
    }
}

#[test]
fn cross_block_backedge_to_function_entry_preserves_initial_release() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();
    let entry_id = func.entry_block;
    let backedge = add_block(
        &mut func,
        vec![make_op(OpCode::IncRef, vec![v], vec![])],
        Terminator::Branch {
            target: entry_id,
            args: vec![],
        },
    );
    let entry = func.blocks.get_mut(&entry_id).unwrap();
    entry.ops = vec![
        make_op(OpCode::DecRef, vec![v], vec![]),
        make_op(OpCode::Call, vec![callee], vec![call_result]),
    ];
    entry.terminator = Terminator::Branch {
        target: backedge,
        args: vec![],
    };
    assert_eq!(
        molt_passes::tir::dominators::build_pred_map(&func)[&entry_id],
        vec![backedge]
    );
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&entry_id].ops[0].opcode, OpCode::DecRef);
    assert_eq!(func.blocks[&backedge].ops[0].opcode, OpCode::IncRef);
}

#[test]
fn cross_block_exception_to_other_handler_does_not_disable_valid_pair() {
    let mut func = make_func();
    let v = func.fresh_value();
    let succ = add_block(
        &mut func,
        vec![make_op(OpCode::DecRef, vec![v], vec![])],
        Terminator::Return { values: vec![] },
    );
    let handler = add_block(&mut func, vec![], Terminator::Return { values: vec![] });
    func.label_id_map.insert(handler.0, 92);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op_with_attr(
        OpCode::CheckException,
        vec![],
        vec![],
        "value",
        molt_passes::tir::ops::AttrValue::Int(92),
    ));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::Branch {
        target: succ,
        args: vec![],
    };
    let stats = run(
        &mut func,
        &mut molt_passes::tir::analysis::AnalysisManager::new(),
    );
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&succ].ops.is_empty());
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
}
