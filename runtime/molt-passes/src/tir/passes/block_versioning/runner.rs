use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{AnalysisManager, LoopForest, PredMap};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::analysis::{
    GuardCandidate, TypeContext, build_producing_op_map, parse_guard_type, value_proves_type,
};
use super::block_clone::{clone_block_with_fresh_values, redirect_terminator};

/// Run the SBBV pass on a TIR function.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "block_versioning",
        ..Default::default()
    };

    if func.blocks.is_empty() {
        return stats;
    }

    // Conservative bail-out: exception handling makes versioning unsafe because
    // a TypeGuard failure inside a try region must propagate to the handler.
    // Because the pass only proceeds when `has_exception_handling == false`,
    // the cached full-CFG `PredMap` has no exception edges and coincides with
    // the terminator-only predecessor relation used for versioning rewrites.
    if func.has_exception_handling {
        return stats;
    }

    let pred_map = am.get::<PredMap>(func).clone();
    let loop_headers: HashSet<BlockId> =
        am.get::<LoopForest>(func).headers.iter().copied().collect();
    let producing_ops = build_producing_op_map(func);

    // Build block-argument type map for type proofs.
    let mut block_arg_types: HashMap<ValueId, TirType> = HashMap::new();
    for block in func.blocks.values() {
        for arg in &block.args {
            block_arg_types.insert(arg.id, arg.ty.clone());
        }
    }

    // Phase 1: Identify versioning candidates.
    // A candidate is a non-loop-header block that contains a TypeGuard with a
    // parseable type and has at least one predecessor that can prove the guard.
    struct VersioningCandidate {
        block_id: BlockId,
        guard: GuardCandidate,
    }

    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    let mut candidates: Vec<VersioningCandidate> = Vec::new();

    for &bid in &block_ids {
        // Do not version loop headers — loop-carried contexts are unreliable.
        if loop_headers.contains(&bid) {
            continue;
        }

        let block = &func.blocks[&bid];

        // Find the first TypeGuard in this block (we version on at most one
        // guard per block to stay within k=2).
        let guard = block.ops.iter().enumerate().find_map(|(idx, op)| {
            if op.opcode != OpCode::TypeGuard {
                return None;
            }
            let guarded_value = *op.operands.first()?;
            let proven_type = parse_guard_type(op)?;
            let result = *op.results.first()?;
            Some(GuardCandidate {
                op_index: idx,
                context: TypeContext {
                    guarded_value,
                    proven_type,
                },
                result,
            })
        });

        let guard = match guard {
            Some(g) => g,
            None => continue,
        };

        // Check that at least one predecessor can prove the guard.
        let preds = pred_map.get(&bid).map(|v| v.as_slice()).unwrap_or(&[]);
        let any_proves = preds.iter().any(|pred_bid| {
            // The guarded value may be a block argument. Trace through the
            // predecessor's branch args to find the actual source value.
            let guarded = guard.context.guarded_value;
            let block = match func.blocks.get(&bid) {
                Some(b) => b,
                None => return false,
            };
            // Find the position of the guarded value in this block's args
            let arg_pos = block.args.iter().position(|a| a.id == guarded);
            let source_value = if let Some(pos) = arg_pos {
                // Trace through predecessor's branch args
                let pred_block = match func.blocks.get(pred_bid) {
                    Some(b) => b,
                    None => return false,
                };
                match &pred_block.terminator {
                    Terminator::Branch { args, .. } => args.get(pos).copied(),
                    Terminator::CondBranch {
                        then_args,
                        else_args,
                        then_block,
                        else_block,
                        ..
                    } => {
                        if *then_block == bid {
                            then_args.get(pos).copied()
                        } else if *else_block == bid {
                            else_args.get(pos).copied()
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            } else {
                Some(guarded) // Not a block arg — use directly
            };
            match source_value {
                Some(src) => value_proves_type(
                    src,
                    &guard.context.proven_type,
                    &producing_ops,
                    &block_arg_types,
                ),
                None => false,
            }
        });

        if !any_proves {
            continue;
        }

        candidates.push(VersioningCandidate {
            block_id: bid,
            guard,
        });
    }

    if candidates.is_empty() {
        return stats;
    }

    // Phase 2: Create specialized block versions and rewire predecessors.
    for candidate in candidates {
        let orig_bid = candidate.block_id;

        // Allocate a new block ID for the specialized version.
        let spec_bid = func.fresh_block();

        // Clone the original block with fresh SSA values.
        let orig_block = func.blocks[&orig_bid].clone();
        let (mut spec_block, value_remap) =
            clone_block_with_fresh_values(&orig_block, spec_bid, func);

        // In the specialized block, remove the TypeGuard op and replace uses
        // of its result with ConstBool(true) — the guard is known to pass.
        let guard_result_in_spec = *value_remap
            .get(&candidate.guard.result)
            .unwrap_or(&candidate.guard.result);
        let guard_operand_in_spec = *value_remap
            .get(&candidate.guard.context.guarded_value)
            .unwrap_or(&candidate.guard.context.guarded_value);

        // Remove the TypeGuard op from the specialized block.
        spec_block.ops.remove(candidate.guard.op_index);
        stats.ops_removed += 1;

        // Insert a ConstBool(true) to replace the guard result, using the same
        // ValueId so all downstream uses are satisfied.
        let const_true_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![guard_result_in_spec],
            attrs: {
                let mut m = HashMap::new();
                m.insert("value".to_string(), AttrValue::Bool(true));
                m
            },
            source_span: None,
        };
        // Insert at the position where the guard was (maintaining op order).
        spec_block
            .ops
            .insert(candidate.guard.op_index, const_true_op);
        stats.ops_added += 1;

        // Refine the type of the guarded value's block arg in the specialized
        // block, if the guarded value is a block argument.
        for arg in &mut spec_block.args {
            if arg.id == guard_operand_in_spec {
                arg.ty = candidate.guard.context.proven_type.clone();
            }
        }

        // Insert the specialized block into the function.
        func.blocks.insert(spec_bid, spec_block);
        stats.values_changed += 1; // track as "blocks versioned"

        // Phase 3: Rewire predecessors.
        // Predecessors that can prove the guard type go to spec_bid.
        // Others stay pointed at orig_bid.
        let preds: Vec<BlockId> = pred_map.get(&orig_bid).cloned().unwrap_or_default();

        // Build an empty arg remap — branch arguments that correspond to block
        // args need to be remapped for the specialized version.
        // The value_remap maps old block-arg ValueIds to new ones, but the
        // branch arguments are the VALUES passed by the predecessor, not the
        // block-arg IDs. So we don't remap branch args — only the target.
        let empty_remap: HashMap<ValueId, ValueId> = HashMap::new();

        for pred_id in preds {
            // Skip self-edges (shouldn't exist for non-loop-headers, but safe).
            if pred_id == orig_bid {
                continue;
            }

            // Trace through predecessor's branch args to find the source value
            let guarded = candidate.guard.context.guarded_value;
            let orig_block = &func.blocks[&orig_bid];
            let arg_pos = orig_block.args.iter().position(|a| a.id == guarded);
            let source_value = if let Some(pos) = arg_pos {
                let pred_block = &func.blocks[&pred_id];
                match &pred_block.terminator {
                    Terminator::Branch { args, .. } => args.get(pos).copied(),
                    Terminator::CondBranch {
                        then_args,
                        else_args,
                        then_block,
                        else_block,
                        ..
                    } => {
                        if *then_block == orig_bid {
                            then_args.get(pos).copied()
                        } else if *else_block == orig_bid {
                            else_args.get(pos).copied()
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            } else {
                Some(guarded)
            };
            let can_prove = source_value.is_some_and(|src| {
                value_proves_type(
                    src,
                    &candidate.guard.context.proven_type,
                    &producing_ops,
                    &block_arg_types,
                )
            });

            if can_prove {
                let pred_block = func.blocks.get_mut(&pred_id).unwrap();
                redirect_terminator(&mut pred_block.terminator, orig_bid, spec_bid, &empty_remap);
            }
            // Otherwise, keep the edge to the generic (original) block.
        }
    }

    stats
}

// ---------------------------------------------------------------------------
