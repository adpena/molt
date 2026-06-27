use std::collections::HashMap;

use super::hints::parse_guard_type;
use super::result_inference::infer_result_types_with_attrs;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

// ---------------------------------------------------------------------------
// Guard-to-type-environment propagation (V8 Maglev "known node info")
// ---------------------------------------------------------------------------

/// Information about a TypeGuard: the guarded value, the proven type, and
/// the block + terminator edge identifying the success path.
struct GuardInfo {
    /// The operand being guarded (TypeGuard's operands[0]).
    guarded_value: ValueId,
    /// The TirType proven by the guard.
    proven_type: TirType,
    /// The block containing the TypeGuard.
    guard_block: BlockId,
}

/// After type refinement has converged, propagate TypeGuard-proven types
/// into all dominated blocks. This is the V8 Maglev "known node information"
/// pattern: once a TypeGuard succeeds, all subsequent uses of the guarded
/// value in dominated blocks can use the proven type.
///
/// The function is called after `refine_types` (the fixpoint has converged)
/// and after the type environment `env` has been finalized. It performs a
/// single dominator-based pass to strengthen types.
///
/// Returns the number of additional refinements made and the proven_types map.
pub(super) fn propagate_guard_types(
    func: &TirFunction,
    env: &mut HashMap<ValueId, TirType>,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> (usize, HashMap<ValueId, TirType>) {
    let mut proven_types: HashMap<ValueId, TirType> = HashMap::new();
    let mut refinements = 0usize;

    // Collect all TypeGuard ops with parseable types.
    let mut guards: Vec<GuardInfo> = Vec::new();
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.opcode != OpCode::TypeGuard {
                continue;
            }
            let guarded_value = match op.operands.first().copied() {
                Some(v) => v,
                None => continue,
            };
            let proven_type = match parse_guard_type(&op.attrs) {
                Some(t) => t,
                None => continue,
            };

            // The TypeGuard result itself is proven to be this type.
            if let Some(&result_id) = op.results.first() {
                proven_types.insert(result_id, proven_type.clone());
                let current = env.get(&result_id).cloned().unwrap_or(TirType::DynBox);
                if current != proven_type {
                    env.insert(result_id, proven_type.clone());
                    refinements += 1;
                }
            }

            guards.push(GuardInfo {
                guarded_value,
                proven_type,
                guard_block: bid,
            });
        }
    }

    if guards.is_empty() {
        return (refinements, proven_types);
    }

    // The dominator tree (`idoms`) is supplied by the caller, which already
    // computed it for the same unmodified CFG — no redundant recompute here.

    // For each TypeGuard, find the success branch of the CondBranch that
    // uses the guard result. The then_block is the success path.
    // Then propagate the proven type to all blocks dominated by the
    // success block.
    for guard in &guards {
        let guard_block = match func.blocks.get(&guard.guard_block) {
            Some(b) => b,
            None => continue,
        };

        // Determine which blocks the guard's proven type should propagate to.
        // Case 1: The block terminates with CondBranch using the guard result
        //         as condition -> then_block and its dominated blocks get the
        //         proven type for the guarded value.
        // Case 2: The guard is in a block that unconditionally branches
        //         (the guard already passed or is guaranteed) -> all dominated
        //         blocks get the proven type.
        let success_blocks: Vec<BlockId> = match &guard_block.terminator {
            Terminator::CondBranch {
                cond, then_block, ..
            } => {
                // Check if the cond is the TypeGuard's result.
                let guard_result = guard_block
                    .ops
                    .iter()
                    .find(|op| {
                        op.opcode == OpCode::TypeGuard
                            && op.operands.first() == Some(&guard.guarded_value)
                    })
                    .and_then(|op| op.results.first().copied());

                if guard_result == Some(*cond) {
                    // then_block is the success path — collect it and all
                    // blocks it dominates.
                    let mut dominated = Vec::new();
                    for &bid in func.blocks.keys() {
                        if dominators::dominates(*then_block, bid, idoms) {
                            dominated.push(bid);
                        }
                    }
                    dominated
                } else {
                    // CondBranch cond is not the guard result — the guard
                    // is unconditional within this block. Propagate to all
                    // blocks dominated by the guard's block (but not the
                    // guard block itself, since the guard is within it).
                    let mut dominated = Vec::new();
                    for &bid in func.blocks.keys() {
                        if bid != guard.guard_block
                            && dominators::dominates(guard.guard_block, bid, idoms)
                        {
                            dominated.push(bid);
                        }
                    }
                    dominated
                }
            }
            // Unconditional branch or other terminator — the guard is always
            // live in dominated blocks.
            _ => {
                let mut dominated = Vec::new();
                for &bid in func.blocks.keys() {
                    if bid != guard.guard_block
                        && dominators::dominates(guard.guard_block, bid, idoms)
                    {
                        dominated.push(bid);
                    }
                }
                dominated
            }
        };

        // In all success-dominated blocks, update the type of the guarded
        // value and mark it as proven.
        for &bid in &success_blocks {
            let block = match func.blocks.get(&bid) {
                Some(b) => b,
                None => continue,
            };

            // Check block args: if any block arg receives the guarded value
            // via an incoming edge, it should also be marked proven.
            // (We handle this conservatively: only the original ValueId.)

            // Check all ops in this block for uses of the guarded value.
            for op in &block.ops {
                for &operand in &op.operands {
                    if operand == guard.guarded_value {
                        // The guarded value is used here — mark it proven.
                        proven_types
                            .entry(guard.guarded_value)
                            .or_insert_with(|| guard.proven_type.clone());
                    }
                }
            }

            // Also check terminator operands.
            match &block.terminator {
                Terminator::CondBranch { cond, .. } if *cond == guard.guarded_value => {
                    proven_types
                        .entry(guard.guarded_value)
                        .or_insert_with(|| guard.proven_type.clone());
                }
                Terminator::Return { values } if values.contains(&guard.guarded_value) => {
                    proven_types
                        .entry(guard.guarded_value)
                        .or_insert_with(|| guard.proven_type.clone());
                }
                _ => {}
            }
        }

        // Update the env for the guarded value itself.
        let current = env
            .get(&guard.guarded_value)
            .cloned()
            .unwrap_or(TirType::DynBox);
        if current == TirType::DynBox || current != guard.proven_type {
            // Only refine if it makes the type more specific.
            // If current is DynBox, the guard proves it. If current is
            // already concrete and different, the guard overrides (the guard
            // is more authoritative than inference).
            if current == TirType::DynBox {
                env.insert(guard.guarded_value, guard.proven_type.clone());
                proven_types
                    .entry(guard.guarded_value)
                    .or_insert_with(|| guard.proven_type.clone());
                refinements += 1;
            }
        } else {
            // Current type matches the guard — mark as proven.
            proven_types
                .entry(guard.guarded_value)
                .or_insert_with(|| guard.proven_type.clone());
        }
    }

    // Second pass: propagate proven types through arithmetic chains.
    // If both operands of an arithmetic op are proven, the result is also proven.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = match func.blocks.get(&bid) {
            Some(b) => b,
            None => continue,
        };
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            // Check if all operands are proven.
            let all_proven =
                !op.operands.is_empty() && op.operands.iter().all(|v| proven_types.contains_key(v));
            if !all_proven {
                continue;
            }
            // Infer the result type from proven operand types.
            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| {
                    proven_types
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| env.get(id).cloned().unwrap_or(TirType::DynBox))
                })
                .collect();
            let result_types = infer_result_types_with_attrs(
                op.opcode,
                &operand_types,
                Some(&op.attrs),
                op.results.len(),
            );
            for (&result_id, result_ty) in op.results.iter().zip(result_types) {
                if let Some(result_ty) = result_ty {
                    proven_types.insert(result_id, result_ty.clone());
                    let current = env.get(&result_id).cloned().unwrap_or(TirType::DynBox);
                    if current != result_ty {
                        env.insert(result_id, result_ty.clone());
                        refinements += 1;
                    }
                }
            }
        }
    }

    (refinements, proven_types)
}
