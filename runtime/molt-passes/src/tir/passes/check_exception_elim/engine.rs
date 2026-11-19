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
        for mut op in block.ops.drain(..) {
            if raise.guarded_raise_disproven(&op) {
                op.mark_nothrow_proof();
                stats.facts_changed += 1;
            }
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
                    } else if raise.exact_types_disprove_raise(&op) {
                        op.mark_nothrow_proof();
                        stats.facts_changed += 1;
                    }
                    new_ops.push(op);
                }
            }
        }
        block.ops = new_ops;
    }

    stats
}

/// The same elimination after the terminal drop phase, where frame-home
/// placement has proven home writes nothrow. A deleted check can leave its
/// handler with no way in; such blocks go too, as `dce` removes them after the
/// default pipeline's run.
pub fn run_post_drop(func: &mut TirFunction) -> PassStats {
    let mut stats = run(func);
    stats.name = "check_exception_elim_post_drop";
    if stats.ops_removed > 0 {
        let reachable = super::super::reachability::metadata_preserving_reachable_blocks(func);
        stats.ops_removed += func
            .retain_blocks(&reachable)
            .expect("check elimination reachability preserves live block references");
    }
    stats
}
