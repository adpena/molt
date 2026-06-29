//! Counted-loop recognition — the canonical counted-loop *contract* (L4).
//!
//! ## Why this module exists
//!
//! The frontend lowers `for i in range(start, stop, step):` to a *counted*
//! arithmetic loop with **no** iterator protocol op (`range_devirt` never fires
//! on it — there is no `CallBuiltin("range")`/`GetIter`/`IterNextUnboxed` to
//! match). The real SSA shape it produces is:
//!
//! ```text
//! preheader:                      // unique reachable non-back-edge pred of H
//!   ... Branch -> H(start, acc0, ...)
//!
//! H (header):  args = [iv, acc, ...]   // MULTI-arg: the IV *and* every
//!   Branch -> C                        // loop-carried value (accumulators)
//!
//! C (cond block, == loop_cond_blocks[H]):
//!   iv_view = Copy(iv)                 // frontend stack-machine copy chains
//!   cond    = Lt|Le|Gt|Ge(iv_view, stop_const)
//!   CondBranch(cond, Body, Exit)
//!
//! Body:
//!   ... user ops (may read iv_view defined in C) ...
//!   iv_next  = Add(iv_view, step_const)
//!   acc_next = <loop-carried update>
//!   Branch -> H(iv_next, acc_next, ...)   // back-edge
//! ```
//!
//! This is the OPPOSITE of the textbook "1-arg header with the comparison in
//! the header" shape the historical `loop_unroll` detector required, which is
//! why `loop_unroll` was inert on every real counted loop (verified 2026-06-04
//! in `docs/design/foundation/04_L4-loops.md` §"CORRECTION"). A literal
//! canonicalization *to* a 1-arg header is impossible: a loop-carried
//! accumulator is intrinsically a header block-argument in SSA. The structurally
//! correct fix (Route B) is to recognize the real multi-arg shape natively and
//! expose it as ONE canonical descriptor that loop transforms consume.
//!
//! ## What this module provides
//!
//! [`recognize_counted_loop`] returns a [`CountedLoop`] for a loop header whose
//! trip count is a compile-time constant, or `None` (a principled refusal — the
//! caller leaves the loop untouched; it is never miscompiled). The descriptor
//! carries the induction variable, the loop-carried block-arg set, the resolved
//! `(start, stop, step, trip_count)`, and the region blocks. It is the single
//! source of truth a downstream transform (`loop_unroll` today; range/SIMD/IV
//! strength-reduction later) reads — no transform re-derives the trip count or
//! re-classifies the IV.
//!
//! ## Soundness boundaries (what is deliberately refused)
//!
//! * **Header → cond-block must be a single unconditional edge.** Loops with
//!   guard blocks interposed between the header and the comparison block are
//!   refused (the chain walk is not modelled here). The frontend counted loop
//!   has a direct `H -> C` edge.
//! * **Exactly one reachable preheader and one back-edge.** Dead structural
//!   blocks (e.g. the `LoopEnd` marker, which has no terminator predecessor yet
//!   still branches to the header) are excluded via terminator-only
//!   reachability — they must NOT be miscounted as a second preheader.
//! * **The IV arg must increment by a non-zero compile-time constant** and the
//!   comparison polarity must agree with the step sign. A degree-≥2 or
//!   non-constant recurrence is refused.
//! * **No nested loop** inside the region (`Body`/`C` must not themselves be
//!   loop headers).
//!
//! Representation note (bug #15): this module derives only the *structural*
//! trip count. It does NOT change how any loop-carried value is represented; a
//! consumer that materialises iteration values must keep each value's `Repr`
//! classification (an unbounded accumulator stays `MaybeBigInt`). The transform
//! in `loop_unroll` only fires for trip counts within the cost-model cap, so the
//! per-iteration induction constants it emits are themselves small.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, LoopForest, LoopForestResult};
use crate::tir::blocks::{BlockId, LoopBreakKind, Terminator};
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::ordered_comparison_trip_count;
use crate::tir::op_kinds_generated::{
    opcode_counted_loop_comparison_role_table, opcode_counted_loop_inverted_comparison_table,
};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::value_identity::{build_copy_map, resolve_copy};

struct LoopGate {
    cmp_cond: ValueId,
    cmp_polarity: CmpPolarity,
    body_id: BlockId,
    exit_id: BlockId,
    exit_args: Vec<ValueId>,
    has_material_exit: bool,
}

#[derive(Clone, Copy)]
enum CmpPolarity {
    AsWritten,
    Inverted,
}

/// A recognized counted loop with a compile-time-constant trip count.
///
/// The induction variable and every loop-carried value are header block-args;
/// `iv_arg_index` selects the IV among them. A transform threads the carried
/// values (all indices `!= iv_arg_index`) through each iteration while the IV
/// takes the constant value `start + k*step` on iteration `k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountedLoop {
    /// The loop header block (a pure phi block: `Branch -> cond_block`).
    pub header: BlockId,
    /// The block holding the loop-exit comparison + `CondBranch(body, exit)`.
    /// Equal to `header` only in the legacy 1-arg synthesized shape where the
    /// comparison lives in the header itself.
    pub cond_block: BlockId,
    /// The loop body block (the `CondBranch` successor that loops back).
    pub body: BlockId,
    /// The exit block (the `CondBranch` successor that does not loop back).
    pub exit: BlockId,
    /// The unique reachable preheader (non-back-edge predecessor of `header`).
    pub preheader: BlockId,
    /// Index into `header.args` of the induction variable.
    pub iv_arg_index: usize,
    /// The induction-variable `ValueId` (`header.args[iv_arg_index].id`).
    pub induction_var: ValueId,
    /// Start value of the induction variable (preheader-provided constant).
    pub start: i64,
    /// Step per iteration (non-zero compile-time constant).
    pub step: i64,
    /// Trip count (number of iterations; always `> 0`).
    pub trip_count: i64,
    /// The exit-edge argument list on the cond block's `CondBranch` (the values
    /// forwarded to `exit`). May reference header args (loop-carried values) or
    /// the IV — a transform substitutes those with their final-iteration values.
    pub exit_args: Vec<ValueId>,
    /// True when `exit` is a real CFG successor of the loop guard. Terminal
    /// structured loops may preserve their break predicate in metadata even when
    /// there is no material post-loop block; range analysis can consume that
    /// proof, but transforms that must branch to an exit block must refuse it.
    pub has_material_exit: bool,
    /// The back-edge argument list on the body's `Branch -> header` (the values
    /// forwarded to the header for the next iteration). `back_args[k]` fills
    /// `header.args[k]`.
    pub back_args: Vec<ValueId>,
    /// The structural `LoopEnd` marker block paired with this header
    /// (`loop_pairs[header]`), if any. The frontend leaves this as an
    /// unreachable dead block; a transform that unrolls the loop away must drop
    /// its now-orphaned `LoopEnd` role so the TIR→SimpleIR back-conversion does
    /// not see a `LoopEnd` without a matching `LoopHeader`.
    pub loop_pairs_end: Option<BlockId>,
}

/// Whole-function map of `ValueId -> i64` for every `ConstInt` definition.
fn build_const_int_map(func: &TirFunction) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && op.results.len() == 1
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                map.insert(op.results[0], *v);
            }
        }
    }
    map
}

/// Locate the op defining `value` anywhere in the function (block, op).
fn find_def(func: &TirFunction, value: ValueId) -> Option<(BlockId, &crate::tir::ops::TirOp)> {
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.results.first() == Some(&value) {
                return Some((bid, op));
            }
        }
    }
    None
}

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

    // The header must be a pure phi block whose sole successor is the cond
    // block: `Branch -> cond_block`. (In the legacy synthesized shape the
    // header IS the cond block and ends in the CondBranch directly — handled
    // by allowing cond_block == header below.)
    let (cond_block_id, cond_block) = match &header_block.terminator {
        Terminator::Branch { target, args } if args.is_empty() => {
            let cb = func.blocks.get(target)?;
            (*target, cb)
        }
        Terminator::CondBranch { .. } => (header, header_block),
        _ => {
            trace!("header terminator not Branch/CondBranch");
            return None;
        }
    };
    trace!("cond_block = {:?}", cond_block_id);

    // When the cond block is a separate block, it must not be a loop header
    // itself (which would mean we walked into a nested loop).
    if cond_block_id != header && loop_forest_contains_header(loop_forest, cond_block_id) {
        return None;
    }
    // Cross-check against the frontend-recorded cond block when present: if the
    // metadata names a different block, our structural pick is suspect — refuse
    // rather than risk picking the wrong comparison.
    if let Some(&meta_cond) = func.loop_cond_blocks.get(&header)
        && meta_cond != cond_block_id
    {
        trace!(
            "meta cond {:?} != structural cond {:?}",
            meta_cond, cond_block_id
        );
        return None;
    }

    // The loop gate is usually a material `CondBranch(cond, body, exit)`, but a
    // terminal structured loop can have no material post-loop block. In that
    // shape the CFG has only the continue edge, while `loop_cond_blocks` +
    // `loop_break_kinds` still preserve the SimpleIR loop-break condition. Use
    // one TIR counted-loop authority for both shapes so value-range, unroll, and
    // representation planning do not grow parallel loop recognizers.
    let Some(gate) = loop_gate(func, header, cond_block_id, cond_block) else {
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

    // No nested loop: the body must not itself be a loop header.
    if loop_forest_contains_header(loop_forest, body_id) {
        trace!("body {:?} is a nested loop header", body_id);
        return None;
    }

    let const_map = build_const_int_map(func);
    let copy_of = build_copy_map(func);

    // The comparison defines `cmp_cond`. It must be Lt/Le/Gt/Ge(iv_view, stop).
    let Some(cmp_op) = cond_block
        .ops
        .iter()
        .find(|op| op.results.first() == Some(&cmp_cond))
    else {
        trace!("no op defines the cond {:?}", cmp_cond);
        return None;
    };
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
    // LHS resolves (through copies) to a header arg → the IV. RHS resolves to a
    // ConstInt → the stop bound.
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

    // The body must end with the back-edge `Branch -> header(back_args)` with
    // one arg per header block-arg.
    let body_block = func.blocks.get(&body_id)?;
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

    // The back-edge value for the IV slot must be `Add(iv_view, step_const)`
    // (resolving copies on the back-edge value and on the Add's IV operand).
    let iv_next_root = resolve_copy(&copy_of, back_args[iv_arg_index]);
    let Some((_def_block, inc_op)) = find_def(func, iv_next_root) else {
        trace!("no def for IV-next {:?}", iv_next_root);
        return None;
    };
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

    // Comparison polarity must match the step sign (a non-terminating or
    // backward-counting mismatch is refused rather than assigned a bogus trip).
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
    // (e.g. the LoopEnd marker) still branch to the header but are unreachable;
    // they must be excluded via terminator-only reachability so they are not
    // miscounted as a second preheader.
    let reachable = dominators::reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly);
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
        if pred_id == body_id {
            backedge_count += 1;
            continue;
        }
        preheader_count += 1;
        preheader = Some(pred_id);
        // The preheader must supply a constant start for the IV slot.
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
    if trip_count <= 0 {
        return None;
    }

    Some(CountedLoop {
        header,
        cond_block: cond_block_id,
        body: body_id,
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

fn loop_forest_contains_header(loop_forest: &LoopForestResult, header: BlockId) -> bool {
    loop_forest
        .headers
        .binary_search_by_key(&header.0, |b| b.0)
        .is_ok()
}

fn loop_gate(
    func: &TirFunction,
    header: BlockId,
    cond_block_id: BlockId,
    cond_block: &crate::tir::blocks::TirBlock,
) -> Option<LoopGate> {
    match &cond_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let then_loops = block_loops_back_to(func, *then_block, header);
            let else_loops = block_loops_back_to(func, *else_block, header);
            match (then_loops, else_loops) {
                (true, false) => {
                    if !then_args.is_empty() {
                        return None;
                    }
                    Some(LoopGate {
                        cmp_cond: *cond,
                        cmp_polarity: CmpPolarity::AsWritten,
                        body_id: *then_block,
                        exit_id: *else_block,
                        exit_args: else_args.clone(),
                        has_material_exit: true,
                    })
                }
                (false, true) => {
                    if !else_args.is_empty() {
                        return None;
                    }
                    Some(LoopGate {
                        cmp_cond: *cond,
                        cmp_polarity: CmpPolarity::Inverted,
                        body_id: *else_block,
                        exit_id: *then_block,
                        exit_args: then_args.clone(),
                        has_material_exit: true,
                    })
                }
                _ => None,
            }
        }
        Terminator::Branch { target, args } if args.is_empty() => {
            structured_terminal_loop_gate(func, header, cond_block_id, cond_block, *target)
        }
        _ => None,
    }
}

fn structured_terminal_loop_gate(
    func: &TirFunction,
    header: BlockId,
    cond_block_id: BlockId,
    cond_block: &crate::tir::blocks::TirBlock,
    body_id: BlockId,
) -> Option<LoopGate> {
    if func.loop_cond_blocks.get(&header).copied() != Some(cond_block_id) {
        return None;
    }
    if !block_loops_back_to(func, body_id, header) {
        return None;
    }
    let break_kind = func.loop_break_kinds.get(&header)?;
    let cmp_cond = unique_loop_guard_cmp_cond(cond_block)?;
    let cmp_polarity = match break_kind {
        LoopBreakKind::BreakIfFalse => CmpPolarity::AsWritten,
        LoopBreakKind::BreakIfTrue => CmpPolarity::Inverted,
    };
    Some(LoopGate {
        cmp_cond,
        cmp_polarity,
        body_id,
        exit_id: func
            .loop_pairs
            .get(&header)
            .copied()
            .unwrap_or(cond_block_id),
        exit_args: Vec::new(),
        has_material_exit: false,
    })
}

fn unique_loop_guard_cmp_cond(cond_block: &crate::tir::blocks::TirBlock) -> Option<ValueId> {
    let mut guard: Option<ValueId> = None;
    for op in &cond_block.ops {
        if opcode_counted_loop_comparison_role_table(op.opcode).is_ordered()
            && op.results.len() == 1
            && guard.replace(op.results[0]).is_some()
        {
            return None;
        }
    }
    guard
}

/// True if `block` unconditionally branches back to `header` (the back-edge).
fn block_loops_back_to(func: &TirFunction, block: BlockId, header: BlockId) -> bool {
    func.blocks.get(&block).is_some_and(
        |b| matches!(&b.terminator, Terminator::Branch { target, .. } if *target == header),
    )
}

/// The argument list `term` passes to `target` along whichever edge reaches it,
/// or `None` if `term` does not branch to `target`.
fn branch_args_to(term: &Terminator, target: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target: t, args } if *t == target => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == target {
                Some(then_args.as_slice())
            } else if *else_block == target {
                Some(else_args.as_slice())
            } else {
                None
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        }
        | Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            if *default == target {
                Some(default_args.as_slice())
            } else {
                cases
                    .iter()
                    .find_map(|(_, b, args)| (*b == target).then_some(args.as_slice()))
            }
        }
        _ => None,
    }
}

/// The set of blocks that make up the loop region between `header` and the
/// back-edge: `{header, cond_block, body}`. Used by a transform to decide which
/// blocks to retire when fully unrolling. (Header → cond is a single edge by
/// construction; there are no interposed guard blocks in a recognized loop.)
pub fn region_blocks(loop_info: &CountedLoop) -> HashSet<BlockId> {
    let mut set = HashSet::new();
    set.insert(loop_info.header);
    set.insert(loop_info.cond_block);
    set.insert(loop_info.body);
    set
}
