//! Guard branches and the physical-call continuation for a packed wrapper.

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

/// Add one read-only test to a guarded path. The test's final result names
/// the predicate; any earlier results are available only on the accepted path.
pub(super) fn test_block(
    caller: &mut TirFunction,
    mut ops: Vec<TirOp>,
    test: TirOp,
    accepted: BlockId,
    refused: BlockId,
) -> BlockId {
    let predicate = *test
        .results
        .last()
        .expect("a guarded test yields a predicate");
    ops.push(test);
    caller.value_types.insert(predicate, TirType::Bool);
    let id = caller.fresh_block();
    caller.blocks.insert(
        id,
        TirBlock {
            id,
            args: vec![],
            ops,
            terminator: Terminator::CondBranch {
                cond: predicate,
                then_block: accepted,
                then_args: vec![],
                else_block: refused,
                else_args: vec![],
            },
        },
    );
    id
}

/// The miss retains the original physical invocation followed immediately by
/// its own check. Its normal path and the fast check's normal path rejoin
/// after both observations, sharing the original result binding.
pub(super) fn ordinary_call(
    caller: &mut TirFunction,
    miss: BlockId,
    checked_hit: BlockId,
    mut call: TirOp,
) {
    let tail = caller.fresh_block();
    let joined = call.results[0];
    let hot = caller.fresh_value();
    let cold = caller.fresh_value();
    let block = caller
        .blocks
        .get_mut(&checked_hit)
        .expect("checked continuation");
    let ty = block.args[0].ty.clone();
    caller.value_types.insert(hot, ty.clone());
    caller.value_types.insert(cold, ty.clone());
    block.args[0].id = hot;
    let rest = block.ops.split_off(1);
    let check = block.ops.first_mut().expect("validated caller Origin");
    // Ordinary placement runs after module inlining on some targets.
    if super::super::async_work_poll::is_call_return_poll(&call) {
        check.mark_async_work_poll();
    }
    let mut original_check = check.clone();
    for value in &mut original_check.operands {
        if *value == joined {
            *value = cold;
        }
    }
    for value in &mut check.operands {
        if *value == joined {
            *value = hot;
        }
    }
    let term = std::mem::replace(
        &mut block.terminator,
        Terminator::Branch {
            target: tail,
            args: vec![hot],
        },
    );
    call.results[0] = cold;
    caller.blocks.insert(
        miss,
        TirBlock {
            id: miss,
            args: vec![],
            ops: vec![call, original_check],
            terminator: Terminator::Branch {
                target: tail,
                args: vec![cold],
            },
        },
    );
    caller.blocks.insert(
        tail,
        TirBlock {
            id: tail,
            args: vec![TirValue { id: joined, ty }],
            ops: rest,
            terminator: term,
        },
    );
    caller.move_loop_terminator(checked_hit, tail);
}
