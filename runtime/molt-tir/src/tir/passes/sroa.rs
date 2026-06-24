//! SROA — Scalar Replacement of Aggregates (Tier-0 substrate **S5, Phase 2d**).
//!
//! SROA promotes the *fields* of a proven-non-escaping object out of heap/stack
//! memory and into pure SSA register values, then deletes the allocation. It is
//! the pass that closes the `bench_struct` allocation cliff: a
//! `Point(i, i+1)` constructed in a hot loop, whose fields are only ever written
//! (and read through MemGVN-forwarded loads), should compile to register moves
//! with **no allocation, no `StoreAttr`, no `LoadAttr`** — exactly what an
//! unboxed C struct would.
//!
//! ## Where SROA sits in the arc (after MemGVN, before DCE)
//!
//! The S5 memory arc has three landed substrates this pass builds on:
//!
//! * **AliasAnalysis** (S5-1) — `region_of` assigns a non-escaping object's
//!   typed slots a per-object [`MemRegion::StackObject`]; `escape_state` proves
//!   the object does not outlive the frame; `root` canonicalizes transparent
//!   SSA-copy aliases.
//! * **MemorySSA** (S5-2a) — the region-aware reaching-def graph.
//! * **MemGVN** (S5-2b) — store-to-load forwarding + redundant-load elimination.
//!   By the time SROA runs, **every forwardable typed-slot `LoadAttr` of a
//!   non-escaping object has already been rewritten to `Copy(stored_value)`**
//!   (with the owned-result `IncRef` MemGVN reproduces). The reads are gone; what
//!   survives on a fully-promotable object is *only* its `StoreAttr` ops and the
//!   allocation itself.
//!
//! SROA's job is to remove that residue. A `store`/`store_init` is
//! side-effecting (it mutates heap memory, so DCE preserves it even when its
//! result is dead — see [`effects::opcode_is_side_effecting`]). The store keeps
//! the allocation live. SROA removes the stores, after which the
//! `ObjectNewBoundStack` is referenced by nothing and DCE — which classifies
//! `ObjectNewBoundStack` as *not* side-effecting (a stack slot has no finalizer;
//! see [`effects`]) — deletes it.
//!
//! [`effects`]: super::effects
//! [`effects::opcode_is_side_effecting`]: super::effects
//! [`MemRegion::StackObject`]: super::alias_analysis::MemRegion::StackObject
//!
//! ## Soundness model (FAIL-CLOSED)
//!
//! SROA removes a store; removing a store is only behavior-preserving when the
//! store had **no observable effect that survives the object's death**. Three
//! orthogonal obligations are discharged, every one fail-closed (a missed proof
//! refuses the SROA, never enables a miscompile):
//!
//! ### 1. The object is unobserved (escape + no surviving load).
//!
//! * `escape_state(obj) ∈ {NoEscape, ArgEscape}` — the object never leaves the
//!   frame, so no external code can read a field through a pointer we don't
//!   track. (`ArgEscape` = borrowed by an effect-free callee that provably only
//!   *reads*; but a borrow is still a *use*, so the borrowing call appears as a
//!   blocker op below and refuses the promotion. The escape gate is the floor;
//!   the per-op scan is the ceiling.)
//! * **Every** op that references `obj` (alias-root-canonicalized) is either the
//!   allocation, a removable typed-slot store into `obj`, or a transparent SSA
//!   copy of `obj`. A surviving `LoadAttr`, an escaping call, a container build,
//!   a return, a `StoreIndex`, **anything else** that names `obj` is a *blocker*
//!   that refuses the promotion. This is strictly stronger than "single reaching
//!   def per field": if the object is observed at all, SROA does not fire.
//!
//! Because the residue after MemGVN is *only stores* for a fully-promotable
//! object, this gate is exactly "MemGVN forwarded away every read and the object
//! does not escape".
//!
//! ### 2. Removing each store is refcount-neutral (the UAF/leak obligation).
//!
//! A `store`/`store_init` into a slot calls the runtime field-set helper, which
//! — for a **pointer** value — `inc_ref`s the new value (capturing a `+1` into
//! the slot) and `dec_ref`s the slot's previous occupant. A non-escaping
//! `ObjectNewBoundStack` is stamped `HEADER_FLAG_IMMORTAL`, so its slots are
//! **never** `dec_ref`'d at frame teardown. Eliminating a store that captured a
//! real `+1` would therefore drop a reference with no matching release — a
//! refcount imbalance. The fail-closed remedy is to fire ONLY when the stored
//! value is **provably a non-pointer immediate**, so the field-set helper's
//! pointer path (the only path that touches a refcount) was never taken and
//! there is nothing to balance:
//!
//! * a `ConstNone` / `ConstBool` / `ConstFloat` (a NaN-boxed immediate, or an
//!   immortal singleton whose inc/dec-ref is a no-op), or
//! * a value whose [`TirType`] is `None` / `Bool` / `F64` (same), or
//! * an integer value whose **entire** proven [`ValueRange`] fits the signed
//!   47-bit inline window ([`ValueRangeResult::fits_inline_int47`]) — so it is
//!   carried as a NaN-boxed immediate, never a heap `BigInt` pointer.
//!
//! A bare `TirType::I64` value is **not** sufficient: `int` is the
//! un-unboxable `MaybeBigInt` carrier (a value `>= 2^46` is a heap `BigInt`
//! behind `TAG_PTR`), so promoting a store of an unproven int would risk the
//! exact refcount imbalance this gate exists to prevent. Only the value-range
//! *proof* that it stays inline makes it safe. This is the same inline-int
//! window the representation lattice uses; SROA reads it through the existing
//! `ValueRange` analysis and never introduces a trusted-unbox.
//!
//! ### 3. The store is a recognized typed-slot store.
//!
//! Only `StoreAttr` ops with `_original_kind ∈ {store, store_init}` and the
//! `[obj, value]` / `value=offset` operand contract are removable. Every other
//! `StoreAttr` spelling (`guarded_field_set`, `set_attr_name`, …) and every
//! op missing the offset proof is a blocker (it falls into obligation 1's
//! "anything else" refusal).
//!
//! ## Why this is the complete promotable set (not a subset)
//!
//! After MemGVN, an object is in one of three states:
//! 1. **Fully promotable** — only stores survive, all refcount-neutral, no
//!    escape. SROA removes the stores; DCE removes the alloc. *Handled.*
//! 2. **Observed** — a surviving load/escape/blocker references `obj`. SROA
//!    refuses (obligation 1). *Correctly skipped* — the object is genuinely
//!    live.
//! 3. **Refcount-bearing** — a store writes an unproven (possibly-`BigInt` /
//!    heap) value. SROA refuses (obligation 2). *Correctly skipped* — the
//!    forwarded read (if any) already made the program correct via MemGVN's
//!    `Copy`; removing the store would unbalance the slot capture.
//!
//! State 1 is the dominant hot-loop pattern (`bench_struct`). States 2/3 are
//! fail-closed refusals: a missed promotion, never a miscompile.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::alias_analysis::{AliasAnalysis, AliasAnalysisResult};
use super::value_range::{ValueRange, ValueRangeResult};
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// `Some((obj_operand, offset))` for the narrow removable typed-slot store
/// (`store` / `store_init`) contract: operands `[obj, value]`, offset on the
/// `value` attr. This is exactly the store set [`super::memory_ssa::typed_slot_store_value`]
/// recognizes (the forwardable-source set), restricted to the two-operand,
/// integer-offset spelling. Any other `StoreAttr` spelling returns `None` and is
/// treated as an opaque blocker.
fn removable_store_obj_offset(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.opcode != OpCode::StoreAttr || op.operands.len() != 2 {
        return None;
    }
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(kind, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => Some((op.operands[0], *offset)),
        _ => None,
    }
}

/// The allocation-site result `ValueId` of an `ObjectNewBoundStack`, or `None`.
/// Escape analysis only rewrites `ObjectNewBound → ObjectNewBoundStack` when the
/// result provably does not escape, so the opcode itself is the non-escape
/// witness; the escape-state query below is a belt-and-suspenders re-check.
fn stack_alloc_result(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::ObjectNewBoundStack || op.results.len() != 1 {
        return None;
    }
    Some(op.results[0])
}

/// True when removing a `store` of `value` is provably refcount-neutral: the
/// runtime field-set helper's pointer path (the only path that touches a
/// refcount) was never taken, because `value` is a non-pointer immediate. See
/// obligation 2 in the module docs. FAIL-CLOSED: any value not *proven* immediate
/// returns `false` and refuses the promotion.
fn store_value_is_refcount_neutral(
    value: ValueId,
    func: &TirFunction,
    const_immediates: &HashSet<ValueId>,
    ranges: &ValueRangeResult,
) -> bool {
    // (a) A constant immediate produced in this function (ConstNone / ConstBool /
    //     ConstFloat, or a ConstInt proven to fit the inline window below).
    if const_immediates.contains(&value) {
        return true;
    }
    // (b) A value whose static type is an always-immediate scalar. None / Bool /
    //     F64 are NaN-boxed immediates or immortal singletons (inc/dec-ref no-op).
    //     `I64` is intentionally excluded — it is the MaybeBigInt carrier; only
    //     the value-range proof in (c) makes an int safe.
    if let Some(TirType::None | TirType::Bool | TirType::F64) = func.value_types.get(&value) {
        return true;
    }
    // (c) An integer whose entire proven range fits the signed 47-bit inline
    //     window ⇒ carried as a NaN-boxed immediate, never a heap BigInt pointer.
    ranges.fits_inline_int47(value)
}

/// Collect the set of `ValueId`s defined by a constant-immediate op whose value
/// is provably a non-pointer immediate (`ConstNone` / `ConstBool` / `ConstFloat`
/// unconditionally; `ConstInt` only when its literal fits the inline window).
/// `ConstStr` / `ConstBytes` are heap pointers and are intentionally excluded.
fn collect_const_immediates(func: &TirFunction, ranges: &ValueRangeResult) -> HashSet<ValueId> {
    let mut set = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            let Some(&result) = op.results.first() else {
                continue;
            };
            match op.opcode {
                OpCode::ConstNone | OpCode::ConstBool | OpCode::ConstFloat => {
                    set.insert(result);
                }
                // A literal int is an inline immediate only when it fits the
                // window; a `ConstInt` of `1 << 60` lowers to a heap BigInt.
                OpCode::ConstInt if ranges.fits_inline_int47(result) => {
                    set.insert(result);
                }
                _ => {}
            }
        }
    }
    set
}

/// True when `term` references `value` (alias-canonicalized) — a return or
/// branch-arg use of the object is an escape/observation that blocks SROA.
fn terminator_references(term: &Terminator, root: ValueId, alias: &AliasAnalysisResult) -> bool {
    let hits = |v: &ValueId| alias.root(*v) == root;
    match term {
        Terminator::Branch { args, .. } => args.iter().any(hits),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => hits(cond) || then_args.iter().any(hits) || else_args.iter().any(hits),
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            hits(value)
                || cases.iter().any(|(_, _, args)| args.iter().any(hits))
                || default_args.iter().any(hits)
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            cases.iter().any(|(_, _, args)| args.iter().any(hits)) || default_args.iter().any(hits)
        }
        Terminator::Return { values } => values.iter().any(hits),
        Terminator::Unreachable => false,
    }
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run_with(func, am)
}

fn run_with(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "sroa",
        ..Default::default()
    };
    // Trivial functions hold no aggregates to scalarize.
    if func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    // Analyses (cloned — `am.get` borrows are released before we mutate `func`,
    // mirroring mem_gvn). ValueRange depends on SCEV (computed inside its
    // `Analysis::compute`); both are dropped by `invalidate_ops` after this
    // OpsOnly pass.
    let alias: AliasAnalysisResult = am.get::<AliasAnalysis>(func).clone();
    let ranges: ValueRangeResult = am.get::<ValueRange>(func).clone();
    let const_immediates = collect_const_immediates(func, &ranges);
    // Recon / fire-evidence instrumentation (`MOLT_SROA_REPORT=1`): per-function
    // diagnostics written to stderr + the debug-artifact channel (the daemon's
    // stderr does not surface through the CLI on a successful build; the artifact
    // does). Reports the promotion count and, per refused root, the blocking op —
    // the production instrument used to verify SROA fires on real code.
    let report = std::env::var("MOLT_SROA_REPORT").as_deref() == Ok("1");
    let mut diag: Vec<String> = Vec::new();

    // Candidate roots: every non-escaping `ObjectNewBoundStack` alloc, keyed by
    // its alias root (transparent copies share one promotion decision).
    let mut candidate_roots: HashSet<ValueId> = HashSet::new();
    let mut raw_stack_allocs = 0usize;
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(result) = stack_alloc_result(op) {
                raw_stack_allocs += 1;
                let root = alias.root(result);
                // Belt-and-suspenders escape re-check (the opcode already proves
                // non-escape; a GlobalEscape here would be a frontend/escape-pass
                // contradiction — refuse rather than trust).
                if matches!(
                    alias.escape_state(root),
                    crate::tir::passes::escape_analysis::EscapeState::NoEscape
                        | crate::tir::passes::escape_analysis::EscapeState::ArgEscape
                ) {
                    candidate_roots.insert(root);
                } else if report {
                    diag.push(format!(
                        "  root v{} REJECTED: escape_state={:?}",
                        root.0,
                        alias.escape_state(root)
                    ));
                }
            }
        }
    }
    if candidate_roots.is_empty() {
        emit_report(report, func, raw_stack_allocs, 0, 0, 0, &diag);
        return stats;
    }

    // ── Single forward scan: classify every op's relationship to each candidate
    // root. A root is promotable iff EVERY op that references it (operands or
    // results, alias-canonicalized) is one of:
    //   * the allocation site itself,
    //   * a transparent SSA copy of the root,
    //   * a removable, refcount-neutral typed-slot store into the root.
    // Any other reference (a surviving load, an escaping call, a container build,
    // a StoreIndex, an unproven-value store, …) is a BLOCKER that disqualifies
    // the root. We also record, per promotable root, the (block, op_idx) of each
    // store to remove.
    let mut blocked: HashSet<ValueId> = HashSet::new();
    // root → list of (block, op_idx) stores to remove.
    let mut stores_to_remove: HashMap<ValueId, Vec<(BlockId, usize)>> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            // Which candidate roots does this op touch (via any operand or result)?
            let mut touched: HashSet<ValueId> = HashSet::new();
            for &v in op.operands.iter().chain(op.results.iter()) {
                let r = alias.root(v);
                if candidate_roots.contains(&r) {
                    touched.insert(r);
                }
            }
            if touched.is_empty() {
                continue;
            }

            // The allocation site: its result IS the root, and its only operand
            // (the class ref) is never a candidate root. Touches exactly this one
            // root. Always allowed.
            if let Some(alloc_result) = stack_alloc_result(op) {
                let alloc_root = alias.root(alloc_result);
                if touched.len() == 1
                    && touched.contains(&alloc_root)
                    && op
                        .operands
                        .iter()
                        .all(|&v| !candidate_roots.contains(&alias.root(v)))
                {
                    continue;
                }
            }

            // A transparent-alias op (`Copy` / no-op `TypeGuard`) threads the
            // object identity to a new SSA value without observing or escaping it;
            // it is pure plumbing. We consult the alias oracle's OWN predicate
            // (`is_transparent_alias_op`) — the single source of truth for "this
            // is an identity move" — rather than re-deriving it: a bare
            // `is_plain_value_copy` check would wrongly reject the frontend's
            // source-location-attributed (`_col_offset`) and `_simple_out`
            // threading copies, which carry non-semantic attrs but are exactly the
            // identity moves the oracle already unified into this root. The
            // `touched.len() == 1` guard ensures the op only plumbs THIS root.
            if alias.is_transparent_alias_op(op) && touched.len() == 1 {
                continue;
            }

            // A removable, refcount-neutral typed-slot store INTO exactly one
            // root, whose value is not itself a candidate root (storing one
            // promotable object into another's slot would be an escape — caught
            // because the value's root would then be in `touched` too).
            if let Some((store_obj, _offset)) = removable_store_obj_offset(op) {
                let store_root = alias.root(store_obj);
                let value = op.operands[1];
                let value_root = alias.root(value);
                let value_is_neutral =
                    store_value_is_refcount_neutral(value, func, &const_immediates, &ranges);
                // The op must touch ONLY this store's target root (the value must
                // not be a candidate root — that would mean the object captures
                // another promotable object, an escape), and the value must be
                // refcount-neutral.
                if touched.len() == 1
                    && touched.contains(&store_root)
                    && !candidate_roots.contains(&value_root)
                    && value_is_neutral
                {
                    stores_to_remove
                        .entry(store_root)
                        .or_default()
                        .push((bid, op_idx));
                    continue;
                }
                if report {
                    diag.push(format!(
                        "  root v{} STORE not-removable: touched={} value=v{} \
                         value_is_candidate={} value_neutral={} fits47={}",
                        store_root.0,
                        touched.len(),
                        value.0,
                        candidate_roots.contains(&value_root),
                        value_is_neutral,
                        ranges.fits_inline_int47(value),
                    ));
                }
            }

            // Anything else that references a candidate root is a blocker:
            // disqualify every root this op touches.
            if report {
                let mut roots: Vec<u32> = touched.iter().map(|r| r.0).collect();
                roots.sort_unstable();
                diag.push(format!(
                    "  roots {:?} BLOCKED by {:?} (kind={:?})",
                    roots,
                    op.opcode,
                    op.attrs.get("_original_kind"),
                ));
            }
            for r in touched {
                blocked.insert(r);
            }
        }

        // A terminator that names a root (return / branch arg) escapes it.
        for &root in &candidate_roots {
            if terminator_references(&block.terminator, root, &alias) {
                if report {
                    diag.push(format!("  root v{} BLOCKED by terminator (escape)", root.0));
                }
                blocked.insert(root);
            }
        }
    }

    // The promotable roots are the candidates with at least one removable store
    // and no blocker. (A root with zero stores is a no-op for SROA — DCE already
    // removes a never-stored alloc — so we only act on roots with stores.)
    let promotable: Vec<ValueId> = stores_to_remove
        .keys()
        .copied()
        .filter(|r| !blocked.contains(r))
        .collect();
    if promotable.is_empty() {
        emit_report(
            report,
            func,
            raw_stack_allocs,
            candidate_roots.len(),
            0,
            0,
            &diag,
        );
        return stats;
    }

    // ── Apply: remove every store to a promotable root. Group by block and
    // remove in descending op-index order so earlier removals never shift the
    // index of a still-pending one. The now-unreferenced `ObjectNewBoundStack`
    // (not side-effecting) is deleted by the DCE pass that follows.
    let mut removals_by_block: HashMap<BlockId, Vec<usize>> = HashMap::new();
    for root in &promotable {
        for &(bid, op_idx) in &stores_to_remove[root] {
            removals_by_block.entry(bid).or_default().push(op_idx);
        }
    }
    for (bid, mut indices) in removals_by_block {
        indices.sort_unstable_by(|a, b| b.cmp(a));
        indices.dedup();
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        for op_idx in indices {
            if op_idx >= block.ops.len() {
                continue;
            }
            // Defensive: only remove if it is still the store we planned for.
            if removable_store_obj_offset(&block.ops[op_idx]).is_none() {
                continue;
            }
            block.ops.remove(op_idx);
            stats.ops_removed += 1;
        }
    }

    emit_report(
        report,
        func,
        raw_stack_allocs,
        candidate_roots.len(),
        promotable.len(),
        stats.ops_removed,
        &diag,
    );

    stats
}

/// Emit the `MOLT_SROA_REPORT` diagnostics for one function: a summary line plus
/// any per-root blocker reasons, to stderr AND the debug-artifact channel (the
/// backend daemon's stderr does not surface through the CLI on a successful
/// build). A no-op when the report flag is off.
#[allow(clippy::too_many_arguments)]
fn emit_report(
    report: bool,
    func: &TirFunction,
    raw_stack_allocs: usize,
    candidates: usize,
    promoted: usize,
    stores_removed: usize,
    diag: &[String],
) {
    if !report || raw_stack_allocs == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(diag.len() + 1);
    lines.push(format!(
        "[SROA] fn={} stack_allocs={raw_stack_allocs} candidates={candidates} \
         promoted={promoted} stores_removed={stores_removed}",
        func.name
    ));
    lines.extend(diag.iter().cloned());
    for line in &lines {
        eprintln!("{line}");
    }
    let sanitized: String = func
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("sroa_report/{sanitized}.txt"),
        lines.join("\n") + "\n",
    );
}

#[cfg(test)]
mod tests;
