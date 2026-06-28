use std::collections::{BTreeMap, HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::liveness::{TirLiveness, TirLivenessResult};
use crate::tir::passes::ownership_lattice_min::{
    DropEligibility, OwnershipLattice, OwnershipRootFacts, PythonLifetimeFacts,
    StatementReleasePlan, op_consumed_operand_root, op_result_absorbs_operand_ownership,
    terminator_branch_args, terminator_uses_root,
};
use crate::tir::values::ValueId;

use super::arcs::{
    ArcDescriptor, EdgeSplit, exception_arcs_for_block, push_edge_split, retarget_arc,
    terminator_arcs,
};
use super::audit::emit_drop_inner_stage_audit;
use super::exception_region::{
    ExceptionRegionDropInsertion, explicit_release_values,
    insert_exception_creation_drops_at_raise, insert_exception_region_match_drops,
};
use super::util::{
    attr_is_true, is_return_deferral_barrier, is_suspension_point, make_op,
    ordered_unique_after_op_values, sorted_unique_values, sorted_values, terminator_mentions_value,
};
use super::{DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR};
use crate::tir::passes::PassStats;

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
    //     `has_state_machine()` check. State-machine drop activation requires
    //     StateSwitch-aware, def-reaching liveness as the ownership fact source.
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
    let ownership_root_facts = OwnershipRootFacts::compute(func, &aliases);
    let drop_eligibility = DropEligibility::new(&aliases, &ownership_root_facts, &live.raw_scalars);
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
                    if is_return_deferral_barrier(op.opcode) {
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

    // ── 4. Loop-carried phi drop-old before the back-edge (design §2.7) ──────
    // Pure reassignment loops can overwrite an owned header phi without reading
    // that phi in the loop body. Straight-line last-use placement cannot see a
    // use, and edge-dying handles only the exit value, so the previous iteration's
    // value must be released on the back-edge that overwrites the slot.
    {
        let mut loop_headers: Vec<BlockId> = func
            .loop_roles
            .iter()
            .filter_map(|(&bid, role)| (*role == LoopRole::LoopHeader).then_some(bid))
            .collect();
        loop_headers.sort_unstable_by_key(|block| block.0);

        for header_bid in loop_headers {
            if !reachable.contains(&header_bid) {
                continue;
            }
            let Some(header) = func.blocks.get(&header_bid) else {
                continue;
            };
            if header.args.is_empty() {
                continue;
            }

            let loop_blocks = crate::tir::dominators::collect_loop_blocks(
                func,
                &pred_map_term,
                &idoms,
                header_bid,
            );
            let mut body_used_roots: HashSet<ValueId> = HashSet::new();
            for &loop_block in &loop_blocks {
                let Some(block) = func.blocks.get(&loop_block) else {
                    continue;
                };
                for op in &block.ops {
                    for &operand in &op.operands {
                        body_used_roots.insert(canon(operand));
                    }
                }
            }

            let mut body_forwarded_into_phi_roots: HashSet<ValueId> = HashSet::new();
            for &loop_block in &loop_blocks {
                let Some(block) = func.blocks.get(&loop_block) else {
                    continue;
                };
                for arc in terminator_arcs(&block.terminator) {
                    if arc.target == header_bid {
                        continue;
                    }
                    let target_has_phis = func
                        .blocks
                        .get(&arc.target)
                        .is_some_and(|target| !target.args.is_empty());
                    if !target_has_phis {
                        continue;
                    }
                    for &value in &arc.args {
                        body_forwarded_into_phi_roots.insert(canon(value));
                    }
                }
            }

            let mut latches: Vec<BlockId> = pred_map_term
                .get(&header_bid)
                .map(|preds| {
                    preds
                        .iter()
                        .copied()
                        .filter(|&pred| crate::tir::dominators::dominates(header_bid, pred, &idoms))
                        .collect()
                })
                .unwrap_or_default();
            latches.sort_unstable_by_key(|block| block.0);

            for latch_bid in latches {
                if !reachable.contains(&latch_bid) {
                    continue;
                }
                let term = func.blocks[&latch_bid].terminator.clone();
                let arcs = terminator_arcs(&term);
                let latch_already_released: HashSet<ValueId> = {
                    let mut released: HashSet<ValueId> = HashSet::new();
                    if let Some(existing) = plans.get(&latch_bid) {
                        for &value in &existing.at_entry {
                            released.insert(canon(value));
                        }
                        for &value in &existing.before_term {
                            released.insert(canon(value));
                        }
                        for values in existing.after_op.values() {
                            for &value in values {
                                released.insert(canon(value));
                            }
                        }
                        for values in existing.before_op.values() {
                            for &value in values {
                                released.insert(canon(value));
                            }
                        }
                    }
                    if let Some(block) = func.blocks.get(&latch_bid) {
                        for op in &block.ops {
                            if op.opcode == OpCode::DecRef
                                && let Some(&value) = op.operands.first()
                            {
                                released.insert(canon(value));
                            }
                        }
                    }
                    released
                };
                let arcs_to_header = arcs.iter().filter(|arc| arc.target == header_bid).count();
                for arc in &arcs {
                    if arc.target != header_bid {
                        continue;
                    }
                    let header = &func.blocks[&header_bid];
                    let mut arc_releases: Vec<ValueId> = Vec::new();
                    for (pos, phi) in header.args.iter().enumerate() {
                        let phi_id = phi.id;
                        if !drop_eligibility.is_droppable(phi_id) {
                            continue;
                        }
                        if body_used_roots.contains(&canon(phi_id)) {
                            continue;
                        }
                        if latch_already_released.contains(&canon(phi_id)) {
                            continue;
                        }
                        if body_forwarded_into_phi_roots.contains(&canon(phi_id)) {
                            continue;
                        }
                        let Some(&edge_value) = arc.args.get(pos) else {
                            continue;
                        };
                        if canon(edge_value) == canon(phi_id) {
                            continue;
                        }
                        arc_releases.push(phi_id);
                    }
                    if arc_releases.is_empty() {
                        continue;
                    }
                    if arcs_to_header == 1 && !arc.is_self_loop_into_own_phi(latch_bid) {
                        let plan = plans.entry(latch_bid).or_insert_with(|| BlockPlan {
                            after_op: HashMap::new(),
                            at_entry: Vec::new(),
                            before_term: Vec::new(),
                            before_op: HashMap::new(),
                            before_exception_op: HashMap::new(),
                            after_exception_op: HashMap::new(),
                            before_term_incref: Vec::new(),
                        });
                        for value in arc_releases {
                            plan.before_term.push(value);
                        }
                    } else {
                        push_edge_split(
                            &mut edge_splits,
                            latch_bid,
                            arc.descriptor,
                            arc.target,
                            arc.args.clone(),
                            vec![],
                            arc_releases,
                        );
                    }
                }
            }
        }
    }
    emit_drop_inner_stage_audit(
        func,
        "after-loop-carried-phi-drop",
        Some(plans.len()),
        Some(edge_splits.len()),
        Some(planned_insertion_count(&plans)),
        Some(reachable.len()),
        audit_start.elapsed().as_millis(),
    );

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
