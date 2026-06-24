//! RC drop insertion (RC drop-insertion substrate, design 20, Phase 3).
//!
//! Inserts `DecRef` ops at every owned value's last use and `IncRef` ops before
//! suspension points for values that survive across a yield. This is the
//! compiler pass that closes molt's whole-program expression-temporary leak: the
//! runtime allocates every heap result with `ref_count = 1` and (before this
//! pass) never decremented it for expression temporaries.
//!
//! Runs `Mutates::Cfg`: it inserts `DecRef`/`IncRef` ops within blocks and MAY
//! SPLIT a critical edge (a fresh block carrying an edge-exact `IncRef`) for the
//! mixed-ownership-phi retain (§5 below). `DecRef`/`IncRef` carry no exception
//! edge, and the edge-split inserts only an unconditional `Branch` — but because
//! the block set/edges CAN change, the pass declares `Cfg` so the manager
//! recomputes CFG-sensitive analyses for the following `refcount_elim_post`.
//!
//! ## Ownership transfer at phi (block-arg) boundaries — the two-sided contract
//!
//! TIR uses MLIR block args as phis: a predecessor's terminator passes a value
//! that binds the successor's block arg on entry. A droppable (heap, function-
//! owned) block arg is treated as carrying ONE owned `+1`; the pass drops it
//! where it dies and TRANSFERS it (no drop) where it is forwarded. Soundness
//! requires BOTH halves of the transfer to be exact — the two over-release
//! classes the round-2/round-3 review exposed:
//!
//! * **Incoming side (§5, the `before_term_incref` / edge-split retain).** Every
//!   incoming edge of an owned phi must deliver an owned `+1`. An edge delivering
//!   a BORROWED value (a transparent alias of a `+0` parameter, or an owned value
//!   whose single `+1` is needed elsewhere too) is RETAINED on that edge. Without
//!   it, the phi's drop releases the caller's borrow → UAF (the loop-accumulator
//!   `x = base; while …: x = x + base` and the if-arm `x = a if c else …`).
//! * **Outgoing side (§3 transfer exclusion).** A value PASSED as a branch arg
//!   into a phi must NOT also be edge-dropped at the join OR in a descendant
//!   block while the phi remains live: its ownership moved into the block arg,
//!   which is released by the phi's own last-use drop. Liveness reports the
//!   forwarded value dead-in to the join (its successor-side identity is the
//!   distinct block-arg SSA value), so the edge-dying rule would otherwise drop it
//!   there, or later when the old source root appears dead, AND at the phi's last
//!   use → double-free.
//!
//! ## Ownership model (design 20 §1)
//!
//! Every op that returns a new heap reference returns it **owned** (`rc += 1`):
//! the current SSA holder is responsible for exactly one dec-ref before the value
//! goes out of scope. Operands are **borrowed** (the callee never decrefs its
//! args). So the drop rule is: at a value's last use, the holder releases its
//! ref — unless the last use itself transfers ownership (a Return value, a branch
//! arg passed to a successor block arg, or an operand the value-range / repr
//! filter proved carries no heap reference).
//!
//! ## What is dropped
//!
//! `DropEligibility` owns the composed predicate for whether a value root is a
//! drop candidate. A value `v` is eligible when ALL hold:
//! * `v` is heap-carrying (NOT a [`TirLivenessResult::is_raw_scalar`] — raw i64 /
//!   bool / float carriers hold no refcount; dropping them would pass a raw
//!   register to `molt_dec_ref_obj`).
//! * `v` is not produced by `StackAlloc` / `ObjectNewBoundStack` (stack lifetime,
//!   no RC — design R6).
//! * `v` is not a function parameter (parameters are borrowed from the caller per
//!   the ABI; the caller owns and drops them).
//!
//! ## Placement (design 20 §2.4–§2.7)
//!
//! * **Straight-line**: after the last op in a block that uses `v`, if `v` is not
//!   live-out of the block, insert `DecRef(v)` right after that op — UNLESS the
//!   last use is a borrow-into-call (see borrow inference below).
//! * **Edge-dying at successor entry** (§2.5, the OpsOnly form): if `v` is
//!   live-out of a predecessor but dead on entry to a particular successor (and
//!   not passed as that edge's block arg), insert `DecRef(v)` at the *start* of
//!   that successor. This avoids edge-splitting (a CFG mutation); the elim pass
//!   hoists the common case. Done by: for each block `B`, for each value live-in
//!   to `B`'s predecessors but dead in `B`, drop at `B`'s entry.
//! * **Loop-carried** (§2.7): a back-edge that passes a NEW value to a header
//!   block arg leaves the PREVIOUS iteration's value dead. The previous value is
//!   the header block arg itself (the phi); if it is not used after the point the
//!   new value is computed, drop it before the back-edge branch. This is the
//!   "consumer releases the slot" rule (CPython's `STORE_FAST`-on-overwrite).
//! * **Exception edges** (§2.6): `CheckException` successors are ordinary CFG
//!   successors here; a value live at the throw point but dead on a handler path
//!   is dropped at the handler's entry by the edge-dying rule.
//!
//! ## Suspension points (design 20 §2.9)
//!
//! For each `StateYield` / `ChanSendYield` / `ChanRecvYield` / `Yield` /
//! `YieldFrom`, every heap-carrying value live ACROSS the yield (live-out of the
//! block at the yield, used after a resume) is `IncRef`'d immediately before the
//! yield: the suspended coroutine frame now owns its own reference, which the
//! frame finalizer releases on teardown.
//!
//! ## Borrow inference (design 20 §3.2)
//!
//! If `v`'s last use is as an operand to a `Call` / `CallMethod` / `CallBuiltin`
//! and `v` is dead after the call, the callee borrows `v` for the call's
//! duration and the caller drops at last use — which is exactly the call site.
//! Inserting `DecRef(v)` right after the call is correct and is what the
//! straight-line rule does; there is no separate IncRef to elide here (molt's ABI
//! is borrow-args, so no IncRef was ever needed around the call). The borrow
//! inference therefore reduces to: drop after the call, never before — which the
//! last-use placement already does. Finalizer-sensitive values only override
//! that placement when they are Python-bound roots (`store_var` / explicit
//! delete boundary); unbound expression temporaries still die at their last use.
//! We keep the call operands out of any *pre-call* drop, which the last-use
//! semantics guarantee.
//!
//! ## Soundness invariants (the over-release hazards this pass must avoid)
//!
//! All ownership reasoning is done over transparent-alias ROOTS (see
//! [`crate::tir::passes::alias_analysis`]). A `Copy` / `TypeGuard` identity move
//! produces a second SSA handle to the SAME owned reference (design §1.2), NOT a
//! new allocation; treating it as a consuming use would double-free. Five
//! soundness rails, each FAIL-CLOSED (keep the +1 / leak rather than risk a UAF):
//!
//! 1. **Alias-root ownership** — a whole alias group is ONE reference, dropped
//!    once at the group's last in-block *touch*, through a live alias of the
//!    root. The drop point dominates every in-block read of the group, so a
//!    later alias-move can never read a freed object. A `Copy` result root that
//!    the ownership lattice classifies as non-owning is a no-incref
//!    bit-passthrough or no-heap marker, so it is excluded from droppability:
//!    releasing it would double-free operand 0 or drop a non-ref carrier.
//! 2. **TerminatorOnly dominance** — an edge-dying drop at a successor `B` is
//!    placed only when the value's def-block dominates `B` in the
//!    **terminator-only** CFG (the view codegen enforces). The *full*-CFG
//!    dominator would admit a value defined mid-block after a `CheckException`
//!    as "dominating" that op's handler, but the exception edge leaves before
//!    the def → use-before-def in codegen. (Observed otherwise as the LLVM
//!    verifier "Instruction does not dominate all uses!" abort.)
//! 3. **Python lifetime release boundaries** — a root released by `DelBoundary` /
//!    `DeleteVar` / pre-existing `DecRef`, statement finalizer release, or
//!    `store_var` scope cleanup is path-authoritative and is never edge-dropped
//!    at a join. The OpsOnly edge-dying form has one block-entry drop for all
//!    incoming paths; adding it beside a path-conditioned Python rebind/delete or
//!    later scope-exit boundary can release the same local owner twice.
//! 4. **Conditionally-valid iterator results** — an `IterNextUnboxed` value
//!    result (from the generated result-validity table) is valid ONLY on the
//!    not-done branch; its slot carries a non-owned `None` sentinel on the
//!    exhaustion edge. It is NEVER
//!    edge-dropped (and never IncRef'd onto a phi edge); the body straight-line
//!    rule releases it on the valid path.
//! 5. **State-machine gate** — the pass bails on full-function RC insertion for
//!    functions with generator/async `StateSwitch` / `StateTransition` /
//!    `StateYield` control flow (a `_poll` dispatcher re-enters
//!    `state_resume_*` blocks carrying none of the normal-flow values), in
//!    addition to `try`/`except` regions. Exception transport drops have their
//!    own idempotency marker so the handler-safe CreationRef/MatchRef releases
//!    can still be inserted before the full-function bail without pretending
//!    native's whole value-tracking RC substrate has been retired.
//! 6. **Backend conditioning** — drop insertion is wired into the shared
//!    pipeline for LLVM / WASM / native Cranelift / Luau. Native suppresses its
//!    legacy value-tracking RC substrate on `drop_inserted` functions, so TIR
//!    drops are the single RC authority for activated functions; Luau consumes
//!    the same shared facts as checked GC no-ops. See
//!    `pass_manager::target_uses_tir_drop_insertion`.
//!
//! ## Diagnostics
//!
//! `MOLT_DEBUG_DROP=<substr>` (or `=ALL`) writes a per-function dump of the
//! post-insertion block/op shape with per-operand repr tags to
//! `<artifact_root>/drop/<func>.txt`, including a `BAILED:` line for functions
//! the activation gate skipped. The instrument every optimization lands with.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::liveness::{TirLiveness, TirLivenessResult};
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;
#[cfg(test)]
use super::ownership_lattice_min::original_kind;
use super::ownership_lattice_min::{
    OwnershipLattice, PythonLifetimeFacts, StatementReleasePlan, copy_transparent_alias,
    exception_creation_ref_values, op_consumed_operand_root, op_result_absorbs_operand_ownership,
    terminator_branch_args, terminator_uses_root,
};

/// The function-level attr the pass sets (round-tripped to the native backend as
/// a marker op) so the SimpleIR `loop_reassign_old_val` ad-hoc dec-ref path is
/// disabled for drop-inserted functions — preventing the R1 double-drop.
pub const DROP_INSERTED_ATTR: &str = "drop_inserted";

/// Function-level attr for the exception-region-only pre-bail slice. It protects
/// CreationRef/MatchRef `DecRef`s across TIR<->SimpleIR round-trips and
/// `refcount_elim` re-runs, but native MUST NOT interpret it as full-function RC
/// ownership: handlers/state machines still need the legacy native value tracker
/// until shared DropInsertion covers their complete lifetime graph.
pub const EXCEPTION_REGION_DROPS_INSERTED_ATTR: &str = "exception_region_drops_inserted";

fn drop_inner_stage_audit_enabled(func: &TirFunction) -> bool {
    let enabled = std::env::var("MOLT_DROP_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_MODULE_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1");
    if !enabled {
        return false;
    }
    match std::env::var("MOLT_DROP_STAGE_AUDIT_FUNC") {
        Ok(filter) if !filter.trim().is_empty() => func.name.contains(filter.trim()),
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_drop_inner_stage_audit(
    func: &TirFunction,
    stage: &str,
    plans: Option<usize>,
    edge_splits: Option<usize>,
    roots: Option<usize>,
    blocks_seen: Option<usize>,
    elapsed_ms: u128,
) {
    if !drop_inner_stage_audit_enabled(func) {
        return;
    }
    let blocks = func.blocks.len();
    let ops = func
        .blocks
        .values()
        .fold(0usize, |count, block| count.saturating_add(block.ops.len()));
    eprintln!(
        "[molt-drop-inner-audit] stage={stage} function={} blocks={} ops={} plans={} edge_splits={} roots={} blocks_seen={} elapsed_ms={} peak_rss_mib={}",
        func.name,
        blocks,
        ops,
        plans
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        edge_splits
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        roots
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        blocks_seen
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms,
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}

#[inline]
pub(crate) fn attr_is_true(func: &TirFunction, name: &str) -> bool {
    matches!(func.attrs.get(name), Some(AttrValue::Bool(true)))
}

fn make_op(opcode: OpCode, operands: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn sorted_values(values: &[ValueId]) -> Vec<ValueId> {
    let mut ordered = values.to_vec();
    ordered.sort_unstable_by_key(|value| value.0);
    ordered
}

fn sorted_unique_values(values: &[ValueId]) -> Vec<ValueId> {
    let mut ordered = sorted_values(values);
    ordered.dedup();
    ordered
}

fn ordered_unique_after_op_values<F>(values: &[ValueId], op: &TirOp, canon: &F) -> Vec<ValueId>
where
    F: Fn(ValueId) -> ValueId,
{
    let mut remaining: HashSet<ValueId> = values.iter().copied().collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    for &operand in &op.operands {
        let root = canon(operand);
        if remaining.remove(&root) {
            ordered.push(root);
        }
    }
    for &result in &op.results {
        let root = canon(result);
        if remaining.remove(&root) {
            ordered.push(root);
        }
    }
    let mut rest: Vec<ValueId> = remaining.into_iter().collect();
    rest.sort_unstable_by_key(|value| value.0);
    ordered.extend(rest);
    ordered
}

#[derive(Debug, Default)]
struct ExceptionRegionDropInsertion {
    dec_refs_added: usize,
    cfg_changed: bool,
}

#[derive(Debug, Clone, Copy)]
struct ValueDefinition {
    block: BlockId,
    op_index: Option<usize>,
}

fn value_definitions(func: &TirFunction) -> HashMap<ValueId, ValueDefinition> {
    let mut defs: HashMap<ValueId, ValueDefinition> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            defs.insert(
                arg.id,
                ValueDefinition {
                    block: bid,
                    op_index: None,
                },
            );
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                defs.insert(
                    result,
                    ValueDefinition {
                        block: bid,
                        op_index: Some(op_index),
                    },
                );
            }
        }
    }
    defs
}

fn explicit_release_values(op: &TirOp) -> Vec<ValueId> {
    if op.opcode == OpCode::DecRef {
        return op.operands.to_vec();
    }
    if op.opcode == OpCode::DeleteVar {
        return op.operands.get(1).copied().into_iter().collect();
    }
    Vec::new()
}

fn insert_exception_creation_drops_at_raise(func: &mut TirFunction) -> usize {
    let creation_refs = exception_creation_ref_values(func);
    if creation_refs.is_empty() {
        return 0;
    }

    let mut inserted = 0usize;
    for block in func.blocks.values_mut() {
        let mut new_ops = Vec::with_capacity(block.ops.len());
        let mut changed = false;
        for op in &block.ops {
            new_ops.push(op.clone());
            if op.opcode != OpCode::Raise {
                continue;
            }
            let mut values: Vec<ValueId> = op
                .operands
                .iter()
                .copied()
                .filter(|value| creation_refs.contains(value))
                .collect();
            values.sort_unstable_by_key(|value| value.0);
            values.dedup();
            for value in values {
                new_ops.push(make_op(OpCode::DecRef, vec![value]));
                inserted += 1;
                changed = true;
            }
        }
        if changed {
            block.ops = new_ops;
        }
    }
    inserted
}

fn definition_available_before_position(
    def: ValueDefinition,
    position: crate::tir::exception_regions::ExceptionOpPosition,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    if def.block == position.block {
        return def
            .op_index
            .is_none_or(|op_index| op_index < position.op_index);
    }
    crate::tir::dominators::dominates(def.block, position.block, idoms)
}

fn definition_available_on_edge(
    def: ValueDefinition,
    pred: BlockId,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    def.block == pred || crate::tir::dominators::dominates(def.block, pred, idoms)
}

fn insert_exception_region_match_drops(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
) -> ExceptionRegionDropInsertion {
    let audit_start = std::time::Instant::now();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-before-analysis",
        None,
        None,
        None,
        None,
        audit_start.elapsed().as_millis(),
    );
    let release_to_matches = am
        .get::<crate::tir::exception_regions::ExceptionRegions>(func)
        .release_to_match_facts
        .clone();
    let release_fact_count: usize = release_to_matches.values().map(Vec::len).sum();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-after-analysis",
        None,
        None,
        Some(release_fact_count),
        Some(release_to_matches.len()),
        audit_start.elapsed().as_millis(),
    );
    if release_to_matches.is_empty() {
        return ExceptionRegionDropInsertion::default();
    }

    let pred_map_term = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map_term,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let defs = value_definitions(func);
    let mut result = ExceptionRegionDropInsertion::default();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-after-dominators",
        None,
        None,
        Some(defs.len()),
        Some(idoms.len()),
        audit_start.elapsed().as_millis(),
    );

    for (position, release_facts) in release_to_matches {
        emit_drop_inner_stage_audit(
            func,
            "exception-region-position-start",
            None,
            None,
            Some(release_facts.len()),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        let (original_args, pop_op, prefix_source_ops, tail_source_ops, tail_source_terminator) = {
            let Some(block) = func.blocks.get(&position.block) else {
                continue;
            };
            if position.op_index >= block.ops.len() {
                continue;
            }
            debug_assert_eq!(
                block.ops[position.op_index].opcode,
                OpCode::Copy,
                "ExceptionRegions release position must point at an exception_pop carrier"
            );
            (
                block.args.clone(),
                block.ops[position.op_index].clone(),
                block.ops[..position.op_index].to_vec(),
                block.ops[position.op_index + 1..].to_vec(),
                block.terminator.clone(),
            )
        };

        let mut incoming_arcs: Vec<(BlockId, ArcDescriptor, Vec<ValueId>)> = pred_map_term
            .get(&position.block)
            .into_iter()
            .flat_map(|preds| preds.iter().copied())
            .flat_map(|pred| {
                func.blocks
                    .get(&pred)
                    .map(|pred_block| {
                        terminator_arcs(&pred_block.terminator)
                            .into_iter()
                            .filter(move |arc| arc.target == position.block)
                            .map(move |arc| (pred, arc.descriptor, arc.args))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();
        incoming_arcs.sort_by_key(|(pred, _, _)| pred.0);
        emit_drop_inner_stage_audit(
            func,
            "exception-region-after-incoming-arcs",
            None,
            None,
            Some(incoming_arcs.len()),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );

        let all_incoming_preds: BTreeSet<BlockId> =
            incoming_arcs.iter().map(|(pred, _, _)| *pred).collect();
        let mut value_entry_preds: BTreeMap<ValueId, BTreeSet<BlockId>> = BTreeMap::new();
        let mut direct_values = BTreeSet::new();
        for fact in release_facts {
            let Some(&def) = defs.get(&fact.value) else {
                continue;
            };
            if fact.entry_predecessors.is_empty() {
                if definition_available_before_position(def, position, &idoms) {
                    direct_values.insert(fact.value);
                }
                continue;
            }
            value_entry_preds
                .entry(fact.value)
                .or_default()
                .extend(fact.entry_predecessors.iter().copied());
        }

        let mut global_values: Vec<ValueId> = value_entry_preds
            .iter()
            .filter_map(|(&value, preds)| {
                let def = *defs.get(&value)?;
                (!all_incoming_preds.is_empty()
                    && preds.is_superset(&all_incoming_preds)
                    && definition_available_before_position(def, position, &idoms))
                .then_some(value)
            })
            .collect();
        global_values.extend(direct_values.iter().copied());
        global_values.sort_unstable_by_key(|value| value.0);
        global_values.dedup();

        let global_set: BTreeSet<ValueId> = global_values.iter().copied().collect();
        let mut path_values: Vec<ValueId> = value_entry_preds
            .keys()
            .copied()
            .filter(|value| !global_set.contains(value))
            .collect();
        path_values.sort_unstable_by_key(|value| value.0);

        if path_values.is_empty() {
            let Some(block) = func.blocks.get_mut(&position.block) else {
                continue;
            };
            let mut new_ops = Vec::with_capacity(block.ops.len() + global_values.len());
            for (idx, op) in block.ops.iter().enumerate() {
                new_ops.push(op.clone());
                if idx == position.op_index {
                    for value in &global_values {
                        new_ops.push(make_op(OpCode::DecRef, vec![*value]));
                        result.dec_refs_added += 1;
                    }
                }
            }
            block.ops = new_ops;
            continue;
        }

        if incoming_arcs.is_empty() {
            continue;
        }

        let mut split_plans = Vec::new();
        for (pred, arc, args) in &incoming_arcs {
            let mut edge_values = global_values.clone();
            let mut edge_specific = Vec::new();
            for value in &path_values {
                let Some(preds) = value_entry_preds.get(value) else {
                    continue;
                };
                if !preds.contains(pred) {
                    continue;
                }
                let Some(&def) = defs.get(value) else {
                    continue;
                };
                if definition_available_on_edge(def, *pred, &idoms) {
                    edge_specific.push(*value);
                }
            }
            if edge_specific.is_empty() {
                continue;
            }
            edge_values.extend(edge_specific);
            edge_values.sort_unstable_by_key(|value| value.0);
            edge_values.dedup();
            split_plans.push((*pred, *arc, args.clone(), edge_values));
        }
        if split_plans.is_empty() {
            continue;
        }
        emit_drop_inner_stage_audit(
            func,
            "exception-region-before-split",
            None,
            Some(split_plans.len()),
            Some(
                split_plans
                    .iter()
                    .map(|(_, _, _, values)| values.len())
                    .sum(),
            ),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );

        let mut tail_arg_remap: HashMap<ValueId, ValueId> = HashMap::new();
        let after_args: Vec<TirValue> = original_args
            .iter()
            .map(|arg| {
                let new_id = func.fresh_value();
                tail_arg_remap.insert(arg.id, new_id);
                func.value_types.insert(new_id, arg.ty.clone());
                TirValue {
                    id: new_id,
                    ty: arg.ty.clone(),
                }
            })
            .collect();
        let mut tail_value_remap = tail_arg_remap.clone();
        for op in &prefix_source_ops {
            if let Some(alias) = copy_transparent_alias(op)
                && let Some(mapped_operand) = tail_value_remap.get(&alias.source).copied()
            {
                tail_value_remap.insert(alias.result, mapped_operand);
            }
        }
        let original_arg_values: Vec<ValueId> = original_args.iter().map(|arg| arg.id).collect();
        let tail_ops: Vec<TirOp> = tail_source_ops
            .iter()
            .map(|op| remap_op_operands(op, &tail_value_remap))
            .collect();
        let tail_terminator = remap_terminator_values(&tail_source_terminator, &tail_value_remap);
        let after_block = func.fresh_block();
        func.blocks.insert(
            after_block,
            TirBlock {
                id: after_block,
                args: after_args,
                ops: tail_ops,
                terminator: tail_terminator,
            },
        );

        if let Some(block) = func.blocks.get_mut(&position.block) {
            block.ops.truncate(position.op_index + 1);
            let mut original_ops = Vec::with_capacity(block.ops.len() + global_values.len());
            for (idx, op) in block.ops.iter().enumerate() {
                original_ops.push(op.clone());
                if idx == position.op_index {
                    for value in &global_values {
                        original_ops.push(make_op(OpCode::DecRef, vec![*value]));
                        result.dec_refs_added += 1;
                    }
                }
            }
            block.ops = original_ops;
            block.terminator = Terminator::Branch {
                target: after_block,
                args: original_arg_values.clone(),
            };
        }

        for (pred, arc, args, edge_values) in split_plans {
            let split_block = func.fresh_block();
            let mut ops = Vec::with_capacity(1 + edge_values.len());
            ops.push(pop_op.clone());
            for value in edge_values {
                ops.push(make_op(OpCode::DecRef, vec![value]));
                result.dec_refs_added += 1;
            }
            func.blocks.insert(
                split_block,
                TirBlock {
                    id: split_block,
                    args: vec![],
                    ops,
                    terminator: Terminator::Branch {
                        target: after_block,
                        args,
                    },
                },
            );
            if let Some(pred_block) = func.blocks.get_mut(&pred) {
                retarget_arc(&mut pred_block.terminator, &arc, split_block);
            }
        }
        emit_drop_inner_stage_audit(
            func,
            "exception-region-before-remap",
            None,
            None,
            Some(tail_value_remap.len()),
            Some(after_block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        remap_uses_dominated_by_split_continuation(func, after_block, &tail_value_remap);
        emit_drop_inner_stage_audit(
            func,
            "exception-region-after-remap",
            None,
            None,
            Some(tail_value_remap.len()),
            Some(after_block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        result.cfg_changed = true;
    }

    emit_drop_inner_stage_audit(
        func,
        "exception-region-complete",
        None,
        None,
        Some(result.dec_refs_added),
        None,
        audit_start.elapsed().as_millis(),
    );
    result
}

/// True if `opcode` is a suspension point that escapes live values into a
/// coroutine frame (design §2.9).
fn is_suspension_point(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::Yield
            | OpCode::YieldFrom
    )
}

/// A stable identifier for ONE outgoing arc of a terminator, so the mixed-
/// ownership-phi retain can retarget exactly that arc when splitting a critical
/// edge (two arcs to the same block with different args must be distinguishable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ArcDescriptor {
    /// The single arc of an unconditional `Branch`.
    Branch,
    /// The `then` arc of a `CondBranch`.
    CondThen,
    /// The `else` arc of a `CondBranch`.
    CondElse,
    /// The case arc at `cases[index]` of a `Switch`.
    SwitchCase(usize),
    /// The `default` arc of a `Switch`.
    SwitchDefault,
}

/// One outgoing arc of a block's terminator: which target it goes to, the args it
/// forwards, and a [`ArcDescriptor`] that pins it for retargeting.
struct Arc {
    descriptor: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
}

impl Arc {
    /// A self-loop arc whose source block is also its target (the latch IS the
    /// header) — treated as ambiguous for IncRef placement, since a
    /// before-terminator IncRef on such an arc would sit on the in-block
    /// straight-line path that the body's drops also traverse. Splitting isolates
    /// the retain onto the edge. `pred` is the block the arc originates from.
    fn is_self_loop_into_own_phi(&self, pred: BlockId) -> bool {
        self.target == pred
    }
}

/// Enumerate every outgoing arc of `term` with its forwarding args and descriptor.
fn terminator_arcs(term: &Terminator) -> Vec<Arc> {
    match term {
        Terminator::Branch { target, args } => vec![Arc {
            descriptor: ArcDescriptor::Branch,
            target: *target,
            args: args.clone(),
        }],
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => vec![
            Arc {
                descriptor: ArcDescriptor::CondThen,
                target: *then_block,
                args: then_args.clone(),
            },
            Arc {
                descriptor: ArcDescriptor::CondElse,
                target: *else_block,
                args: else_args.clone(),
            },
        ],
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut out: Vec<Arc> = cases
                .iter()
                .enumerate()
                .map(|(i, (_, b, args))| Arc {
                    descriptor: ArcDescriptor::SwitchCase(i),
                    target: *b,
                    args: args.clone(),
                })
                .collect();
            out.push(Arc {
                descriptor: ArcDescriptor::SwitchDefault,
                target: *default,
                args: default_args.clone(),
            });
            out
        }
        // `StateDispatch` mirrors `Switch`'s arc shape (cases + default).  Reuse
        // the `SwitchCase`/`SwitchDefault` descriptors: `drop_insertion` bails on
        // state-machine functions (the `has_state_machine` guard in `run`), so
        // this arm is unreachable in practice, but keeps the arc model total and
        // correct should that guard ever be lifted for `_poll` bodies.
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            let mut out: Vec<Arc> = cases
                .iter()
                .enumerate()
                .map(|(i, (_, b, args))| Arc {
                    descriptor: ArcDescriptor::SwitchCase(i),
                    target: *b,
                    args: args.clone(),
                })
                .collect();
            out.push(Arc {
                descriptor: ArcDescriptor::SwitchDefault,
                target: *default,
                args: default_args.clone(),
            });
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Retarget exactly the arc named by `desc` to `new_target`, and CLEAR that arc's
/// forwarded args (the inserted edge-split block now supplies them via its own
/// `Branch`). Used to splice a critical-edge-split block onto one arc.
fn retarget_arc(term: &mut Terminator, desc: &ArcDescriptor, new_target: BlockId) {
    match (term, desc) {
        (Terminator::Branch { target, args }, ArcDescriptor::Branch) => {
            *target = new_target;
            args.clear();
        }
        (
            Terminator::CondBranch {
                then_block,
                then_args,
                ..
            },
            ArcDescriptor::CondThen,
        ) => {
            *then_block = new_target;
            then_args.clear();
        }
        (
            Terminator::CondBranch {
                else_block,
                else_args,
                ..
            },
            ArcDescriptor::CondElse,
        ) => {
            *else_block = new_target;
            else_args.clear();
        }
        (Terminator::Switch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            if let Some((_, b, args)) = cases.get_mut(*i) {
                *b = new_target;
                args.clear();
            }
        }
        (
            Terminator::Switch {
                default,
                default_args,
                ..
            },
            ArcDescriptor::SwitchDefault,
        ) => {
            *default = new_target;
            default_args.clear();
        }
        // `StateDispatch` shares the `SwitchCase`/`SwitchDefault` arc descriptors
        // (see `terminator_arcs`).  Unreachable while `drop_insertion` bails on
        // state machines, but kept total for correctness if that guard is lifted.
        (Terminator::StateDispatch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            if let Some((_, b, args)) = cases.get_mut(*i) {
                *b = new_target;
                args.clear();
            }
        }
        (
            Terminator::StateDispatch {
                default,
                default_args,
                ..
            },
            ArcDescriptor::SwitchDefault,
        ) => {
            *default = new_target;
            default_args.clear();
        }
        // Descriptor/terminator mismatch is a logic error — the descriptor was
        // produced from THIS terminator by `terminator_arcs` and the terminator is
        // not mutated between enumeration and retarget. Leave unchanged (fail-
        // closed: a missed retarget keeps the original edge — the IncRef block is
        // then unreachable/dead, a leak at worst, never a UAF).
        _ => {}
    }
}

fn remap_value(value: ValueId, remap: &HashMap<ValueId, ValueId>) -> ValueId {
    remap.get(&value).copied().unwrap_or(value)
}

fn remap_op_operands(op: &TirOp, remap: &HashMap<ValueId, ValueId>) -> TirOp {
    let mut out = op.clone();
    out.operands = out
        .operands
        .iter()
        .map(|&value| remap_value(value, remap))
        .collect();
    out
}

fn remap_terminator_values(term: &Terminator, remap: &HashMap<ValueId, ValueId>) -> Terminator {
    let remap_values = |values: &[ValueId]| -> Vec<ValueId> {
        values
            .iter()
            .map(|&value| remap_value(value, remap))
            .collect()
    };
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: *target,
            args: remap_values(args),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: remap_value(*cond, remap),
            then_block: *then_block,
            then_args: remap_values(then_args),
            else_block: *else_block,
            else_args: remap_values(else_args),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: remap_value(*value, remap),
            cases: cases
                .iter()
                .map(|(case, target, args)| (*case, *target, remap_values(args)))
                .collect(),
            default: *default,
            default_args: remap_values(default_args),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(case, target, args)| (*case, *target, remap_values(args)))
                .collect(),
            default: *default,
            default_args: remap_values(default_args),
        },
        Terminator::Return { values } => Terminator::Return {
            values: remap_values(values),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

fn remap_uses_dominated_by_split_continuation(
    func: &mut TirFunction,
    continuation: BlockId,
    remap: &HashMap<ValueId, ValueId>,
) {
    if remap.is_empty() {
        return;
    }
    let pred_map = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let mut dominated_blocks: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|block| crate::tir::dominators::dominates(continuation, *block, &idoms))
        .collect();
    dominated_blocks.sort_unstable_by_key(|block| block.0);

    for bid in dominated_blocks {
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        for op in &mut block.ops {
            for operand in &mut op.operands {
                if let Some(new_value) = remap.get(operand).copied() {
                    *operand = new_value;
                }
            }
        }
        block.terminator = remap_terminator_values(&block.terminator, remap);
    }
}

/// A critical-edge split to materialize: insert a fresh block holding `retains`
/// IncRefs + a `Branch(target, args)`, and retarget `pred`'s `arc` to it.
struct EdgeSplit {
    pred: BlockId,
    arc: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
    retains: Vec<ValueId>,
    releases: Vec<ValueId>,
}

fn push_edge_split(
    splits: &mut Vec<EdgeSplit>,
    pred: BlockId,
    arc: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
    retains: Vec<ValueId>,
    releases: Vec<ValueId>,
) {
    if let Some(existing) = splits
        .iter_mut()
        .find(|split| split.pred == pred && split.arc == arc)
    {
        debug_assert_eq!(existing.target, target);
        debug_assert_eq!(existing.args, args);
        existing.retains.extend(retains);
        existing.releases.extend(releases);
        return;
    }
    splits.push(EdgeSplit {
        pred,
        arc,
        target,
        args,
        retains,
        releases,
    });
}

struct ExceptionArc {
    op_index: usize,
    target: BlockId,
    args: Vec<ValueId>,
}

fn exception_arcs_for_block(func: &TirFunction, block: &TirBlock) -> Vec<ExceptionArc> {
    let label_to_block = crate::tir::dominators::exception_label_to_block(func);
    block
        .ops
        .iter()
        .enumerate()
        .filter_map(|(op_index, op)| {
            if !crate::tir::dominators::is_exception_transfer_edge(op.opcode) {
                return None;
            }
            let target_label = match op.attrs.get("value") {
                Some(AttrValue::Int(label)) => *label,
                _ => return None,
            };
            let target = *label_to_block.get(&target_label)?;
            Some(ExceptionArc {
                op_index,
                target,
                args: op.operands.clone(),
            })
        })
        .collect()
}

/// Run drop insertion. See module docs for the algorithm.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "drop_insertion",
        ..Default::default()
    };

    // Conservative activation gate. Drop placement keys on single-entry
    // dominance (per-block last-use, edge-dying at successor entry), so it is
    // UNSOUND over any CFG that is not dominator-structured. Two such shapes are
    // bailed:
    //
    //  1. Real exception-HANDLER regions (`try`/`except` → `TryStart`/`TryEnd`,
    //     or a `StateBlockStart`/`StateBlockEnd`-delimited region) —
    //     `has_exception_handlers()`. (A bare universal `CheckException` is NOT a
    //     handler — it propagates to the function exception EXIT — and is fully
    //     handled as an ordinary CFG successor.)
    //
    //  2. A lowered coroutine `_poll` STATE MACHINE (`StateSwitch` dispatch +
    //     `StateTransition`/`StateYield`/`AllocTask`) — `has_state_machine()`.
    //     The state dispatch RE-ENTERS resume blocks, so a value defined in one
    //     state region reaches a resume block the dominator walk does NOT see as
    //     dominated; a drop placed there is a use-before-def (the LLVM verifier
    //     rejects it: `dec_ref %v` before `%v = ...`; on native it double-frees).
    //     Design §2.9's frame-finalizer model handles the high-level SUSPENSION,
    //     but NOT this post-lowering re-entrant CFG. A generator can be lowered to
    //     a `_poll` body carrying `StateSwitch` WITHOUT the `StateBlock*`
    //     delimiters, so predicate (1) alone misses it — hence the dedicated
    //     `has_state_machine()` check. Re-enabling drops for these is the
    //     follow-up that needs StateSwitch-aware (def-reaching) liveness.
    //
    // Idempotency: a function may be re-lifted (the native module path re-lifts
    // `ir.functions` → TIR for the inliner) and re-run through this pipeline (the
    // module-slot-promotion path re-runs `run_pipeline` on promoted functions).
    // The `lower_from_simple` round-trip preserves drop marker attrs, and the
    // DecRef/IncRef ops survive the re-lift as real ops — so re-running the
    // pass would DOUBLE-insert drops (a refcount underflow / use-after-free).
    // Skip a function whose full RC is already TIR-managed; for functions that
    // only carry the exception-region pre-bail marker, skip just that pre-bail
    // slice below and still attempt the full drop pass when the CFG permits it.
    let debug_this = std::env::var("MOLT_DEBUG_DROP")
        .map(|p| p == "ALL" || func.name.contains(&p))
        .unwrap_or(false);
    if attr_is_true(func, DROP_INSERTED_ATTR) {
        return stats;
    }
    let audit_start = std::time::Instant::now();
    emit_drop_inner_stage_audit(
        func,
        "start",
        None,
        None,
        None,
        None,
        audit_start.elapsed().as_millis(),
    );
    let exception_region_drops_already_inserted =
        attr_is_true(func, EXCEPTION_REGION_DROPS_INSERTED_ATTR);
    let exception_creation_drops = if exception_region_drops_already_inserted {
        0
    } else {
        insert_exception_creation_drops_at_raise(func)
    };
    if exception_creation_drops > 0 {
        am.invalidate_ops();
    }
    let exception_region_inserted = if exception_region_drops_already_inserted {
        ExceptionRegionDropInsertion::default()
    } else {
        insert_exception_region_match_drops(func, am)
    };
    let pre_bail_drops = exception_creation_drops + exception_region_inserted.dec_refs_added;
    if pre_bail_drops > 0 {
        stats.ops_added += pre_bail_drops;
        func.attrs.insert(
            EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
            AttrValue::Bool(true),
        );
        if exception_region_inserted.cfg_changed {
            am.invalidate_cfg();
        } else {
            am.invalidate_ops();
        }
    }
    if func.has_state_machine() {
        // STRIP `DelBoundary` boundary markers before bailing: this pass is
        // the only consumer on drop-activated targets, and a bailed function
        // inserts no drops at all (its temporaries already leak — the
        // pre-existing handler-function class), so the boundary has nothing
        // to bind to. Leaving it would hit backend lowerings that have no
        // arm for it.
        let mut stripped = 0usize;
        for block in func.blocks.values_mut() {
            let before = block.ops.len();
            block.ops.retain(|op| op.opcode != OpCode::DelBoundary);
            stripped += before - block.ops.len();
        }
        stats.ops_removed += stripped;
        if debug_this {
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("drop/{}.txt", func.name),
                format!(
                    "[DROP] {} BAILED: exc_handlers={} state_machine={} exception_region_match_drops={} del_boundaries_stripped={}\n",
                    func.name,
                    func.has_exception_handlers(),
                    func.has_state_machine(),
                    exception_region_inserted.dec_refs_added,
                    stripped,
                ),
            );
        }
        return stats;
    }
    emit_drop_inner_stage_audit(
        func,
        "after-pre-bail-slice",
        None,
        None,
        Some(pre_bail_drops),
        None,
        audit_start.elapsed().as_millis(),
    );

    let live: TirLivenessResult = am.get::<TirLiveness>(func).clone();
    emit_drop_inner_stage_audit(
        func,
        "after-liveness",
        None,
        None,
        Some(live.raw_scalars.len()),
        live.live_in.len().checked_add(live.live_out.len()),
        audit_start.elapsed().as_millis(),
    );

    // Alias-root canonicalization (design 20 §1.2) and root-only ownership facts
    // are stable across the DelBoundary normalization below: DelBoundary carries
    // no results and therefore cannot change alias roots or result-validity /
    // non-owning-copy root facts. Statement-boundary facts are computed later on
    // the normalized op stream so their op indices remain exact.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(func);
    let ownership_root_facts =
        super::ownership_lattice_min::OwnershipRootFacts::compute(func, &aliases);
    let drop_eligibility = super::ownership_lattice_min::DropEligibility::new(
        &aliases,
        &ownership_root_facts,
        &live.raw_scalars,
    );
    let canon = |v: ValueId| -> ValueId { drop_eligibility.root(v) };

    emit_drop_inner_stage_audit(
        func,
        "after-value-classification",
        None,
        None,
        Some(ownership_root_facts.non_owning_copy_result_roots().len()),
        None,
        audit_start.elapsed().as_millis(),
    );

    // Alias-root canonicalization (design 20 §1.2 — `Copy`/`TypeGuard` are
    // borrowed aliases, holding NO new reference). Ownership — and therefore the
    // drop obligation — is per alias ROOT, not per SSA value. The drop pass
    // operates entirely in root space: every value reference is canonicalized to
    // its root, and we drop each root EXACTLY ONCE (at the last use of any chain
    // member). Dropping each `Copy` independently is a refcount underflow /
    // use-after-free (the loop-carried accumulator loads its phi via
    // `load_var`→`Copy` every iteration; a per-copy drop double-frees the live
    // accumulator). This is the SAME union-find the liveness analysis used, so the
    // live sets (in root space) line up with these canonicalized placements.
    // Interior-borrow keepalive (design 20). A value produced by a borrowing read
    // (`LoadAttr`/`Index`) may borrow into / index its SOURCE object's backing
    // store; using such a result keeps the source object live. This is the SAME
    // relation the liveness analysis consumes (so cross-block keepalive is already
    // reflected in `live.is_live_out`), applied here ALSO to the within-block
    // straight-line `last_use` scan: a source object's last in-block "touch" must
    // extend through the last use of any borrow result derived from it, or the drop
    // would land before the consumer reads the borrow. (The round-6 BLOCKER-1 UAF:
    // `Counter._handle` is a raw-int registry handle whose owning wrapper's
    // finalizer destroys the registry entry — dropping the wrapper after the
    // `get_attr` but before `molt_counter_len(handle)` made `len(Counter(...))`
    // return 0.) FAIL-CLOSED: for an owned-result load this only defers the drop a
    // few ops (harmless); for the borrow/handle case it is required for soundness.
    let borrows = crate::tir::passes::alias_analysis::build_borrow_provenance(func, &aliases);
    emit_drop_inner_stage_audit(
        func,
        "after-alias-borrow",
        None,
        None,
        None,
        None,
        audit_start.elapsed().as_millis(),
    );

    // A root is droppable iff DropEligibility says it is heap-carrying,
    // function-owned per OwnershipRootFacts, and its own alias root. The raw
    // scalar carrier set still comes from liveness/representation; the composed
    // predicate lives in the ownership module rather than this placement pass.
    // Class-3 (non-owning, unmapped) `Copy` results are their OWN alias root (the
    // union-find declines to fold them), so the `r == v` rail alone would admit
    // them; exclude the lattice-owned non-owning roots explicitly.
    // ── 0a. `del`-boundary normalization (#58) ────────────────────────────────
    // The frontend carries a function-scope `del x` as `DelBoundary(v)` so the
    // Python lifetime boundary survives optimization (it used to lower to
    // NOTHING, leaving the release at whatever SSA-last-use happened to be —
    // coincidentally early). This pass is the release authority on
    // drop-activated targets, so the boundary BECOMES the release: rewrite in
    // place to `DecRef(root)` when the root is pass-owned (droppable); delete
    // otherwise (raw carrier / param / stack / borrowed alias — CPython's
    // frame-slot decref is equally unobservable there). Rewritten roots are
    // recorded in `PythonLifetimeFacts`: §1 must never place a second drop
    // (exactly-once — the alloc's +1 now belongs to the del), and §0b must
    // never defer them (an explicit boundary beats scope exit; its operand also
    // trips §0b's gate (c) DecRef rail, so the protection is doubled).
    {
        let mut removed = 0usize;
        let mut normalized = 0usize;
        for block in func.blocks.values_mut() {
            let had = block.ops.len();
            let mut rewritten: Vec<TirOp> = Vec::with_capacity(had);
            for mut op in block.ops.drain(..) {
                if op.opcode != OpCode::DelBoundary {
                    rewritten.push(op);
                    continue;
                }
                let Some(&v) = op.operands.first() else {
                    continue;
                };
                let r = canon(v);
                // `DelBoundary` is unconditional. A conditionally-valid result
                // may be stale on one outgoing edge, so deletion is the only
                // safe normalization for that root.
                if drop_eligibility.is_droppable(r)
                    && !drop_eligibility.is_conditionally_valid_result_root(r)
                {
                    op.opcode = OpCode::DecRef;
                    op.operands = vec![r];
                    op.results.clear();
                    normalized += 1;
                    rewritten.push(op);
                }
            }
            removed += had - rewritten.len();
            block.ops = rewritten;
        }
        // Deletions must count as changes: the caller back-converts to
        // SimpleIR only for changed functions, and the stale SimpleIR would
        // still carry the boundary op.
        stats.ops_removed += removed;
        stats.values_changed += normalized;
    }

    let ownership_lattice =
        OwnershipLattice::compute_with_root_facts(func, &aliases, ownership_root_facts.clone());
    let python_lifetime_facts = PythonLifetimeFacts::compute(func, &aliases);
    let statement_release_plan = StatementReleasePlan::compute(
        &ownership_lattice,
        &python_lifetime_facts,
        &drop_eligibility,
    );
    let explicit_release_blocks: HashMap<ValueId, HashSet<BlockId>> = func
        .blocks
        .iter()
        .flat_map(|(&bid, block)| {
            block.ops.iter().flat_map(move |op| {
                explicit_release_values(op)
                    .into_iter()
                    .map(canon)
                    .map(move |root| (root, bid))
                    .collect::<Vec<_>>()
            })
        })
        .fold(HashMap::new(), |mut acc, (root, bid)| {
            acc.entry(root).or_default().insert(bid);
            acc
        });
    let boundary_release_roots =
        python_lifetime_facts.boundary_release_roots(&drop_eligibility, &ownership_lattice);
    let python_lifetime_roots: HashSet<ValueId> = boundary_release_roots
        .iter()
        .chain(explicit_release_blocks.keys())
        .copied()
        .collect();
    emit_drop_inner_stage_audit(
        func,
        "after-boundary-root-planning",
        None,
        None,
        Some(
            boundary_release_roots
                .len()
                .saturating_add(explicit_release_blocks.len()),
        ),
        None,
        audit_start.elapsed().as_millis(),
    );

    let pred_map_term = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map_term,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let def_block: HashMap<ValueId, BlockId> = {
        let mut m: HashMap<ValueId, BlockId> = HashMap::new();
        for (&bid, block) in &func.blocks {
            for arg in &block.args {
                m.insert(arg.id, bid);
            }
            for op in &block.ops {
                for &r in &op.results {
                    m.insert(r, bid);
                }
            }
        }
        m
    };
    emit_drop_inner_stage_audit(
        func,
        "after-dominators-def-block",
        None,
        None,
        Some(def_block.len()),
        Some(idoms.len()),
        audit_start.elapsed().as_millis(),
    );

    // The plan: per block, a list of (insert_after_op_index OR at-entry, value)
    // DecRef placements, plus per-block at-entry edge-dying drops, plus
    // suspension IncRefs. We collect first (read-only over `func`), then apply.
    struct BlockPlan {
        /// DecRef(v) to insert immediately AFTER op at this index (straight-line
        /// last-use). Keyed by op index → values dropped after it.
        after_op: HashMap<usize, Vec<ValueId>>,
        /// DecRef(v) to insert at the START of the block (edge-dying values that
        /// arrive live from a predecessor but die on entry here).
        at_entry: Vec<ValueId>,
        /// DecRef(v) to insert just BEFORE the terminator (loop-carried phi whose
        /// last live use is the back-edge / values live-in but dead before exit).
        before_term: Vec<ValueId>,
        /// IncRef(v) to insert immediately BEFORE the op at this index (a
        /// suspension point). Keyed by op index → values inc-ref'd before it.
        before_op: HashMap<usize, Vec<ValueId>>,
        /// IncRef(v) to insert immediately BEFORE an exception-transfer op, with
        /// an exactly paired normal-fallthrough DecRef in `after_exception_op`.
        /// This models a borrowed value passed as an exception-transfer edge
        /// payload into an owned handler block arg: on the exceptional path the
        /// handler arg owns the retained +1; on the normal path the retain is
        /// released immediately after the transfer op. Unlike `before_op`,
        /// duplicate entries are load-bearing and are not deduplicated during
        /// insertion.
        before_exception_op: HashMap<usize, Vec<ValueId>>,
        /// Normal-fallthrough release for `before_exception_op` retains.
        after_exception_op: HashMap<usize, Vec<ValueId>>,
        /// IncRef(v) to insert just BEFORE the terminator (the mixed-ownership-phi
        /// retain, design §ownership / §5): a BORROWED value `v` this block passes
        /// as a branch arg into a successor's OWNED block-arg (phi) must be retained
        /// on the edge so the phi is uniformly owned and the downstream drop
        /// releases a real `+1` rather than the caller's borrow. Placed before the
        /// terminator only when this block reaches the successor via a single,
        /// unambiguous arc (the common preheader / if-arm shape); the ambiguous
        /// multi-arc-same-target case is handled by an edge split instead.
        before_term_incref: Vec<ValueId>,
    }
    let mut plans: HashMap<BlockId, BlockPlan> = HashMap::new();
    let planned_insertion_count = |plans: &HashMap<BlockId, BlockPlan>| -> usize {
        plans
            .values()
            .map(|plan| {
                plan.after_op.values().map(Vec::len).sum::<usize>()
                    + plan.at_entry.len()
                    + plan.before_term.len()
                    + plan.before_op.values().map(Vec::len).sum::<usize>()
                    + plan
                        .before_exception_op
                        .values()
                        .map(Vec::len)
                        .sum::<usize>()
                    + plan
                        .after_exception_op
                        .values()
                        .map(Vec::len)
                        .sum::<usize>()
                    + plan.before_term_incref.len()
            })
            .sum()
    };

    // Predecessor map (terminator-only edges) for edge-dying placement.
    let pred_map = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );

    let block_ids: Vec<BlockId> = {
        let mut v: Vec<BlockId> = func.blocks.keys().copied().collect();
        v.sort_unstable_by_key(|b| b.0);
        v
    };
    let reachable = crate::tir::dominators::reachable_blocks_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    // Critical-edge splits to materialize.  One split block is the edge-local RC
    // authority for a concrete outgoing terminator arc: it may hold IncRefs for
    // borrowed values entering owned phis and/or DecRefs for path-specific Python
    // lifetime releases.  Collected here, applied after the op rebuild so block-id
    // allocation does not disturb in-place op insertion.
    let mut edge_splits: Vec<EdgeSplit> = Vec::new();

    // Per-edge: the roots live INTO the successor's body (so we can test clean-
    // transfer condition (c) without re-deriving liveness).
    //
    // A root is "live into a successor body" iff some live-in value of a successor
    // `S` aliases it AND that value is not one of `S`'s own block args (block args
    // are killed at `S`'s entry — they are the phi we may be feeding, not a body
    // use). This is the precise "consumed elsewhere than via a phi we feed" test.
    let edge_body_live_roots: HashMap<(BlockId, ArcDescriptor), HashSet<ValueId>> = {
        let mut m: HashMap<(BlockId, ArcDescriptor), HashSet<ValueId>> = HashMap::new();
        for &bid in &block_ids {
            if !reachable.contains(&bid) {
                continue;
            }
            let block = &func.blocks[&bid];
            for arc in terminator_arcs(&block.terminator) {
                if !reachable.contains(&arc.target) {
                    continue;
                }
                let mut roots: HashSet<ValueId> = HashSet::new();
                let succ_args: HashSet<ValueId> = func
                    .blocks
                    .get(&arc.target)
                    .map(|s| s.args.iter().map(|a| a.id).collect())
                    .unwrap_or_default();
                if let Some(set) = live.live_in.get(&arc.target) {
                    for &m in set {
                        if !succ_args.contains(&m) {
                            roots.insert(canon(m));
                        }
                    }
                }
                m.insert((bid, arc.descriptor), roots);
            }
        }
        m
    };
    emit_drop_inner_stage_audit(
        func,
        "after-edge-body-live-roots",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(edge_body_live_roots.values().map(HashSet::len).sum()),
        Some(edge_body_live_roots.len()),
        audit_start.elapsed().as_millis(),
    );

    // A branch argument can transfer an owned root into a successor block arg
    // (phi). The immediate successor-entry drop is already guarded by
    // `incoming_arg_roots` in §3. The same transfer remains active through
    // descendant blocks while the phi is live there: the source root is no longer
    // the release authority, and dropping it on a later die-edge would double-free
    // the object when the phi itself is released.
    let block_mentions_value = |bid: BlockId, value: ValueId| -> bool {
        func.blocks.get(&bid).is_some_and(|block| {
            block.ops.iter().any(|op| op.operands.contains(&value))
                || terminator_mentions_value(&block.terminator, value)
        })
    };
    let transferred_phi_edges_by_root: HashMap<ValueId, Vec<(ValueId, BlockId)>> = {
        let mut by_root: HashMap<ValueId, Vec<(ValueId, BlockId)>> = HashMap::new();
        for &pred in &block_ids {
            if !reachable.contains(&pred) {
                continue;
            }
            let Some(pred_block) = func.blocks.get(&pred) else {
                continue;
            };
            for arc in terminator_arcs(&pred_block.terminator) {
                if !reachable.contains(&arc.target) {
                    continue;
                }
                let Some(target_block) = func.blocks.get(&arc.target) else {
                    continue;
                };
                for (pos, &arg) in arc.args.iter().enumerate() {
                    if live.is_raw_scalar(arg)
                        || drop_eligibility.is_conditionally_valid_result_root(arg)
                    {
                        continue;
                    }
                    let root = canon(arg);
                    let Some(phi) = target_block.args.get(pos) else {
                        continue;
                    };
                    if root == phi.id
                        || ownership_lattice.is_conditionally_valid_result_root(root)
                        || !drop_eligibility.is_droppable(root)
                    {
                        continue;
                    }
                    by_root.entry(root).or_default().push((phi.id, arc.target));
                }
            }
        }
        by_root
    };
    emit_drop_inner_stage_audit(
        func,
        "after-transferred-phi-edges",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(transferred_phi_edges_by_root.values().map(Vec::len).sum()),
        Some(transferred_phi_edges_by_root.len()),
        audit_start.elapsed().as_millis(),
    );
    let transferred_phi_args_by_root: HashMap<ValueId, HashSet<ValueId>> =
        transferred_phi_edges_by_root
            .iter()
            .map(|(&root, transfers)| {
                (
                    root,
                    transfers
                        .iter()
                        .map(|(phi, _)| *phi)
                        .collect::<HashSet<_>>(),
                )
            })
            .collect();
    let transferred_phi_live_blocks_by_root: HashMap<ValueId, HashSet<BlockId>> = {
        let mut by_root: HashMap<ValueId, HashSet<BlockId>> = HashMap::new();
        for (&root, transfers) in &transferred_phi_edges_by_root {
            for &(phi, transfer_target) in transfers {
                let mut after_transfer: HashSet<BlockId> = HashSet::new();
                let mut forward_stack = vec![transfer_target];
                while let Some(cur) = forward_stack.pop() {
                    if !reachable.contains(&cur) || !after_transfer.insert(cur) {
                        continue;
                    }
                    let Some(block) = func.blocks.get(&cur) else {
                        continue;
                    };
                    for arc in terminator_arcs(&block.terminator) {
                        forward_stack.push(arc.target);
                    }
                }

                let mut reaches_phi_mention: HashSet<BlockId> = HashSet::new();
                let mut reverse_stack: Vec<BlockId> = after_transfer
                    .iter()
                    .copied()
                    .filter(|&bid| block_mentions_value(bid, phi))
                    .collect();
                while let Some(cur) = reverse_stack.pop() {
                    if !after_transfer.contains(&cur) || !reaches_phi_mention.insert(cur) {
                        continue;
                    }
                    if let Some(preds) = pred_map.get(&cur) {
                        reverse_stack.extend(preds.iter().copied());
                    }
                }
                by_root.entry(root).or_default().extend(reaches_phi_mention);
            }
        }
        by_root
    };
    emit_drop_inner_stage_audit(
        func,
        "after-transferred-phi-live-blocks",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(
            transferred_phi_live_blocks_by_root
                .values()
                .map(HashSet::len)
                .sum(),
        ),
        Some(transferred_phi_live_blocks_by_root.len()),
        audit_start.elapsed().as_millis(),
    );
    // Python local cleanup roots are born at `store_var` source roots, but the
    // actual owner can move through block args as control flow joins. Track that
    // origin transitive closure so a cleanup `DecRef(phi)` is recognized as the
    // release authority for the original store-var source root.
    let python_origin_roots_by_carrier_root: HashMap<ValueId, HashSet<ValueId>> = {
        let mut origins: HashMap<ValueId, HashSet<ValueId>> = HashMap::new();
        for &root in &python_lifetime_roots {
            origins.entry(root).or_default().insert(root);
        }

        let mut changed = true;
        while changed {
            changed = false;
            for &pred in &block_ids {
                if !reachable.contains(&pred) {
                    continue;
                }
                let Some(pred_block) = func.blocks.get(&pred) else {
                    continue;
                };
                for arc in terminator_arcs(&pred_block.terminator) {
                    if !reachable.contains(&arc.target) {
                        continue;
                    }
                    let Some(target_block) = func.blocks.get(&arc.target) else {
                        continue;
                    };
                    let mut transferred_current_roots: HashSet<ValueId> = HashSet::new();
                    for (pos, &arg) in arc.args.iter().enumerate() {
                        if live.is_raw_scalar(arg)
                            || drop_eligibility.is_conditionally_valid_result_root(arg)
                        {
                            continue;
                        }
                        let current_root = canon(arg);
                        let Some(source_roots) = origins.get(&current_root).cloned() else {
                            continue;
                        };
                        let Some(phi) = target_block.args.get(pos) else {
                            continue;
                        };
                        if !drop_eligibility.is_droppable(phi.id) {
                            continue;
                        }
                        if edge_body_live_roots
                            .get(&(pred, arc.descriptor))
                            .is_some_and(|roots| roots.contains(&current_root))
                        {
                            continue;
                        }
                        if !transferred_current_roots.insert(current_root) {
                            continue;
                        }
                        let entry = origins.entry(phi.id).or_default();
                        for source_root in source_roots {
                            if entry.insert(source_root) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        origins
    };
    emit_drop_inner_stage_audit(
        func,
        "after-python-origin-roots",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(
            python_origin_roots_by_carrier_root
                .values()
                .map(HashSet::len)
                .sum(),
        ),
        Some(python_origin_roots_by_carrier_root.len()),
        audit_start.elapsed().as_millis(),
    );

    // A source Python root can stop being the current cleanup carrier after it
    // clean-transfers through block args. Project the direct phi-live map back
    // onto each source origin so return-boundary planning does not release the
    // stale source root when a live carrier will be released downstream.
    let python_origin_transferred_phi_live_blocks_by_root: HashMap<ValueId, HashSet<BlockId>> = {
        let mut by_root = transferred_phi_live_blocks_by_root.clone();
        for (&carrier_root, source_roots) in &python_origin_roots_by_carrier_root {
            let Some(blocks) = transferred_phi_live_blocks_by_root.get(&carrier_root) else {
                continue;
            };
            for &source_root in source_roots {
                if !python_lifetime_roots.contains(&source_root) {
                    continue;
                }
                by_root
                    .entry(source_root)
                    .or_default()
                    .extend(blocks.iter().copied());
            }
        }
        by_root
    };
    emit_drop_inner_stage_audit(
        func,
        "after-python-origin-transferred-phi-live-blocks",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(
            python_origin_transferred_phi_live_blocks_by_root
                .values()
                .map(HashSet::len)
                .sum(),
        ),
        Some(python_origin_transferred_phi_live_blocks_by_root.len()),
        audit_start.elapsed().as_millis(),
    );
    let transferred_phi_live_on_block = |root: ValueId, bid: BlockId| -> bool {
        python_origin_transferred_phi_live_blocks_by_root
            .get(&root)
            .is_some_and(|blocks| blocks.contains(&bid))
    };

    let python_origin_release_blocks: HashMap<ValueId, HashSet<BlockId>> = {
        let mut by_root = explicit_release_blocks.clone();
        for (&bid, block) in &func.blocks {
            for op in &block.ops {
                for value in explicit_release_values(op) {
                    let release_root = canon(value);
                    if let Some(source_roots) =
                        python_origin_roots_by_carrier_root.get(&release_root)
                    {
                        for &source_root in source_roots {
                            if python_lifetime_roots.contains(&source_root) {
                                by_root.entry(source_root).or_default().insert(bid);
                            }
                        }
                    }
                }
            }
        }
        by_root
    };

    let boundary_roots_handled_before_return: HashMap<BlockId, HashSet<ValueId>> = {
        let mut handled: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
        let mut transferred_value_roots: HashMap<ValueId, HashSet<ValueId>> = HashMap::new();
        let mut incoming: HashMap<
            (BlockId, ValueId),
            Vec<(BlockId, ArcDescriptor, Vec<ValueId>, bool)>,
        > = HashMap::new();
        let mut boundary_roots: Vec<ValueId> = boundary_release_roots.iter().copied().collect();
        boundary_roots.sort_unstable_by_key(|v| v.0);
        for &pred in &block_ids {
            if !reachable.contains(&pred) {
                continue;
            }
            let Some(pred_block) = func.blocks.get(&pred) else {
                continue;
            };
            for arc in terminator_arcs(&pred_block.terminator) {
                if !reachable.contains(&arc.target) {
                    continue;
                }
                let Some(target_block) = func.blocks.get(&arc.target) else {
                    continue;
                };
                let mut transferred_on_arc: HashSet<ValueId> = HashSet::new();
                for (pos, &arg) in arc.args.iter().enumerate() {
                    if live.is_raw_scalar(arg)
                        || drop_eligibility.is_conditionally_valid_result_root(arg)
                    {
                        continue;
                    }
                    let current_root = canon(arg);
                    let Some(source_roots) = python_origin_roots_by_carrier_root.get(&current_root)
                    else {
                        continue;
                    };
                    let body_live = edge_body_live_roots
                        .get(&(pred, arc.descriptor))
                        .is_some_and(|s| s.contains(&current_root));
                    if body_live || !transferred_on_arc.insert(current_root) {
                        continue;
                    }
                    let Some(phi) = target_block.args.get(pos) else {
                        continue;
                    };
                    if drop_eligibility.is_droppable(phi.id) {
                        transferred_value_roots.entry(phi.id).or_default().extend(
                            source_roots
                                .iter()
                                .copied()
                                .filter(|root| python_lifetime_roots.contains(root)),
                        );
                    }
                }
                if !matches!(target_block.terminator, Terminator::Return { .. }) {
                    continue;
                }
                for &root in &boundary_roots {
                    if ownership_lattice.is_conditionally_valid_result_root(root)
                        || terminator_uses_root(&target_block.terminator, root, &canon)
                    {
                        continue;
                    }
                    match def_block.get(&root) {
                        Some(&dblk)
                            if crate::tir::dominators::dominates(dblk, arc.target, &idoms) => {}
                        _ => continue,
                    }
                    let body_live = edge_body_live_roots
                        .get(&(pred, arc.descriptor))
                        .is_some_and(|s| s.contains(&root));
                    if transferred_phi_live_on_block(root, pred) {
                        handled.entry(arc.target).or_default().insert(root);
                        continue;
                    }
                    let transfers_root = !body_live
                        && arc.args.iter().enumerate().any(|(pos, &arg)| {
                            if live.is_raw_scalar(arg)
                                || drop_eligibility.is_conditionally_valid_result_root(arg)
                            {
                                return false;
                            }
                            let current_root = canon(arg);
                            if !python_origin_roots_by_carrier_root
                                .get(&current_root)
                                .is_some_and(|roots| roots.contains(&root))
                            {
                                return false;
                            }
                            target_block
                                .args
                                .get(pos)
                                .is_some_and(|phi| drop_eligibility.is_droppable(phi.id))
                        });
                    incoming.entry((arc.target, root)).or_default().push((
                        pred,
                        arc.descriptor,
                        arc.args.clone(),
                        transfers_root,
                    ));
                }
            }
        }
        for ((target, root), arcs) in incoming {
            let any_transferred = arcs.iter().any(|(_, _, _, transferred)| *transferred);
            if !any_transferred {
                continue;
            }
            handled.entry(target).or_default().insert(root);
            if arcs.iter().all(|(_, _, _, transferred)| *transferred) {
                continue;
            }
            for (pred, arc, args, transferred) in arcs {
                if transferred {
                    continue;
                }
                push_edge_split(
                    &mut edge_splits,
                    pred,
                    arc,
                    target,
                    args,
                    vec![],
                    vec![root],
                );
            }
        }
        let mut explicit_roots: Vec<ValueId> = python_lifetime_roots
            .iter()
            .copied()
            .filter(|root| explicit_release_blocks.contains_key(root))
            .collect();
        explicit_roots.sort_unstable_by_key(|root| root.0);
        let mut explicit_released_entry_roots: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
        let explicit_release_dominates_block = |root: ValueId, block: BlockId| -> bool {
            python_origin_release_blocks
                .get(&root)
                .is_some_and(|blocks| {
                    blocks.iter().any(|&release_block| {
                        release_block == block
                            || crate::tir::dominators::dominates(release_block, block, &idoms)
                    })
                })
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &root in &explicit_roots {
                let Some(&root_def) = def_block.get(&root) else {
                    continue;
                };
                let mut incoming_by_target: BTreeMap<
                    BlockId,
                    Vec<(BlockId, ArcDescriptor, Vec<ValueId>, bool)>,
                > = BTreeMap::new();
                for &pred in &block_ids {
                    if !reachable.contains(&pred) {
                        continue;
                    }
                    if !crate::tir::dominators::dominates(root_def, pred, &idoms) {
                        continue;
                    }
                    let released_before_edge = explicit_released_entry_roots
                        .get(&pred)
                        .is_some_and(|roots| roots.contains(&root))
                        || explicit_release_dominates_block(root, pred);
                    let Some(pred_block) = func.blocks.get(&pred) else {
                        continue;
                    };
                    for arc in terminator_arcs(&pred_block.terminator) {
                        if !reachable.contains(&arc.target) {
                            continue;
                        }
                        if explicit_released_entry_roots
                            .get(&arc.target)
                            .is_some_and(|roots| roots.contains(&root))
                        {
                            continue;
                        }
                        let Some(target_block) = func.blocks.get(&arc.target) else {
                            continue;
                        };
                        if matches!(target_block.terminator, Terminator::Return { .. }) {
                            continue;
                        }
                        if edge_body_live_roots
                            .get(&(pred, arc.descriptor))
                            .is_some_and(|roots| roots.contains(&root))
                        {
                            continue;
                        }
                        if transferred_phi_live_on_block(root, pred) {
                            continue;
                        }
                        let transfers_root = arc.args.iter().enumerate().any(|(pos, &arg)| {
                            if live.is_raw_scalar(arg)
                                || drop_eligibility.is_conditionally_valid_result_root(arg)
                            {
                                return false;
                            }
                            let current_root = canon(arg);
                            python_origin_roots_by_carrier_root
                                .get(&current_root)
                                .is_some_and(|roots| roots.contains(&root))
                                && target_block
                                    .args
                                    .get(pos)
                                    .is_some_and(|phi| drop_eligibility.is_droppable(phi.id))
                        });
                        if transfers_root {
                            continue;
                        }
                        incoming_by_target.entry(arc.target).or_default().push((
                            pred,
                            arc.descriptor,
                            arc.args,
                            released_before_edge,
                        ));
                    }
                }
                for (target, arcs) in incoming_by_target {
                    let any_released = arcs.iter().any(|(_, _, _, released)| *released);
                    let any_unreleased = arcs.iter().any(|(_, _, _, released)| !*released);
                    if !(any_released && any_unreleased) {
                        continue;
                    }
                    for (pred, arc, args, released) in arcs {
                        if released {
                            continue;
                        }
                        push_edge_split(
                            &mut edge_splits,
                            pred,
                            arc,
                            target,
                            args,
                            vec![],
                            vec![root],
                        );
                    }
                    explicit_released_entry_roots
                        .entry(target)
                        .or_default()
                        .insert(root);
                    handled.entry(target).or_default().insert(root);
                    changed = true;
                }
            }
        }
        for root in explicit_roots {
            let Some(&root_def) = def_block.get(&root) else {
                continue;
            };
            for &pred in &block_ids {
                if !reachable.contains(&pred) {
                    continue;
                }
                if !crate::tir::dominators::dominates(root_def, pred, &idoms) {
                    continue;
                }
                let released_before_edge = explicit_released_entry_roots
                    .get(&pred)
                    .is_some_and(|roots| roots.contains(&root))
                    || explicit_release_dominates_block(root, pred);
                if released_before_edge {
                    continue;
                }
                let Some(pred_block) = func.blocks.get(&pred) else {
                    continue;
                };
                for arc in terminator_arcs(&pred_block.terminator) {
                    if !reachable.contains(&arc.target) {
                        continue;
                    }
                    let Some(target_block) = func.blocks.get(&arc.target) else {
                        continue;
                    };
                    if !matches!(target_block.terminator, Terminator::Return { .. }) {
                        continue;
                    }
                    if terminator_uses_root(&target_block.terminator, root, &canon)
                        || edge_body_live_roots
                            .get(&(pred, arc.descriptor))
                            .is_some_and(|roots| roots.contains(&root))
                    {
                        continue;
                    }
                    if transferred_phi_live_on_block(root, pred) {
                        handled.entry(arc.target).or_default().insert(root);
                        continue;
                    }
                    let transfers_root = arc.args.iter().enumerate().any(|(pos, &arg)| {
                        if live.is_raw_scalar(arg)
                            || drop_eligibility.is_conditionally_valid_result_root(arg)
                        {
                            return false;
                        }
                        let current_root = canon(arg);
                        python_origin_roots_by_carrier_root
                            .get(&current_root)
                            .is_some_and(|roots| roots.contains(&root))
                            && target_block
                                .args
                                .get(pos)
                                .is_some_and(|phi| drop_eligibility.is_droppable(phi.id))
                    });
                    if transfers_root {
                        handled.entry(arc.target).or_default().insert(root);
                        continue;
                    }
                    push_edge_split(
                        &mut edge_splits,
                        pred,
                        arc.descriptor,
                        arc.target,
                        arc.args.clone(),
                        vec![],
                        vec![root],
                    );
                    handled.entry(arc.target).or_default().insert(root);
                }
            }
        }
        for &bid in &block_ids {
            if !reachable.contains(&bid) {
                continue;
            }
            let Some(block) = func.blocks.get(&bid) else {
                continue;
            };
            if !matches!(block.terminator, Terminator::Return { .. }) {
                continue;
            }
            for op in &block.ops {
                if !matches!(op.opcode, OpCode::DecRef | OpCode::DelBoundary) {
                    continue;
                }
                let Some(&release_value) = op.operands.first() else {
                    continue;
                };
                let release_root = canon(release_value);
                let Some(roots) = transferred_value_roots
                    .get(&release_value)
                    .or_else(|| transferred_value_roots.get(&release_root))
                    .or_else(|| python_origin_roots_by_carrier_root.get(&release_root))
                else {
                    continue;
                };
                let release_def_dominates = def_block
                    .get(&release_value)
                    .is_some_and(|&dblk| crate::tir::dominators::dominates(dblk, bid, &idoms));
                if !release_def_dominates {
                    continue;
                }
                for &root in roots {
                    if terminator_uses_root(&block.terminator, root, &canon) {
                        continue;
                    }
                    match def_block.get(&root) {
                        Some(&dblk) if crate::tir::dominators::dominates(dblk, bid, &idoms) => {
                            handled.entry(bid).or_default().insert(root);
                        }
                        _ => {}
                    }
                }
            }
        }
        handled
    };
    emit_drop_inner_stage_audit(
        func,
        "after-boundary-roots-before-return",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(
            boundary_roots_handled_before_return
                .values()
                .map(HashSet::len)
                .sum(),
        ),
        Some(boundary_roots_handled_before_return.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── 0b. FinalizerSensitive release deferral (#58, the ordering keystone) ──
    // CPython releases a named local at its Python lifetime boundary (`del` /
    // rebinding / scope exit), not at its last read. For a value whose release
    // (transitively) fires a `__del__`, that timing is OBSERVABLE — releasing
    // `bag = [A()]` at its SSA last-use fires `A.__del__` before the rest of
    // the function body runs (doc 50 §A, repro c_scope). The ownership lattice
    // names exactly those values (FinalizerSensitive: `defines_del` allocation
    // roots closed over absorbing container constructors). For each such root
    // this pass would otherwise release at SSA-last-use, DEFER the release to
    // the Python lifetime boundary: §1/§1b skip it and a `DecRef` lands before
    // the DEF BLOCK'S OWN `Terminator::Return` (merged into `plans` after §5,
    // sorted by ValueId so multi-instance finalizer order matches CPython's
    // creation-order frame teardown).
    //
    // SAME-BLOCK-ONLY (the rung-1 soundness frame). The deferred DecRef is
    // placed exclusively before the terminator of the block that DEFINES the
    // root, and only when that terminator is `Return`. This is provably sound
    // by straight-line construction: the only way to execute the block's
    // terminator is to fall through every op after the def, so the def always
    // precedes the DecRef on every path that reaches it — no dominance
    // analysis required. Every mid-block exception edge departs the block
    // BEFORE the terminator, so on those paths the DecRef simply does not run
    // (the value leaks on the exception path — the SAME fail-closed
    // leak-not-UAF class as §3's dominance guard; per-region exception
    // cleanup is doc 45's arc). CROSS-BLOCK placement is deliberately NOT
    // attempted at this rung: block-granularity dominance cannot see WHERE
    // inside a block an exception edge departs (the §3 trap), pred-map
    // comparisons cannot see a self-pred exception edge (terminator-pred and
    // exception-pred of the same successor dedup to one entry), and
    // unresolved universal-check targets exist only inside the codegen
    // drivers (observed: a module-chunk deferred DecRef in a shared Return
    // block aborted LLVM verification with "Instruction does not dominate
    // all uses!"). Cross-block deferral needs op-granular dominance — the
    // ownership-boundaries rung. Python function bodies overwhelmingly lower
    // to a single Return-terminated block, so the c_scope class is covered.
    //
    // FAIL-CLOSED GATES — any failure keeps the pre-#58 SSA-last-use placement
    // for that value (never a UAF, never a NEW silent-skip class):
    //   (a) droppable alias ROOT (heap-carrying, function-owned, own root);
    //   (b) defined by an op (not a block arg / phi) in a block whose
    //       terminator is `Return` (same-block-only, above);
    //   (c) no ownership-consuming or explicit-RC use anywhere: never a
    //       branch arg or terminator use, never consumed by an op (the
    //       CallArgs builder), never an operand of an IncRef/DecRef/Free
    //       already in the IR (an explicit release — a `del` boundary
    //       rewritten by §0a, or module-scope `del` — is its own authority),
    //       never an operand ABSORBED by a container constructor (CPython's
    //       BUILD_LIST consumes the stack ref, so the molt temp `+1` mirrors
    //       a ref that dies AT construction — deferring it held the element
    //       past `container.clear()`, finalizer_matrix `container_hold`),
    //       and the alias group never touches a NAMED-SLOT move
    //       (`store_var`/`load_var` Copy): a slot-backed local has
    //       `del`/rebinding boundaries the slot machinery owns. The c_scope
    //       class is PURE-SSA locals — the frontend emits no slot for them.
    //   (d) the function has no suspension points (resumable-frame ownership
    //       is its own arc; lowered state machines already bail the pass).
    let deferred: HashSet<ValueId>;
    let mut deferred_return_placements: Vec<(BlockId, ValueId)> = Vec::new();
    {
        let lattice = &ownership_lattice;
        let sensitive_roots: HashSet<ValueId> = lattice
            .finalizer_sensitive_roots()
            .iter()
            .copied()
            .filter(|&r| drop_eligibility.is_droppable(r))
            .collect();
        let has_suspension = !sensitive_roots.is_empty()
            && func
                .blocks
                .values()
                .any(|b| b.ops.iter().any(|o| is_suspension_point(o.opcode)));
        let mut accepted: HashSet<ValueId> = HashSet::new();
        if !sensitive_roots.is_empty() && !has_suspension {
            // Gate (c): one scan over the whole function for disqualifying uses.
            // Gate (b') NAMED-LOCAL proof, collected in the same scan: only a
            // value the frontend stamped `bound_local` (its result is bound to
            // a plain function-local NAME) carries CPython's frame-teardown
            // boundary. An UNNAMED expression temp (`bag.append(A())`'s
            // argument) dies at its statement exactly like CPython's consumed
            // stack ref — deferring it held elements past `container.clear()`
            // (finalizer_container_clear regression). The name-binding fact is
            // otherwise ERASED by lowering — this is the named-local rung of
            // the council lattice arriving as a carried fact, not an
            // inference from use-shape.
            let mut disqualified: HashSet<ValueId> = HashSet::new();
            for &bid in &block_ids {
                if !reachable.contains(&bid) {
                    continue;
                }
                let block = &func.blocks[&bid];
                for v in terminator_branch_args(&block.terminator) {
                    disqualified.insert(canon(v));
                }
                for &r in &sensitive_roots {
                    if terminator_uses_root(&block.terminator, r, &canon) {
                        disqualified.insert(r);
                    }
                }
                for op in &block.ops {
                    if matches!(op.opcode, OpCode::IncRef | OpCode::DecRef | OpCode::Free) {
                        for &operand in &op.operands {
                            disqualified.insert(canon(operand));
                        }
                    }
                    if let Some(r) = op_consumed_operand_root(op, &canon) {
                        disqualified.insert(r);
                    }
                    // Gate (c) transfer rail: an operand ABSORBED by a
                    // container constructor keeps its SSA-last-use release —
                    // the CONTAINER value carries the Python scope boundary.
                    if op_result_absorbs_operand_ownership(op) {
                        for &operand in &op.operands {
                            disqualified.insert(canon(operand));
                        }
                    }
                }
            }
            for &r in &sensitive_roots {
                if disqualified.contains(&r) {
                    continue;
                }
                // Gate (b'/c): PythonLifetimeFacts owns whether this root is a
                // bound-local deferral instead of a slot-backed local with its
                // own del/rebinding release boundary.
                if !python_lifetime_facts.is_return_boundary_deferred_root(r, &drop_eligibility) {
                    continue;
                }
                let Some(&dblk) = def_block.get(&r) else {
                    continue;
                };
                // Gate (b): an op-defined root (not a phi) whose own block
                // ends in `Return`.
                if func.blocks[&dblk].args.iter().any(|a| a.id == r) {
                    continue;
                }
                if !reachable.contains(&dblk) {
                    continue;
                }
                if !matches!(func.blocks[&dblk].terminator, Terminator::Return { .. }) {
                    continue;
                }
                accepted.insert(r);
                deferred_return_placements.push((dblk, r));
            }
        }
        deferred = accepted;
    }
    // Deterministic finalizer order: ValueId ascending == creation order ==
    // CPython's observed frame-teardown DEL order (finalizer_matrix `many`).
    deferred_return_placements.sort_unstable_by_key(|&(b, v)| (b.0, v.0));
    emit_drop_inner_stage_audit(
        func,
        "after-finalizer-deferral",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(deferred.len()),
        Some(deferred_return_placements.len()),
        audit_start.elapsed().as_millis(),
    );

    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        let mut plan = BlockPlan {
            after_op: HashMap::new(),
            at_entry: Vec::new(),
            before_term: Vec::new(),
            before_op: HashMap::new(),
            before_exception_op: HashMap::new(),
            after_exception_op: HashMap::new(),
            before_term_incref: Vec::new(),
        };

        // ── 1. Straight-line last-use drops (alias-root space) ───────────────
        // For every alias ROOT used by an op in this block, find the LAST op
        // index where any chain member is used as an operand. If the root is
        // droppable AND not live-out of this block AND not transferred by a
        // branch arg / terminator use (which pass ownership), drop the ROOT after
        // its last op-use. Canonicalizing collapses a `Copy`-chain into one
        // entity → one drop per owned object (no double-free across copies).
        //
        // Branch args / terminator direct uses are canonicalized to roots: a
        // copied value passed on an edge transfers the ROOT's ownership.
        let branch_arg_roots: HashSet<ValueId> = terminator_branch_args(&block.terminator)
            .into_iter()
            .map(canon)
            .collect();
        // `DeleteVar(missing, old)` is the executable Python `del name` /
        // slot-overwrite boundary. The op stores the missing sentinel into the
        // slot; the old occupant's slot-owned reference must release
        // immediately after that store, not at later SSA last-use and not by a
        // hidden runtime side effect.
        let mut delete_var_release_after_op: HashMap<usize, Vec<ValueId>> = HashMap::new();
        for (idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::DeleteVar {
                continue;
            }
            if let Some(&old_slot_value) = op.operands.get(1) {
                let root = canon(old_slot_value);
                if drop_eligibility.is_droppable(root) {
                    delete_var_release_after_op
                        .entry(idx)
                        .or_default()
                        .push(root);
                }
            }
        }
        // Last op-use index per ROOT (max over all aliases). A use of operand `v`
        // at index `idx` is a last-use candidate for `canon(v)` AND for every
        // source-object root `v` borrows from (interior-borrow keepalive): the
        // source must stay live through the borrow result's last use.
        let mut last_use: HashMap<ValueId, usize> = HashMap::new();
        let record_use = |root: ValueId, idx: usize, lu: &mut HashMap<ValueId, usize>| {
            lu.entry(root)
                .and_modify(|e| {
                    if idx > *e {
                        *e = idx;
                    }
                })
                .or_insert(idx);
        };
        for (idx, op) in block.ops.iter().enumerate() {
            for &operand in &op.operands {
                record_use(canon(operand), idx, &mut last_use);
                if !borrows.is_empty() {
                    for src_root in borrows.keepalive_roots(operand, &canon) {
                        record_use(src_root, idx, &mut last_use);
                    }
                }
            }
        }
        for (&v, &idx) in &last_use {
            // `v` is already a root (last_use is keyed by canon'd operands).
            if !drop_eligibility.is_droppable(v) {
                continue;
            }
            // §0b finalizer-ordering deferral: this root's release lands at the
            // Return boundary, not its SSA last-use.
            if deferred.contains(&v) {
                continue;
            }
            // §0a `del`-boundary: the rewritten DecRef IS this root's release —
            // a trailing last-use drop here would be the double-free.
            if python_lifetime_facts.has_explicit_release_boundary(v) {
                continue;
            }
            // Transferred via branch arg (root space) → no drop (successor owns).
            if branch_arg_roots.contains(&v) {
                continue;
            }
            // Live-out of this block → dropped later; not here.
            if live.is_live_out(bid, v) {
                continue;
            }
            if statement_release_plan.contains_released_root(v) {
                continue;
            }
            // Releasing a Python-bound finalizer-sensitive root can execute
            // Python `__del__`. Unless an explicit DecRef already marks the
            // Python `del` boundary, hold named-local roots until the dominated
            // return boundary rather than firing at SSA last read. Unbound
            // expression temporaries are intentionally not in
            // `boundary_release_roots`; they keep last-use placement.
            if boundary_release_roots.contains(&v) {
                continue;
            }
            // Consumed by the terminator (Return value / cond) — canonicalize the
            // terminator's direct uses to roots and skip if `v` is among them.
            if terminator_uses_root(&block.terminator, v, &canon) {
                continue;
            }
            // Consumed AS AN OPERAND by its last-use op (design §1.2
            // takes-ownership): a CallArgs builder handed to `call_bind` /
            // `call_indirect` is freed inside the call (PtrDropGuard). Ownership
            // transferred to the op exactly like a Return value — no trailing
            // DecRef, or we double-free the `TYPE_ID_CALLARGS` object.
            if op_consumed_operand_root(&block.ops[idx], &canon) == Some(v) {
                continue;
            }
            // The owned object dies after op `idx` in this block: drop the root
            // after it.
            plan.after_op.entry(idx).or_default().push(v);
        }

        // ── 1b. Dead-result drops (defined-but-never-used owned values) ──────
        // The §1 scan keys drops on `last_use`, which is built EXCLUSIVELY from
        // values that appear as an OPERAND somewhere. An owned result that is
        // produced but NEVER consumed (zero uses — neither as an operand, nor a
        // branch arg, nor a terminator use) is therefore ABSENT from `last_use`
        // and would leak: its `+1` is never released, so for a `TYPE_ID_OBJECT`
        // with a `__del__` the finalizer NEVER runs (CPython runs it at the last
        // reference drop). The canonical example is a discarded constructor whose
        // local is dead or `del`'d: `def f(): x = Demo(); del x` lowers to a
        // `call_bind` whose owned result has no further use. The edge-dying rule
        // (§3) cannot catch it either — that rule requires the value to be
        // live-out of a predecessor, but a zero-use value is dead immediately.
        //
        // For a value with no uses, the LAST program point at which it is live is
        // immediately AFTER its defining op, so that is where its drop belongs.
        // We apply the SAME guards as the §1 last-use path (droppable / not
        // branch-transferred / not live-out / not terminator-consumed) plus the
        // conditionally-owned-iterator exclusion (§2.8): the value result of an
        // `IterNextUnboxed` is a non-owned `None` sentinel on the exhaustion path
        // and must never be dropped unconditionally. A result that IS used was
        // already handled by §1 (its root is in `last_use`); checking `last_use`
        // membership in ROOT space avoids any double-drop.
        for (idx, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                let r = canon(result);
                // Only the value's own root carries the ownership obligation; an
                // aliased result (`r != result`) is released through its root.
                if r != result {
                    continue;
                }
                // Already released by the §1 last-use path (some op used it).
                if last_use.contains_key(&r) {
                    continue;
                }
                // §0b finalizer-ordering deferral: released at the Return
                // boundary instead (the c_scope zero-use container shape).
                if deferred.contains(&r) {
                    continue;
                }
                if !drop_eligibility.is_droppable(r) {
                    continue;
                }
                // Conditionally-valid iterator value result: never drop it (it is
                // stale garbage on the iterator-exhaustion path).
                if drop_eligibility.is_conditionally_valid_result_root(result) {
                    continue;
                }
                // Transferred via branch arg (root space) → successor owns it.
                if branch_arg_roots.contains(&r) {
                    continue;
                }
                // Live-out of this block → dropped later, not here.
                if live.is_live_out(bid, r) {
                    continue;
                }
                if statement_release_plan.contains_released_root(r) {
                    continue;
                }
                // Zero-use Python-bound finalizer-sensitive roots are still
                // locals for finalizer ordering: drop them at the frame
                // boundary, not immediately after construction. Unbound
                // expression temporaries are not Python-bound and die here.
                if boundary_release_roots.contains(&r) {
                    continue;
                }
                // Consumed by the terminator (Return value / cond).
                if terminator_uses_root(&block.terminator, r, &canon) {
                    continue;
                }
                // The owned object is dead the instant it is produced: drop it
                // immediately after its defining op.
                plan.after_op.entry(idx).or_default().push(r);
            }
        }

        if matches!(block.terminator, Terminator::Return { .. })
            && !boundary_release_roots.is_empty()
        {
            let mut roots: Vec<ValueId> = boundary_release_roots.iter().copied().collect();
            roots.sort_unstable_by_key(|v| v.0);
            for root in roots {
                if ownership_lattice.is_conditionally_valid_result_root(root) {
                    continue;
                }
                if terminator_uses_root(&block.terminator, root, &canon) {
                    continue;
                }
                if boundary_roots_handled_before_return
                    .get(&bid)
                    .is_some_and(|roots| roots.contains(&root))
                {
                    continue;
                }
                match def_block.get(&root) {
                    Some(&dblk) if crate::tir::dominators::dominates(dblk, bid, &idoms) => {
                        plan.before_term.push(root);
                    }
                    _ => {}
                }
            }
        }

        if let Some(by_op) = statement_release_plan.after_op().get(&bid) {
            for (&idx, roots) in by_op {
                plan.after_op
                    .entry(idx)
                    .or_default()
                    .extend(roots.iter().copied());
            }
        }
        for (idx, roots) in delete_var_release_after_op {
            plan.after_op.entry(idx).or_default().extend(roots);
        }

        // ── 2. Suspension-point IncRef ───────────────────────────────────────
        // For each yield op at index `i`, every heap-carrying value that is
        // (a) DEFINED before the yield (an op result at index < i, or a block
        // arg), AND (b) live ACROSS the yield (live-out of the block — used after
        // a resume) gets an IncRef immediately before the yield so the suspended
        // frame owns its own reference.
        //
        // Requirement (a) is soundness-critical: a value defined AFTER the yield
        // is not yet in scope at the yield, so referencing it in an IncRef placed
        // before the yield would be a use-before-def (a TIR verify failure).
        // Build the set of values defined at or before each op position.
        if block.ops.iter().any(|o| is_suspension_point(o.opcode)) {
            // `live_out` is already in alias-root space (liveness canonicalized).
            let live_out_here: HashSet<ValueId> = live
                .live_out
                .get(&bid)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            // Roots defined at-or-before each op (block args are roots).
            let mut defined: HashSet<ValueId> = block.args.iter().map(|a| canon(a.id)).collect();
            for (idx, op) in block.ops.iter().enumerate() {
                if is_suspension_point(op.opcode) {
                    let mut seen: HashSet<ValueId> = HashSet::new();
                    for &v in &live_out_here {
                        // `v` is a root; IncRef the root if it is droppable and
                        // already defined before the yield.
                        if drop_eligibility.is_droppable(v)
                            && defined.contains(&v)
                            && seen.insert(v)
                        {
                            plan.before_op.entry(idx).or_default().push(v);
                        }
                    }
                }
                // The op's results become defined AFTER it executes (in root
                // space — a copy result canonicalizes to an already-defined root).
                for &r in &op.results {
                    defined.insert(canon(r));
                }
            }
        }

        // ── 2b. Exception-edge owned-arg retain ─────────────────────────────
        // `CheckException`/`TryStart` carry implicit edges to handler blocks.
        // When the target handler has droppable block args, the edge has the same
        // uniform-owned-phi obligation as an ordinary branch edge, except the edge
        // is conditional inside the op: the normal fallthrough must keep its
        // original ownership while the exceptional transfer may need a retained
        // +1. Therefore a borrowed/non-owned payload gets:
        //
        //     IncRef(v); CheckException(...v...); DecRef(v)
        //
        // The `DecRef` is skipped when the check transfers to the handler, so the
        // retained +1 becomes the handler arg's owned reference. On the normal
        // path it is balanced immediately. A clean function-owned payload needs no
        // retain: the single +1 conditionally flows to the handler on the
        // exceptional path and remains with the normal path otherwise.
        for arc in exception_arcs_for_block(func, block) {
            let Some(handler) = func.blocks.get(&arc.target) else {
                continue;
            };
            if handler.args.is_empty() {
                continue;
            }
            let mut retains = Vec::new();
            for (idx, &v) in arc.args.iter().enumerate() {
                let Some(handler_arg) = handler.args.get(idx) else {
                    continue;
                };
                if !drop_eligibility.is_droppable(handler_arg.id)
                    || drop_eligibility.is_raw_scalar_root(canon(v))
                {
                    continue;
                }
                let root = canon(v);
                if drop_eligibility.is_conditionally_valid_result_root(v) {
                    continue;
                }
                if drop_eligibility.is_droppable(root) {
                    continue;
                }
                retains.push(v);
            }
            if retains.is_empty() {
                continue;
            }
            plan.before_exception_op
                .entry(arc.op_index)
                .or_default()
                .extend(retains.iter().copied());
            plan.after_exception_op
                .entry(arc.op_index)
                .or_default()
                .extend(retains);
        }

        plans.insert(bid, plan);
    }
    emit_drop_inner_stage_audit(
        func,
        "after-block-plan-build",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── 3. Edge-dying drops at successor entry (design §2.5 OpsOnly form) ─────
    // A value V is dropped at the START of block B when:
    //   * V is live-out of at least one predecessor P of B (i.e. P keeps it
    //     alive across the edge), AND
    //   * V is NOT live-in to B (B does not need it), AND
    //   * V is NOT a block arg of B (block args are re-supplied by the edge), AND
    //   * V's defining block DOMINATES B (V is provably available at B's entry —
    //     SSA-dominance soundness; see below), AND
    //   * V is droppable.
    // This releases the value on the path where it dies. Because every path into
    // B that delivered V must release it, and B is a join, dropping once at B's
    // entry is correct ONLY when V dies on ALL incoming paths. We therefore
    // require V to be dead-in to B and live-out of EVERY predecessor that can
    // reach B (so no path still needs it). The elim pass later hoists/dedups.
    //
    // DOMINANCE GUARD (soundness-critical, FAIL-CLOSED). The backward liveness
    // dataflow OVER-APPROXIMATES across the universal `CheckException` edges (C2
    // commit 430e09793): a value can be marked live-out of an exception-edge
    // predecessor whose def-block does NOT terminator-dominate the handler/join
    // block B. A `DecRef(V)` placed at B's entry where V's def does not dominate
    // B is a use-before-def → SSA dominance violation (observed as the LLVM
    // verifier "Instruction does not dominate all uses!" abort on
    // `molt_dec_ref_obj(%isinstance)`).
    //
    // We use the **TerminatorOnly** dominator tree, NOT the Full (analysis) one.
    // This is the SAME view the TIR verifier and the LLVM/native codegen use for
    // SSA dominance (dominators.rs CfgEdgePolicy doc): a handler block reached
    // only via a mid-block exception edge has NO terminator-predecessor, so a
    // value defined in the protected region does NOT terminator-dominate it.
    // The Full tree would (wrongly, for codegen purposes) say a value defined
    // mid-block AFTER a CheckException "dominates" that op's handler — but the
    // exception edge leaves from BEFORE the def, so at the instruction level the
    // def does not dominate the handler. TerminatorOnly dominance matches what
    // codegen enforces, so a guard built on it never admits an
    // exception-path use-before-def.
    //
    // FAIL-CLOSED: if V's def-block does not terminator-dominate B, we DO NOT
    // drop here (keep the +1 / accept a possible leak on that exception path) —
    // the under-release direction. Never over-release (UAF).
    // (`pred_map_term` / `idoms` / `def_block` are built once, above §0b,
    // which shares this exact TerminatorOnly view.)
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let preds = match pred_map.get(&bid) {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let block_args: HashSet<ValueId> = func.blocks[&bid].args.iter().map(|a| a.id).collect();
        // Roots that some predecessor passes as a branch ARG into THIS block's
        // phi(s). Such a value transfers its ownership INTO the block arg on the
        // edge — it is NOT dying on entry, even though liveness reports it dead-in
        // to `B` (its successor-side identity is the block arg, a distinct SSA
        // value). Edge-dropping it here would double-free: the block arg (phi) is
        // the owner now and is released by ITS own last-use / loop / exit drop.
        // (This is the dual of the §5 mixed-ownership retain: §5 ensures the
        // transferred value is owned; this ensures the transfer itself is not also
        // released at the join. Without it, an owned value forwarded into a phi
        // through a multi-block chain — the shape the inliner produces for
        // `x = a + a; return x + a` — was dropped BOTH at the join entry AND at the
        // phi's last use → `invalid object header before dec_ref`.) The per-arc
        // `terminator_arcs` enumeration (filtered to `arc.target == bid`) is the
        // precise per-edge form — it is the single arc-enumeration helper §5 also
        // uses, so there is one source of truth for "args forwarded on this edge".
        let incoming_arg_roots: HashSet<ValueId> = {
            let mut s = HashSet::new();
            for p in preds {
                if let Some(pblock) = func.blocks.get(p) {
                    for arc in terminator_arcs(&pblock.terminator) {
                        if arc.target == bid {
                            for &v in &arc.args {
                                s.insert(canon(v));
                            }
                        }
                    }
                }
            }
            s
        };
        // FINDING 3 (round-4) — keying precision of `incoming_arg_roots`. This set
        // is keyed by alias ROOT over ALL predecessors, NOT per (root, edge). A
        // root forwarded into B's phi by SOME predecessor is excluded from B's
        // edge-dying drop on EVERY incoming path. The theoretically-imprecise case:
        // pred P1 forwards root R into B's phi (transfer), while a DIFFERENT pred P2
        // delivers R live-out and R dies on the P2→B edge without being forwarded.
        // The global exclusion would then skip R's legitimate drop on the P2 path.
        //
        // This is FAIL-CLOSED — leak-never-UAF — and the precise form is NOT a
        // localized change. (1) Fail-closed: on the P2 path R merely leaks (its +1
        // is never released); it is NEVER double-freed, because the phi that P1's
        // transfer fed is released exactly once by the phi's own last-use drop, and
        // the excluded R is never dropped at all. The over-release direction (the
        // only UAF risk) cannot occur. (2) Not localized: the edge-dying rule is
        // deliberately the OpsOnly form — it places ONE `DecRef` at B's *entry*,
        // which fires on EVERY incoming path, and relies on `all_preds_deliver` +
        // the elim pass hoisting the common case (see the §2.5 design note above).
        // Dropping R on the P2 edge but not the P1 edge would require SPLITTING the
        // P2 die-edge to host a per-edge drop — abandoning the at-entry form and
        // splitting a potentially large number of die-edges (a CFG explosion the
        // OpsOnly design exists to avoid). Refining the keying without that split is
        // impossible: a single at-entry drop cannot distinguish the path it runs on.
        //
        // Reachability: in molt's SSA construction a phi at B is fed by each
        // predecessor passing ITS version of the variable to the SAME arg position,
        // so R-from-P1 and R-from-P2 are normally DISTINCT SSA values (P2 would pass
        // its own W, not R) → R is not live-out of P2 and the case does not arise.
        // It can only appear if a value defined above both P1 and P2 is forwarded to
        // the phi on one edge AND separately live on the other — a shape the
        // frontend does not emit for plain joins, and which (per the fail-closed
        // analysis) costs at most a leak if a future frontend/inliner shape does.
        // Pinned by `forwarded_into_phi_other_pred_live_is_leak_not_uaf` below.
        let mut candidates: HashSet<ValueId> = HashSet::new();
        for p in preds {
            if let Some(set) = live.live_out.get(p) {
                candidates.extend(set.iter().copied());
            }
        }
        // Root-level live-in to B: any alias member of the root is live-in.
        let root_live_in = |root: ValueId| -> bool {
            live.live_in
                .get(&bid)
                .is_some_and(|set| set.iter().any(|&m| canon(m) == root))
        };
        let transferred_phi_live_at = |root: ValueId| -> bool {
            let Some(phis) = transferred_phi_args_by_root.get(&root) else {
                return false;
            };
            let live_in = live
                .live_in
                .get(&bid)
                .is_some_and(|set| set.iter().any(|v| phis.contains(v)));
            let live_out = live
                .live_out
                .get(&bid)
                .is_some_and(|set| set.iter().any(|v| phis.contains(v)));
            let mentioned_here = phis.iter().any(|&phi| block_mentions_value(bid, phi));
            let after_transfer_reaches_phi = transferred_phi_live_blocks_by_root
                .get(&root)
                .is_some_and(|blocks| blocks.contains(&bid));
            live_in || live_out || mentioned_here || after_transfer_reaches_phi
        };
        // Roots already scheduled to drop at this block's entry (dedup by root,
        // not raw value — two aliases of the same group must drop once).
        let mut entry_root_seen: HashSet<ValueId> = HashSet::new();
        for v in candidates {
            if !drop_eligibility.is_droppable(v) {
                continue;
            }
            // Conditionally-valid iterator value result (§2.8): NEVER drop it on a
            // die-edge. On the exhaustion edge the value-out slot is uninitialized
            // garbage; a `DecRef` here is a UAF (review P0 #2(b)). On the not-done
            // edge it is consumed by the body's straight-line drop instead. We test
            // the alias ROOT so a transparent copy of the value result is covered
            // too. (An `IterNextUnboxed` value result is never itself a transparent
            // alias of another value — it is a fresh op result — so its root is
            // itself unless a later `Copy` of it widened the group; in that case
            // the whole group is conditionally-valid and equally unsafe to
            // edge-drop.)
            if drop_eligibility.is_conditionally_valid_result_root(v)
                || ownership_lattice.is_conditionally_valid_result_root(canon(v))
            {
                continue;
            }
            let root = canon(v);
            // Python lifetime boundaries are path-conditioned release
            // authorities. The single at-entry edge-dying form would run on every
            // path into `bid`, so pairing it with a body-only statement/rebind
            // boundary or a later scope-exit boundary can release the same local
            // owner twice. Fail closed by leaving such roots to the Python
            // boundary planners rather than synthesizing a join-entry drop from
            // SSA liveness alone.
            if boundary_release_roots.contains(&root)
                || explicit_release_blocks.contains_key(&root)
                || statement_release_plan.contains_released_root(root)
            {
                continue;
            }
            if block_args.contains(&v) || block_args.iter().any(|&a| canon(a) == root) {
                continue;
            }
            // Transferred-into-phi exclusion: `v` (or an alias) is passed as a
            // branch arg into THIS block's phi by some predecessor → its ownership
            // moves into the block arg, it does not die here. Dropping it would
            // double-free the phi's object.
            if incoming_arg_roots.contains(&root) {
                continue;
            }
            // A prior edge may have transferred this source root into a phi in an
            // ancestor/join block. While that phi is live through this block, the
            // phi remains the release authority. Dropping the old source root here
            // would release the same owned object once under the pre-transfer name
            // and once under the phi name.
            if transferred_phi_live_at(root) {
                continue;
            }
            // Dead on entry to B (root-level — no alias member live-in).
            if root_live_in(root) {
                continue;
            }
            // Must die on ALL incoming paths: every predecessor delivers the root
            // group live-out (some alias member live-out of each predecessor), so
            // the single drop here releases it exactly once on every path. A
            // predecessor without the root live-out would mean that path never
            // owned it → a spurious drop on that path.
            let all_preds_deliver = preds.iter().all(|p| {
                live.live_out
                    .get(p)
                    .is_some_and(|s| s.iter().any(|&m| canon(m) == root))
            });
            if !all_preds_deliver {
                continue;
            }
            // DOMINANCE GUARD (fail-closed): V's def-block must dominate B under
            // the TerminatorOnly tree, else V is not provably defined at B's
            // entry and the DecRef would be a use-before-def. Skip (keep the +1).
            match def_block.get(&v) {
                Some(&dblk) if crate::tir::dominators::dominates(dblk, bid, &idoms) => {}
                _ => continue,
            }
            // One drop per root group at this entry.
            if !entry_root_seen.insert(root) {
                continue;
            }
            plans
                .entry(bid)
                .or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
                    before_exception_op: HashMap::new(),
                    after_exception_op: HashMap::new(),
                    before_term_incref: Vec::new(),
                })
                .at_entry
                .push(v);
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-edge-dying",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── 4. Loop-carried phi drops before the back-edge (design §2.7) ─────────
    // A header block arg (phi) `p` whose back-edge passes a NEW value leaves the
    // previous iteration's `p` dead once the new value is computed. If `p` is
    // live-out of the loop body's latch block ONLY because of the phi-slot (i.e.
    // `p` is not used after the point the new value is produced) we would
    // double-count; the conservative correct rule the straight-line + edge-dying
    // rules already implement is: `p` is dropped at its last use. The loop EXIT
    // case (the final phi value, dead after the loop) is handled by edge-dying at
    // the exit block. No separate action needed here beyond what §1–§3 produce;
    // this block is retained as the documented anchor for the loop-carried case
    // and validated by the loop unit test.

    // ── 5. Mixed-ownership phi retain (design §ownership) ─────────────────────
    // A TIR block argument is the SSA phi: each predecessor edge passes a value
    // that binds the arg on entry. The straight-line / edge-dying / loop-carried
    // rules above treat a DROPPABLE (heap, function-owned) block arg as carrying
    // exactly ONE owned `+1` — they DROP it on the path where it dies and TRANSFER
    // it (no drop) where it is forwarded as a branch arg. That is sound ONLY when
    // EVERY incoming edge actually delivers an owned `+1` into the phi.
    //
    // It is NOT sound when an edge delivers a BORROWED value:
    //   * `x = base` then a loop `while …: x = x + base` — the loop-ENTRY edge
    //     binds the accumulator phi to `Copy(base)`, a transparent alias of the
    //     borrowed parameter `base` (the caller owns it; this function never does).
    //     The loop body then drops the phi every iteration, decrementing `base`'s
    //     refcount below the caller's borrow → premature free → UAF / SIGABRT /
    //     SIGSEGV (the round-2 over-release). The control `x = 0` is immune: the
    //     phi is then raw (inline), not droppable, so no drop is placed at all.
    //   * `x = a if c else fresh()` — the `then` arm binds the merge phi to the
    //     borrowed `a`; a later `x + …` drops the merge phi → the same UAF on the
    //     `c` path.
    //
    // THE FIX (uniform ownership at phi boundaries): when a DROPPABLE block arg
    // (an owned phi) has any incoming edge delivering a BORROWED value, RETAIN
    // (`IncRef`) that value on THAT edge. The phi then uniformly owns a `+1` on
    // every path, so the downstream drop releases a real reference and never the
    // caller's borrow. This composes with molt's `+0` borrowed-parameter ABI: the
    // parameter itself stays borrowed; the RETAINED copy is what flows into the
    // phi. It is also exactly correct for the degenerate shapes — `apply(base, 0)`
    // (loop body never runs) returns `x` which IS `base`, and the entry retain is
    // precisely the `+1` the return ABI must transfer to the caller.
    //
    // CLEAN-TRANSFER (no retain) vs BORROWED (retain). An edge value `v` binding
    // an owned phi delivers a clean owned `+1` iff ALL hold:
    //   (a) `v` is heap-carrying (a raw/inline `v` — e.g. `ConstInt 0` feeding a
    //       boxed phi — carries no refcount: `molt_*_ref_obj` is a runtime no-op on
    //       a non-pointer tag, so such an input is self-balancing and an `IncRef`
    //       on it would be a type error on a raw register; SKIP it, mirroring the
    //       repr filter the whole pass uses);
    //   (b) `v` is `droppable` (function-owned: heap, not a parameter, not stack,
    //       not a non-owning `Copy`) AND its alias `root` is not a parameter; and
    //   (c) THIS branch-arg is the sole downstream owner of `root(v)` — `root(v)`
    //       is not also forwarded to another phi (another arg position / edge) and
    //       is not live into a successor's body. If `root(v)` is consumed elsewhere
    //       too, the function's single `+1` stays with that other consumer and this
    //       edge transfers nothing → it must be retained (e.g. `t = f(); x = t;
    //       while …: x = x + 1; return t` — `t` is owned but BOTH seeds the phi and
    //       is returned, so the phi needs its own `+1`).
    // If (a)–(c) hold → clean transfer, NO retain. Otherwise → RETAIN on the edge.
    // FAIL-CLOSED: any doubt retains (an extra `IncRef` is at worst a leak the gates
    // catch — never a UAF). A blanket "never drop mixed phis" is rejected by spec:
    // it would leak the previous accumulator EVERY iteration (O(n) residual).
    //
    // PLACEMENT must be edge-exact. When this block (`P`) reaches the phi block via
    // a SINGLE arc carrying these args (an unconditional `Branch`, or a
    // `CondBranch`/`Switch` with exactly one arm to that target — the preheader and
    // if-arm shapes molt lowers), the `IncRef` goes just before `P`'s terminator
    // (`before_term_incref`). When `P` reaches the target on MULTIPLE arcs with
    // different args (a critical edge — e.g. a `Switch` routing two cases to one
    // block), a before-terminator `IncRef` would wrongly fire on the other arc; we
    // SPLIT that critical edge (a fresh block holding the `IncRef` + a `Branch`),
    // which is why this pass is `Mutates::Cfg`.
    //
    // Owned phis bail with the function (state-machine / exception-handler gate at
    // the top of `run`), so this never runs over `_poll` / handler CFGs.

    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        // Only successor blocks WITH owned block-arg phis matter.
        // Examine each outgoing arc of this block's terminator.
        let term = func.blocks[&bid].terminator.clone();
        let arcs = terminator_arcs(&term);
        for arc in &arcs {
            let Some(succ_block) = func.blocks.get(&arc.target) else {
                continue;
            };
            if succ_block.args.is_empty() {
                continue;
            }
            // How many arcs of THIS block target `arc.target` (placement ambiguity:
            // >1 ⇒ critical edge, must split to place an edge-exact IncRef).
            let arcs_to_target = arcs.iter().filter(|a| a.target == arc.target).count();
            // Compute the retains for THIS arc.
            let mut arc_retains: Vec<ValueId> = Vec::new();
            let mut transferred_roots: HashSet<ValueId> = HashSet::new();
            for (pos, &v) in arc.args.iter().enumerate() {
                let Some(phi) = succ_block.args.get(pos) else {
                    continue;
                };
                let phi_id = phi.id;
                // The phi must be an OWNED obj-lane phi (droppable) for the
                // transfer-ownership assumption to apply. A non-droppable phi
                // (raw/param/stack) is never dropped → no retain obligation.
                if !drop_eligibility.is_droppable(phi_id) {
                    continue;
                }
                // (a) raw/inline edge value → self-balancing, cannot RC. Skip.
                if drop_eligibility.is_raw_scalar_root(canon(v)) {
                    continue;
                }
                let root = canon(v);
                // (b) clean transfer requires the value be function-owned with a
                //     non-parameter root. A borrowed value (param-rooted, or a
                //     non-owning copy, or otherwise not droppable) is NOT a clean
                //     transfer → retain.
                //
                // Test droppability on the ROOT, not on `v` directly: in the
                // alias-root model `is_droppable(x)` is FALSE for any non-root alias
                // (`canon(x) != x`), but a forwarded value is very often an alias of
                // a fresh owned root (`s_next = Copy(s + "x")`, a bare-`Copy` SSA
                // move the union-find folds into `s + "x"`). Checking `is_droppable(v)`
                // would then misclassify that clean-owned forward as borrowed and
                // RETAIN it every iteration — a per-iteration leak of the
                // accumulator (the exact "fresh owned back-edge value must NOT be
                // retained" hazard). `is_droppable(root)` already excludes params /
                // stack / non-owning-copy roots, so it is the correct ownership
                // test for the value the edge actually delivers.
                let function_owned = drop_eligibility.is_droppable(root);
                // Conditionally-valid iterator value result feeding a phi: its
                // backing slot is only valid on the not-done path — never mint an
                // independent ref obligation for it on an edge. Treat as needing a
                // retain only if we cannot prove clean transfer; but since it is
                // never `droppable`-owned in the transfer sense here, fall through
                // to the borrowed branch is unsafe (it would IncRef a possibly
                // uninitialized slot). So SKIP iter-cond values entirely (they are
                // handled by the body straight-line rule on the valid path).
                if drop_eligibility.is_conditionally_valid_result_root(v)
                    || ownership_lattice.is_conditionally_valid_result_root(root)
                {
                    continue;
                }
                let clean_transfer = function_owned
                    // (c) sole downstream owner on THIS executed arc: if the root is
                    //     not live into the successor body, its original +1 may move
                    //     into exactly one owned phi. Additional owned phis on the
                    //     same arc need one retain each.
                    && !edge_body_live_roots
                        .get(&(bid, arc.descriptor))
                        .is_some_and(|s| s.contains(&root))
                    && transferred_roots.insert(root);
                if clean_transfer {
                    continue;
                }
                // BORROWED edge into an owned phi → retain `v` on THIS arc.
                arc_retains.push(v);
            }
            if arc_retains.is_empty() {
                continue;
            }
            if arcs_to_target == 1 && !arc.is_self_loop_into_own_phi(bid) {
                // Single, unambiguous arc to the target: place the IncRef before
                // this block's terminator. (A self-loop where the block is its own
                // successor AND its terminator forwards into its own phi is treated
                // as ambiguous below — splitting keeps the IncRef off the in-block
                // straight-line path.)
                let p = plans.entry(bid).or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
                    before_exception_op: HashMap::new(),
                    after_exception_op: HashMap::new(),
                    before_term_incref: Vec::new(),
                });
                for v in arc_retains {
                    p.before_term_incref.push(v);
                }
            } else {
                // Critical / ambiguous edge: split it. The new block carries the
                // IncRefs then an unconditional Branch to the target with the same
                // args this arc forwarded.
                push_edge_split(
                    &mut edge_splits,
                    bid,
                    arc.descriptor,
                    arc.target,
                    arc.args.clone(),
                    arc_retains,
                    vec![],
                );
            }
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-mixed-phi-retain",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── 0b placements: deferred FinalizerSensitive releases at each Return ───
    // Merged AFTER §1–§5 so they always append to (never overwrite) the
    // per-block plans, and kept in the pre-sorted (BlockId, ValueId-ascending)
    // order — ValueId order is creation order, matching CPython's observed
    // frame-teardown `__del__` sequence for multiple finalizer-bearing locals.
    for &(ret_bid, v) in &deferred_return_placements {
        plans
            .entry(ret_bid)
            .or_insert_with(|| BlockPlan {
                after_op: HashMap::new(),
                at_entry: Vec::new(),
                before_term: Vec::new(),
                before_op: HashMap::new(),
                before_exception_op: HashMap::new(),
                after_exception_op: HashMap::new(),
                before_term_incref: Vec::new(),
            })
            .before_term
            .push(v);
    }
    emit_drop_inner_stage_audit(
        func,
        "after-deferred-placement-merge",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── Apply the plans ──────────────────────────────────────────────────────
    let mut inserted = stats.ops_added;
    let mut plan_block_ids: Vec<BlockId> = plans.keys().copied().collect();
    plan_block_ids.sort_unstable_by_key(|bid| bid.0);
    for bid in plan_block_ids {
        let Some(plan) = plans.get(&bid) else {
            continue;
        };
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        // Rebuild the op vector inserting before_op (IncRef) / after_op (DecRef).
        let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len() + 8);
        // at_entry DecRefs first.
        for v in sorted_unique_values(&plan.at_entry) {
            new_ops.push(make_op(OpCode::DecRef, vec![v]));
            inserted += 1;
        }
        for (idx, op) in block.ops.iter().enumerate() {
            // before_op IncRefs (suspension).
            if let Some(vals) = plan.before_op.get(&idx) {
                for v in sorted_unique_values(vals) {
                    new_ops.push(make_op(OpCode::IncRef, vec![v]));
                    inserted += 1;
                }
            }
            if let Some(vals) = plan.before_exception_op.get(&idx) {
                for v in sorted_values(vals) {
                    new_ops.push(make_op(OpCode::IncRef, vec![v]));
                    inserted += 1;
                }
            }
            new_ops.push(op.clone());
            if let Some(vals) = plan.after_exception_op.get(&idx) {
                for v in sorted_values(vals) {
                    new_ops.push(make_op(OpCode::DecRef, vec![v]));
                    inserted += 1;
                }
            }
            // after_op DecRefs (straight-line last use).
            if let Some(vals) = plan.after_op.get(&idx) {
                for v in ordered_unique_after_op_values(vals, op, &canon) {
                    new_ops.push(make_op(OpCode::DecRef, vec![v]));
                    inserted += 1;
                }
            }
        }
        // before_term_incref IncRefs (the mixed-ownership-phi retain, §5): the
        // BORROWED value this block forwards into a successor's owned phi gets a
        // `+1` here, just before the terminator, on the unambiguous single arc.
        // Placed BEFORE the before_term DecRefs so a value both retained-for-a-phi
        // and dropped-on-another-arc is incref'd before the drop (net correct).
        for v in sorted_values(&plan.before_term_incref) {
            new_ops.push(make_op(OpCode::IncRef, vec![v]));
            inserted += 1;
        }
        // before_term DecRefs — the §0b deferred FinalizerSensitive releases
        // at Return boundaries (and the documented loop-carried anchor /
        // future edge-split upgrade). Insertion order is preserved: §0b
        // pre-sorted by ValueId so multi-instance `__del__` order matches
        // CPython's creation-order frame teardown.
        for v in sorted_unique_values(&plan.before_term) {
            new_ops.push(make_op(OpCode::DecRef, vec![v]));
            inserted += 1;
        }
        block.ops = new_ops;
    }
    emit_drop_inner_stage_audit(
        func,
        "after-plan-apply",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(inserted),
        Some(func.blocks.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── Apply critical-edge splits (§5 ambiguous-arc retains) ─────────────────
    // Each split inserts a fresh block on ONE arc: it holds the retained-value
    // IncRefs then an unconditional Branch to the original target with the args
    // that arc forwarded. The predecessor's terminator is retargeted to the new
    // block (and that arc's args cleared — the new block now supplies them).
    for split in &edge_splits {
        let new_bid = func.fresh_block();
        let mut ops: Vec<TirOp> = Vec::with_capacity(split.retains.len() + split.releases.len());
        for v in sorted_values(&split.retains) {
            ops.push(make_op(OpCode::IncRef, vec![v]));
            inserted += 1;
        }
        for v in sorted_unique_values(&split.releases) {
            ops.push(make_op(OpCode::DecRef, vec![v]));
            inserted += 1;
        }
        func.blocks.insert(
            new_bid,
            crate::tir::blocks::TirBlock {
                id: new_bid,
                args: vec![],
                ops,
                terminator: Terminator::Branch {
                    target: split.target,
                    args: split.args.clone(),
                },
            },
        );
        if let Some(pred) = func.blocks.get_mut(&split.pred) {
            retarget_arc(&mut pred.terminator, &split.arc, new_bid);
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-edge-split-apply",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(inserted),
        Some(func.blocks.len()),
        audit_start.elapsed().as_millis(),
    );

    // Full-function drop authority is a semantic fact, not a mutation count.
    // A function with zero inserted DecRefs can still have borrowed parameters
    // or transparent aliases that the native legacy tracker would otherwise
    // release at scope exit. Mark every non-bailed function that reaches this
    // point so native has exactly one RC authority even when the correct TIR
    // edit is the empty edit.
    func.attrs
        .insert(DROP_INSERTED_ATTR.to_string(), AttrValue::Bool(true));
    stats.facts_changed += 1;
    if debug_this {
        let mut out = format!("[DROP] {} inserted={} blocks:\n", func.name, inserted);
        if !deferred.is_empty() {
            let mut d: Vec<u32> = deferred.iter().map(|v| v.0).collect();
            d.sort_unstable();
            out.push_str(&format!(
                "  deferred(finalizer-sensitive→Return)={:?} placements={:?}\n",
                d,
                deferred_return_placements
                    .iter()
                    .map(|&(b, v)| (b.0, v.0))
                    .collect::<Vec<_>>()
            ));
        }
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in bids {
            let b = &func.blocks[&bid];
            let args: Vec<u32> = b.args.iter().map(|a| a.id.0).collect();
            out.push_str(&format!(
                "  bb{} args={:?} term={:?}\n",
                bid.0, args, b.terminator
            ));
            for op in &b.ops {
                let ops: Vec<u32> = op.operands.iter().map(|o| o.0).collect();
                let res: Vec<u32> = op.results.iter().map(|r| r.0).collect();
                let reprs: Vec<String> = op
                    .operands
                    .iter()
                    .map(|o| {
                        format!(
                            "{}:{}",
                            o.0,
                            if live.is_raw_scalar(*o) {
                                "raw"
                            } else {
                                "heap"
                            }
                        )
                    })
                    .collect();
                // The `_original_kind` carried by a `Copy` is load-bearing for the
                // alias/ownership model (it decides whether the Copy is a no-incref
                // bit-passthrough alias of operand 0 or a fresh owned value). Surface
                // it in the dump so a re-reviewer can audit the alias-set membership
                // against the lowering truth at a glance.
                let kind = match op.attrs.get("_original_kind") {
                    Some(AttrValue::Str(s)) => format!(" kind={s}"),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "    {:?} ops={:?} -> {:?}  [{}]{}\n",
                    op.opcode,
                    ops,
                    res,
                    reprs.join(","),
                    kind
                ));
            }
        }
        let _ =
            crate::debug_artifacts::write_debug_artifact(format!("drop/{}.txt", func.name), out);
    }
    stats.ops_added = inserted;
    stats
}

fn terminator_mentions_value(term: &Terminator, value: ValueId) -> bool {
    match term {
        Terminator::Branch { args, .. } => args.contains(&value),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => *cond == value || then_args.contains(&value) || else_args.contains(&value),
        Terminator::Switch {
            value: cond,
            cases,
            default_args,
            ..
        } => {
            *cond == value
                || cases.iter().any(|(_, _, args)| args.contains(&value))
                || default_args.contains(&value)
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            cases.iter().any(|(_, _, args)| args.contains(&value)) || default_args.contains(&value)
        }
        Terminator::Return { values } => values.contains(&value),
        Terminator::Unreachable => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::TirValue;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn const_str(result: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str("x".into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn finalizer_object(result: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("defines_del".into(), AttrValue::Bool(true));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn finalizer_call_bind(result: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
        attrs.insert("defines_del".into(), AttrValue::Bool(true));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn count_decrefs(func: &TirFunction) -> usize {
        func.blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .count()
    }
    fn count_increfs(func: &TirFunction) -> usize {
        func.blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::IncRef)
            .count()
    }

    fn original_copy(kind: &str, results: Vec<ValueId>) -> TirOp {
        let mut copy = op(OpCode::Copy, vec![], results);
        copy.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        copy
    }

    fn original_copy_with_operands(
        kind: &str,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
    ) -> TirOp {
        let mut copy = op(OpCode::Copy, operands, results);
        copy.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        copy
    }

    fn original_store_var(var: &str, operand: ValueId, result: ValueId) -> TirOp {
        let mut copy = original_copy_with_operands("store_var", vec![operand], vec![result]);
        copy.attrs.insert("_var".into(), AttrValue::Str(var.into()));
        copy
    }

    fn try_start(label: i64) -> TirOp {
        let mut start = op(OpCode::TryStart, vec![], vec![]);
        start.attrs.insert("value".into(), AttrValue::Int(label));
        start
    }

    #[test]
    fn zero_insertion_borrowed_param_function_still_marks_drop_inserted() {
        let mut func = TirFunction::new(
            "borrowed_param_no_owned_temps".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let param = func.blocks[&func.entry_block].args[0].id;
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![op(OpCode::Call, vec![param], vec![])];
            entry.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(
            stats.ops_added, 0,
            "borrowed-param-only functions need no physical drops"
        );
        assert_eq!(count_decrefs(&func), 0);
        assert_eq!(count_increfs(&func), 0);
        assert!(
            matches!(
                func.attrs.get(DROP_INSERTED_ATTR),
                Some(AttrValue::Bool(true))
            ),
            "zero-insertion full analysis must still disable native legacy RC cleanup"
        );
    }

    #[test]
    fn exception_region_match_release_inserts_before_handler_full_drop() {
        let mut func = TirFunction::new("split_exception_cleanup".into(), vec![], TirType::None);
        let clean = func.fresh_block();
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 4);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: clean,
            args: vec![],
        };
        func.blocks.insert(
            clean,
            TirBlock {
                id: clean,
                args: vec![],
                ops: vec![original_copy("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original_copy("exception_last_pending", vec![exc])],
                terminator: Terminator::Branch {
                    target: handler_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_pop,
            TirBlock {
                id: handler_pop,
                args: vec![],
                ops: vec![original_copy("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        assert!(matches!(
            func.attrs.get(EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ));
        assert!(
            matches!(
                func.attrs.get(DROP_INSERTED_ATTR),
                Some(AttrValue::Bool(true))
            ),
            "handler functions now run on full shared DropInsertion ownership"
        );
        assert_eq!(
            func.blocks[&clean]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::DecRef)
                .count(),
            0,
            "the sibling normal cleanup pop must not own the handler match ref"
        );
        let handler_ops = &func.blocks[&handler_pop].ops;
        assert_eq!(handler_ops[0].opcode, OpCode::Copy);
        assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
        assert_eq!(handler_ops[1].operands, vec![exc]);

        let stats = run(&mut func, &mut am);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(
            func.blocks[&handler_pop]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::DecRef)
                .count(),
            1,
            "full drop_inserted marker must make the handler ownership slice idempotent"
        );
    }

    #[test]
    fn exception_creation_ref_releases_at_raise_with_handler_full_drop() {
        let mut func = TirFunction::new("raise_creation_cleanup".into(), vec![], TirType::None);
        let handler = func.fresh_block();
        let exc = func.fresh_value();
        func.label_id_map.insert(handler.0, 4);

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![
            try_start(4),
            original_copy("exception_new_builtin_one", vec![exc]),
            op(OpCode::Raise, vec![exc], vec![]),
        ];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original_copy("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        assert!(matches!(
            func.attrs.get(EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ));
        assert!(
            matches!(
                func.attrs.get(DROP_INSERTED_ATTR),
                Some(AttrValue::Bool(true))
            ),
            "raise-path CreationRef release composes with full handler DropInsertion"
        );
        let entry_ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(entry_ops[2].opcode, OpCode::Raise);
        assert_eq!(entry_ops[3].opcode, OpCode::DecRef);
        assert_eq!(entry_ops[3].operands, vec![exc]);
    }

    #[test]
    fn exception_edge_borrowed_payload_retains_for_owned_handler_arg() {
        let mut func = TirFunction::new(
            "exception_edge_borrowed_payload".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_arg = func.fresh_value();
        func.value_types.insert(handler_arg, TirType::DynBox);
        func.label_id_map.insert(handler.0, 4);

        let param = func.blocks[&func.entry_block].args[0].id;
        let mut check = op(OpCode::CheckException, vec![param], vec![]);
        check.attrs.insert("value".into(), AttrValue::Int(4));
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![try_start(4), check];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::Call, vec![handler_arg], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(
            stats.ops_added, 3,
            "borrowed payload retain+normal release plus handler arg release"
        );
        let entry_ops = &func.blocks[&func.entry_block].ops;
        let check_idx = entry_ops
            .iter()
            .position(|op| op.opcode == OpCode::CheckException)
            .expect("check_exception survives");
        assert_eq!(entry_ops[check_idx - 1].opcode, OpCode::IncRef);
        assert_eq!(entry_ops[check_idx - 1].operands, vec![param]);
        assert_eq!(entry_ops[check_idx + 1].opcode, OpCode::DecRef);
        assert_eq!(entry_ops[check_idx + 1].operands, vec![param]);

        let handler_ops = &func.blocks[&handler].ops;
        assert_eq!(handler_ops[0].opcode, OpCode::Call);
        assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
        assert_eq!(handler_ops[1].operands, vec![handler_arg]);
    }

    #[test]
    fn try_start_edge_borrowed_payload_retains_for_owned_handler_arg() {
        let mut func = TirFunction::new(
            "try_start_edge_borrowed_payload".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_arg = func.fresh_value();
        func.value_types.insert(handler_arg, TirType::DynBox);
        func.label_id_map.insert(handler.0, 4);

        let param = func.blocks[&func.entry_block].args[0].id;
        let mut start = op(OpCode::TryStart, vec![param], vec![]);
        start.attrs.insert("value".into(), AttrValue::Int(4));
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![start];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::Call, vec![handler_arg], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(
            stats.ops_added, 3,
            "try_start borrowed payload retain+normal release plus handler arg release"
        );
        let entry_ops = &func.blocks[&func.entry_block].ops;
        let start_idx = entry_ops
            .iter()
            .position(|op| op.opcode == OpCode::TryStart)
            .expect("try_start survives");
        assert_eq!(entry_ops[start_idx - 1].opcode, OpCode::IncRef);
        assert_eq!(entry_ops[start_idx - 1].operands, vec![param]);
        assert_eq!(entry_ops[start_idx + 1].opcode, OpCode::DecRef);
        assert_eq!(entry_ops[start_idx + 1].operands, vec![param]);

        let handler_ops = &func.blocks[&handler].ops;
        assert_eq!(handler_ops[0].opcode, OpCode::Call);
        assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
        assert_eq!(handler_ops[1].operands, vec![handler_arg]);
    }

    #[test]
    fn exception_creation_ref_release_is_path_local_for_alternative_raises() {
        let mut func = TirFunction::new("raise_creation_diamond".into(), vec![], TirType::None);
        let then_raise = func.fresh_block();
        let else_raise = func.fresh_block();
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                op(OpCode::ConstBool, vec![], vec![cond]),
                original_copy("exception_new_builtin_one", vec![exc]),
            ];
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: then_raise,
                then_args: vec![],
                else_block: else_raise,
                else_args: vec![],
            };
        }
        for block in [then_raise, else_raise] {
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: vec![],
                    ops: vec![op(OpCode::Raise, vec![exc], vec![])],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 2);
        for block in [then_raise, else_raise] {
            let ops = &func.blocks[&block].ops;
            assert_eq!(ops[0].opcode, OpCode::Raise);
            assert_eq!(ops[1].opcode, OpCode::DecRef);
            assert_eq!(ops[1].operands, vec![exc]);
        }
        assert_eq!(
            count_decrefs(&func),
            2,
            "CreationRef release is path-local: each mutually exclusive raise edge must release the shared SSA ref on the path that actually raises"
        );
    }

    #[test]
    fn handler_match_ref_is_not_released_at_reraise() {
        let mut func = TirFunction::new("reraise_match_ref_cleanup".into(), vec![], TirType::None);
        let handler = func.fresh_block();
        let exc = func.fresh_value();
        func.label_id_map.insert(handler.0, 4);

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original_copy("exception_last_pending", vec![exc]),
                    op(OpCode::Raise, vec![exc], vec![]),
                    original_copy("exception_pop", vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        let handler_ops = &func.blocks[&handler].ops;
        assert_eq!(handler_ops[1].opcode, OpCode::Raise);
        assert_eq!(
            handler_ops[2].opcode,
            OpCode::Copy,
            "the reraise itself must not consume the handler MatchRef"
        );
        assert_eq!(handler_ops[3].opcode, OpCode::DecRef);
        assert_eq!(handler_ops[3].operands, vec![exc]);
    }

    #[test]
    fn exception_region_match_release_splits_shared_pop_by_dominating_edge() {
        let mut func = TirFunction::new("shared_exception_pop".into(), vec![], TirType::None);
        let normal = func.fresh_block();
        let shared_pop = func.fresh_block();
        let after_pop = func.fresh_block();
        let handler = func.fresh_block();
        let handler_body = func.fresh_block();
        func.label_id_map.insert(handler.0, 4);
        let exc = func.fresh_value();
        let matched = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: normal,
            args: vec![],
        };
        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            shared_pop,
            TirBlock {
                id: shared_pop,
                args: vec![],
                ops: vec![original_copy("exception_pop", vec![])],
                terminator: Terminator::Branch {
                    target: after_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            after_pop,
            TirBlock {
                id: after_pop,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original_copy("exception_last_pending", vec![exc]),
                    op(OpCode::Copy, vec![exc], vec![matched]),
                ],
                terminator: Terminator::Branch {
                    target: handler_body,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_body,
            TirBlock {
                id: handler_body,
                args: vec![],
                ops: vec![op(OpCode::Copy, vec![matched], vec![])],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![],
                },
            },
        );

        let mut am = AnalysisManager::new();
        let before_blocks = func.blocks.len();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        assert!(
            func.blocks.len() >= before_blocks + 2,
            "shared pop release must split out a post-pop continuation and a handler pop clone"
        );
        assert_eq!(
            func.blocks[&shared_pop]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::DecRef)
                .count(),
            0,
            "normal path must not see a handler-only MatchRef DecRef"
        );
        let handler_successor = match &func.blocks[&handler_body].terminator {
            Terminator::Branch { target, .. } => *target,
            other => panic!("handler edge should remain unconditional, got {other:?}"),
        };
        assert_ne!(
            handler_successor, shared_pop,
            "handler edge should be retargeted to a split pop block"
        );
        let split_ops = &func.blocks[&handler_successor].ops;
        assert_eq!(split_ops[0].opcode, OpCode::Copy);
        assert_eq!(split_ops[1].opcode, OpCode::DecRef);
        assert_eq!(split_ops[1].operands, vec![exc]);
        crate::tir::verify::verify_function(&func)
            .expect("path-specific exception MatchRef release must preserve SSA dominance");
    }

    #[test]
    fn exception_region_match_release_splits_shared_pop_with_block_args() {
        let mut func = TirFunction::new(
            "shared_exception_pop_with_arg".into(),
            vec![],
            TirType::None,
        );
        let normal = func.fresh_block();
        let handler = func.fresh_block();
        let handler_body = func.fresh_block();
        let shared_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 4);
        let exc = func.fresh_value();
        let normal_arg = func.fresh_value();
        let handler_arg = func.fresh_value();
        let pop_arg = func.fresh_value();
        let tail_value = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: normal,
            args: vec![],
        };
        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![const_str(normal_arg)],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![normal_arg],
                },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original_copy("exception_last_pending", vec![exc]),
                    const_str(handler_arg),
                ],
                terminator: Terminator::Branch {
                    target: handler_body,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_body,
            TirBlock {
                id: handler_body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![handler_arg],
                },
            },
        );
        func.blocks.insert(
            shared_pop,
            TirBlock {
                id: shared_pop,
                args: vec![TirValue {
                    id: pop_arg,
                    ty: TirType::Str,
                }],
                ops: vec![
                    original_copy("exception_pop", vec![]),
                    op(OpCode::Copy, vec![pop_arg], vec![tail_value]),
                ],
                terminator: Terminator::Return {
                    values: vec![tail_value],
                },
            },
        );

        let mut am = AnalysisManager::new();
        let before_blocks = func.blocks.len();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        assert_eq!(
            func.blocks.len(),
            before_blocks + 2,
            "block-arg shared pop needs exactly one continuation and one handler split"
        );
        assert_eq!(
            func.blocks[&shared_pop]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::DecRef)
                .count(),
            0,
            "normal path must not see a handler-only MatchRef DecRef"
        );

        let continuation = match &func.blocks[&shared_pop].terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(
                    args,
                    &vec![pop_arg],
                    "the original pop block must forward its incoming phi payload"
                );
                *target
            }
            other => panic!("shared pop must branch to a continuation, got {other:?}"),
        };
        let continuation_block = &func.blocks[&continuation];
        assert_eq!(continuation_block.args.len(), 1);
        let continuation_arg = continuation_block.args[0].id;
        assert_ne!(
            continuation_arg, pop_arg,
            "the moved tail must own a fresh block arg instead of reusing the pre-split phi"
        );
        assert_eq!(continuation_block.ops[0].opcode, OpCode::Copy);
        assert_eq!(continuation_block.ops[0].operands, vec![continuation_arg]);

        let handler_successor = match &func.blocks[&handler_body].terminator {
            Terminator::Branch { target, .. } => *target,
            other => panic!("handler edge should remain unconditional, got {other:?}"),
        };
        assert_ne!(
            handler_successor, shared_pop,
            "handler edge should be retargeted to a path-specific pop clone"
        );
        let split = &func.blocks[&handler_successor];
        assert_eq!(split.ops[0].opcode, OpCode::Copy);
        assert_eq!(split.ops[1].opcode, OpCode::DecRef);
        assert_eq!(split.ops[1].operands, vec![exc]);
        match &split.terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(*target, continuation);
                assert_eq!(
                    args,
                    &vec![handler_arg],
                    "the handler split must forward the original handler edge payload"
                );
            }
            other => panic!("handler split must branch to continuation, got {other:?}"),
        }

        crate::tir::verify::verify_function(&func)
            .expect("block-arg path-specific exception release must preserve SSA dominance");
    }

    #[test]
    fn exception_region_match_release_remaps_dominated_successor_uses() {
        let mut func = TirFunction::new(
            "shared_exception_pop_successor_uses_arg".into(),
            vec![],
            TirType::None,
        );
        let normal = func.fresh_block();
        let handler = func.fresh_block();
        let handler_body = func.fresh_block();
        let shared_pop = func.fresh_block();
        let after_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 4);

        let exc = func.fresh_value();
        let normal_arg = func.fresh_value();
        let handler_arg = func.fresh_value();
        let pop_arg = func.fresh_value();
        let pop_alias = func.fresh_value();
        let tail_value = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: normal,
            args: vec![],
        };
        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![const_str(normal_arg)],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![normal_arg],
                },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original_copy("exception_last_pending", vec![exc]),
                    const_str(handler_arg),
                ],
                terminator: Terminator::Branch {
                    target: handler_body,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_body,
            TirBlock {
                id: handler_body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: shared_pop,
                    args: vec![handler_arg],
                },
            },
        );
        func.blocks.insert(
            shared_pop,
            TirBlock {
                id: shared_pop,
                args: vec![TirValue {
                    id: pop_arg,
                    ty: TirType::Str,
                }],
                ops: vec![
                    original_copy_with_operands("load_var", vec![pop_arg], vec![pop_alias]),
                    original_copy("exception_pop", vec![]),
                ],
                terminator: Terminator::Branch {
                    target: after_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            after_pop,
            TirBlock {
                id: after_pop,
                args: vec![],
                ops: vec![op(OpCode::Copy, vec![pop_alias], vec![tail_value])],
                terminator: Terminator::Return {
                    values: vec![tail_value],
                },
            },
        );

        let mut am = AnalysisManager::new();
        let before_blocks = func.blocks.len();
        let stats = run(&mut func, &mut am);

        assert_eq!(stats.ops_added, 1);
        assert_eq!(
            func.blocks.len(),
            before_blocks + 2,
            "shared pop needs a continuation plus the handler-specific release split"
        );

        let continuation = match &func.blocks[&shared_pop].terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(args, &vec![pop_arg]);
                *target
            }
            other => panic!("shared pop must branch to a continuation, got {other:?}"),
        };
        let continuation_arg = func.blocks[&continuation].args[0].id;
        assert_ne!(continuation_arg, pop_arg);
        assert_eq!(
            func.blocks[&after_pop].ops[0].operands,
            vec![continuation_arg],
            "dominated successor must read the post-split continuation arg, not the stale pre-split phi"
        );

        crate::tir::verify::verify_function(&func)
            .expect("post-pop split must preserve SSA dominance through dominated successors");
    }

    #[test]
    fn finalizer_sensitive_container_releases_at_return_boundary() {
        let mut func = TirFunction::new("finalizer_scope".into(), vec![], TirType::None);
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(op(OpCode::BuildList, vec![item], vec![list]));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::BuildList)
            .expect("BuildList op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(list_idx + 1, item), (marker_idx + 1, list)],
            "absorbed producer temp releases at list construction; container owner releases at return"
        );
    }

    #[test]
    fn result_carrying_store_var_keeps_container_owner_to_return_boundary() {
        let mut func =
            TirFunction::new("finalizer_scope_store_result".into(), vec![], TirType::None);
        let item = func.fresh_value();
        let list = func.fresh_value();
        let stored = func.fresh_value();
        for v in [item, list, stored] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![list],
                vec![stored],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let store_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
            .expect("store_var marker must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert!(
            !dropped
                .iter()
                .any(|(idx, value)| *idx == store_idx + 1 && *value == list),
            "store_var is a no-incref local lifetime marker; it must not release the source owner"
        );
        assert_eq!(
            dropped,
            vec![(list_idx + 1, item), (marker_idx + 1, list)],
            "result-carrying store_var aliases the source owner and defers finalizer-sensitive locals to return"
        );
    }

    #[test]
    fn store_var_boundary_transferred_to_cleanup_block_arg_releases_once() {
        let mut func = TirFunction::new(
            "store_var_cleanup_join_transfers_owner".into(),
            vec![],
            TirType::None,
        );
        let class_obj = func.fresh_value();
        let stored = func.fresh_value();
        let cleanup_arg = func.fresh_value();
        for v in [class_obj, stored, cleanup_arg] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        let cleanup = func.fresh_block();
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(class_obj));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![class_obj],
                vec![stored],
            ));
            b.terminator = Terminator::Branch {
                target: cleanup,
                args: vec![stored],
            };
        }
        func.blocks.insert(
            cleanup,
            TirBlock {
                id: cleanup,
                args: vec![TirValue {
                    id: cleanup_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::DelBoundary, vec![cleanup_arg], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let entry_drops: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !entry_drops.contains(&class_obj),
            "store_var transfers the single owner to the cleanup block arg; the source root must not be dropped on the predecessor"
        );

        let cleanup_drops: Vec<ValueId> = func.blocks[&cleanup]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            cleanup_drops,
            vec![cleanup_arg],
            "cleanup block arg is the release authority; original store_var root must not receive a second return-boundary DecRef"
        );
    }

    #[test]
    fn store_var_transfer_phi_live_in_descendant_blocks_old_root_drop() {
        let mut func = TirFunction::new(
            "store_var_transfer_phi_live_in_descendant".into(),
            vec![],
            TirType::None,
        );
        let join = func.fresh_block();
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let ret = func.fresh_block();
        let list = func.fresh_value();
        let stored = func.fresh_value();
        let phi = func.fresh_value();
        let cond = func.fresh_value();
        for v in [list, stored, phi] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);

        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops
                .push(original_copy_with_operands("list_new", vec![], vec![list]));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![list],
                vec![stored],
            ));
            b.terminator = Terminator::Branch {
                target: join,
                args: vec![stored],
            };
        }
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args: vec![],
                    else_block,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            then_block,
            TirBlock {
                id: then_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: ret,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            ret,
            TirBlock {
                id: ret,
                args: vec![],
                ops: vec![op(OpCode::DelBoundary, vec![phi], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        crate::tir::verify::verify_function(&func)
            .expect("drop insertion must preserve SSA after descendant phi-transfer exclusion");

        let entry_increfs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::IncRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            entry_increfs.is_empty(),
            "clean transfer into the phi must not retain a second owner: {entry_increfs:?}"
        );

        let else_drops: Vec<ValueId> = func.blocks[&else_block]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !else_drops.contains(&list),
            "the source root was transferred into the live phi; descendant edge-dying must not release it"
        );

        let ret_drops: Vec<ValueId> = func.blocks[&ret]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            ret_drops,
            vec![phi],
            "the phi remains the release authority on the descendant return path"
        );
    }

    #[test]
    fn store_var_scope_root_survives_loop_exit_to_return_boundary() {
        let mut func = TirFunction::new(
            "store_var_scope_root_survives_loop_exit".into(),
            vec![],
            TirType::None,
        );
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let owner = func.fresh_value();
        let stored = func.fresh_value();
        let alias = func.fresh_value();
        let cond = func.fresh_value();
        let call_result = func.fresh_value();
        for v in [owner, stored, alias, call_result] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);

        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(owner));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![owner],
                vec![stored],
            ));
            b.ops.push(original_copy_with_operands(
                "copy_var",
                vec![owner],
                vec![alias],
            ));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
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
                ops: vec![op(OpCode::Call, vec![alias], vec![call_result])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
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

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let exit_drops: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            exit_drops,
            vec![owner],
            "a Python-bound store_var root is released at the scope-exit return boundary only; edge-dying must not pre-release it on the loop exit"
        );
    }

    #[test]
    fn store_var_boundary_mixed_return_paths_split_non_transfer_release() {
        let mut func =
            TirFunction::new("store_var_mixed_return_paths".into(), vec![], TirType::None);
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let ret = func.fresh_block();
        let class_obj = func.fresh_value();
        let stored = func.fresh_value();
        let fallback = func.fresh_value();
        let selected = func.fresh_value();
        let cond = func.fresh_value();
        for v in [class_obj, stored, fallback, selected] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(class_obj));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![class_obj],
                vec![stored],
            ));
            b.ops.push(finalizer_object(fallback));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block,
                then_args: vec![],
                else_block,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            then_block,
            TirBlock {
                id: then_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: ret,
                    args: vec![stored],
                },
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: ret,
                    args: vec![fallback],
                },
            },
        );
        func.blocks.insert(
            ret,
            TirBlock {
                id: ret,
                args: vec![TirValue {
                    id: selected,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::DelBoundary, vec![selected], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ret_drops: Vec<ValueId> = func.blocks[&ret]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            ret_drops,
            vec![selected],
            "return block drops the selected phi only; the original store_var root is path-specific"
        );

        let split_releases_class_obj = func.blocks.iter().any(|(&bid, block)| {
            bid != entry
                && bid != then_block
                && bid != else_block
                && bid != ret
                && block
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![class_obj])
                && matches!(
                    &block.terminator,
                    Terminator::Branch { target, args }
                        if *target == ret && args == &vec![fallback]
                )
        });
        assert!(
            split_releases_class_obj,
            "the non-transfer edge must release the original store_var root in an edge split"
        );
    }

    #[test]
    fn store_var_rebind_epoch_closes_old_scope_cleanup_candidate() {
        let mut func = TirFunction::new(
            "store_var_rebind_epoch_cleanup".into(),
            vec![],
            TirType::None,
        );
        let rebind = func.fresh_block();
        let keep = func.fresh_block();
        let join = func.fresh_block();
        let cleanup = func.fresh_block();
        let old_owner = func.fresh_value();
        let old_stored = func.fresh_value();
        let new_owner = func.fresh_value();
        let new_stored = func.fresh_value();
        let rebind_current = func.fresh_value();
        let keep_current = func.fresh_value();
        let current_phi = func.fresh_value();
        let cleanup_phi = func.fresh_value();
        let cond = func.fresh_value();
        let old_len = func.fresh_value();
        for v in [
            old_owner,
            old_stored,
            new_owner,
            new_stored,
            rebind_current,
            keep_current,
            current_phi,
            cleanup_phi,
        ] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);
        func.value_types.insert(old_len, TirType::I64);

        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![],
                vec![old_owner],
            ));
            b.ops
                .push(original_store_var("or_clause", old_owner, old_stored));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: rebind,
                then_args: vec![old_stored],
                else_block: keep,
                else_args: vec![old_stored],
            };
        }
        func.blocks.insert(
            rebind,
            TirBlock {
                id: rebind,
                args: vec![TirValue {
                    id: rebind_current,
                    ty: TirType::DynBox,
                }],
                ops: vec![
                    original_copy_with_operands("len", vec![rebind_current], vec![old_len]),
                    original_copy_with_operands("list_new", vec![], vec![new_owner]),
                    original_store_var("or_clause", new_owner, new_stored),
                ],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![new_stored],
                },
            },
        );
        func.blocks.insert(
            keep,
            TirBlock {
                id: keep,
                args: vec![TirValue {
                    id: keep_current,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![keep_current],
                },
            },
        );
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: current_phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: cleanup,
                    args: vec![current_phi],
                },
            },
        );
        func.blocks.insert(
            cleanup,
            TirBlock {
                id: cleanup,
                args: vec![TirValue {
                    id: cleanup_phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::DelBoundary, vec![cleanup_phi], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        crate::tir::verify::verify_function(&func)
            .expect("drop insertion must preserve SSA through store_var rebind cleanup");

        let old_direct_drops: Vec<(BlockId, usize)> = func
            .blocks
            .iter()
            .flat_map(|(&bid, block)| {
                block.ops.iter().enumerate().filter_map(move |(idx, op)| {
                    (op.opcode == OpCode::DecRef && op.operands == vec![old_owner])
                        .then_some((bid, idx))
                })
            })
            .collect();
        assert_eq!(
            old_direct_drops,
            Vec::<(BlockId, usize)>::new(),
            "once the old source owner transfers into local block args, cleanup must release the current epoch carrier, not the original root: {old_direct_drops:?}"
        );

        let rebind_current_drops: Vec<ValueId> = func.blocks[&rebind]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            rebind_current_drops,
            vec![rebind_current],
            "the rebind path must close the previous carried local epoch exactly once"
        );

        let cleanup_drops: Vec<ValueId> = func.blocks[&cleanup]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            cleanup_drops,
            vec![cleanup_phi],
            "scope cleanup must release the current local epoch only"
        );
        assert!(
            !cleanup_drops.contains(&old_owner) && !cleanup_drops.contains(&new_owner),
            "producer roots transferred into the current cleanup phi must not be dropped again"
        );
    }

    #[test]
    fn store_var_origin_carrier_live_to_return_cleanup_suppresses_source_release() {
        let mut func = TirFunction::new(
            "store_var_origin_carrier_live_to_return_cleanup".into(),
            vec![],
            TirType::None,
        );
        let carrier_a_block = func.fresh_block();
        let carrier_b_block = func.fresh_block();
        let return_pred = func.fresh_block();
        let ret = func.fresh_block();
        let owner = func.fresh_value();
        let stored = func.fresh_value();
        let carrier_a = func.fresh_value();
        let carrier_b = func.fresh_value();
        let use_result = func.fresh_value();
        for value in [owner, stored, carrier_a, carrier_b, use_result] {
            func.value_types.insert(value, TirType::DynBox);
        }

        let entry = func.entry_block;
        {
            let block = func.blocks.get_mut(&entry).unwrap();
            block.ops.push(original_copy_with_operands(
                "object_new",
                vec![],
                vec![owner],
            ));
            block.ops.push(original_store_var("args", owner, stored));
            block.terminator = Terminator::Branch {
                target: carrier_a_block,
                args: vec![stored],
            };
        }
        func.blocks.insert(
            carrier_a_block,
            TirBlock {
                id: carrier_a_block,
                args: vec![TirValue {
                    id: carrier_a,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: carrier_b_block,
                    args: vec![carrier_a],
                },
            },
        );
        func.blocks.insert(
            carrier_b_block,
            TirBlock {
                id: carrier_b_block,
                args: vec![TirValue {
                    id: carrier_b,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: return_pred,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            return_pred,
            TirBlock {
                id: return_pred,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: ret,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            ret,
            TirBlock {
                id: ret,
                args: vec![],
                ops: vec![op(OpCode::Call, vec![carrier_b], vec![use_result])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        crate::tir::verify::verify_function(&func)
            .expect("origin-carrier return cleanup must preserve SSA");

        let source_drops: Vec<(BlockId, ValueId)> = func
            .blocks
            .iter()
            .flat_map(|(&bid, block)| {
                block.ops.iter().filter_map(move |op| {
                    let &operand = op.operands.first()?;
                    (op.opcode == OpCode::DecRef && (operand == owner || operand == stored))
                        .then_some((bid, operand))
                })
            })
            .collect();
        assert_eq!(
            source_drops,
            Vec::<(BlockId, ValueId)>::new(),
            "once a store_var source has moved into a live return-cleanup carrier, the source root is no longer an edge or return release authority"
        );

        let ret_drops: Vec<ValueId> = func.blocks[&ret]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            ret_drops.contains(&carrier_b),
            "the live carrier remains the cleanup authority at return; got {ret_drops:?}"
        );
    }

    #[test]
    fn owned_root_forwarded_to_three_owned_phis_gets_two_retains() {
        let mut func = TirFunction::new(
            "owned_root_three_phi_retain_multiplicity".into(),
            vec![],
            TirType::None,
        );
        let join = func.fresh_block();
        let owner = func.fresh_value();
        let a = func.fresh_value();
        let b = func.fresh_value();
        let c = func.fresh_value();
        for v in [owner, a, b, c] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let block = func.blocks.get_mut(&entry).unwrap();
            block.ops.push(finalizer_call_bind(owner));
            block.terminator = Terminator::Branch {
                target: join,
                args: vec![owner, owner, owner],
            };
        }
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![
                    TirValue {
                        id: a,
                        ty: TirType::DynBox,
                    },
                    TirValue {
                        id: b,
                        ty: TirType::DynBox,
                    },
                    TirValue {
                        id: c,
                        ty: TirType::DynBox,
                    },
                ],
                ops: vec![
                    op(OpCode::DelBoundary, vec![a], vec![]),
                    op(OpCode::DelBoundary, vec![b], vec![]),
                    op(OpCode::DelBoundary, vec![c], vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let entry_increfs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::IncRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            entry_increfs,
            vec![owner, owner],
            "one original owner transfers to the first phi; the other two owned phis need retained references"
        );
    }

    #[test]
    fn store_var_boundary_transferred_through_loop_phi_releases_phi_once() {
        let mut func = TirFunction::new(
            "store_var_loop_phi_transfers_owner".into(),
            vec![],
            TirType::None,
        );
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let set_owner = func.fresh_value();
        let stored = func.fresh_value();
        let current_phi = func.fresh_value();
        let next_owner = func.fresh_value();
        let next_stored = func.fresh_value();
        let cond = func.fresh_value();
        for v in [set_owner, stored, current_phi, next_owner, next_stored] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(original_copy_with_operands(
                "set_new",
                vec![],
                vec![set_owner],
            ));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![set_owner],
                vec![stored],
            ));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![stored],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: current_phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: exit,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    original_copy_with_operands("set_new", vec![], vec![next_owner]),
                    original_copy_with_operands("store_var", vec![next_owner], vec![next_stored]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_stored],
                },
            },
        );
        let mut exit_boundary = op(OpCode::DelBoundary, vec![current_phi], vec![]);
        exit_boundary
            .attrs
            .insert("s_value".into(), AttrValue::Str("inherited".into()));
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![exit_boundary],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let exit_drops: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            exit_drops,
            vec![current_phi],
            "the set_new owner moved into the loop phi; the phi boundary is the sole release authority"
        );
        assert!(
            !exit_drops.contains(&set_owner),
            "the original producer root must not be return-boundary dropped after a clean phi transfer"
        );
        assert!(
            !exit_drops.contains(&next_owner),
            "the backedge producer root must also transfer into the loop phi instead of dropping beside it at return"
        );
    }

    #[test]
    fn result_carrying_store_var_later_container_absorb_keeps_owner_to_return_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_store_result_later_absorb".into(),
            vec![],
            TirType::None,
        );
        let list = func.fresh_value();
        let stored = func.fresh_value();
        let item = func.fresh_value();
        for v in [list, stored, item] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops
                .push(original_copy_with_operands("list_new", vec![], vec![list]));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![list],
                vec![stored],
            ));
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_append",
                vec![stored, item],
                vec![],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let store_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
            .expect("store_var marker must survive");
        let append_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_append"))
            .expect("list_append op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert!(
            !dropped
                .iter()
                .any(|(idx, value)| *idx == store_idx + 1 && *value == list),
            "a result-carrying store_var must not release an empty container before later mutation through its alias"
        );
        assert_eq!(
            dropped,
            vec![(append_idx + 1, item), (marker_idx + 1, list)],
            "later container absorption makes the Python-bound owner finalizer-sensitive without moving its release before return"
        );
        assert!(
            !dropped
                .iter()
                .any(|(idx, value)| *idx == list_idx + 1 && *value == list),
            "empty container owner must survive past construction once bound to a Python local"
        );
    }

    #[test]
    fn copy_list_new_finalizer_sensitive_container_releases_at_return_boundary() {
        let mut func = TirFunction::new("finalizer_scope_copy_list".into(), vec![], TirType::None);
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(list_idx + 1, item), (marker_idx + 1, list)],
            "Copy-preserved list_new must release the producer temp at the absorption boundary"
        );
    }

    #[test]
    fn copy_class_def_descriptor_temp_releases_at_class_construction_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_copy_class_def".into(),
            vec![],
            TirType::None,
        );
        let name = func.fresh_value();
        let descriptor = func.fresh_value();
        let class_obj = func.fresh_value();
        for v in [name, descriptor, class_obj] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(name));
            b.ops.push(finalizer_object(descriptor));
            b.ops.push(original_copy_with_operands(
                "class_def",
                vec![name, descriptor],
                vec![class_obj],
            ));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![class_obj],
                vec![],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let class_def_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("class_def"))
            .expect("class_def op must survive");
        let store_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
            .expect("store_var op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let tracked_drops: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .filter_map(|(idx, op)| {
                let dropped = op.operands[0];
                [descriptor, class_obj]
                    .contains(&dropped)
                    .then_some((idx, dropped))
            })
            .collect();
        let descriptor_drop_idx = tracked_drops
            .iter()
            .find_map(|(idx, dropped)| (*dropped == descriptor).then_some(*idx))
            .expect("descriptor temp must be dropped");
        let class_drop_idx = tracked_drops
            .iter()
            .find_map(|(idx, dropped)| (*dropped == class_obj).then_some(*idx))
            .expect("class owner must be dropped");
        assert!(
            class_def_idx < descriptor_drop_idx && descriptor_drop_idx < store_idx,
            "descriptor temp must release at the class construction boundary before the class owner is used"
        );
        assert_eq!(
            class_drop_idx,
            marker_idx + 1,
            "class owner should remain live until the Python boundary"
        );
    }

    #[test]
    fn call_bind_list_new_finalizer_sensitive_container_releases_at_return_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_call_bind_list".into(),
            vec![],
            TirType::None,
        );
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(list_idx + 1, item), (marker_idx + 1, list)],
            "call_bind-created finalizer temps release at list_new while the container owner defers"
        );
    }

    #[test]
    fn unbound_finalizer_container_call_arg_releases_at_call_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_unbound_call_arg".into(),
            vec![],
            TirType::None,
        );
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops.push(op(OpCode::Call, vec![list], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let call_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Call && op.operands == vec![list])
            .expect("call op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(list_idx + 1, item), (call_idx + 1, list)],
            "unbound finalizer-sensitive expression temps die at their last use, not at frame return"
        );
        assert!(
            call_idx < marker_idx,
            "fixture must keep a later side effect after the call boundary"
        );
    }

    #[test]
    fn call_bind_check_exception_list_new_finalizer_releases_at_return_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_real_call_bind_list".into(),
            vec![],
            TirType::None,
        );
        let callee = func.fresh_value();
        let builder = func.fresh_value();
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [callee, builder, item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::ModuleGetAttr, vec![], vec![callee]));
            b.ops.push(original_copy_with_operands(
                "callargs_new",
                vec![],
                vec![builder],
            ));
            let mut call = finalizer_call_bind(item);
            call.operands = vec![callee, builder];
            b.ops.push(call);
            b.ops.push(op(OpCode::CheckException, vec![], vec![]));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(op(OpCode::CheckException, vec![], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert!(
            dropped.contains(&(list_idx + 1, item)),
            "call result temp must release at the list_new absorption boundary: {dropped:?}"
        );
        assert!(
            dropped.contains(&(marker_idx + 1, list)),
            "absorbing list owner must still release at return boundary: {dropped:?}"
        );
    }

    #[test]
    fn list_append_absorbed_temp_releases_at_append_boundary() {
        let mut func =
            TirFunction::new("finalizer_scope_list_append".into(), vec![], TirType::None);
        let list = func.fresh_value();
        let item = func.fresh_value();
        for v in [list, item] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops
                .push(original_copy_with_operands("list_new", vec![], vec![list]));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_append",
                vec![list, item],
                vec![],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let append_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_append"))
            .expect("list_append op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(append_idx + 1, item), (marker_idx + 1, list)],
            "list_append absorbs the producer temp but the container owner stays boundary-deferred"
        );
    }

    #[test]
    fn module_set_attr_releases_absorbed_value_before_later_borrowed_use() {
        let mut func = TirFunction::new(
            "finalizer_scope_module_set_attr".into(),
            vec![],
            TirType::None,
        );
        let module = func.fresh_value();
        let name = func.fresh_value();
        let item = func.fresh_value();
        let list = func.fresh_value();
        let popped = func.fresh_value();
        for v in [module, name, item, list, popped] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(op(OpCode::ModuleSetAttr, vec![module, name, list], vec![]));
            b.ops.push(original_copy_with_operands(
                "list_pop",
                vec![list],
                vec![popped],
            ));
            b.ops
                .push(op(OpCode::ModuleDelGlobal, vec![module, name], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let store_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::ModuleSetAttr)
            .expect("module_set_attr op must survive");
        let pop_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_pop"))
            .expect("list_pop op must survive");
        let tracked_drops: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .filter_map(|(idx, op)| {
                let dropped = op.operands[0];
                [item, list, popped]
                    .contains(&dropped)
                    .then_some((idx, dropped))
            })
            .collect();
        assert_eq!(
            tracked_drops,
            vec![
                (list_idx + 1, item),
                (store_idx + 1, list),
                (pop_idx + 1, popped),
            ],
            "module_set_attr owns the Python-visible global lifetime, so the compiler-owned list ref must release at the storage boundary"
        );
    }

    #[test]
    fn generic_attr_store_releases_absorbed_defaults_tuple_before_later_borrowed_use() {
        let mut func = TirFunction::new(
            "finalizer_scope_generic_attr_defaults".into(),
            vec![],
            TirType::None,
        );
        let item = func.fresh_value();
        let func_obj = func.fresh_value();
        let defaults = func.fresh_value();
        let version = func.fresh_value();
        for v in [item, func_obj, defaults, version] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(finalizer_object(func_obj));
            b.ops.push(original_copy_with_operands(
                "tuple_new",
                vec![item],
                vec![defaults],
            ));
            let mut store = op(OpCode::StoreAttr, vec![func_obj, defaults], vec![]);
            store.attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("set_attr_generic_obj".into()),
            );
            store
                .attrs
                .insert("s_value".into(), AttrValue::Str("__defaults__".into()));
            b.ops.push(store);
            b.ops.push(op(
                OpCode::FunctionDefaultsVersion,
                vec![func_obj],
                vec![version],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let tuple_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("tuple_new"))
            .expect("tuple_new op must survive");
        let store_idx = ops
            .iter()
            .position(|op| {
                op.opcode == OpCode::StoreAttr && original_kind(op) == Some("set_attr_generic_obj")
            })
            .expect("generic attr store must survive");
        let version_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::FunctionDefaultsVersion)
            .expect("later borrowed function read must survive");
        let tracked_drops: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .filter_map(|(idx, op)| {
                let dropped = op.operands[0];
                [item, defaults, version]
                    .contains(&dropped)
                    .then_some((idx, dropped))
            })
            .collect();
        let drop_index = |value| {
            tracked_drops
                .iter()
                .find_map(|(idx, dropped)| (*dropped == value).then_some(*idx))
                .expect("tracked owned value must be released")
        };
        assert_eq!(drop_index(item), tuple_idx + 1);
        assert_eq!(
            drop_index(defaults),
            store_idx + 1,
            "generic attr storage retains the value, so the compiler-owned defaults tuple must release at the store boundary before later borrowed function reads"
        );
        assert!(drop_index(version) > version_idx);
        assert!(
            drop_index(defaults) < version_idx,
            "the compiler-owned defaults ref must be released before the later borrowed defaults-version read"
        );
    }

    #[test]
    fn discarded_list_pop_result_releases_at_pop_boundary() {
        let mut func = TirFunction::new("finalizer_scope_list_pop".into(), vec![], TirType::None);
        let item = func.fresh_value();
        let list = func.fresh_value();
        let popped = func.fresh_value();
        for v in [item, list, popped] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(original_copy_with_operands(
                "list_pop",
                vec![list],
                vec![popped],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let pop_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_pop"))
            .expect("list_pop op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![
                (list_idx + 1, item),
                (pop_idx + 1, popped),
                (marker_idx + 1, list),
            ],
            "discarded list_pop result releases at pop boundary while list owner defers"
        );
    }

    #[test]
    fn named_local_absorbed_into_list_is_not_released_at_absorption_boundary() {
        let mut func = TirFunction::new(
            "finalizer_scope_named_local_list".into(),
            vec![],
            TirType::None,
        );
        let item = func.fresh_value();
        let list = func.fresh_value();
        for v in [item, list] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops
                .push(original_copy_with_operands("store_var", vec![item], vec![]));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let list_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
            .expect("list_new op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert!(
            !dropped.contains(&(list_idx + 1, item)),
            "Python-bound local must not drop at the list absorption statement: {dropped:?}"
        );
        assert_eq!(
            dropped,
            vec![(list_idx + 1, list), (marker_idx + 1, item)],
            "the expression container releases at statement last use while the Python-bound local root waits for the frame boundary"
        );
    }

    #[test]
    fn non_finalizer_local_store_releases_at_last_use_not_return_boundary() {
        let mut func = TirFunction::new("ordinary_local_scope".into(), vec![], TirType::None);
        let list = func.fresh_value();
        func.value_types.insert(list, TirType::DynBox);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops
                .push(original_copy_with_operands("list_new", vec![], vec![list]));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let store_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
            .expect("store_var marker must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(store_idx + 1, list)],
            "ordinary local stores must not create a second return-boundary cleanup"
        );
        assert!(
            store_idx < marker_idx,
            "fixture keeps a side-effect marker after the local store"
        );
    }

    #[test]
    fn edge_dying_skips_finalizer_boundary_owned_local_root() {
        let mut func =
            TirFunction::new("finalizer_boundary_edge_exit".into(), vec![], TirType::None);
        let gate = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let item = func.fresh_value();
        let list = func.fresh_value();
        let cond = func.fresh_value();
        let body_arg = func.fresh_value();
        let body_use = func.fresh_value();
        for v in [item, list, body_arg, body_use] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);
        {
            let b = func.blocks.get_mut(&func.entry_block).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(original_copy_with_operands(
                "list_new",
                vec![item],
                vec![list],
            ));
            b.ops
                .push(original_copy_with_operands("store_var", vec![list], vec![]));
            b.terminator = Terminator::Branch {
                target: gate,
                args: vec![],
            };
        }
        func.blocks.insert(
            gate,
            TirBlock {
                id: gate,
                args: vec![],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![list],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![TirValue {
                    id: body_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::Copy, vec![body_arg], vec![body_use])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![op(OpCode::WarnStderr, vec![], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let exit_ops = &func.blocks[&exit].ops;
        let marker_idx = exit_ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("exit marker must survive");
        let dropped: Vec<(usize, ValueId)> = exit_ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert_eq!(
            dropped,
            vec![(marker_idx + 1, list)],
            "edge-dying must not release a finalizer-sensitive local before its return boundary"
        );
    }

    #[test]
    fn explicit_decref_is_the_finalizer_del_boundary() {
        let mut func = TirFunction::new("finalizer_del".into(), vec![], TirType::None);
        let item = func.fresh_value();
        func.value_types.insert(item, TirType::DynBox);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_object(item));
            b.ops.push(op(OpCode::DecRef, vec![item], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            decrefs,
            vec![item],
            "explicit DecRef/`del` consumes the finalizer boundary and must not be duplicated at return"
        );
    }

    #[test]
    fn delete_var_releases_old_slot_at_delete_boundary() {
        let mut func = TirFunction::new(
            "delete_var_finalizer_boundary".into(),
            vec![],
            TirType::None,
        );
        let missing = func.fresh_value();
        let item = func.fresh_value();
        let deleted = func.fresh_value();
        func.value_types.insert(missing, TirType::None);
        func.value_types.insert(item, TirType::DynBox);
        func.value_types.insert(deleted, TirType::None);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            let mut missing_op = op(OpCode::ConstNone, vec![], vec![missing]);
            missing_op
                .attrs
                .insert("_original_kind".into(), AttrValue::Str("missing".into()));
            b.ops.push(missing_op);
            b.ops.push(finalizer_object(item));
            let mut delete = op(OpCode::DeleteVar, vec![missing, item], vec![deleted]);
            delete
                .attrs
                .insert("_var".into(), AttrValue::Str("item".into()));
            b.ops.push(delete);
            b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let ops = &func.blocks[&entry].ops;
        let delete_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::DeleteVar)
            .expect("delete_var op must survive");
        let marker_idx = ops
            .iter()
            .position(|op| op.opcode == OpCode::WarnStderr)
            .expect("marker op must survive");
        let dropped: Vec<(usize, ValueId)> = ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef)
            .map(|(idx, op)| (idx, op.operands[0]))
            .collect();
        assert!(
            dropped.contains(&(delete_idx + 1, item)),
            "delete_var must drop the old occupant immediately after storing missing: {dropped:?}"
        );
        assert!(
            !dropped
                .iter()
                .any(|(idx, value)| *value == item && *idx > marker_idx),
            "old slot occupant must not be deferred past later side effects: {dropped:?}"
        );
    }

    /// Regression (RC drop-insertion substrate, design 20): the real `accumulate`
    /// loop-slot shape from the frontend SimpleIR, run through the FULL pipeline.
    /// The loop loads its carried accumulator via `load_var`→`Copy` every
    /// iteration; a per-SSA-value drop pass double-frees the live accumulator
    /// (the activation blocker — `invalid object header before dec_ref` /
    /// use-after-free at n≥50k). The alias-root-aware drop pass must drop each
    /// underlying heap object EXACTLY ONCE per program point. This test asserts
    /// the no-double-drop invariant directly on the post-pipeline TIR: within any
    /// block, no two `DecRef`s name values that share an alias root.
    #[test]
    fn loop_slot_accumulator_no_double_drop() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::passes::alias_analysis::build_alias_union_find;
        use crate::tir::passes::run_pipeline;
        use crate::tir::type_refine::refine_types;

        let mk = |kind: &str,
                  out: Option<&str>,
                  var: Option<&str>,
                  args: Vec<&str>,
                  val: Option<i64>,
                  sval: Option<&str>| OpIR {
            kind: kind.into(),
            out: out.map(|s| s.to_string()),
            var: var.map(|s| s.to_string()),
            args: if args.is_empty() {
                None
            } else {
                Some(args.iter().map(|s| s.to_string()).collect())
            },
            value: val,
            s_value: sval.map(|s| s.to_string()),
            ..OpIR::default()
        };
        // Shape from tmp/.../native/final_ir/bigint_accumulator__accumulate.txt:
        // total = 1<<60 ; i=0 ; while i<n: total=total+1; total=total-1; total=total+1; i=i+1 ; return total
        let func_ir = FunctionIR {
            name: "diag__accumulate".into(),
            params: vec!["n".into()],
            ops: vec![
                mk("const", Some("v106"), None, vec![], Some(1), None),
                mk("const", Some("v107"), None, vec![], Some(60), None),
                mk(
                    "lshift",
                    Some("v108"),
                    None,
                    vec!["v106", "v107"],
                    None,
                    None,
                ),
                mk("const", Some("v109"), None, vec![], Some(0), None),
                mk("const", Some("v114"), None, vec![], Some(1), None),
                mk("const", Some("v117"), None, vec![], Some(1), None),
                mk("const", Some("v120"), None, vec![], Some(1), None),
                mk("const", Some("v123"), None, vec![], Some(1), None),
                mk(
                    "store_var",
                    None,
                    Some("_bb1_arg0"),
                    vec!["v108"],
                    None,
                    None,
                ),
                mk(
                    "store_var",
                    None,
                    Some("_bb1_arg1"),
                    vec!["v109"],
                    None,
                    None,
                ),
                mk("jump", None, None, vec![], Some(8), None),
                mk("label", None, None, vec![], Some(8), None),
                mk("loop_start", None, None, vec![], None, None),
                mk(
                    "load_var",
                    Some("_v19"),
                    Some("_bb1_arg0"),
                    vec![],
                    None,
                    None,
                ),
                mk(
                    "load_var",
                    Some("_v20"),
                    Some("_bb1_arg1"),
                    vec![],
                    None,
                    None,
                ),
                mk("lt", Some("v112"), None, vec!["_v20", "n"], None, None),
                mk("loop_break_if_false", None, None, vec!["v112"], None, None),
                mk("add", Some("v115"), None, vec!["_v19", "v114"], None, None),
                mk("sub", Some("v118"), None, vec!["v115", "v117"], None, None),
                mk("add", Some("v121"), None, vec!["v118", "v120"], None, None),
                mk("add", Some("v124"), None, vec!["_v20", "v123"], None, None),
                mk(
                    "store_var",
                    None,
                    Some("_bb1_arg0"),
                    vec!["v121"],
                    None,
                    None,
                ),
                mk(
                    "store_var",
                    None,
                    Some("_bb1_arg1"),
                    vec!["v124"],
                    None,
                    None,
                ),
                mk("loop_continue", None, None, vec![], None, None),
                mk("loop_end", None, None, vec![], None, None),
                mk("jump", None, None, vec![], Some(12), None),
                mk("label", None, None, vec![], Some(12), None),
                mk("ret", None, Some("_v19"), vec!["_v19"], None, None),
            ],
            param_types: Some(vec!["Any".into()]),
            source_file: None,
            is_extern: false,
        };

        let mut tir_func = lower_to_tir(&func_ir);
        refine_types(&mut tir_func);
        // Run the full optimization pipeline to reach the realistic lowered loop
        // shape (Copy-aliased loop-slot loads), THEN run drop insertion directly.
        // The pass is a complete primitive but intentionally NOT wired into
        // `build_default_pipeline` yet (Phase-5 native-RC retirement is the
        // remaining activation prerequisite — see the pass_manager activation
        // note), so we invoke it explicitly here to exercise the alias-root
        // placement on the production-shaped IR.
        run_pipeline(
            &mut tir_func,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        {
            let mut am = AnalysisManager::new();
            run(&mut tir_func, &mut am);
        }

        // Invariant: within any block, no two DecRefs share an alias root — a
        // double-drop of one heap object is the activation-blocker use-after-free.
        let aliases = build_alias_union_find(&tir_func);
        for block in tir_func.blocks.values() {
            let mut dropped_roots: HashSet<ValueId> = HashSet::new();
            for op in &block.ops {
                if op.opcode == OpCode::DecRef {
                    let root = aliases.root(op.operands[0]);
                    assert!(
                        dropped_roots.insert(root),
                        "double-drop of alias root {root:?} in one block: {:?}",
                        block.ops
                    );
                }
            }
        }
        // The loop body must drop SOMETHING (the dead intermediates + the prev
        // accumulator) — a fully-inert pass would mean the leak is unclosed.
        let total_decrefs: usize = tir_func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .count();
        assert!(
            total_decrefs >= 2,
            "loop accumulator must insert drops, got {total_decrefs}"
        );
    }

    /// Branch-arg transfer to a successor must NOT be edge-dropped (design §2.5).
    /// Regression for the `while True: break` shape: `v` is computed in `entry`,
    /// passed as a branch arg to `join`, and received as `join`'s block param `p`.
    /// `v`'s ownership transfers to `p` across the edge — the edge-dying rule must
    /// recognize the per-edge transfer (`incoming_arg_roots` via `terminator_arcs`)
    /// and NOT also drop `v` at `join`'s entry. Doing so double-frees the object the param now
    /// owns (the observed `invalid object header before dec_ref` UAF). `p` is then
    /// returned (transferred to the caller), so the function inserts ZERO drops.
    #[test]
    fn branch_arg_transfer_not_edge_dropped() {
        let mut func = TirFunction::new("xfer".into(), vec![], TirType::DynBox);
        let v = func.fresh_value();
        let p = func.fresh_value();
        func.value_types.insert(v, TirType::Str);
        func.value_types.insert(p, TirType::Str);
        let join = func.fresh_block();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(v));
            b.terminator = Terminator::Branch {
                target: join,
                args: vec![v],
            };
        }
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: p,
                    ty: TirType::Str,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![p] },
            },
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // No DecRef of `v` (transferred to `p`), and none of `p` (returned).
        let dropped: Vec<ValueId> = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            !dropped.contains(&v),
            "branch-arg `v` transferred to the successor param must NOT be edge-dropped (double-free); dropped={dropped:?}",
        );
        assert_eq!(
            count_decrefs(&func),
            0,
            "transfer-through-edge + return must insert zero drops; dropped={dropped:?}",
        );
    }

    /// `IterNextUnboxed` writes its value result only on the not-done edge. A
    /// Python-bound local fed from that value may be released through the loop
    /// phi that carries the previous valid element, but the raw value result
    /// itself must not be scheduled for Return-boundary cleanup on the exhausted
    /// edge. Regression for the dict-values/tinygrad UAF:
    /// `DecRef(iter_value)` ran after `done == true`, where the value slot still
    /// held the previous element's stale pointer.
    #[test]
    fn iter_next_unboxed_value_not_return_boundary_dropped_on_exhaustion_edge() {
        let mut func = TirFunction::new(
            "iter_next_unboxed_conditional_value_exit".into(),
            vec![],
            TirType::None,
        );
        let iter = func.fresh_value();
        let initial_local = func.fresh_value();
        let local_phi = func.fresh_value();
        let iter_value = func.fresh_value();
        let done = func.fresh_value();
        let stored_local = func.fresh_value();
        for value in [iter, initial_local, local_phi, iter_value, stored_local] {
            func.value_types.insert(value, TirType::DynBox);
        }
        func.value_types.insert(done, TirType::Bool);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(iter));
            b.ops.push(finalizer_object(initial_local));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![initial_local],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: local_phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(
                    OpCode::IterNextUnboxed,
                    vec![iter],
                    vec![iter_value, done],
                )],
                terminator: Terminator::CondBranch {
                    cond: done,
                    then_block: exit,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![original_copy_with_operands(
                    "store_var",
                    vec![iter_value],
                    vec![stored_local],
                )],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![stored_local],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![op(OpCode::DelBoundary, vec![local_phi], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let exit_drops: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            exit_drops
                .iter()
                .filter(|&&value| value == local_phi)
                .count(),
            1,
            "the valid carried local owner must be released at the loop exit exactly once; exit_drops={exit_drops:?}"
        );
        assert!(
            !exit_drops.contains(&iter_value),
            "the conditionally valid iter value must not be dropped on the \
             exhausted edge; exit_drops={exit_drops:?}",
        );
    }

    /// Straight-line temp: v1 = Call(a); v2 = Call(v1); Return(v2).
    /// v1 dies after op 2 → exactly one DecRef(v1). v2 is returned (transferred)
    /// → not dropped.
    #[test]
    fn straight_line_temp_dropped_once() {
        let mut func = TirFunction::new("sl".into(), vec![], TirType::DynBox);
        let a = func.fresh_value();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();
        for v in [a, v1, v2] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(a));
            b.ops.push(op(OpCode::Call, vec![a], vec![v1]));
            b.ops.push(op(OpCode::Call, vec![v1], vec![v2]));
            b.terminator = Terminator::Return { values: vec![v2] };
        }
        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);
        assert!(stats.ops_added >= 1);
        // a dies after op 1; v1 dies after op 2; v2 is returned. So DecRef(a) and
        // DecRef(v1), not DecRef(v2).
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(decrefs.contains(&a), "a must be dropped at last use");
        assert!(decrefs.contains(&v1), "v1 must be dropped at last use");
        assert!(!decrefs.contains(&v2), "returned value must not be dropped");
        assert!(func.attrs.contains_key(DROP_INSERTED_ATTR));
    }

    /// `list_pop` removes an element from the list and returns a fresh owned
    /// reference to that removed element. If the Python result is discarded, the
    /// returned owner must be released immediately at the pop statement; otherwise
    /// finalizer-bearing elements survive until unrelated container teardown
    /// (`finalizer_container_clear.py`: `bag2.pop()` must run A(11).__del__ before
    /// the following print).
    #[test]
    fn unused_list_pop_result_is_dropped_at_pop_boundary() {
        let mut func = TirFunction::new("list_pop_dead_result".into(), vec![], TirType::DynBox);
        let list = func.fresh_value();
        let idx = func.fresh_value();
        let popped = func.fresh_value();
        let ret = func.fresh_value();
        for v in [list, idx, popped, ret] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(list));
            b.ops.push(op(OpCode::ConstNone, vec![], vec![idx]));
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("list_pop".into()));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![list, idx],
                results: vec![popped],
                attrs,
                source_span: None,
            });
            b.ops.push(op(OpCode::ConstNone, vec![], vec![ret]));
            b.terminator = Terminator::Return { values: vec![ret] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        let pop_idx = ops
            .iter()
            .position(|o| {
                o.opcode == OpCode::Copy
                    && matches!(
                        o.attrs.get("_original_kind"),
                        Some(AttrValue::Str(k)) if k == "list_pop"
                    )
            })
            .expect("list_pop op present");
        let dec_popped_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![popped])
            .expect("unused list_pop result must be dropped");
        assert!(
            dec_popped_idx > pop_idx,
            "list_pop result must be released after the runtime removes/returns it; ops={ops:?}"
        );
    }

    /// `dataclass_new_values` returns the newly-created instance as an owned
    /// reference. Class attachment (`dataclass_set_class`) mutates metadata and
    /// returns `None`, so the constructor result remains the only owner that can
    /// trigger the instance finalizer at function exit. Releasing only field
    /// operands leaks the parent instance and skips child-finalizer teardown.
    #[test]
    fn dataclass_new_values_result_is_dropped_after_last_metadata_use() {
        let mut func = TirFunction::new(
            "dataclass_new_values_owner_drop".into(),
            vec![],
            TirType::DynBox,
        );
        let name = func.fresh_value();
        let fields = func.fresh_value();
        let flags = func.fresh_value();
        let child = func.fresh_value();
        let instance = func.fresh_value();
        let class_obj = func.fresh_value();
        let ret = func.fresh_value();
        for v in [name, fields, flags, child, instance, class_obj, ret] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(name));
            b.ops.push(const_str(fields));
            b.ops.push(op(OpCode::ConstNone, vec![], vec![flags]));
            b.ops.push(op(OpCode::Call, vec![], vec![child]));
            let mut ctor_attrs = AttrDict::new();
            ctor_attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("dataclass_new_values".into()),
            );
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![name, fields, flags, child],
                results: vec![instance],
                attrs: ctor_attrs,
                source_span: None,
            });
            b.ops.push(op(OpCode::Call, vec![], vec![class_obj]));
            let mut set_class_attrs = AttrDict::new();
            set_class_attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("dataclass_set_class".into()),
            );
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![instance, class_obj],
                results: vec![],
                attrs: set_class_attrs,
                source_span: None,
            });
            b.ops.push(op(OpCode::ConstNone, vec![], vec![ret]));
            b.terminator = Terminator::Return { values: vec![ret] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        let set_class_idx = ops
            .iter()
            .position(|o| {
                o.opcode == OpCode::Copy
                    && matches!(
                        o.attrs.get("_original_kind"),
                        Some(AttrValue::Str(k)) if k == "dataclass_set_class"
                    )
            })
            .expect("dataclass_set_class op present");
        let dec_instance_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![instance])
            .expect("dataclass instance result must be dropped");
        assert!(
            dec_instance_idx > set_class_idx,
            "dataclass instance owner must survive metadata attachment and then release; ops={ops:?}"
        );
    }

    /// A CallArgs builder consumed by `call_bind` / `call_indirect` must NOT get
    /// a trailing DecRef: the runtime entry (`molt_call_bind_ic`, via
    /// `PtrDropGuard`) frees the builder internally, so an inserted DecRef would
    /// double-free the `TYPE_ID_CALLARGS` object (design-20 finding #3C: the
    /// method-call `'invalid object header before dec_ref'` abort). The callee
    /// (operand 0) and the call RESULT are still dropped normally.
    #[test]
    fn call_bind_callargs_operand_not_dropped() {
        let mut func = TirFunction::new("cb".into(), vec![], TirType::DynBox);
        let callee = func.fresh_value(); // the bound method (a fresh owned ref)
        let builder = func.fresh_value(); // the CallArgs builder
        let result = func.fresh_value(); // the call result
        for v in [callee, builder, result] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // callee = <fresh owned value> (model as a Call so it is owned).
            b.ops.push(op(OpCode::Call, vec![], vec![callee]));
            // builder = callargs_new (opaque Copy carrying _original_kind).
            let mut ca = AttrDict::new();
            ca.insert(
                "_original_kind".into(),
                AttrValue::Str("callargs_new".into()),
            );
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![builder],
                attrs: ca,
                source_span: None,
            });
            // result = call_bind(callee, builder) — Call carrying _original_kind.
            let mut cb = AttrDict::new();
            cb.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![callee, builder],
                results: vec![result],
                attrs: cb,
                source_span: None,
            });
            b.terminator = Terminator::Return {
                values: vec![result],
            };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            !decrefs.contains(&builder),
            "the CallArgs builder is consumed by call_bind; it must NOT be DecRef'd (double-free)"
        );
        assert!(
            decrefs.contains(&callee),
            "the callee (borrowed-then-dead) must be dropped at its last use (the call)"
        );
        // The result is returned → not dropped here.
        assert!(!decrefs.contains(&result));
    }

    /// Interior-borrow keepalive (round-6 BLOCKER-1). A heap object's LAST DIRECT
    /// operand use is a `LoadAttr` that extracts a value the object's backing store
    /// owns (the `Counter._handle` raw-int registry-handle shape: the wrapper's
    /// finalizer destroys the registry entry the handle indexes). The extracted
    /// value `h` is then consumed by a later `Call`. The source object `obj` MUST be
    /// dropped AFTER `h`'s last use (the Call), NEVER right after the `LoadAttr` —
    /// dropping it earlier runs the finalizer and invalidates `h` (the observed UAF:
    /// `len(Counter(...))` returned 0). Mirrors the de-sugared fast-path lowering
    /// `h = get_attr(counts, "_handle"); molt_counter_len(h)`.
    #[test]
    fn loadattr_source_kept_alive_through_borrow_result_use() {
        let mut func = TirFunction::new("borrow".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value(); // the wrapper (fresh owned)
        let h = func.fresh_value(); // LoadAttr(obj) — borrows into obj's store
        let len_fn = func.fresh_value(); // the `molt_counter_len` builtin
        let res = func.fresh_value(); // Call(len_fn, h) result
        for v in [obj, h, len_fn, res] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(obj)); // op 0: obj = fresh owned
            b.ops.push(op(OpCode::LoadAttr, vec![obj], vec![h])); // op 1: h = obj._handle (last DIRECT use of obj)
            b.ops.push(const_str(len_fn)); // op 2: the builtin
            b.ops.push(op(OpCode::Call, vec![len_fn, h], vec![res])); // op 3: len(h) — needs obj alive
            b.terminator = Terminator::Return { values: vec![res] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        // Find the Call (the consumer of the borrow result) and the DecRef(obj).
        let call_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call)
            .expect("call present");
        let decref_obj_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![obj]);
        assert!(
            decref_obj_idx.is_some(),
            "source object must still be dropped (no leak); ops={ops:?}"
        );
        assert!(
            decref_obj_idx.unwrap() > call_idx,
            "source object must be dropped AFTER the borrow result's consuming Call \
             (interior-borrow keepalive), not at its last direct operand use; \
             decref@{:?} call@{call_idx} ops={ops:?}",
            decref_obj_idx.unwrap(),
        );
    }

    /// Interior-borrow keepalive across a transparent `Copy` of the source (the
    /// `load_var` shape): `obj` is loaded via a `Copy` (alias root = obj), the alias
    /// feeds a `LoadAttr`, and the LoadAttr result is consumed later. The drop of
    /// the underlying object (alias root) must still be deferred past the consumer.
    #[test]
    fn loadattr_keepalive_through_copy_aliased_source() {
        let mut func = TirFunction::new("borrow_alias".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
        let obj_alias = func.fresh_value(); // Copy(obj) — load_var alias
        let h = func.fresh_value(); // LoadAttr(obj_alias)
        let consumer = func.fresh_value(); // Call(h) result
        for v in [obj, obj_alias, h, consumer] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(obj));
            b.ops.push({
                let mut o = op(OpCode::Copy, vec![obj], vec![obj_alias]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                o
            });
            b.ops.push(op(OpCode::LoadAttr, vec![obj_alias], vec![h]));
            b.ops.push(op(OpCode::Call, vec![h], vec![consumer]));
            b.terminator = Terminator::Return {
                values: vec![consumer],
            };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        let call_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call)
            .expect("call present");
        // The underlying object is released through some alias of its root, exactly
        // once, AFTER the consumer. Find any DecRef whose operand aliases obj's root.
        let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
        let obj_root = aliases.root(obj);
        let decref_positions: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.opcode == OpCode::DecRef
                    && o.operands
                        .first()
                        .is_some_and(|&v| aliases.root(v) == obj_root)
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            decref_positions.len(),
            1,
            "the source object's group must be released exactly once; ops={ops:?}"
        );
        assert!(
            decref_positions[0] > call_idx,
            "source object drop must follow the borrow result's consumer; \
             decref@{} call@{call_idx} ops={ops:?}",
            decref_positions[0],
        );
    }

    /// Raw i64 values get ZERO drops (perf contract / design R3).
    #[test]
    fn raw_i64_gets_no_drops() {
        let mut func = TirFunction::new("raw".into(), vec![], TirType::I64);
        let c0 = func.fresh_value();
        let c1 = func.fresh_value();
        let s = func.fresh_value();
        for v in [c0, c1, s] {
            func.value_types.insert(v, TirType::I64);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            let mut a0 = AttrDict::new();
            a0.insert("value".into(), AttrValue::Int(3));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![c0],
                attrs: a0,
                source_span: None,
            });
            let mut a1 = AttrDict::new();
            a1.insert("value".into(), AttrValue::Int(4));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![c1],
                attrs: a1,
                source_span: None,
            });
            b.ops.push(op(OpCode::Add, vec![c0, c1], vec![s]));
            b.terminator = Terminator::Return { values: vec![s] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        assert_eq!(count_decrefs(&func), 0, "raw i64 lane must get zero drops");
    }

    /// StackAlloc values get ZERO drops (design R6).
    #[test]
    fn stack_alloc_gets_no_drops() {
        let mut func = TirFunction::new("st".into(), vec![], TirType::DynBox);
        let s = func.fresh_value();
        let used = func.fresh_value();
        func.value_types.insert(s, TirType::DynBox);
        func.value_types.insert(used, TirType::DynBox);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::StackAlloc, vec![], vec![s]));
            b.ops.push(op(OpCode::LoadAttr, vec![s], vec![used]));
            b.terminator = Terminator::Return { values: vec![used] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(!decrefs.contains(&s), "stack value must never be dropped");
    }

    /// A lowered coroutine `_poll` STATE MACHINE (a `StateSwitch` dispatch) must
    /// get ZERO drops — the pass bails (`has_state_machine`). Regression for the
    /// LLVM verifier failure where a drop placed in a state-resume block
    /// referenced a value defined only on the non-taken first-entry path
    /// (`dec_ref %v` before `%v = ...`; a use-before-def that also double-frees on
    /// native). A generator can carry `StateSwitch` WITHOUT `StateBlock*`
    /// delimiters, so the handler bail alone misses it.
    #[test]
    fn state_machine_function_gets_no_drops() {
        let mut func = TirFunction::new("poll".into(), vec![], TirType::DynBox);
        let st = func.fresh_value();
        let v = func.fresh_value();
        func.value_types.insert(st, TirType::I64);
        func.value_types.insert(v, TirType::Str);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // A state-machine dispatch op marks this as a lowered `_poll` body.
            b.ops.push(op(OpCode::StateSwitch, vec![st], vec![]));
            // A heap temp whose naive last-use drop would be unsound over the
            // re-entrant state CFG.
            b.ops.push(const_str(v));
            b.ops.push(op(OpCode::Call, vec![v], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }
        assert!(
            func.has_state_machine(),
            "fixture must look like a lowered state machine",
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        assert_eq!(
            count_decrefs(&func),
            0,
            "state-machine `_poll` body must get zero drops (pass bails)",
        );
        assert_eq!(count_increfs(&func), 0);
    }

    /// Loop-carried phi `s` used on BOTH the loop body (new value computed, old
    /// `s` dead on the back-edge path) AND the exit path (a non-alias consumer),
    /// in the real-phi (LLVM) shape. The header phi must be dropped on the path
    /// where it dies — the back-edge body block — exactly once. Regression for the
    /// LLVM string-concat leak: the drop pass inserted NO `DecRef(s_phi)` for this
    /// shape (the accumulator's old value leaked every iteration: `dealloc=5/n`).
    ///
    /// Shape (mirrors `string_concat__concat` after lowering):
    ///   entry: s0 = ConstStr; br header(s0)
    ///   header(s_phi): cond_br c, body, exit
    ///   body: s_new = Add(s_phi, "x"); br header(s_new)   // old s_phi dies here
    ///   exit: r = Len(s_phi); return r                    // s_phi consumed, dies
    #[test]
    fn loop_carried_phi_dropped_on_backedge() {
        let mut func = TirFunction::new("acc".into(), vec![], TirType::I64);
        let s0 = func.fresh_value();
        let s_phi = func.fresh_value();
        let s_alias = func.fresh_value();
        let lit = func.fresh_value();
        let cond = func.fresh_value();
        let s_new = func.fresh_value();
        let r = func.fresh_value();
        func.value_types.insert(s0, TirType::Str);
        func.value_types.insert(s_phi, TirType::Str);
        func.value_types.insert(s_alias, TirType::Str);
        func.value_types.insert(lit, TirType::Str);
        func.value_types.insert(cond, TirType::Bool);
        func.value_types.insert(s_new, TirType::Str);
        func.value_types.insert(r, TirType::I64);

        // Mirror the lowered `string_concat__concat` CFG precisely: the cond lives
        // in a SEPARATE block (`cond_blk`, real bb3) reached from the header, and a
        // transparent `Copy` of the phi (`s_alias`, real `%11 = copy %9`) is the
        // value actually consumed on BOTH the loop body and the exit paths. The
        // exit goes through an intermediate `pre_exit` block (real bb6). This is
        // the shape the simpler direct-header-cond fixture did NOT reproduce.
        let header = func.fresh_block();
        let cond_blk = func.fresh_block();
        let body = func.fresh_block();
        let pre_exit = func.fresh_block();
        let exit = func.fresh_block();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(s0));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![s0],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: s_phi,
                    ty: TirType::Str,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: cond_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            cond_blk,
            TirBlock {
                id: cond_blk,
                args: vec![],
                // `s_alias = Copy(s_phi)` — a transparent alias (root = s_phi) used by
                // both successors; plus the loop condition.
                ops: vec![
                    op(OpCode::Copy, vec![s_phi], vec![s_alias]),
                    op(OpCode::ConstBool, vec![], vec![cond]),
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: pre_exit,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    const_str(lit),
                    op(OpCode::Add, vec![s_alias, lit], vec![s_new]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![s_new],
                },
            },
        );
        func.blocks.insert(
            pre_exit,
            TirBlock {
                id: pre_exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![],
                },
            },
        );
        // A fresh (non-alias) consumer of the aliased phi → it dies after it.
        // `Call` borrows its operand and returns a fresh owned value (the real IR
        // uses a `len`-carrying op here; the only property that matters for
        // liveness is that the result is NOT a transparent alias).
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![op(OpCode::Call, vec![s_alias], vec![r])],
                terminator: Terminator::Return { values: vec![r] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        // The header phi `s_phi` (and the literal `lit`) are owned heap values that
        // die — `s_phi` on the back-edge body path and on the exit path, `lit`
        // after the Add. The pass MUST drop the accumulator; a fully-inert result
        // is the leak. Assert `s_phi` is dropped somewhere.
        let dropped: HashSet<ValueId> = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            dropped.contains(&s_phi),
            "loop-carried phi accumulator must be dropped (else it leaks every \
             iteration); drops={dropped:?}",
        );
        // And no double-drop of any root within a single block.
        let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
        for block in func.blocks.values() {
            let mut roots: HashSet<ValueId> = HashSet::new();
            for o in &block.ops {
                if o.opcode == OpCode::DecRef {
                    assert!(
                        roots.insert(aliases.root(o.operands[0])),
                        "double-drop in one block: {:?}",
                        block.ops,
                    );
                }
            }
        }
    }

    /// Parameters are borrowed — never dropped.
    /// `IterNextUnboxed` writes its value result only on the not-done edge. A
    /// following `store_var` makes that result look like a Python local-store
    /// root, but the done edge reaches loop exit with the value slot
    /// uninitialized. The local lifetime rail must therefore not schedule an
    /// unconditional return-boundary `DecRef(value)` in the exit block.
    #[test]
    fn iter_next_unboxed_del_boundary_not_dropped_on_done_return_boundary() {
        let mut func = TirFunction::new(
            "iter_next_conditional_local_boundary".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let iter = func.blocks[&func.entry_block].args[0].id;
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let seed = func.fresh_value();
        let current = func.fresh_value();
        let value = func.fresh_value();
        let done = func.fresh_value();
        let stored = func.fresh_value();
        for v in [seed, current, value, stored] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(done, TirType::Bool);

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_str(seed));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![seed],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: current,
                    ty: TirType::DynBox,
                }],
                ops: vec![op(OpCode::IterNextUnboxed, vec![iter], vec![value, done])],
                terminator: Terminator::CondBranch {
                    cond: done,
                    then_block: exit,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![original_copy_with_operands(
                    "store_var",
                    vec![value],
                    vec![stored],
                )],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![value],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![
                    op(OpCode::WarnStderr, vec![], vec![]),
                    op(OpCode::DelBoundary, vec![value], vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let exit_decrefs: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !exit_decrefs.contains(&value),
            "done-edge return must not drop the conditionally-valid iterator value; exit drops={exit_decrefs:?}"
        );
        assert!(
            func.blocks[&exit]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::DelBoundary),
            "drop insertion must consume DelBoundary markers even when the safe action is deletion"
        );
    }

    #[test]
    fn params_not_dropped() {
        let mut func = TirFunction::new("p".into(), vec![TirType::Str], TirType::DynBox);
        let p0 = ValueId(0);
        let r = func.fresh_value();
        func.value_types.insert(r, TirType::Str);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::Call, vec![p0], vec![r]));
            b.terminator = Terminator::Return { values: vec![r] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(!decrefs.contains(&p0), "parameter must not be dropped");
    }

    /// Borrow inference: a value whose only use is a call argument and is dead
    /// after the call is dropped AFTER the call (last-use), never before.
    #[test]
    fn borrow_into_call_dropped_after() {
        let mut func = TirFunction::new("bc".into(), vec![], TirType::DynBox);
        let x = func.fresh_value();
        let res = func.fresh_value();
        let out = func.fresh_value();
        for v in [x, res, out] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(x));
            b.ops.push(op(OpCode::Call, vec![x], vec![res]));
            b.ops.push(op(OpCode::Call, vec![res], vec![out]));
            b.terminator = Terminator::Return { values: vec![out] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // x's last use is op 1 (the call). DecRef(x) must come AFTER op 1, before
        // the next op. Find the index of DecRef(x) and assert it follows the call.
        let ops = &func.blocks[&entry].ops;
        let call_x_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call && o.operands == vec![x])
            .unwrap();
        let decref_x_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![x]);
        assert!(decref_x_idx.is_some(), "x dropped at last use");
        assert!(decref_x_idx.unwrap() > call_x_idx, "drop AFTER the call");
    }

    /// Generator yield: a value live across the yield gets an IncRef before it.
    #[test]
    fn yield_increfs_live_across() {
        let mut func = TirFunction::new("g".into(), vec![], TirType::DynBox);
        let header = func.entry_block;
        let resume = func.fresh_block();
        let x = func.fresh_value();
        let yval = func.fresh_value();
        let used = func.fresh_value();
        for v in [x, yval, used] {
            func.value_types.insert(v, TirType::Str);
        }
        {
            let b = func.blocks.get_mut(&header).unwrap();
            b.ops.push(const_str(x));
            b.ops.push(const_str(yval));
            // Yield: x is live across (used in resume), yval is the yielded value.
            b.ops.push(op(OpCode::Yield, vec![yval], vec![]));
            b.terminator = Terminator::Branch {
                target: resume,
                args: vec![],
            };
        }
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![],
                ops: vec![op(OpCode::Call, vec![x], vec![used])],
                terminator: Terminator::Return { values: vec![used] },
            },
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // x must be IncRef'd before the Yield (it survives into the frame).
        let header_ops = &func.blocks[&header].ops;
        let yield_idx = header_ops
            .iter()
            .position(|o| o.opcode == OpCode::Yield)
            .unwrap();
        let incref_x_before = header_ops[..yield_idx]
            .iter()
            .any(|o| o.opcode == OpCode::IncRef && o.operands == vec![x]);
        assert!(incref_x_before, "live-across-yield value must be IncRef'd");
        assert!(count_increfs(&func) >= 1);
    }

    /// Loop accumulator: a heap accumulator threaded through a header block arg
    /// and updated on the back-edge gets a drop for the dead previous value, and
    /// the loop-exit value is dropped (dead after the loop).
    #[test]
    fn loop_accumulator_dropped() {
        let mut func = TirFunction::new("loop".into(), vec![], TirType::DynBox);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let acc0 = func.fresh_value();
        let acc_phi = func.fresh_value();
        let cond = func.fresh_value();
        let acc_next = func.fresh_value();
        for v in [acc0, acc_phi, acc_next] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(acc0));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![acc0],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: acc_phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
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
                // acc_next = Call(acc_phi): consumes the phi, produces a new owned acc.
                ops: vec![op(OpCode::Call, vec![acc_phi], vec![acc_next])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![acc_next],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                // The final acc_phi is dead (not returned).
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The loop-exit value (acc_phi, live-out of header into exit but dead in
        // exit) must be dropped at the exit block entry (edge-dying rule).
        let exit_decrefs: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            exit_decrefs.contains(&acc_phi),
            "loop-exit dead accumulator must be dropped at exit entry; got {exit_decrefs:?}"
        );
    }

    #[test]
    fn explicit_del_boundary_root_not_edge_dropped_at_loop_exit() {
        let mut func = TirFunction::new("explicit_boundary_loop".into(), vec![], TirType::None);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let stale_release_root = func.fresh_value();
        let current_seed = func.fresh_value();
        let current_phi = func.fresh_value();
        let cond = func.fresh_value();
        let next_value = func.fresh_value();
        let next_slot = func.fresh_value();
        for v in [
            stale_release_root,
            current_seed,
            current_phi,
            next_value,
            next_slot,
        ] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(stale_release_root));
            b.ops.push(const_str(current_seed));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![current_seed],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: current_phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        let mut body_boundary = op(OpCode::DelBoundary, vec![stale_release_root], vec![]);
        body_boundary
            .attrs
            .insert("s_value".into(), AttrValue::Str("value".into()));
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    body_boundary,
                    const_str(next_value),
                    original_copy_with_operands("store_var", vec![next_value], vec![next_slot]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_slot],
                },
            },
        );
        let mut exit_boundary = op(OpCode::DelBoundary, vec![current_phi], vec![]);
        exit_boundary
            .attrs
            .insert("s_value".into(), AttrValue::Str("value".into()));
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![exit_boundary],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let body_drops: Vec<ValueId> = func.blocks[&body]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            body_drops.contains(&stale_release_root),
            "the explicit body boundary must remain the release authority; drops={body_drops:?}"
        );
        let exit_entry_drops: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .take_while(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !exit_entry_drops.contains(&stale_release_root),
            "a path-conditioned explicit release root must not also be edge-dropped at the loop exit; drops={exit_entry_drops:?}"
        );
    }

    #[test]
    fn explicit_del_boundary_splits_shared_return_keep_path_release() {
        let mut func = TirFunction::new("explicit_boundary_diamond".into(), vec![], TirType::None);
        let del_path = func.fresh_block();
        let keep_path = func.fresh_block();
        let exit = func.fresh_block();
        let owner = func.fresh_value();
        let stored = func.fresh_value();
        let cond = func.fresh_value();
        for v in [owner, stored] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(owner));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![owner],
                vec![stored],
            ));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: del_path,
                then_args: vec![],
                else_block: keep_path,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            del_path,
            TirBlock {
                id: del_path,
                args: vec![],
                ops: vec![op(OpCode::DelBoundary, vec![stored], vec![])],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            keep_path,
            TirBlock {
                id: keep_path,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![],
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

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let del_drops: Vec<ValueId> = func.blocks[&del_path]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            del_drops,
            vec![owner],
            "the explicit del path must keep exactly its boundary release"
        );
        let exit_drops: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !exit_drops.contains(&owner),
            "the shared return block cannot drop a root already released on another incoming path"
        );
        let keep_split_releases = func.blocks.iter().any(|(&bid, block)| {
            bid != entry
                && bid != del_path
                && bid != keep_path
                && bid != exit
                && block
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![owner])
                && matches!(
                    &block.terminator,
                    Terminator::Branch { target, args }
                        if *target == exit && args.is_empty()
                )
        });
        assert!(
            keep_split_releases,
            "the keep path must get an edge-local release for the owner skipped by the del path"
        );
    }

    #[test]
    fn explicit_del_boundary_join_before_return_splits_keep_edge() {
        let mut func = TirFunction::new(
            "explicit_boundary_join_before_return".into(),
            vec![],
            TirType::None,
        );
        let del_path = func.fresh_block();
        let keep_path = func.fresh_block();
        let join = func.fresh_block();
        let exit = func.fresh_block();
        let owner = func.fresh_value();
        let stored = func.fresh_value();
        let cond = func.fresh_value();
        for v in [owner, stored] {
            func.value_types.insert(v, TirType::DynBox);
        }
        func.value_types.insert(cond, TirType::Bool);

        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(finalizer_call_bind(owner));
            b.ops.push(original_copy_with_operands(
                "store_var",
                vec![owner],
                vec![stored],
            ));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: del_path,
                then_args: vec![],
                else_block: keep_path,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            del_path,
            TirBlock {
                id: del_path,
                args: vec![],
                ops: vec![op(OpCode::DelBoundary, vec![stored], vec![])],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            keep_path,
            TirBlock {
                id: keep_path,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![],
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

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let del_drops: Vec<ValueId> = func.blocks[&del_path]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(
            del_drops,
            vec![owner],
            "the explicit del path must keep exactly its boundary release"
        );
        for (block_id, label) in [(join, "join"), (exit, "return")] {
            let drops: Vec<ValueId> = func.blocks[&block_id]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::DecRef)
                .map(|op| op.operands[0])
                .collect();
            assert!(
                !drops.contains(&owner),
                "the {label} block cannot drop a root already released on one incoming history: {drops:?}"
            );
        }
        let keep_split_releases = func.blocks.iter().any(|(&bid, block)| {
            bid != entry
                && bid != del_path
                && bid != keep_path
                && bid != join
                && bid != exit
                && block
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![owner])
                && matches!(
                    &block.terminator,
                    Terminator::Branch { target, args }
                        if *target == join && args.is_empty()
                )
        });
        assert!(
            keep_split_releases,
            "the keep path must release the owner before histories merge at the join"
        );
    }

    /// Mixed-ownership phi, INCOMING side (§5 retain). A loop accumulator phi is
    /// seeded on the loop-ENTRY edge with a transparent alias of a BORROWED
    /// parameter (`x = base`), and updated on the back-edge with a fresh owned
    /// value. Because the loop body drops the phi each iteration, the borrowed
    /// entry value must be RETAINED on the entry edge (before the preheader's
    /// terminator) so the phi uniformly owns a `+1`. The back-edge's fresh owned
    /// value must NOT be retained (that would leak the accumulator each iteration).
    #[test]
    fn mixed_phi_borrowed_param_retained_on_entry_edge() {
        // param `base` (id 0), preheader binds the accumulator phi to Copy(base).
        let mut func = TirFunction::new("apply".into(), vec![TirType::Str], TirType::DynBox);
        let base = ValueId(0);
        let pre = func.fresh_block(); // preheader
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let x0 = func.fresh_value(); // Copy(base) — borrowed alias seeding the phi
        let acc_phi = func.fresh_value();
        let load_x = func.fresh_value(); // Copy(acc_phi) in body
        let cond = func.fresh_value();
        let acc_next = func.fresh_value(); // fresh owned (Call result)
        for v in [x0, acc_phi, load_x, acc_next] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.terminator = Terminator::Branch {
                target: pre,
                args: vec![],
            };
        }
        // preheader: x0 = copy_var(base) → transparent alias of the param.
        func.blocks.insert(
            pre,
            TirBlock {
                id: pre,
                args: vec![],
                ops: vec![{
                    let mut o = op(OpCode::Copy, vec![base], vec![x0]);
                    o.attrs
                        .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                    o
                }],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![x0],
                },
            },
        );
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: acc_phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
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
                ops: vec![
                    {
                        let mut o = op(OpCode::Copy, vec![acc_phi], vec![load_x]);
                        o.attrs
                            .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                        o
                    },
                    // acc_next = Call(load_x, base): fresh owned, reads base each iter.
                    op(OpCode::Call, vec![load_x, base], vec![acc_next]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![acc_next],
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
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The preheader must IncRef the borrowed `x0` (alias of the param) before
        // its terminator — the entry-edge retain.
        let pre_increfs: Vec<ValueId> = func.blocks[&pre]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            pre_increfs.contains(&x0),
            "borrowed param alias seeding the loop phi must be retained on the entry edge; got {pre_increfs:?}"
        );
        // The back-edge (body) must NOT retain the fresh owned `acc_next` — that
        // would leak one accumulator per iteration.
        let body_increfs: Vec<ValueId> = func.blocks[&body]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            !body_increfs.contains(&acc_next),
            "fresh owned back-edge value must NOT be retained (would leak); got {body_increfs:?}"
        );
        // The param itself is never dropped (borrowed ABI).
        let any_decref_base = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .any(|o| o.opcode == OpCode::DecRef && o.operands == vec![base]);
        assert!(!any_decref_base, "parameter must never be directly dropped");
    }

    /// Mixed-ownership phi, OUTGOING side (§3 incoming-arg exclusion). An owned
    /// value is FORWARDED as a branch arg into a join block's phi through a
    /// multi-block chain (the shape the inliner produces). The value's ownership
    /// transfers INTO the phi, so it must NOT be edge-dropped at the join entry —
    /// the phi is released by its own last-use drop. A spurious join-entry drop
    /// plus the phi's drop is a double-free.
    #[test]
    fn forwarded_owned_value_not_edge_dropped_at_join() {
        let mut func = TirFunction::new("fwd".into(), vec![], TirType::DynBox);
        let mid = func.fresh_block();
        let join = func.fresh_block();
        let owned = func.fresh_value(); // fresh owned (ConstStr)
        let fwd = func.fresh_value(); // Copy(owned) — alias forwarded to the phi
        let phi = func.fresh_value();
        let used = func.fresh_value();
        for v in [owned, fwd, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.terminator = Terminator::Branch {
                target: mid,
                args: vec![],
            };
        }
        func.blocks.insert(
            mid,
            TirBlock {
                id: mid,
                args: vec![],
                ops: vec![const_str(owned), {
                    let mut o = op(OpCode::Copy, vec![owned], vec![fwd]);
                    o.attrs
                        .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                    o
                }],
                // Forward `fwd` (owned, via alias) into the join's phi.
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![fwd],
                },
            },
        );
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::Call, vec![phi], vec![used])],
                terminator: Terminator::Return { values: vec![used] },
            },
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The forwarded owned value (`fwd`, alias root `owned`) must NOT be dropped
        // at the join entry — it transferred into the phi. The phi's own last-use
        // drop (after the Call) releases the object exactly once.
        let join_entry_decrefs: Vec<ValueId> = func.blocks[&join]
            .ops
            .iter()
            .take_while(|o| o.opcode == OpCode::DecRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            !join_entry_decrefs.contains(&fwd) && !join_entry_decrefs.contains(&owned),
            "forwarded owned value must not be edge-dropped at the join; got {join_entry_decrefs:?}"
        );
        // Exactly one DecRef releases the group (the phi at its last use in join).
        let total_group_decrefs = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| {
                o.opcode == OpCode::DecRef
                    && o.operands
                        .first()
                        .is_some_and(|&v| v == fwd || v == owned || v == phi)
            })
            .count();
        assert_eq!(
            total_group_decrefs, 1,
            "the owned forwarded group must be released exactly once, not double-freed"
        );
    }

    #[test]
    fn phi_edge_clean_transfer_ignores_release_on_other_branch() {
        let mut func = TirFunction::new("branch_or_phi".into(), vec![], TirType::None);
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let join = func.fresh_block();
        let source = func.fresh_value(); // fresh owned result used by both arms
        let then_alias = func.fresh_value(); // transparent alias forwarded to the phi
        let fallback = func.fresh_value();
        let selected = func.fresh_value(); // `source or fallback` result on else arm
        let phi = func.fresh_value();
        for v in [source, then_alias, fallback, selected, phi] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::Call, vec![], vec![source]));
            b.terminator = Terminator::CondBranch {
                cond: source,
                then_block,
                then_args: vec![],
                else_block,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            then_block,
            TirBlock {
                id: then_block,
                args: vec![],
                ops: vec![{
                    let mut o = op(OpCode::Copy, vec![source], vec![then_alias]);
                    o.attrs
                        .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                    o
                }],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![then_alias],
                },
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![],
                ops: vec![
                    const_str(fallback),
                    op(OpCode::Or, vec![source, fallback], vec![selected]),
                ],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![selected],
                },
            },
        );
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::Call, vec![phi], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        let then_increfs: Vec<ValueId> = func.blocks[&then_block]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            then_increfs.is_empty(),
            "release planning is path-sensitive: an else-arm release must not \
             force a retain on the clean-transfer then edge, got {then_increfs:?}"
        );
    }

    /// Mixed-ownership phi, CRITICAL-EDGE SPLIT (§5 ambiguous-arc retain; round-4
    /// Finding 2). When a predecessor reaches an OWNED phi via MORE THAN ONE arc
    /// with DIFFERENT args, a before-terminator IncRef would wrongly fire on the
    /// other arc, so the pass SPLITS the critical edge: it inserts a fresh block
    /// holding the edge-exact `IncRef` + an unconditional `Branch` to the target,
    /// and retargets exactly that arc to the new block. This is the only path that
    /// allocates a block (it is why the pass is `Mutates::Cfg`), and it shipped
    /// with ZERO coverage before this test.
    ///
    /// Shape: `entry` ends in a `Switch` whose case-0 and DEFAULT arcs BOTH target
    /// `join` but forward DIFFERENT args into `join`'s single owned phi — case-0
    /// forwards a transparent alias of the borrowed param `base` (BORROWED → must
    /// retain), default forwards a freshly minted owned `ConstStr` (clean transfer
    /// → no retain). `join` consumes the phi (a `Call`) and returns nothing, so the
    /// phi is dropped and the borrowed case-0 edge needs its `+1`. Because case-0
    /// and default both go to `join`, the retain cannot be placed before `entry`'s
    /// terminator (it would also fire on the default arc); it must be split onto
    /// the case-0 arc.
    #[test]
    fn mixed_phi_critical_edge_split_inserts_fresh_incref_block() {
        // param `base` (id 0): borrowed heap Str.
        let mut func = TirFunction::new("split".into(), vec![TirType::Str], TirType::DynBox);
        let base = ValueId(0);
        let join = func.fresh_block();
        let case0_alias = func.fresh_value(); // Copy(base) — borrowed alias (case 0 arg)
        let sel = func.fresh_value(); // Switch selector (raw)
        let fresh_owned = func.fresh_value(); // ConstStr — fresh owned (default arg)
        let phi = func.fresh_value(); // join's owned obj-lane phi
        let used = func.fresh_value(); // Call(phi) result
        for v in [case0_alias, fresh_owned, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(sel, TirType::I64);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // case0_alias = copy_var(base): a transparent (borrowed) alias of the
            // param; fresh_owned = ConstStr: a freshly minted owned value; sel: the
            // raw Switch selector.
            b.ops.push({
                let mut o = op(OpCode::Copy, vec![base], vec![case0_alias]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            });
            b.ops.push(const_str(fresh_owned));
            b.ops.push(op(OpCode::ConstInt, vec![], vec![sel]));
            // Switch: case 0 → join(case0_alias); default → join(fresh_owned).
            // TWO arcs to `join` with DIFFERENT args ⇒ a critical edge.
            b.terminator = Terminator::Switch {
                value: sel,
                cases: vec![(0, join, vec![case0_alias])],
                default: join,
                default_args: vec![fresh_owned],
            };
        }
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: phi,
                    ty: TirType::Str,
                }],
                // Consume the phi (drops it at its last use) and return nothing so the
                // phi dies in `join` — the case-0 borrowed edge therefore needs a +1.
                ops: vec![op(OpCode::Call, vec![phi], vec![used])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let n_blocks_before = func.blocks.len();
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        // A fresh block must have been inserted by the critical-edge split.
        assert!(
            func.blocks.len() > n_blocks_before,
            "the critical-edge split must allocate a fresh block; before={n_blocks_before} after={}",
            func.blocks.len()
        );

        // `entry`'s case-0 arc must now target a NEW block (not `join`): the retarget.
        // The default arc must still go to `join` (unsplit, clean transfer).
        let (case0_target, default_target) = match &func.blocks[&entry].terminator {
            Terminator::Switch { cases, default, .. } => (cases[0].1, *default),
            other => panic!("entry terminator must remain a Switch, got {other:?}"),
        };
        assert_ne!(
            case0_target, join,
            "the borrowed case-0 arc must be retargeted away from `join` to the split block"
        );
        assert_eq!(
            default_target, join,
            "the clean-transfer default arc must stay pointed at `join` (not split)"
        );

        // The split block must (a) hold an IncRef of the borrowed alias `case0_alias`
        // and (b) branch unconditionally to `join` forwarding that same arg.
        let split = &func.blocks[&case0_target];
        let split_increfs: Vec<ValueId> = split
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            split_increfs.contains(&case0_alias),
            "the split block must retain (IncRef) the borrowed case-0 value; got {split_increfs:?}"
        );
        match &split.terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(
                    *target, join,
                    "split block must branch to the original target"
                );
                assert_eq!(
                    args,
                    &vec![case0_alias],
                    "split block must forward the case-0 arg it took over"
                );
            }
            other => panic!("split block must end in an unconditional Branch, got {other:?}"),
        }

        // The default (clean-transfer, freshly owned) value must NOT be retained
        // anywhere — retaining it would leak.
        let any_incref_fresh = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .any(|o| o.opcode == OpCode::IncRef && o.operands.first() == Some(&fresh_owned));
        assert!(
            !any_incref_fresh,
            "the clean-transfer default value must not be retained (would leak)"
        );

        // The split result must be a valid CFG: re-run the analysis self-check over
        // the post-split function (mirrors MOLT_VERIFY_ANALYSIS=1) — a malformed
        // split (dangling edge / unreachable target) would diverge the recomputed
        // dominators from a fresh build.
        let mut verify_am = AnalysisManager::new();
        let preds = crate::tir::dominators::build_pred_map_with(
            &func,
            crate::tir::dominators::CfgEdgePolicy::Full,
        );
        let reachable = crate::tir::dominators::reachable_blocks_with(
            &func,
            crate::tir::dominators::CfgEdgePolicy::Full,
        );
        assert!(
            reachable.contains(&case0_target),
            "the split block must be reachable from entry"
        );
        assert!(
            preds.get(&join).is_some_and(|p| p.contains(&case0_target)),
            "the split block must be a predecessor of the original target"
        );
        // Liveness recomputes cleanly over the mutated CFG (would panic on a
        // use-before-def introduced by a bad split).
        let _ = verify_am.get::<TirLiveness>(&func).clone();
    }

    /// FINDING 3 (round-4) fail-closed pin. `incoming_arg_roots` keys on alias
    /// ROOT over ALL predecessors, so a root forwarded into a join's phi by ANY
    /// predecessor is excluded from that join's edge-dying drop on EVERY path.
    /// This test pins the load-bearing invariant the imprecision must preserve:
    /// the exclusion can only ever LEAK, NEVER double-free (over-release → UAF).
    ///
    /// Shape (a diamond where the SAME owned root reaches a join on BOTH edges):
    /// `entry` mints one owned value `r`, then branches to `p1` / `p2`. `p1`
    /// forwards `r` straight into the join's phi (a transfer). `p2` forwards `r`
    /// into the join's phi too (through a transparent alias `r_alias`, the
    /// load-var shape) — so `r`'s root is forwarded by MORE THAN ONE predecessor
    /// and is a member of `incoming_arg_roots`. The join consumes the phi (a
    /// `Call`) and returns nothing. There is exactly ONE underlying owned object;
    /// the assertion is that the pass emits AT MOST ONE `DecRef` naming any member
    /// of `r`'s group across the whole function — never two (the double-free the
    /// global keying must not introduce). A leak (zero drops) would be acceptable
    /// per the fail-closed contract; a double-free would be the UAF bug.
    #[test]
    fn forwarded_into_phi_other_pred_live_is_leak_not_uaf() {
        let mut func = TirFunction::new("diamond".into(), vec![], TirType::DynBox);
        let p1 = func.fresh_block();
        let p2 = func.fresh_block();
        let join = func.fresh_block();
        let r = func.fresh_value(); // fresh owned (ConstStr) defined in entry
        let cond = func.fresh_value();
        let r_alias = func.fresh_value(); // Copy(r) in p2 — transparent alias of r
        let phi = func.fresh_value(); // join's owned obj-lane phi
        let used = func.fresh_value(); // Call(phi) result
        for v in [r, r_alias, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(r));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: p1,
                then_args: vec![],
                else_block: p2,
                else_args: vec![],
            };
        }
        // p1: forward `r` straight into the join phi.
        func.blocks.insert(
            p1,
            TirBlock {
                id: p1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![r],
                },
            },
        );
        // p2: r_alias = load_var(r) [transparent alias]; forward the alias into the
        // SAME phi position → `r`'s root is forwarded by a 2nd predecessor.
        func.blocks.insert(
            p2,
            TirBlock {
                id: p2,
                args: vec![],
                ops: vec![{
                    let mut o = op(OpCode::Copy, vec![r], vec![r_alias]);
                    o.attrs
                        .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                    o
                }],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![r_alias],
                },
            },
        );
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![TirValue {
                    id: phi,
                    ty: TirType::Str,
                }],
                ops: vec![op(OpCode::Call, vec![phi], vec![used])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The single owned object (`r`'s alias group: r, r_alias, phi) must be
        // released AT MOST once — never twice. (Fail-closed: a leak is allowed; a
        // double-free is the UAF the global keying must never introduce.)
        let group_decrefs = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| {
                o.opcode == OpCode::DecRef
                    && o.operands
                        .first()
                        .is_some_and(|&v| v == r || v == r_alias || v == phi)
            })
            .count();
        assert!(
            group_decrefs <= 1,
            "incoming_arg_roots over-all-preds keying must never double-free a \
             forwarded root (fail-closed: leak ok, UAF never); got {group_decrefs} \
             DecRefs of the owned group"
        );
    }
}
