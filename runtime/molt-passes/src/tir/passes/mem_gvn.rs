//! MemGVN — store-to-load forwarding + redundant-load elimination (S5-2b).
//!
//! The first consumer of the [`MemorySSA`](super::memory_ssa::MemorySSA)
//! substrate (S5-2a). It performs two memory redundancy eliminations over the
//! reaching-def graph MemorySSA provides:
//!
//! 1. **Store-to-load forwarding.** A proven-pure typed-slot load
//!    (`r = obj.<offset>`) whose single reaching memory version is exactly a
//!    ordinary typed-slot store proved release-neutral by the shared pristine-slot
//!    analysis (`store obj.<offset> = v`) to the **same object root and the
//!    same offset** is replaced by `Copy(v)`. The load no longer touches the
//!    heap — it becomes a pure SSA register read of the stored value.
//!
//! 2. **Redundant-load elimination.** Two proven-pure loads of the **same
//!    object root and offset** that read the **same** memory version (no
//!    clobber between them — MemorySSA gives this) collapse: the later load is
//!    replaced by `Copy(<earlier load's result>)`.
//!
//! ## Why an explicit offset/root check on top of `is_direct_def_of_use`
//!
//! MemorySSA's reaching-def is *region-aware*. Direct field accesses carry a
//! byte offset and, when proven, exact allocation identity. Unknown receivers
//! can still alias each other — so MemorySSA's `is_direct_def_of_use` answers the
//! question "is this store the most-recent memory version this load observes?"
//! but NOT "do the store and load name the same byte". In every
//! case the reaching-def relation is **necessary but not sufficient** for
//! forwarding: this pass adds the *sufficient* conditions —
//!
//!   * same object root (transparent-alias-resolved), and
//!   * same statically-known field offset,
//!
//! both read off the concrete `store` / `LoadAttr` operands and the store's
//! shared pristine fact. Only when the
//! reaching def is the single direct def (no phi, no intervening clobber) AND
//! the store and load provably name the *same slot* do we forward. This is
//! fail-closed: a missed equality only prevents an optimization.
//!
//! ## Soundness (a wrong forward is a silent miscompile — the worst class)
//!
//! Forwarding `r = obj.<off>` → `Copy(v)` is sound iff:
//!
//! * **No intervening clobber.** `mem.is_direct_def_of_use(store_ver, load)` is
//!   true ⇒ the store's version is the load's reaching def ⇒ no may-aliasing
//!   `MemoryDef` (in particular no `GenericHeap` call/raise/yield barrier, no
//!   overwriting store) lies on any path between them. A barrier between store
//!   and load produces a fresh version that intercepts the load, and the query
//!   fails. (Test: `forward_blocked_by_interposed_call`.)
//! * **No memory phi.** A phi-merged version is a *distinct* version from the
//!   store's, so `is_direct_def_of_use` is false at a join — forwarding across
//!   a `MemoryPhi` is structurally impossible here. (Test:
//!   `forward_blocked_by_memory_phi_merge`.)
//! * **Must-alias slot.** Same root + same offset ⇒ the load reads exactly the
//!   bytes the store wrote. Different offset (or different root) is never
//!   forwarded. (Test: `forward_blocked_by_different_offset`.)
//! * **Value dominates the load.** The store's block dominates the load's block
//!   (or is the same block, store before load) — established by the MemorySSA
//!   dominator-tree renaming walk and re-checked here via `dominates` +
//!   op-order, plus the strict-CFG-reachability guard the post-lowering verifier
//!   requires (mirrors `gvn.rs`). So `Copy(v)` never references a value before
//!   its definition.
//!
//! ## Refcount safety (THE soundness keystone — a dropped IncRef is a UAF)
//!
//! A typed-slot load returns an **owned** reference: the runtime
//! `molt_guarded_field_get` / `molt_object_field_get` path
//! (`object_field_get_ptr_raw`) unconditionally `inc_ref_bits` the slot value
//! before returning it, so the load's result `r` carries a +1 the frontend
//! ownership model balances with a later `DecRef(r)`. A *bare* `Copy(v) → r`
//! (`copy_var`, a plain pointer assignment) would NOT add that +1 — yet the
//! frontend's `DecRef(r)` still runs, underflowing the object's refcount into a
//! use-after-free. (`gvn.rs` never faces this: it value-numbers only const /
//! primitive-typed pure ops — never a `LoadAttr` — so it has no precedent here.)
//!
//! Therefore every forward emits `IncRef(source); Copy(source) → r` in place of
//! the load: the `IncRef` reproduces *exactly* the +1 the load itself performed
//! (`inc_ref_bits` no-ops on inline non-pointer values, so the inc is the right
//! action for a pointer source and a harmless no-op for an inline int source —
//! identical to what the load did). The result `r` then owns its reference and
//! the existing `DecRef(r)` balances. `copy_prop` later folds `r → source`,
//! turning the pair into `IncRef(source) … DecRef(source)` — balanced. This
//! holds for both forwarding flavors: an initialization independently takes the slot's own
//! +1 (`object_field_set_ptr_raw` `inc_ref_bits(val)`), leaving the stored SSA
//! value's ownership intact for the forwarded `IncRef`; and an earlier load's
//! result is itself an owned +1 that the second owned load duplicated.
//!
//! `mem_gvn` runs AFTER `refcount_elim` in the pipeline, so the emitted `IncRef`
//! is final — it is a genuinely required reference acquisition, not a redundant
//! pair to be cleaned up.
//!
//! ## Repr safety (the `apply(f, 1<<60, 7)` bigint oracle class)
//!
//! The forwarded value is the *exact* SSA value the store wrote (`v`) or an
//! earlier load's result. The new `Copy(source) → r` carries no `_original_kind`
//! — a pure, representation-transparent SSA move (`copy_prop`/`dce` clean it up).
//! The result `r` keeps its `ValueId`, so its repr in `representation_plan` is
//! unchanged; and `source` carries whatever repr it was already assigned.
//! Forwarding therefore can never introduce a repr *more aggressive* than what
//! was already proven: if the stored field value is `MaybeBigInt`, the forwarded
//! copy stays `MaybeBigInt`, and no trusted-unbox is created. (Differential:
//! `struct_field_forwarding.py`, the `>= 1 << 60` field.)
//!
//! ## Mutation class
//!
//! [`Mutates::OpsOnly`](crate::tir::pass_manager::Mutates::OpsOnly): every
//! rewrite replaces a `LoadAttr` op in place with a `Copy` op and inserts an
//! `IncRef` immediately before it — same block, same result `ValueId`, no
//! block/edge/terminator change and no exception-edge op added or removed
//! (`IncRef`/`Copy` are pure, non-throwing, non-terminator ops). The
//! CFG-structure analyses stay valid; the ops-sensitive caches (DefMap,
//! AliasAnalysis, MemorySSA) are dropped by the manager's `invalidate_ops`
//! afterward.
//!
//! [`AliasAnalysisResult::region_of`]: super::alias_analysis::AliasAnalysisResult::region_of
//! [`MemRegion::GenericHeap`]: super::alias_analysis::MemRegion::GenericHeap

use std::collections::HashMap;

use super::PassStats;
use super::alias_analysis::{AliasAnalysis, AliasAnalysisResult};
use super::memory_ssa::{MemAccess, MemorySSA, MemorySsaResult, typed_slot_store_value};
use crate::tir::analysis::{AnalysisManager, ImmediateDoms, StrictReachable};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// A planned rewrite of the load at `(block, op_idx)` into `Copy(source)`,
/// preserving the load's result `ValueId`.
struct Forward {
    block: BlockId,
    op_idx: usize,
    /// The SSA value to copy from (a stored value, or an earlier load's result).
    source: ValueId,
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run_with(func, am)
}

fn run_with(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "mem_gvn",
        ..Default::default()
    };

    // Trivial functions have no memory redundancy to eliminate.
    if func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    // Analyses (cloned, as gvn.rs does — `am.get` borrows are released before
    // we mutate `func`). MemorySSA depends on AliasAnalysis (computed first via
    // the lower AnalysisId ordinal); both are dropped by `invalidate_ops` after
    // this OpsOnly pass.
    let alias: AliasAnalysisResult = am.get::<AliasAnalysis>(func).clone();
    let mem: MemorySsaResult = am.get::<MemorySSA>(func).clone();
    let idoms = am.get::<ImmediateDoms>(func).clone();
    // Strict-CFG reachability (terminator-only). A forwarded `Copy(source)`
    // emitted into a block reachable only via exception edges, sourced from a
    // value defined in a strict-CFG block, would make the post-lowering
    // verifier (`verify_lir`, dominance over the strict subgraph) reject the
    // new operand — exactly the guard `gvn.rs` applies to cross-block copies.
    let strict_reachable = am.get::<StrictReachable>(func).clone();

    // Map every stored memory version to the (target_root, offset, value) it
    // wrote, so a load's reaching def can be matched against the must-alias
    // slot. Only ordinary stores whose exact sites have shared pristine-slot
    // facts are forwardable sources. Every other `store` may run the old
    // value's destructor after writing, so reentrant Python can change the slot
    // again; it remains a `GenericHeap` clobber and is never in this map. Calls,
    // raises, and yields likewise have no slot value and cannot forward.
    let mut store_def_slot: HashMap<u32, (ValueId, i64, ValueId)> = HashMap::new();
    for access in mem.defs.values() {
        if let MemAccess::Def {
            ver, block, op_idx, ..
        } = access
            && mem.slot_access.stores.contains_key(&(*block, *op_idx))
            && let Some(op) = func.blocks.get(block).and_then(|b| b.ops.get(*op_idx))
            && let Some((target, value, offset)) = typed_slot_store_value(op)
        {
            store_def_slot.insert(ver.0, (alias.root(target), offset, value));
        }
    }

    // For redundant-load elimination: the first load observed for a given
    // (reaching_version, root, offset) becomes the leader; later loads reading
    // the SAME memory version of the SAME slot copy from it. Keyed structurally
    // so two loads only collapse when they provably read the same bytes under
    // the same memory version.
    let mut load_leader: HashMap<(u32, ValueId, i64), (BlockId, ValueId)> = HashMap::new();

    let mut forwards: Vec<Forward> = Vec::new();

    let diag = std::env::var("MOLT_MEMGVN_DIAG").as_deref() == Ok("1");
    if diag {
        let mut n_load = 0usize;
        let mut n_store = 0usize;
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for b in func.blocks.values() {
            for o in &b.ops {
                if matches!(o.opcode, OpCode::LoadAttr | OpCode::StoreAttr) {
                    if o.opcode == OpCode::LoadAttr {
                        n_load += 1
                    } else {
                        n_store += 1
                    }
                    let k = match o.attrs.get("_original_kind") {
                        Some(AttrValue::Str(s)) => s.clone(),
                        _ => "<none>".into(),
                    };
                    *kinds.entry(format!("{:?}:{k}", o.opcode)).or_default() += 1;
                }
            }
        }
        if n_load + n_store > 0 || !mem.uses.is_empty() {
            eprintln!(
                "[MEMGVN_DIAG] fn={} loadattr={n_load} storeattr={n_store} mem_uses={} store_def_slots={} kinds={:?}",
                func.name,
                mem.uses.len(),
                store_def_slot.len(),
                kinds,
            );
        }
        if std::env::var("MOLT_MEMGVN_DUMP")
            .map(|p| func.name.contains(&p))
            .unwrap_or(false)
        {
            for (&bid, b) in &func.blocks {
                for (oi, o) in b.ops.iter().enumerate() {
                    let defv = mem.def_at(bid, oi).map(|v| v.0);
                    let usev = mem.reaching_def_for_use(bid, oi).map(|v| v.0);
                    eprintln!(
                        "[MEMGVN_OP] fn={} blk={} op={oi} {:?} kind={:?} region={:?} def={defv:?} use={usev:?}",
                        func.name,
                        bid.0,
                        o.opcode,
                        o.attrs.get("_original_kind"),
                        mem.slot_access.region_at(&alias, (bid, oi), o),
                    );
                }
            }
        }
    }

    // Iterate the recorded Uses in a deterministic order (block id, then op
    // index) so leader selection and the resulting rewrites are stable.
    let mut use_positions: Vec<(BlockId, usize)> = mem.uses.keys().copied().collect();
    use_positions.sort_unstable_by_key(|(b, i)| (b.0, *i));

    for (block, op_idx) in use_positions {
        let Some(load_op) = func.blocks.get(&block).and_then(|b| b.ops.get(op_idx)) else {
            continue;
        };
        if diag {
            eprintln!(
                "[MEMGVN_USE0] fn={} blk={} op={op_idx} opcode={:?} nops={} value_attr={:?} kind={:?} typed_slot={:?}",
                func.name,
                block.0,
                load_op.opcode,
                load_op.operands.len(),
                load_op.attrs.get("value"),
                load_op.attrs.get("_original_kind"),
                load_op.plain_typed_slot_load(),
            );
        }
        // Only proven-pure typed-slot loads carry a forwardable (obj, offset).
        let Some((load_obj, load_offset)) = load_op.plain_typed_slot_load() else {
            continue;
        };
        let load_root = alias.root(load_obj);
        let load_result = load_op.results[0];

        let Some(reaching) = mem.reaching_def_for_use(block, op_idx) else {
            continue;
        };

        if diag {
            let in_store = store_def_slot.get(&reaching.0).copied();
            eprintln!(
                "[MEMGVN_USE] fn={} blk={} op={op_idx} root={:?} off={load_offset} reaching=v{} \
                 store_slot={:?} is_direct={} leader={:?}",
                func.name,
                block.0,
                load_root.0,
                reaching.0,
                in_store.map(|(r, o, _)| (r.0, o)),
                mem.is_direct_def_of_use(reaching, block, op_idx),
                load_leader
                    .get(&(reaching.0, load_root, load_offset))
                    .map(|(b, v)| (b.0, v.0)),
            );
        }

        // ── 1. Store-to-load forwarding ────────────────────────────────────
        // The reaching def must be EXACTLY this store version (a single direct
        // def — never a phi or an intervening clobber) AND name the same slot.
        if let Some(&(store_root, store_offset, stored_value)) = store_def_slot.get(&reaching.0)
            && store_root == load_root
            && store_offset == load_offset
            && mem.is_direct_def_of_use(reaching, block, op_idx)
        {
            // Locate the store's defining block to check dominance / strict
            // reachability of the forwarded value into the load's block.
            if let Some(MemAccess::Def {
                block: store_block, ..
            }) = mem.access(reaching)
                && value_reaches_use(*store_block, block, &idoms, &strict_reachable)
            {
                forwards.push(Forward {
                    block,
                    op_idx,
                    source: stored_value,
                });
                // A forwarded load is itself a witness of (version, slot): a
                // later load of the same slot under the same version may copy
                // from this load's result too. Register it as the leader if
                // none exists yet.
                load_leader
                    .entry((reaching.0, load_root, load_offset))
                    .or_insert((block, load_result));
                continue;
            }
        }

        // ── 2. Redundant-load elimination ──────────────────────────────────
        // A prior load of the same slot under the same reaching version, whose
        // block dominates this one (and is strict-CFG-reachable), is a valid
        // source for a Copy.
        let key = (reaching.0, load_root, load_offset);
        if let Some(&(leader_block, leader_result)) = load_leader.get(&key) {
            if value_reaches_use(leader_block, block, &idoms, &strict_reachable)
                && leader_result != load_result
            {
                forwards.push(Forward {
                    block,
                    op_idx,
                    source: leader_result,
                });
                continue;
            }
            // Leader is not in scope for this use (e.g. a sibling block) — this
            // load becomes a fresh leader for its own dominated region.
            load_leader.insert(key, (block, load_result));
        } else {
            load_leader.insert(key, (block, load_result));
        }
    }

    // Apply the rewrites. Each forwarded LoadAttr becomes, IN PLACE:
    //
    //     IncRef(source)
    //     Copy(source) -> r       (r = the load's original result ValueId)
    //
    // The `IncRef` reproduces the +1 the owned-result load performed (see the
    // module-level "Refcount safety" note — dropping it is a use-after-free);
    // the `Copy` is the representation-transparent value move `copy_prop`/`dce`
    // resolve. The load's result ValueId is preserved so downstream uses and the
    // value's repr are unchanged.
    //
    // Inserting the `IncRef` shifts every op index at/after the insertion point,
    // so per block we apply forwards in DESCENDING op_idx order: a rewrite at a
    // higher index never perturbs the index of a still-pending lower one.
    let mut by_block: HashMap<BlockId, Vec<&Forward>> = HashMap::new();
    for fwd in &forwards {
        by_block.entry(fwd.block).or_default().push(fwd);
    }
    for (block_id, mut block_forwards) in by_block {
        block_forwards.sort_unstable_by_key(|f| std::cmp::Reverse(f.op_idx));
        let Some(block) = func.blocks.get_mut(&block_id) else {
            continue;
        };
        for fwd in block_forwards {
            if fwd.op_idx >= block.ops.len() {
                continue;
            }
            // Defensive: only rewrite if it is still the load we planned for.
            if block.ops[fwd.op_idx].opcode != OpCode::LoadAttr {
                continue;
            }
            let old = block.ops[fwd.op_idx].clone();
            let result = old.results[0];
            // Replace the load with the value Copy …
            let mut copy_op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![fwd.source],
                results: vec![result],
                attrs: Default::default(),
                source_span: None,
            };
            copy_op.inherit_source_from(&old);
            block.ops[fwd.op_idx] = copy_op;
            // … and acquire the reference the load used to acquire, immediately
            // before the Copy so `r` is owned at every use the load dominated.
            let mut inc_ref = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::IncRef,
                operands: vec![fwd.source],
                results: vec![],
                attrs: Default::default(),
                source_span: None,
            };
            inc_ref.inherit_source_from(&old);
            block.ops.insert(fwd.op_idx, inc_ref);
            stats.values_changed += 1;
            stats.ops_added += 1;
        }
    }

    stats
}

/// True when a value defined in `def_block` provably reaches a use in
/// `use_block`: `def_block` dominates `use_block`, and (for a cross-block
/// forward) both blocks are strict-CFG-reachable so the post-lowering verifier
/// accepts the new operand. Same-block forwards bypass the strict-CFG check
/// (the verifier orders same-block defs/uses by op index, and the MemorySSA
/// renaming walk guarantees the def precedes the use within the block).
fn value_reaches_use(
    def_block: BlockId,
    use_block: BlockId,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    strict_reachable: &std::collections::HashSet<BlockId>,
) -> bool {
    if !dominates(def_block, use_block, idoms) {
        return false;
    }
    if def_block == use_block {
        return true;
    }
    strict_reachable.contains(&def_block) && strict_reachable.contains(&use_block)
}
