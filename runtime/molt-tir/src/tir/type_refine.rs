use std::collections::{HashMap, HashSet};

use super::blocks::BlockId;
use super::dominators;
use super::function::TirFunction;
use super::ops::OpCode;
use super::types::TirType;
use super::values::ValueId;

mod cfg_edges;
mod facts;
mod guards;
mod hints;
mod proven;
mod result_inference;
#[cfg(test)]
mod tests;

use self::cfg_edges::collect_branch_edges;
use self::facts::{
    fact_or_bottom, is_bottom_type, is_refined_public_type, join_assign_type_fact,
    publish_fact_type,
};
use self::guards::propagate_guard_types;
use self::hints::parse_guard_type;
#[cfg(test)]
use self::hints::parse_return_type_str;
pub use self::proven::extract_proven_map;
pub(super) use self::result_inference::infer_scalar_return_result_type;
use self::result_inference::{
    attr_result_type_override, infer_result_facts_with_attrs, infer_result_types_with_attrs,
};

/// Maximum number of fixpoint iterations before a fail-closed nonconvergence
/// diagnostic.
const MAX_ROUNDS: usize = 20;

/// Extract a map from every [`ValueId`] to its refined [`TirType`] in a
/// **post-refinement** TIR function.  Block argument types come from the
/// function directly (they were written back by [`refine_types`]); op result
/// types are re-inferred in a single forward pass (safe because refinement
/// has already converged).
///
/// After the forward inference pass, TypeGuard-proven types are propagated
/// into dominated blocks via the dominator tree.
pub fn extract_type_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut env: HashMap<ValueId, TirType> = func.value_types.clone();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = &func.blocks[&bid];

        // Block arguments carry their type in-place and mirror it into the
        // function-owned value map for consumers that need a single query
        // surface.
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }

        // Re-infer op result types from operand types (single pass — the
        // fixpoint has already converged so one pass is sufficient).
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            if op
                .results
                .iter()
                .all(|result_id| env.contains_key(result_id))
            {
                continue;
            }

            // TypeGuard results get the proven type directly.
            if op.opcode == OpCode::TypeGuard
                && let Some(proven_ty) = parse_guard_type(&op.attrs)
            {
                for &result_id in &op.results {
                    env.entry(result_id).or_insert_with(|| proven_ty.clone());
                }
                continue;
            }

            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            let inferred_types = infer_result_types_with_attrs(
                op.opcode,
                &operand_types,
                Some(&op.attrs),
                op.results.len(),
            );
            for (&result_id, inferred) in op.results.iter().zip(inferred_types) {
                if let Some(inferred) = inferred {
                    env.insert(result_id, inferred);
                } else {
                    env.insert(result_id, TirType::DynBox);
                }
            }
        }
    }

    env
}

/// Refine types in a TIR function.
/// Iterates to fixpoint (max 20 rounds, fail-closed on timeout).
/// Returns the number of values refined from DynBox to concrete types.
pub fn refine_types(func: &mut TirFunction) -> usize {
    let mut all_value_ids: HashSet<ValueId> = HashSet::new();
    let mut defined_values: HashSet<ValueId> = HashSet::new();
    let mut produced_values: HashSet<ValueId> = HashSet::new();

    for block in func.blocks.values() {
        for arg in &block.args {
            all_value_ids.insert(arg.id);
            defined_values.insert(arg.id);
        }
        for op in &block.ops {
            for &operand_id in &op.operands {
                all_value_ids.insert(operand_id);
            }
            for &result_id in &op.results {
                all_value_ids.insert(result_id);
                defined_values.insert(result_id);
                produced_values.insert(result_id);
            }
        }
    }
    for &id in func.value_types.keys() {
        all_value_ids.insert(id);
    }

    // `Never` is bottom: the solver has not yet seen the producer's facts.
    // `DynBox` is top: the value is dynamically typed and must never be
    // narrowed again by a later transfer in this fixpoint.
    let mut facts: HashMap<ValueId, TirType> = HashMap::new();
    for (&id, ty) in &func.value_types {
        if produced_values.contains(&id) {
            continue;
        }
        if !defined_values.contains(&id) || !matches!(ty, TirType::DynBox) {
            facts.insert(id, ty.clone());
        }
    }

    if let Some(entry) = func.blocks.get_mut(&func.entry_block) {
        for (arg, param_ty) in entry.args.iter_mut().zip(func.param_types.iter()) {
            arg.ty = param_ty.clone();
            facts.insert(arg.id, param_ty.clone());
        }
    }

    // Track which values started as DynBox so we can count refinements.
    let initially_dynbox: Vec<ValueId> = all_value_ids
        .iter()
        .filter(|id| {
            !matches!(
                facts.get(id),
                Some(ty) if is_refined_public_type(ty)
            )
        })
        .copied()
        .collect();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    // Pre-compute: for each block, collect all incoming edges (predecessor
    // block → arg values). We accumulate across all blocks' terminators.
    // Key: target BlockId. Value: list of (source BlockId, incoming arg
    // values). Source identity matters because back-edges (incoming edges
    // from blocks dominated by the target) must be treated specially when
    // seeding loop-induction-variable block-arg types — see the seed pass
    // below (after `eh_handler_args` is computed so the seed honors the
    // EH-handler exclusion).
    let mut incoming_edges: HashMap<BlockId, Vec<(BlockId, Vec<ValueId>)>> = HashMap::new();
    for (source_bid, block) in &func.blocks {
        let edges = collect_branch_edges(block);
        for (target_id, arg_values) in edges {
            incoming_edges
                .entry(target_id)
                .or_default()
                .push((*source_bid, arg_values));
        }
    }
    let pred_map = dominators::build_pred_map(func);
    let idoms = dominators::compute_idoms(func, &pred_map);
    let reachable_blocks = dominators::executable_reachable_blocks(func);

    // Pre-compute op snapshots once (ops don't change during refinement,
    // only the type environment does). Avoids O(ops × rounds) Vec allocations.
    //
    // We snapshot the `return_type` attr (when present) into a typed
    // `TirType` rather than carrying the full `AttrDict` — the attr is
    // immutable across rounds and cloning AttrDict per op per round
    // would dominate the refinement cost.
    let ops_by_block: HashMap<BlockId, Vec<(OpCode, Vec<ValueId>, Vec<ValueId>, Option<TirType>)>> =
        block_order
            .iter()
            .map(|&bid| {
                let ops = func.blocks[&bid]
                    .ops
                    .iter()
                    .map(|op| {
                        // The attr-derived AUTHORITATIVE result-type override: a
                        // type the OP ITSELF determines, which must win over
                        // operand-based inference. Producers (attr-keyed, so
                        // pre-extracted HERE — the fixpoint snapshot drops attrs):
                        //
                        //  1. Object allocation `_type_hint` — structural class
                        //     identity minted by the allocator itself.
                        //  2. Call/CallMethod/CallBuiltin `return_type` — the
                        //     frontend's structural return type. (Legacy
                        //     `_type_hint` is semantic transport metadata and must
                        //     NOT refine representation, so it is ignored.)
                        //  3. CallBuiltin `name` for structural builtin return
                        //     types (`len`, predicates, `ord`, `chr`).
                        //  4. TypeGuard's proven type.
                        //  5. A `Copy`-spelled fresh value (the SSA converter's
                        //     fallback for ops without a dedicated OpCode). Two
                        //     classifier-backed cases, in priority order:
                        //     (a) RAW-CARRIER scalar conversions
                        //         (`copy_kind_raw_carrier_type`: int/float/bool
                        //         conversions) mint a NEW raw-register value typed
                        //         by the CONVERSION — operand-0 propagation here is
                        //         the round-8 repr miscompile (`int(t)`, t: float,
                        //         typed F64 → def_var repr mismatch).
                        //     (b) other fresh-value-minting kinds
                        //         (`copy_kind_mints_fresh_owned_ref`) pin their
                        //         intrinsic type (`fresh_value_kind_result_type`) —
                        //         the #45 fix (`complex_from_obj` typed F64 from its
                        //         real-part operand routed float+complex down the
                        //         unboxed fadd path).
                        //     Everything else (transparent aliases) propagates
                        //     operand 0's type.
                        let result_type_override = attr_result_type_override(op.opcode, &op.attrs);
                        (
                            op.opcode,
                            op.operands.clone(),
                            op.results.clone(),
                            result_type_override,
                        )
                    })
                    .collect();
                (bid, ops)
            })
            .collect();

    // When exception handling is present, identify blocks that start with
    // StateBlockStart (exception handler entry points). Block arguments of
    // these blocks should stay DynBox — the exception may come from any
    // type context, so propagating a refined type would be unsound.
    let has_eh = func.has_exception_handling;
    let mut eh_handler_args: std::collections::HashSet<ValueId> = std::collections::HashSet::new();
    if has_eh {
        for block in func.blocks.values() {
            let is_exception_label_entry = func.label_id_map.contains_key(&block.id.0);
            // A block whose first op is StateBlockStart or CheckException
            // is an exception handler. A block named by `label_id_map` is also
            // an exception-label entry even when it is an empty forwarding
            // block; implicit exception edges can land there with values from
            // arbitrary protected contexts. In both cases, args must stay
            // DynBox and downstream merge args must see that top fact.
            let starts_with_handler_marker = block.ops.first().is_some_and(|first_op| {
                matches!(
                    first_op.opcode,
                    OpCode::StateBlockStart | OpCode::CheckException
                )
            });
            if is_exception_label_entry || starts_with_handler_marker {
                for arg in &block.args {
                    eh_handler_args.insert(arg.id);
                    facts.insert(arg.id, TirType::DynBox);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Loop-induction-variable seeding (Lattner-style narrow-then-widen).
    // ---------------------------------------------------------------------------
    //
    // The fixpoint below is monotonic upward (`meet` is lattice join in
    // this codebase's terminology — `meet(I64, DynBox) = DynBox`).
    // Without this pre-pass, a block-arg whose entry-edge brings I64
    // and whose back-edge brings `Add(self_arg, ConstInt)` is stuck at
    // DynBox: the body sees `i: DynBox` initially, infers
    // `Add(DynBox, I64)` → no type, stays DynBox, the back-edge brings
    // DynBox, and `meet(I64-from-entry, DynBox-from-back) = DynBox`
    // widens the entry-side I64 in the very first round.
    //
    // Fix: pre-seed each block-arg's env entry to the meet of its
    // **non-back-edge** incoming values' types. Back-edges (incoming
    // edges from blocks the target dominates) are excluded from the
    // initial seed so the body's inference can run with the optimistic
    // entry type, after which the fixpoint runs normally and either
    // confirms the seed (back-edge type matches) or widens it.
    //
    // This is sound: in SSA, a block arg's only mutators are its
    // incoming edges. If the body actually re-types the loop arg
    // (e.g., `i = obj.foo(i)` returning DynBox), the back-edge brings
    // DynBox, the fixpoint widens to DynBox, and we land at the same
    // conservative type as before. The seeding only **gains** precision
    // — it never loses it.
    //
    // Two sub-passes are required:
    //   (a) **Warmup**: a single forward pass over op result types so
    //       that const ops (`ConstInt`, `ConstStr`, etc.) and any other
    //       op whose operand types are already known produce refined
    //       values in `env`. Without warmup, every op result starts as
    //       DynBox (from the bulk init at the top of `refine_types`)
    //       and the seed reads DynBox for the entry-edge value (e.g.,
    //       `i_init = ConstInt(0)` is still DynBox), defeating the seed.
    //   (b) **Seed**: meet of non-back-edge incoming types per block-arg.
    {
        // (a) Warmup — single forward pass over op result types.
        for &block_id in &block_order {
            let ops_snapshot = &ops_by_block[&block_id];
            for (opcode, operands, results, return_type_hint) in ops_snapshot {
                if results.is_empty() {
                    continue;
                }
                if has_eh && matches!(opcode, OpCode::CheckException) {
                    for &result_id in results {
                        facts.insert(result_id, TirType::DynBox);
                    }
                    continue;
                }
                let operand_facts: Vec<TirType> = operands
                    .iter()
                    .map(|id| fact_or_bottom(&facts, *id))
                    .collect();
                let inferred_facts = if results.len() == 1 {
                    vec![return_type_hint.clone().unwrap_or_else(|| {
                        infer_result_facts_with_attrs(*opcode, &operand_facts, None, 1)
                            .into_iter()
                            .next()
                            .unwrap_or(TirType::Never)
                    })]
                } else {
                    infer_result_facts_with_attrs(*opcode, &operand_facts, None, results.len())
                };
                for (&result_id, inferred) in results.iter().zip(inferred_facts) {
                    join_assign_type_fact(&mut facts, result_id, inferred);
                }
            }
        }

        // (b) Seed block-args from non-back-edge incoming meets.
        for (&block_id, edge_list) in &incoming_edges {
            if !reachable_blocks.contains(&block_id) {
                continue;
            }
            let block_args: Vec<(usize, ValueId)> = match func.blocks.get(&block_id) {
                Some(block) => block
                    .args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (i, a.id))
                    .collect(),
                None => continue,
            };
            if block_args.is_empty() {
                continue;
            }
            for (i, arg_id) in block_args {
                // Honor the EH-handler exclusion the fixpoint also
                // enforces — exception handler args must stay DynBox.
                if eh_handler_args.contains(&arg_id) {
                    continue;
                }
                let mut accumulated = TirType::Never;
                let mut saw_non_back_edge = false;
                for (source_bid, edge_args) in edge_list {
                    if i >= edge_args.len() {
                        continue;
                    }
                    if !reachable_blocks.contains(source_bid) {
                        continue;
                    }
                    // A back-edge is one where the target block
                    // dominates the source block (Muchnick §13.4).
                    let is_back_edge = dominators::dominates(block_id, *source_bid, &idoms);
                    if is_back_edge {
                        continue;
                    }
                    let incoming_fact = facts.get(&edge_args[i]).cloned().unwrap_or(TirType::Never);
                    accumulated = accumulated.meet(&incoming_fact);
                    saw_non_back_edge = true;
                }
                if saw_non_back_edge {
                    let current = fact_or_bottom(&facts, arg_id);
                    if is_bottom_type(&current) {
                        if !is_bottom_type(&accumulated) {
                            facts.insert(arg_id, accumulated);
                        }
                    } else {
                        join_assign_type_fact(&mut facts, arg_id, accumulated);
                    }
                }
            }
        }
    }

    // Fixpoint iteration over a finite-height fact lattice.
    //
    // Each round splits into TWO phases:
    //   Phase 1 — propagate op result types from operand types in every
    //     block.
    //   Phase 2 — recompute block-arg types from incoming-edge meets in
    //     every block.
    //
    // The split matters because op results in a loop body depend on the
    // current type of the loop's induction-variable block-arg. With the
    // per-block "ops-then-args" order, recomputing the header's arg
    // (Phase 2) before the body's ops (Phase 1) collapsed any IV
    // pre-seed back to DynBox via the still-DynBox back-edge. Splitting
    // ensures the body's ops always see the latest header-arg type from
    // the previous round (or the seed in round 0), producing a refined
    // back-edge value that the next round's Phase 2 confirms.
    let mut converged = false;
    for _round in 0..MAX_ROUNDS {
        let mut changed = false;

        // Phase 1: op result types in every block.
        for &block_id in &block_order {
            let ops_snapshot = &ops_by_block[&block_id];

            for (opcode, operands, results, return_type_hint) in ops_snapshot {
                if results.is_empty() {
                    continue;
                }

                // Do not refine results of CheckException — the value
                // coming out of an exception check is dynamically typed.
                if has_eh && matches!(opcode, OpCode::CheckException) {
                    for &result_id in results {
                        changed |= join_assign_type_fact(&mut facts, result_id, TirType::DynBox);
                    }
                    continue;
                }

                let operand_facts: Vec<TirType> = operands
                    .iter()
                    .map(|id| fact_or_bottom(&facts, *id))
                    .collect();

                // Frontend-provided return-type hint takes precedence for
                // opaque call-like opcodes; falls back to operand-based
                // inference for everything else.
                let inferred_facts = if results.len() == 1 {
                    vec![return_type_hint.clone().unwrap_or_else(|| {
                        infer_result_facts_with_attrs(*opcode, &operand_facts, None, 1)
                            .into_iter()
                            .next()
                            .unwrap_or(TirType::Never)
                    })]
                } else {
                    infer_result_facts_with_attrs(*opcode, &operand_facts, None, results.len())
                };

                for (&result_id, inferred) in results.iter().zip(inferred_facts) {
                    changed |= join_assign_type_fact(&mut facts, result_id, inferred);
                }
            }
        }

        // Phase 2: block-arg types in every block.
        for &block_id in &block_order {
            if !reachable_blocks.contains(&block_id) {
                continue;
            }
            // Recompute block argument types from all incoming edges.
            // Start from Never (bottom) and meet all incoming values.
            if let Some(edge_list) = incoming_edges.get(&block_id) {
                let arg_count = func.blocks[&block_id].args.len();
                for i in 0..arg_count {
                    let arg_id = func.blocks[&block_id].args[i].id;

                    // Exception handler block args must stay DynBox —
                    // the exception could come from any type context.
                    if eh_handler_args.contains(&arg_id) {
                        changed |= join_assign_type_fact(&mut facts, arg_id, TirType::DynBox);
                        continue;
                    }

                    let mut accumulated = TirType::Never;
                    for (source_bid, edge_args) in edge_list {
                        if !reachable_blocks.contains(source_bid) {
                            continue;
                        }
                        if i < edge_args.len() {
                            let incoming_fact =
                                facts.get(&edge_args[i]).cloned().unwrap_or(TirType::Never);
                            accumulated = accumulated.meet(&incoming_fact);
                        }
                    }
                    changed |= join_assign_type_fact(&mut facts, arg_id, accumulated);
                }
            }
        }

        if !changed {
            converged = true;
            break;
        }
    }

    assert!(
        converged,
        "type refinement failed to converge after {MAX_ROUNDS} rounds in {}",
        func.name
    );

    let mut env: HashMap<ValueId, TirType> = HashMap::new();
    for &id in &all_value_ids {
        env.insert(
            id,
            facts
                .get(&id)
                .cloned()
                .map(publish_fact_type)
                .unwrap_or(TirType::DynBox),
        );
    }
    for (&id, fact) in &facts {
        env.entry(id)
            .or_insert_with(|| publish_fact_type(fact.clone()));
    }

    // --- Guard-to-type-environment propagation ---
    // After the fixpoint has converged, propagate TypeGuard-proven types
    // into all dominated blocks. This is additive and cannot break the
    // existing fixpoint — it only strengthens types that were DynBox.
    // Reuse the dominator tree already computed above (the fixpoint loop only
    // refines types, never the CFG), instead of recomputing it.
    let (guard_refinements, _proven) = propagate_guard_types(func, &mut env, &idoms);

    // Write refined types back into the function-owned map and mirror block
    // argument entries into their in-place `TirValue` records.
    for block in func.blocks.values_mut() {
        for arg in &mut block.args {
            if let Some(ty) = env.get(&arg.id) {
                arg.ty = ty.clone();
            }
        }
    }
    func.value_types = env.clone();

    // Count refinements: values that started as DynBox and are now concrete.
    let fixpoint_refinements = initially_dynbox
        .iter()
        .filter(|id| {
            env.get(id)
                .map(|ty| !matches!(ty, TirType::DynBox))
                .unwrap_or(false)
        })
        .count();

    fixpoint_refinements + guard_refinements
}
