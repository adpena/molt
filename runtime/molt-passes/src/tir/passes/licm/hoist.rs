use std::collections::{HashMap, HashSet};

use super::super::value_range::ValueRange;
use super::PassStats;
use super::safety::is_hoistable;
use crate::tir::analysis::{AnalysisManager, DefMap, LoopForest};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;
/// Find the preheader block for a loop header.
/// The preheader is the unique predecessor of the header that is NOT
/// part of the loop body.  If no unique preheader exists, returns None.
fn find_preheader(
    func: &TirFunction,
    header_bid: BlockId,
    loop_blocks: &HashSet<BlockId>,
) -> Option<BlockId> {
    let mut preds: Vec<BlockId> = Vec::new();
    for (&bid, block) in &func.blocks {
        let targets = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            _ => vec![],
        };
        if targets.contains(&header_bid) && !loop_blocks.contains(&bid) {
            preds.push(bid);
        }
    }
    if preds.len() == 1 {
        Some(preds[0])
    } else {
        None
    }
}

pub(super) fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "licm",
        ..Default::default()
    };

    // Functions with exception handling are NOT skipped wholesale -
    // the per-op `is_hoistable` predicate restricts hoisting to a
    // safe-list of side-effect-free, never-throwing ops (constants,
    // pure arithmetic on already-computed values, etc.) for which
    // hoisting out of a try-bearing loop is observably equivalent
    // to lazy in-loop computation: the only behavioural difference
    // is the trivial "computed in preheader even if loop ran zero
    // iterations" case, which doesn't affect any program output.
    //
    // This is critical for performance: the frontend liberally emits
    // CHECK_EXCEPTION ops, and the lower_from_simple detection sets
    // `has_exception_handling = true` whenever any TryStart/TryEnd/
    // CheckException op appears.  In practice virtually every
    // non-trivial loop body trips this flag, so the prior wholesale
    // skip turned LICM into a no-op for nearly all real code.
    //
    // Loop-detection (natural-loop construction via dominators) is
    // unchanged by exception ops, since try_start/try_end/
    // check_exception don't add back-edges or alter the CFG topology
    // beyond the normal block-splitting that any structured
    // construct does.

    // The loop forest (headers from `loop_roles`, sorted by id for
    // deterministic tie-breaking; bodies via dominator-based natural-loop
    // construction) is shared with BCE through the analysis manager.
    let forest = am.get::<LoopForest>(func).clone();
    if forest.headers.is_empty() {
        return stats;
    }

    // Value-range proof, shared with BCE/SROA via the analysis manager. Used to
    // DISPROVE the throw condition of a `pure_may_throw` op (a shift whose count
    // is in `[0, 63]`, a divide whose divisor is non-zero), which makes that op
    // provably nothrow at the hoist site and therefore LICM-hoistable. Cloned
    // because we take `&mut func.blocks` below; the analysis is a pure function
    // of the function and LICM only moves invariant ops (never changes any value
    // range), so the snapshot stays valid across the hoists. (A hoisted op keeps
    // its operands and result `ValueId`s, so the value-keyed ranges still line
    // up after the move.)
    let vr = am.get::<ValueRange>(func).clone();

    // Process all loops, innermost first, so ops hoisted from an inner
    // loop's body into the inner preheader become visible to the
    // enclosing loop and can be hoisted further out if invariant there.
    //
    // Step 1: index each loop's natural-loop block set by header (headers are
    // already in ascending-id order from the forest). Natural loops nest
    // properly: an inner loop's body is a strict subset of its enclosing
    // outer loop's body.
    let loop_block_sets: Vec<(BlockId, HashSet<BlockId>)> = forest
        .headers
        .iter()
        .map(|&h| (h, forest.bodies[&h].clone()))
        .collect();

    // Step 2: nesting depth = number of OTHER loops whose block set
    // contains this loop's header. A header at depth k is inside k
    // enclosing loops. Sorting by descending depth yields a post-order
    // over the loop forest: every inner loop is processed before its
    // enclosing loop.
    let mut headers_with_depth: Vec<(BlockId, usize)> = loop_block_sets
        .iter()
        .map(|(h, _)| {
            let depth = loop_block_sets
                .iter()
                .filter(|(other_h, other_blocks)| *other_h != *h && other_blocks.contains(h))
                .count();
            (*h, depth)
        })
        .collect();
    // Descending depth -> innermost first. Stable across ties.
    headers_with_depth.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    let loop_headers: Vec<BlockId> = headers_with_depth.into_iter().map(|(h, _)| h).collect();

    // Value -> defining-block map (block args, op results, and params at entry),
    // shared with GVN through the analysis manager. Cloned because LICM mutates
    // its local copy as it hoists ops (recording their new preheader def block);
    // the cached analysis is not mutated.
    let mut value_def_block = am.get::<DefMap>(func).clone();

    // Collect all values used as block arguments in terminators.
    // These participate in phi resolution and must not be hoisted.
    let mut phi_values: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    phi_values.insert(*v);
                }
            }
            Terminator::CondBranch {
                then_args,
                else_args,
                ..
            } => {
                for v in then_args.iter().chain(else_args.iter()) {
                    phi_values.insert(*v);
                }
            }
            _ => {}
        }
    }

    // Index the precomputed loop block sets by header for cheap lookup.
    let loop_blocks_by_header: HashMap<BlockId, HashSet<BlockId>> =
        loop_block_sets.into_iter().collect();

    for header_bid in &loop_headers {
        let loop_blocks = match loop_blocks_by_header.get(header_bid) {
            Some(set) => set,
            None => continue,
        };

        let preheader = match find_preheader(func, *header_bid, loop_blocks) {
            Some(p) => p,
            None => continue, // No unique preheader - can't hoist.
        };

        // Stable iteration order over loop blocks: keeps hoisted-op
        // ordering deterministic for golden-file diffs and for downstream
        // passes that depend on textual TIR equivalence.
        let mut sorted_loop_blocks: Vec<BlockId> = loop_blocks.iter().copied().collect();
        sorted_loop_blocks.sort_unstable_by_key(|b| b.0);

        // Collect invariant ops: ops in loop blocks whose operands are
        // all defined outside the loop.
        // Iterate to fixpoint: hoisting one op may make another invariant.
        for _round in 0..10 {
            let mut hoisted_this_round = 0usize;

            for &loop_bid in &sorted_loop_blocks {
                let block = match func.blocks.get(&loop_bid) {
                    Some(b) => b,
                    None => continue,
                };

                let mut to_hoist: Vec<usize> = Vec::new();

                for (i, op) in block.ops.iter().enumerate() {
                    if !is_hoistable(op, &vr) {
                        continue;
                    }
                    if op.results.is_empty() {
                        continue;
                    }

                    // Skip ops whose results participate in phi nodes.
                    let result_is_phi = op.results.iter().any(|r| phi_values.contains(r));
                    if result_is_phi {
                        continue;
                    }

                    // Check if all operands are defined outside the loop.
                    let all_invariant = op.operands.iter().all(|&operand| {
                        match value_def_block.get(&operand) {
                            Some(def_bid) => !loop_blocks.contains(def_bid),
                            None => true, // Unknown def = conservative: treat as external.
                        }
                    });

                    if all_invariant {
                        to_hoist.push(i);
                    }
                }

                // Hoist ops from back to front to preserve indices.
                for &idx in to_hoist.iter().rev() {
                    let op = func.blocks.get_mut(&loop_bid).unwrap().ops.remove(idx);
                    // Update def_block for the hoisted values.
                    for &res in &op.results {
                        value_def_block.insert(res, preheader);
                    }
                    // Insert at the end of the preheader (before the terminator,
                    // which is handled structurally, not in the ops vec).
                    func.blocks.get_mut(&preheader).unwrap().ops.push(op);
                    hoisted_this_round += 1;
                    stats.ops_removed += 1; // removed from loop
                    stats.ops_added += 1; // added to preheader
                }
            }

            if hoisted_this_round == 0 {
                break;
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
