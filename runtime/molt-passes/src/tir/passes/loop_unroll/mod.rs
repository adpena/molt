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
//! 3. The cloned region (all guard-path and body-path ops) `<=` the cost model's unroll
//!    body cap (default 20 ops; prevents code bloat).
//! 4. No real exception **handler** region in the function
//!    ([`TirFunction::has_exception_handlers`]). A bare `CheckException`
//!    observation op is retained. Exception side exits are admitted only when
//!    they cannot observe the normal exit's final-value substitution. A `try:` block
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
use crate::tir::analysis::{Analysis, LoopForest};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::values::ValueId;

// The copy-resolution model is shared with the counted-loop recognizer so both
// recognition and exit-arg substitution resolve frontend Copy/store_var chains
// identically.
use super::value_identity::{build_copy_map, resolve_copy};

mod terminators;

#[cfg(test)]
mod tests;

use terminators::{
    branches_to, header_args_from, redirect_terminator, substitute_terminator_values,
    terminator_value_refs,
};

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
    for &bid in &region {
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

/// Cloned exception observations keep their targets. Unlike the normal exit,
/// an early exception must never observe the final iteration's carried values.
/// Region-result escapes are checked separately by `region_value_escapes`.
fn side_exits_observe_carried_state(func: &TirFunction, c: &CountedLoop) -> bool {
    if !c.has_side_exits {
        return false;
    }
    let region = counted_loop::region_blocks(c);
    let carried: HashSet<_> = func.blocks[&c.header]
        .args
        .iter()
        .map(|arg| arg.id)
        .collect();
    let labels = crate::tir::dominators::exception_label_to_block(func);
    for &bid in &region {
        for target in crate::tir::dominators::exception_successors(&func.blocks[&bid], &labels) {
            if !func.blocks[&target].args.is_empty() {
                return true;
            }
        }
    }
    func.blocks.iter().any(|(bid, block)| {
        !region.contains(bid)
            && (block
                .ops
                .iter()
                .flat_map(|op| &op.operands)
                .any(|v| carried.contains(v))
                || terminator_value_refs(&block.terminator)
                    .iter()
                    .any(|v| carried.contains(v)))
    })
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

    let loop_forest = <LoopForest as Analysis>::compute(func);

    let mut candidates = Vec::new();
    for &header_id in &loop_forest.headers {
        let Some(loop_info) =
            counted_loop::recognize_counted_loop_with_loop_forest(func, header_id, &loop_forest)
        else {
            continue;
        };
        if !loop_info.has_material_exit || side_exits_observe_carried_state(func, &loop_info) {
            continue;
        }

        // Cost model: trip count within the full-unroll cap.
        if loop_info.trip_count <= 0 || loop_info.trip_count > tti.unroll_max_trip() {
            continue;
        }
        // A finite mathematical trip count does not imply that the failed-
        // guard value fits the i64 constants emitted by this transformation.
        if i64::try_from(
            loop_info.start as i128 + loop_info.trip_count as i128 * loop_info.step as i128,
        )
        .is_err()
        {
            continue;
        }

        // Cost model: complete cloned region size within the
        // anti-bloat body cap.
        let region_ops: usize = counted_loop::region_blocks(&loop_info)
            .iter()
            .map(|bid| func.blocks[bid].ops.len())
            .sum();
        if region_ops > tti.unroll_max_body() {
            continue;
        }

        // No region value may escape other than via the modelled threading.
        if region_value_escapes(func, &loop_info) {
            continue;
        }

        if func
            .validate_block_retirement(
                &counted_loop::region_blocks(&loop_info),
                &HashSet::from([loop_info.header]),
                &HashSet::from([loop_info.header]),
                func.entry_block == loop_info.header,
            )
            .is_err()
        {
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

    // The descriptor supplies execution order, including interposed transfer
    // boundaries. No block's operations may disappear during region retirement.
    let cond_ops: Vec<TirOp> = c
        .guard_path
        .iter()
        .flat_map(|bid| func.blocks[bid].ops.iter().cloned())
        .collect();
    let body_ops: Vec<TirOp> = c
        .body_path
        .iter()
        .flat_map(|bid| func.blocks[bid].ops.iter().cloned())
        .collect();

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
        let iter_value = i64::try_from(c.start as i128 + k as i128 * c.step as i128)
            .expect("admitted counted-loop iteration must fit i64");
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
    let final_iv_value = i64::try_from(c.start as i128 + c.trip_count as i128 * c.step as i128)
        .expect("admitted counted-loop final guard must fit i64");
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

    // The final failed guard is still executed in the rolled loop. Preserve
    // its observations and definitions, including polls separated into blocks.
    let mut final_remap: HashMap<ValueId, ValueId> = header_arg_ids
        .iter()
        .copied()
        .zip(current_carried.iter().copied())
        .collect();
    for op in &cond_ops {
        let operands = op
            .operands
            .iter()
            .map(|v| final_remap.get(v).copied().unwrap_or(*v))
            .collect();
        let results: Vec<ValueId> = op
            .results
            .iter()
            .map(|&value| {
                let fresh = func.fresh_value();
                final_remap.insert(value, fresh);
                fresh
            })
            .collect();
        let mut cloned = op.clone();
        cloned.operands = operands;
        cloned.results = results;
        stats.ops_added += 1;
        stats.values_changed += cloned.results.len();
        landing_ops.push(cloned);
    }

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
            let root = resolve_copy(&copy_of, v);
            final_remap
                .get(&root)
                .or_else(|| final_remap.get(&v))
                .copied()
                .unwrap_or(v)
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

    // Retire the complete normal-path region and its loop metadata.
    let region_ops_count = cond_ops.len() + body_ops.len();

    // The structural `LoopEnd` marker that paired with this header is now
    // orphaned: with the loop unrolled away there is no `LoopHeader` for it to
    // close. Left in place, the TIR→SimpleIR back-conversion — which pairs
    // `LoopHeader`/`LoopEnd` to re-emit `loop_start`/`loop_end` for the native
    // and WASM backends — would crash on a `LoopEnd` block with no matching
    // header (observed as a backend-daemon compile abort). The frontend emits
    // this marker as an unreachable dead block (no terminator predecessor), so
    // we drop its role; if it is now wholly unreachable we remove the block too.
    let end_marker = c.loop_pairs_end;
    func.retire_loop_metadata(c.header);
    // Retire an unreachable orphaned end marker in the same atomic removal as
    // the loop region: its old backedge may still target the retired header.
    // A marker still reachable or owned by another loop remains intact.
    let reachable = super::reachability::metadata_preserving_reachable_blocks(func);
    let retained = func
        .blocks
        .keys()
        .copied()
        .filter(|bid| {
            !region.contains(bid) && (Some(*bid) != end_marker || reachable.contains(bid))
        })
        .collect();
    func.retain_blocks(&retained)
        .expect("loop unroll rewiring must retire the whole region without dangling edges");

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

    stats.ops_removed += region_ops_count;
}
