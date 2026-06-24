use std::collections::{HashMap, HashSet};

use super::blocks::{BlockId, Terminator, TirBlock};
use super::dominators;
use super::function::TirFunction;
use super::op_kinds_generated::{
    TypeRefineAttrResultTypeRule, TypeRefineOperandTypeRule,
    opcode_is_proven_result_type_seed_table, opcode_operand_independent_result_tir_type,
    opcode_type_refine_attr_result_type_rule_table, opcode_type_refine_operand_type_rule_table,
};
use super::ops::{AttrDict, AttrValue, OpCode};
use super::types::TirType;
use super::values::ValueId;

/// Maximum number of fixpoint iterations before a fail-closed nonconvergence
/// diagnostic.
const MAX_ROUNDS: usize = 20;

fn fact_or_bottom(facts: &HashMap<ValueId, TirType>, id: ValueId) -> TirType {
    facts.get(&id).cloned().unwrap_or(TirType::Never)
}

fn is_bottom_type(ty: &TirType) -> bool {
    matches!(ty, TirType::Never)
}

fn contains_bottom_type(ty: &TirType) -> bool {
    match ty {
        TirType::Never => true,
        TirType::List(inner)
        | TirType::Set(inner)
        | TirType::Iterator(inner)
        | TirType::Box(inner)
        | TirType::Ptr(inner) => contains_bottom_type(inner),
        TirType::Dict(key, value) => contains_bottom_type(key) || contains_bottom_type(value),
        TirType::Tuple(items) | TirType::Union(items) => items.iter().any(contains_bottom_type),
        _ => false,
    }
}

fn publish_fact_type(ty: TirType) -> TirType {
    if is_bottom_type(&ty) {
        TirType::DynBox
    } else {
        ty
    }
}

fn is_refined_public_type(ty: &TirType) -> bool {
    !matches!(ty, TirType::DynBox | TirType::Never)
}

fn join_assign_type_fact(
    facts: &mut HashMap<ValueId, TirType>,
    id: ValueId,
    incoming: TirType,
) -> bool {
    let current = fact_or_bottom(facts, id);
    let joined = current.meet(&incoming);
    if joined != current {
        facts.insert(id, joined);
        true
    } else {
        false
    }
}

fn infer_result_facts_with_attrs(
    opcode: OpCode,
    operand_facts: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
    result_count: usize,
) -> Vec<TirType> {
    if result_count == 0 {
        return vec![];
    }

    if matches!(opcode, OpCode::TypeGuard)
        && let Some(attrs) = attrs
        && let Some(proven_ty) = parse_guard_type(attrs)
    {
        return vec![proven_ty; result_count];
    }

    let operands_ready = operand_facts.iter().all(|ty| !is_bottom_type(ty));
    let operand_types: Vec<TirType> = operand_facts.to_vec();
    infer_result_types_with_attrs(opcode, &operand_types, attrs, result_count)
        .into_iter()
        .map(|inferred| match inferred {
            Some(ty) if contains_bottom_type(&ty) => TirType::Never,
            Some(ty) => ty,
            None if operands_ready => TirType::DynBox,
            None => TirType::Never,
        })
        .collect()
}

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

/// Parse the `expected_type` or `ty` attribute of a TypeGuard op into a TirType.
/// Returns `None` for unrecognised type strings.
fn parse_guard_type(attrs: &super::ops::AttrDict) -> Option<TirType> {
    let type_str = attrs
        .get("expected_type")
        .or_else(|| attrs.get("ty"))
        .and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        })?;

    match type_str.to_ascii_lowercase().as_str() {
        "int" | "i64" => Some(TirType::I64),
        "float" | "f64" => Some(TirType::F64),
        "bool" => Some(TirType::Bool),
        "str" | "string" => Some(TirType::Str),
        "none" | "nonetype" => Some(TirType::None),
        "bytes" => Some(TirType::Bytes),
        "list" => Some(TirType::List(Box::new(TirType::DynBox))),
        "dict" => Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox),
        )),
        "set" => Some(TirType::Set(Box::new(TirType::DynBox))),
        "tuple" => Some(TirType::Tuple(vec![])),
        "bigint" => Some(TirType::BigInt),
        _ => None,
    }
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
fn propagate_guard_types(
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

/// Extract a map of values whose types are **proven** (not speculated).
///
/// A type is proven if it comes from:
/// - A constant op (ConstInt, ConstFloat, etc.)
/// - A TypeGuard success path
/// - Arithmetic on proven values
///
/// This map is a subset of the full type_map. The native backend can skip
/// redundant guards for values in this map.
pub fn extract_proven_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    // Start with constants — they are always proven.
    let mut proven: HashMap<ValueId, TirType> = HashMap::new();

    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            if opcode_is_proven_result_type_seed_table(op.opcode)
                && let Some(ty) = opcode_operand_independent_result_tir_type(op.opcode)
            {
                for &r in &op.results {
                    proven.insert(r, ty.clone());
                }
            }
        }
    }

    // Run the guard propagation to add TypeGuard-proven values.
    let mut env = extract_type_map(func);
    let pred_map = dominators::build_pred_map(func);
    let idoms = dominators::compute_idoms(func, &pred_map);
    let (_refinements, guard_proven) = propagate_guard_types(func, &mut env, &idoms);
    for (vid, ty) in guard_proven {
        proven.insert(vid, ty);
    }

    // Propagate through arithmetic chains.
    for &bid in &block_order {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            let all_proven =
                !op.operands.is_empty() && op.operands.iter().all(|v| proven.contains_key(v));
            if !all_proven {
                continue;
            }
            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| proven.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            let result_types = infer_result_types_with_attrs(
                op.opcode,
                &operand_types,
                Some(&op.attrs),
                op.results.len(),
            );
            for (&result_id, result_ty) in op.results.iter().zip(result_types) {
                if let Some(result_ty) = result_ty {
                    proven.insert(result_id, result_ty.clone());
                }
            }
        }
    }

    proven
}

/// Parse a return-type hint string into a `TirType`, returning
/// `None` when the hint carries no useful refinement (so callers
/// fall through to operand-based inference instead of forcing the
/// result to `DynBox`).
///
/// Routes through `TirType::from_type_hint` so the *single* helper
/// in `tir/types.rs` defines the contract for all hint-to-type
/// mappings (builtin scalars, containers, user classes, BigInt).
/// The post-process here translates `from_type_hint`'s `DynBox`
/// fallback into `None` to preserve the "fall through to inference"
/// semantics this function had before the centralization.
///
/// Practical effect of routing through `from_type_hint`:
///   - Methods returning `list` / `dict` / `set` / `tuple` /
///     `BigInt` now refine to the corresponding container/special
///     type (was previously DynBox, leaving lane inference blind).
///   - Methods returning a user class (e.g. `factory() -> Point`)
///     refine the call result to `UserClass("Point")`, propagating
///     the typed-IR foundation through the type-refine fixpoint.
fn parse_return_type_str(name: &str) -> Option<TirType> {
    match TirType::from_type_hint(name) {
        TirType::DynBox => None,
        ty => Some(ty),
    }
}

fn tuple_index_result_type(items: &[TirType]) -> TirType {
    items
        .iter()
        .fold(TirType::Never, |acc, item| acc.meet(item))
}

fn dict_index_key_matches(dict_key_ty: &TirType, index_ty: &TirType) -> bool {
    matches!(dict_key_ty, TirType::DynBox) || dict_key_ty == index_ty
}

fn structural_builtin_return_type(name: &str) -> Option<TirType> {
    match name {
        "len" | "id" | "ord" => Some(TirType::I64),
        "bool" | "hasattr" | "isinstance" | "issubclass" => Some(TirType::Bool),
        "chr" => Some(TirType::Str),
        _ => None,
    }
}

fn attr_result_type_override(opcode: OpCode, attrs: &AttrDict) -> Option<TirType> {
    match opcode_type_refine_attr_result_type_rule_table(opcode) {
        TypeRefineAttrResultTypeRule::None => None,
        TypeRefineAttrResultTypeRule::ObjectTypeHint => match attrs.get("_type_hint") {
            Some(AttrValue::Str(name)) => match TirType::from_type_hint(name) {
                class_ty @ TirType::UserClass(_) => Some(class_ty),
                _ => None,
            },
            _ => None,
        },
        TypeRefineAttrResultTypeRule::CallReturnType => {
            attrs.get("return_type").and_then(|v| match v {
                AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                _ => None,
            })
        }
        TypeRefineAttrResultTypeRule::CallBuiltinReturnType => attrs
            .get("return_type")
            .and_then(|v| match v {
                AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                _ => None,
            })
            .or_else(|| {
                attrs.get("name").and_then(|v| match v {
                    AttrValue::Str(s) => structural_builtin_return_type(s.as_str()),
                    _ => None,
                })
            }),
        TypeRefineAttrResultTypeRule::TypeGuard => parse_guard_type(attrs),
        TypeRefineAttrResultTypeRule::CopyOriginalKind => {
            let original_kind = match attrs.get("_original_kind") {
                Some(AttrValue::Str(k)) => Some(k.as_str()),
                _ => None,
            };
            crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type(original_kind).or_else(
                || {
                    original_kind
                        .filter(|k| {
                            crate::tir::passes::alias_analysis::copy_kind_mints_fresh_owned_ref(k)
                        })
                        .map(fresh_value_kind_result_type)
                },
            )
        }
    }
}

pub(super) fn infer_scalar_return_result_type(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
) -> Option<TirType> {
    infer_result_type_with_attrs(opcode, operand_types, attrs).filter(|ty| {
        matches!(
            ty,
            TirType::I64
                | TirType::F64
                | TirType::Bool
                | TirType::None
                | TirType::Str
                | TirType::Bytes
        )
    })
}

/// Variant of [`infer_result_type`] that consults a structural `return_type`
/// `AttrValue::Str` for opaque call-like opcodes that operand-only inference
/// cannot resolve.
fn infer_result_type_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
) -> Option<TirType> {
    infer_result_types_with_attrs(opcode, operand_types, attrs, 1)
        .into_iter()
        .next()
        .flatten()
}

fn infer_result_types_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
    result_count: usize,
) -> Vec<Option<TirType>> {
    if result_count == 0 {
        return vec![];
    }
    if matches!(opcode, OpCode::IterNextUnboxed) && result_count == 2 {
        let elem_ty = match operand_types {
            [TirType::Iterator(elem_ty)] => Some(elem_ty.as_ref().clone()),
            _ => None,
        };
        return vec![elem_ty, Some(TirType::Bool)];
    }
    // CheckedAdd result types are intrinsic to the opcode: results[0] is the
    // wrapping i64 sum, results[1] the signed-overflow flag. This must hold
    // through the module phase's SimpleIR re-lift — the WASM/LIR lowering
    // derives local types from these, and an untyped flag would fail wasm
    // validation.
    if matches!(opcode, OpCode::CheckedAdd) && result_count == 2 {
        return vec![Some(TirType::I64), Some(TirType::Bool)];
    }
    if result_count != 1 {
        return vec![None; result_count];
    }
    vec![infer_single_result_type_with_attrs(
        opcode,
        operand_types,
        attrs,
    )]
}

fn infer_single_result_type_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
) -> Option<TirType> {
    if let Some(attrs) = attrs
        && let Some(ty) = attr_result_type_override(opcode, attrs)
    {
        return Some(ty);
    }
    if let Some(ty) = opcode_operand_independent_result_tir_type(opcode) {
        return Some(ty);
    }
    match opcode_type_refine_operand_type_rule_table(opcode) {
        // Add: numeric arithmetic + string concatenation + string/list repetition
        TypeRefineOperandTypeRule::Add => match operand_types {
            [TirType::Str, TirType::Str] => Some(TirType::Str), // "a" + "b"
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Mul: numeric arithmetic + string/list repetition (str * int, int * str)
        TypeRefineOperandTypeRule::Mul => match operand_types {
            [TirType::Str, TirType::I64] | [TirType::I64, TirType::Str] => Some(TirType::Str),
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Sub, Mod, Pow: numeric only (str-str is TypeError in Python).
        // InplaceSub mirrors Sub for typed scalars; mutable-type sequence
        // ops (list -= ...) are TypeError in CPython for these opcodes.
        TypeRefineOperandTypeRule::NumericArithmetic => infer_numeric_arithmetic(operand_types),
        TypeRefineOperandTypeRule::TrueDivision => {
            // Python: division always produces float unless both are DynBox.
            match operand_types {
                [TirType::I64, TirType::I64]
                | [TirType::F64, TirType::F64]
                | [TirType::I64, TirType::F64]
                | [TirType::F64, TirType::I64] => Some(TirType::F64),
                _ => infer_numeric_arithmetic(operand_types),
            }
        }
        // Unary Neg/Pos
        TypeRefineOperandTypeRule::UnaryNumeric => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            [TirType::F64] => Some(TirType::F64),
            _ => None,
        },

        // Boolean value-select ops remain operand-dependent: the opcode itself
        // is not enough unless both operands are Bool.
        TypeRefineOperandTypeRule::BoolSelect => match operand_types {
            [TirType::Bool, TirType::Bool] => Some(TirType::Bool),
            _ => None,
        },

        // Bitwise ops other than shifts are closed over the inline I64 lane.
        // Shifts can promote beyond the inline range and must stay boxed until
        // the runtime operator decides whether bigint promotion is required.
        TypeRefineOperandTypeRule::BitwiseI64 => match operand_types {
            [TirType::I64, TirType::I64] => Some(TirType::I64),
            _ => None,
        },
        TypeRefineOperandTypeRule::BitNotI64 => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            _ => None,
        },

        // Containers with operand-dependent element shape stay here.
        TypeRefineOperandTypeRule::BuildTuple => Some(TirType::Tuple(operand_types.to_vec())),
        TypeRefineOperandTypeRule::GetIter => match operand_types {
            [TirType::List(elem_ty) | TirType::Set(elem_ty)] => {
                Some(TirType::Iterator(Box::new(elem_ty.as_ref().clone())))
            }
            [TirType::Tuple(items)] if !items.is_empty() => {
                Some(TirType::Iterator(Box::new(tuple_index_result_type(items))))
            }
            [TirType::Dict(key_ty, _)] => {
                Some(TirType::Iterator(Box::new(key_ty.as_ref().clone())))
            }
            [TirType::Str] => Some(TirType::Iterator(Box::new(TirType::Str))),
            [TirType::Bytes] => Some(TirType::Iterator(Box::new(TirType::I64))),
            _ => None,
        },
        TypeRefineOperandTypeRule::IterNext => match operand_types {
            [TirType::Iterator(elem_ty)] => Some(elem_ty.as_ref().clone()),
            _ => None,
        },
        TypeRefineOperandTypeRule::Index => match operand_types {
            [TirType::Str, TirType::I64 | TirType::Bool] => Some(TirType::Str),
            [TirType::Bytes, TirType::I64 | TirType::Bool] => Some(TirType::I64),
            [TirType::List(elem_ty), TirType::I64 | TirType::Bool] => {
                Some(elem_ty.as_ref().clone())
            }
            [TirType::Tuple(items), TirType::I64 | TirType::Bool] if !items.is_empty() => {
                Some(tuple_index_result_type(items))
            }
            [TirType::Dict(key_ty, value_ty), index_ty]
                if dict_index_key_matches(key_ty.as_ref(), index_ty) =>
            {
                Some(value_ty.as_ref().clone())
            }
            _ => None,
        },
        // Fresh-value and raw-carrier Copy spellings are handled by the attr
        // rule before this point. The operand rule means transparent aliasing.
        TypeRefineOperandTypeRule::Copy => operand_types.first().cloned(),

        // Box/Unbox
        TypeRefineOperandTypeRule::BoxVal => operand_types
            .first()
            .map(|t| TirType::Box(Box::new(t.clone()))),
        TypeRefineOperandTypeRule::UnboxVal => match operand_types.first() {
            Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
            _ => None,
        },

        TypeRefineOperandTypeRule::None => None,
    }
}

/// Result type of a fresh-value-minting op (one that falls back to
/// `OpCode::Copy` carrying its kind in `_original_kind` but, per
/// [`crate::tir::passes::alias_analysis::copy_kind_mints_fresh_owned_ref`],
/// constructs a NEW owned object rather than aliasing operand[0]).
///
/// The result type is intrinsic to the op, NOT operand[0]'s type. The vast
/// majority mint heap objects the TIR does not model further (`complex`, dicts,
/// lists, sets, tuples, ranges, slices, iterators, generic instances) → DynBox.
/// A handful mint a statically-known scalar/str result and are typed precisely
/// so the scalar lanes still fire on them. `int()`/`int_from_*` are intentionally
/// DynBox (may return a heap BigInt; an I64 type would license a trusted-unbox on
/// a BigInt pointer — the same carrier-soundness rule `ConstBigInt` follows).
fn fresh_value_kind_result_type(kind: &str) -> TirType {
    match kind {
        "float_from_obj" => TirType::F64,
        "str_from_obj" | "repr_from_obj" | "ascii_from_obj" | "string_format" | "string_join" => {
            TirType::Str
        }
        _ => TirType::DynBox,
    }
}

/// Infer the result type of a numeric-only binary operation.
/// Does NOT handle string concatenation or repetition — those are handled
/// at the opcode level (Add for concat, Mul for repetition).
fn infer_numeric_arithmetic(operand_types: &[TirType]) -> Option<TirType> {
    match operand_types {
        [TirType::I64, TirType::I64] => Some(TirType::I64),
        [TirType::F64, TirType::F64] => Some(TirType::F64),
        // Python numeric promotion: int op float → float
        [TirType::I64, TirType::F64] | [TirType::F64, TirType::I64] => Some(TirType::F64),
        _ => None,
    }
}

/// Collect (target_block, arg_values) edges from a terminator.
fn collect_branch_edges(block: &TirBlock) -> Vec<(BlockId, Vec<ValueId>)> {
    match &block.terminator {
        Terminator::Branch { target, args } => {
            vec![(*target, args.clone())]
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            vec![
                (*then_block, then_args.clone()),
                (*else_block, else_args.clone()),
            ]
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
            ..
        } => {
            let mut edges: Vec<(BlockId, Vec<ValueId>)> = cases
                .iter()
                .map(|(_, target, args)| (*target, args.clone()))
                .collect();
            edges.push((*default, default_args.clone()));
            edges
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};
    use std::f64::consts::PI;

    /// Helper: build a simple function with one block containing the given ops.
    fn single_block_func(ops: Vec<TirOp>, next_value: u32) -> TirFunction {
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![],
            ops,
            terminator: Terminator::Return { values: vec![] },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        TirFunction {
            name: "test".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        }
    }

    fn make_op(
        opcode: OpCode,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
        attrs: AttrDict,
    ) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    fn int_attr(val: i64) -> AttrDict {
        let mut m = AttrDict::new();
        m.insert("value".into(), AttrValue::Int(val));
        m
    }

    fn float_attr(val: f64) -> AttrDict {
        let mut m = AttrDict::new();
        m.insert("value".into(), AttrValue::Float(val));
        m
    }

    fn str_attr(val: &str) -> AttrDict {
        let mut m = AttrDict::new();
        m.insert("value".into(), AttrValue::Str(val.into()));
        m
    }

    // ---- Test 1: Constants resolve to concrete types ----
    #[test]
    fn constants_resolve_to_concrete_types() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(42)),
            make_op(OpCode::ConstFloat, vec![], vec![ValueId(1)], float_attr(PI)),
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(2)],
                str_attr("hello"),
            ),
            make_op(OpCode::ConstBool, vec![], vec![ValueId(3)], AttrDict::new()),
            make_op(OpCode::ConstNone, vec![], vec![ValueId(4)], AttrDict::new()),
            make_op(
                OpCode::ConstBytes,
                vec![],
                vec![ValueId(5)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 6);
        let refined = refine_types(&mut func);
        // All 6 values should be refined from DynBox to concrete types.
        assert_eq!(refined, 6);
    }

    // ---- Test 2: Arithmetic propagates types ----
    #[test]
    fn arithmetic_propagates_i64() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 3); // two consts + one add result
    }

    #[test]
    fn module_get_attr_result_stays_dynbox() {
        let ops = vec![
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(0)],
                str_attr("module_name"),
            ),
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(1)],
                str_attr("Point"),
            ),
            make_op(
                OpCode::ModuleGetAttr,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        let refined = refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(refined, 2, "only the const_str operands refine to Str");
        assert_eq!(
            type_map.get(&ValueId(2)),
            Some(&TirType::DynBox),
            "module_get_attr result must not inherit the module operand type"
        );
    }

    #[test]
    fn exception_label_forwarding_args_widen_downstream_merge_args() {
        let normal_value = ValueId(0);
        let handler_arg = ValueId(1);
        let merge_arg = ValueId(2);
        let entry = BlockId(0);
        let handler = BlockId(1);
        let merge = BlockId(2);

        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![],
                ops: vec![
                    make_op(OpCode::TryStart, vec![], vec![], int_attr(10)),
                    make_op(OpCode::ConstInt, vec![], vec![normal_value], int_attr(1)),
                ],
                terminator: Terminator::Branch {
                    target: merge,
                    args: vec![normal_value],
                },
            },
        );
        blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: merge,
                    args: vec![handler_arg],
                },
            },
        );
        blocks.insert(
            merge,
            TirBlock {
                id: merge,
                args: vec![TirValue {
                    id: merge_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg],
                },
            },
        );
        let mut label_id_map = HashMap::new();
        label_id_map.insert(handler.0, 10);
        let mut func = TirFunction {
            name: "exception_label_forwarding_args_widen_downstream_merge_args".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::DynBox,
            blocks,
            entry_block: entry,
            next_value: 3,
            next_block: 3,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: true,
            label_id_map,
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&handler_arg), Some(&TirType::DynBox));
        assert_eq!(
            type_map.get(&merge_arg),
            Some(&TirType::DynBox),
            "merge block args must widen when an exception-label forwarding block can feed DynBox"
        );
        assert_eq!(
            func.blocks[&merge].args[0].ty,
            TirType::DynBox,
            "refine_types must write the widened merge type back into block args"
        );
    }

    #[test]
    fn module_lookup_results_stay_dynbox() {
        for opcode in [
            OpCode::ModuleCacheGet,
            OpCode::ModuleGetGlobal,
            OpCode::ModuleGetName,
        ] {
            let operands = if opcode == OpCode::ModuleCacheGet {
                vec![ValueId(0)]
            } else {
                vec![ValueId(0), ValueId(1)]
            };
            let ops = vec![
                make_op(
                    OpCode::ConstStr,
                    vec![],
                    vec![ValueId(0)],
                    str_attr("module_name"),
                ),
                make_op(
                    OpCode::ConstStr,
                    vec![],
                    vec![ValueId(1)],
                    str_attr("answer"),
                ),
                make_op(opcode, operands, vec![ValueId(2)], AttrDict::new()),
            ];
            let mut func = single_block_func(ops, 3);
            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&ValueId(2)),
                Some(&TirType::DynBox),
                "{opcode:?} result must not inherit the module/name operand type"
            );
        }
    }

    // ---- Test 3: Mixed arithmetic promotes to F64 ----
    #[test]
    fn mixed_arithmetic_promotes_to_f64() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(1)],
                float_attr(2.0),
            ),
            make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 3);
    }

    // ---- Test 4: Comparison produces Bool ----
    #[test]
    fn comparison_produces_bool() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::Eq,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 3);
    }

    // ---- Test 5: Block argument meet ----
    #[test]
    fn block_arg_meet_same_types() {
        // Two predecessor blocks both pass I64 to a join block's arg.
        let entry_id = BlockId(0);
        let then_id = BlockId(1);
        let else_id = BlockId(2);
        let join_id = BlockId(3);

        let mut blocks = HashMap::new();

        // Entry: cond branch to then/else
        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![TirValue {
                    id: ValueId(0),
                    ty: TirType::Bool,
                }],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: then_id,
                    then_args: vec![],
                    else_block: else_id,
                    else_args: vec![],
                },
            },
        );

        // Then: const int, branch to join
        blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::ConstInt,
                    vec![],
                    vec![ValueId(1)],
                    int_attr(10),
                )],
                terminator: Terminator::Branch {
                    target: join_id,
                    args: vec![ValueId(1)],
                },
            },
        );

        // Else: const int, branch to join
        blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::ConstInt,
                    vec![],
                    vec![ValueId(2)],
                    int_attr(20),
                )],
                terminator: Terminator::Branch {
                    target: join_id,
                    args: vec![ValueId(2)],
                },
            },
        );

        // Join: one block arg (starts as DynBox), return
        blocks.insert(
            join_id,
            TirBlock {
                id: join_id,
                args: vec![TirValue {
                    id: ValueId(3),
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![ValueId(3)],
                },
            },
        );

        let mut func = TirFunction {
            name: "join_test".into(),
            param_names: vec!["p0".into()],
            param_types: vec![TirType::Bool],
            return_type: TirType::I64,
            blocks,
            entry_block: entry_id,
            next_value: 4,
            next_block: 4,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let refined = refine_types(&mut func);

        // ValueId(1), ValueId(2) (const ints) and ValueId(3) (block arg) should
        // all be refined. ValueId(3) should be meet(I64, I64) = I64.
        assert!(refined >= 3);
        assert_eq!(func.blocks[&join_id].args[0].ty, TirType::I64);
    }

    #[test]
    fn block_arg_meet_different_types_produces_union() {
        // One branch passes I64, another passes F64 → Union(I64, F64).
        let entry_id = BlockId(0);
        let then_id = BlockId(1);
        let else_id = BlockId(2);
        let join_id = BlockId(3);

        let mut blocks = HashMap::new();

        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![TirValue {
                    id: ValueId(0),
                    ty: TirType::Bool,
                }],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: then_id,
                    then_args: vec![],
                    else_block: else_id,
                    else_args: vec![],
                },
            },
        );

        blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::ConstInt,
                    vec![],
                    vec![ValueId(1)],
                    int_attr(10),
                )],
                terminator: Terminator::Branch {
                    target: join_id,
                    args: vec![ValueId(1)],
                },
            },
        );

        blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::ConstFloat,
                    vec![],
                    vec![ValueId(2)],
                    float_attr(PI),
                )],
                terminator: Terminator::Branch {
                    target: join_id,
                    args: vec![ValueId(2)],
                },
            },
        );

        blocks.insert(
            join_id,
            TirBlock {
                id: join_id,
                args: vec![TirValue {
                    id: ValueId(3),
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![ValueId(3)],
                },
            },
        );

        let mut func = TirFunction {
            name: "union_test".into(),
            param_names: vec!["p0".into()],
            param_types: vec![TirType::Bool],
            return_type: TirType::DynBox,
            blocks,
            entry_block: entry_id,
            next_value: 4,
            next_block: 4,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let refined = refine_types(&mut func);
        assert!(refined >= 3);

        let join_arg_ty = &func.blocks[&join_id].args[0].ty;
        // Union member order depends on HashMap iteration order; accept either.
        assert!(
            *join_arg_ty == TirType::Union(vec![TirType::I64, TirType::F64])
                || *join_arg_ty == TirType::Union(vec![TirType::F64, TirType::I64]),
            "expected Union(I64, F64) in any order, got {:?}",
            join_arg_ty
        );
    }

    // ---- Test 6: Fixpoint convergence ----
    #[test]
    fn fixpoint_converges() {
        // Chain: ConstInt → Add → Add — all should resolve in ≤2 rounds.
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::Add,
                vec![ValueId(2), ValueId(0)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 4);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 4);
    }

    // ---- Test 7: DynBox stays DynBox when operands are unknown ----
    #[test]
    fn dynbox_stays_dynbox_for_unknown_operands() {
        // Add(DynBox, DynBox) → DynBox (no refinement possible)
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![
                TirValue {
                    id: ValueId(0),
                    ty: TirType::DynBox,
                },
                TirValue {
                    id: ValueId(1),
                    ty: TirType::DynBox,
                },
            ],
            ops: vec![make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            )],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        let mut func = TirFunction {
            name: "dynbox_test".into(),
            param_names: vec!["p0".into(), "p1".into()],
            param_types: vec![TirType::DynBox, TirType::DynBox],
            return_type: TirType::DynBox,
            blocks,
            entry_block: entry_id,
            next_value: 3,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };
        let refined = refine_types(&mut func);
        assert_eq!(refined, 0);
    }

    #[test]
    fn dynamic_transfer_widens_stale_precise_result_and_stays_idempotent() {
        let source = ValueId(0);
        let result = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
        let mut func = single_block_func(
            vec![make_op(OpCode::Copy, vec![source], vec![result], attrs)],
            2,
        );
        func.value_types.insert(source, TirType::DynBox);
        func.value_types.insert(result, TirType::Str);

        refine_types(&mut func);
        let once = extract_type_map(&func);
        assert_eq!(
            once.get(&result),
            Some(&TirType::DynBox),
            "a dynamic producer must widen stale precise facts to top"
        );

        refine_types(&mut func);
        let twice = extract_type_map(&func);
        assert_eq!(
            twice.get(&result),
            Some(&TirType::DynBox),
            "post-refinement extraction must not re-narrow widened top facts"
        );
    }

    #[test]
    fn never_bottom_waits_for_late_dominator_and_drops_stale_result_fact() {
        let body_id = BlockId(0);
        let entry_id = BlockId(1);
        let source = ValueId(0);
        let alias = ValueId(1);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::Copy,
                vec![source],
                vec![alias],
                AttrDict::new(),
            )],
            terminator: Terminator::Return {
                values: vec![alias],
            },
        };
        let entry = TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![source],
                int_attr(41),
            )],
            terminator: Terminator::Branch {
                target: body_id,
                args: vec![],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(body_id, body);
        blocks.insert(entry_id, entry);

        let mut func = TirFunction {
            name: "never_bottom_late_dominator".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::I64,
            blocks,
            entry_block: entry_id,
            next_value: 2,
            next_block: 2,
            attrs: AttrDict::new(),
            value_types: HashMap::from([(alias, TirType::Str)]),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        refine_types(&mut func);
        let once = extract_type_map(&func);
        assert_eq!(
            once.get(&alias),
            Some(&TirType::I64),
            "bottom operands must wait for the dominating producer instead of \
             publishing the stale result fact or cementing DynBox"
        );
        assert_eq!(
            func.value_types.get(&alias),
            Some(&TirType::I64),
            "refine_types must publish the converged fact, not the stale input fact"
        );

        refine_types(&mut func);
        assert_eq!(
            extract_type_map(&func),
            once,
            "Never-bottom convergence must be idempotent after publication"
        );
    }

    #[test]
    fn dense_check_exception_results_are_top_and_idempotent() {
        let ops: Vec<TirOp> = (0..1024)
            .map(|idx| {
                make_op(
                    OpCode::CheckException,
                    vec![],
                    vec![ValueId(idx)],
                    AttrDict::new(),
                )
            })
            .collect();
        let mut func = single_block_func(ops, 1024);
        func.name = "dense_exception_poll".into();
        func.has_exception_handling = true;

        refine_types(&mut func);
        let value_types_after_first = func.value_types.clone();
        assert_eq!(value_types_after_first.len(), 1024);
        assert!(
            value_types_after_first
                .values()
                .all(|ty| matches!(ty, TirType::DynBox)),
            "check_exception result facts are dynamic top"
        );

        refine_types(&mut func);
        assert_eq!(
            func.value_types, value_types_after_first,
            "dense exception refinement must be idempotent"
        );
    }

    /// Lock-in for the loop-induction-variable seeding contract.
    ///
    /// CFG:
    /// ```text
    /// entry:  i_init = ConstInt(0); branch header(i_init)
    /// header(i: ?):  cond = ConstBool(true); cond_branch body, exit
    /// body:  one = ConstInt(1); i_next = Add(i, one); branch header(i_next)
    /// exit:  return
    /// ```
    /// Without IV seeding, `i` ends up DynBox: the body sees `i: DynBox`
    /// initially, infers `Add(DynBox, I64)` as no-type, the back-edge
    /// brings DynBox, and `meet(I64, DynBox) = DynBox` widens the entry.
    /// With IV seeding, `i` is initialized to I64 (the entry-edge type
    /// alone, since the back-edge is excluded from the seed), the body
    /// then infers `Add(I64, I64) = I64`, the back-edge confirms I64,
    /// and the fixpoint converges to I64.
    #[test]
    fn loop_iv_block_arg_seeded_to_entry_type() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let body_id = BlockId(2);
        let exit_id = BlockId(3);

        let i_init = ValueId(0);
        let i = ValueId(1);
        let cond = ValueId(2);
        let one = ValueId(3);
        let i_next = ValueId(4);

        let entry = TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_init],
            },
        };
        let header = TirBlock {
            id: header_id,
            args: vec![TirValue {
                id: i,
                ty: TirType::DynBox, // intentionally pessimistic — the seeding fix narrows it
            }],
            ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
                let mut a = AttrDict::new();
                a.insert("value".into(), AttrValue::Bool(true));
                a
            })],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body_id,
                then_args: vec![],
                else_block: exit_id,
                else_args: vec![],
            },
        };
        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
                make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
            ],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_next],
            },
        };
        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };

        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);
        blocks.insert(header_id, header);
        blocks.insert(body_id, body);
        blocks.insert(exit_id, exit);

        let mut func = TirFunction {
            name: "iv_loop".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 5,
            next_block: 4,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let _refined = refine_types(&mut func);

        let header_block = &func.blocks[&header_id];
        let i_arg_ty = header_block
            .args
            .iter()
            .find(|a| a.id == i)
            .map(|a| a.ty.clone())
            .expect("loop header arg `i` present");
        assert_eq!(
            i_arg_ty,
            TirType::I64,
            "loop induction variable seeded with entry-edge I64 must converge to I64, got {:?}",
            i_arg_ty
        );
    }

    #[test]
    fn loop_iv_seed_widens_to_dynbox_when_backedge_is_dynamic() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let body_id = BlockId(2);
        let exit_id = BlockId(3);

        let i_init = ValueId(0);
        let i = ValueId(1);
        let cond = ValueId(2);
        let i_next = ValueId(3);

        let entry = TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_init],
            },
        };
        let header = TirBlock {
            id: header_id,
            args: vec![TirValue {
                id: i,
                ty: TirType::DynBox,
            }],
            ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
                let mut a = AttrDict::new();
                a.insert("value".into(), AttrValue::Bool(true));
                a
            })],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body_id,
                then_args: vec![],
                else_block: exit_id,
                else_args: vec![],
            },
        };
        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::Call,
                vec![i],
                vec![i_next],
                AttrDict::new(),
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_next],
            },
        };
        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };

        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);
        blocks.insert(header_id, header);
        blocks.insert(body_id, body);
        blocks.insert(exit_id, exit);

        let mut func = TirFunction {
            name: "iv_loop_dynamic_backedge".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 4,
            next_block: 4,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        refine_types(&mut func);
        let first = func.value_types.clone();
        assert_eq!(
            func.blocks[&header_id].args[0].ty,
            TirType::DynBox,
            "entry-edge I64 seed must widen when the reachable back-edge is dynamic"
        );
        assert_eq!(
            first.get(&i_next),
            Some(&TirType::DynBox),
            "dynamic back-edge producer must publish top, not stay at bottom"
        );

        refine_types(&mut func);
        assert_eq!(
            func.value_types, first,
            "loop-carried dynamic widening must be stable, not an oscillation fallback"
        );
    }

    #[test]
    fn unreachable_loop_end_edge_does_not_widen_reachable_loop_arg() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let body_id = BlockId(2);
        let exit_id = BlockId(3);
        let dead_loop_end_id = BlockId(4);

        let i_init = ValueId(0);
        let i = ValueId(1);
        let cond = ValueId(2);
        let one = ValueId(3);
        let i_next = ValueId(4);
        let dead_none = ValueId(5);

        let entry = TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_init],
            },
        };
        let header = TirBlock {
            id: header_id,
            args: vec![TirValue {
                id: i,
                ty: TirType::DynBox,
            }],
            ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
                let mut a = AttrDict::new();
                a.insert("value".into(), AttrValue::Bool(true));
                a
            })],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body_id,
                then_args: vec![],
                else_block: exit_id,
                else_args: vec![],
            },
        };
        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
                make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
            ],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![i_next],
            },
        };
        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        let dead_loop_end = TirBlock {
            id: dead_loop_end_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstNone,
                vec![],
                vec![dead_none],
                AttrDict::new(),
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![dead_none],
            },
        };

        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);
        blocks.insert(header_id, header);
        blocks.insert(body_id, body);
        blocks.insert(exit_id, exit);
        blocks.insert(dead_loop_end_id, dead_loop_end);

        let mut func = TirFunction {
            name: "unreachable_loop_end_meet".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 6,
            next_block: 5,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::from([(dead_loop_end_id, LoopRole::LoopEnd)]),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        refine_types(&mut func);

        let header_block = &func.blocks[&header_id];
        let i_arg_ty = header_block
            .args
            .iter()
            .find(|a| a.id == i)
            .map(|a| a.ty.clone())
            .expect("loop header arg `i` present");
        assert_eq!(
            i_arg_ty,
            TirType::I64,
            "unreachable loop-end incoming values must not widen reachable loop-carried types"
        );
    }

    /// Locks in the contract that `InplaceAdd`/`InplaceSub`/`InplaceMul`
    /// participate in numeric arithmetic inference identically to their
    /// regular `Add`/`Sub`/`Mul` counterparts. Without this, an
    /// accumulator pattern like `total += i` (lowered as `InplaceAdd`)
    /// stays at DynBox even when both operands are I64, causing the
    /// native backend to coerce to a float lane and silently miscompile
    /// the integer accumulator (printed bits look like a denormal float).
    #[test]
    fn inplace_add_typed_to_i64_for_int_operands() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(10)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(20)),
            make_op(
                OpCode::InplaceAdd,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::InplaceSub,
                vec![ValueId(2), ValueId(1)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::InplaceMul,
                vec![ValueId(3), ValueId(0)],
                vec![ValueId(4)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 5);
        let _refined = refine_types(&mut func);

        // Re-extract the type map post-refinement to inspect op result
        // types (block args were already covered by other tests).
        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(2)),
            Some(&TirType::I64),
            "InplaceAdd of (I64, I64) must produce I64"
        );
        assert_eq!(
            env.get(&ValueId(3)),
            Some(&TirType::I64),
            "InplaceSub of (I64, I64) must produce I64"
        );
        assert_eq!(
            env.get(&ValueId(4)),
            Some(&TirType::I64),
            "InplaceMul of (I64, I64) must produce I64"
        );
    }

    /// Round-8 regression: a FRESH-VALUE scalar-conversion `Copy`
    /// (`int_from_obj`/`float_from_obj`) is NOT a transparent type alias of its
    /// operand — it mints a NEW raw-register value whose type the conversion
    /// determines. `int(t)` with `t: float` lowers to `Copy[int_from_obj](t)`; the
    /// old `Copy => operand_types.first()` rule type-aliased it to `t`'s `F64`,
    /// flooding the downstream integer accumulator (`total += int(t)`) with a
    /// spurious float carrier → native `def_var` repr mismatch / LIR-verifier
    /// branch-repr divergence (`os._seconds_float_to_sec_nsec`). A TRANSPARENT
    /// alias (`copy_var`/bare `Copy`) MUST still propagate the operand type.
    #[test]
    fn int_from_obj_copy_of_float_is_i64_not_aliased_to_operand() {
        let int_from_obj_attr = {
            let mut a = AttrDict::new();
            a.insert(
                "_original_kind".into(),
                AttrValue::Str("int_from_obj".into()),
            );
            a
        };
        let copy_var_attr = {
            let mut a = AttrDict::new();
            a.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
            a
        };
        let ops = vec![
            // t = <float> (a const float stands in for the float parameter).
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(0)],
                AttrDict::new(),
            ),
            // sec = int(t)  →  Copy[int_from_obj](t). MUST type to I64, not F64.
            make_op(
                OpCode::Copy,
                vec![ValueId(0)],
                vec![ValueId(1)],
                int_from_obj_attr,
            ),
            // total = 0; total += sec  →  the integer accumulator that mis-typed.
            make_op(OpCode::ConstInt, vec![], vec![ValueId(2)], int_attr(0)),
            make_op(
                OpCode::InplaceAdd,
                vec![ValueId(2), ValueId(1)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
            // A TRANSPARENT alias of the float MUST keep the operand's F64 type.
            make_op(
                OpCode::Copy,
                vec![ValueId(0)],
                vec![ValueId(4)],
                copy_var_attr,
            ),
        ];
        let mut func = single_block_func(ops, 5);
        refine_types(&mut func);
        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(1)),
            Some(&TirType::I64),
            "Copy[int_from_obj](F64) must produce I64 (a fresh int), NOT alias the float operand"
        );
        assert_eq!(
            env.get(&ValueId(3)),
            Some(&TirType::I64),
            "InplaceAdd(I64 accumulator, int(t)) must stay I64 — the accumulator must not float-contaminate"
        );
        assert_eq!(
            env.get(&ValueId(4)),
            Some(&TirType::F64),
            "a TRANSPARENT-alias Copy (copy_var) must still propagate operand 0's F64 type"
        );
    }

    /// The `copy_kind_raw_carrier_type` source of truth: raw-carrier scalar
    /// conversions map to their precise scalar; every other `Copy` kind (including
    /// heap-producing fresh values and transparent aliases) returns `None` so the
    /// caller keeps operand-0 propagation. Pins the narrow scope that keeps the
    /// heap-value type lattice byte-identical to the pre-fix behavior.
    #[test]
    fn raw_carrier_type_is_scoped_to_scalar_conversions() {
        use crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type;
        assert_eq!(
            copy_kind_raw_carrier_type(Some("int_from_obj")),
            Some(TirType::I64)
        );
        assert_eq!(
            copy_kind_raw_carrier_type(Some("int_from_str_of_obj")),
            Some(TirType::I64)
        );
        assert_eq!(
            copy_kind_raw_carrier_type(Some("float_from_obj")),
            Some(TirType::F64)
        );
        assert_eq!(
            copy_kind_raw_carrier_type(Some("contains")),
            Some(TirType::Bool)
        );
        // Heap-producing fresh values → None (operand-0 propagation / DynBox floor).
        assert_eq!(copy_kind_raw_carrier_type(Some("str_from_obj")), None);
        assert_eq!(copy_kind_raw_carrier_type(Some("list_new")), None);
        assert_eq!(copy_kind_raw_carrier_type(Some("tuple_new")), None);
        assert_eq!(copy_kind_raw_carrier_type(Some("enumerate")), None);
        // Transparent aliases / bare Copy / unknown → None.
        assert_eq!(copy_kind_raw_carrier_type(Some("copy_var")), None);
        assert_eq!(copy_kind_raw_carrier_type(Some("guard_tag")), None);
        assert_eq!(copy_kind_raw_carrier_type(None), None);
    }

    // ---- Guard propagation tests ----

    fn make_type_guard_op(operand: ValueId, result: ValueId, expected_type: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("expected_type".into(), AttrValue::Str(expected_type.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TypeGuard,
            operands: vec![operand],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    // ---- Test: TypeGuard result gets proven type ----
    #[test]
    fn typeguard_result_gets_proven_type() {
        // TypeGuard(%x, "int") -> %ok should type %ok as I64.
        let ops = vec![make_type_guard_op(ValueId(0), ValueId(1), "int")];
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            }],
            ops,
            terminator: Terminator::Return {
                values: vec![ValueId(1)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        let mut func = TirFunction {
            name: "guard_test".into(),
            param_names: vec!["x".into()],
            param_types: vec![TirType::DynBox],
            return_type: TirType::DynBox,
            blocks,
            entry_block: entry_id,
            next_value: 2,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let refined = refine_types(&mut func);
        assert!(refined >= 1, "TypeGuard should refine at least the result");

        let type_map = extract_type_map(&func);
        assert_eq!(
            type_map.get(&ValueId(1)),
            Some(&TirType::I64),
            "TypeGuard result should be I64"
        );
    }

    // ---- Test: Guard propagates to dominated blocks via CondBranch ----
    #[test]
    fn guard_propagates_to_dominated_blocks() {
        // bb0: %x = param(DynBox); %ok = TypeGuard(%x, "int"); CondBranch(%ok, bb1, bb2)
        // bb1 (success): Add(%x, %x) -> should know %x is I64
        // bb2 (fail): return
        let entry_id = BlockId(0);
        let success_id = BlockId(1);
        let fail_id = BlockId(2);

        let mut blocks = HashMap::new();

        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![TirValue {
                    id: ValueId(0),
                    ty: TirType::DynBox,
                }],
                ops: vec![make_type_guard_op(ValueId(0), ValueId(1), "int")],
                terminator: Terminator::CondBranch {
                    cond: ValueId(1),
                    then_block: success_id,
                    then_args: vec![],
                    else_block: fail_id,
                    else_args: vec![],
                },
            },
        );

        blocks.insert(
            success_id,
            TirBlock {
                id: success_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::Add,
                    vec![ValueId(0), ValueId(0)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                )],
                terminator: Terminator::Return {
                    values: vec![ValueId(2)],
                },
            },
        );

        blocks.insert(
            fail_id,
            TirBlock {
                id: fail_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut func = TirFunction {
            name: "guard_prop_test".into(),
            param_names: vec!["x".into()],
            param_types: vec![TirType::DynBox],
            return_type: TirType::DynBox,
            blocks,
            entry_block: entry_id,
            next_value: 3,
            next_block: 3,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        refine_types(&mut func);
        let proven = extract_proven_map(&func);

        // The TypeGuard result should be proven I64.
        assert_eq!(
            proven.get(&ValueId(1)),
            Some(&TirType::I64),
            "TypeGuard result should be proven I64"
        );

        // The guarded value should be proven I64 (used in dominated block).
        assert_eq!(
            proven.get(&ValueId(0)),
            Some(&TirType::I64),
            "Guarded value should be proven I64 in dominated blocks"
        );
    }

    // ---- Test: Constants are always proven ----
    #[test]
    fn constants_are_proven() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(42)),
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(1)],
                float_attr(2.5),
            ),
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(2)],
                str_attr("hello"),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        refine_types(&mut func);
        let proven = extract_proven_map(&func);

        assert_eq!(proven.get(&ValueId(0)), Some(&TirType::I64));
        assert_eq!(proven.get(&ValueId(1)), Some(&TirType::F64));
        assert_eq!(proven.get(&ValueId(2)), Some(&TirType::Str));
    }

    // ---- Test: Arithmetic on proven values is proven ----
    #[test]
    fn arithmetic_on_proven_is_proven() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        refine_types(&mut func);
        let proven = extract_proven_map(&func);

        assert_eq!(proven.get(&ValueId(2)), Some(&TirType::I64));
    }

    #[test]
    fn list_index_refines_to_element_type() {
        let list = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![list, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types
            .insert(list, TirType::List(Box::new(TirType::Bool)));
        func.value_types.insert(index, TirType::I64);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&item), Some(&TirType::Bool));
        assert_eq!(
            func.value_types.get(&item),
            Some(&TirType::Bool),
            "refine_types must persist list element facts for backend plans"
        );
    }

    #[test]
    fn list_index_with_non_integer_index_stays_dynbox() {
        let list = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![list, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types
            .insert(list, TirType::List(Box::new(TirType::Bool)));
        func.value_types.insert(index, TirType::Str);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
    }

    #[test]
    fn str_index_refines_to_str_for_integer_indices() {
        for index_ty in [TirType::I64, TirType::Bool] {
            let value = ValueId(0);
            let index = ValueId(1);
            let item = ValueId(2);
            let ops = vec![make_op(
                OpCode::Index,
                vec![value, index],
                vec![item],
                AttrDict::new(),
            )];
            let mut func = single_block_func(ops, 3);
            func.value_types.insert(value, TirType::Str);
            func.value_types.insert(index, index_ty.clone());

            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&item),
                Some(&TirType::Str),
                "str indexed by {index_ty:?} should refine to Str"
            );
        }
    }

    #[test]
    fn bytes_index_refines_to_i64_for_integer_indices() {
        for index_ty in [TirType::I64, TirType::Bool] {
            let value = ValueId(0);
            let index = ValueId(1);
            let item = ValueId(2);
            let ops = vec![make_op(
                OpCode::Index,
                vec![value, index],
                vec![item],
                AttrDict::new(),
            )];
            let mut func = single_block_func(ops, 3);
            func.value_types.insert(value, TirType::Bytes);
            func.value_types.insert(index, index_ty.clone());

            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&item),
                Some(&TirType::I64),
                "bytes indexed by {index_ty:?} should refine to I64"
            );
        }
    }

    #[test]
    fn immutable_sequence_index_with_non_integer_index_stays_dynbox() {
        for value_ty in [TirType::Str, TirType::Bytes] {
            let value = ValueId(0);
            let index = ValueId(1);
            let item = ValueId(2);
            let ops = vec![make_op(
                OpCode::Index,
                vec![value, index],
                vec![item],
                AttrDict::new(),
            )];
            let mut func = single_block_func(ops, 3);
            func.value_types.insert(value, value_ty.clone());
            func.value_types.insert(index, TirType::Str);

            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&item),
                Some(&TirType::DynBox),
                "{value_ty:?} indexed by Str must stay conservative"
            );
        }
    }

    #[test]
    fn tuple_index_refines_homogeneous_element_type() {
        let tuple = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![tuple, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types
            .insert(tuple, TirType::Tuple(vec![TirType::Str, TirType::Str]));
        func.value_types.insert(index, TirType::I64);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&item), Some(&TirType::Str));
    }

    #[test]
    fn tuple_index_refines_to_element_join_for_mixed_tuple() {
        let tuple = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![tuple, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(
            tuple,
            TirType::Tuple(vec![TirType::I64, TirType::Str, TirType::I64]),
        );
        func.value_types.insert(index, TirType::I64);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::Union(vec![TirType::I64, TirType::Str]))
        );
    }

    #[test]
    fn dict_index_refines_matching_key_to_value_type() {
        let dict = ValueId(0);
        let key = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![dict, key],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(
            dict,
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
        );
        func.value_types.insert(key, TirType::Str);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&item), Some(&TirType::I64));
    }

    #[test]
    fn dict_index_with_nonmatching_key_stays_dynbox() {
        let dict = ValueId(0);
        let key = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![dict, key],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(
            dict,
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
        );
        func.value_types.insert(key, TirType::I64);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
    }

    #[test]
    fn builtin_len_return_refines_to_i64_without_transport_hint() {
        let list = ValueId(0);
        let result = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("len".into()));
        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![list],
            vec![result],
            attrs,
        )];
        let mut func = single_block_func(ops, 2);
        func.value_types
            .insert(list, TirType::List(Box::new(TirType::DynBox)));

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&result), Some(&TirType::I64));
    }

    #[test]
    fn builtin_predicate_returns_refine_to_bool() {
        for name in ["bool", "hasattr", "isinstance", "issubclass"] {
            let value = ValueId(0);
            let result = ValueId(1);
            let mut attrs = AttrDict::new();
            attrs.insert("name".into(), AttrValue::Str(name.into()));
            let ops = vec![make_op(
                OpCode::CallBuiltin,
                vec![value],
                vec![result],
                attrs,
            )];
            let mut func = single_block_func(ops, 2);
            func.value_types.insert(value, TirType::DynBox);

            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&result),
                Some(&TirType::Bool),
                "call_builtin {name} should refine to Bool"
            );
        }
    }

    #[test]
    fn builtin_ord_and_chr_return_types_refine() {
        let value = ValueId(0);
        let ord_result = ValueId(1);
        let chr_result = ValueId(2);
        let mut ord_attrs = AttrDict::new();
        ord_attrs.insert("name".into(), AttrValue::Str("ord".into()));
        let mut chr_attrs = AttrDict::new();
        chr_attrs.insert("name".into(), AttrValue::Str("chr".into()));
        let ops = vec![
            make_op(
                OpCode::CallBuiltin,
                vec![value],
                vec![ord_result],
                ord_attrs,
            ),
            make_op(
                OpCode::CallBuiltin,
                vec![ord_result],
                vec![chr_result],
                chr_attrs,
            ),
        ];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, TirType::Str);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&ord_result), Some(&TirType::I64));
        assert_eq!(type_map.get(&chr_result), Some(&TirType::Str));
    }

    #[test]
    fn ord_at_return_type_refines_to_i64() {
        let text = ValueId(0);
        let index = ValueId(1);
        let result = ValueId(2);
        let ops = vec![make_op(
            OpCode::OrdAt,
            vec![text, index],
            vec![result],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(text, TirType::Str);
        func.value_types.insert(index, TirType::I64);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&result), Some(&TirType::I64));
    }

    #[test]
    fn unknown_builtin_return_stays_dynbox() {
        let value = ValueId(0);
        let result = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("dynamic_builtin".into()));
        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![value],
            vec![result],
            attrs,
        )];
        let mut func = single_block_func(ops, 2);
        func.value_types.insert(value, TirType::DynBox);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&result), Some(&TirType::DynBox));
    }

    #[test]
    fn iter_next_unboxed_done_flag_refines_to_bool() {
        let iter = ValueId(0);
        let elem = ValueId(1);
        let done = ValueId(2);
        let ops = vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter],
            vec![elem, done],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(iter, TirType::DynBox);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&elem),
            Some(&TirType::DynBox),
            "iterator element stays conservative until iterator element provenance is represented"
        );
        assert_eq!(type_map.get(&done), Some(&TirType::Bool));
        assert_eq!(
            func.value_types.get(&done),
            Some(&TirType::Bool),
            "refine_types must persist multi-result done-flag facts"
        );
    }

    #[test]
    fn get_iter_refines_known_iterable_element_types() {
        let cases = [
            (
                TirType::List(Box::new(TirType::I64)),
                TirType::Iterator(Box::new(TirType::I64)),
            ),
            (
                TirType::Set(Box::new(TirType::Str)),
                TirType::Iterator(Box::new(TirType::Str)),
            ),
            (
                TirType::Tuple(vec![TirType::I64, TirType::Str]),
                TirType::Iterator(Box::new(TirType::Union(vec![TirType::I64, TirType::Str]))),
            ),
            (
                TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
                TirType::Iterator(Box::new(TirType::Str)),
            ),
            (TirType::Str, TirType::Iterator(Box::new(TirType::Str))),
            (TirType::Bytes, TirType::Iterator(Box::new(TirType::I64))),
        ];

        for (iterable_ty, expected_iter_ty) in cases {
            let iterable = ValueId(0);
            let iter = ValueId(1);
            let ops = vec![make_op(
                OpCode::GetIter,
                vec![iterable],
                vec![iter],
                AttrDict::new(),
            )];
            let mut func = single_block_func(ops, 2);
            func.value_types.insert(iterable, iterable_ty.clone());

            refine_types(&mut func);
            let type_map = extract_type_map(&func);

            assert_eq!(
                type_map.get(&iter),
                Some(&expected_iter_ty),
                "GetIter({iterable_ty:?}) should refine to {expected_iter_ty:?}"
            );
        }
    }

    #[test]
    fn iterator_consumers_refine_element_types() {
        let iter = ValueId(0);
        let iter_next_elem = ValueId(1);
        let unboxed_elem = ValueId(2);
        let done = ValueId(3);
        let for_iter_elem = ValueId(4);
        let ops = vec![
            make_op(
                OpCode::IterNext,
                vec![iter],
                vec![iter_next_elem],
                AttrDict::new(),
            ),
            make_op(
                OpCode::IterNextUnboxed,
                vec![iter],
                vec![unboxed_elem, done],
                AttrDict::new(),
            ),
            make_op(
                OpCode::ForIter,
                vec![iter],
                vec![for_iter_elem],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 5);
        func.value_types
            .insert(iter, TirType::Iterator(Box::new(TirType::I64)));

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(type_map.get(&iter_next_elem), Some(&TirType::I64));
        assert_eq!(type_map.get(&unboxed_elem), Some(&TirType::I64));
        assert_eq!(type_map.get(&done), Some(&TirType::Bool));
        assert_eq!(type_map.get(&for_iter_elem), Some(&TirType::I64));
    }

    #[test]
    fn iter_next_unboxed_done_flag_not_proven_without_proven_iterator() {
        let iter = ValueId(0);
        let elem = ValueId(1);
        let done = ValueId(2);
        let ops = vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter],
            vec![elem, done],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(iter, TirType::DynBox);

        refine_types(&mut func);
        let proven = extract_proven_map(&func);

        assert_eq!(proven.get(&elem), None);
        assert_eq!(
            proven.get(&done),
            None,
            "done flag type is inferred but not proven unless the iterator operand is proven"
        );
    }

    // ---- Test: parse_guard_type handles various type strings ----
    #[test]
    fn parse_guard_type_variants() {
        let cases = vec![
            ("int", TirType::I64),
            ("INT", TirType::I64),
            ("i64", TirType::I64),
            ("float", TirType::F64),
            ("f64", TirType::F64),
            ("bool", TirType::Bool),
            ("str", TirType::Str),
            ("string", TirType::Str),
            ("none", TirType::None),
            ("NoneType", TirType::None),
            ("bytes", TirType::Bytes),
            ("bigint", TirType::BigInt),
        ];
        for (input, expected) in cases {
            let mut attrs = AttrDict::new();
            attrs.insert("expected_type".into(), AttrValue::Str(input.into()));
            assert_eq!(
                parse_guard_type(&attrs),
                Some(expected),
                "parse_guard_type({:?}) mismatch",
                input
            );
        }
    }

    // ---- Test: parse_guard_type returns None for unknown types ----
    #[test]
    fn parse_guard_type_unknown() {
        let mut attrs = AttrDict::new();
        attrs.insert(
            "expected_type".into(),
            AttrValue::Str("SomeCustomClass".into()),
        );
        assert_eq!(parse_guard_type(&attrs), None);
    }

    // ---- Test: TypeGuard with "ty" attr (used by type_guard_hoist) ----
    #[test]
    fn typeguard_ty_attr_works() {
        let mut attrs = AttrDict::new();
        attrs.insert("ty".into(), AttrValue::Str("INT".into()));
        assert_eq!(parse_guard_type(&attrs), Some(TirType::I64));
    }

    // ---- Test: parse_return_type_str routes through TirType::from_type_hint ----
    /// Pin the contract that `parse_return_type_str` uses the
    /// centralized `TirType::from_type_hint` helper, so any future
    /// hint added there (e.g. richer `Func:<sig>` parsing) is
    /// automatically picked up by the type-refine seeding path.
    /// Builtin scalars + None / NoneType keep their existing
    /// behavior; containers + BigInt + user classes are newly
    /// refined (previously returned None and stayed DynBox).
    #[test]
    fn parse_return_type_str_uses_centralized_helper() {
        // Existing builtin-scalar contracts (preserved).
        assert_eq!(parse_return_type_str("int"), Some(TirType::I64));
        assert_eq!(parse_return_type_str("float"), Some(TirType::F64));
        assert_eq!(parse_return_type_str("bool"), Some(TirType::Bool));
        assert_eq!(parse_return_type_str("str"), Some(TirType::Str));
        assert_eq!(parse_return_type_str("bytes"), Some(TirType::Bytes));
        assert_eq!(parse_return_type_str("None"), Some(TirType::None));
        assert_eq!(parse_return_type_str("NoneType"), Some(TirType::None));

        // Newly refined container/special types.
        assert_eq!(
            parse_return_type_str("list"),
            Some(TirType::List(Box::new(TirType::DynBox))),
            "method returning `list` must seed type-refine with \
             List(DynBox), not DynBox — otherwise lane inference \
             never sees the container type"
        );
        assert_eq!(
            parse_return_type_str("dict"),
            Some(TirType::Dict(
                Box::new(TirType::DynBox),
                Box::new(TirType::DynBox)
            ))
        );
        assert_eq!(
            parse_return_type_str("set"),
            Some(TirType::Set(Box::new(TirType::DynBox)))
        );
        assert_eq!(
            parse_return_type_str("tuple"),
            Some(TirType::Tuple(Vec::new()))
        );
        assert_eq!(parse_return_type_str("BigInt"), Some(TirType::BigInt));

        // User-class refinement: the live use of TirType::UserClass
        // through the type-refine seeding path.
        assert_eq!(
            parse_return_type_str("Point"),
            Some(TirType::UserClass("Point".into())),
            "method returning a user class must propagate UserClass \
             through type-refine — enables direct dispatch / \
             escape analysis precision on the result of factory \
             methods"
        );
        assert_eq!(
            parse_return_type_str("MyDataClass"),
            Some(TirType::UserClass("MyDataClass".into()))
        );

        // Structured compound containers refine through the same helper.
        assert_eq!(
            parse_return_type_str("list[int]"),
            Some(TirType::List(Box::new(TirType::I64)))
        );
        assert_eq!(
            parse_return_type_str("dict[str, list[float]]"),
            Some(TirType::Dict(
                Box::new(TirType::Str),
                Box::new(TirType::List(Box::new(TirType::F64)))
            ))
        );

        // Dynamic / malformed / unknown hints fall through to None so the
        // caller's operand-based inference takes over (rather than
        // forcing DynBox).
        assert_eq!(parse_return_type_str("Any"), None);
        assert_eq!(parse_return_type_str("Unknown"), None);
        assert_eq!(parse_return_type_str(""), None);
        assert_eq!(parse_return_type_str("Func:foo"), None);
        assert_eq!(parse_return_type_str("BoundMethod:list:append"), None);
        assert_eq!(parse_return_type_str("list[]"), None);
        assert_eq!(parse_return_type_str("list[Any]"), None);
    }

    #[test]
    fn object_new_bound_type_hint_is_structural_class_result_type() {
        let result = ValueId(0);
        let mut attrs = AttrDict::new();
        attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
        let mut func = single_block_func(
            vec![make_op(OpCode::ObjectNewBound, vec![], vec![result], attrs)],
            1,
        );
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
            values: vec![result],
        };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&result),
            Some(&TirType::UserClass("Point".into())),
            "object_new_bound _type_hint is the structural class-id contract, not legacy scalar transport",
        );
    }

    #[test]
    fn legacy_type_hint_does_not_refine_call_return_type() {
        let result = ValueId(0);
        let mut attrs = AttrDict::new();
        attrs.insert("_type_hint".into(), AttrValue::Str("int".into()));
        let mut func = single_block_func(
            vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
            1,
        );
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
            values: vec![result],
        };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&result),
            Some(&TirType::DynBox),
            "legacy SimpleIR `_type_hint` must remain semantic transport metadata, not call-return proof",
        );
    }

    #[test]
    fn structural_return_type_refines_call_return_type() {
        let result = ValueId(0);
        let mut attrs = AttrDict::new();
        attrs.insert("return_type".into(), AttrValue::Str("int".into()));
        attrs.insert("_type_hint".into(), AttrValue::Str("str".into()));
        let mut func = single_block_func(
            vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
            1,
        );
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
            values: vec![result],
        };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&result),
            Some(&TirType::I64),
            "explicit structural return_type remains the call-return refinement contract",
        );
    }
}
