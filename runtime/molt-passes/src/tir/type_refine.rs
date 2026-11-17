use std::collections::VecDeque;
use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::blocks::BlockId;
use super::dominators;
use super::function::TirFunction;
use super::ops::OpCode;
use super::types::TirType;
use super::values::ValueId;

mod cfg_edges;
mod exact;
mod facts;
mod hints;
mod proven;
mod result_inference;
#[cfg(test)]
mod tests;

use self::cfg_edges::collect_branch_edges;
pub use self::exact::{
    ExactScalarWitness, ExactSeeds, exact_scalar_witnesses, extract_exact_scalar_map,
    solve_exact_scalars,
};
pub(crate) use self::facts::infer_return_type;
use self::facts::{
    fact_or_bottom, is_bottom_type, is_refined_public_type, join_assign_type_fact,
    publish_fact_type,
};
#[cfg(test)]
use self::hints::parse_return_type_str;
pub use self::proven::extract_proven_map;
use self::result_inference::{
    attr_result_type_override, infer_result_facts_with_attrs, infer_result_types_with_attrs,
};

/// Extract a map from every [`ValueId`] to its refined [`TirType`] in a
/// **post-refinement** TIR function.  Block argument types come from the
/// function directly (they were written back by [`refine_types`]); op result
/// types are re-inferred in a single forward pass (safe because refinement
/// has already converged).
pub fn extract_type_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let exact = extract_exact_scalar_map(func);
    extract_type_map_with_exact(func, &exact)
}

fn extract_type_map_with_exact(
    func: &TirFunction,
    exact: &HashMap<ValueId, TirType>,
) -> HashMap<ValueId, TirType> {
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
            if let Some(result_type) = crate::tir::op_semantics::op_instance_facts_for_op(op, exact)
                .and_then(|facts| facts.result_type)
            {
                for &result in &op.results {
                    env.insert(result, result_type.clone());
                }
                continue;
            }
            if op
                .results
                .iter()
                .all(|result_id| env.contains_key(result_id))
            {
                continue;
            }

            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            let inferred_types = infer_result_types_with_attrs(
                op.opcode,
                op.origin,
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
/// Propagates changed facts to their users until the worklist is empty.
/// Returns the number of values refined from DynBox to concrete types.
pub fn refine_types(func: &mut TirFunction) -> usize {
    let exact = extract_exact_scalar_map(func);
    let mut all_value_ids: HashSet<ValueId> = HashSet::default();
    let mut defined_values: HashSet<ValueId> = HashSet::default();
    let mut produced_values: HashSet<ValueId> = HashSet::default();

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
    let mut facts: HashMap<ValueId, TirType> = HashMap::default();
    for (&id, ty) in &func.value_types {
        if produced_values.contains(&id) {
            continue;
        }
        if !defined_values.contains(&id) || !matches!(ty, TirType::DynBox) {
            facts.insert(id, ty.clone());
        }
    }

    if let Some(entry) = func.blocks.get_mut(&func.entry_block) {
        for (index, (arg, param_ty)) in entry
            .args
            .iter_mut()
            .zip(func.param_types.iter())
            .enumerate()
        {
            // A clone's proven entry type is at least as precise as an
            // annotation (design 78, 3.7).
            let param_ty = match func.exact_param_types.get(index) {
                Some(exact) if *exact != TirType::DynBox => exact,
                _ => param_ty,
            };
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
    let mut incoming_edges: HashMap<BlockId, Vec<(BlockId, Vec<ValueId>)>> = HashMap::default();
    for &source_bid in &block_order {
        let block = &func.blocks[&source_bid];
        let edges = collect_branch_edges(block);
        for (target_id, arg_values) in edges {
            incoming_edges
                .entry(target_id)
                .or_default()
                .push((source_bid, arg_values));
        }
    }
    let pred_map = dominators::build_pred_map(func);
    let tree = dominators::compute_dominator_tree(func, &pred_map);
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
                        //  2. Call/CallMethod `return_type` — the
                        //     frontend's structural return type. (Legacy
                        //     `_type_hint` is semantic transport metadata and must
                        //     NOT refine representation, so it is ignored.)
                        //  3. Public builtin lookup has no name-derived
                        //     result proof; arbitrary replacements stay dynamic.
                        //  4. A `Copy`-spelled owned value (the SSA converter's
                        //     fallback for ops without a dedicated OpCode). Two
                        //     classifier-backed cases, in priority order:
                        //     (a) RAW-CARRIER scalar conversions
                        //         (`copy_kind_raw_carrier_type`: int/float/bool
                        //         conversions) mint a NEW raw-register value typed
                        //         by the CONVERSION — operand-0 propagation here is
                        //         the round-8 repr miscompile (`int(t)`, t: float,
                        //         typed F64 → def_var repr mismatch).
                        //     (b) other owned-result kinds
                        //         (`copy_kind_mints_owned_value`) pin their
                        //         intrinsic type (`owned_value_kind_result_type`) —
                        //         the #45 fix (`complex_from_obj` typed F64 from its
                        //         real-part operand routed float+complex down the
                        //         unboxed fadd path).
                        //     Everything else (transparent aliases) propagates
                        //     operand 0's type.
                        let result_type_override = if op.has_valid_shape() {
                            crate::tir::op_semantics::op_instance_facts_for_op(op, &exact)
                                .and_then(|facts| facts.result_type)
                                .or_else(|| {
                                    attr_result_type_override(
                                        op.opcode,
                                        op.origin,
                                        &op.attrs,
                                        &op.operands,
                                    )
                                })
                        } else {
                            None
                        };
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

    // A coroutine frame's spill slot holds only what the frame split stores
    // there, so a reload has the type of those stores.
    let spill_sources = spill_slot_sources(func);

    // Match exact provenance's exceptional-entry admission. Normal loop labels
    // and blocks beginning with an exception poll still have ordinary inputs.
    let has_eh = func.has_exception_handling;
    let handlers = cfg_edges::handler_entry_blocks(func);
    let mut eh_handler_args: molt_ir::deterministic_hash::HashSet<ValueId> = molt_ir::deterministic_hash::HashSet::default();
    for block in func.blocks.values() {
        if handlers.contains(&block.id) {
            for arg in &block.args {
                eh_handler_args.insert(arg.id);
                facts.insert(arg.id, TirType::DynBox);
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Loop-induction-variable seeding (Lattner-style narrow-then-widen).
    // ---------------------------------------------------------------------------
    //
    // The fixpoint below is monotonic upward (`meet` is lattice join in
    // this codebase's terminology — `meet(I64, DynBox) = DynBox`).
    // Pre-seed each block-arg's env entry to the meet of its
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
    //       values in `facts`. Without warmup, op results still have
    //       bottom facts, so their non-back-edge values cannot seed a
    //       block argument yet.
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
                if let Some(sources) = spill_sources.get(&results[0]) {
                    let stored = spilled_type(&facts, sources);
                    join_assign_type_fact(&mut facts, results[0], stored);
                    continue;
                }
                let operand_facts: Vec<TirType> = operands
                    .iter()
                    .map(|id| fact_or_bottom(&facts, *id))
                    .collect();
                let inferred_facts = if results.len() == 1 {
                    vec![return_type_hint.clone().unwrap_or_else(|| {
                        infer_result_facts_with_attrs(*opcode, None, &operand_facts, None, 1)
                            .into_iter()
                            .next()
                            .unwrap_or(TirType::Never)
                    })]
                } else {
                    infer_result_facts_with_attrs(
                        *opcode,
                        None,
                        &operand_facts,
                        None,
                        results.len(),
                    )
                };
                for (&result_id, inferred) in results.iter().zip(inferred_facts) {
                    join_assign_type_fact(&mut facts, result_id, inferred);
                }
            }
        }

        // (b) Seed block-args from non-back-edge incoming meets.
        for &block_id in &block_order {
            let Some(edge_list) = incoming_edges.get(&block_id) else {
                continue;
            };
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
                    let is_back_edge = tree.dominates(block_id, *source_bid);
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

    // A changed value can affect an operation's block or the block receiving
    // it as an edge argument. Record both kinds of users; CFG successors alone
    // miss dominating values used several blocks away. Sorted, deduplicated
    // users keep scheduling deterministic without repeatedly queuing a block.
    let mut users: HashMap<ValueId, Vec<BlockId>> = HashMap::default();
    for &block_id in &block_order {
        for (_, operands, _, _) in &ops_by_block[&block_id] {
            for &operand in operands {
                users.entry(operand).or_default().push(block_id);
            }
        }
        if reachable_blocks.contains(&block_id)
            && let Some(edges) = incoming_edges.get(&block_id)
        {
            for (source, arguments) in edges {
                if reachable_blocks.contains(source) {
                    for &argument in arguments {
                        users.entry(argument).or_default().push(block_id);
                    }
                }
            }
        }
    }
    for &block_id in &block_order {
        for (_, _, results, _) in &ops_by_block[&block_id] {
            if let Some(sources) = results.first().and_then(|result| spill_sources.get(result)) {
                for &source in sources {
                    users.entry(source).or_default().push(block_id);
                }
            }
        }
    }
    for blocks in users.values_mut() {
        blocks.sort_unstable_by_key(|block| block.0);
        blocks.dedup();
    }

    // Run to quiescence, not a fixed number of whole-function sweeps. A valid
    // chain of copies through block arguments can require arbitrarily many
    // sweeps, while each fact still makes finite progress under the shared join.
    //
    // The unrestricted recursive TirType domain is not finite height:
    // List(Never), List(List(Never)), ... would keep growing. The existing
    // result inference prevents this: a constructor containing any unresolved
    // nested Never yields root Never instead. Finite authored seed types are
    // the only source of nested bottom; joins combine their existing structural
    // positions, and constructors cannot wrap those holes in new layers.
    // Once a position acquires a bottom-free fact, same-shape joins only widen
    // its finite children; a shape mismatch forms a bounded union and then top.
    // Consequently every value changes finitely often, including recursive
    // tuple, box and iterator cycles. Requeueing only on a changed join reaches
    // the fixed point without a depth cutoff or a different type lattice.
    let mut pending: VecDeque<BlockId> = block_order.iter().copied().collect();
    let mut queued: HashSet<BlockId> = block_order.iter().copied().collect();
    while let Some(block_id) = pending.pop_front() {
        queued.remove(&block_id);
        let mut enqueue_users = |value: ValueId| {
            if let Some(blocks) = users.get(&value) {
                for &user in blocks {
                    if queued.insert(user) {
                        pending.push_back(user);
                    }
                }
            }
        };
        let ops_snapshot = &ops_by_block[&block_id];

        for (opcode, operands, results, return_type_hint) in ops_snapshot {
            if results.is_empty() {
                continue;
            }

            // Do not refine results of CheckException — the value
            // coming out of an exception check is dynamically typed.
            if has_eh && matches!(opcode, OpCode::CheckException) {
                for &result_id in results {
                    if join_assign_type_fact(&mut facts, result_id, TirType::DynBox) {
                        enqueue_users(result_id);
                    }
                }
                continue;
            }

            if let Some(sources) = spill_sources.get(&results[0]) {
                let stored = spilled_type(&facts, sources);
                if join_assign_type_fact(&mut facts, results[0], stored) {
                    enqueue_users(results[0]);
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
                    infer_result_facts_with_attrs(*opcode, None, &operand_facts, None, 1)
                        .into_iter()
                        .next()
                        .unwrap_or(TirType::Never)
                })]
            } else {
                infer_result_facts_with_attrs(*opcode, None, &operand_facts, None, results.len())
            };

            for (&result_id, inferred) in results.iter().zip(inferred_facts) {
                if join_assign_type_fact(&mut facts, result_id, inferred) {
                    enqueue_users(result_id);
                }
            }
        }

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
                    if join_assign_type_fact(&mut facts, arg_id, TirType::DynBox) {
                        enqueue_users(arg_id);
                    }
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
                if join_assign_type_fact(&mut facts, arg_id, accumulated) {
                    enqueue_users(arg_id);
                }
            }
        }
    }

    let mut env: HashMap<ValueId, TirType> = HashMap::default();
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
    func.return_type = infer_return_type(func.blocks.values(), &exact);

    // Count refinements: values that started as DynBox and are now concrete.
    initially_dynbox
        .iter()
        .filter(|id| {
            env.get(id)
                .map(|ty| !matches!(ty, TirType::DynBox))
                .unwrap_or(false)
        })
        .count()
}

/// Each reload of a coroutine frame's spill slot, with the values the frame
/// split stores into that slot. The split owns the spill area: no other
/// operation and no runtime path writes a slot at or past the spill base, and a
/// reload runs only after one of those stores.
fn spill_slot_sources(func: &TirFunction) -> HashMap<ValueId, Vec<ValueId>> {
    use super::ops::AttrValue;
    let mut sources = HashMap::default();
    let Some(AttrValue::Int(base)) = func
        .attrs
        .get(super::lower_from_simple::STATEFUL_SPILL_BASE_ATTR)
    else {
        return sources;
    };
    let Some(frame) = func
        .blocks
        .get(&func.entry_block)
        .and_then(|entry| entry.args.first())
        .map(|arg| arg.id)
    else {
        return sources;
    };
    let spill_offset = |op: &super::ops::TirOp| match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) if offset >= base => Some(*offset),
        _ => None,
    };
    let mut stores: HashMap<i64, Vec<ValueId>> = HashMap::default();
    let mut loads: Vec<(ValueId, i64)> = Vec::new();
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|block| block.0);
    for block in &block_order {
        for op in &func.blocks[block].ops {
            let Some(offset) = spill_offset(op) else {
                continue;
            };
            // A release stores None where the value is dead; no reload
            // reads it, so it does not widen the slot's type.
            if op.opcode == OpCode::ClosureStore
                && !matches!(
                    op.attrs.get("frame_slot_release"),
                    Some(AttrValue::Bool(true))
                )
                && let [owner, value] = op.operands.as_slice()
                && *owner == frame
            {
                stores.entry(offset).or_default().push(*value);
            } else if op.opcode == OpCode::ClosureLoad
                && let ([owner], [result]) = (op.operands.as_slice(), op.results.as_slice())
                && *owner == frame
            {
                loads.push((*result, offset));
            }
        }
    }
    for (result, offset) in loads {
        if let Some(stored) = stores.get(&offset) {
            sources.insert(result, stored.clone());
        }
    }
    sources
}

/// The join of the types stored into one spill slot.
fn spilled_type(facts: &HashMap<ValueId, TirType>, sources: &[ValueId]) -> TirType {
    sources.iter().fold(TirType::Never, |joined, source| {
        joined.meet(&fact_or_bottom(facts, *source))
    })
}
