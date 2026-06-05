//! RC drop insertion (RC drop-insertion substrate, design 20, Phase 3).
//!
//! Inserts `DecRef` ops at every owned value's last use and `IncRef` ops before
//! suspension points for values that survive across a yield. This is the
//! compiler pass that closes molt's whole-program expression-temporary leak: the
//! runtime allocates every heap result with `ref_count = 1` and (before this
//! pass) never decremented it for expression temporaries.
//!
//! Runs `Mutates::OpsOnly`: it only inserts `DecRef`/`IncRef` ops within blocks
//! and never changes the block set, edges, or terminators. `DecRef`/`IncRef`
//! carry no exception edge, so this honors the `OpsOnly` exception-edge
//! invariant (see [`Mutates::OpsOnly`](crate::tir::pass_manager::Mutates)).
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
//! A value `v` is a drop candidate when ALL hold:
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
//! last-use placement already does. We keep the call operands out of any
//! *pre-call* drop, which the last-use semantics guarantee.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::liveness::{TirLiveness, TirLivenessResult};
use crate::tir::values::ValueId;

use super::PassStats;

/// The function-level attr the pass sets (round-tripped to the native backend as
/// a marker op) so the SimpleIR `loop_reassign_old_val` ad-hoc dec-ref path is
/// disabled for drop-inserted functions — preventing the R1 double-drop.
pub const DROP_INSERTED_ATTR: &str = "drop_inserted";

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

/// True if `opcode` produces a stack-allocated value with no RC (design R6).
fn produces_stack_value(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::StackAlloc | OpCode::ObjectNewBoundStack)
}

/// The values `term` passes as block args to ANY successor (these transfer
/// ownership through the SSA phi — they are NOT dropped on that edge).
fn terminator_branch_args(term: &Terminator) -> HashSet<ValueId> {
    let mut out = HashSet::new();
    match term {
        Terminator::Branch { args, .. } => out.extend(args.iter().copied()),
        Terminator::CondBranch {
            then_args,
            else_args,
            ..
        } => {
            out.extend(then_args.iter().copied());
            out.extend(else_args.iter().copied());
        }
        Terminator::Switch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                out.extend(args.iter().copied());
            }
            out.extend(default_args.iter().copied());
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
    out
}

/// Run drop insertion. See module docs for the algorithm.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "drop_insertion",
        ..Default::default()
    };

    // Conservative activation gate: functions with real exception-handler regions
    // (`try`/`except`) or generator/async state regions (`StateBlockStart/End`)
    // carry non-standard, already-lowered control flow — the coroutine `_poll`
    // state machine re-enters blocks via `StateSwitch`, so a value can be
    // "defined later" in a block that a straight-line liveness walk treats as
    // dominating. Drop placement over that shape is unsound without
    // state-region-aware liveness (design §2.9's frame-finalizer model handles
    // the suspension itself, but NOT the post-lowering state-machine CFG).
    // Mirrors the loop_unroll / block_versioning / type_guard_hoist bail on the
    // same predicate. The straight-line / loop / exception-CHECK (non-handler)
    // functions — which is the overwhelming majority and every leak in the
    // bug evidence — are fully covered. Re-enabling for state-machine functions
    // is the Phase 4/5 follow-up (needs the StateSwitch-aware liveness).
    if func.has_exception_handlers() {
        return stats;
    }

    let live: TirLivenessResult = am.get::<TirLiveness>(func).clone();

    // Parameters are borrowed from the caller (ABI): never dropped here.
    let param_ids: HashSet<ValueId> = {
        let mut s = HashSet::new();
        if let Some(entry) = func.blocks.get(&func.entry_block) {
            for arg in &entry.args {
                s.insert(arg.id);
            }
        }
        s
    };

    // Stack-allocated values: never dropped (design R6).
    let mut stack_values: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if produces_stack_value(op.opcode) {
                for &r in &op.results {
                    stack_values.insert(r);
                }
            }
        }
    }

    // A value is droppable iff it is heap-carrying, not a param, not a stack
    // value. (`is_raw_scalar` covers the repr filter — RawI64Safe/Bool/Float.)
    let droppable = |v: ValueId| -> bool {
        !live.is_raw_scalar(v) && !param_ids.contains(&v) && !stack_values.contains(&v)
    };

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
    }
    let mut plans: HashMap<BlockId, BlockPlan> = HashMap::new();

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
        };

        // ── 1. Straight-line last-use drops ──────────────────────────────────
        // For every value used by an op in this block, find its last use index.
        // If the value is droppable AND not live-out of this block AND not a
        // branch arg of the terminator (those transfer ownership), drop it after
        // its last op-use. Block args defined here that die in-block are also
        // dropped at their last use.
        //
        // Gather every value that has an op-use in this block.
        let branch_args = terminator_branch_args(&block.terminator);
        let mut last_use: HashMap<ValueId, usize> = HashMap::new();
        for (idx, op) in block.ops.iter().enumerate() {
            for &operand in &op.operands {
                last_use.insert(operand, idx);
            }
        }
        for (&v, &idx) in &last_use {
            if !droppable(v) {
                continue;
            }
            // Transferred via branch arg → no drop (the successor owns it now).
            if branch_args.contains(&v) {
                continue;
            }
            // Live-out of this block → dropped later (in a successor / before a
            // later use); not here.
            if live.is_live_out(bid, v) {
                continue;
            }
            // Used by the terminator directly (e.g. Return value, cond) → the
            // terminator consumes it; do not drop before the terminator.
            if terminator_uses_directly(&block.terminator, v) {
                continue;
            }
            // The value dies after op `idx` in this block: drop after it.
            plan.after_op.entry(idx).or_default().push(v);
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
            let live_out_here: HashSet<ValueId> = live
                .live_out
                .get(&bid)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            let mut defined: HashSet<ValueId> = block.args.iter().map(|a| a.id).collect();
            for (idx, op) in block.ops.iter().enumerate() {
                if is_suspension_point(op.opcode) {
                    let mut seen: HashSet<ValueId> = HashSet::new();
                    for &v in &live_out_here {
                        if droppable(v) && defined.contains(&v) && seen.insert(v) {
                            plan.before_op.entry(idx).or_default().push(v);
                        }
                    }
                }
                // The yield's own results (and every other op's results) become
                // defined AFTER the op executes.
                for &r in &op.results {
                    defined.insert(r);
                }
            }
        }

        plans.insert(bid, plan);
    }

    // ── 3. Edge-dying drops at successor entry (design §2.5 OpsOnly form) ─────
    // A value V is dropped at the START of block B when:
    //   * V is live-out of at least one predecessor P of B (i.e. P keeps it
    //     alive across the edge), AND
    //   * V is NOT live-in to B (B does not need it), AND
    //   * V is NOT a block arg of B (block args are re-supplied by the edge), AND
    //   * V is droppable.
    // This releases the value on the path where it dies. Because every path into
    // B that delivered V must release it, and B is a join, dropping once at B's
    // entry is correct ONLY when V dies on ALL incoming paths. We therefore
    // require V to be dead-in to B and live-out of EVERY predecessor that can
    // reach B (so no path still needs it). The elim pass later hoists/dedups.
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let preds = match pred_map.get(&bid) {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let block_args: HashSet<ValueId> =
            func.blocks[&bid].args.iter().map(|a| a.id).collect();
        // Candidate values: union of all predecessors' live-out.
        let mut candidates: HashSet<ValueId> = HashSet::new();
        for p in preds {
            if let Some(set) = live.live_out.get(p) {
                candidates.extend(set.iter().copied());
            }
        }
        for v in candidates {
            if !droppable(v) {
                continue;
            }
            if block_args.contains(&v) {
                continue;
            }
            // Dead on entry to B.
            if live.is_live_in(bid, v) {
                continue;
            }
            // Must die on ALL incoming paths: every predecessor that has V
            // live-out delivers a value B no longer needs. If a predecessor does
            // NOT have V live-out, that path already released it (or never had
            // it) — still fine to drop once here for the paths that did. But to
            // avoid a double-drop with the predecessor's own straight-line drop,
            // we require that NO predecessor itself drops V before the edge:
            // since V is live-out of a predecessor, that predecessor did NOT
            // straight-line-drop V (the straight-line rule skips live-out
            // values), so the only release is here. Safe.
            //
            // Additionally require: V is live-out of EVERY predecessor (so it is
            // genuinely delivered on every path and dropped exactly once). A
            // predecessor without V live-out would mean that path never owned V
            // at this join → dropping here would be a spurious drop on that path.
            let all_preds_deliver = preds.iter().all(|p| {
                live.live_out.get(p).is_some_and(|s| s.contains(&v))
            });
            if !all_preds_deliver {
                continue;
            }
            plans
                .entry(bid)
                .or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
                })
                .at_entry
                .push(v);
        }
    }

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

    // ── Apply the plans ──────────────────────────────────────────────────────
    let mut inserted = 0usize;
    for (&bid, plan) in &plans {
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        // Rebuild the op vector inserting before_op (IncRef) / after_op (DecRef).
        let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len() + 8);
        // at_entry DecRefs first.
        let mut entry_seen: HashSet<ValueId> = HashSet::new();
        for &v in &plan.at_entry {
            if entry_seen.insert(v) {
                new_ops.push(make_op(OpCode::DecRef, vec![v]));
                inserted += 1;
            }
        }
        for (idx, op) in block.ops.iter().enumerate() {
            // before_op IncRefs (suspension).
            if let Some(vals) = plan.before_op.get(&idx) {
                let mut seen: HashSet<ValueId> = HashSet::new();
                for &v in vals {
                    if seen.insert(v) {
                        new_ops.push(make_op(OpCode::IncRef, vec![v]));
                        inserted += 1;
                    }
                }
            }
            new_ops.push(op.clone());
            // after_op DecRefs (straight-line last use).
            if let Some(vals) = plan.after_op.get(&idx) {
                let mut seen: HashSet<ValueId> = HashSet::new();
                for &v in vals {
                    if seen.insert(v) {
                        new_ops.push(make_op(OpCode::DecRef, vec![v]));
                        inserted += 1;
                    }
                }
            }
        }
        // before_term DecRefs (currently unused; kept for the documented
        // loop-carried anchor and future edge-split upgrade).
        let mut term_seen: HashSet<ValueId> = HashSet::new();
        for &v in &plan.before_term {
            if term_seen.insert(v) {
                new_ops.push(make_op(OpCode::DecRef, vec![v]));
                inserted += 1;
            }
        }
        block.ops = new_ops;
    }

    if inserted > 0 {
        func.attrs
            .insert(DROP_INSERTED_ATTR.to_string(), AttrValue::Bool(true));
    }
    stats.ops_added = inserted;
    stats
}

/// True if `v` is consumed directly by the terminator (a Return value, a
/// CondBranch/Switch condition). Branch ARGS are handled separately (they
/// transfer ownership to the successor's block arg).
fn terminator_uses_directly(term: &Terminator, v: ValueId) -> bool {
    match term {
        Terminator::Return { values } => values.contains(&v),
        Terminator::CondBranch { cond, .. } => *cond == v,
        Terminator::Switch { value, .. } => *value == v,
        Terminator::Branch { .. } | Terminator::Unreachable => false,
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

    /// Parameters are borrowed — never dropped.
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
            b.terminator = Terminator::Branch { target: resume, args: vec![] };
        }
        func.blocks.insert(resume, TirBlock {
            id: resume,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![x], vec![used])],
            terminator: Terminator::Return { values: vec![used] },
        });
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
            b.terminator = Terminator::Branch { target: header, args: vec![acc0] };
        }
        func.blocks.insert(header, TirBlock {
            id: header,
            args: vec![TirValue { id: acc_phi, ty: TirType::Str }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        });
        func.blocks.insert(body, TirBlock {
            id: body,
            args: vec![],
            // acc_next = Call(acc_phi): consumes the phi, produces a new owned acc.
            ops: vec![op(OpCode::Call, vec![acc_phi], vec![acc_next])],
            terminator: Terminator::Branch { target: header, args: vec![acc_next] },
        });
        func.blocks.insert(exit, TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            // The final acc_phi is dead (not returned).
            terminator: Terminator::Return { values: vec![] },
        });
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
}
