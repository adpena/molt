//! MemGVN — store-to-load forwarding + redundant-load elimination (S5-2b).
//!
//! The first consumer of the [`MemorySSA`](super::memory_ssa::MemorySSA)
//! substrate (S5-2a). It performs two memory redundancy eliminations over the
//! reaching-def graph MemorySSA provides:
//!
//! 1. **Store-to-load forwarding.** A proven-pure typed-slot load
//!    (`r = obj.<offset>`) whose single reaching memory version is exactly a
//!    typed-slot store (`obj.<offset> = v`) to the **same object root and the
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
//! MemorySSA's reaching-def is *region-aware*, but the Phase-1 alias oracle
//! ([`AliasAnalysisResult::region_of`]) degrades every typed-slot access of a
//! *heap* object to [`MemRegion::GenericHeap`] (no concrete class id yet —
//! `class_of` returns `None`). Under that coarse region, a store to offset 0
//! and a store to offset 8 of the same object both classify as `GenericHeap`,
//! which may-aliases itself — so MemorySSA's `is_direct_def_of_use` answers the
//! question "is this store the most-recent memory version this load observes?"
//! but NOT "do the store and load name the same byte". A *stack* object refines
//! to a `StackObject { root }` region (still offset-blind in Phase 1). In every
//! case the reaching-def relation is **necessary but not sufficient** for
//! forwarding: this pass adds the *sufficient* conditions —
//!
//!   * same object root (transparent-alias-resolved), and
//!   * same statically-known field offset,
//!
//! both read off the concrete `StoreAttr` / `LoadAttr` operands. Only when the
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
//! `molt_guarded_field_get_ptr` / `molt_object_field_get` path
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
//! holds for both forwarding flavors: a store independently takes the slot's own
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
//! (`IncRef`/`Copy` are pure, non-throwing, non-terminator ops — exactly the
//! IncRef/DecRef class `escape_analysis`, also `OpsOnly`, adds and removes). The
//! CFG-structure analyses stay valid; the ops-sensitive caches (DefMap,
//! AliasAnalysis, MemorySSA) are dropped by the manager's `invalidate_ops`
//! afterward.
//!
//! [`AliasAnalysisResult::region_of`]: super::alias_analysis::AliasAnalysisResult::region_of
//! [`MemRegion::GenericHeap`]: super::alias_analysis::MemRegion::GenericHeap
//! [`MemRegion::StackObject`]: super::alias_analysis::MemRegion::StackObject

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

/// `Some((obj, offset))` for a proven-pure typed-slot load `r = obj.<offset>`.
///
/// This is exactly the load set the alias oracle classifies
/// [`LoadPurity::ProvenPure`](super::alias_analysis::LoadPurity::ProvenPure)
/// (`load_attr_is_typed_slot`: `_original_kind ∈ {load, guarded_field_get}`),
/// and MemorySSA records as a [`MemAccess::Use`]. The object operand is always
/// `operands[0]`; the offset is the `value` attr — read the SAME way
/// `alias_analysis::region_of` reads them (`op.operands.first()` +
/// `load_attr_offset`), so the slot identity here matches the region identity
/// MemorySSA reasoned over.
///
/// Production note: the lowered guarded form is
/// `guarded_field_get [obj, class_bits, expected]` (3 operands, the inline
/// class-version guard ABI), not a bare 1-operand load. We therefore gate on
/// the *kind*, not the arity — `operands[0]` is the object in every spelling.
fn typed_slot_load(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.opcode != OpCode::LoadAttr || op.operands.is_empty() || op.results.len() != 1 {
        return None;
    }
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(kind, "load" | "guarded_field_get") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => Some((op.operands[0], *offset)),
        _ => None,
    }
}

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
    // slot. Only typed-slot stores (`store`/`store_init`) are forwardable
    // sources; a `GenericHeap` clobber def (a call/raise/yield) has no slot
    // value and is never in this map — a load reaching such a def is correctly
    // not forwarded.
    let mut store_def_slot: HashMap<u32, (ValueId, i64, ValueId)> = HashMap::new();
    for access in mem.defs.values() {
        if let MemAccess::Def {
            ver, block, op_idx, ..
        } = access
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
                        alias.region_of(o),
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
                typed_slot_load(load_op),
            );
        }
        // Only proven-pure typed-slot loads carry a forwardable (obj, offset).
        let Some((load_obj, load_offset)) = typed_slot_load(load_op) else {
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
            let result = block.ops[fwd.op_idx].results[0];
            let source_span = block.ops[fwd.op_idx].source_span;
            // Replace the load with the value Copy …
            block.ops[fwd.op_idx] = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![fwd.source],
                results: vec![result],
                attrs: Default::default(),
                source_span,
            };
            // … and acquire the reference the load used to acquire, immediately
            // before the Copy so `r` is owned at every use the load dominated.
            block.ops.insert(
                fwd.op_idx,
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::IncRef,
                    operands: vec![fwd.source],
                    results: vec![],
                    attrs: Default::default(),
                    source_span,
                },
            );
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

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

    /// `obj.<offset> = val` typed-slot store.
    fn store(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
        let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        o
    }

    /// `r = obj.<offset>` proven-pure typed-slot load.
    fn load(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
        let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("load".into()));
        o
    }

    /// An opaque call that clobbers `GenericHeap`.
    fn call(args: Vec<ValueId>, r: ValueId) -> TirOp {
        op(OpCode::Call, args, vec![r])
    }

    fn run_fresh(func: &mut TirFunction) -> PassStats {
        let mut am = AnalysisManager::new();
        run(func, &mut am)
    }

    // ── 1. Simple same-block store-to-load forwarding ──────────────────────

    #[test]
    fn forward_same_block_store_to_load() {
        // store(obj, val, 0); r = load(obj, 0); return r
        // → the load becomes Copy(val).
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(stats.values_changed, 1, "the load is forwarded");
        let ops = &func.blocks[&func.entry_block].ops;
        // store@0; the load@1 becomes IncRef(val)@1 + Copy(val)->r@2 (the
        // IncRef reproduces the owned-result +1 the load performed).
        assert_eq!(
            ops[1].opcode,
            OpCode::IncRef,
            "owned-ref acquired before the Copy"
        );
        assert_eq!(ops[1].operands, vec![val], "IncRef of the forwarded value");
        assert_eq!(ops[2].opcode, OpCode::Copy, "load rewritten to Copy");
        assert_eq!(ops[2].operands, vec![val], "copies the stored value");
        assert_eq!(ops[2].results, vec![r], "result ValueId preserved");
        assert!(ops[2].attrs.is_empty(), "pure SSA move — no _original_kind");
    }

    // ── 2. Forward blocked by an interposed call (GenericHeap clobber) ─────

    #[test]
    fn forward_blocked_by_interposed_call() {
        // store(obj, val, 0); call(obj); r = load(obj, 0)
        // The call is a GenericHeap def between store and load → the load's
        // reaching def is the call, not the store → NOT forwarded.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let call_r = func.fresh_value();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(call(vec![obj], call_r));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(stats.values_changed, 0, "clobbering call blocks forwarding");
        assert_eq!(
            func.blocks[&func.entry_block].ops[2].opcode,
            OpCode::LoadAttr,
            "load stays a real LoadAttr across the call barrier"
        );
    }

    // ── 3. Forward blocked by a different offset (must-alias slot) ─────────

    #[test]
    fn forward_blocked_by_different_offset() {
        // store(obj, val, 8); r = load(obj, 0)
        // Different offset → the load does NOT read the store's bytes → NOT
        // forwarded, even though MemorySSA's coarse GenericHeap region makes
        // the store the load's reaching def.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 8));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "offset mismatch must block forwarding (different slot)"
        );
        assert_eq!(
            func.blocks[&func.entry_block].ops[1].opcode,
            OpCode::LoadAttr
        );
    }

    /// Same object, same offset, but a SECOND store to a DIFFERENT offset in
    /// between must not disturb the offset-0 forward: the offset-0 load still
    /// reaches the offset-0 store's bytes. (Phase-1 GenericHeap regions make the
    /// offset-8 store the most-recent reaching def, so MemorySSA reports the
    /// offset-8 version — and our offset check correctly REFUSES to forward the
    /// offset-8 value into the offset-0 load. Fail-closed: a missed forward, not
    /// a wrong one.)
    #[test]
    fn interposed_other_offset_store_does_not_misforward() {
        // store(obj, v0, 0); store(obj, v8, 8); r = load(obj, 0)
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v0 = ValueId(1);
        let v8 = ValueId(2);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, v0, 0));
            entry.ops.push(store(obj, v8, 8));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        // The reaching def of load@0 is the offset-8 store (coarse region), and
        // its offset (8) != the load's (0), so no forward fires. Crucially we
        // never forward v8 into the offset-0 load.
        assert_eq!(
            stats.values_changed, 0,
            "must not forward the offset-8 value into the offset-0 load"
        );
    }

    // ── 4. Cross-block forward through a single dominating def ─────────────

    #[test]
    fn forward_cross_block_through_dominating_store() {
        // bb0: store(obj, val, 0) → bb1 → bb2: r = load(obj, 0)
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb2,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![load(obj, 0, r)],
                terminator: Terminator::Return { values: vec![r] },
            },
        );
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "cross-block forward through linear chain"
        );
        let bb2_ops = &func.blocks[&bb2].ops;
        assert_eq!(
            bb2_ops[0].opcode,
            OpCode::IncRef,
            "owned-ref acquired in the use block"
        );
        assert_eq!(bb2_ops[0].operands, vec![val]);
        assert_eq!(bb2_ops[1].opcode, OpCode::Copy);
        assert_eq!(bb2_ops[1].operands, vec![val]);
        assert_eq!(bb2_ops[1].results, vec![r]);
    }

    // ── 5. NO forward through a MemoryPhi merge ────────────────────────────

    #[test]
    fn forward_blocked_by_memory_phi_merge() {
        // bb0 -> {bb1: store(obj,v1,0), bb2: store(obj,v2,0)} -> bb3: r = load(obj,0)
        // The join places a MemoryPhi; the load reads the phi version, which is
        // not any single store's version → NOT forwarded.
        let mut func = TirFunction::new(
            "f".into(),
            vec![
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
                TirType::Bool,
            ],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let cond = ValueId(3);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![store(obj, v1, 0)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![store(obj, v2, 0)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![load(obj, 0, r)],
                terminator: Terminator::Return { values: vec![r] },
            },
        );
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "a phi-merged load has no single direct store def — forwarding blocked"
        );
        assert_eq!(func.blocks[&bb3].ops[0].opcode, OpCode::LoadAttr);
    }

    // ── 6. Redundant-load elimination, same block ──────────────────────────

    #[test]
    fn redundant_load_elim_same_block() {
        // r1 = load(obj, 0); r2 = load(obj, 0); return r1 + r2
        // No store and no clobber between → both loads read LIVE_ON_ENTRY for
        // the same slot → the second collapses to Copy(r1).
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
        let obj = ValueId(0);
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        let sum = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(load(obj, 0, r1));
            entry.ops.push(load(obj, 0, r2));
            entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
            entry.terminator = Terminator::Return { values: vec![sum] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(stats.values_changed, 1, "the second load is redundant");
        let ops = &func.blocks[&func.entry_block].ops;
        // load@0 stays the leader; the redundant load@1 becomes IncRef(r1)@1 +
        // Copy(r1)->r2@2 (each owned load duplicates the +1, so r2 must too).
        assert_eq!(ops[0].opcode, OpCode::LoadAttr, "first load is the leader");
        assert_eq!(
            ops[1].opcode,
            OpCode::IncRef,
            "second load's owned +1 is reacquired"
        );
        assert_eq!(ops[1].operands, vec![r1]);
        assert_eq!(ops[2].opcode, OpCode::Copy, "second load reuses the first");
        assert_eq!(ops[2].operands, vec![r1]);
        assert_eq!(ops[2].results, vec![r2]);
    }

    // ── 7. Redundant-load blocked by a clobber between the two loads ───────

    #[test]
    fn redundant_load_blocked_by_clobber() {
        // r1 = load(obj, 0); call(obj); r2 = load(obj, 0)
        // The call clobbers the slot (GenericHeap def) → the two loads read
        // DIFFERENT memory versions → the second is NOT redundant.
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
        let obj = ValueId(0);
        let r1 = func.fresh_value();
        let call_r = func.fresh_value();
        let r2 = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(load(obj, 0, r1));
            entry.ops.push(call(vec![obj], call_r));
            entry.ops.push(load(obj, 0, r2));
            entry.terminator = Terminator::Return { values: vec![r2] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "a clobber between two loads makes the second non-redundant"
        );
        assert_eq!(
            func.blocks[&func.entry_block].ops[2].opcode,
            OpCode::LoadAttr
        );
    }

    /// Two loads of the SAME slot in non-dominating sibling blocks must NOT
    /// collapse: neither leader dominates the other.
    #[test]
    fn redundant_load_not_across_sibling_blocks() {
        // bb0 cond → {bb1: r1 = load(obj,0)} / {bb2: r2 = load(obj,0)} → bb3
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::Bool],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let cond = ValueId(1);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        let arg = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![load(obj, 0, r1)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![r1],
                },
            },
        );
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![load(obj, 0, r2)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![r2],
                },
            },
        );
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![crate::tir::values::TirValue {
                    id: arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![arg] },
            },
        );
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "sibling-block loads must not collapse (no dominance)"
        );
        assert_eq!(func.blocks[&bb1].ops[0].opcode, OpCode::LoadAttr);
        assert_eq!(func.blocks[&bb2].ops[0].opcode, OpCode::LoadAttr);
    }

    // ── 8. Different objects are not forwarded ─────────────────────────────

    #[test]
    fn forward_blocked_by_different_object() {
        // store(a, val, 0); r = load(b, 0)  — distinct objects, same offset.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let a = ValueId(0);
        let b = ValueId(1);
        let val = ValueId(2);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(a, val, 0));
            entry.ops.push(load(b, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        // The reaching def of load(b) is the store(a) version (coarse
        // GenericHeap region may-aliases), but the roots differ → NOT forwarded.
        assert_eq!(
            stats.values_changed, 0,
            "distinct object roots must block forwarding"
        );
    }

    /// A transparent Copy alias of the object is recognized: store through the
    /// root, load through an alias of the same root → forwarded.
    #[test]
    fn forward_through_transparent_alias() {
        // a = Copy(obj); store(obj, val, 0); r = load(a, 0)  → Copy(val)
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let a = func.fresh_value();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(op(OpCode::Copy, vec![obj], vec![a]));
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(a, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "load through a transparent alias of the store target forwards"
        );
        // Copy(obj)->a@0; store@1; load@2 → IncRef(val)@2 + Copy(val)->r@3.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[2].opcode, OpCode::IncRef);
        assert_eq!(ops[2].operands, vec![val]);
        assert_eq!(ops[3].opcode, OpCode::Copy);
        assert_eq!(ops[3].operands, vec![val]);
    }

    // ── Production op-shape coverage ───────────────────────────────────────

    /// The production guarded-load ABI is `guarded_field_get [obj, class_bits,
    /// expected]` (3 operands — the inline class-version guard), NOT a bare
    /// 1-operand `load`. `typed_slot_load` must accept it (object = operand[0]).
    /// This pins the arity contract that, when violated, silently disables the
    /// pass on every real program (the S5-2b recon finding).
    #[test]
    fn guarded_field_get_three_operand_form_is_a_typed_slot_load() {
        let mut o = op(
            OpCode::LoadAttr,
            vec![ValueId(0), ValueId(1), ValueId(2)],
            vec![ValueId(3)],
        );
        o.attrs.insert("value".into(), AttrValue::Int(8));
        o.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("guarded_field_get".into()),
        );
        assert_eq!(
            typed_slot_load(&o),
            Some((ValueId(0), 8)),
            "the 3-operand guarded load must classify as a typed-slot load of operand[0]@offset"
        );
    }

    /// Two production-shaped guarded loads of the same field with no clobber
    /// between collapse: the second `guarded_field_get` becomes Copy of the
    /// first load's result. Exercises the real op shape end to end through the
    /// pass (not the synthetic 1-operand `load`).
    #[test]
    fn redundant_guarded_field_get_collapses() {
        fn gget(obj: ValueId, cls: ValueId, ver: ValueId, offset: i64, r: ValueId) -> TirOp {
            let mut o = op(OpCode::LoadAttr, vec![obj, cls, ver], vec![r]);
            o.attrs.insert("value".into(), AttrValue::Int(offset));
            o.attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("guarded_field_get".into()),
            );
            // The class the frontend proved (S5-1.5): makes the op classify as a
            // precise `TypedField` region — the production shape MemGVN consumes.
            o.attrs
                .insert("_class".into(), AttrValue::Str("Point".into()));
            o
        }
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let cls = ValueId(1);
        let ver = ValueId(2);
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        let sum = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(gget(obj, cls, ver, 8, r1));
            entry.ops.push(gget(obj, cls, ver, 8, r2));
            entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
            entry.terminator = Terminator::Return { values: vec![sum] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "the second guarded load is redundant"
        );
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(
            ops[0].opcode,
            OpCode::LoadAttr,
            "first guarded load is the leader"
        );
        assert_eq!(ops[1].opcode, OpCode::IncRef, "the duplicated owned +1");
        assert_eq!(ops[1].operands, vec![r1]);
        assert_eq!(ops[2].opcode, OpCode::Copy, "second collapses to a Copy");
        assert_eq!(ops[2].operands, vec![r1]);
    }

    /// THE real-code unblock: two `guarded_field_get` of the same field with an
    /// interposed `CheckException` (the ubiquitous post-op exception check the
    /// frontend emits) still collapse. Before the alias-oracle fix that
    /// classifies `CheckException` as a non-clobbering flag read, the
    /// `CheckException` was a `GenericHeap` memory Def that bumped the memory
    /// version between the two reads — so the second read reached a DIFFERENT
    /// version and never deduped. Every real method body interleaves
    /// `CheckException` between field accesses, so this was why MemGVN fired on
    /// ZERO production functions (the S5-2b gate-6 finding). A `CheckException`
    /// reads the pending-exception flag and never writes heap, so the dedup is
    /// sound: on the no-exception path the slot is unchanged; on the exception
    /// path control leaves at the `CheckException` and the second load is never
    /// reached.
    #[test]
    fn redundant_guarded_load_across_check_exception_collapses() {
        fn gget(obj: ValueId, cls: ValueId, ver: ValueId, offset: i64, r: ValueId) -> TirOp {
            let mut o = op(OpCode::LoadAttr, vec![obj, cls, ver], vec![r]);
            o.attrs.insert("value".into(), AttrValue::Int(offset));
            o.attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("guarded_field_get".into()),
            );
            o.attrs
                .insert("_class".into(), AttrValue::Str("Point".into()));
            o
        }
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let cls = ValueId(1);
        let ver = ValueId(2);
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        let sum = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(gget(obj, cls, ver, 8, r1));
            // The post-load exception check — a flag read, NOT a heap write.
            entry.ops.push(op(OpCode::CheckException, vec![], vec![]));
            entry.ops.push(gget(obj, cls, ver, 8, r2));
            entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
            entry.terminator = Terminator::Return { values: vec![sum] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "an interposed CheckException must NOT block redundant-load elim"
        );
        let ops = &func.blocks[&func.entry_block].ops;
        // gget@0; CheckException@1; the redundant gget@2 → IncRef(r1)@2 +
        // Copy(r1)->r2@3 (the check stays exactly where it was).
        assert_eq!(
            ops[0].opcode,
            OpCode::LoadAttr,
            "first guarded load is the leader"
        );
        assert_eq!(
            ops[1].opcode,
            OpCode::CheckException,
            "the check is preserved"
        );
        assert_eq!(ops[2].opcode, OpCode::IncRef, "the duplicated owned +1");
        assert_eq!(ops[2].operands, vec![r1]);
        assert_eq!(
            ops[3].opcode,
            OpCode::Copy,
            "the second load collapses across the check"
        );
        assert_eq!(ops[3].operands, vec![r1]);
    }

    /// A raw `store_init` (2-operand constructor write) forwarded into a
    /// production guarded `guarded_field_get` of the same single field: the
    /// load becomes Copy of the stored value. This is the constructor-then-read
    /// pattern (no interposed other-offset store to block it).
    #[test]
    fn raw_store_forwards_into_guarded_field_get() {
        fn store_init(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
            let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
            o.attrs.insert("value".into(), AttrValue::Int(offset));
            o.attrs
                .insert("_original_kind".into(), AttrValue::Str("store_init".into()));
            o.attrs
                .insert("_class".into(), AttrValue::Str("Point".into()));
            o
        }
        fn gget(obj: ValueId, cls: ValueId, ver: ValueId, offset: i64, r: ValueId) -> TirOp {
            let mut o = op(OpCode::LoadAttr, vec![obj, cls, ver], vec![r]);
            o.attrs.insert("value".into(), AttrValue::Int(offset));
            o.attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("guarded_field_get".into()),
            );
            o.attrs
                .insert("_class".into(), AttrValue::Str("Point".into()));
            o
        }
        let mut func = TirFunction::new(
            "f".into(),
            vec![
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
            ],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let cls = ValueId(2);
        let ver = ValueId(3);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store_init(obj, val, 8));
            entry.ops.push(gget(obj, cls, ver, 8, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "the constructor store forwards into the guarded read"
        );
        // store_init@0; gget@1 → IncRef(val)@1 + Copy(val)->r@2.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::IncRef);
        assert_eq!(ops[1].operands, vec![val]);
        assert_eq!(ops[2].opcode, OpCode::Copy);
        assert_eq!(ops[2].operands, vec![val]);
    }

    // ── Unconditional production path ──────────────────────────────────────

    #[test]
    fn run_forwards_without_ambient_disable_path() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);
        assert_eq!(stats.values_changed, 1, "production pass forwards the load");
        assert_eq!(
            func.blocks[&func.entry_block].ops[1].opcode,
            OpCode::IncRef,
            "forwarded load acquires the owned reference"
        );
        assert_eq!(func.blocks[&func.entry_block].ops[2].opcode, OpCode::Copy);
    }

    // ── Refcount discipline (the soundness keystone) ───────────────────────

    /// EVERY forwarded load must be immediately preceded by an `IncRef` of the
    /// SAME source it copies. A typed-slot load returns an OWNED (+1) reference
    /// (`object_field_get_ptr_raw` unconditionally `inc_ref_bits`); a bare
    /// `Copy` would drop that +1 while the frontend's matching `DecRef` still
    /// runs → use-after-free. This test pins the `IncRef(source); Copy(source)`
    /// shape so a future "simplify to a plain Copy" regresses LOUDLY here, not
    /// as a silent heap-corruption miscompile in production.
    #[test]
    fn every_forward_acquires_a_reference() {
        // Two forwards in one block (a store-to-load AND a redundant-load), so
        // the descending-index apply order is exercised too:
        //   store(obj,val,0); r1 = load(obj,0); r2 = load(obj,0); sum=r1+r2
        // r1 forwards from the store; r2 is redundant against r1.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        let sum = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r1));
            entry.ops.push(load(obj, 0, r2));
            entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
            entry.terminator = Terminator::Return { values: vec![sum] };
        }
        let stats = run_fresh(&mut func);
        assert_eq!(stats.values_changed, 2, "both loads forward");
        assert_eq!(stats.ops_added, 2, "one IncRef inserted per forward");

        // Invariant: scanning the block, every `Copy` whose result was an
        // original load result is immediately preceded by `IncRef(sameSource)`.
        let ops = &func.blocks[&func.entry_block].ops;
        let mut checked = 0;
        for (i, o) in ops.iter().enumerate() {
            if o.opcode == OpCode::Copy && (o.results == vec![r1] || o.results == vec![r2]) {
                assert!(i >= 1, "a forwarded Copy must have a preceding op");
                let prev = &ops[i - 1];
                assert_eq!(prev.opcode, OpCode::IncRef, "Copy is preceded by IncRef");
                assert_eq!(
                    prev.operands, o.operands,
                    "the IncRef acquires exactly the value the Copy forwards"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 2, "both forwarded copies validated");
    }
}
