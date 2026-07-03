//! Tuple scalarization (boxing-elimination) sub-pass.
//!
//! Eliminates intermediate tuples that are built and immediately unpacked
//! (`a, b = b, a + b`). See the module-level docs on [`super`].

use std::collections::HashMap;

use super::super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

// ---------------------------------------------------------------------------
// Tuple Scalarization (Boxing Elimination)
// ---------------------------------------------------------------------------
//
// Eliminates intermediate tuples that are built and immediately unpacked.
//
// ```python
// a, b = b, a + b     # Fibonacci swap
// ```
//
// Before scalarization:
//   %tuple = BuildTuple(%b, %a_plus_b)
//   (%new_a, %new_b) = Copy[_original_kind="unpack_sequence"](%tuple)
//
// After scalarization:
//   %new_a = Copy(%b)
//   %new_b = Copy(%a_plus_b)
//
// The BuildTuple + unpack_sequence pair is pure overhead: a heap allocation
// created and immediately destroyed.  Scalarization replaces this with
// direct SSA value copies -- zero allocation, zero refcount traffic.
//
// Safety conditions:
// 1. The BuildTuple result must not escape (used only by the unpack op).
// 2. The unpack element count must match the BuildTuple operand count.
// 3. Both ops must be in the same block (ensures no intervening control flow).

/// A matched BuildTuple + unpack_sequence pair eligible for scalarization.
#[derive(Debug)]
struct TupleScalarizeCandidate {
    /// Block containing both ops.
    block_id: BlockId,
    /// Index of the BuildTuple op within the block.
    build_idx: usize,
    /// Index of the unpack_sequence (Copy with _original_kind) op within the block.
    unpack_idx: usize,
    /// Operands of the BuildTuple (the element values being packed).
    tuple_elements: Vec<ValueId>,
    /// Results of the unpack_sequence (the unpacked target values).
    unpack_results: Vec<ValueId>,
}

/// Eliminate intermediate tuples that are built and immediately unpacked.
///
/// Scans every block for `BuildTuple` ops whose result is used exactly once
/// by an `unpack_sequence` (represented as `Copy` with `_original_kind`
/// attribute) in the same block.  When the element counts match, both ops
/// are replaced with direct `Copy` ops connecting tuple elements to unpack
/// targets.
pub fn run_tuple_scalarize(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "tuple_scalarize",
        ..Default::default()
    };

    // Phase 1: Build a use-count map for all values.
    // Count uses in both ops and terminators.
    let mut use_counts: HashMap<ValueId, usize> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            for &operand in &op.operands {
                *use_counts.entry(operand).or_insert(0) += 1;
            }
        }
        // Count terminator uses.
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                *use_counts.entry(*cond).or_insert(0) += 1;
                for v in then_args {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
                for v in else_args {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                *use_counts.entry(*value).or_insert(0) += 1;
                for (_, _, args) in cases {
                    for v in args {
                        *use_counts.entry(*v).or_insert(0) += 1;
                    }
                }
                for v in default_args {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for v in args {
                        *use_counts.entry(*v).or_insert(0) += 1;
                    }
                }
                for v in default_args {
                    *use_counts.entry(*v).or_insert(0) += 1;
                }
            }
            Terminator::Unreachable => {}
        }
    }

    // Phase 2: Find scalarization candidates.
    // A candidate is a BuildTuple whose single-result value is used exactly
    // once, and that single use is an unpack_sequence in the same block with
    // matching element count.
    let mut candidates: Vec<TupleScalarizeCandidate> = Vec::new();

    for (&bid, block) in &func.blocks {
        // Index BuildTuple results in this block for quick lookup.
        // Map from result ValueId -> (op index, operands).
        let mut build_tuples: HashMap<ValueId, (usize, Vec<ValueId>)> = HashMap::new();

        for (i, op) in block.ops.iter().enumerate() {
            if op.opcode == OpCode::BuildTuple && op.results.len() == 1 {
                let tuple_val = op.results[0];
                build_tuples.insert(tuple_val, (i, op.operands.clone()));
            }
        }

        if build_tuples.is_empty() {
            continue;
        }

        // Scan for unpack_sequence ops that consume a locally-built tuple.
        for (i, op) in block.ops.iter().enumerate() {
            // unpack_sequence is stored as Copy with _original_kind = "unpack_sequence"
            if op.opcode != OpCode::Copy {
                continue;
            }
            let is_unpack = op
                .attrs
                .get("_original_kind")
                .is_some_and(|v| matches!(v, AttrValue::Str(s) if s == "unpack_sequence"));
            if !is_unpack {
                continue;
            }

            // unpack_sequence has exactly one operand (the tuple) and N results.
            if op.operands.len() != 1 || op.results.is_empty() {
                continue;
            }

            let tuple_val = op.operands[0];

            // Check if this tuple was built in the same block.
            let (build_idx, tuple_elements) = match build_tuples.get(&tuple_val) {
                Some(entry) => (entry.0, &entry.1),
                None => continue,
            };

            // Check that the tuple value is used exactly once (by this unpack).
            // This guarantees the tuple doesn't escape.
            let count = use_counts.get(&tuple_val).copied().unwrap_or(0);
            if count != 1 {
                continue;
            }

            // Check element count match.
            if tuple_elements.len() != op.results.len() {
                continue;
            }

            // The BuildTuple must come before the unpack in the same block.
            if build_idx >= i {
                continue;
            }

            candidates.push(TupleScalarizeCandidate {
                block_id: bid,
                build_idx,
                unpack_idx: i,
                tuple_elements: tuple_elements.to_vec(),
                unpack_results: op.results.clone(),
            });
        }
    }

    if candidates.is_empty() {
        return stats;
    }

    // Phase 3: Apply scalarization.
    // Process candidates per-block, sorted by descending op index so that
    // removals don't invalidate earlier indices.
    //
    // Group candidates by block.
    let mut by_block: HashMap<BlockId, Vec<&TupleScalarizeCandidate>> = HashMap::new();
    for c in &candidates {
        by_block.entry(c.block_id).or_default().push(c);
    }

    for (bid, mut block_candidates) in by_block {
        // Sort by descending unpack_idx so we can remove from the end first.
        block_candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.unpack_idx));

        let block = func.blocks.get_mut(&bid).unwrap();

        for candidate in &block_candidates {
            // Build replacement Copy ops for each element.
            let copy_ops: Vec<TirOp> = candidate
                .tuple_elements
                .iter()
                .zip(candidate.unpack_results.iter())
                .map(|(&src, &dst)| TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: vec![src],
                    results: vec![dst],
                    attrs: AttrDict::new(),
                    source_span: None,
                })
                .collect();

            let n_copies = copy_ops.len();

            // Remove the unpack_sequence op (higher index first).
            block.ops.remove(candidate.unpack_idx);
            stats.ops_removed += 1;

            // Remove the BuildTuple op.
            block.ops.remove(candidate.build_idx);
            stats.ops_removed += 1;

            // Insert the Copy ops at the BuildTuple's former position.
            // After removing both ops, the insertion point is build_idx
            // (the unpack was after the build, and removing build shifted
            // everything down by 1, but we already removed the unpack which
            // was at a higher index, so build_idx is still correct).
            for (j, copy_op) in copy_ops.into_iter().enumerate() {
                block.ops.insert(candidate.build_idx + j, copy_op);
            }
            stats.ops_added += n_copies;
            stats.values_changed += 1;
        }
    }

    stats
}
