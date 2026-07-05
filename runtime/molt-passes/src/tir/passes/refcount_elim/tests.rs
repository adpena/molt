use super::balance::is_refcount_balance_op;
use super::{run, run_post_drop};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

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

/// Helper to add a new block with the given ops and terminator.
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

// -----------------------------------------------------------------------
// Test 1: Adjacent IncRef+DecRef â†’ both removed
// -----------------------------------------------------------------------
#[test]
fn adjacent_incref_decref_removed() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 2: Reversed DecRef+IncRef â†’ both removed
// -----------------------------------------------------------------------
#[test]
fn reversed_decref_incref_removed() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 3: IncRef/DecRef on StackAlloc value â†’ removed (no pairing needed)
// -----------------------------------------------------------------------
#[test]
fn stackalloc_incref_decref_removed() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::StackAlloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    assert_eq!(
        func.blocks[&func.entry_block].ops[0].opcode,
        OpCode::StackAlloc
    );
}

// -----------------------------------------------------------------------
// Test 4: IncRef with intervening Call (v not passed) â†’ eliminated by
//         deferred RC since v has no heap exposure
// -----------------------------------------------------------------------
#[test]
fn incref_with_call_barrier_no_heap_exposure() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // Intra-block can't pair across Call barrier, but deferred RC
    // eliminates both because v has no heap exposure.
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
}

// -----------------------------------------------------------------------
// Test 5: No IncRef/DecRef â†’ no changes
// -----------------------------------------------------------------------
#[test]
fn no_incref_decref_no_changes() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
}

// -----------------------------------------------------------------------
// Test 6: Different values, no heap exposure â†’ both eliminated by
//         deferred RC
// -----------------------------------------------------------------------
#[test]
fn different_values_no_heap_exposure() {
    let mut func = make_func();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::IncRef, vec![v1], vec![]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v2], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // No intra-block pairing (different values), but deferred RC
    // eliminates both since neither has heap exposure.
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 0);
}

// ===================================================================
// Cross-block tests
// ===================================================================

// -----------------------------------------------------------------------
// Test 7: Cross-block IncRef(x) in pred â†’ DecRef(x) in succ (sole pred)
// -----------------------------------------------------------------------
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
    assert!(func.blocks[&succ_bid].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 8: Cross-block with multiple predecessors, no heap exposure â†’
//         eliminated by deferred RC
// -----------------------------------------------------------------------
#[test]
fn cross_block_multiple_preds_no_heap_exposure() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // Deferred RC eliminates both â€” v has no heap exposure.
    assert_eq!(stats.ops_removed, 2);
}

// -----------------------------------------------------------------------
// Test 9: Cross-block with trailing barrier, no heap exposure â†’
//         eliminated by deferred RC
// -----------------------------------------------------------------------
#[test]
fn cross_block_trailing_barrier_no_heap_exposure() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // Deferred RC eliminates both â€” v not passed to Call.
    assert_eq!(stats.ops_removed, 2);
}

// -----------------------------------------------------------------------
// Test 10: Loop-invariant IncRef/DecRef elimination
// -----------------------------------------------------------------------
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
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
    entry.terminator = Terminator::Branch {
        target: header_bid,
        args: vec![],
    };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&header_bid].ops.len(), 1);
    assert_eq!(func.blocks[&header_bid].ops[0].opcode, OpCode::ConstBool);
}

// -----------------------------------------------------------------------
// Test 11: Loop-invariant NOT eliminated when value defined in header
// -----------------------------------------------------------------------
#[test]
fn loop_noninvariant_not_eliminated() {
    let mut func = make_func();
    let v = func.fresh_value();
    let cond = func.fresh_value();

    let exit_bid = add_block(&mut func, vec![], Terminator::Return { values: vec![] });

    let header_bid = add_block(
        &mut func,
        vec![
            make_op(OpCode::ConstInt, vec![], vec![v]),
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // Intra-block pairing handles it (adjacent, no barrier).
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&header_bid].ops.len(), 2);
}

// -----------------------------------------------------------------------
// Test 12: Cross-block with reversed pair (DecRef trailing, IncRef leading)
// -----------------------------------------------------------------------
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
    assert!(func.blocks[&succ_bid].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 13: Cross-block with leading barrier in succ, no heap exposure â†’
//          eliminated by deferred RC
// -----------------------------------------------------------------------
#[test]
fn cross_block_leading_barrier_no_heap_exposure() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // Deferred RC eliminates both â€” v has no heap exposure.
    assert_eq!(stats.ops_removed, 2);
}

// ===================================================================
// Deferred RC (Deutsch-Bobrow) tests
// ===================================================================

// -----------------------------------------------------------------------
// Test 14: IncRef/DecRef on local-only value â†’ eliminated
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_local_only_eliminated() {
    let mut func = make_func();
    let v = func.fresh_value();
    let result = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

// -----------------------------------------------------------------------
// Test 15: IncRef/DecRef on returned value â†’ NOT eliminated
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_returned_value_kept() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // v returned (heap exposure) + Call barrier = nothing eliminated.
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 4);
}

// -----------------------------------------------------------------------
// Test 16: IncRef/DecRef on value stored to attr â†’ NOT eliminated
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_heap_store_kept() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

// -----------------------------------------------------------------------
// Test 17: Barrier but no heap exposure â†’ deferred RC eliminates
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_barrier_no_heap_exposure_eliminated() {
    let mut func = make_func();
    let v = func.fresh_value();
    let callee = func.fresh_value();
    let call_result = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // v not passed to Call, not returned â€” deferred RC eliminates.
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
}

// -----------------------------------------------------------------------
// Test 18: Value passed to Call â†’ kept
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_call_arg_kept() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // v passed to Call = heap exposure. Call is barrier. Nothing eliminated.
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 4);
}

// -----------------------------------------------------------------------
// Test 19: Mixed â€” only non-exposed values eliminated
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_mixed_exposure() {
    let mut func = make_func();
    let local_v = func.fresh_value();
    let heap_v = func.fresh_value();
    let target = func.fresh_value();
    let add_result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::ConstInt, vec![], vec![local_v]));
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // local_v eliminated (no heap exposure), heap_v kept (StoreAttr).
    assert_eq!(stats.ops_removed, 2);
    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 5);
    let remaining_refs: Vec<_> = entry
        .ops
        .iter()
        .filter(|op| is_refcount_balance_op(op.opcode))
        .collect();
    assert_eq!(remaining_refs.len(), 2);
    for op in &remaining_refs {
        assert_eq!(op.operands[0], heap_v);
    }
}

#[test]
fn exception_region_drop_marker_protects_lone_decref_without_full_drop_gate() {
    let mut func = make_func();
    let v = func.fresh_value();

    func.attrs.insert(
        crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
        crate::tir::ops::AttrValue::Bool(true),
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    assert_eq!(
        stats.ops_removed, 0,
        "exception-only pre-bail drops must receive post-drop protection in refcount_elim"
    );
    assert!(
        !func
            .attrs
            .contains_key(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
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
        crate::tir::ops::AttrValue::Int(label),
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

    let stats = run_post_drop(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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
        crate::tir::ops::AttrValue::Int(label),
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

    let stats = run_post_drop(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

    let stats = run_post_drop(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

// -----------------------------------------------------------------------
// Test 20: ClosureStore causes heap exposure
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_closure_store_kept() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.ops_removed, 0);
}

// -----------------------------------------------------------------------
// Test 21: BuildList causes heap exposure for elements
// -----------------------------------------------------------------------
#[test]
fn deferred_rc_build_list_kept() {
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

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // elem has heap exposure via BuildList, Call is barrier.
    assert_eq!(stats.ops_removed, 0);
}

// -----------------------------------------------------------------------
// Step 6 (DecRefâ†’Free) FinalizerSensitive guard (task #57 / council E).
//
// A `__del__`-bearing instance must keep its finalizer-aware `DecRef`: an
// `OpCode::Free` is a direct dealloc that does NOT route through
// `maybe_run_object_finalizer`, so promoting it would silently skip `__del__`.
// Step 6 keys on `alloc_vals` (Alloc/StackAlloc results); finalizer roots are
// `ObjectNewBound` results â€” disjoint by construction â€” so the guard is
// defense-in-depth (provable-correct-by-construction). These tests pin both
// halves of that proof: the shared finalizer fact recognizes the root, and a
// finalizer-bearing DecRef is never rewritten to Free.
// -----------------------------------------------------------------------

fn make_op_with_attr(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    key: &str,
    value: crate::tir::ops::AttrValue,
) -> TirOp {
    let mut op = make_op(opcode, operands, results);
    op.attrs.insert(key.to_string(), value);
    op
}

/// The shared finalizer fact (`escape_analysis::finalizer_alloc_roots`, the
/// SAME source of truth Step 6 queries) recognizes a `defines_del`
/// `ObjectNewBound` result, and does NOT flag a `defines_del`-free one.
#[test]
fn finalizer_alloc_roots_recognizes_defines_del_object() {
    use crate::tir::ops::AttrValue;
    let mut func = make_func();
    let del_obj = func.fresh_value();
    let plain_obj = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op_with_attr(
        OpCode::ObjectNewBound,
        vec![],
        vec![del_obj],
        "defines_del",
        AttrValue::Bool(true),
    ));
    entry
        .ops
        .push(make_op(OpCode::ObjectNewBound, vec![], vec![plain_obj]));
    entry.terminator = Terminator::Return { values: vec![] };

    let roots = crate::tir::passes::escape_analysis::finalizer_alloc_roots(&func);
    assert!(
        roots.contains(&del_obj),
        "the __del__-bearing ObjectNewBound result must be a finalizer root"
    );
    assert!(
        !roots.contains(&plain_obj),
        "a finalizer-free ObjectNewBound result must NOT be a finalizer root"
    );
}

/// A finalizer-bearing instance's `DecRef` is NEVER promoted to `Free` by
/// Step 6: it must keep its finalizer-aware release so `dec_ref_ptr` dispatches
/// `__del__`. (Heap-exposed here via a `Call` barrier so Step 5 does not strip
/// the DecRef before Step 6 â€” pinning that even a Step-6-reachable finalizer
/// DecRef stays a DecRef.)
#[test]
fn step6_never_promotes_finalizer_decref_to_free() {
    use crate::tir::ops::AttrValue;
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
    // Heap-expose `del_obj` (Call may capture) so Step 5 keeps its DecRef.
    entry.ops.push(make_op(
        OpCode::Call,
        vec![callee, del_obj],
        vec![call_result],
    ));
    entry
        .ops
        .push(make_op(OpCode::DecRef, vec![del_obj], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

/// Probe: confirm Step 6 does not fire on a plain unique-owned `Alloc` value
/// either, because Step 5 (deferred-RC) deletes every non-heap-exposed
/// IncRef/DecRef BEFORE Step 6 runs, and Step 6's promotion requires exactly
/// the same `!heap_exposed` predicate. This documents that Step 6 is currently
/// unreachable in `run` mode (and skipped in `post_drop`), so the finalizer
/// guard is correct-by-construction defense-in-depth: there is NO surviving
/// DecRef for it to promote, finalizer-bearing or not.
#[test]
fn step6_unreachable_plain_alloc_decref_removed_by_step5() {
    let mut func = make_func();
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::Alloc, vec![], vec![v]));
    entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    let ops = &func.blocks[&func.entry_block].ops;
    // Step 5 strips the non-heap-exposed DecRef; nothing reaches Step 6, so no
    // Free is ever produced.
    assert!(
        !ops.iter().any(|op| op.opcode == OpCode::Free),
        "Step 5 removes the DecRef before Step 6 can promote it to Free"
    );
    assert!(
        !ops.iter().any(|op| op.opcode == OpCode::DecRef),
        "the unique-owned non-heap-exposed DecRef is removed by Step 5"
    );
}
