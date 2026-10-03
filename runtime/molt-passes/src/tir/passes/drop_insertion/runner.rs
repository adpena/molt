use std::collections::{BTreeMap, HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::liveness::{compute_liveness_in_domain, compute_raw_scalars};
use crate::tir::passes::ownership_lattice_min::{
    DropEligibility, OperandTransfer, OwnershipLattice, OwnershipRootFacts, PythonLifetimeFacts,
    StatementReleasePlan, op_result_absorbs_operand_ownership, terminator_branch_args,
    terminator_uses_root,
};
use crate::tir::values::ValueId;

use super::arcs::{
    ArcSite, EdgeSplit, exception_arcs_for_block, push_edge_split, retarget_arc, terminator_arcs,
};
use super::audit::emit_drop_inner_stage_audit;
use super::availability::PointAvailability;
use super::exception_region::{
    ExceptionRegionDropInsertion, insert_exception_creation_drops_at_raise,
    insert_exception_region_match_drops,
};
use super::transfers::TransferPlan;
use super::util::{
    attr_is_true, is_return_deferral_barrier, make_op, ordered_unique_after_op_values,
    sorted_unique_values, sorted_values,
};
use super::{DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR};
use crate::tir::passes::PassStats;

/// Run drop insertion. See module docs for the algorithm.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run_planned(func, am, None)
}

/// The frame clear DropInsertion plans before one `Return` (design 20 §1.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FrameClear {
    /// Roots released before the return, in placement order: the lexical
    /// bindings that reach it owned and the deferred named owners.
    pub(crate) releases: Vec<ValueId>,
    /// Lexical roots the return publishes rather than releases.
    pub(crate) published: Vec<ValueId>,
}

impl FrameClear {
    /// The frame's whole teardown once the return value holds a reference of
    /// its own: every binding, the published ones included, in the frame's
    /// release order.
    pub(crate) fn teardown(&self) -> Vec<ValueId> {
        let mut bindings = self.releases.clone();
        bindings.extend_from_slice(&self.published);
        sorted_unique_values(&bindings)
    }
}

/// The frame clear of each reachable `Return` block of `func`, exactly as
/// `run` places it. An inlined activation reproduces it at its exits. Plans a
/// copy; `func` is unchanged.
pub(crate) fn frame_clear(func: &TirFunction) -> BTreeMap<BlockId, FrameClear> {
    let mut probe = func.clone();
    let mut clears = BTreeMap::new();
    run_planned(&mut probe, &mut AnalysisManager::new(), Some(&mut clears));
    clears
}

fn run_planned(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
    frame_clears: Option<&mut BTreeMap<BlockId, FrameClear>>,
) -> PassStats {
    let mut stats = PassStats {
        name: "drop_insertion",
        ..Default::default()
    };

    // Every invocation, including generator/coroutine polls, uses the same
    // ownership authority. Suspension is made explicit before analysis.
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
    // Storage across activations is explicit in ClosureStore/ClosureLoad.
    // Expose the hidden suspension exits before asking the shared ownership
    // analyses where each invocation's references end.
    let activation_exits = super::activation::expose_activation_exits(func);
    if activation_exits != 0 {
        stats.facts_changed += activation_exits;
        am.invalidate_cfg();
    }
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
    emit_drop_inner_stage_audit(
        func,
        "after-pre-bail-slice",
        None,
        None,
        Some(pre_bail_drops),
        None,
        audit_start.elapsed().as_millis(),
    );

    // Alias-root canonicalization (design 20 §1.2) and root-only ownership facts
    // are stable across the DelBoundary normalization below: DelBoundary carries
    // no results and therefore cannot change alias roots or result-validity /
    // non-owning-copy root facts. Statement-boundary facts are computed later on
    // the normalized op stream so their op indices remain exact.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(func);
    let ownership_root_facts = OwnershipRootFacts::compute(func, &aliases);
    let raw_scalars = compute_raw_scalars(func);
    let drop_eligibility = DropEligibility::new(&aliases, &ownership_root_facts, &raw_scalars);
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
    emit_drop_inner_stage_audit(
        func,
        "after-alias",
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
    // Capture source binding provenance before DelBoundary becomes DecRef.
    // Normalization changes placement, not these stable alias-root facts.
    // A physical DecRef may instead belong to an exception or expression owner.
    let mut python_lifetime_facts = PythonLifetimeFacts::compute(func, &aliases);

    // ── 0a. `del`-boundary normalization (#58) ────────────────────────────────
    // The frontend carries a function-scope `del x` as `DelBoundary(v)` so the
    // Python lifetime boundary survives optimization (it used to lower to
    // NOTHING, leaving the release at whatever SSA-last-use happened to be —
    // coincidentally early). This pass is the release authority on
    // drop-activated targets, so the boundary BECOMES the release: rewrite in
    // place to `DecRef(root)` when the root is pass-owned (droppable); delete
    // otherwise (raw carrier / borrowed param / stack / borrowed alias — CPython's
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
                assert!(
                    !ownership_root_facts.is_binding_view_root(r),
                    "DropInsertion({}): DelBoundary of binding view {:?}; clear the owning home instead",
                    func.name,
                    r
                );
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
    python_lifetime_facts.refresh_explicit_release_roots(func, &aliases);

    // Normalize uses before solving liveness. Deleted DelBoundary operands must
    // not keep phantom uses alive; definitions and representations are unchanged,
    // so the established domain can be reused without repeating its analysis.
    let live = compute_liveness_in_domain(func, &aliases, &raw_scalars);
    emit_drop_inner_stage_audit(
        func,
        "after-liveness",
        None,
        None,
        Some(live.raw_scalars.len()),
        live.live_in.len().checked_add(live.live_out.len()),
        audit_start.elapsed().as_millis(),
    );
    let ownership_lattice = OwnershipLattice::compute(func, &aliases);
    let statement_release_plan = StatementReleasePlan::compute(
        &ownership_lattice,
        &python_lifetime_facts,
        &drop_eligibility,
    );
    let explicit_release_roots = python_lifetime_facts.explicit_release_roots();
    let boundary_release_roots =
        python_lifetime_facts.boundary_release_roots(&drop_eligibility, &ownership_lattice);
    emit_drop_inner_stage_audit(
        func,
        "after-boundary-root-planning",
        None,
        None,
        Some(
            boundary_release_roots
                .len()
                .saturating_add(explicit_release_roots.len()),
        ),
        None,
        audit_start.elapsed().as_millis(),
    );

    // The plan: per block, a list of (insert_after_op_index OR at-entry, value)
    // DecRef placements, plus per-block at-entry edge-dying drops, plus
    // IncRefs before adopting ops. We collect first (read-only over `func`),
    // then apply.
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
        /// IncRef(v) to insert immediately BEFORE the op at this index, with
        /// multiplicity: the retains of an adopting op (`transfers.rs`).
        before_op: HashMap<usize, Vec<ValueId>>,
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
                    + plan.before_term_incref.len()
            })
            .sum()
    };

    let block_ids: Vec<BlockId> = {
        let mut v: Vec<BlockId> = func.blocks.keys().copied().collect();
        v.sort_unstable_by_key(|b| b.0);
        v
    };
    let reachable = crate::tir::dominators::reachable_blocks_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let exception_labels = crate::tir::dominators::exception_label_to_block(func);
    let exceptional_entries: HashSet<BlockId> = func
        .blocks
        .values()
        .flat_map(|block| {
            exception_arcs_for_block(&exception_labels, block)
                .into_iter()
                .filter(|arc| {
                    crate::tir::dominators::exception_edge_binds_handler_arguments(
                        block.ops[arc.op_index].opcode,
                    )
                })
        })
        .map(|arc| arc.target)
        .collect();
    // Critical-edge splits to materialize.  One split block is the edge-local RC
    // authority for a concrete outgoing terminator arc: it may hold IncRefs for
    // borrowed values entering owned phis and/or DecRefs for path-specific
    // releases.  Collected here, applied after the op rebuild so block-id
    // allocation does not disturb in-place op insertion.
    let mut edge_splits: Vec<EdgeSplit> = Vec::new();

    // One availability authority for every placement below. A release may name
    // a root only where its definition reaches on every normal and exceptional
    // path, a conditional result only inside its initialized region, and only
    // where the root's name still owns its object. The same custody classifies
    // every block-argument binding, on terminator and exception arcs alike, for
    // the retains placed below. Computed after `DelBoundary` normalization,
    // whose op indices it records. Placement only reads `func` until the plans
    // are applied.
    let mut points = PointAvailability::compute_with_transport(
        func,
        &ownership_root_facts,
        &drop_eligibility,
        &live,
        &exception_labels,
        &reachable,
    );
    let (moves, canonical_arcs) = points.transport_counts();
    emit_drop_inner_stage_audit(
        func,
        "after-phi-transport",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(moves),
        Some(canonical_arcs),
        audit_start.elapsed().as_millis(),
    );

    // Boundary-held reference custody. Python local owners and explicitly
    // released references keep their objects to their declared boundary rather
    // than their last SSA use. An explicit release alone is not a local binding. The obligation follows the object: a join, loop or handler argument
    // that some canonical arc moves one of them into holds the object now and
    // keeps it to the same boundary. Only function-owned roots carry it; an
    // explicit boundary on a borrowed root (`del` of a parameter) releases
    // nothing. The lexical planner below places these releases; the SSA
    // placements (§1, §1b, §1c, §3, §3b) leave them alone.
    let boundary_held_roots = points.with_carriers(
        boundary_release_roots
            .iter()
            .chain(explicit_release_roots)
            .copied()
            .filter(|&root| drop_eligibility.is_droppable(root)),
    );

    // Reference-release custody and Python binding custody are distinct.
    // Both follow the same canonical moves; only positive binding provenance
    // requires a home store to end the old source-name owner. In particular an
    // exception MatchRef stays owned by its region when a handler binds it.
    let binding_custody_seeds =
        python_lifetime_facts.binding_custody_roots(&drop_eligibility, &ownership_lattice);
    let binding_roots = points.with_carriers(binding_custody_seeds.iter().copied());

    // ── 0b. Python named-owner release deferral ──
    // Explicit DEL_BOUNDARY/slot ownership remains authoritative. For a
    // boundaryless pure-SSA root, positive bound_local provenance requires the
    // Python local lifetime even if no defines_del fact is present: opaque
    // results and mutable classes can have observable destruction.
    //
    // This existing planner accepts only owned op-defined roots in their own
    // Return block, with no suspension, branch transfer, explicit RC boundary,
    // or named-slot ownership. An operation that adopts such a root receives a
    // retained reference (`transfers.rs`). Unmarked expression temporaries
    // retain last-use release. Mid-block exception cleanup is not proved by
    // return placement; this change does not claim broader exception-path
    // lifetime coverage.
    let deferred: HashSet<ValueId>;
    let mut deferred_return_placements: Vec<(BlockId, ValueId)> = Vec::new();
    {
        let named_owner_roots =
            python_lifetime_facts.return_boundary_candidate_roots(&drop_eligibility);
        let mut accepted: HashSet<ValueId> = HashSet::new();
        if !named_owner_roots.is_empty() {
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
                for &r in &named_owner_roots {
                    if terminator_uses_root(&block.terminator, r, &canon) {
                        disqualified.insert(r);
                    }
                }
                for op in &block.ops {
                    if is_return_deferral_barrier(op.opcode) {
                        for &operand in &op.operands {
                            disqualified.insert(canon(operand));
                        }
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
            for &r in &named_owner_roots {
                if disqualified.contains(&r) {
                    continue;
                }
                // Gate (b'/c): PythonLifetimeFacts owns whether this root is a
                // bound-local deferral instead of a slot-backed local with its
                // own del/rebinding release boundary.
                if !python_lifetime_facts.is_return_boundary_deferred_root(r, &drop_eligibility) {
                    continue;
                }
                // Gate (b): an op-defined root (not a phi) whose own block
                // ends in `Return`.
                let Some(dblk) = points.result_block(r) else {
                    continue;
                };
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

    // ── Taken operands ───────────────────────────────────────────────────────
    // A frame home store consumes the binding it stores, a runtime entry that
    // frees its builder consumes it, and a source Python call instruction
    // adopts each argument whose custody is `Transferred`, on both of their
    // continuations (`transfers.rs`). Plan each taking op once: an owned root
    // that nothing reads afterwards moves its own +1 into the first position
    // naming it, unless a Python boundary keeps it, and every other position
    // is retained right before the op. "Reads afterwards" is the one last-read
    // projection that §1's last-use releases read below. A move retires the
    // root's name there, before any placement asks where the root is owned.
    //
    // The deferred and statement releases below are placed by root, not by
    // where the root is still owned, so those roots always keep theirs. A
    // lexical binding stays bound across adoption and generic consumption,
    // and is retained there. A binding store ends the binding it stores: the home
    // owns the object from then on, so a lexical root moves into it, and no
    // `Return`, landing, arc or `DelBoundary` release names it again.
    let last_reads: HashMap<BlockId, HashMap<ValueId, usize>> = block_ids
        .iter()
        .copied()
        .filter(|bid| reachable.contains(bid))
        .map(|bid| {
            let block = &func.blocks[&bid];
            (
                bid,
                points.last_reads(block, bid, &live, &exception_labels, &canon),
            )
        })
        .collect();
    let has_binding_custody = |root: ValueId| binding_roots.contains(&root);
    let transfers = TransferPlan::compute(
        func,
        &block_ids,
        &drop_eligibility,
        &live,
        &last_reads,
        &|root: ValueId, transfer: OperandTransfer| {
            deferred.contains(&root)
                || statement_release_plan.contains_released_root(root)
                || ((transfer != OperandTransfer::BindingStore || !has_binding_custody(root))
                    && (boundary_held_roots.contains(&root)
                        || python_lifetime_facts.has_explicit_release_boundary(root)))
        },
        &has_binding_custody,
        &|root| {
            let mut sources: Vec<_> = binding_custody_seeds
                .iter()
                .copied()
                .filter(|&seed| points.with_carriers([seed]).contains(&root))
                .collect();
            sources.sort_unstable();
            sources
                .into_iter()
                .map(|seed| {
                    format!(
                        "{seed:?}: {}",
                        python_lifetime_facts.describe_binding_provenance(seed, &ownership_lattice)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        },
    );
    points.retire_adopted(transfers.moved());
    let (moved, retained) = transfers.counts();
    emit_drop_inner_stage_audit(
        func,
        "after-adopted-operands",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(moved),
        Some(retained),
        audit_start.elapsed().as_millis(),
    );

    // The handler-argument positions each `CheckException` must retain. Its
    // landing block retains them on the exceptional path only (§2b).
    let mut landing_retains: HashMap<(BlockId, usize), Vec<usize>> = HashMap::new();
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        let exception_arcs = exception_arcs_for_block(&exception_labels, block);
        let mut plan = BlockPlan {
            after_op: HashMap::new(),
            at_entry: Vec::new(),
            before_term: Vec::new(),
            before_op: HashMap::new(),
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
        // Last read index per ownership root, including transparent aliases and
        // each observation's handler demand: the one projection that the
        // adoption plan also read (`PointAvailability::last_reads`).
        let last_use = &last_reads[&bid];
        for (&v, &idx) in last_use {
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
            // Python `__del__`. Lexical custody holds a named-local owner, and
            // the block args that took its object, until its Python boundary
            // rather than firing at SSA last read. Unbound expression
            // temporaries are not lexical; they keep last-use placement.
            if boundary_held_roots.contains(&v) {
                continue;
            }
            // Consumed by the terminator (Return value / cond) — canonicalize the
            // terminator's direct uses to roots and skip if `v` is among them.
            if terminator_uses_root(&block.terminator, v, &canon) {
                continue;
            }
            // Moved into its last-use op, which takes it (`transfers.rs`): a
            // frame home store, a CallArgs builder that `call_bind` /
            // `call_indirect` free, or an argument of a source Python call
            // instruction. The op owns it on both continuations, like a Return
            // value, so a trailing DecRef would release it twice.
            if transfers.moves(bid, idx, v) {
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
                // locals for finalizer ordering: lexical custody drops them at
                // the frame boundary, not immediately after construction.
                // Unbound expression temporaries are not lexical and die here.
                if boundary_held_roots.contains(&r) {
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

        // ── 1c. Dead block args ───────────────────────────────────────────────
        // Every canonical arc moves or retains a +1 into each owned block arg
        // it binds, exception edges included. An arg that nothing in its block
        // reads, forwards or keeps live dies on entry, so it is released there,
        // once on every entry: joins, loop headers and handlers alike. Every arc
        // must bind it an owned reference; a raw or uninitialized input leaves
        // it alone. A lexical arg keeps its object to its Python boundary.
        for (position, arg) in block.args.iter().enumerate() {
            let root = arg.id;
            if !drop_eligibility.is_droppable(root)
                || !points.binds_owner_on_every_arc(bid, position)
                || boundary_held_roots.contains(&root)
                || last_use.contains_key(&root)
                || branch_arg_roots.contains(&root)
                || live.is_live_out(bid, root)
                || terminator_uses_root(&block.terminator, root, &canon)
                || statement_release_plan.contains_released_root(root)
            {
                continue;
            }
            plan.at_entry.push(root);
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

        // ── 2b. Exception-edge owned-arg retain ─────────────────────────────
        // A raising `CheckException` binds its handler's block args to its
        // operands exactly like branch args bind a phi, and `points` classifies
        // them with the §5 rules. A function-owned root that the handler body
        // does not read moves its single +1 into the first argument it binds.
        // The +1 stays with the normal continuation when the check does not
        // raise, and `points` reports the root unowned wherever the handler
        // path leads. Any other owned binding (a borrowed payload, a root the
        // handler still reads, a root bound twice) needs a +1 on the
        // exceptional path only. The check's landing block retains it there
        // (`exception_edges.rs`), so the normal path pays nothing and the
        // observation stays point-exact. A region registration (`TryStart`)
        // keeps its handler reachable but never raises into it: `points`
        // records no binding for it, so it retains nothing.
        for arc in exception_arcs {
            let positions = points.retained_positions(bid, ArcSite::Exception(arc.op_index));
            if !positions.is_empty() {
                landing_retains.insert((bid, arc.op_index), positions);
            }
        }

        // An adopting op's retains (`transfers.rs`), one per position that
        // cannot take its root's own +1.
        for (idx, operands) in transfers.retains(bid) {
            plan.before_op
                .entry(idx)
                .or_default()
                .extend_from_slice(operands);
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

    // ── Lexical custody ──────────────────────────────────────────────────────
    // A lexical root keeps its object to a Python boundary rather than its last
    // SSA use. It is released
    //   * before the terminator of a Return that it reaches owned on every
    //     entry and that does not return it; and
    //   * on a terminator arc from a block whose exit owns it into a join that
    //     it does not reach owned the same way: another entry lacks it, or the
    //     arc re-enters the root's own definition without binding its argument
    //     to itself. The arc does not move it, and nothing past the arc can use
    //     it: SSA dominance, a move's clean-transfer condition, a release or
    //     validity rules each such use out. A root that the join still reads
    //     is left unreleased rather than freed under that read.
    // A move, an explicit release or an adoption ends custody by itself,
    // and a check's landing releases what its handler abandons. An arc that is
    // its target's only entry carries every owner there unchanged. An arc
    // release sits on a split of the arc itself, where the owner ends.
    let mut lexical: Vec<ValueId> = boundary_held_roots.iter().copied().collect();
    lexical.sort_unstable_by_key(|root| root.0);
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        if matches!(block.terminator, Terminator::Return { .. }) {
            let plan = plans
                .get_mut(&bid)
                .expect("reachable block plan must exist before lexical custody");
            for &root in &lexical {
                if points.available_at_exit(root, bid)
                    && !terminator_uses_root(&block.terminator, root, &canon)
                {
                    plan.before_term.push(root);
                }
            }
            continue;
        }
        for arc in terminator_arcs(&block.terminator) {
            if points.incoming_sources(arc.target).len() < 2 {
                continue;
            }
            let site = ArcSite::Terminator(arc.descriptor);
            let target = &func.blocks[&arc.target];
            let releases: Vec<ValueId> = lexical
                .iter()
                .copied()
                .filter(|&root| {
                    if points.moves_on(bid, site, root)
                        || points.lives_into_body(root, arc.target)
                        || !points.available_at_exit(root, bid)
                    {
                        return false;
                    }
                    let rebinds =
                        points.definition_block(root) == Some(arc.target)
                            && !target.args.iter().zip(&arc.args).any(|(argument, &value)| {
                                argument.id == root && canon(value) == root
                            });
                    rebinds || !points.available_at_entry(root, arc.target)
                })
                .collect();
            if !releases.is_empty() {
                push_edge_split(
                    &mut edge_splits,
                    bid,
                    arc.descriptor,
                    arc.target,
                    arc.args,
                    vec![],
                    releases,
                );
            }
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-lexical-custody",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(lexical.len()),
        Some(points.retirement_counts().1),
        audit_start.elapsed().as_millis(),
    );

    // A frame-clear query stops here. Every `Return` release is planned: the
    // lexical ones above and the deferred named owners of §0b. What follows is
    // SSA last-use, edge and publication placement.
    if let Some(frame_clears) = frame_clears {
        for &bid in &block_ids {
            if !reachable.contains(&bid) {
                continue;
            }
            let block = &func.blocks[&bid];
            if !matches!(block.terminator, Terminator::Return { .. }) {
                continue;
            }
            let mut releases = plans[&bid].before_term.clone();
            releases.extend(
                deferred_return_placements
                    .iter()
                    .filter(|&&(exit, _)| exit == bid)
                    .map(|&(_, root)| root),
            );
            let published = lexical
                .iter()
                .copied()
                .filter(|&root| terminator_uses_root(&block.terminator, root, &canon))
                .collect();
            frame_clears.insert(
                bid,
                FrameClear {
                    releases: sorted_unique_values(&releases),
                    published,
                },
            );
        }
        return stats;
    }

    // ── 0c. Owned return publication ────────────────────────────────────────
    // A call returns one owned result. A direct return of a borrowed parameter
    // (or any transparent alias of it) therefore cannot merely forward the
    // borrowed bits: the caller would later release an ownership edge that the
    // callee never minted. Publish that edge here, at the shared TIR boundary
    // consumed by every backend. Fresh/function-owned results, a transferred
    // parameter among them, transfer their existing +1 and receive no retain.
    // A return that frame teardown could invalidate returns the frontend's
    // owned capture (`binding_alias`), taken before the teardown starts; a read
    // after the returned root's own release is malformed, and nothing here
    // repairs it. A frame binding view is such a return, and one that would
    // need a retain here fails the producer contract. Mixed block-arg phis are
    // made uniformly owned by §5 below, so they likewise need no second return
    // retain.
    //
    // This stage deliberately MERGES into the completed per-block plan. An
    // earlier pre-plan implementation was silently overwritten by the canonical
    // block-plan insertion below, reproducing the missing retain despite a
    // locally correct ownership predicate.
    //
    // Deduplicate by alias root: Return is a single result-publication boundary
    // even if malformed or intermediate TIR repeats the same alias in its value
    // vector. Use the concrete returned SSA value for placement so it always
    // dominates this terminator; the ownership predicate remains root-owned.
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        let Terminator::Return { values } = &block.terminator else {
            continue;
        };
        let mut retained_roots = HashSet::new();
        let mut retained_values = Vec::new();
        for &value in values {
            let root = canon(value);
            if drop_eligibility.return_requires_owned_publication(value)
                && retained_roots.insert(root)
            {
                // A frame binding view names what only its home owns, and the
                // frame's exit, which immediately precedes every normal return
                // of a framed body, releases the homes before this retain could
                // run. No placement keeps it: the frontend returns an owned
                // capture (`binding_alias`) taken before the exit. The retain
                // would be a use-after-free, and nothing after this pass knows
                // views, so the producer contract fails here, before any backend
                // sees the body.
                assert!(
                    !ownership_root_facts.is_binding_view_root(root),
                    "{}: return of frame binding view {value:?} after the frame's exit; \
                     the frontend must return an owned capture taken before `trace_exit`",
                    func.name
                );
                retained_values.push(value);
            }
        }
        if retained_values.is_empty() {
            continue;
        }
        plans
            .get_mut(&bid)
            .expect("reachable block plan must exist before return publication")
            .before_term_incref
            .extend(retained_values);
    }
    emit_drop_inner_stage_audit(
        func,
        "after-owned-return-publication",
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
    //   * V is AVAILABLE at B's entry: defined, initialized if it is a
    //     conditional result, and still owned, on every normal and exceptional
    //     path into B (see below), AND
    //   * V is droppable.
    // This releases the value on the path where it dies. Because every path into
    // B that delivered V must release it, and B is a join, dropping once at B's
    // entry is correct ONLY when V dies on ALL incoming paths. We therefore
    // require V to be dead-in to B and live-out of EVERY predecessor that can
    // reach B (so no path still needs it). The elim pass later hoists/dedups.
    //
    // AVAILABILITY GUARD (soundness-critical, FAIL-CLOSED). The entry DecRef runs
    // on every path into B, including an exception edge that leaves a block
    // before V's definition. Block dominance cannot decide that. The Full tree
    // lets a definition below a `CheckException` "dominate" the handler. The
    // TerminatorOnly tree ignores the exception entries of a mixed block, one
    // with both terminator and exception predecessors, such as the exit that
    // `raise; jump exit` shares with every check. `PointAvailability` splits
    // blocks at each observation and also answers conditional-result validity.
    // It also declines where some path into B arrives after V gave up its
    // object: an arc, terminator or exception, that moved V into a block
    // argument, or an explicit release or adoption. A handler whose
    // argument a check's payload binds is the common case: liveness reports V
    // dead there, while every predecessor still has it live-out on its normal
    // continuation. When it declines, V is not dropped here. §3b releases it on
    // the normal arcs that still own it, and landings on the exceptional
    // entries that do. A use-before-def is the LLVM verifier "Instruction does
    // not dominate all uses!" abort; a release of a moved root is `invalid
    // object header before dec_ref`. Never over-release; a residual leak is the
    // fail-closed direction.
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let preds = points.incoming_sources(bid);
        if preds.is_empty() {
            continue;
        }
        // Exceptional abandonment has one ordered owner: the observation's
        // landing. An entry release would split that unwind between two
        // planners, making an older last-use operand die before a younger
        // owner that was live on the skipped normal continuation. Ordinary
        // entries into the same block are handled edge-exactly by section 3b.
        if exceptional_entries.contains(&bid) {
            continue;
        }
        let block_args: HashSet<ValueId> = func.blocks[&bid].args.iter().map(|a| a.id).collect();
        // A root that an incoming arc moves into one of THIS block's args is not
        // dying on entry, even though liveness reports it dead-in to `B` (its
        // successor-side identity is the block arg, a distinct SSA value). The
        // block arg (phi) is the owner now and is released by ITS own last use,
        // dead-arg entry release or lexical boundary. (This is the dual of the
        // §5 mixed-ownership retain: §5 ensures the transferred value is owned;
        // the availability guard below ensures the transfer itself is not also
        // released at the join. Without it, an owned value forwarded into a phi
        // through a multi-block chain — the shape the inliner produces for
        // `x = a + a; return x + a` — was dropped BOTH at the join entry AND at
        // the phi's last use → `invalid object header before dec_ref`.) The
        // guard reads every canonical arc, including a check's payload bound to
        // a handler arg, and every earlier move that reaches `B` before the root
        // is defined again. One at-entry drop cannot distinguish incoming paths,
        // so a root that transfers or dies on only some incoming arcs is
        // released per arc by §3b or by the exceptional landings; only
        // genuinely path-specific ownership allocates an edge block.
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
        // Roots already scheduled to drop at this block's entry (dedup by root,
        // not raw value — two aliases of the same group must drop once).
        let mut entry_root_seen: HashSet<ValueId> = HashSet::new();
        for v in candidates {
            if !drop_eligibility.is_droppable(v) {
                continue;
            }
            // A conditionally-valid iterator value is dropped here only where it
            // is initialized: the availability guard below never admits its
            // exhaustion edge, whose slot holds stale bits (review P0 #2(b)).
            let root = canon(v);
            // Python lifetime boundaries are path-conditioned release
            // authorities. The single at-entry edge-dying form would run on every
            // path into `bid`, so pairing it with a body-only statement/rebind
            // boundary or a later scope-exit boundary can release the same local
            // owner twice. Lexical custody and the statement plan own these
            // roots; SSA liveness alone never synthesizes a join-entry drop.
            if boundary_held_roots.contains(&root)
                || statement_release_plan.contains_released_root(root)
            {
                continue;
            }
            if block_args.contains(&v) || block_args.iter().any(|&a| canon(a) == root) {
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
            // AVAILABILITY GUARD (fail-closed; see above): defined, initialized
            // and still owned on every entry.
            if !points.available_at_entry(v, bid) {
                continue;
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

    // ── 3b. Path-specific edge-dying drops ───────────────────────────────────
    //
    // The compact at-entry rule above is intentionally limited to roots that
    // every predecessor delivers.  Mutually exclusive branch-local loops expose
    // the complementary shape: each loop owns an iterator that is live on its
    // continue edge, dies on its exhausted edge, and reaches a shared join whose
    // sibling predecessor never defined that iterator.  No value can be dropped
    // at the join without violating dominance, and dropping before the branch
    // would destroy the iterator on the continue path.
    //
    // Close that whole CFG class with exact edge placement.  A single-successor
    // predecessor can release immediately before its terminator.  A branching
    // predecessor gets one split block for the dying arc; `push_edge_split`
    // coalesces every release on that arc, so CFG growth is bounded by ownership-
    // divergent edges rather than by values.  Lexical roots, moved roots and
    // borrowed values keep their own authorities and are excluded here. A
    // conditional iterator result is released only where `PointAvailability`
    // finds it initialized, never on its exhaustion edge.
    let entry_planned_roots_by_block: HashMap<BlockId, HashSet<ValueId>> = plans
        .iter()
        .map(|(&block, plan)| {
            (
                block,
                plan.at_entry.iter().map(|&value| canon(value)).collect(),
            )
        })
        .collect();
    let before_term_planned_roots_by_block: HashMap<BlockId, HashSet<ValueId>> = plans
        .iter()
        .map(|(&block, plan)| {
            (
                block,
                plan.before_term.iter().map(|&value| canon(value)).collect(),
            )
        })
        .collect();
    for &pred in &block_ids {
        if !reachable.contains(&pred) {
            continue;
        }
        let Some(pred_block) = func.blocks.get(&pred) else {
            continue;
        };
        let arcs = terminator_arcs(&pred_block.terminator);
        if arcs.is_empty() {
            continue;
        }
        let Some(live_out) = live.live_out.get(&pred) else {
            continue;
        };
        let mut candidates: Vec<ValueId> = live_out.iter().copied().collect();
        candidates.sort_unstable_by_key(|value| value.0);

        for arc in &arcs {
            if !reachable.contains(&arc.target) {
                continue;
            }
            // When this arc is its target's only canonical entry, the compact
            // at-entry rule above is already exact: no exception edge or sibling
            // arc can reach the target without the owner. Avoid opening per-edge
            // sets on this overwhelmingly common straight-line CFG shape.
            if points.incoming_sources(arc.target).len() == 1
                && !exceptional_entries.contains(&arc.target)
            {
                continue;
            }
            let entry_planned_roots = entry_planned_roots_by_block.get(&arc.target);
            let pred_planned_roots = before_term_planned_roots_by_block.get(&pred);
            let transferred_roots: HashSet<ValueId> =
                arc.args.iter().map(|&value| canon(value)).collect();
            let mut arc_root_seen: HashSet<ValueId> = HashSet::new();

            for &value in &candidates {
                let root = canon(value);
                if !arc_root_seen.insert(root)
                    || entry_planned_roots.is_some_and(|roots| roots.contains(&root))
                    || pred_planned_roots.is_some_and(|roots| roots.contains(&root))
                    || transferred_roots.contains(&root)
                    || points.lives_into_body(root, arc.target)
                    || boundary_held_roots.contains(&root)
                    || statement_release_plan.contains_released_root(root)
                    || !drop_eligibility.is_droppable(value)
                {
                    continue;
                }
                // The one availability authority: defined on every entry of
                // `pred`, exceptional ones included, and for a conditional
                // result initialized at `pred`'s exit or by this very arc.
                let available = if arcs.len() == 1 {
                    points.available_at_exit(value, pred)
                } else {
                    points.available_on_arc(value, pred, arc.target)
                };
                if !available {
                    continue;
                }

                if arcs.len() == 1 {
                    plans
                        .entry(pred)
                        .or_insert_with(|| BlockPlan {
                            after_op: HashMap::new(),
                            at_entry: Vec::new(),
                            before_term: Vec::new(),
                            before_op: HashMap::new(),
                            before_term_incref: Vec::new(),
                        })
                        .before_term
                        .push(value);
                } else {
                    push_edge_split(
                        &mut edge_splits,
                        pred,
                        arc.descriptor,
                        arc.target,
                        arc.args.clone(),
                        vec![],
                        vec![value],
                    );
                }
            }
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-path-specific-edge-dying",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

    // ── 5. Mixed-ownership phi retain (design §ownership) ─────────────────────
    // A TIR block argument is the SSA phi: each predecessor edge passes a value
    // that binds the arg on entry. The straight-line / edge-dying / dead-arg
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
    // `points` (`availability.rs`) owns this classification for every canonical
    // arc. §2b places its exception-edge retains, this section its terminator
    // retains, and each clean transfer's root is reported unowned to every
    // release placed downstream of it.
    //
    // PLACEMENT is edge-exact. An unconditional, non-self edge can retain
    // before its terminator. Every conditional, switch and self edge uses a
    // split block so no sibling path executes its retains. The split carries
    // the original payload and preserves repeated ownership obligations.
    //
    // The same owned-phi contract applies to ordinary, activation and handler
    // CFGs; availability distinguishes their actual incoming ownership.

    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        // Only successor blocks WITH owned block-arg phis matter.
        // Examine each outgoing arc of this block's terminator.
        let term = func.blocks[&bid].terminator.clone();
        let arcs = terminator_arcs(&term);
        for arc in &arcs {
            // Retain each owned phi binding that cannot take its root's own +1.
            let arc_retains = points.retains(bid, ArcSite::Terminator(arc.descriptor));
            if arc_retains.is_empty() {
                continue;
            }
            if arcs.len() == 1 && !arc.is_self_loop_into_own_phi(bid) {
                // Only an unconditional edge may retain before its terminator.
                // With multiple outgoing arcs, a retain here would also execute
                // on unselected siblings, even if their destinations differ.
                // Self-loops split too, isolating the retain from body releases.
                let p = plans.entry(bid).or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
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
    let (regions, region_blocks) = points.retirement_counts();
    emit_drop_inner_stage_audit(
        func,
        "after-retirement-regions",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(regions),
        Some(region_blocks),
        audit_start.elapsed().as_millis(),
    );

    // An operation's normal cleanup cannot precede the observation of its
    // failure: that would run a finalizer before the exceptional edge unwinds
    // the remaining expression owners. Reuse async-work placement's exact
    // observation authority, including a uniquely reached successor block.
    // Keep each producer's operand/result release order on success; the
    // exceptional landing derives its unwind from final normal-path liveness.
    let mut after_observation: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::new();
    let exact_types = crate::tir::type_refine::extract_exact_scalar_map(func);
    let const_ints = crate::tir::passes::check_exception_elim::classify::const_int_values(func);
    let predecessors = crate::tir::dominators::build_pred_map(func);
    let mut release_sites: Vec<_> = plans
        .iter()
        .flat_map(|(&bid, plan)| plan.after_op.keys().map(move |&index| (bid, index)))
        .collect();
    release_sites.sort_unstable();
    for (bid, index) in release_sites {
        let op = &func.blocks[&bid].ops[index];
        if !crate::tir::passes::check_exception_elim::classify::op_may_raise(
            &exact_types,
            &const_ints,
            op,
        ) || op.opcode == OpCode::CheckException
        {
            continue;
        }
        let Some(crate::tir::passes::exception_observation::PostOperationObservation::Check(
            block,
            observation,
        )) = crate::tir::passes::exception_observation::post_operation_observation(
            func,
            bid,
            index,
            None,
            &predecessors,
            &exact_types,
            &const_ints,
        )
        else {
            continue;
        };
        let values = plans
            .get_mut(&bid)
            .unwrap()
            .after_op
            .remove(&index)
            .unwrap();
        after_observation
            .entry((block, observation))
            .or_default()
            .extend(ordered_unique_after_op_values(&values, op, &canon));
    }

    // ── Apply the plans ──────────────────────────────────────────────────────
    let mut inserted = stats.ops_added;
    // Each check's landing retains, keyed by its index in the rebuilt block.
    let mut observation_retains: HashMap<(BlockId, usize), Vec<usize>> =
        HashMap::with_capacity(landing_retains.len());
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
            // before_op IncRefs, with multiplicity (adoption retains).
            if let Some(vals) = plan.before_op.get(&idx) {
                for v in sorted_values(vals) {
                    new_ops.push(make_op(OpCode::IncRef, vec![v]));
                    inserted += 1;
                }
            }
            if let Some(positions) = landing_retains.remove(&(bid, idx)) {
                observation_retains.insert((bid, new_ops.len()), positions);
            }
            new_ops.push(op.clone());
            if let Some(values) = after_observation.remove(&(bid, idx)) {
                for value in values {
                    new_ops.push(make_op(OpCode::DecRef, vec![value]));
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
        // before_term_incref IncRefs (owned return publication, §0c, and the
        // mixed-ownership-phi retain, §5): a BORROWED value gets a `+1` here,
        // just before the terminator, at its ownership-transfer boundary.
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
    assert!(
        after_observation.is_empty(),
        "DropInsertion lost an exception observation's success cleanup"
    );
    assert!(
        landing_retains.is_empty(),
        "DropInsertion planned landing retains in a block it did not rebuild"
    );
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

    // Ordinary releases/consuming calls are now physical lifetime boundaries.
    // Close paths that bypass them at an exact exception observation, using the
    // shared liveness/ownership domain and explicit landing-block payloads. The
    // same landings retain the handler arguments that §2b planned.
    // Only no-result RC operations and argument-free edge blocks were added
    // since this domain was computed; definitions, aliases and representations
    // are unchanged. Reuse it instead of repeating carrier/value-range analysis.
    inserted += super::exception_edges::insert_exception_edge_releases(
        func,
        &drop_eligibility,
        &ownership_root_facts,
        &aliases,
        &live.raw_scalars,
        &observation_retains,
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
        let mut bindings: Vec<_> = binding_roots.iter().copied().collect();
        let mut held: Vec<_> = boundary_held_roots.iter().copied().collect();
        bindings.sort_unstable();
        held.sort_unstable();
        out.push_str(&format!(
            "  binding_custody={bindings:?} boundary_held={held:?}\n"
        ));
        if !deferred.is_empty() {
            let mut d: Vec<u32> = deferred.iter().map(|v| v.0).collect();
            d.sort_unstable();
            out.push_str(&format!(
                "  deferred(named-owner→Return)={:?} placements={:?}\n",
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
                    "    {:?} ops={:?} -> {:?}  [{}]{} source={:?} wire_out={:?}\n",
                    op.opcode,
                    ops,
                    res,
                    reprs.join(","),
                    kind,
                    op.source_op_index(),
                    op.attrs.get("_simple_out"),
                ));
            }
        }
        let _ =
            crate::debug_artifacts::write_debug_artifact(format!("drop/{}.txt", func.name), out);
    }
    stats.ops_added = inserted;
    stats
}
