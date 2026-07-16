use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;

use super::super::PassStats;
use super::classify::{const_int_values, op_clears_pending_exception, op_may_raise};
use super::flow::compute_block_entry_pending;

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "check_exception_elim",
        ..Default::default()
    };

    let entry_pending = compute_block_entry_pending(func);
    let const_ints = const_int_values(func);
    let value_types = func.value_types.clone();

    for block in func.blocks.values_mut() {
        let mut pending_exception_possible = entry_pending.get(&block.id).copied().unwrap_or(false);
        let mut new_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op.opcode {
                OpCode::CheckException => {
                    let async_work_poll = op.is_async_work_poll();
                    if pending_exception_possible || async_work_poll {
                        pending_exception_possible = false;
                        new_ops.push(op);
                    } else {
                        stats.ops_removed += 1;
                    }
                }
                _ => {
                    if op_clears_pending_exception(&op) {
                        pending_exception_possible = false;
                    } else if op_may_raise(&value_types, &const_ints, &op) {
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
