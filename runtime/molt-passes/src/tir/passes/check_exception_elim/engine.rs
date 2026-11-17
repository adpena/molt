use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;

use super::super::PassStats;
use super::classify::{
    RaiseFacts, check_has_exception_edge, op_clears_pending_exception, pending_after_check,
};
use super::flow::compute_block_entry_pending;

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "check_exception_elim",
        ..Default::default()
    };

    let raise = RaiseFacts::of(func);
    let entry_pending = compute_block_entry_pending(func, &raise);

    for block in func.blocks.values_mut() {
        let mut pending_exception_possible = entry_pending.get(&block.id).copied().unwrap_or(false);
        let mut new_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op.opcode {
                OpCode::CheckException => {
                    let keep = pending_exception_possible
                        || op.is_async_work_poll()
                        || !op.results.is_empty()
                        || !check_has_exception_edge(&op);
                    pending_exception_possible =
                        pending_after_check(&op, pending_exception_possible);
                    if keep {
                        new_ops.push(op);
                    } else {
                        stats.ops_removed += 1;
                    }
                }
                _ => {
                    if op_clears_pending_exception(&op) {
                        pending_exception_possible = false;
                    } else if raise.op_may_raise(&op) {
                        pending_exception_possible = true;
                    }
                    new_ops.push(op);
                }
            }
        }
        block.ops = new_ops;
    }

    stats
}
