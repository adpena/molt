use molt_ir::deterministic_hash::HashMap;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::{classify::RaiseFacts, run};

fn make_check_exception() -> TirOp {
    let mut attrs = AttrDict::default();
    attrs.insert("value".into(), AttrValue::Int(100));
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn make_const_int(value: i64, out: ValueId) -> TirOp {
    let mut attrs = AttrDict::default();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![out],
        attrs,
        source_span: None,
    }
}

fn make_call(callee: &str, out: ValueId) -> TirOp {
    let mut attrs = AttrDict::default();
    attrs.insert("s_value".into(), AttrValue::Str(callee.to_string()));
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![out],
        attrs,
        source_span: None,
    }
}

#[test]
fn frame_region_clear_fact_survives_only_nonraising_backedges() {
    let mut func = TirFunction::new(
        "frame_region_pending".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = func.entry_block;
    let header = func.fresh_block();
    let latch = func.fresh_block();
    let fallback = func.fresh_block();
    let admitted = func.fresh_value();
    func.value_types.insert(admitted, TirType::Bool);
    let preheader = func.blocks.get_mut(&entry).unwrap();
    preheader.ops.push(TirOp::new(
        OpCode::FrameRegionAdmits,
        vec![],
        vec![admitted],
    ));
    preheader.terminator = Terminator::CondBranch {
        cond: admitted,
        then_block: header,
        then_args: vec![],
        else_block: fallback,
        else_args: vec![],
    };
    let branch = |target| Terminator::Branch {
        target,
        args: vec![],
    };
    for (id, terminator) in [
        (header, branch(latch)),
        (latch, branch(header)),
        (fallback, Terminator::Return { values: vec![] }),
    ] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator,
            },
        );
    }
    let pending = |func: &TirFunction| {
        super::flow::compute_block_entry_pending(func, &RaiseFacts::of(func), &HashMap::default())
    };
    let before = pending(&func);
    assert!(!before[&header]);
    assert!(!before[&latch]);
    assert!(before[&fallback]);

    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    func.blocks
        .get_mut(&latch)
        .unwrap()
        .ops
        .push(make_call("callback", result));
    let after = pending(&func);
    assert!(after[&header]);
    assert!(after[&latch]);
    assert!(after[&fallback]);
}

fn make_module_get_attr(module: ValueId, attr_name: ValueId, out: ValueId) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ModuleGetAttr,
        operands: vec![module, attr_name],
        results: vec![out],
        attrs: AttrDict::default(),
        source_span: None,
    }
}

fn make_binary(opcode: OpCode, lhs: ValueId, rhs: ValueId, out: ValueId) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands: vec![lhs, rhs],
        results: vec![out],
        attrs: AttrDict::default(),
        source_span: None,
    }
}

fn make_original_kind(kind: molt_ir::OpKind) -> TirOp {
    let attrs = AttrDict::default();

    TirOp {
        origin: Some(kind),
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn with_check_handler(mut function: TirFunction) -> TirFunction {
    let id = BlockId(function.next_block);
    function.next_block += 1;
    function.has_exception_handling = true;
    function.label_id_map.insert(id.0, 100);
    function.blocks.insert(
        id,
        TirBlock {
            id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    function
}

fn make_func_with_block(ops: Vec<TirOp>) -> TirFunction {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![],
        ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::default();
    blocks.insert(entry_id, block);
    with_check_handler(TirFunction {
        name: "test".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 1,
        ..TirFunction::new(
            "test".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        )
    })
}

#[test]
fn aggregate_allocation_and_callback_failures_keep_their_exception_checks() {
    for opcode in [
        OpCode::BuildList,
        OpCode::BuildTuple,
        OpCode::BuildDict,
        OpCode::BuildSet,
        OpCode::BuildSlice,
    ] {
        let mut function = make_func_with_block(vec![
            make_check_exception(),
            TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![],
                results: vec![ValueId(0)],
                attrs: AttrDict::default(),
                source_span: None,
            },
            make_check_exception(),
        ]);
        run(&mut function);
        let ops = &function.blocks[&function.entry_block].ops;
        let allocation = ops.iter().position(|op| op.opcode == opcode).unwrap();
        assert_eq!(
            ops[allocation + 1].opcode,
            OpCode::CheckException,
            "{opcode:?} can fail even with no elements"
        );
    }
}

fn make_two_block_func(entry_ops: Vec<TirOp>, successor_ops: Vec<TirOp>) -> TirFunction {
    let entry_id = BlockId(0);
    let successor_id = BlockId(1);
    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: entry_ops,
        terminator: Terminator::Branch {
            target: successor_id,
            args: vec![],
        },
    };
    let successor = TirBlock {
        id: successor_id,
        args: vec![],
        ops: successor_ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::default();
    blocks.insert(entry_id, entry);
    blocks.insert(successor_id, successor);
    with_check_handler(TirFunction {
        name: "two_block_test".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 2,
        ..TirFunction::new(
            "two_block_test".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        )
    })
}

#[test]
fn first_check_kept() {
    let mut func =
        make_func_with_block(vec![make_const_int(1, ValueId(0)), make_check_exception()]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
}

#[test]
fn redundant_check_after_pure_ops_dropped() {
    let mut func = make_func_with_block(vec![
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_const_int(2, ValueId(1)),
        make_const_int(3, ValueId(2)),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn full_width_boxing_preserves_local_and_successor_exception_edges() {
    use crate::tir::op_kinds_generated::{GvnNumberingRole, opcode_gvn_numbering_role_table};

    assert_eq!(
        opcode_gvn_numbering_role_table(OpCode::BoxVal),
        GvnNumberingRole::Never,
        "a repeated box can independently fail allocation"
    );
    for split_block in [false, true] {
        let boxed = TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::BoxVal,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs: AttrDict::default(),
            source_span: None,
        };
        let mut ops = vec![
            make_const_int(i64::MAX, ValueId(0)),
            make_check_exception(),
            boxed,
        ];
        let mut func = if split_block {
            make_two_block_func(ops, vec![make_check_exception()])
        } else {
            ops.push(make_check_exception());
            make_func_with_block(ops)
        };
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0, "split_block={split_block}");
        assert_eq!(
            func.blocks
                .values()
                .flat_map(|block| &block.ops)
                .filter(|op| op.opcode == OpCode::CheckException)
                .count(),
            2,
            "successful input materialization cannot prove boxing non-throwing"
        );
    }
}

#[test]
fn untargeted_observers_preserve_literal_and_poll_failures_locally_and_across_cfg() {
    for literal in [
        None,
        Some(OpCode::ConstStr),
        Some(OpCode::ConstBytes),
        Some(OpCode::ConstBigInt),
    ] {
        for split_block in [false, true] {
            let mut observer = make_check_exception();
            observer.attrs.remove("value");
            if literal.is_none() {
                observer.mark_async_work_poll();
            }
            let mut ops = vec![make_check_exception()];
            if let Some(opcode) = literal {
                ops.push(TirOp {
                    origin: None,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![ValueId(0)],
                    attrs: AttrDict::from_iter([("s_value".into(), AttrValue::Str("123".into()))]),
                    source_span: None,
                });
            }
            ops.push(observer);
            let mut func = if split_block {
                make_two_block_func(ops, vec![make_check_exception()])
            } else {
                ops.push(make_check_exception());
                make_func_with_block(ops)
            };
            let stats = run(&mut func);
            assert_eq!(
                stats.ops_removed, 0,
                "{literal:?} split_block={split_block}: observation does not clear pending state"
            );
            assert_eq!(
                func.blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .filter(|op| op.opcode == OpCode::CheckException)
                    .count(),
                3
            );
        }
    }
}

#[test]
fn result_bearing_checks_and_untargeted_observers_survive_clean_state() {
    for targeted in [false, true] {
        let mut observer = make_check_exception();
        observer.results.push(ValueId(0));
        if !targeted {
            observer.attrs.remove("value");
        }
        let mut func = make_func_with_block(vec![make_check_exception(), observer]);
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "observer result must retain its definition"
        );
        assert_eq!(func.blocks[&BlockId(0)].ops[1].results, vec![ValueId(0)]);
    }
    let mut observer = make_check_exception();
    observer.attrs.remove("value");
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        observer,
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(
        stats.ops_removed, 1,
        "passive observer preserves an already clean state"
    );
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
}

#[test]
fn targeted_poll_proves_clean_fallthrough_but_must_execute() {
    let mut poll = make_check_exception();
    poll.mark_async_work_poll();
    let mut func = make_two_block_func(
        vec![make_check_exception(), poll],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert!(func.blocks[&BlockId(1)].ops.is_empty());
    assert!(func.blocks[&BlockId(0)].ops[1].is_async_work_poll());
}

#[test]
fn preserved_copy_fallbacks_fail_closed_with_or_without_poll_marker() {
    let unmarked = make_original_kind(molt_ir::OpKind::ExceptionFinallyPendingObserver);
    let mut observer = make_original_kind(molt_ir::OpKind::ExceptionFinallyPendingObserver);
    observer.mark_async_work_poll();
    let probe = make_func_with_block(vec![unmarked.clone(), observer.clone()]);
    let raise = RaiseFacts::of(&probe);

    assert!(raise.op_may_raise(&unmarked));
    assert!(
        raise.op_may_raise(&observer),
        "polling can introduce a pending exception"
    );
}

#[test]
fn preserved_copy_transport_does_not_bypass_shared_effects() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_original_kind(molt_ir::OpKind::StoreVar),
        make_original_kind(molt_ir::OpKind::LoadVar),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_call_kept() {
    let mut func = make_func_with_block(vec![
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_call("foo", ValueId(1)),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_module_get_attr_is_kept() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_module_get_attr(ValueId(0), ValueId(1), ValueId(2)),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 3);
}

#[test]
fn many_redundant_checks_collapsed() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_const_int(2, ValueId(1)),
        make_check_exception(),
        make_const_int(3, ValueId(2)),
        make_check_exception(),
        make_call("foo", ValueId(3)),
        make_check_exception(),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 4);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 6);
}

#[test]
fn first_check_in_normal_successor_dropped_after_checked_predecessor() {
    let mut func = make_two_block_func(
        vec![make_check_exception()],
        vec![make_const_int(2, ValueId(1)), make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 1);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
}

#[test]
fn checked_loop_entry_through_line_drops_header_observer() {
    let mut func = make_two_block_func(
        vec![
            make_check_exception(),
            make_original_kind(molt_ir::OpKind::Line),
        ],
        vec![make_check_exception()],
    );
    func.blocks.get_mut(&BlockId(1)).unwrap().terminator = Terminator::Branch {
        target: BlockId(1),
        args: vec![],
    };
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert!(func.blocks[&BlockId(1)].ops.is_empty());
    assert!(
        func.blocks[&BlockId(0)]
            .ops
            .iter()
            .any(|op| op.origin == Some(molt_ir::OpKind::Line))
    );
}

#[test]
fn first_check_in_successor_kept_when_predecessor_may_raise() {
    let mut func = make_two_block_func(
        vec![make_call("foo", ValueId(1))],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
}

#[test]
fn exception_target_entry_remains_conservative() {
    let mut func = make_two_block_func(
        vec![make_check_exception()],
        vec![make_const_int(2, ValueId(1)), make_check_exception()],
    );
    func.label_id_map.clear();
    func.label_id_map.insert(1, 100);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 2);
}

#[test]
fn explicit_exception_clear_feeds_successor_elision() {
    let mut func = make_two_block_func(
        vec![
            make_check_exception(),
            make_original_kind(molt_ir::OpKind::ExceptionClear),
        ],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 0);
}

#[test]
fn check_after_i64_mod_by_nonzero_const_is_dropped() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_i64_floor_div_by_nonzero_const_is_dropped() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::FloorDiv, lhs, rhs, out),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn a_comparison_proven_nothrow_by_exact_types_carries_the_proof() {
    // Exact ints make `a > b` unable to raise, so its check goes. The proof
    // travels on the op: a later pass may weaken the whole-function type map
    // (GVN did, in bench_procedural_gen), and the exception-region verifier
    // must still see a nothrow op instead of a raise with no check.
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::Gt, lhs, rhs, out),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    let compare = func.blocks[&BlockId(0)]
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Gt)
        .unwrap()
        .clone();
    assert!(compare.has_nothrow_proof());
    // Facts a later pass loses: the same op, with no exact operand types.
    let weakened = make_func_with_block(vec![compare.clone()]);
    assert!(!RaiseFacts::of(&weakened).op_may_raise(&compare));

    // Unknown boxed operands may call `__gt__`: no proof, the check stays.
    let mut unknown = make_func_with_block(vec![
        make_binary(OpCode::Gt, lhs, rhs, out),
        make_check_exception(),
    ]);
    let stats = run(&mut unknown);
    assert_eq!(stats.ops_removed, 0);
    assert!(!unknown.blocks[&BlockId(0)].ops[0].has_nothrow_proof());
}

#[test]
fn a_shift_by_a_constant_machine_count_cannot_raise_and_says_so() {
    // A negative count raises ValueError and a count of 64 or more is not a
    // machine shift; a count in [0, 63] cannot raise. The proof is recorded
    // on the op so frame-observer readers (home placement) see it too.
    for (count, raises) in [(1, false), (0, false), (63, false), (-1, true), (64, true)] {
        for opcode in [OpCode::Shr, OpCode::Shl] {
            let lhs = ValueId(0);
            let rhs = ValueId(1);
            let out = ValueId(2);
            let mut func = make_func_with_block(vec![
                make_const_int(9, lhs),
                make_const_int(count, rhs),
                make_check_exception(),
                make_binary(opcode, lhs, rhs, out),
                make_check_exception(),
            ]);
            func.value_types.insert(lhs, TirType::I64);
            func.value_types.insert(rhs, TirType::I64);

            let stats = run(&mut func);

            let ops = &func.blocks[&BlockId(0)].ops;
            let shift = ops.iter().find(|op| op.opcode == opcode).unwrap();
            assert_eq!(
                stats.ops_removed,
                usize::from(!raises),
                "{opcode:?} by {count}"
            );
            assert_eq!(shift.has_nothrow_proof(), !raises, "{opcode:?} by {count}");
        }
    }
}

#[test]
fn check_after_true_div_by_nonzero_integer_const_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::Div, lhs, rhs, out),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn malformed_floor_div_result_arity_fails_closed() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let mut malformed = make_binary(OpCode::FloorDiv, lhs, rhs, ValueId(2));
    malformed.results.clear();
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        malformed,
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn check_after_i64_mod_by_zero_const_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(0, rhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn check_after_i64_mod_by_dynamic_rhs_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn class_allocation_preserves_pending_exception_checks_across_blocks() {
    let class_alloc = |opcode| {
        let mut attrs = AttrDict::default();
        attrs.insert("value".into(), AttrValue::Int(16));
        TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs,
            source_span: None,
        }
    };
    let mut generic = make_original_kind(molt_ir::OpKind::AllocClass);
    generic.operands = vec![ValueId(0)];
    generic.results = vec![ValueId(1)];
    generic.attrs.insert("value".into(), AttrValue::Int(16));
    for allocation in [class_alloc(OpCode::ObjectNewBound), generic] {
        assert!(
            RaiseFacts::of(&make_func_with_block(vec![allocation.clone()]))
                .op_may_raise(&allocation)
        );
        let mut same_block = make_func_with_block(vec![
            make_check_exception(),
            allocation.clone(),
            make_check_exception(),
        ]);
        assert_eq!(run(&mut same_block).ops_removed, 0);
        let mut across_blocks = make_two_block_func(
            vec![make_check_exception(), allocation],
            vec![make_check_exception()],
        );
        assert_eq!(run(&mut across_blocks).ops_removed, 0);
        assert_eq!(across_blocks.blocks[&BlockId(1)].ops.len(), 1);
    }
}

fn operator_index() -> TirOp {
    let mut op = make_call("molt_operator_index", ValueId(1));
    op.runtime = Some(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex);
    op.origin = Some(molt_ir::OpKind::Call);
    op.operands = vec![ValueId(0)];
    op.source_span = Some((3, 19));
    op
}

#[test]
fn operator_index_captures_an_exact_int_as_an_owned_alias_only_on_clean_entry() {
    use molt_ir::OpKind;
    for opcode in [OpCode::ConstInt, OpCode::ConstBigInt] {
        let mut integer = make_const_int(7, ValueId(0));
        integer.opcode = opcode;
        if opcode == OpCode::ConstBigInt {
            integer.attrs = AttrDict::from_iter([(
                "s_value".into(),
                AttrValue::Str("18446744073709551617".into()),
            )]);
        }
        let mut func = make_two_block_func(
            vec![integer, make_check_exception()],
            vec![operator_index(), make_check_exception()],
        );
        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1);
        let ops = &func.blocks[&BlockId(1)].ops;
        assert_eq!(ops.len(), 1);
        let alias = &ops[0];
        assert_eq!(
            (alias.opcode, alias.origin),
            (OpCode::Copy, Some(OpKind::BindingAlias))
        );
        assert!(alias.runtime.is_none() && alias.callee.is_none());
        assert_eq!(alias.operands, [ValueId(0)]);
        assert_eq!(alias.results, [ValueId(1)]);
        assert_eq!(alias.source_span, Some((3, 19)));
    }
}

#[test]
fn operator_index_preserves_pending_errors_dynamic_dispatch_and_terminal_ownership() {
    use molt_ir::OpKind;
    for refusal in [
        "entry",
        "raising op",
        "unresolved check",
        "handler",
        "bool",
        "annotation",
        "transferred argument",
        "compiled callee",
        "callable object",
        "terminal",
    ] {
        let mut first = make_const_int(1, ValueId(0));
        if refusal == "bool" {
            first.opcode = OpCode::ConstBool;
            first.attrs = AttrDict::from_iter([("value".into(), AttrValue::Bool(true))]);
        }
        let mut prefix = vec![first];
        if refusal != "entry" {
            prefix.push(make_check_exception());
        }
        if refusal == "raising op" {
            prefix.push(make_call("unknown", ValueId(3)));
        }
        let mut call = operator_index();
        match refusal {
            "transferred argument" => {
                call.set_argument_custody(&[molt_ir::ir::ParameterCustody::Transferred])
            }
            "compiled callee" => call.callee = Some(molt_ir::FunctionId(0)),
            "callable object" => call.origin = Some(OpKind::BuiltinFunc),
            _ => {}
        }
        let mut func = make_two_block_func(prefix, vec![call, make_check_exception()]);
        match refusal {
            "unresolved check" => func.label_id_map.clear(),
            "handler" => {
                func.label_id_map.clear();
                func.label_id_map.insert(1, 100);
            }
            "annotation" => {
                let entry = func.blocks.get_mut(&BlockId(0)).unwrap();
                entry.ops.remove(0);
                entry.args.push(crate::tir::values::TirValue {
                    id: ValueId(0),
                    ty: TirType::I64,
                });
                func.param_names.push("n".into());
                func.param_types.push(TirType::I64);
            }
            _ => {}
        }
        if refusal == "terminal" {
            super::run_post_drop(&mut func);
        } else {
            run(&mut func);
        }
        assert_eq!(
            func.blocks[&BlockId(1)].ops[0].opcode,
            OpCode::Call,
            "{refusal}"
        );
    }
}
