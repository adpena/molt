//! TIR liveness analysis (RC drop-insertion substrate, design 20, Phase 2).
//!
//! Standard backward dataflow liveness over the final-SSA TIR, with one
//! domain-specific twist: **representation-filtered live sets**. A value whose
//! physical carrier holds no refcounted heap obligation — a bare `i64`
//! (`Repr::RawI64Safe`), an inline bool (`Repr::Bool`), a bare `f64`
//! (`Repr::FloatUnboxed`), the `None` singleton/sentinel, or an unreachable
//! `Repr::Never` — is excluded from the live sets. The drop pass
//! consumes these sets to place `DecRef`s; a raw scalar carries no refcount, so
//! including it would lead the drop pass to emit a `DecRef` on a register that is
//! not a NaN-boxed pointer (a type confusion). Filtering here keeps the drop
//! pass's last-use placement automatically sound for the raw lanes — the
//! overflow-peel fast loop's accumulators receive ZERO drops structurally.
//!
//! ## Dataflow
//!
//! ```text
//! LiveOut[B] = ⋃ { LiveIn[S] | S ∈ succ(B) }
//! LiveIn[B]  = (LiveOut[B] \ Kill[B]) ∪ Use[B]
//! ```
//!
//! where, restricted to the heap-carrying values:
//! * `Use[B]`  — values read by ops in `B` before any in-block definition, plus
//!   the values `B`'s terminator passes as branch/condition args, plus the
//!   values predecessors deliver to `B`'s block args (those bind `B`'s args, so
//!   they are uses *of the predecessor*, accounted via the successor's block-arg
//!   live-in — see [`live_out_of`]).
//! * `Kill[B]` — values defined by ops in `B` (op results) and `B`'s own block
//!   args (an SSA def at block entry).
//!
//! Iterated to a fixpoint over a reverse-postorder block walk (back-edges
//! converge because the transfer functions are monotone over the finite value
//! set).
//!
//! ## Block-argument (phi) semantics
//!
//! TIR uses MLIR-style block arguments instead of phi nodes: a predecessor's
//! terminator passes a list of values that bind the successor's block args on
//! entry. The passed value is a *use* in the predecessor; the block arg is a
//! *def* (kill) in the successor. [`live_out_of`] threads this precisely: a
//! value live-in to a successor's body propagates to the predecessor's live-out
//! **only if** it is not one of the successor's block args (those are killed at
//! the successor's entry and re-supplied by the edge args, which are separately
//! counted as predecessor uses).

use std::collections::{HashMap, HashSet};

use crate::repr::Repr;
use crate::representation_plan::raw_i64_carrier_values_for;
use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Per-function liveness result: the heap-carrying live-in / live-out value sets
/// for every block, plus the raw-scalar exclusion set the drop pass reuses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TirLivenessResult {
    /// block → set of heap-carrying values live on entry.
    pub live_in: HashMap<BlockId, HashSet<ValueId>>,
    /// block → set of heap-carrying values live on exit (union of successors'
    /// live-in, minus successor block args supplied by this block's edges).
    pub live_out: HashMap<BlockId, HashSet<ValueId>>,
    /// Values whose physical carrier holds no refcounted heap obligation
    /// (RawI64Safe / Bool / FloatUnboxed / None / Never). Excluded from
    /// `live_in`/`live_out`; exposed so the
    /// drop pass can apply the identical filter to last-use candidates without
    /// recomputing the value-range proof.
    pub raw_scalars: HashSet<ValueId>,
}

impl TirLivenessResult {
    /// True iff `val` (a heap-carrying value) is live on entry to `block`.
    pub fn is_live_in(&self, block: BlockId, val: ValueId) -> bool {
        self.live_in
            .get(&block)
            .is_some_and(|set| set.contains(&val))
    }

    /// True iff `val` (a heap-carrying value) is live on exit from `block`.
    pub fn is_live_out(&self, block: BlockId, val: ValueId) -> bool {
        self.live_out
            .get(&block)
            .is_some_and(|set| set.contains(&val))
    }

    /// True iff `val`'s carrier holds no refcounted heap obligation. Such values
    /// are never dropped.
    pub fn is_raw_scalar(&self, val: ValueId) -> bool {
        self.raw_scalars.contains(&val)
    }

    /// The index of the LAST op in `block` that uses `val`, or `None` if `val`
    /// is not used by any op in the block. Terminator uses are NOT included
    /// (callers that must account for the terminator check `live_out` and the
    /// terminator args separately). A raw-scalar value always returns `None`
    /// from the live-set queries but `last_use_in_block` still reports its true
    /// last op-use position (the position query is repr-agnostic — the caller
    /// applies the repr filter before acting on it).
    pub fn last_use_in_block(&self, block: &TirBlock, val: ValueId) -> Option<usize> {
        let mut last = None;
        for (idx, op) in block.ops.iter().enumerate() {
            if op.operands.contains(&val) {
                last = Some(idx);
            }
        }
        last
    }
}

/// The successors of `block` under the terminator-only CFG (the edges that carry
/// SSA values via block args). Exception edges are handled by the drop pass's
/// CheckException logic, not by liveness propagation — at this analysis layer a
/// value live across a potentially-throwing op is captured by ordinary
/// straight-line liveness (the op is just another op in the block).
fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut out: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            out.push(*default);
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// The values `term` *uses* directly (the condition of a CondBranch, the switch
/// value, and Return values) — NOT the branch args, which are handled by the
/// successor block-arg propagation in [`live_out_of`].
fn terminator_direct_uses(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::Branch { .. } => vec![],
        Terminator::CondBranch { cond, .. } => vec![*cond],
        Terminator::Switch { value, .. } => vec![*value],
        // `StateDispatch` reads the saved state from the frame header at codegen
        // time, not an SSA value — it has no direct value use.
        Terminator::StateDispatch { .. } => vec![],
        Terminator::Return { values } => values.clone(),
        Terminator::Unreachable => vec![],
    }
}

/// For an edge `B → S`, the values `B` passes to bind `S`'s block args (indexed
/// by arg position). Returns the args delivered specifically to successor `succ`
/// on the matching edge.
fn edge_args_to(term: &Terminator, succ: BlockId) -> Vec<ValueId> {
    match term {
        Terminator::Branch { target, args } if *target == succ => args.clone(),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            let mut out = Vec::new();
            if *then_block == succ {
                out.extend(then_args.iter().copied());
            }
            if *else_block == succ {
                out.extend(else_args.iter().copied());
            }
            out
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
            let mut out = Vec::new();
            for (_, b, args) in cases {
                if *b == succ {
                    out.extend(args.iter().copied());
                }
            }
            if *default == succ {
                out.extend(default_args.iter().copied());
            }
            out
        }
        _ => vec![],
    }
}

/// Compute LiveOut[B] from the current LiveIn of B's successors.
///
/// A value `v` is live-out of `B` if some successor `S` needs it live on entry.
/// For block-argument SSA that means:
/// * `v` is live-in to `S` AND `v` is not one of `S`'s block args (block args are
///   defined at `S`'s entry — a `v` that aliases a block arg id is killed there,
///   not propagated back), OR
/// * `v` is passed by `B`'s edge to `S` as a block arg (an explicit use in `B`).
fn live_out_of(
    block: &TirBlock,
    live_in: &HashMap<BlockId, HashSet<ValueId>>,
    block_args: &HashMap<BlockId, HashSet<ValueId>>,
    heap_carrying: &dyn Fn(ValueId) -> bool,
    canon: &dyn Fn(ValueId) -> ValueId,
    keepalive_roots: &dyn Fn(ValueId) -> Vec<ValueId>,
) -> HashSet<ValueId> {
    let mut out = HashSet::new();
    for succ in terminator_successors(&block.terminator) {
        if let Some(succ_in) = live_in.get(&succ) {
            let succ_args = block_args.get(&succ);
            for &v in succ_in {
                // `v` is already an alias root (live sets are in root space).
                let is_succ_arg = succ_args.is_some_and(|a| a.contains(&v));
                if !is_succ_arg {
                    out.insert(v);
                }
            }
        }
        // Edge args this block supplies to the successor's block args are direct
        // uses in this block — they are live-out of this block (their value must
        // survive to the branch). Canonicalize to the alias root so a copied edge
        // arg keeps its underlying object live.
        for v in edge_args_to(&block.terminator, succ) {
            if heap_carrying(v) {
                out.insert(canon(v));
            }
            // A borrow result forwarded on an edge keeps its source object live-out
            // of this block (the source must reach the successor where the borrow
            // is consumed). Design 20 interior-borrow keepalive.
            for src_root in keepalive_roots(v) {
                out.insert(src_root);
            }
        }
    }
    out
}

/// Liveness analysis marker, cached by the [`AnalysisManager`].
///
/// [`AnalysisManager`]: crate::tir::analysis::AnalysisManager
pub struct TirLiveness;

impl Analysis for TirLiveness {
    type Result = TirLivenessResult;
    const ID: AnalysisId = AnalysisId::Liveness;
    // Liveness depends on the CFG edges (successor relation) and on the ops
    // within blocks (use/def positions), so it is invalidated by both.
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        compute_liveness(func)
    }
}

/// Floor a value's `TirType` to its representation and test whether that carrier
/// holds no refcounted heap obligation (Bool / FloatUnboxed / None / Never).
/// Values with no type fact floor to `DynBox` (heap-carrying) — conservative: at
/// worst a redundant drop that the runtime fast-paths.
fn carrier_is_non_heap_by_type(ty: &TirType) -> bool {
    matches!(ty, TirType::None)
        || matches!(
            Repr::default_for(ty),
            Repr::Bool | Repr::FloatUnboxed | Repr::Never
        )
}

/// The set of values whose carrier holds no refcounted heap obligation: the
/// value-range / CheckedAdd / GPU-index RawI64Safe set, plus every value whose
/// `TirType` floors to Bool / FloatUnboxed / None / Never.
fn compute_raw_scalars(func: &TirFunction) -> HashSet<ValueId> {
    let scev = crate::tir::passes::scev::compute_scev(func);
    let vr = crate::tir::passes::value_range::compute_value_range(func, &scev);
    let mut raw = raw_i64_carrier_values_for(func, &vr);

    // Add the by-type non-heap carriers (bool / float / None / never). We must visit
    // every value the function defines (block args and op results).
    let type_of = |id: ValueId| -> Option<&TirType> { func.value_types.get(&id) };
    for block in func.blocks.values() {
        for arg in &block.args {
            // Block args carry their own type on `TirValue`; the function-owned
            // `value_types` mirror may also hold it. Prefer the arg's own type.
            if carrier_is_non_heap_by_type(&arg.ty) {
                raw.insert(arg.id);
            }
        }
        for op in &block.ops {
            for &res in &op.results {
                if let Some(ty) = type_of(res)
                    && carrier_is_non_heap_by_type(ty)
                {
                    raw.insert(res);
                }
            }
        }
    }
    raw
}

/// Backward-dataflow liveness with representation filtering. See module docs.
///
/// **Alias-root canonicalization (design 20 §1.2 — `Copy`/`TypeGuard` are
/// borrowed aliases).** A transparent SSA copy (`b = Copy(a)`, including the
/// SimpleIR `copy_var` / `load_var` / `identity_alias` carriers that lower to
/// `Copy`) names the SAME heap object as its source — it carries NO new
/// reference. Liveness therefore operates in **alias-root space**: every value
/// reference (use, def/kill, edge arg, terminator use) is canonicalized to its
/// alias root before the dataflow runs. This collapses a `Copy`-chain into one
/// ownership entity so the drop pass drops the underlying object EXACTLY ONCE (at
/// the last use of any chain member), instead of once per `Copy` — the latter is
/// a refcount underflow / use-after-free (the loop-carried accumulator loads its
/// phi via `load_var`→`Copy` every iteration; dropping each copy double-frees the
/// live accumulator). Block args have no defining op, so the union-find never
/// unions them away — a loop-header phi stays its own root and is the single
/// owner of the loop-carried value.
pub fn compute_liveness(func: &TirFunction) -> TirLivenessResult {
    let raw_scalars = compute_raw_scalars(func);
    // Alias union-find: canonicalize transparent copies to their root owner.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(func);
    let canon = |v: ValueId| -> ValueId { aliases.root(v) };
    // A root is heap-carrying unless the root itself is a raw scalar. We test the
    // ROOT's repr: the carrier of the owned object is the root's carrier (a
    // `Copy` of a raw i64 is still raw; a `Copy` of a boxed value is boxed).
    let heap_carrying = |v: ValueId| -> bool { !raw_scalars.contains(&canon(v)) };
    // Interior-borrow keepalive (design 20): a use of a value produced by a
    // borrowing read (`LoadAttr`/`Index`) keeps its SOURCE object live too (the
    // result may borrow into / index the source's backing store — e.g. the
    // `Counter._handle` raw-int registry handle, whose owning wrapper's finalizer
    // destroys the registry entry). Threaded into both the per-block Use sets and
    // the edge-arg/terminator-use propagation so the source's live range covers the
    // borrow result's live range identically in forward and backward directions.
    let borrows = crate::tir::passes::alias_analysis::build_borrow_provenance(func, &aliases);
    // The heap-carrying source roots a use of `v` keeps alive (in addition to `v`'s
    // own root). Empty on the common path (no borrowing reads / non-borrow value).
    let keepalive_roots = |v: ValueId| -> Vec<ValueId> {
        if borrows.is_empty() {
            return Vec::new();
        }
        borrows
            .keepalive_roots(v, &canon)
            .into_iter()
            .filter(|&r| !raw_scalars.contains(&r))
            .collect()
    };

    // Per-block block-arg id sets (kills at block entry).
    let mut block_args: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        block_args.insert(bid, block.args.iter().map(|a| a.id).collect());
    }

    // Per-block Use / Kill restricted to heap-carrying values.
    // Use[B]  = values used by an op in B before any in-block def, plus the
    //           terminator's direct uses (cond / switch value / return values).
    // Kill[B] = op results + block args.
    let mut use_set: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut kill_set: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        let mut uses: HashSet<ValueId> = HashSet::new();
        let mut defs: HashSet<ValueId> = HashSet::new();
        // Block args are defined at entry. A block arg is its own alias root
        // (no defining op), so `canon` is identity here, but we apply it for
        // uniformity.
        for arg in &block.args {
            defs.insert(canon(arg.id));
        }
        for op in &block.ops {
            for &operand in &op.operands {
                let r = canon(operand);
                // Upward-exposed use: read before it is defined in this block.
                if !defs.contains(&r) && heap_carrying(operand) {
                    uses.insert(r);
                }
                // A use of a borrow result is also a use of its source object(s):
                // each keepalive source root is upward-exposed unless defined
                // earlier in this block (design 20 interior-borrow keepalive).
                for src_root in keepalive_roots(operand) {
                    if !defs.contains(&src_root) {
                        uses.insert(src_root);
                    }
                }
            }
            // A transparent-copy result is the SAME owned object as its root, so
            // it does NOT kill the root (it is a borrow alias). Only NON-alias
            // results define a fresh value. We canonicalize the result: if it
            // aliases an existing root, `canon(res)` is that root and inserting it
            // is a no-op for liveness (the root was already live/defined). A
            // genuine fresh result canonicalizes to itself.
            for &res in &op.results {
                defs.insert(canon(res));
            }
        }
        // Terminator direct uses are reads at the end of the block; they are
        // upward-exposed unless defined earlier in the block.
        for v in terminator_direct_uses(&block.terminator) {
            let r = canon(v);
            if !defs.contains(&r) && heap_carrying(v) {
                uses.insert(r);
            }
            // Borrow keepalive for a terminator direct use (a returned borrow
            // result keeps its source live to the return).
            for src_root in keepalive_roots(v) {
                if !defs.contains(&src_root) {
                    uses.insert(src_root);
                }
            }
        }
        use_set.insert(bid, uses);
        // Kill = all defs (op results + block args), in root space.
        kill_set.insert(bid, defs);
    }

    // Fixpoint over reverse-postorder (processing predecessors-after-successors
    // converges fastest, but any order reaches the same fixpoint).
    let order = crate::tir::dominators::reachable_blocks_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let block_ids: Vec<BlockId> = {
        let mut v: Vec<BlockId> = func.blocks.keys().copied().collect();
        v.sort_unstable_by_key(|b| b.0);
        v
    };

    let mut live_in: HashMap<BlockId, HashSet<ValueId>> =
        block_ids.iter().map(|&b| (b, HashSet::new())).collect();
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> =
        block_ids.iter().map(|&b| (b, HashSet::new())).collect();

    let mut changed = true;
    while changed {
        changed = false;
        // Iterate in descending BlockId order as a cheap reverse-ish walk.
        for &bid in block_ids.iter().rev() {
            let Some(block) = func.blocks.get(&bid) else {
                continue;
            };
            let new_out = live_out_of(
                block,
                &live_in,
                &block_args,
                &heap_carrying,
                &canon,
                &keepalive_roots,
            );
            // LiveIn = (LiveOut \ Kill) ∪ Use
            let kill = &kill_set[&bid];
            let uses = &use_set[&bid];
            let mut new_in: HashSet<ValueId> = new_out
                .iter()
                .copied()
                .filter(|v| !kill.contains(v))
                .collect();
            new_in.extend(uses.iter().copied());

            if new_out != live_out[&bid] {
                live_out.insert(bid, new_out);
                changed = true;
            }
            if new_in != live_in[&bid] {
                live_in.insert(bid, new_in);
                changed = true;
            }
        }
    }
    // Unreachable blocks (not in the reachable set) keep their empty sets — a
    // block no path executes contributes no liveness.
    let _ = &order;

    TirLivenessResult {
        live_in,
        live_out,
        raw_scalars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Straight-line: v1 = Call(); v2 = Call(v1); Return(v2). v1's last op-use is
    /// op index 1 and v1 is dead afterward (not live-out, not in Return).
    #[test]
    fn straight_line_last_use() {
        let mut func = TirFunction::new("sl".into(), vec![], TirType::DynBox);
        let v0 = func.fresh_value(); // some root str
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();
        func.value_types.insert(v0, TirType::Str);
        func.value_types.insert(v1, TirType::Str);
        func.value_types.insert(v2, TirType::Str);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(v0));
            b.ops.push(op(OpCode::Call, vec![v0], vec![v1]));
            b.ops.push(op(OpCode::Call, vec![v1], vec![v2]));
            b.terminator = Terminator::Return { values: vec![v2] };
        }
        let res = compute_liveness(&func);
        let block = &func.blocks[&entry];
        assert_eq!(res.last_use_in_block(block, v1), Some(2));
        // v1 is not live-out of entry (no successors) and dead after op 2.
        assert!(!res.is_live_out(entry, v1));
        // v2 is defined AND used (returned) within entry → not upward-exposed,
        // so it is neither live-in nor live-out: a within-block value the drop
        // pass handles purely by straight-line last-use, not via the live sets.
        assert!(!res.live_in[&entry].contains(&v2));
        assert!(!res.is_live_out(entry, v2));
    }

    /// CondBranch: value used in both arms and live-out of the cond block.
    #[test]
    fn used_in_both_branches_is_live_out() {
        let mut func = TirFunction::new("br".into(), vec![], TirType::DynBox);
        let cond = func.fresh_value();
        let x = func.fresh_value();
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();
        func.value_types.insert(cond, TirType::Bool);
        func.value_types.insert(x, TirType::Str);
        func.value_types.insert(r1, TirType::Str);
        func.value_types.insert(r2, TirType::Str);
        let entry = func.entry_block;
        let then_b = func.fresh_block();
        let else_b = func.fresh_block();
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(x));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: then_b,
                then_args: vec![],
                else_block: else_b,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            then_b,
            TirBlock {
                id: then_b,
                args: vec![],
                ops: vec![op(OpCode::Call, vec![x], vec![r1])],
                terminator: Terminator::Return { values: vec![r1] },
            },
        );
        func.blocks.insert(
            else_b,
            TirBlock {
                id: else_b,
                args: vec![],
                ops: vec![op(OpCode::Call, vec![x], vec![r2])],
                terminator: Terminator::Return { values: vec![r2] },
            },
        );
        let res = compute_liveness(&func);
        // x is used in both successors → live-out of entry.
        assert!(res.is_live_out(entry, x));
        assert!(res.is_live_in(then_b, x));
        assert!(res.is_live_in(else_b, x));
    }

    /// Loop-carried block arg: value live-in at the header, propagated via the
    /// back-edge arg.
    #[test]
    fn loop_carried_block_arg_live() {
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
                terminator: Terminator::Return {
                    values: vec![acc_phi],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        let res = compute_liveness(&func);
        // acc_phi is a block ARG of the header → defined (killed) at header
        // entry, so it is NOT live-in to its own block (standard MLIR block-arg
        // dataflow). Its liveness manifests as: it is live-out of the header
        // (used in the body successor), and it is live-in to the body.
        assert!(!res.is_live_in(header, acc_phi));
        assert!(res.is_live_out(header, acc_phi));
        assert!(res.is_live_in(body, acc_phi));
        // acc_next is passed on the back-edge → live-out of body.
        assert!(res.is_live_out(body, acc_next));
    }

    /// A raw i64 value (proven inline by value-range) is excluded from the live
    /// sets even when used.
    #[test]
    fn raw_i64_excluded_from_live_sets() {
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
        let res = compute_liveness(&func);
        // c0 / c1 are small inline ints (range-proven) → raw scalars, excluded.
        assert!(res.is_raw_scalar(c0));
        assert!(res.is_raw_scalar(c1));
        assert!(!res.live_in[&entry].contains(&c0));
        assert!(!res.live_in[&entry].contains(&c1));
    }

    /// A Bool value is filtered out of the live sets by the by-type floor.
    #[test]
    fn bool_excluded_from_live_sets() {
        let mut func = TirFunction::new("b".into(), vec![], TirType::Bool);
        let c = func.fresh_value();
        func.value_types.insert(c, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::ConstBool, vec![], vec![c]));
            b.terminator = Terminator::Return { values: vec![c] };
        }
        let res = compute_liveness(&func);
        assert!(res.is_raw_scalar(c));
        assert!(!res.live_in[&entry].contains(&c));
    }

    /// A None sentinel uses the generic i64 transport carrier but has no
    /// refcounted heap ownership obligation, so RC placement must ignore it.
    #[test]
    fn none_excluded_from_live_sets() {
        let mut func = TirFunction::new("none".into(), vec![], TirType::None);
        let n = func.fresh_value();
        func.value_types.insert(n, TirType::None);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::ConstNone, vec![], vec![n]));
            b.terminator = Terminator::Return { values: vec![n] };
        }
        let res = compute_liveness(&func);
        assert!(res.is_raw_scalar(n));
        assert!(!res.live_in[&entry].contains(&n));
    }
}
