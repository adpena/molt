use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;

use super::super::PassStats;
use super::classify::{
    RaiseFacts, check_has_exception_edge, op_clears_pending_exception, pending_after_check,
};
use super::flow::compute_block_entry_pending;

/// On a clean entry operator.index retains an exact Python int and returns that
/// same object, including a heap-backed large integer. With a pending exception
/// it returns the exception sentinel instead.
fn forwarded_index(op: &TirOp, facts: &RaiseFacts) -> Option<TirOp> {
    let [argument] = op.operands.as_slice() else {
        return None;
    };
    let [result] = op.results.as_slice() else {
        return None;
    };
    if op.is_async_work_poll()
        || op.runtime != Some(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex)
        || op.operand_custody(0) != molt_ir::ir::ParameterCustody::Borrowed
        || crate::tir::call_targets::direct_boxed_runtime(op).is_none()
        || !matches!(
            facts.exact_types().get(argument),
            Some(TirType::I64 | TirType::BigInt)
        )
    {
        return None;
    }
    Some(super::super::ownership_lattice_min::owned_alias(
        *argument,
        *result,
        op.source_span,
    ))
}

pub fn run(func: &mut TirFunction) -> PassStats {
    run_with_pending(func, true)
}

fn run_with_pending(func: &mut TirFunction, before_drop: bool) -> PassStats {
    let mut stats = PassStats {
        name: "check_exception_elim",
        ..Default::default()
    };

    let raise = RaiseFacts::of(func);
    let mut labels = crate::tir::dominators::exception_label_to_block(func);
    labels.retain(|_, target| func.blocks.contains_key(target));
    let entry_pending = compute_block_entry_pending(func, &raise, &labels);

    for block in func.blocks.values_mut() {
        let mut pending_exception_possible = entry_pending.get(&block.id).copied().unwrap_or(false);
        let mut new_ops = Vec::with_capacity(block.ops.len());
        for mut op in block.ops.drain(..) {
            if before_drop
                && !pending_exception_possible
                && let Some(alias) = forwarded_index(&op, &raise)
            {
                op = alias;
                stats.values_changed += 1;
            }
            if raise.guarded_raise_disproven(&op) {
                op.mark_nothrow_proof();
                stats.facts_changed += 1;
            }
            match op.opcode {
                OpCode::CheckException => {
                    let has_edge = check_has_exception_edge(&op, &labels);
                    let keep = pending_exception_possible
                        || op.is_async_work_poll()
                        || !op.results.is_empty()
                        || !has_edge;
                    pending_exception_possible =
                        pending_after_check(has_edge, &op, pending_exception_possible);
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
    let mut stats = run_with_pending(func, false);
    stats.name = "check_exception_elim_post_drop";
    if stats.ops_removed > 0 {
        let reachable = super::super::reachability::metadata_preserving_reachable_blocks(func);
        stats.ops_removed += func
            .retain_blocks(&reachable)
            .expect("check elimination reachability preserves live block references");
    }
    stats
}
