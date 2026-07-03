//! Generator frame-elision fusion — Tier-B (doc 26 Phase 1, the D1 blueprint
//! `07_D1-coroelide.md`).
//!
//! This is a **module** transform (it needs the consumer caller AND the
//! generator `_poll` body simultaneously), run from
//! [`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline) AFTER
//! the E1 inliner. It recognizes the shape
//!
//! ```text
//!   g = AllocTask(task_kind="generator", poll=P, closure_size=N)   // in caller
//!   it = GetIter(g)                                                // single use
//!   loop { pair = IterNext(it); done = pair[1]; if done break;     // single use
//!          elem = pair[0]; <consumer body using elem> }
//! ```
//!
//! and **splices** `P`'s body into the caller, eliminating the heap frame
//! (`AllocTask` → `molt_task_new`), the per-yield `(value, done)` pair tuple,
//! the indirect `_poll` call, and the `STATE_SWITCH` dispatch. The generator's
//! own control flow becomes the fused loop; each `STATE_YIELD(pair)` binds the
//! element directly to the consumer's for-target and runs the consumer body
//! inline.
//!
//! ## What the splice actually rebuilds
//!
//! A generator `_poll` lowers to a **linear / structured** TIR body: a
//! `state_switch` marker op, then code interleaved with `state_yield(pair,
//! next_state)` ops, with the resume-after-yield being the *fall-through* (the
//! state dispatch CFG that the native/LLVM backends reconstruct from the
//! `next_state` ids does NOT exist as TIR edges). The frame slots are MEMORY:
//! `closure_load(self, offset)` / `closure_store(self, offset, v)` where
//! `offset < GEN_CONTROL_BYTES` (48) are the control slots (send=0, throw=8,
//! closed=16) and `offset >= 48` are the generator's captured params + spilled
//! locals.
//!
//! The fused form is the explicit state machine the backend would have built,
//! but with the consumer body interleaved and the frame promoted to SSA:
//!
//! ```text
//!   preheader: br dispatch(slot_inits..., state=ENTRY)
//!   dispatch(slot_phis..., state_phi):
//!       switch state_phi -> [seg_0, resume_1, ..., resume_{n-1}, exhausted]
//!   seg_K (the code from after yield K-1 through yield K):
//!       ... cloned P ops (closure_load(slot)->phi, closure_store(slot,v)->thread) ...
//!       elem = pair[0]; IncRef(elem)
//!       br consumer(elem, updated_slots..., next_state_K)
//!   consumer(elem, slot_phis..., ret_state):
//!       <original consumer body using elem>
//!       br dispatch(slot_phis..., ret_state)     // continue
//!       (or br loop_exit on break)
//!   exhausted: br loop_exit
//! ```
//!
//! The control slots (send/throw/closed) are eliminated: the recognition
//! predicate proves no `.send()`/`.throw()`/`.close()` can reach this generator
//! (the object never escapes the single `GetIter` use), so every send-slot read
//! is dead and every throw-slot read is `None`; the throw-injection `raise`
//! folds away under the re-run `run_pipeline` (SCCP proves `None is not None`).
//!
//! ## Soundness
//!
//! Conservative-correct by construction: every recognition gate that is not met
//! leaves the IR byte-identical (the generator stays Tier D — heap frame +
//! runtime `molt_generator_send`, which is correct and preserved). The splice is
//! followed by `verify_function` and a `run_pipeline` re-run (which itself
//! verifies). One explicit `IncRef(elem)` per yield site replicates the `+1`
//! ownership the eliminated `IterNext` calling convention delivered. No other RC
//! op is added or removed.
//!
//! Phase 1 scope (doc 26): single- and multi-yield generators with no
//! `YieldFrom`, no real exception HANDLER region (`has_exception_handlers()`),
//! no `.send`/`.throw`/`.close`, single non-escaping `AllocTask` instance. See
//! the bail table in [`collect_fusion_candidates`] / [`is_poll_fusable`].

mod clone;
mod wire;

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};

use super::super::blocks::{BlockId, Terminator};
use super::super::call_graph::CallGraph;
use super::super::function::{TirFunction, TirModule};
use super::super::op_kinds_generated::{
    GeneratorFusionIterUseRole, opcode_generator_fusion_iter_use_role_table,
    opcode_generator_fusion_poll_role_table,
};
use super::super::ops::{AttrValue, OpCode, TirOp};
use super::super::target_info::TargetInfo;
use super::super::types::TirType;
use super::super::values::ValueId;

use clone::{
    LocalInit, clone_and_rewrite_poll, const_int_op, const_none_op, local_slot_init_const,
};
use wire::wire_fused_loop;

/// Byte size of the generator control header. Frame offsets `< GEN_CONTROL_BYTES`
/// are the control slots — `GEN_SEND_OFFSET=0` (the `.send()` value),
/// `GEN_THROW_OFFSET=8` (the pending `.throw()` exception), `GEN_CLOSED_OFFSET=16`
/// (the exhausted flag), `GEN_YIELD_FROM_OFFSET=32` (the delegation target);
/// offsets `>= GEN_CONTROL_BYTES` are the generator's captured params + spilled
/// locals. Mirrors `GEN_CONTROL_SIZE` in `src/molt/frontend/_types.py` and
/// `crate::GENERATOR_CONTROL_BYTES`.
pub(super) const GEN_CONTROL_BYTES: i64 = 48;

/// Collect the set of USER frame-slot offsets (`>= GEN_CONTROL_BYTES`) the poll
/// body accesses via `ClosureLoad`/`ClosureStore`, in ascending order.
fn collect_user_frame_slots(poll: &TirFunction) -> Vec<i64> {
    let mut slots = BTreeSet::new();
    for block in poll.blocks.values() {
        for op in &block.ops {
            if matches!(op.opcode, OpCode::ClosureLoad | OpCode::ClosureStore)
                && let Some(off) = attr_value_int(op)
                && off >= GEN_CONTROL_BYTES
            {
                slots.insert(off);
            }
        }
    }
    slots.into_iter().collect()
}

/// Statistics from one [`run_generator_fusion`] invocation over a module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionStats {
    /// Number of generator frames elided (one per successful splice).
    pub frames_elided: usize,
    /// Number of yield sites spliced into consumer bodies.
    pub yield_sites_spliced: usize,
    /// Names of the consumer functions whose body was changed by fusion (a
    /// generator was spliced in). Production codegen must back-convert /
    /// re-lower ONLY these functions' (post-fusion) TIR — the module phase folds
    /// this into its `changed_functions` set exactly as it does the inliner's.
    pub changed_functions: Vec<String>,
}

/// Read an op's integer `value` attr (slot offset / next-state id).
pub(super) fn attr_value_int(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Read an op's `s_value` string attr (poll function name).
fn attr_s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read an op's `task_kind` string attr.
fn attr_task_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("task_kind") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read an op's `_original_kind` string attr (the SimpleIR op-name annotation
/// preserved on `Copy`-lowered ops such as `iter`).
pub(super) fn attr_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// True if `op` is the consumer's `GetIter` over a value — either the
/// first-class [`OpCode::GetIter`] or the runtime `iter` op (lowered as a
/// `Copy` carrying `_original_kind == "iter"`, the form the frontend emits for
/// `for x in <expr>`).
pub(super) fn is_get_iter_op(op: &TirOp) -> bool {
    op.opcode == OpCode::GetIter
        || (op.opcode == OpCode::Copy && attr_original_kind(op) == Some("iter"))
}

/// A recognized fusion candidate: an `AllocTask(generator)` consumed by a single
/// `GetIter` → single `IterNext`-loop in `caller`.
pub(super) struct FusionCandidate {
    /// Block + op index of the `AllocTask` in the caller.
    pub(super) alloc_block: BlockId,
    pub(super) alloc_idx: usize,
    /// The generator frame value produced by `AllocTask`.
    pub(super) alloc_val: ValueId,
    /// The `_poll` function name (a module-defined function).
    pub(super) poll_name: String,
    /// Block holding the `GetIter` (or `iter` Copy) in the caller.
    pub(super) get_iter_block: BlockId,
    /// The iterator value produced by `GetIter`.
    pub(super) iter_val: ValueId,
    /// The loop-condition block holding the `IterNext` + done-check.
    pub(super) cond_block: BlockId,
    /// The `(value, done)` pair value produced by `IterNext`.
    pub(super) pair_val: ValueId,
    /// The block holding the `Index(pair, 0)` element-extraction (the body block,
    /// or the cond block if the element is extracted before the branch).
    pub(super) elem_block: BlockId,
    /// The element value (`pair[0]`).
    pub(super) elem_val: ValueId,
    /// The block control branches to on `done == true` (loop exit) and
    /// `done == false` (loop body).
    pub(super) exit_block: BlockId,
    pub(super) body_block: BlockId,
    /// The loop header (the `LoopHeader`-role block that targets `cond_block`).
    /// Present iff the consumer carries structured loop metadata.
    pub(super) loop_header: Option<BlockId>,
}

/// Run generator fusion over `module`. Returns the elided-frame statistics.
///
pub fn run_generator_fusion(
    module: &mut TirModule,
    call_graph: &CallGraph,
    tti: &TargetInfo,
) -> FusionStats {
    let mut stats = FusionStats::default();

    // Snapshot every fusable poll body up front (owned clones), keyed by name —
    // the splice reads the poll body while holding `&mut` on the caller, and the
    // borrow checker cannot prove disjointness through the module vector.
    let poll_bodies: HashMap<String, TirFunction> = module
        .functions
        .iter()
        .filter(|f| is_poll_fusable(f, call_graph))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if poll_bodies.is_empty() {
        return stats;
    }

    // Map function name -> index for O(1) caller lookup, owned (drops the borrow
    // on `module.functions` before mutation).
    let index_of: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Caller names processed in deterministic order.
    let mut caller_names: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();
    caller_names.sort();

    for caller_name in caller_names {
        let Some(&caller_idx) = index_of.get(&caller_name) else {
            continue;
        };
        // Collect candidates over the current caller body, then splice them one
        // at a time (re-collecting after each splice — a successful splice
        // rewrites the caller's blocks, invalidating prior coordinates).
        loop {
            let candidate = {
                let caller = &module.functions[caller_idx];
                collect_fusion_candidates(caller, &poll_bodies, call_graph)
                    .into_iter()
                    .next()
            };
            let Some(candidate) = candidate else { break };
            let Some(poll) = poll_bodies.get(&candidate.poll_name) else {
                break;
            };
            let poll_owned = poll.clone();
            let caller = &mut module.functions[caller_idx];
            let spliced = apply_fusion(caller, &poll_owned, &candidate, &mut stats);
            if spliced {
                if !stats.changed_functions.contains(&caller_name) {
                    stats.changed_functions.push(caller_name.clone());
                }
                // Re-optimize the merged caller jointly (SCCP folds the dead
                // throw-check, LICM/escape/BCE clean up the fused loop). Bracket
                // with type refinement on both sides, matching the inliner's
                // refine→pipeline→refine contract so the backends receive a
                // fully-refined body.
                super::super::type_refine::refine_types(caller);
                let _ = super::run_pipeline(caller, tti);
                super::super::type_refine::refine_types(caller);
            } else {
                // The candidate could not be spliced (a conservative
                // mid-analysis bail). Stop processing this caller to avoid an
                // infinite re-collect loop on the same un-spliceable site.
                break;
            }
        }
    }

    stats
}

/// Whether `poll` is a generator `_poll` body that may be fused (Phase 1).
///
/// Conservative-correct exclusions — any one keeps the generator at Tier D:
/// * not a generator at all (no `StateYield`).
/// * `YieldFrom` (delegation; cannot be linearized in Phase 1).
/// * `StateBlockStart`/`StateBlockEnd` (async generator state region) or a real
///   `try`/`except` HANDLER ([`has_exception_handlers`](TirFunction::has_exception_handlers)).
/// * recursive (a self-edge / cycle in the call graph) — unbounded splice.
/// * the entry block has a predecessor — the splice assumes the entry is the
///   single linear start (no branch targets it).
fn is_poll_fusable(poll: &TirFunction, call_graph: &CallGraph) -> bool {
    let mut has_yield = false;
    for block in poll.blocks.values() {
        for op in &block.ops {
            let role = opcode_generator_fusion_poll_role_table(op.opcode);
            if role.rejects_fusion() {
                return false;
            }
            if role.is_required_yield() {
                has_yield = true;
            }
        }
    }
    if !has_yield {
        return false;
    }
    if poll.has_exception_handlers() {
        return false;
    }
    if call_graph.recursive_set().contains(&poll.name) {
        return false;
    }
    if entry_has_predecessor(poll) {
        return false;
    }
    true
}

/// True if any terminator targets `func`'s entry block.
fn entry_has_predecessor(func: &TirFunction) -> bool {
    let entry = func.entry_block;
    func.blocks.values().any(|b| match &b.terminator {
        Terminator::Branch { target, .. } => *target == entry,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == entry || *else_block == entry,
        Terminator::Switch { cases, default, .. } => {
            *default == entry || cases.iter().any(|(_, t, _)| *t == entry)
        }
        Terminator::StateDispatch { cases, default, .. } => {
            *default == entry || cases.iter().any(|(_, t, _)| *t == entry)
        }
        Terminator::Return { .. } | Terminator::Unreachable => false,
    })
}

/// Build a use-count map over the whole function (ops + terminators), so a
/// "single use" recognition test is exact.
fn build_use_counts(func: &TirFunction) -> HashMap<ValueId, usize> {
    let mut counts: HashMap<ValueId, usize> = HashMap::new();
    let bump = |v: ValueId, c: &mut HashMap<ValueId, usize>| {
        *c.entry(v).or_insert(0) += 1;
    };
    for block in func.blocks.values() {
        for op in &block.ops {
            for &v in &op.operands {
                bump(v, &mut counts);
            }
        }
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for &v in args {
                    bump(v, &mut counts);
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                bump(*cond, &mut counts);
                for &v in then_args {
                    bump(v, &mut counts);
                }
                for &v in else_args {
                    bump(v, &mut counts);
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                bump(*value, &mut counts);
                for (_, _, args) in cases {
                    for &v in args {
                        bump(v, &mut counts);
                    }
                }
                for &v in default_args {
                    bump(v, &mut counts);
                }
            }
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for &v in args {
                        bump(v, &mut counts);
                    }
                }
                for &v in default_args {
                    bump(v, &mut counts);
                }
            }
            Terminator::Return { values } => {
                for &v in values {
                    bump(v, &mut counts);
                }
            }
            Terminator::Unreachable => {}
        }
    }
    counts
}

/// Collect fusion candidates in `caller`. Phase 1 recognizes at most one
/// candidate per call (re-collected after each splice). Deterministic order:
/// blocks sorted by id, ops in index order.
///
/// Bail table (each leaves the IR unchanged):
/// * `AllocTask` is not a `generator` (future/coroutine) — out of scope.
/// * the poll body is not in `poll_bodies` (not fusable, or external).
/// * `> 1` `AllocTask` with the same poll name in the caller — multi-instance,
///   Phase 1 handles single-instance only.
/// * the frame value has any use other than the single `GetIter` (a `.send`/
///   `.throw`/`.close` method call, an escape into a container, a store).
/// * the `GetIter` result has any use other than the single `IterNext`.
/// * the `IterNext` result is not destructured by exactly `Index(pair,1)` (done)
///   + `Index(pair,0)` (elem) feeding a `CondBranch`/loop break.
fn collect_fusion_candidates(
    caller: &TirFunction,
    poll_bodies: &HashMap<String, TirFunction>,
    _call_graph: &CallGraph,
) -> Vec<FusionCandidate> {
    let use_counts = build_use_counts(caller);

    // Count AllocTask instances per poll name (multi-instance → bail).
    let mut alloc_count: HashMap<&str, usize> = HashMap::new();
    for block in caller.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::AllocTask
                && attr_task_kind(op) == Some("generator")
                && let Some(name) = attr_s_value(op)
            {
                *alloc_count.entry(name).or_insert(0) += 1;
            }
        }
    }

    // Definition map: value -> (block, op_idx) for single-result ops.
    let mut def_of: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for &bid in &block_ids {
        for (i, op) in caller.blocks[&bid].ops.iter().enumerate() {
            if let Some(&r) = op.results.first() {
                def_of.insert(r, (bid, i));
            }
        }
    }

    let mut candidates = Vec::new();

    for &alloc_block in &block_ids {
        let block = &caller.blocks[&alloc_block];
        for (alloc_idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::AllocTask || attr_task_kind(op) != Some("generator") {
                continue;
            }
            let Some(poll_name) = attr_s_value(op).map(str::to_string) else {
                continue;
            };
            if !poll_bodies.contains_key(&poll_name) {
                continue;
            }
            if alloc_count.get(poll_name.as_str()).copied().unwrap_or(0) != 1 {
                continue; // multi-instance — Phase 1 bail.
            }
            let Some(&alloc_val) = op.results.first() else {
                continue;
            };

            // The frame value must have exactly one use: the GetIter.
            if use_counts.get(&alloc_val).copied().unwrap_or(0) != 1 {
                continue;
            }
            let Some(get_iter) = find_single_get_iter_use(caller, alloc_val) else {
                continue;
            };
            let (get_iter_block, iter_val) = get_iter;

            // The iterator value's uses must be exactly: the `IterNext`, plus
            // (optionally) the consumer's `is(iter, None)` not-iterable guard
            // (the frontend emits `if iter is None: raise TypeError` around
            // `for x in <expr>`; that `Is` use is benign — fusion replaces the
            // iterator with a non-None sentinel so the guard folds False).
            if !iter_uses_are_next_and_optional_none_guard(caller, iter_val) {
                continue;
            }
            let Some((cond_block, pair_val)) = find_single_iter_next_use(caller, iter_val) else {
                continue;
            };

            // Destructure: the pair must feed exactly Index(pair,1)=done and
            // Index(pair,0)=elem, with done driving the cond_block's CondBranch.
            let Some(destructure) =
                recognize_pair_destructure(caller, cond_block, pair_val, &def_of)
            else {
                continue;
            };

            let loop_header = caller.loop_pairs.keys().find_map(|h| {
                // The header whose cond block is `cond_block` (the loop's
                // condition test).
                if caller.loop_cond_blocks.get(h) == Some(&cond_block) {
                    Some(*h)
                } else {
                    None
                }
            });

            candidates.push(FusionCandidate {
                alloc_block,
                alloc_idx,
                alloc_val,
                poll_name,
                get_iter_block,
                iter_val,
                cond_block,
                pair_val,
                elem_block: destructure.elem_block,
                elem_val: destructure.elem_val,
                exit_block: destructure.exit_block,
                body_block: destructure.body_block,
                loop_header,
            });
            // Phase 1: one candidate per pass invocation (re-collected after the
            // splice mutates the caller).
            return candidates;
        }
    }

    candidates
}

/// Find the single `GetIter`/`iter` use of `frame_val`, returning
/// `(block, op_idx, iter_val)`.
fn find_single_get_iter_use(
    caller: &TirFunction,
    frame_val: ValueId,
) -> Option<(BlockId, ValueId)> {
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for bid in block_ids {
        for op in caller.blocks[&bid].ops.iter() {
            if op.operands.first() == Some(&frame_val) && is_get_iter_op(op) {
                let iter_val = *op.results.first()?;
                return Some((bid, iter_val));
            }
        }
    }
    None
}

/// True if every use of `iter_val` (in ops + terminators) is either the
/// `IterNext`, an `Is(iter, None)` not-iterable guard, or a `GetIter`/`iter`
/// op that produced it. Any other use (an escape, a `.send`/`.throw`/`.close`
/// method dispatch, a store) disqualifies fusion.
fn iter_uses_are_next_and_optional_none_guard(caller: &TirFunction, iter_val: ValueId) -> bool {
    let mut saw_next = false;
    for block in caller.blocks.values() {
        for op in &block.ops {
            let uses_it = op.operands.contains(&iter_val);
            // The defining GetIter/iter op has iter_val in `results`, not a use.
            if op.results.contains(&iter_val) {
                continue;
            }
            if !uses_it {
                continue;
            }
            match opcode_generator_fusion_iter_use_role_table(op.opcode) {
                GeneratorFusionIterUseRole::NextUse if op.operands.first() == Some(&iter_val) => {
                    saw_next = true
                }
                GeneratorFusionIterUseRole::NoneGuard => {
                    /* the `is(iter, None)` not-iterable guard — benign */
                }
                _ => return false,
            }
        }
        // No terminator should consume the raw iterator value.
        if terminator_uses(&block.terminator, iter_val) {
            return false;
        }
    }
    saw_next
}

/// True if a terminator references `v` in any of its value slots.
fn terminator_uses(term: &Terminator, v: ValueId) -> bool {
    match term {
        Terminator::Branch { args, .. } => args.contains(&v),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => *cond == v || then_args.contains(&v) || else_args.contains(&v),
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            *value == v || default_args.contains(&v) || cases.iter().any(|(_, _, a)| a.contains(&v))
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => default_args.contains(&v) || cases.iter().any(|(_, _, a)| a.contains(&v)),
        Terminator::Return { values } => values.contains(&v),
        Terminator::Unreachable => false,
    }
}

/// Find the single `IterNext` use of `iter_val`, returning `(block, pair_val)`.
fn find_single_iter_next_use(
    caller: &TirFunction,
    iter_val: ValueId,
) -> Option<(BlockId, ValueId)> {
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for bid in block_ids {
        for op in caller.blocks[&bid].ops.iter() {
            if op.opcode == OpCode::IterNext && op.operands.first() == Some(&iter_val) {
                let pair_val = *op.results.first()?;
                return Some((bid, pair_val));
            }
        }
    }
    None
}

/// The result of recognizing the `(value, done)` pair destructure in the loop.
struct PairDestructure {
    /// The block holding `elem = Index(pair, 0)`.
    elem_block: BlockId,
    /// The element value (`pair[0]`).
    elem_val: ValueId,
    /// The `done == true` (loop exit) and `done == false` (loop body) targets.
    exit_block: BlockId,
    body_block: BlockId,
}

/// Recognize the pair destructure: `done = Index(pair, 1)` in `cond_block`
/// driving its `CondBranch`, and `elem = Index(pair, 0)` (in the body block, or
/// in `cond_block` before the branch).
fn recognize_pair_destructure(
    caller: &TirFunction,
    cond_block: BlockId,
    pair_val: ValueId,
    _def_of: &HashMap<ValueId, (BlockId, usize)>,
) -> Option<PairDestructure> {
    let block = caller.blocks.get(&cond_block)?;

    // The done flag: Index(pair, idx) where the index const == 1.
    let mut done: Option<ValueId> = None;
    let mut elem_in_cond: Option<ValueId> = None;
    for op in block.ops.iter() {
        if op.opcode != OpCode::Index || op.operands.first() != Some(&pair_val) {
            continue;
        }
        let Some(&idx_val) = op.operands.get(1) else {
            continue;
        };
        let Some(k) = const_int_of(caller, idx_val) else {
            continue;
        };
        let Some(&res) = op.results.first() else {
            continue;
        };
        if k == 1 {
            done = Some(res);
        } else if k == 0 {
            elem_in_cond = Some(res);
        }
    }
    let done_val = done?;

    // The cond_block terminator must be a CondBranch on done_val: TRUE → exit,
    // FALSE → body (the IterNext loop's break-if-done polarity).
    let (exit_block, body_block) = match &block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            else_block,
            ..
        } if *cond == done_val => (*then_block, *else_block),
        _ => return None,
    };

    // The element: Index(pair, 0). Usually the first op of the body block; may
    // also already live in the cond block (before the branch).
    if let Some(elem_val) = elem_in_cond {
        return Some(PairDestructure {
            elem_block: cond_block,
            elem_val,
            exit_block,
            body_block,
        });
    }
    let body = caller.blocks.get(&body_block)?;
    for op in body.ops.iter() {
        if op.opcode == OpCode::Index && op.operands.first() == Some(&pair_val) {
            let Some(&idx_val) = op.operands.get(1) else {
                continue;
            };
            if const_int_of(caller, idx_val) == Some(0) {
                let elem_val = *op.results.first()?;
                return Some(PairDestructure {
                    elem_block: body_block,
                    elem_val,
                    exit_block,
                    body_block,
                });
            }
        }
    }
    None
}

/// Resolve the integer constant a value holds, if it is a `ConstInt`.
fn const_int_of(caller: &TirFunction, v: ValueId) -> Option<i64> {
    for block in caller.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt && op.results.first() == Some(&v) {
                return attr_value_int(op);
            }
        }
    }
    None
}

// ===========================================================================
// The splice (single-yield-site — the Tier-B keystone)
// ===========================================================================
//
// Phase 1 splices the structurally-cleanest class that covers the perf keystone
// (`bench_generator_iter`) and the os.walk inner loop: **single-yield-site
// generators** — exactly one `StateYield` in the poll body. This is the
// `while <cond>: yield <expr>; <step>` shape (a yield inside the generator's own
// loop) and the bare `def g(): ...; yield <expr>` shape. The generator's own
// control flow becomes the fused loop; the single yield binds the element to the
// consumer's for-target and runs the consumer body inline; the frame's user
// slots become loop-carried phis (param slots seeded from the `AllocTask` args,
// local slots from the poll's entry-block init stores).
//
// Multi-yield-SITE generators (sequential `yield a; yield b; ...`) need a
// return-dispatch over yield-delimited segments — doc-26 Phase-1 Finding #1 —
// and bail soundly here (the generator stays Tier D: a correct heap frame).

/// A user frame slot's resolved promotion data.
pub(super) struct SlotInfo {
    /// Frame byte offset (`>= GEN_CONTROL_BYTES`).
    pub(super) offset: i64,
    /// The preheader init value, expressed in the CALLER's value space (a clone
    /// of the AllocTask arg for a param slot, or a fresh clone of the poll's
    /// entry init for a local slot, or a fresh `None` for an unwritten slot).
    pub(super) init_caller_val: ValueId,
}

/// Apply the fusion splice for `candidate`. Returns `true` iff the caller was
/// mutated; `false` on a conservative bail (caller left byte-identical).
fn apply_fusion(
    caller: &mut TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
    stats: &mut FusionStats,
) -> bool {
    // --- Phase-1 gate: exactly one yield site. ---
    let yield_count: usize = poll
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| opcode_generator_fusion_poll_role_table(op.opcode).is_required_yield())
        .count();
    if yield_count != 1 {
        // Multi-yield-site (sequential `yield a; yield b; ...`) needs a
        // return-dispatch over yield-delimited segments — doc-26 Phase-1
        // Finding #1. Conservative bail: the generator stays Tier D.
        return false;
    }

    // --- Consumer-carried-state gate. A function-scope consumer threads its own
    //     loop-carried values (e.g. an accumulator `total`) as block ARGUMENTS
    //     on its loop header — the standard SSA loop-phi form. Splicing the
    //     generator's loop in between those edges requires re-threading those
    //     carried values through the fused loop (doc-26 Phase-1 Finding #1,
    //     function-scope extension). Phase 1 handles the consumer whose loop
    //     region carries NO block args (module-scope consumers keep `total` in the
    //     module dict via ModuleGetAttr/SetAttr, so their loop blocks are
    //     arg-less); bail soundly (Tier D) when any block in the consumer loop
    //     region — the cond/body blocks, the loop header, and the continue target
    //     the body branches back to — carries args. ---
    let mut consumer_region: Vec<BlockId> = vec![candidate.cond_block, candidate.body_block];
    if let Some(h) = candidate.loop_header {
        consumer_region.push(h);
    }
    // The block the body loops back to (the continue target) is the carried-phi
    // header in the function-scope shape.
    if let Some(body) = caller.blocks.get(&candidate.body_block) {
        match &body.terminator {
            Terminator::Branch { target, .. } => consumer_region.push(*target),
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                consumer_region.push(*then_block);
                consumer_region.push(*else_block);
            }
            _ => {}
        }
    }
    for b in consumer_region {
        if caller
            .blocks
            .get(&b)
            .is_some_and(|blk| !blk.args.is_empty())
        {
            return false;
        }
    }

    // --- Resolve the AllocTask args (the generator's parameter values, caller
    //     space) so param slots can be seeded. ---
    let alloc_args: Vec<ValueId> = caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx]
        .operands
        .clone();

    // --- Plan each user slot: offset + caller-space init value. A slot whose
    //     init cannot be resolved soundly bails the whole splice. ---
    let user_slots = collect_user_frame_slots(poll);
    let mut slot_infos: Vec<SlotInfo> = Vec::with_capacity(user_slots.len());
    // Pre-materialize init values in the caller. We append const/copy ops into
    // the AllocTask block before the AllocTask (so they dominate the loop).
    let mut preheader_init_ops: Vec<TirOp> = Vec::new();
    for &offset in &user_slots {
        // Param slot? offset == GEN_CONTROL_BYTES + 8*i, i < alloc_args.len().
        let rel = offset - GEN_CONTROL_BYTES;
        if rel % 8 != 0 {
            return false; // non-8-aligned slot — unexpected shape, bail.
        }
        let idx = (rel / 8) as usize;
        if idx < alloc_args.len() {
            // Parameter slot: init = the AllocTask arg (already a caller value).
            slot_infos.push(SlotInfo {
                offset,
                init_caller_val: alloc_args[idx],
            });
            continue;
        }
        // Local slot: init from the poll entry-block init store, materialized as
        // a caller const. We only support a const/None init in Phase 1 (the
        // common `i = 0` / unbound-local case); a non-const local init bails.
        let init_val = match local_slot_init_const(poll, offset) {
            Some(LocalInit::Int(v)) => {
                let nv = caller.fresh_value();
                caller.value_types.insert(nv, TirType::I64);
                preheader_init_ops.push(const_int_op(nv, v));
                nv
            }
            Some(LocalInit::None_) => {
                let nv = caller.fresh_value();
                caller.value_types.insert(nv, TirType::None);
                preheader_init_ops.push(const_none_op(nv));
                nv
            }
            None => return false, // non-trivial local init — bail (Tier D).
        };
        slot_infos.push(SlotInfo {
            offset,
            init_caller_val: init_val,
        });
    }

    // --- Clone + rewrite the poll body into the caller. ---
    let Some(clone) = clone_and_rewrite_poll(poll, caller, &slot_infos) else {
        // The clone bailed (e.g. an unpromotable slot store pattern). Any fresh
        // ids / preheader ops we minted are inert (never inserted into a block),
        // so the caller is still byte-identical.
        return false;
    };

    // --- Wire the fused loop. ---
    if !wire_fused_loop(caller, candidate, &clone, &slot_infos, preheader_init_ops) {
        return false;
    }

    stats.frames_elided += 1;
    stats.yield_sites_spliced += 1;

    // SSA-validity is an invariant of the splice, not a hope: a malformed splice
    // panics here rather than silently corrupting the program (mirrors the E1
    // inliner). The `run_pipeline` re-run the driver performs verifies again.
    if let Err(errors) = super::super::verify::verify_function(caller) {
        panic!(
            "[generator_fusion] verification failed after splicing poll '{}' into '{}': {:?}",
            candidate.poll_name, caller.name, errors
        );
    }
    true
}
