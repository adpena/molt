use std::collections::HashSet;

use crate::tir::analysis::{Analysis, LoopForest, LoopForestResult};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::ordered_comparison_trip_count;
use crate::tir::op_kinds_generated::{
    opcode_counted_loop_comparison_role_table, opcode_counted_loop_inverted_comparison_table,
};
use crate::tir::ops::OpCode;

use super::super::value_identity::{build_copy_map, resolve_copy};
use super::descriptor::CountedLoop;
use super::facts::{branch_args_to, build_const_int_map, find_def, loop_forest_contains_header};
use super::gate::{CmpPolarity, LoopGate, loop_gate};
use super::paths::{loop_body_path, loop_guard_path};

/// Recognize a counted loop rooted at `header`, or refuse with `None`.
///
/// `header` must be a LoopForest header. The caller is responsible for
/// iterating headers in a deterministic order.
pub fn recognize_counted_loop(func: &TirFunction, header: BlockId) -> Option<CountedLoop> {
    let loop_forest = <LoopForest as Analysis>::compute(func);
    recognize_counted_loop_with_loop_forest(func, header, &loop_forest)
}

/// Recognize a counted loop using the caller-provided canonical LoopForest.
pub(crate) fn recognize_counted_loop_with_loop_forest(
    func: &TirFunction,
    header: BlockId,
    loop_forest: &LoopForestResult,
) -> Option<CountedLoop> {
    macro_rules! trace {
        ($($a:tt)*) => {
            if std::env::var("MOLT_DEBUG_COUNTED_LOOP").is_ok() {
                let _ = crate::debug_artifacts::append_debug_artifact(
                    "counted_loop_trace.txt",
                    format!("[counted_loop {:?} fn={}] {}\n", header, func.name, format!($($a)*)),
                );
            }
        };
    }
    if !loop_forest_contains_header(loop_forest, header) {
        return None;
    }
    trace!("BEGIN recognition");
    let header_block = func.blocks.get(&header)?;

    let natural_body = loop_forest.bodies.get(&header)?;
    // Prefer a material guard. Only terminal structured loops need the
    // frontend's non-material break boundary as a fallback.
    let guard_path = loop_guard_path(func, header, natural_body, None).or_else(|| {
        func.loop_break_kinds.get(&header)?;
        loop_guard_path(
            func,
            header,
            natural_body,
            func.loop_cond_blocks.get(&header).copied(),
        )
    })?;
    let cond_block_id = *guard_path.last()?;
    trace!("cond_block = {:?}", cond_block_id);

    // Metadata may name an earlier comparison block split from its terminator.
    if let Some(&meta_cond) = func.loop_cond_blocks.get(&header)
        && !guard_path.contains(&meta_cond)
    {
        trace!(
            "meta cond {:?} != structural cond {:?}",
            meta_cond, cond_block_id
        );
        return None;
    }

    // The loop gate is usually a material `CondBranch(cond, body, exit)`, but a
    // terminal structured loop can have no material post-loop block. In that
    // shape the CFG has only the continue edge, while `loop_cond_blocks` and
    // `loop_break_kinds` still preserve the SimpleIR loop-break condition.
    let Some(gate) = loop_gate(func, header, &guard_path, natural_body) else {
        trace!("cond block is not a counted-loop gate");
        return None;
    };
    let LoopGate {
        cmp_cond,
        cmp_polarity,
        body_id,
        exit_id,
        exit_args,
        has_material_exit,
    } = gate;

    let body_path = loop_body_path(func, header, body_id, natural_body)?;
    let latch = *body_path.last()?;
    let region: HashSet<_> = guard_path.iter().chain(&body_path).copied().collect();
    // Every executable in-loop route must be represented once. This rejects
    // handler reentry, bypasses and nested loops instead of assigning them the
    // normal path's recurrence. External exception exits only truncate it.
    if region.len() != guard_path.len() + body_path.len() || &region != natural_body {
        return None;
    }
    let reachable = dominators::executable_reachable_blocks(func);
    let preds = dominators::build_pred_map(func);
    let labels = dominators::exception_label_to_block(func);
    let mut has_side_exits = false;
    for &bid in &region {
        let block = func.blocks.get(&bid)?;
        if bid != header
            && (!block.args.is_empty()
                || loop_forest_contains_header(loop_forest, bid)
                || preds
                    .get(&bid)?
                    .iter()
                    .any(|p| reachable.contains(p) && !region.contains(p)))
        {
            return None;
        }
        if let Terminator::Branch { target, args } = &block.terminator
            && *target != header
            && !args.is_empty()
        {
            return None;
        }
        for target in dominators::exception_successors(block, &labels) {
            if region.contains(&target) {
                return None;
            }
            has_side_exits = true;
        }
    }

    let const_map = build_const_int_map(func);
    let copy_of = build_copy_map(func);

    // The comparison defines `cmp_cond`. It must be Lt/Le/Gt/Ge(iv_view, stop).
    let Some((cmp_block, cmp_op)) = find_def(func, cmp_cond) else {
        trace!("no op defines the cond {:?}", cmp_cond);
        return None;
    };
    if !guard_path.contains(&cmp_block) {
        return None;
    }
    let cmp_role = opcode_counted_loop_comparison_role_table(cmp_op.opcode);
    if !cmp_role.is_ordered() {
        trace!("cond op is {:?}, not a comparison", cmp_op.opcode);
        return None;
    }
    let cmp_kind = match cmp_polarity {
        CmpPolarity::AsWritten => cmp_op.opcode,
        CmpPolarity::Inverted => opcode_counted_loop_inverted_comparison_table(cmp_op.opcode)?,
    };
    let cmp_role = opcode_counted_loop_comparison_role_table(cmp_kind);
    if cmp_op.operands.len() != 2 {
        return None;
    }

    let cmp_lhs_root = resolve_copy(&copy_of, cmp_op.operands[0]);
    let Some(iv_arg_index) = header_block.args.iter().position(|a| a.id == cmp_lhs_root) else {
        trace!(
            "cmp lhs {:?} (root {:?}) is not a header arg",
            cmp_op.operands[0], cmp_lhs_root
        );
        return None;
    };
    let induction_var = header_block.args[iv_arg_index].id;
    let Some(&stop) = const_map.get(&resolve_copy(&copy_of, cmp_op.operands[1])) else {
        trace!(
            "cmp rhs {:?} is not a ConstInt (runtime bound)",
            cmp_op.operands[1]
        );
        return None;
    };

    // The latch must end with the back-edge `Branch -> header(back_args)` with
    // one arg per header block-arg.
    let body_block = func.blocks.get(&latch)?;
    let back_args = match &body_block.terminator {
        Terminator::Branch { target, args }
            if *target == header && args.len() == header_block.args.len() =>
        {
            args.clone()
        }
        _ => {
            trace!("body terminator is not the expected back-edge Branch");
            return None;
        }
    };

    // The back-edge value for the IV slot must be `Add(iv_view, step_const)`,
    // resolving copies on the back-edge value and on the Add's IV operand.
    let iv_next_root = resolve_copy(&copy_of, back_args[iv_arg_index]);
    let Some((def_block, inc_op)) = find_def(func, iv_next_root) else {
        trace!("no def for IV-next {:?}", iv_next_root);
        return None;
    };
    if !body_path.contains(&def_block) {
        return None;
    }
    if inc_op.opcode != OpCode::Add || inc_op.operands.len() != 2 {
        trace!("IV-next def is {:?}, not a binary Add", inc_op.opcode);
        return None;
    }
    if resolve_copy(&copy_of, inc_op.operands[0]) != induction_var {
        trace!("IV-next Add lhs does not resolve to the IV");
        return None;
    }
    let Some(&step) = const_map.get(&resolve_copy(&copy_of, inc_op.operands[1])) else {
        trace!("IV step is not a ConstInt");
        return None;
    };
    if step == 0 {
        return None;
    }

    let polarity_ok = if cmp_role.requires_positive_step() {
        step > 0
    } else {
        step < 0
    };
    if !polarity_ok {
        trace!("polarity mismatch: cmp {:?} step {}", cmp_kind, step);
        return None;
    }

    // Exactly one reachable preheader and one back-edge. Dead structural blocks
    // still branching to the header are excluded via executable reachability.
    // An implicit entry to the header has no recurrence argument payload.
    if preds.get(&header)?.iter().any(|pred| {
        reachable.contains(pred)
            && dominators::exception_successors(&func.blocks[pred], &labels).contains(&header)
    }) {
        return None;
    }
    let mut preheader: Option<BlockId> = None;
    let mut preheader_count = 0usize;
    let mut backedge_count = 0usize;
    let mut start: Option<i64> = None;
    for (&pred_id, pred_block) in &func.blocks {
        if !reachable.contains(&pred_id) {
            continue;
        }
        let Some(pred_args) = branch_args_to(&pred_block.terminator, header) else {
            continue;
        };
        // A degenerate conditional can target the header twice with different
        // payloads. A predecessor count alone is not a unique initial state.
        let mut conflicting_payload = false;
        pred_block.terminator.for_each_edge(|target, args| {
            if target == header && args != pred_args {
                conflicting_payload = true;
            }
        });
        if conflicting_payload {
            return None;
        }
        if pred_id == latch {
            backedge_count += 1;
            continue;
        }
        preheader_count += 1;
        preheader = Some(pred_id);
        if pred_args.len() == header_block.args.len() {
            start = const_map
                .get(&resolve_copy(&copy_of, pred_args[iv_arg_index]))
                .copied();
        }
    }
    if preheader_count != 1 || backedge_count != 1 {
        trace!(
            "preheader_count={} backedge_count={} (need 1/1)",
            preheader_count, backedge_count
        );
        return None;
    }
    let preheader = preheader?;
    let Some(start) = start else {
        trace!("preheader IV-slot arg is not a ConstInt start");
        return None;
    };

    let trip_count = ordered_comparison_trip_count(cmp_role, start, stop, step)?;
    trace!(
        "RECOGNIZED: iv_idx={} start={} stop={} step={} trip={}",
        iv_arg_index, start, stop, step, trip_count
    );
    if trip_count < 0 {
        return None;
    }

    Some(CountedLoop {
        header,
        cond_block: cond_block_id,
        body: body_id,
        guard_path,
        body_path,
        has_side_exits,
        exit: exit_id,
        preheader,
        iv_arg_index,
        induction_var,
        start,
        step,
        trip_count,
        exit_args,
        has_material_exit,
        back_args,
        loop_pairs_end: func.loop_pairs.get(&header).copied(),
    })
}
