//! Loop Unrolling Pass for TIR.
//!
//! Fully unrolls counted loops with a compile-time-constant trip count by
//! materialising one straight-line copy of the loop region per iteration, with
//! the induction variable replaced by its per-iteration constant and every
//! loop-carried value (accumulators) threaded through the chain of copies. The
//! unrolled code enables SCCP to fold constants per-iteration and DCE to
//! eliminate the now-dead comparison/branch, producing straight-line code for
//! tight numeric loops.
//!
//! ## The canonical counted-loop contract (L4)
//!
//! The loop shape is recognized by [`counted_loop::recognize_counted_loop`],
//! the single source of truth for "is this a constant-trip counted loop, and
//! what are its IV / loop-carried values / start / stop / step?". That
//! recognizer handles the REAL shape the frontend emits for
//! `for i in range(...)`:
//!
//! ```text
//! preheader:  Branch -> H(start, acc0, ...)
//! H (header): args = [iv, acc, ...]      // MULTI-arg: IV + loop-carried set
//!   Branch -> C
//! C (cond):   iv_view = Copy(iv); cond = Lt(iv_view, stop)
//!   CondBranch(cond, Body, Exit)
//! Body:       ... uses iv_view ...; iv_next = Add(iv_view, step); ...
//!   Branch -> H(iv_next, acc_next, ...)  // back-edge
//! ```
//!
//! Historically this pass required the textbook "1-arg header with the
//! comparison in the header" shape and was therefore inert on every real
//! counted loop (the accumulator forces a multi-arg header). The pass now
//! consumes the [`counted_loop::CountedLoop`] descriptor, which models the
//! multi-arg header + separate cond block directly. The legacy 1-arg shape is a
//! strict special case of the descriptor (`cond_block == header`) and remains
//! handled.
//!
//! ## Unroll criteria (all required)
//!
//! 1. The loop is a recognized [`counted_loop::CountedLoop`] (constant trip
//!    count, single reachable preheader and back-edge, constant step with
//!    polarity matching the comparison).
//! 2. Trip count `<=` the cost model's unroll trip cap (`TargetInfo`, default 8).
//! 3. The cloned region (cond-block ops + body ops) `<=` the cost model's unroll
//!    body cap (default 20 ops; prevents code bloat).
//! 4. No real exception **handler** region in the function
//!    ([`TirFunction::has_exception_handlers`]). A bare `CheckException`
//!    observation op in the body is NOT a hazard: each unrolled clone retains
//!    the same handler label, which points at the function-exit handler block
//!    OUTSIDE the loop — so every clone correctly propagates a raised exception
//!    straight to the caller, exactly as the rolled loop would. A `try:` block
//!    *inside* the loop body (`TryStart`/`TryEnd`) makes `has_exception_handlers`
//!    true and is correctly refused.
//! 5. No nested loop inside the region.
//! 6. No body-region value escapes other than through the modelled back-edge /
//!    exit-arg threading.
//!
//! Representation soundness (bug #15): the descriptor only fires within the
//! trip-count cap, so the per-iteration induction constants this pass emits are
//! small. It does NOT promote any loop-carried value to a different `Repr`: a
//! carried accumulator's defining ops are cloned verbatim (same opcodes,
//! attrs), so an unbounded `MaybeBigInt` accumulator stays a `MaybeBigInt`
//! BigInt-correct chain — the unroll is a structural duplication, never a
//! representation change.
//!
//! Reference: Muchnick ch. 17, LLVM LoopUnrollPass.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::counted_loop::{self, CountedLoop};
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::values::ValueId;

// The copy-resolution model (`build_copy_map` / `resolve`) is shared with the
// `counted_loop` recognizer so both the recognition and the exit-arg
// substitution resolve frontend `Copy`/`store_var` chains identically.
use super::counted_loop::{build_copy_map, resolve};

/// Reject if any value defined inside the loop *region* (cond block or body) is
/// used anywhere outside the region — except as a back-edge or exit-edge
/// argument, both of which the unroll transform rewrites explicitly. Region
/// header block-args (the IV + loop-carried values) are local to the loop and
/// are threaded through the unrolled chain, so they never escape after the
/// rewrite either.
fn region_value_escapes(func: &TirFunction, loop_info: &CountedLoop) -> bool {
    let region = counted_loop::region_blocks(loop_info);

    // Values defined inside the region's cond block or body (op results). Header
    // block-args are handled by the threading and intentionally excluded here.
    let mut region_defs: HashSet<ValueId> = HashSet::new();
    for &bid in &[loop_info.cond_block, loop_info.body] {
        if let Some(block) = func.blocks.get(&bid) {
            for op in &block.ops {
                for r in &op.results {
                    region_defs.insert(*r);
                }
            }
        }
    }

    for (&bid, block) in &func.blocks {
        if region.contains(&bid) {
            continue;
        }
        // Op operands outside the region must not reference region defs.
        for op in &block.ops {
            for v in &op.operands {
                if region_defs.contains(v) {
                    return true;
                }
            }
        }
        // Terminator argument/condition references outside the region likewise,
        // EXCEPT the exit block's incoming args are handled by the transform —
        // but those args live on the COND block's terminator (inside the
        // region), so they are not scanned here. Any region def used by a
        // non-region terminator is a genuine escape.
        for v in terminator_value_refs(&block.terminator) {
            if region_defs.contains(&v) {
                return true;
            }
        }
    }
    false
}

/// Every `ValueId` referenced by a terminator (condition + all branch args).
fn terminator_value_refs(term: &Terminator) -> Vec<ValueId> {
    let mut refs = Vec::new();
    match term {
        Terminator::Branch { args, .. } => refs.extend(args.iter().copied()),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            refs.push(*cond);
            refs.extend(then_args.iter().copied());
            refs.extend(else_args.iter().copied());
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            refs.push(*value);
            for (_, _, args) in cases {
                refs.extend(args.iter().copied());
            }
            refs.extend(default_args.iter().copied());
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                refs.extend(args.iter().copied());
            }
            refs.extend(default_args.iter().copied());
        }
        Terminator::Return { values } => refs.extend(values.iter().copied()),
        Terminator::Unreachable => {}
    }
    refs
}

/// Detect counted loops eligible for full unrolling. Each loop header is run
/// through the canonical [`counted_loop`] recognizer; loops that pass the
/// cost-model caps and the escape/handler checks are returned.
fn find_unroll_candidates(func: &TirFunction, tti: &TargetInfo) -> Vec<CountedLoop> {
    // No real exception HANDLER region (a `try:`/generator state region) may be
    // present. A bare `CheckException` observation op is fine — see the module
    // doc and the soundness note in `docs/design/foundation/04_L4-loops.md`.
    if func.has_exception_handlers() {
        return Vec::new();
    }

    let mut header_ids: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| (*role == LoopRole::LoopHeader).then_some(*bid))
        .collect();
    header_ids.sort_by_key(|b| b.0);

    let mut candidates = Vec::new();
    for header_id in header_ids {
        let Some(loop_info) = counted_loop::recognize_counted_loop(func, header_id) else {
            continue;
        };

        // Cost model: trip count within the full-unroll cap.
        if loop_info.trip_count > tti.unroll_max_trip() {
            continue;
        }

        // Cost model: cloned region size (cond-block ops + body ops) within the
        // anti-bloat body cap.
        let cond_ops = func
            .blocks
            .get(&loop_info.cond_block)
            .map(|b| b.ops.len())
            .unwrap_or(0);
        let body_ops = func
            .blocks
            .get(&loop_info.body)
            .map(|b| b.ops.len())
            .unwrap_or(0);
        // When cond_block == header (legacy shape) the header ops ARE the cond
        // ops; counting them once is correct.
        let region_ops = cond_ops + body_ops;
        if region_ops > tti.unroll_max_body() {
            continue;
        }

        // No region value may escape other than via the modelled threading.
        if region_value_escapes(func, &loop_info) {
            continue;
        }

        candidates.push(loop_info);
    }
    candidates
}

pub fn run(func: &mut TirFunction, tti: &TargetInfo) -> PassStats {
    let mut stats = PassStats {
        name: "loop_unroll",
        ..Default::default()
    };

    let candidates = find_unroll_candidates(func, tti);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        unroll_counted_loop(func, &candidate, &mut stats);
    }

    stats
}

/// Fully unroll one recognized counted loop. Replaces the header/cond/body
/// region with a single straight-line "landing" block holding `trip_count`
/// copies of `cond_block.ops ++ body.ops` (one per iteration), with:
///
/// * the induction-variable header arg bound to its per-iteration constant
///   `start + k*step`, and
/// * every loop-carried header arg threaded from iteration `k`'s back-edge
///   value into iteration `k+1`'s region.
///
/// The landing block then branches to the loop exit, forwarding the exit-edge
/// arguments with their final-iteration values substituted in.
fn unroll_counted_loop(func: &mut TirFunction, c: &CountedLoop, stats: &mut PassStats) {
    // Snapshot the region we are about to clone, before any mutation.
    let header_block = match func.blocks.get(&c.header) {
        Some(b) => b.clone(),
        None => return,
    };
    // The header args are the loop-carried state vector (IV at `iv_arg_index`).
    let header_arg_ids: Vec<ValueId> = header_block.args.iter().map(|a| a.id).collect();
    if header_arg_ids.len() != c.back_args.len() {
        return;
    }

    // Region ops to clone per iteration, in execution order: cond-block ops then
    // body ops. When cond_block == header (legacy shape) the header *is* the
    // cond block; its ops are the cond ops and we must not double-count the body.
    let cond_ops: Vec<TirOp> = match func.blocks.get(&c.cond_block) {
        Some(b) => b.ops.clone(),
        None => return,
    };
    let body_ops: Vec<TirOp> = match func.blocks.get(&c.body) {
        Some(b) => b.ops.clone(),
        None => return,
    };

    // Preheader's args to the header give the initial loop-carried values.
    let preheader_args: Vec<ValueId> = match func.blocks.get(&c.preheader) {
        Some(b) => match header_args_from(&b.terminator, c.header) {
            Some(a) if a.len() == header_arg_ids.len() => a.to_vec(),
            _ => return,
        },
        None => return,
    };

    let copy_of = build_copy_map(func);

    // The current loop-carried state, indexed like header args. For the IV slot
    // we use a freshly materialised per-iteration constant; for every other slot
    // we thread the previous iteration's back-edge value.
    let mut current_carried: Vec<ValueId> = preheader_args.clone();

    let mut landing_ops: Vec<TirOp> = Vec::new();

    for k in 0..c.trip_count {
        // Per-iteration remap: header arg j -> current carried value.
        let mut remap: HashMap<ValueId, ValueId> = HashMap::new();

        // IV slot: materialise start + k*step as a fresh ConstInt.
        let iter_value = c.start + k * c.step;
        let iter_const_id = func.fresh_value();
        let mut const_attrs = AttrDict::new();
        const_attrs.insert("value".into(), AttrValue::Int(iter_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![iter_const_id],
            attrs: const_attrs,
            source_span: None,
        });
        current_carried[c.iv_arg_index] = iter_const_id;

        // Bind header args to the current carried state.
        for (j, &arg_id) in header_arg_ids.iter().enumerate() {
            remap.insert(arg_id, current_carried[j]);
        }

        // Clone cond-block ops then body ops, allocating fresh results and
        // extending the remap. The cloned comparison op becomes dead (its
        // CondBranch is replaced by the straight-line chain); DCE removes it.
        for op in cond_ops.iter().chain(body_ops.iter()) {
            let new_results: Vec<ValueId> = op
                .results
                .iter()
                .map(|&result| {
                    let new_value = func.fresh_value();
                    remap.insert(result, new_value);
                    new_value
                })
                .collect();
            let new_operands: Vec<ValueId> = op
                .operands
                .iter()
                .map(|v| remap.get(v).copied().unwrap_or(*v))
                .collect();
            landing_ops.push(TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: new_operands,
                results: new_results.clone(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
            });
            stats.ops_added += 1;
            stats.values_changed += new_results.len();
        }

        // Compute next iteration's carried state from the back-edge args.
        let mut next_carried = Vec::with_capacity(header_arg_ids.len());
        for &back in &c.back_args {
            next_carried.push(remap.get(&back).copied().unwrap_or(back));
        }
        current_carried = next_carried;
    }

    // Final loop-carried state after the last iteration. The IV's post-loop
    // value is start + trip_count*step (the value that fails the comparison).
    let final_iv_value = c.start + c.trip_count * c.step;
    let final_iv_const = func.fresh_value();
    {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(final_iv_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![final_iv_const],
            attrs,
            source_span: None,
        });
    }
    // `current_carried` now holds the final values for every non-IV slot; set
    // the IV slot to the post-loop constant.
    current_carried[c.iv_arg_index] = final_iv_const;

    // Substitute the exit-edge arguments. Each exit arg references either:
    //   * a header arg (a loop-carried value, possibly the IV) — directly or via
    //     a Copy chain — which we map to its final value, or
    //   * a value defined before the loop (loop-invariant) — kept as-is.
    let final_by_header: HashMap<ValueId, ValueId> = header_arg_ids
        .iter()
        .copied()
        .zip(current_carried.iter().copied())
        .collect();
    let new_exit_args: Vec<ValueId> = c
        .exit_args
        .iter()
        .map(|&v| {
            let root = resolve(&copy_of, v);
            final_by_header.get(&root).copied().unwrap_or(v)
        })
        .collect();

    // Allocate the landing block: the straight-line unrolled region, then a
    // branch to the loop exit carrying the final exit args.
    let landing = func.fresh_block();
    let landing_block = TirBlock {
        id: landing,
        args: Vec::new(),
        ops: landing_ops,
        terminator: Terminator::Branch {
            target: c.exit,
            args: new_exit_args,
        },
    };
    func.blocks.insert(landing, landing_block);

    // Redirect every predecessor of the header to the landing block (dropping
    // the now-unused header args — the landing block has no block args).
    let region = counted_loop::region_blocks(c);
    let preds: Vec<BlockId> = func
        .blocks
        .iter()
        .filter_map(|(&bid, b)| {
            if region.contains(&bid) || bid == landing {
                return None;
            }
            if branches_to(&b.terminator, c.header) {
                Some(bid)
            } else {
                None
            }
        })
        .collect();
    for pred in preds {
        if let Some(b) = func.blocks.get_mut(&pred) {
            redirect_terminator(&mut b.terminator, c.header, landing);
        }
    }
    if func.entry_block == c.header {
        func.entry_block = landing;
    }

    // Retire the region blocks (header, cond block, body) and the header's loop
    // metadata. When cond_block == header the set collapses to {header, body}.
    let header_ops_count = header_block.ops.len();
    let cond_ops_count = if c.cond_block == c.header {
        0
    } else {
        cond_ops.len()
    };
    let body_ops_count = body_ops.len();

    // The structural `LoopEnd` marker that paired with this header is now
    // orphaned: with the loop unrolled away there is no `LoopHeader` for it to
    // close. Left in place, the TIR→SimpleIR back-conversion — which pairs
    // `LoopHeader`/`LoopEnd` to re-emit `loop_start`/`loop_end` for the native
    // and WASM backends — would crash on a `LoopEnd` block with no matching
    // header (observed as a backend-daemon compile abort). The frontend emits
    // this marker as an unreachable dead block (no terminator predecessor), so
    // we drop its role; if it is now wholly unreachable we remove the block too.
    let end_marker = c.loop_pairs_end;
    if let Some(end_bid) = end_marker {
        func.loop_roles.remove(&end_bid);
        func.loop_pairs.remove(&end_bid);
        func.loop_break_kinds.remove(&end_bid);
        func.loop_cond_blocks.remove(&end_bid);
    }

    for &bid in &region {
        func.blocks.remove(&bid);
    }
    func.loop_roles.remove(&c.header);
    func.loop_pairs.remove(&c.header);
    func.loop_break_kinds.remove(&c.header);
    func.loop_cond_blocks.remove(&c.header);

    // Drop the orphaned `LoopEnd` marker block if it is now unreachable through
    // terminator edges from the entry — it is the dead frontend marker and would
    // otherwise linger as a no-predecessor block. (A block that is still
    // reachable is left intact; only its role was stripped above.)
    if let Some(end_bid) = end_marker
        && func.blocks.contains_key(&end_bid)
    {
        let reachable = crate::tir::dominators::reachable_blocks_with(
            func,
            crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
        );
        if !reachable.contains(&end_bid) {
            func.blocks.remove(&end_bid);
        }
    }

    // GLOBAL header-arg fixup. A loop-carried header arg (the IV or an
    // accumulator) is an SSA value DEFINED by the now-deleted header block. Any
    // surviving block that still references such an arg by its value id — most
    // importantly a NESTED loop's exit block, which forwards the inner loop's
    // accumulator to the ENCLOSING loop's back-edge using the inner header-arg
    // value directly — would reference a value that no longer has a definition
    // ("%N used but never defined"), and the dead-but-referenced computation
    // then drives the native back-conversion into a hang. Every such post-loop
    // use logically observes the loop-carried value AFTER the final iteration,
    // so we rewrite each surviving reference to its final value. Region blocks
    // are already removed; the landing block consumes only fresh values, so it
    // is unaffected.
    if !final_by_header.is_empty() {
        for block in func.blocks.values_mut() {
            if block.id == landing {
                continue;
            }
            for op in &mut block.ops {
                for operand in &mut op.operands {
                    if let Some(&final_v) = final_by_header.get(operand) {
                        *operand = final_v;
                    }
                }
            }
            substitute_terminator_values(&mut block.terminator, &final_by_header);
        }
    }

    stats.ops_removed += header_ops_count + cond_ops_count + body_ops_count;
}

/// Replace every value reference in `term` (condition + all branch args) that
/// appears as a key in `subst` with its mapped value.
fn substitute_terminator_values(term: &mut Terminator, subst: &HashMap<ValueId, ValueId>) {
    fn remap(v: &mut ValueId, subst: &HashMap<ValueId, ValueId>) {
        if let Some(&nv) = subst.get(v) {
            *v = nv;
        }
    }
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                remap(v, subst);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            remap(cond, subst);
            for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                remap(v, subst);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            remap(value, subst);
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    remap(v, subst);
                }
            }
            for v in default_args {
                remap(v, subst);
            }
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    remap(v, subst);
                }
            }
            for v in default_args {
                remap(v, subst);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                remap(v, subst);
            }
        }
        Terminator::Unreachable => {}
    }
}

/// Extract the args passed to `header` from a predecessor terminator.
fn header_args_from(term: &Terminator, header: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target, args } if *target == header => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                Some(then_args.as_slice())
            } else if *else_block == header {
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
        } => {
            if *default == header {
                Some(default_args.as_slice())
            } else {
                cases.iter().find_map(|(_, b, args)| {
                    if *b == header {
                        Some(args.as_slice())
                    } else {
                        None
                    }
                })
            }
        }
        _ => None,
    }
}

/// Returns `true` if the terminator references `target` as any successor.
fn branches_to(term: &Terminator, target: BlockId) -> bool {
    match term {
        Terminator::Branch { target: t, .. } => *t == target,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == target || *else_block == target,
        Terminator::Switch { cases, default, .. } => {
            *default == target || cases.iter().any(|(_, b, _)| *b == target)
        }
        _ => false,
    }
}

/// Replace every successor reference to `from` with `to` in `term`. The landing
/// block has zero block arguments by construction, so we MUST also drop any
/// argument list that was being forwarded to `from` to keep TIR verification
/// (block-arg arity match) sound.
fn redirect_terminator(term: &mut Terminator, from: BlockId, to: BlockId) {
    match term {
        Terminator::Branch { target, args } if *target == from => {
            *target = to;
            args.clear();
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == from {
                *then_block = to;
                then_args.clear();
            }
            if *else_block == from {
                *else_block = to;
                else_args.clear();
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == from {
                    *b = to;
                    args.clear();
                }
            }
            if *default == from {
                *default = to;
                default_args.clear();
            }
        }
        _ => {}
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    fn const_int_op(result: ValueId, value: i64) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m
            },
            source_span: None,
        }
    }

    fn add_op(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn cmp_op(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    struct TestLoop {
        func: TirFunction,
        header: BlockId,
        body: BlockId,
        exit: BlockId,
        /// Result of the user's body Add op (defined inside body, never escapes).
        body_op_result: ValueId,
    }

    /// Build a TIR function that mirrors the post-`range_devirt` SSA shape for
    /// `for i in range(start, stop, step): body_op(i)`.
    ///
    /// CFG:
    /// ```text
    /// entry: ConstInt(start), ConstInt(stop), ConstInt(step)
    ///        Branch -> header(start_val)
    /// header(ind_var):
    ///   cond = Lt|Gt(ind_var, stop_val)
    ///   CondBranch(cond, body, exit)
    /// body:
    ///   ... body_op_count user Add ops ...
    ///   next_ind = Add(ind_var, step_val)
    ///   Branch -> header(next_ind)
    /// exit:
    ///   Return
    /// ```
    fn build_test_loop(start: i64, stop: i64, step: i64, body_op_count: usize) -> TestLoop {
        assert!(step != 0, "step must be non-zero in test fixture");
        assert!(
            body_op_count >= 1,
            "tests rely on at least one user body op"
        );

        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let external = func.fresh_value();
        let body_op_result = func.fresh_value();
        let next_ind = func.fresh_value();

        // Entry/preheader → header(start_val)
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(external, 10));
            entry.ops.push(const_int_op(start_val, start));
            entry.ops.push(const_int_op(stop_val, stop));
            entry.ops.push(const_int_op(step_val, step));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_val],
            };
        }

        // Header: cmp + CondBranch (body-or-exit, no successor args).
        let cmp = if step > 0 { OpCode::Lt } else { OpCode::Gt };
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![cmp_op(cmp, ind_var, stop_val, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Body: user op(s) + increment Add(ind_var, step_val) -> next_ind
        let mut body_ops = vec![add_op(ind_var, external, body_op_result)];
        for _ in 1..body_op_count {
            let extra_result = func.fresh_value();
            body_ops.push(add_op(ind_var, external, extra_result));
        }
        body_ops.push(add_op(ind_var, step_val, next_ind));

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_ind],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);

        TestLoop {
            func,
            header,
            body,
            exit,
            body_op_result,
        }
    }

    fn attr_int(op: &TirOp, name: &str) -> Option<i64> {
        match op.attrs.get(name) {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    #[test]
    fn unrolls_small_range_loop_into_four_body_copies() {
        // for i in range(0, 4, 1): body_op(i)
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);

        let stats = run(&mut func, &TargetInfo::native_release_fast());

        // 4 trips × (1 user Add + 1 increment Add) = 8 Adds plus 4 iter constants.
        // Stats add: 4 iter ConstInts are produced via fresh_value but accounted
        // for separately from the 4 trip × 2 body ops. Our unroller bumps
        // ops_added/values_changed once per cloned body op.
        assert!(stats.ops_added > 0, "loop_unroll should fire");
        assert!(stats.values_changed > 0);
        assert!(!func.blocks.contains_key(&header));
        assert!(!func.blocks.contains_key(&body));
        assert!(!func.loop_roles.contains_key(&header));

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, args } => {
                assert!(args.is_empty(), "entry must drop header arg after redirect");
                *target
            }
            _ => panic!("entry should branch to unrolled landing block"),
        };
        assert_ne!(landing, header);
        assert_ne!(landing, body);

        let landing_block = &func.blocks[&landing];
        let add_count = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        // Body had 2 Adds (user op + increment); unrolled 4 times → 8 Adds.
        assert_eq!(add_count, 8, "body Adds should be cloned once per trip");

        let iteration_constants: Vec<_> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        // The first 4 ConstInts are the per-iteration induction values.
        assert!(
            iteration_constants.starts_with(&[0, 1, 2, 3]),
            "expected leading iteration constants 0..4, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn unrolls_loop_with_explicit_start_and_step() {
        // for i in range(2, 10, 2): body_op(i) -> trip count = 4 (2,4,6,8)
        let TestLoop { mut func, .. } = build_test_loop(2, 10, 2, 1);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(stats.ops_added > 0);

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let iteration_constants: Vec<_> = func.blocks[&landing]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iteration_constants.starts_with(&[2, 4, 6, 8]),
            "expected iteration constants 2,4,6,8, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn unrolls_loop_with_negative_step() {
        // for i in range(5, 0, -1): body_op(i) -> trip count = 5 (5,4,3,2,1)
        let TestLoop { mut func, .. } = build_test_loop(5, 0, -1, 1);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(
            stats.ops_added > 0,
            "negative-step loop should still unroll"
        );

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let iteration_constants: Vec<_> = func.blocks[&landing]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iteration_constants.starts_with(&[5, 4, 3, 2, 1]),
            "expected iteration constants 5..1, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn does_not_unroll_body_larger_than_max_unroll_ops() {
        let TestLoop {
            mut func,
            header,
            body,
            body_op_result,
            ..
        } = build_test_loop(
            0,
            4,
            1,
            TargetInfo::native_release_fast().unroll_max_body() + 1,
        );
        let entry_target_before = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to header"),
        };

        let stats = run(&mut func, &TargetInfo::native_release_fast());

        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(entry_target_before, header);
        assert!(func.blocks.contains_key(&header));
        assert!(func.blocks.contains_key(&body));
        assert!(func.loop_roles.contains_key(&header));
        assert_eq!(
            func.blocks[&body].ops[0].results,
            vec![body_op_result],
            "oversized body should remain intact"
        );
    }

    #[test]
    fn does_not_unroll_when_trip_count_exceeds_limit() {
        // for i in range(0, 100): trip count = 100 > unroll trip cap (8)
        let TestLoop {
            mut func, header, ..
        } = build_test_loop(0, 100, 1, 1);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0, "loop with 100 trips should not unroll");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_when_step_is_zero_step_means_no_loop() {
        // We can't construct a step=0 loop via build_test_loop's assert,
        // so simulate it directly: cmp Lt with step=0 in increment.
        let mut func = TirFunction::new("zero_step".into(), vec![], TirType::None);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let next_ind = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(start_val, 0));
            entry.ops.push(const_int_op(stop_val, 4));
            entry.ops.push(const_int_op(step_val, 0));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_val],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![cmp_op(OpCode::Lt, ind_var, stop_val, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![add_op(ind_var, step_val, next_ind)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_ind],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0, "step=0 loop must never unroll");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_when_polarity_disagrees_with_step_sign() {
        // Build a loop where the comparison is Lt but step is negative —
        // this is a non-terminating loop that range_devirt would never emit,
        // and the unroller must reject it rather than synthesize bogus trips.
        let TestLoop {
            mut func, header, ..
        } = build_test_loop(5, 0, -1, 1);
        // Mutate the comparison from Gt (as built for step=-1) to Lt to break
        // the polarity contract.
        let header_block = func.blocks.get_mut(&header).unwrap();
        for op in header_block.ops.iter_mut() {
            if op.opcode == OpCode::Gt {
                op.opcode = OpCode::Lt;
            }
        }
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(
            stats.ops_added, 0,
            "polarity/step mismatch must reject the loop"
        );
        assert!(func.blocks.contains_key(&header));
    }

    /// L4 gate semantics: a function carrying the `has_exception_handling`
    /// flag *and* a bare `CheckException` observation op in the loop body —
    /// but NO real handler region — is still unrolled. Every cloned
    /// `CheckException` retains the same handler label (the function-exit
    /// handler, outside the loop), so the unrolled code has identical exception
    /// semantics to the rolled loop.
    #[test]
    fn unrolls_with_check_exception_observation_in_body() {
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);
        // Add a bare CheckException observation op to the body and set the
        // coarse flag, exactly as the production frontend does.
        func.blocks.get_mut(&body).unwrap().ops.insert(
            0,
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(99));
                    m
                },
                source_span: None,
            },
        );
        func.has_exception_handling = true;
        assert!(
            !func.has_exception_handlers(),
            "CheckException alone is not a handler region"
        );
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(
            stats.ops_added > 0,
            "CheckException-only function must still unroll"
        );
        assert!(!func.blocks.contains_key(&header));
    }

    /// Adversarial: a real `try:` block (TryStart) inside the loop body makes
    /// `has_exception_handlers()` true and unrolling is correctly refused —
    /// duplicating a per-iteration handler region would need handler-label
    /// remapping this pass does not perform.
    #[test]
    fn does_not_unroll_with_try_handler_in_body() {
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);
        func.blocks.get_mut(&body).unwrap().ops.insert(
            0,
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::TryStart,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
        );
        func.has_exception_handling = true;
        assert!(func.has_exception_handlers());
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0, "try-in-body must block unroll");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_nested_loop_inner_when_body_is_a_header() {
        // Mark the body block itself as a LoopHeader to simulate nesting.
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);
        func.loop_roles.insert(body, LoopRole::LoopHeader);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0, "nested loop must be rejected");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn rejects_loop_when_body_value_escapes_to_exit() {
        // Add a use of body_op_result in the exit block — the structural
        // detector must refuse to unroll because rewriting that use after the
        // loop disappears would be unsound under the current contract.
        let TestLoop {
            mut func,
            header,
            exit,
            body_op_result,
            ..
        } = build_test_loop(0, 4, 1, 1);
        let dummy = func.fresh_value();
        let exit_block = func.blocks.get_mut(&exit).unwrap();
        exit_block
            .ops
            .push(add_op(body_op_result, body_op_result, dummy));

        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0, "escaping body value must block unroll");
        assert!(func.blocks.contains_key(&header));
    }

    /// Regression test for the structural detector: loop is built WITHOUT any
    /// `range_role` metadata (only real ConstInt + Add + Lt ops, exactly what
    /// `range_devirt` produces for `for i in range(8): ...`). Proves the
    /// metadata-keyed dead path is gone and the structural recognizer fires.
    #[test]
    fn structural_detector_unrolls_real_range_devirt_shape() {
        // for i in range(0, 8, 1): body_op(i)
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 8, 1, 1);

        // Sanity: no op in the function carries a `range_role` attribute.
        for block in func.blocks.values() {
            for op in &block.ops {
                assert!(
                    !op.attrs.contains_key("range_role"),
                    "fixture must not emit range_role metadata"
                );
            }
        }

        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(
            stats.ops_added > 0 && stats.values_changed > 0,
            "structural detector must fire on real range_devirt CFG shape"
        );
        assert!(!func.blocks.contains_key(&header), "header must be removed");
        assert!(!func.blocks.contains_key(&body), "body must be removed");

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let landing_block = &func.blocks[&landing];

        // Eight per-iteration induction-value constants must have been
        // emitted, in the order 0,1,…,7.
        let iter_consts: Vec<i64> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iter_consts.starts_with(&[0, 1, 2, 3, 4, 5, 6, 7]),
            "iter constants must be 0..8, got {iter_consts:?}"
        );

        // Body had 2 Adds; unrolled 8× → 16 Adds total.
        let add_count = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(add_count, 16);

        crate::tir::verify::verify_function(&func)
            .expect("post-unroll function must satisfy TIR verifier");
    }

    /// Build the REAL frontend counted-loop shape for
    /// `for i in range(start, stop, step): total += i` — a MULTI-arg header
    /// (`[iv, acc]`) that `Branch`es to a SEPARATE cond block holding the
    /// comparison, with the accumulator escaping through the exit edge. This is
    /// the shape the historical 1-arg detector could never match.
    ///
    /// CFG:
    /// ```text
    /// entry: ConstInt(start/stop/step/acc0) ; Branch -> H(start, acc0)
    /// H(iv, acc):  Branch -> C
    /// C: iv_view = Copy(iv) ; cond = Lt|Gt(iv_view, stop) ; CondBranch(cond, B, E)
    /// B: acc_next = Add(acc, iv_view) ; iv_next = Add(iv_view, step)
    ///    Branch -> H(iv_next, acc_next)
    /// E(acc_out): Return acc_out
    /// ```
    struct MultiArgLoop {
        func: TirFunction,
        header: BlockId,
        cond: BlockId,
        body: BlockId,
        exit: BlockId,
    }

    fn build_multiarg_counted_loop(start: i64, stop: i64, step: i64) -> MultiArgLoop {
        assert!(step != 0);
        let mut func = TirFunction::new("multi".into(), vec![], TirType::I64);

        let header = func.fresh_block();
        let cond = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();

        let iv = func.fresh_value();
        let acc = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let acc0 = func.fresh_value();
        let iv_view = func.fresh_value();
        let cmp = func.fresh_value();
        let acc_next = func.fresh_value();
        let iv_next = func.fresh_value();
        let exit_arg = func.fresh_value();

        // Entry / preheader.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(start_val, start));
            entry.ops.push(const_int_op(stop_val, stop));
            entry.ops.push(const_int_op(step_val, step));
            entry.ops.push(const_int_op(acc0, 0));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_val, acc0],
            };
        }

        // Header: multi-arg phi block, Branch -> cond.
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: acc,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: cond,
                    args: vec![],
                },
            },
        );

        // Cond block: iv_view = Copy(iv); cmp; CondBranch(cmp, body, exit(acc)).
        let cmp_kind = if step > 0 { OpCode::Lt } else { OpCode::Gt };
        func.blocks.insert(
            cond,
            TirBlock {
                id: cond,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![iv],
                        results: vec![iv_view],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                    cmp_op(cmp_kind, iv_view, stop_val, cmp),
                ],
                terminator: Terminator::CondBranch {
                    cond: cmp,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    // The accumulator (a header arg) escapes through the exit.
                    else_args: vec![acc],
                },
            },
        );

        // Body: acc_next = Add(acc, iv_view); iv_next = Add(iv_view, step);
        // Branch -> header(iv_next, acc_next).
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    add_op(acc, iv_view, acc_next),
                    add_op(iv_view, step_val, iv_next),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![iv_next, acc_next],
                },
            },
        );

        // Exit: Return acc_out.
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: exit_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![exit_arg],
                },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.loop_cond_blocks.insert(header, cond);

        MultiArgLoop {
            func,
            header,
            cond,
            body,
            exit,
        }
    }

    /// The recognizer + transform fully unroll the real multi-arg-header shape,
    /// threading the accumulator through each iteration and forwarding its final
    /// value to the exit. `for i in range(0,4): total += i` (total = 0+1+2+3).
    #[test]
    fn unrolls_real_multiarg_header_with_accumulator() {
        let MultiArgLoop {
            mut func,
            header,
            cond,
            body,
            exit,
        } = build_multiarg_counted_loop(0, 4, 1);

        // Recognizer must produce a 2-arg descriptor with the IV at index 0.
        let info = counted_loop::recognize_counted_loop(&func, header)
            .expect("multi-arg counted loop must be recognized");
        assert_eq!(info.cond_block, cond);
        assert_eq!(info.body, body);
        assert_eq!(info.exit, exit);
        assert_eq!(info.iv_arg_index, 0);
        assert_eq!(info.trip_count, 4);
        assert_eq!(info.start, 0);
        assert_eq!(info.step, 1);

        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(stats.ops_added > 0, "multi-arg loop must unroll");
        assert!(!func.blocks.contains_key(&header), "header retired");
        assert!(!func.blocks.contains_key(&cond), "cond block retired");
        assert!(!func.blocks.contains_key(&body), "body retired");

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, args } => {
                assert!(args.is_empty(), "entry drops header args after redirect");
                *target
            }
            _ => panic!("entry should branch to landing"),
        };

        // The exit-edge accumulator must be forwarded with a real (non-header)
        // SSA value defined in the landing block.
        let landing_block = &func.blocks[&landing];
        let exit_args = match &landing_block.terminator {
            Terminator::Branch { target, args } if *target == exit => args.clone(),
            _ => panic!("landing must branch to exit with the final accumulator"),
        };
        assert_eq!(exit_args.len(), 1, "one escaping accumulator");
        // The forwarded accumulator must be produced inside the landing block
        // (an Add chain), not a dangling header arg.
        let produced: std::collections::HashSet<ValueId> = landing_block
            .ops
            .iter()
            .flat_map(|op| op.results.iter().copied())
            .collect();
        assert!(
            produced.contains(&exit_args[0]),
            "final accumulator must be defined in the landing block, got {:?}",
            exit_args[0]
        );

        // Per-iteration IV constants 0,1,2,3 plus the final post-loop IV (4).
        let consts: Vec<i64> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            consts.starts_with(&[0, 1, 2, 3]),
            "iteration constants must lead 0..4, got {consts:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled multi-arg loop must pass TIR verification");

        // Round-trip through the TIR→SimpleIR back-conversion the native and
        // WASM backends consume. This must not panic: after the loop is unrolled
        // away there is no structured loop region, and any leftover `LoopEnd`
        // marker (none here, but exercised by the dead-marker test) would make
        // the back-conversion abort.
        let _simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
    }

    /// Negative step on the multi-arg shape: `for i in range(3,0,-1): total += i`
    /// → trip count 3 (i = 3,2,1).
    #[test]
    fn unrolls_real_multiarg_header_negative_step() {
        let MultiArgLoop {
            mut func, header, ..
        } = build_multiarg_counted_loop(3, 0, -1);
        let info = counted_loop::recognize_counted_loop(&func, header).unwrap();
        assert_eq!(info.trip_count, 3);
        assert_eq!(info.step, -1);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(stats.ops_added > 0);
        assert!(!func.blocks.contains_key(&header));

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let consts: Vec<i64> = func.blocks[&landing]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(consts.starts_with(&[3, 2, 1]), "got {consts:?}");
        crate::tir::verify::verify_function(&func).expect("verifier");
    }

    /// A dead `LoopEnd`-marker block that still branches to the header must NOT
    /// be miscounted as a second preheader: the recognizer excludes unreachable
    /// blocks via terminator-only reachability.
    #[test]
    fn dead_loop_end_block_is_not_a_second_preheader() {
        let MultiArgLoop {
            mut func, header, ..
        } = build_multiarg_counted_loop(0, 4, 1);
        // Insert an unreachable block that branches to the header with matching
        // arity — exactly the LoopEnd marker the frontend leaves behind.
        let dead = func.fresh_block();
        let d0 = func.fresh_value();
        let d1 = func.fresh_value();
        func.blocks.insert(
            dead,
            TirBlock {
                id: dead,
                args: vec![],
                ops: vec![const_int_op(d0, 0), const_int_op(d1, 0)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![d0, d1],
                },
            },
        );
        func.loop_roles.insert(dead, LoopRole::LoopEnd);
        // Pair the marker with the header, exactly as the frontend's
        // `loop_pairs` records it.
        func.loop_pairs.insert(header, dead);

        // Still recognized (dead block excluded from preheader counting), and the
        // descriptor carries the paired LoopEnd marker for cleanup.
        let info = counted_loop::recognize_counted_loop(&func, header)
            .expect("dead LoopEnd must not block recognition");
        assert_eq!(info.trip_count, 4);
        assert_eq!(info.loop_pairs_end, Some(dead));

        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(stats.ops_added > 0, "must still unroll despite dead marker");
        assert!(!func.blocks.contains_key(&header));
        // The orphaned, now-unreachable LoopEnd marker block and its role must be
        // gone so the back-conversion never sees a LoopEnd without a LoopHeader.
        assert!(
            !func.blocks.contains_key(&dead),
            "unreachable LoopEnd marker must be removed after unroll"
        );
        assert!(!func.loop_roles.contains_key(&dead));

        // No LoopEnd role may survive without a matching LoopHeader, and the
        // TIR→SimpleIR back-conversion must not panic on the result.
        assert!(
            !func.loop_roles.values().any(|r| *r == LoopRole::LoopEnd),
            "no orphaned LoopEnd role may remain after the loop is unrolled away"
        );
        crate::tir::verify::verify_function(&func).expect("verifier");
        let _simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
    }

    /// Recognizer refusal: a non-constant `stop` (not a ConstInt) yields `None`
    /// (a principled refusal — the loop is left intact, never miscompiled).
    #[test]
    fn refuses_non_constant_stop() {
        let MultiArgLoop {
            mut func,
            header,
            cond,
            ..
        } = build_multiarg_counted_loop(0, 4, 1);
        // Replace the stop ConstInt operand of the comparison with a fresh
        // unknown value (simulate `range(n)` with a runtime bound).
        let unknown = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.args.push(TirValue {
                id: unknown,
                ty: TirType::I64,
            });
        }
        let cond_block = func.blocks.get_mut(&cond).unwrap();
        for op in cond_block.ops.iter_mut() {
            if matches!(op.opcode, OpCode::Lt) {
                op.operands[1] = unknown;
            }
        }
        assert!(
            counted_loop::recognize_counted_loop(&func, header).is_none(),
            "runtime-bounded loop must be refused"
        );
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert_eq!(stats.ops_added, 0);
        assert!(func.blocks.contains_key(&header));
    }

    /// Reproduce the production `for i in range(8): total += i` shape closely:
    /// the body uses `InplaceAdd` and a store-to-slot `Copy(value, slot) -> ()`
    /// (a no-result 2-operand copy), the exit block is a SHARED MERGE reached
    /// both from the loop and from a non-loop predecessor, and a dead `LoopEnd`
    /// marker branches to the header. After unrolling, the whole function must
    /// round-trip through `lower_to_simple_ir` (the native/WASM back-conversion)
    /// without panicking or hanging.
    #[test]
    fn unrolled_inplace_add_shared_exit_round_trips_to_simple_ir() {
        let mut func = TirFunction::new("repro".into(), vec![], TirType::I64);

        let guard = func.fresh_block(); // entry's CondBranch (empty-range guard)
        let preheader = func.fresh_block();
        let header = func.fresh_block();
        let cond = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let dead_end = func.fresh_block();

        let iv = func.fresh_value();
        let acc = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let acc0 = func.fresh_value();
        let slot = func.fresh_value();
        let guard_cond = func.fresh_value();
        let early = func.fresh_value();
        let iv_view = func.fresh_value();
        let cmp = func.fresh_value();
        let acc_next = func.fresh_value();
        let iv_next = func.fresh_value();
        let exit_arg = func.fresh_value();

        // Entry: consts + a guard CondBranch (models the empty-range check).
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(start_val, 0));
            entry.ops.push(const_int_op(stop_val, 8));
            entry.ops.push(const_int_op(step_val, 1));
            entry.ops.push(const_int_op(acc0, 0));
            entry.ops.push(const_int_op(slot, 0));
            entry.ops.push(const_int_op(guard_cond, 1));
            entry.terminator = Terminator::Branch {
                target: guard,
                args: vec![],
            };
        }
        // Guard: CondBranch → early-return path / preheader.
        func.blocks.insert(
            guard,
            TirBlock {
                id: guard,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: guard_cond,
                    then_block: exit, // shared merge: non-loop path into the exit
                    then_args: vec![acc0],
                    else_block: preheader,
                    else_args: vec![],
                },
            },
        );
        // Preheader → header(start, acc0).
        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![const_int_op(early, 0)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![start_val, acc0],
                },
            },
        );
        // Header (multi-arg) → cond.
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: acc,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: cond,
                    args: vec![],
                },
            },
        );
        // Cond: iv_view = Copy(iv); cmp; CondBranch(cmp, body, exit(acc)).
        func.blocks.insert(
            cond,
            TirBlock {
                id: cond,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![iv],
                        results: vec![iv_view],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("_simple_out".into(), AttrValue::Str("i".into()));
                            m
                        },
                        source_span: None,
                    },
                    cmp_op(OpCode::Lt, iv_view, stop_val, cmp),
                ],
                terminator: Terminator::CondBranch {
                    cond: cmp,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![acc],
                },
            },
        );
        // Body: InplaceAdd(acc, iv_view) -> acc_next; store-to-slot Copy with no
        // result; store_var Copy(acc_next,acc_next) is implicit via back-edge;
        // iv_next = Add(iv_view, step).
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::InplaceAdd,
                        operands: vec![acc, iv_view],
                        results: vec![acc_next],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("_simple_out".into(), AttrValue::Str("total".into()));
                            m
                        },
                        source_span: None,
                    },
                    // store-to-slot: Copy(value, slot) -> () (no result, 2 operands)
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![acc_next, slot],
                        results: vec![],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("_original_kind".into(), AttrValue::Str("store".into()));
                            m
                        },
                        source_span: None,
                    },
                    add_op(iv_view, step_val, iv_next),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![iv_next, acc_next],
                },
            },
        );
        // Exit: shared merge (from guard's then-edge and the loop). Return.
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: exit_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![exit_arg],
                },
            },
        );
        // Dead LoopEnd marker (no predecessors) branching to the header.
        let d0 = func.fresh_value();
        let d1 = func.fresh_value();
        func.blocks.insert(
            dead_end,
            TirBlock {
                id: dead_end,
                args: vec![],
                ops: vec![const_int_op(d0, 0), const_int_op(d1, 0)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![d0, d1],
                },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.loop_roles.insert(dead_end, LoopRole::LoopEnd);
        func.loop_pairs.insert(header, dead_end);
        func.loop_cond_blocks.insert(header, cond);

        // Recognized and unrolled (trip 8).
        let info = counted_loop::recognize_counted_loop(&func, header)
            .expect("repro loop must be recognized");
        assert_eq!(info.trip_count, 8);
        let stats = run(&mut func, &TargetInfo::native_release_fast());
        assert!(stats.ops_added > 0, "repro must unroll");
        assert!(!func.blocks.contains_key(&header));
        assert!(
            !func.loop_roles.values().any(|r| *r == LoopRole::LoopEnd),
            "no orphaned LoopEnd may remain"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled repro must pass TIR verification");

        // The back-conversion must terminate without panicking on the unrolled,
        // shared-exit, store-slot-bearing IR, and leave NO loop markers (a stray
        // loop_start/loop_end would make the native loop reconstruction hang).
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        for op in &simple {
            assert!(
                !matches!(
                    op.kind.as_str(),
                    "loop_start"
                        | "loop_end"
                        | "loop_continue"
                        | "loop_break"
                        | "loop_break_if_true"
                        | "loop_break_if_false"
                ),
                "stray loop marker {} after full unroll",
                op.kind
            );
        }
        assert!(!simple.is_empty());
    }
}
