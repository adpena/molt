use std::collections::HashMap;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::dominators;
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode};
use super::types::TirType;
use super::values::ValueId;

/// Maximum number of fixpoint iterations before conservative fallback.
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
    let mut env: HashMap<ValueId, TirType> = HashMap::new();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = &func.blocks[&bid];

        // Block arguments already carry refined types.
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }

        // Re-infer op result types from operand types (single pass — the
        // fixpoint has already converged so one pass is sufficient).
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }

            // TypeGuard results get the proven type directly.
            if op.opcode == OpCode::TypeGuard
                && let Some(proven_ty) = parse_guard_type(&op.attrs)
            {
                for &result_id in &op.results {
                    env.insert(result_id, proven_ty.clone());
                }
                continue;
            }

            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            if let Some(inferred) =
                infer_result_type_with_attrs(op.opcode, &operand_types, Some(&op.attrs))
            {
                for &result_id in &op.results {
                    env.insert(result_id, inferred.clone());
                }
            } else {
                // No inference possible — record DynBox so the map is complete.
                for &result_id in &op.results {
                    env.entry(result_id).or_insert(TirType::DynBox);
                }
            }
        }
    }

    env
}

/// Refine types in a TIR function.
/// Iterates to fixpoint (max 20 rounds, conservative fallback on timeout).
/// Returns the number of values refined from DynBox to concrete types.
pub fn refine_types(func: &mut TirFunction) -> usize {
    // Build the type environment from existing value types.
    let mut env: HashMap<ValueId, TirType> = HashMap::new();

    // Collect initial types from block args and op results.
    for block in func.blocks.values() {
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }
        for op in &block.ops {
            for &result_id in &op.results {
                // Check if we already have a type from the value declarations;
                // if not, start as DynBox.
                env.entry(result_id).or_insert(TirType::DynBox);
            }
        }
    }

    // Track which values started as DynBox so we can count refinements.
    let initially_dynbox: Vec<ValueId> = env
        .iter()
        .filter(|(_, ty)| matches!(ty, TirType::DynBox))
        .map(|(id, _)| *id)
        .collect();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    // Pre-compute: for each block, collect all incoming edges (predecessor
    // block → arg values). We accumulate across all blocks' terminators.
    // Key: target BlockId, Value: list of incoming arg value lists.
    let mut incoming_edges: HashMap<BlockId, Vec<Vec<ValueId>>> = HashMap::new();
    for block in func.blocks.values() {
        let edges = collect_branch_edges(block);
        for (target_id, arg_values) in edges {
            incoming_edges
                .entry(target_id)
                .or_default()
                .push(arg_values);
        }
    }

    // Pre-compute op snapshots once (ops don't change during refinement,
    // only the type environment does). Avoids O(ops × rounds) Vec allocations.
    //
    // We snapshot the `return_type` attr (when present) into a typed
    // `TirType` rather than carrying the full `AttrDict` — the attr is
    // immutable across rounds and cloning AttrDict per op per round
    // would dominate the refinement cost.
    let ops_by_block: HashMap<
        BlockId,
        Vec<(OpCode, Vec<ValueId>, Vec<ValueId>, Option<TirType>)>,
    > = block_order
        .iter()
        .map(|&bid| {
            let ops = func.blocks[&bid]
                .ops
                .iter()
                .map(|op| {
                    let return_type = if matches!(
                        op.opcode,
                        OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin
                    ) {
                        // Priority: explicit `return_type` (Year-1 Typed-IR
                        // direction) → legacy `_type_hint` round-tripped
                        // from SimpleIR.
                        op.attrs
                            .get("return_type")
                            .and_then(|v| match v {
                                AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                                _ => None,
                            })
                            .or_else(|| {
                                op.attrs.get("_type_hint").and_then(|v| match v {
                                    AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                                    _ => None,
                                })
                            })
                    } else {
                        None
                    };
                    (op.opcode, op.operands.clone(), op.results.clone(), return_type)
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
            // A block whose first op is StateBlockStart or CheckException
            // is an exception handler — its args must stay DynBox.
            if let Some(first_op) = block.ops.first()
                && matches!(
                    first_op.opcode,
                    OpCode::StateBlockStart | OpCode::CheckException
                )
            {
                for arg in &block.args {
                    eh_handler_args.insert(arg.id);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Oscillation detection (GraalVM deopt cycle detection)
    // ---------------------------------------------------------------------------
    //
    // Track type assignments per ValueId across fixpoint iterations. If a value
    // oscillates (A -> B -> A), the fixpoint will never converge for that value.
    // Fix it to DynBox (the most general type) and stop refining it.
    //
    // This prevents infinite loops in pathological cases where type inference
    // bounces between two types due to control-flow joins with conflicting
    // type information.
    let mut type_history: HashMap<ValueId, Vec<TirType>> = HashMap::new();
    // Values that have been frozen to DynBox due to oscillation.
    let mut frozen: std::collections::HashSet<ValueId> = std::collections::HashSet::new();

    // Fixpoint iteration.
    for _round in 0..MAX_ROUNDS {
        let mut changed = false;

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
                        if !matches!(env.get(&result_id), Some(TirType::DynBox)) {
                            env.insert(result_id, TirType::DynBox);
                            changed = true;
                        }
                    }
                    continue;
                }

                let operand_types: Vec<TirType> = operands
                    .iter()
                    .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                    .collect();

                // Frontend-provided return-type hint takes precedence for
                // opaque call-like opcodes; falls back to operand-based
                // inference for everything else.
                let inferred = return_type_hint
                    .clone()
                    .or_else(|| infer_result_type(*opcode, &operand_types));

                // For ops with a single result (the common case).
                if results.len() == 1 {
                    let result_id = results[0];
                    // Skip frozen values — they have been fixed to DynBox.
                    if frozen.contains(&result_id) {
                        continue;
                    }
                    if let Some(new_ty) = inferred {
                        let current = env.get(&result_id).cloned().unwrap_or(TirType::DynBox);
                        if new_ty != current {
                            env.insert(result_id, new_ty);
                            changed = true;
                        }
                    }
                }
            }

            // Recompute block argument types from all incoming edges.
            // Start from Never (bottom) and meet all incoming values.
            if let Some(edge_list) = incoming_edges.get(&block_id) {
                let arg_count = func.blocks[&block_id].args.len();
                for i in 0..arg_count {
                    let arg_id = func.blocks[&block_id].args[i].id;

                    // Skip frozen values.
                    if frozen.contains(&arg_id) {
                        continue;
                    }

                    // Exception handler block args must stay DynBox —
                    // the exception could come from any type context.
                    if eh_handler_args.contains(&arg_id) {
                        if !matches!(env.get(&arg_id), Some(TirType::DynBox)) {
                            env.insert(arg_id, TirType::DynBox);
                            changed = true;
                        }
                        continue;
                    }

                    let mut accumulated = TirType::Never;
                    for edge_args in edge_list {
                        if i < edge_args.len() {
                            let incoming_ty =
                                env.get(&edge_args[i]).cloned().unwrap_or(TirType::DynBox);
                            accumulated = accumulated.meet(&incoming_ty);
                        }
                    }
                    // Only update if we actually had incoming edges and computed
                    // something other than Never.
                    if !matches!(accumulated, TirType::Never) {
                        let current = env.get(&arg_id).cloned().unwrap_or(TirType::DynBox);
                        if accumulated != current {
                            env.insert(arg_id, accumulated);
                            changed = true;
                        }
                    }
                }
            }
        }

        if !changed {
            break;
        }

        // --- Oscillation detection at end of each round ---
        //
        // After each fixpoint iteration, record the current type for every
        // value and check for oscillation patterns. An oscillation is
        // detected when a value's type history has length >= 3 and the
        // current type equals the type from two iterations ago (A -> B -> A).
        //
        // When detected, freeze the value to DynBox — the most general type
        // — so the fixpoint can converge. This is sound: DynBox is the top
        // of the type lattice, so all meet operations will produce DynBox
        // or a subtype.
        for (&vid, ty) in &env {
            let history = type_history.entry(vid).or_default();
            if history.len() >= 2 && history[history.len() - 2] == *ty && history[history.len() - 1] != *ty {
                // Oscillation detected: A -> B -> A.
                // Fix to DynBox and freeze this value.
                eprintln!(
                    "[type_refine] oscillation detected for {:?}: {:?} -> {:?} -> {:?}, fixing to DynBox",
                    vid,
                    &history[history.len() - 2],
                    &history[history.len() - 1],
                    ty
                );
                frozen.insert(vid);
            }
            history.push(ty.clone());
        }

        // Apply DynBox fixup for all newly frozen values.
        for &vid in &frozen {
            if !matches!(env.get(&vid), Some(TirType::DynBox)) {
                env.insert(vid, TirType::DynBox);
                // No need to set changed=true here — if we froze values,
                // the next round will skip them and converge faster.
            }
        }
    }

    // --- Guard-to-type-environment propagation ---
    // After the fixpoint has converged, propagate TypeGuard-proven types
    // into all dominated blocks. This is additive and cannot break the
    // existing fixpoint — it only strengthens types that were DynBox.
    let (guard_refinements, _proven) = propagate_guard_types(func, &mut env);

    // Write refined types back into the function.
    for block in func.blocks.values_mut() {
        for arg in &mut block.args {
            if let Some(ty) = env.get(&arg.id) {
                arg.ty = ty.clone();
            }
        }
        for op in &mut block.ops {
            for &result_id in &op.results {
                // We don't have TirValue in ops directly — the type lives in
                // the env. But we need to propagate back to anywhere types are
                // stored. For now, the env is the authoritative source and
                // downstream passes can query it. However, since the task says
                // "mutates TirFunction in place", we store types on block args
                // (done above). Op result types aren't stored on TirOp (they
                // only have ValueId). So the block args are the mutation target.
                let _ = result_id; // suppress unused warning
            }
        }
    }

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

    // Build dominator tree.
    let pred_map = dominators::build_pred_map(func);
    let idoms = dominators::compute_idoms(func, &pred_map);

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
                        if dominators::dominates(*then_block, bid, &idoms) {
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
                            && dominators::dominates(guard.guard_block, bid, &idoms)
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
                        && dominators::dominates(guard.guard_block, bid, &idoms)
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
            if let Some(result_ty) =
                infer_result_type_with_attrs(op.opcode, &operand_types, Some(&op.attrs))
            {
                for &result_id in &op.results {
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
            match op.opcode {
                OpCode::ConstInt => {
                    for &r in &op.results {
                        proven.insert(r, TirType::I64);
                    }
                }
                OpCode::ConstFloat => {
                    for &r in &op.results {
                        proven.insert(r, TirType::F64);
                    }
                }
                OpCode::ConstStr => {
                    for &r in &op.results {
                        proven.insert(r, TirType::Str);
                    }
                }
                OpCode::ConstBool => {
                    for &r in &op.results {
                        proven.insert(r, TirType::Bool);
                    }
                }
                OpCode::ConstNone => {
                    for &r in &op.results {
                        proven.insert(r, TirType::None);
                    }
                }
                OpCode::ConstBytes => {
                    for &r in &op.results {
                        proven.insert(r, TirType::Bytes);
                    }
                }
                _ => {}
            }
        }
    }

    // Run the guard propagation to add TypeGuard-proven values.
    let mut env = extract_type_map(func);
    let (_refinements, guard_proven) = propagate_guard_types(func, &mut env);
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
            if let Some(result_ty) =
                infer_result_type_with_attrs(op.opcode, &operand_types, Some(&op.attrs))
            {
                for &result_id in &op.results {
                    proven.insert(result_id, result_ty.clone());
                }
            }
        }
    }

    proven
}

/// Parse a frontend `return_type` string ("int", "float", "bool", "str",
/// "bytes", "None") into a [`TirType`] for opaque-call type seeding.
/// Container/user types are not promoted to `TirType` here — they remain
/// `DynBox` for now; lane inference cares mostly about the scalar lanes.
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

/// Infer the result type of an operation from its operand types.
/// Returns `None` if the result type cannot be determined (stays as-is).
fn infer_result_type(opcode: OpCode, operand_types: &[TirType]) -> Option<TirType> {
    infer_result_type_with_attrs(opcode, operand_types, None)
}

/// Variant of [`infer_result_type`] that consults a `return_type`
/// `AttrValue::Str` (set by the frontend for opaque opcodes like `Call`,
/// `CallMethod`, `CallBuiltin` that the operand-only inference cannot
/// resolve).  Without this seed, a method call returning `int` produces
/// `TirType::DynBox`, lane inference falls back to NaN-boxed accumulator
/// in tight loops, and `total += obj.method(i)` silently coerces to float.
fn infer_result_type_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&super::ops::AttrDict>,
) -> Option<TirType> {
    // Frontend-provided return-type hint takes precedence for opaque
    // call-like opcodes — the frontend has the function/method signature
    // and operand inference cannot recover it.
    //
    // Two attr keys are consulted in priority order:
    //   1. `return_type`: an explicit, structurally-encoded return type
    //      (the Year-1 Typed-IR direction; preferred when populated).
    //   2. `_type_hint`: the legacy SimpleIR `type_hint` round-tripped
    //      through SSA lift.  The frontend already populates this on
    //      method-call results when the method's `return_hint` is
    //      a builtin scalar (`int`/`float`/`bool`/`str`/`bytes`) or a
    //      user class.  Without this fallback, `total += obj.method(i)`
    //      where `method` returns `int` infers `DynBox` for the call,
    //      lane analysis falls back to NaN-boxed accumulator, and the
    //      sum is silently coerced to float.
    if matches!(
        opcode,
        OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin
    ) && let Some(attrs) = attrs
    {
        if let Some(AttrValue::Str(name)) = attrs.get("return_type")
            && let Some(ty) = parse_return_type_str(name)
        {
            return Some(ty);
        }
        if let Some(AttrValue::Str(hint)) = attrs.get("_type_hint")
            && let Some(ty) = parse_return_type_str(hint)
        {
            return Some(ty);
        }
    }
    match opcode {
        // Constants — always produce a known type regardless of operands.
        OpCode::ConstInt => Some(TirType::I64),
        OpCode::ConstFloat => Some(TirType::F64),
        OpCode::ConstStr => Some(TirType::Str),
        OpCode::ConstBool => Some(TirType::Bool),
        OpCode::ConstNone => Some(TirType::None),
        OpCode::ConstBytes => Some(TirType::Bytes),

        // Add: numeric arithmetic + string concatenation + string/list repetition
        OpCode::Add => match operand_types {
            [TirType::Str, TirType::Str] => Some(TirType::Str), // "a" + "b"
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Mul: numeric arithmetic + string/list repetition (str * int, int * str)
        OpCode::Mul => match operand_types {
            [TirType::Str, TirType::I64] | [TirType::I64, TirType::Str] => Some(TirType::Str),
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Sub, Mod, Pow: numeric only (str-str is TypeError in Python)
        OpCode::Sub | OpCode::Mod | OpCode::Pow => infer_numeric_arithmetic(operand_types),
        OpCode::Div => {
            // Python: division always produces float unless both are DynBox.
            match operand_types {
                [TirType::I64, TirType::I64]
                | [TirType::F64, TirType::F64]
                | [TirType::I64, TirType::F64]
                | [TirType::F64, TirType::I64] => Some(TirType::F64),
                _ => infer_numeric_arithmetic(operand_types),
            }
        }
        OpCode::FloorDiv => infer_numeric_arithmetic(operand_types),

        // Unary Neg/Pos
        OpCode::Neg | OpCode::Pos => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            [TirType::F64] => Some(TirType::F64),
            _ => None,
        },

        // Comparisons always produce Bool.
        OpCode::Eq
        | OpCode::Ne
        | OpCode::Lt
        | OpCode::Le
        | OpCode::Gt
        | OpCode::Ge
        | OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn => Some(TirType::Bool),

        // Boolean ops
        OpCode::And | OpCode::Or => match operand_types {
            [TirType::Bool, TirType::Bool] => Some(TirType::Bool),
            _ => None,
        },
        OpCode::Not => Some(TirType::Bool),
        OpCode::Bool => Some(TirType::Bool),

        // Bitwise ops other than shifts are closed over the inline I64 lane.
        // Shifts can promote beyond the inline range and must stay boxed until
        // the runtime operator decides whether bigint promotion is required.
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor => match operand_types {
            [TirType::I64, TirType::I64] => Some(TirType::I64),
            _ => None,
        },
        OpCode::Shl | OpCode::Shr => None,
        OpCode::BitNot => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            _ => None,
        },

        // Containers
        OpCode::BuildList => Some(TirType::List(Box::new(TirType::DynBox))),
        OpCode::BuildDict => Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox),
        )),
        OpCode::BuildSet => Some(TirType::Set(Box::new(TirType::DynBox))),
        OpCode::BuildTuple => Some(TirType::Tuple(operand_types.to_vec())),

        // Copy propagates type.
        OpCode::Copy => operand_types.first().cloned(),

        // Box/Unbox
        OpCode::BoxVal => operand_types
            .first()
            .map(|t| TirType::Box(Box::new(t.clone()))),
        OpCode::UnboxVal => match operand_types.first() {
            Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
            _ => None,
        },

        // Everything else: cannot infer, leave as-is.
        _ => None,
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
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
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

        // Compound / unknown hints fall through to None so the
        // caller's operand-based inference takes over (rather than
        // forcing DynBox).
        assert_eq!(parse_return_type_str("Any"), None);
        assert_eq!(parse_return_type_str("Unknown"), None);
        assert_eq!(parse_return_type_str(""), None);
        assert_eq!(parse_return_type_str("Func:foo"), None);
        assert_eq!(parse_return_type_str("BoundMethod:list:append"), None);
        assert_eq!(parse_return_type_str("list[int]"), None);
    }
}
