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

use std::collections::{BTreeSet, HashMap, HashSet};

use super::super::blocks::{BlockId, Terminator, TirBlock};
use super::super::call_graph::CallGraph;
use super::super::function::{TirFunction, TirModule};
use super::super::op_kinds_generated::opcode_has_exception_label_attr_table;
use super::super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::super::target_info::TargetInfo;
use super::super::types::TirType;
use super::super::values::{TirValue, ValueId};

/// Byte size of the generator control header. Frame offsets `< GEN_CONTROL_BYTES`
/// are the control slots — `GEN_SEND_OFFSET=0` (the `.send()` value),
/// `GEN_THROW_OFFSET=8` (the pending `.throw()` exception), `GEN_CLOSED_OFFSET=16`
/// (the exhausted flag), `GEN_YIELD_FROM_OFFSET=32` (the delegation target);
/// offsets `>= GEN_CONTROL_BYTES` are the generator's captured params + spilled
/// locals. Mirrors `GEN_CONTROL_SIZE` in `src/molt/frontend/_types.py` and
/// `crate::GENERATOR_CONTROL_BYTES`.
const GEN_CONTROL_BYTES: i64 = 48;

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
fn attr_value_int(op: &TirOp) -> Option<i64> {
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
fn attr_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// True if `op` is the consumer's `GetIter` over a value — either the
/// first-class [`OpCode::GetIter`] or the runtime `iter` op (lowered as a
/// `Copy` carrying `_original_kind == "iter"`, the form the frontend emits for
/// `for x in <expr>`).
fn is_get_iter_op(op: &TirOp) -> bool {
    op.opcode == OpCode::GetIter
        || (op.opcode == OpCode::Copy && attr_original_kind(op) == Some("iter"))
}

/// A recognized fusion candidate: an `AllocTask(generator)` consumed by a single
/// `GetIter` → single `IterNext`-loop in `caller`.
struct FusionCandidate {
    /// Block + op index of the `AllocTask` in the caller.
    alloc_block: BlockId,
    alloc_idx: usize,
    /// The generator frame value produced by `AllocTask`.
    alloc_val: ValueId,
    /// The `_poll` function name (a module-defined function).
    poll_name: String,
    /// Block holding the `GetIter` (or `iter` Copy) in the caller.
    get_iter_block: BlockId,
    /// The iterator value produced by `GetIter`.
    iter_val: ValueId,
    /// The loop-condition block holding the `IterNext` + done-check.
    cond_block: BlockId,
    /// The `(value, done)` pair value produced by `IterNext`.
    pair_val: ValueId,
    /// The block holding the `Index(pair, 0)` element-extraction (the body block,
    /// or the cond block if the element is extracted before the branch).
    elem_block: BlockId,
    /// The element value (`pair[0]`).
    elem_val: ValueId,
    /// The block control branches to on `done == true` (loop exit) and
    /// `done == false` (loop body).
    exit_block: BlockId,
    body_block: BlockId,
    /// The loop header (the `LoopHeader`-role block that targets `cond_block`).
    /// Present iff the consumer carries structured loop metadata.
    loop_header: Option<BlockId>,
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
            match op.opcode {
                OpCode::StateYield => has_yield = true,
                OpCode::YieldFrom
                | OpCode::Yield
                | OpCode::StateBlockStart
                | OpCode::StateBlockEnd
                | OpCode::ChanSendYield
                | OpCode::ChanRecvYield
                | OpCode::StateTransition
                | OpCode::AllocTask => return false,
                _ => {}
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
            match op.opcode {
                OpCode::IterNext if op.operands.first() == Some(&iter_val) => saw_next = true,
                OpCode::Is => { /* the `is(iter, None)` not-iterable guard — benign */ }
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
struct SlotInfo {
    /// Frame byte offset (`>= GEN_CONTROL_BYTES`).
    offset: i64,
    /// The preheader init value, expressed in the CALLER's value space (a clone
    /// of the AllocTask arg for a param slot, or a fresh clone of the poll's
    /// entry init for a local slot, or a fresh `None` for an unwritten slot).
    init_caller_val: ValueId,
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
        .filter(|op| op.opcode == OpCode::StateYield)
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

/// A local slot's entry-init constant.
enum LocalInit {
    Int(i64),
    None_,
}

/// Resolve a LOCAL slot's entry init: the value the poll's entry block stores
/// into `offset` before the loop. Phase 1 supports a `ConstInt` init or a
/// `None`/`missing` init (the unbound-local sentinel). Returns `None` for any
/// other (non-trivially-promotable) init.
fn local_slot_init_const(poll: &TirFunction, offset: i64) -> Option<LocalInit> {
    let entry = poll.blocks.get(&poll.entry_block)?;
    // The LAST entry-block store to this slot is the effective init (a `missing`
    // sentinel store is typically followed by the real `= 0` store).
    let mut result: Option<LocalInit> = None;
    for op in &entry.ops {
        if op.opcode == OpCode::ClosureStore && attr_value_int(op) == Some(offset) {
            let &stored = op.operands.get(1)?;
            let loc = def_location(poll, stored)?;
            let def = &poll.blocks[&loc.0].ops[loc.1];
            result = if def.opcode == OpCode::ConstInt {
                Some(LocalInit::Int(attr_value_int(def)?))
            } else if def.opcode == OpCode::ConstNone || attr_original_kind(def) == Some("missing")
            {
                Some(LocalInit::None_)
            } else {
                return None;
            };
        }
    }
    result
}

/// Locate the (block, op_idx) defining `v` (single-result ops).
fn def_location(func: &TirFunction, v: ValueId) -> Option<(BlockId, usize)> {
    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            if op.results.first() == Some(&v) {
                return Some((bid, i));
            }
        }
    }
    None
}

fn const_int_op(result: ValueId, value: i64) -> TirOp {
    let mut a = AttrDict::new();
    a.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: a,
        source_span: None,
    }
}

fn const_none_op(result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

// ---------------------------------------------------------------------------
// Clone + rewrite the poll body
// ---------------------------------------------------------------------------

/// The product of cloning + rewriting the poll body into the caller.
struct ClonedPoll {
    /// Fresh entry block id of the cloned body (the preheader spine).
    entry: BlockId,
    /// Every fresh cloned block id (deterministic order).
    cloned_blocks: Vec<BlockId>,
    /// The cloned block + op index holding the (single) `state_yield`.
    yield_block: BlockId,
    yield_idx: usize,
    /// The yielded pair value (cloned).
    yield_pair: ValueId,
    /// Cloned blocks terminating in `Return` (the exhausted / normal exits).
    return_blocks: Vec<BlockId>,
    /// Slot phi value per user slot (index-aligned with the `slot_infos` passed
    /// to [`clone_and_rewrite_poll`]).
    slot_phis: Vec<ValueId>,
    /// Per slot, the value flowing on the loop back-edge (the cloned in-loop
    /// store value), or `None` for a loop-invariant slot (no in-loop store →
    /// thread the phi unchanged).
    slot_backedge: Vec<Option<ValueId>>,
}

/// True if `op` is a generator-frame bookkeeping op the splice drops: trace
/// slots, exception-stack save/restore, source-line markers. These are frame
/// activation/teardown overhead with no fused-loop meaning.
fn is_bookkeeping_op(op: &TirOp) -> bool {
    matches!(
        attr_original_kind(op),
        Some(
            "trace_enter_slot"
                | "trace_exit"
                | "exception_stack_enter"
                | "exception_stack_depth"
                | "exception_stack_exit"
                | "exception_stack_set_depth"
                | "line"
        )
    )
}

/// Clone the poll body into the caller with fresh ids, applying the frame-slot
/// promotion + control-slot elimination rewrites. Returns `None` (bail) on any
/// unpromotable shape (a user slot stored in more than one in-loop site, an
/// `IncRef`/`DecRef` of the frame pointer, etc.).
fn clone_and_rewrite_poll(
    poll: &TirFunction,
    caller: &mut TirFunction,
    slot_infos: &[SlotInfo],
) -> Option<ClonedPoll> {
    // Map each user slot offset -> a fresh slot phi value, and -> its index.
    let slot_phis: Vec<ValueId> = slot_infos
        .iter()
        .map(|_| {
            let v = caller.fresh_value();
            caller.value_types.entry(v).or_insert(TirType::DynBox);
            v
        })
        .collect();
    let slot_index: HashMap<i64, usize> = slot_infos
        .iter()
        .enumerate()
        .map(|(i, s)| (s.offset, i))
        .collect();

    // Fresh exception-label remap (mirrors the inliner): the poll body's
    // per-function SimpleIR labels must not collide with the caller's.
    let label_remap = build_label_remap(poll, caller);

    // Value remap: poll ValueId -> caller ValueId. Pre-seed user-slot loads to
    // the slot phi and control-slot loads to a shared `None`.
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();

    // A single cloned `None` (for send/throw slot reads) materialized in the
    // cloned entry block.
    let none_for_control = caller.fresh_value();
    caller
        .value_types
        .entry(none_for_control)
        .or_insert(TirType::None);

    // Pre-seed: every `closure_load(self, off)` result.
    for block in poll.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ClosureLoad
                && let Some(off) = attr_value_int(op)
                && let Some(&res) = op.results.first()
            {
                if off >= GEN_CONTROL_BYTES {
                    let Some(&idx) = slot_index.get(&off) else {
                        return None; // load of a slot we didn't plan — bail.
                    };
                    value_map.insert(res, slot_phis[idx]);
                } else {
                    // Control slot (send=0 / throw=8 / others): reads `None`.
                    value_map.insert(res, none_for_control);
                }
            }
        }
    }

    // The generator's exception-stack save/restore values: the results of the
    // prologue `exception_stack_enter` / `exception_stack_depth` ops. These ops
    // are bookkeeping (dropped), so their result values vanish. The body
    // restores them before every `check_exception` via a `Copy(exc_val, exc_val)`
    // (the SimpleIR `exception_stack_set_depth`/restore idiom captured as a Copy)
    // and passes the copies as `CheckException` operands. After fusion the
    // generator exception stack does not exist: we DROP those restore-copies and
    // CLEAR the `CheckException` operands (the consumer's own `CheckException`
    // carries no operands either — it reads the runtime pending flag directly).
    let exc_stack_vals: HashSet<ValueId> = poll
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| {
            matches!(
                attr_original_kind(op),
                Some("exception_stack_enter" | "exception_stack_depth")
            )
        })
        .filter_map(|op| op.results.first().copied())
        .collect();
    // Transitively include the restore-copies' results (a Copy of an exc value is
    // itself an exc-derived value that later copies/checks consume).
    let mut exc_derived = exc_stack_vals.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for block in poll.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Copy
                    && !op.attrs.contains_key("_original_kind")
                    && op.operands.iter().any(|v| exc_derived.contains(v))
                    && let Some(&res) = op.results.first()
                    && exc_derived.insert(res)
                {
                    changed = true;
                }
            }
        }
    }
    // The poll's exception-EXIT block (the `CheckException` handler/exit target)
    // receives the saved exc-stack values as BLOCK ARGS on the implicit exception
    // edge. Those args are exc-stack-derived too: fold them into `exc_derived` so
    // the clone strips them (the post-fusion exception edge carries no args).
    // The exit block is found via the inverse of `label_id_map`: the block whose
    // label is a `CheckException` `value` target.
    {
        let mut exc_target_labels: HashSet<i64> = HashSet::new();
        for block in poll.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::CheckException
                    && let Some(AttrValue::Int(l)) = op.attrs.get("value")
                {
                    exc_target_labels.insert(*l);
                }
            }
        }
        for (&block_u32, &label) in &poll.label_id_map {
            if exc_target_labels.contains(&label)
                && let Some(b) = poll.blocks.get(&BlockId(block_u32))
            {
                for arg in &b.args {
                    exc_derived.insert(arg.id);
                }
            }
        }
    }
    // Block remap: poll BlockId -> fresh caller BlockId (deterministic order).
    let mut poll_block_ids: Vec<BlockId> = poll.blocks.keys().copied().collect();
    poll_block_ids.sort_by_key(|b| b.0);
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();
    for &bid in &poll_block_ids {
        block_map.insert(bid, caller.fresh_block());
    }

    // Mint fresh value ids for every non-pre-seeded result and every block arg.
    let fresh_for = |old: ValueId,
                     value_map: &mut HashMap<ValueId, ValueId>,
                     caller: &mut TirFunction|
     -> ValueId {
        if let Some(&existing) = value_map.get(&old) {
            return existing;
        }
        let v = caller.fresh_value();
        value_map.insert(old, v);
        v
    };
    for &bid in &poll_block_ids {
        let block = &poll.blocks[&bid];
        for arg in &block.args {
            fresh_for(arg.id, &mut value_map, caller);
        }
        for op in &block.ops {
            for r in &op.results {
                fresh_for(*r, &mut value_map, caller);
            }
        }
    }

    let remap = |v: ValueId, vm: &HashMap<ValueId, ValueId>| -> ValueId {
        *vm.get(&v)
            .unwrap_or_else(|| panic!("generator_fusion: poll value {v} has no remap"))
    };
    let remap_block = |b: BlockId| -> BlockId {
        *block_map
            .get(&b)
            .unwrap_or_else(|| panic!("generator_fusion: poll block {b} has no remap"))
    };

    // Per-slot back-edge value: the LAST user-slot store's (remapped) value.
    // A slot stored in >1 distinct block (conditional store) bails — the simple
    // single-reaching-def threading would be unsound.
    let mut slot_store_blocks: Vec<Option<BlockId>> = vec![None; slot_infos.len()];
    let mut slot_backedge: Vec<Option<ValueId>> = vec![None; slot_infos.len()];

    let mut cloned_blocks: Vec<BlockId> = Vec::with_capacity(poll_block_ids.len());
    let mut yield_block_idx: Option<(BlockId, usize, ValueId)> = None;
    let mut return_blocks: Vec<BlockId> = Vec::new();

    for &bid in &poll_block_ids {
        let src = &poll.blocks[&bid];
        let new_bid = remap_block(bid);
        cloned_blocks.push(new_bid);

        // Cloned block args (entry stays arg-less — the poll's `self` param is
        // eliminated; no other block in a well-formed poll carries args except
        // the exception-exit block, which becomes unreachable).
        // The cloned entry is arg-less (`self` is eliminated). Every other block:
        // keep its args EXCEPT the exception-stack values (`exc_derived`). The
        // poll's exception-exit block carries the saved exc-stack depth/value as
        // args, supplied on the implicit `CheckException` edge; after fusion that
        // edge passes no args (the consumer's own handler convention), so a
        // retained exc-stack arg would be an unsatisfied phi at the exception
        // edge ("predecessor … branches with 0 argument(s) but phi … required").
        // The ops that consumed those args were the dropped exc-stack-restore
        // copies, so the args are dead and safely removed.
        let new_args: Vec<TirValue> = if bid == poll.entry_block {
            Vec::new()
        } else {
            src.args
                .iter()
                .filter(|a| !exc_derived.contains(&a.id))
                .map(|a| TirValue {
                    id: remap(a.id, &value_map),
                    ty: a.ty.clone(),
                })
                .collect()
        };

        let mut new_ops: Vec<TirOp> = Vec::with_capacity(src.ops.len());
        for op in src.ops.iter() {
            // Drop bookkeeping + the lone state_switch.
            if op.opcode == OpCode::StateSwitch || is_bookkeeping_op(op) {
                continue;
            }
            // Drop closure_load (its result was pre-seeded to a phi/None).
            if op.opcode == OpCode::ClosureLoad {
                continue;
            }
            // closure_store: control slot -> drop; user slot -> record back-edge.
            if op.opcode == OpCode::ClosureStore {
                let off = attr_value_int(op).unwrap_or(-1);
                if off >= GEN_CONTROL_BYTES {
                    let &idx = slot_index.get(&off)?;
                    let &stored = op.operands.get(1)?;
                    // Entry-block stores are the init (handled in the preheader),
                    // not the back-edge. Only record stores OUTSIDE the entry.
                    if bid != poll.entry_block {
                        if let Some(prev) = slot_store_blocks[idx]
                            && prev != bid
                        {
                            return None; // conditional/multi-block store — bail.
                        }
                        slot_store_blocks[idx] = Some(bid);
                        slot_backedge[idx] = Some(remap(stored, &value_map));
                    }
                }
                continue;
            }
            // state_yield: keep a marker copy (rewritten in wire_fused_loop). We
            // record its location and pair operand, and DROP it from the op
            // stream — the split happens at this index in the cloned block.
            if op.opcode == OpCode::StateYield {
                let &pair = op.operands.first()?;
                yield_block_idx = Some((new_bid, new_ops.len(), remap(pair, &value_map)));
                continue;
            }
            // Drop the exception-stack restore-copies (a `Copy(exc_val, ..)`
            // whose result is an exc-derived value). After fusion the generator
            // exception stack does not exist; these are pure bookkeeping.
            if op.opcode == OpCode::Copy
                && op.results.first().is_some_and(|r| exc_derived.contains(r))
            {
                continue;
            }
            // `CheckException` propagates a body exception to the function exit;
            // it is kept, but its operands (the cloned exception-stack restore
            // values) are CLEARED — the consumer's own `CheckException` reads the
            // runtime pending flag directly and carries no operands.
            let mut attrs = clone_attrs_drop_simple_names(&op.attrs);
            remap_exception_label_attr_local(op.opcode, &mut attrs, &label_remap);
            let operands: Vec<ValueId> = if op.opcode == OpCode::CheckException {
                Vec::new()
            } else {
                op.operands.iter().map(|v| remap(*v, &value_map)).collect()
            };
            new_ops.push(TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands,
                results: op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                attrs,
                source_span: op.source_span,
            });
        }

        let new_term = clone_terminator_local(&src.terminator, &value_map, &block_map);
        if matches!(new_term, Terminator::Return { .. }) {
            return_blocks.push(new_bid);
        }

        caller.blocks.insert(
            new_bid,
            TirBlock {
                id: new_bid,
                args: new_args,
                ops: new_ops,
                terminator: new_term,
            },
        );
    }

    // Materialize the shared `None` for control-slot reads at the top of the
    // cloned entry block (dominates every use).
    let entry_clone = remap_block(poll.entry_block);
    caller
        .blocks
        .get_mut(&entry_clone)
        .unwrap()
        .ops
        .insert(0, const_none_op(none_for_control));

    // Transfer the poll's value_types for cloned values (remapped keys).
    let poll_param_ids: HashSet<ValueId> = poll.blocks[&poll.entry_block]
        .args
        .iter()
        .map(|a| a.id)
        .collect();
    for (old, ty) in &poll.value_types {
        if poll_param_ids.contains(old) {
            continue;
        }
        if let Some(&new) = value_map.get(old) {
            caller.value_types.entry(new).or_insert_with(|| ty.clone());
        }
    }

    // Transfer the poll's `label_id_map` (BlockId.0 → SimpleIR label) with the
    // block key remapped through `block_map` and the label VALUE remapped through
    // `label_remap` — the same table the cloned `CheckException`/`TryStart`/
    // `TryEnd` ops' `value` attrs were rewritten through. Without this, a cloned
    // `CheckException` whose handler/exit label was remapped to N has no block
    // carrying label N, and LLVM lowering fails ("check_exception target label N
    // is not present in label map"); the native back-conversion likewise cannot
    // resolve the exception edge.
    for (old_block_u32, label_val) in &poll.label_id_map {
        if let Some(new_bid) = block_map.get(&BlockId(*old_block_u32)) {
            let new_label = label_remap.get(label_val).copied().unwrap_or(*label_val);
            caller.label_id_map.entry(new_bid.0).or_insert(new_label);
        }
    }

    let (yield_block, yield_idx, yield_pair) = yield_block_idx?;

    Some(ClonedPoll {
        entry: entry_clone,
        cloned_blocks,
        yield_block,
        yield_idx,
        yield_pair,
        return_blocks,
        slot_phis,
        slot_backedge,
    })
}

/// Clone an op's attrs, dropping the SimpleIR value-name annotations (which are
/// function-local name strings with no id to remap — copying them verbatim would
/// alias the poll's names onto caller values).
fn clone_attrs_drop_simple_names(attrs: &AttrDict) -> AttrDict {
    attrs
        .iter()
        .filter(|(k, _)| k.as_str() != "_simple_out" && !k.starts_with("_simple_result_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Rewrite a cloned exception op's `value` label through `label_remap`.
fn remap_exception_label_attr_local(
    opcode: OpCode,
    attrs: &mut AttrDict,
    label_remap: &HashMap<i64, i64>,
) {
    if !opcode_has_exception_label_attr_table(opcode) {
        return;
    }
    if let Some(AttrValue::Int(old)) = attrs.get("value")
        && let Some(&new) = label_remap.get(old)
    {
        attrs.insert("value".into(), AttrValue::Int(new));
    }
}

/// Build the poll->fresh exception-label remap (mirrors the inliner's
/// `build_label_remap`): every label the poll uses is reassigned strictly above
/// the caller's current max so the cloned exception edges cannot collide.
fn build_label_remap(poll: &TirFunction, caller: &TirFunction) -> HashMap<i64, i64> {
    let poll_labels = function_label_ids(poll);
    if poll_labels.is_empty() {
        return HashMap::new();
    }
    let caller_max = function_label_ids(caller).iter().copied().max();
    let start = caller_max.map(|m| m + 1).unwrap_or(0);
    let mut remap = HashMap::with_capacity(poll_labels.len());
    for (label, next) in poll_labels.into_iter().zip(start..) {
        remap.insert(label, next);
    }
    remap
}

/// The set of SimpleIR label ids `func` uses (label_id_map values + exception-op
/// `value` labels).
fn function_label_ids(func: &TirFunction) -> BTreeSet<i64> {
    let mut labels: BTreeSet<i64> = func.label_id_map.values().copied().collect();
    for block in func.blocks.values() {
        for op in &block.ops {
            if opcode_has_exception_label_attr_table(op.opcode)
                && let Some(AttrValue::Int(l)) = op.attrs.get("value")
            {
                labels.insert(*l);
            }
        }
    }
    labels
}

/// Clone a terminator, remapping value operands + block targets.
fn clone_terminator_local(
    term: &Terminator,
    value_map: &HashMap<ValueId, ValueId>,
    block_map: &HashMap<BlockId, BlockId>,
) -> Terminator {
    let rv = |v: ValueId| *value_map.get(&v).unwrap_or(&v);
    let rb = |b: BlockId| *block_map.get(&b).unwrap_or(&b);
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: rb(*target),
            args: args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: rv(*cond),
            then_block: rb(*then_block),
            then_args: then_args.iter().map(|v| rv(*v)).collect(),
            else_block: rb(*else_block),
            else_args: else_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: rv(*value),
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

// ---------------------------------------------------------------------------
// Wire the fused loop
// ---------------------------------------------------------------------------

/// Wire the cloned (rewritten) poll body into the consumer loop:
///  * add the slot phis as args on the cloned loop header; thread init values
///    from the preheader and back-edge values from the loop latch;
///  * splice the consumer body at the yield site (bind `elem`, `IncRef`, run the
///    body, return to the post-yield continuation);
///  * route the cloned exhausted-return to the consumer's loop exit;
///  * delete the frame-creation ops (`AllocTask`/`GetIter`/`IterNext`) and
///    redirect the consumer's loop entry to the generator preheader.
///
/// Returns `false` (bail) on any structural surprise (no detectable loop header
/// for a yield that the recognition required be in a loop, etc.).
fn wire_fused_loop(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    clone: &ClonedPoll,
    slot_infos: &[SlotInfo],
    preheader_init_ops: Vec<TirOp>,
) -> bool {
    let n_slots = slot_infos.len();

    // --- 1. Detect the cloned loop header (the back-edge target). ---
    // The cloned blocks are NOT yet connected to the caller's CFG (the
    // preheader is wired in step 5), so a global dominance walk would treat them
    // as unreachable. Detect the loop header purely WITHIN the cloned subgraph:
    // a DFS from the cloned entry over cloned successors; a back-edge is an edge
    // C→H where H is still on the DFS stack (an ancestor of C). H is the loop
    // header, C the latch. If no back-edge exists the yield is straight-line
    // (`def g(): yield x`) and the slots flow through without a phi.
    let cloned_set: HashSet<BlockId> = clone.cloned_blocks.iter().copied().collect();
    let (loop_header, latch) = detect_cloned_back_edge(caller, clone.entry, &cloned_set);

    // --- 2. Add slot phis as header args + thread the slot values. ---
    if let Some(header) = loop_header {
        let Some(latch) = latch else { return false };
        // Append slot phis to the header's args. Precompute the phi types
        // BEFORE the mutable header borrow (the type comes from the slot's init
        // value's recorded fact).
        let phi_types: Vec<TirType> = (0..n_slots)
            .map(|i| caller_value_ty(caller_ty_lookup(caller, slot_infos, i)))
            .collect();
        {
            let hdr = caller.blocks.get_mut(&header).unwrap();
            for (i, &phi) in clone.slot_phis.iter().enumerate() {
                hdr.args.push(TirValue {
                    id: phi,
                    ty: phi_types[i].clone(),
                });
            }
        }
        // Every predecessor of `header` must now pass `n_slots` extra args.
        //   * the preheader (cloned entry): the init values.
        //   * the latch (back-edge): the back-edge values (phi for invariants).
        //   * any other pred is unexpected for a generator loop → bail.
        // Compute preds within the cloned subgraph (the cloned blocks are not yet
        // connected to the rest of the caller).
        let preds: Vec<BlockId> = cloned_set
            .iter()
            .copied()
            .filter(|&b| block_targets(caller, b, header))
            .collect();
        for pred in preds {
            let init_args: Vec<ValueId> = if pred == clone.entry {
                slot_infos.iter().map(|s| s.init_caller_val).collect()
            } else if pred == latch {
                (0..n_slots)
                    .map(|i| clone.slot_backedge[i].unwrap_or(clone.slot_phis[i]))
                    .collect()
            } else {
                // A third pred (e.g. an irreducible edge) — Phase-1 bail.
                return false;
            };
            append_branch_args(caller, pred, header, &init_args);
        }
    } else {
        // No loop: the slots are straight-line. Replace each slot phi's uses by
        // its init value directly (no phi needed). We do this by retargeting the
        // value in every cloned op — but since the clone already substituted
        // closure-loads to the phi id, we instead seed the phi as a Copy of the
        // init at the entry. Insert `phi = Copy(init)` at the cloned entry top.
        let entry = clone.entry;
        let entry_block = caller.blocks.get_mut(&entry).unwrap();
        for (i, &phi) in clone.slot_phis.iter().enumerate() {
            entry_block.ops.insert(
                0,
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: vec![slot_infos[i].init_caller_val],
                    results: vec![phi],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            );
        }
    }

    // --- 3. Splice the consumer body at the yield site. ---
    // Split the cloned yield block into [pre-yield | post-yield].
    let (pre_block, post_block) = match split_block_at(caller, clone.yield_block, clone.yield_idx) {
        Some(pair) => pair,
        None => return false,
    };
    // pre_block ends (currently) with a Branch to post_block (from split). We
    // instead: extract elem = Index(yield_pair, 0), IncRef(elem), branch to the
    // consumer body. The consumer body (the caller's body_block) on continue
    // branches to post_block.
    let elem_index_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![clone.yield_pair, const_zero(caller)],
        results: vec![candidate.elem_val],
        attrs: {
            let mut a = AttrDict::new();
            a.insert("container_type".into(), AttrValue::Str("tuple".into()));
            a
        },
        source_span: None,
    };
    let incref_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IncRef,
        operands: vec![candidate.elem_val],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    {
        let pb = caller.blocks.get_mut(&pre_block).unwrap();
        pb.ops.push(elem_index_op);
        pb.ops.push(incref_op);
        pb.terminator = Terminator::Branch {
            target: candidate.body_block,
            args: Vec::new(),
        };
    }

    // The consumer body block currently starts with `elem = Index(orig_pair, 0)`
    // (referencing the now-dead IterNext pair). Remove that leading op (elem is
    // now bound by `pre_block`).
    remove_orig_elem_index(caller, candidate);

    // --- 4. Route the cloned exhausted-return blocks to the loop exit. ---
    for &rb in &clone.return_blocks {
        caller.blocks.get_mut(&rb).unwrap().terminator = Terminator::Branch {
            target: candidate.exit_block,
            args: Vec::new(),
        };
    }

    // --- 5. Delete the frame-creation ops. ---
    delete_frame_creation_ops(caller, candidate, clone.entry, preheader_init_ops);

    // --- 6. Rewire the consumer's old loop header edges. The old loop header
    //        (`loop_header`, e.g. the `loop_start` block) had two kinds of
    //        predecessor: the loop ENTRY (from outside the loop) and the
    //        CONTINUE back-edge (from the consumer body). After fusion:
    //          * the ENTRY edge → the generator preheader (the cloned entry);
    //          * the CONTINUE edge → the generator post-yield block.
    //        We split the old-header preds by whether they are reachable from
    //        `body_block` (continue) or not (entry). The old header + the old
    //        cond/iter_next block become unreachable and DCE removes them.
    if !rewire_consumer_header_edges(caller, candidate, clone.entry, post_block) {
        return false;
    }

    // --- 7. Prune the now-unreachable old consumer-loop blocks. After the
    //        rewiring, the consumer's old loop header + cond block (with its
    //        `IterNext`/done-`Index` on the deleted pair) are unreachable from
    //        entry. `verify_function` skips unreachable blocks, but the
    //        TIR→SimpleIR back-conversion would still emit their `jump`/`label`
    //        ops + dangling uses of the deleted pair value — which the native
    //        codegen's `jump` handler rejects (`label_blocks[&target_id]` panic).
    //        Remove them here so codegen never sees them. ---
    prune_unreachable_blocks(caller);

    true
}

/// Remove every block unreachable from the function entry, and drop any dangling
/// `loop_*` / `label_id_map` metadata keyed on them. This is a self-contained
/// cleanup so the splice never hands codegen an unreachable block carrying stale
/// ops (a use of a deleted value, a `jump` to a removed label).
///
/// Reachability uses the FULL CFG-edge policy (terminator edges PLUS the implicit
/// `CheckException` → handler/exit edges): a cloned exception-exit block is
/// reached only via the propagated-exception edge, never a terminator, so a
/// terminator-only walk would wrongly delete it — and then a surviving
/// `CheckException` whose `value` label targets it fails LLVM lowering
/// ("check_exception target label N is not present in label map").
fn prune_unreachable_blocks(caller: &mut TirFunction) {
    use super::super::dominators::{CfgEdgePolicy, reachable_blocks_with};
    let reachable = reachable_blocks_with(caller, CfgEdgePolicy::Full);
    let dead: Vec<BlockId> = caller
        .blocks
        .keys()
        .copied()
        .filter(|b| !reachable.contains(b))
        .collect();
    for b in dead {
        caller.blocks.remove(&b);
        caller.loop_roles.remove(&b);
        caller.loop_pairs.remove(&b);
        caller.loop_break_kinds.remove(&b);
        caller.loop_cond_blocks.remove(&b);
        caller.label_id_map.remove(&b.0);
    }
    // Drop loop metadata whose VALUE (end / cond block) was pruned.
    let live: HashSet<BlockId> = caller.blocks.keys().copied().collect();
    caller
        .loop_pairs
        .retain(|h, e| live.contains(h) && live.contains(e));
    caller
        .loop_cond_blocks
        .retain(|h, c| live.contains(h) && live.contains(c));
}

/// Detect the loop header + latch within the cloned subgraph via a DFS from the
/// cloned entry. A back-edge is an edge `C -> H` where `H` is on the DFS stack
/// when `C`'s successors are walked (`H` is an ancestor of `C`). Returns
/// `(Some(header), Some(latch))` for the FIRST back-edge found, or `(None, None)`
/// if the cloned region is acyclic (a straight-line yield).
fn detect_cloned_back_edge(
    caller: &TirFunction,
    entry: BlockId,
    cloned: &HashSet<BlockId>,
) -> (Option<BlockId>, Option<BlockId>) {
    let succs = |b: BlockId| -> Vec<BlockId> {
        match caller.blocks.get(&b).map(|blk| &blk.terminator) {
            Some(Terminator::Branch { target, .. }) => vec![*target],
            Some(Terminator::CondBranch {
                then_block,
                else_block,
                ..
            }) => vec![*then_block, *else_block],
            Some(Terminator::Switch { cases, default, .. }) => {
                let mut v: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
                v.push(*default);
                v
            }
            _ => Vec::new(),
        }
    };
    // Iterative DFS tracking the current path stack (ancestors).
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut on_stack: HashSet<BlockId> = HashSet::new();
    // Stack frames: (block, next-successor-index, successor-list).
    let mut stack: Vec<(BlockId, usize, Vec<BlockId>)> = Vec::new();
    visited.insert(entry);
    on_stack.insert(entry);
    stack.push((entry, 0, succs(entry)));
    while !stack.is_empty() {
        let (node, i, s) = {
            let top = stack.last().unwrap();
            (top.0, top.1, top.2.clone())
        };
        if i < s.len() {
            stack.last_mut().unwrap().1 += 1;
            let next = s[i];
            if !cloned.contains(&next) {
                continue; // an edge leaving the cloned region — ignore.
            }
            if on_stack.contains(&next) {
                // Back-edge node -> next: next is the header, node the latch.
                return (Some(next), Some(node));
            }
            if visited.insert(next) {
                on_stack.insert(next);
                let ns = succs(next);
                stack.push((next, 0, ns));
            }
        } else {
            on_stack.remove(&node);
            stack.pop();
        }
    }
    (None, None)
}

/// The TirType to record for slot `i`'s phi — derived from the slot's init
/// value's known type (param args carry their own type; const ints are I64).
fn caller_ty_lookup(caller: &TirFunction, slot_infos: &[SlotInfo], i: usize) -> Option<TirType> {
    caller
        .value_types
        .get(&slot_infos[i].init_caller_val)
        .cloned()
}

fn caller_value_ty(t: Option<TirType>) -> TirType {
    t.unwrap_or(TirType::DynBox)
}

/// True if `block`'s terminator targets `target`.
fn block_targets(caller: &TirFunction, block: BlockId, target: BlockId) -> bool {
    match caller.blocks.get(&block).map(|b| &b.terminator) {
        Some(Terminator::Branch { target: t, .. }) => *t == target,
        Some(Terminator::CondBranch {
            then_block,
            else_block,
            ..
        }) => *then_block == target || *else_block == target,
        Some(Terminator::Switch { cases, default, .. }) => {
            *default == target || cases.iter().any(|(_, t, _)| *t == target)
        }
        _ => false,
    }
}

/// Append `extra` args to `pred`'s branch terminator edge that targets `header`.
fn append_branch_args(caller: &mut TirFunction, pred: BlockId, header: BlockId, extra: &[ValueId]) {
    let block = caller.blocks.get_mut(&pred).unwrap();
    match &mut block.terminator {
        Terminator::Branch { target, args } if *target == header => {
            args.extend_from_slice(extra);
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                then_args.extend_from_slice(extra);
            }
            if *else_block == header {
                else_args.extend_from_slice(extra);
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            if *default == header {
                default_args.extend_from_slice(extra);
            }
            for (_, t, a) in cases.iter_mut() {
                if *t == header {
                    a.extend_from_slice(extra);
                }
            }
        }
        _ => {}
    }
}

/// Materialize a `ConstInt(0)` in the caller (for the `Index(pair, 0)` element
/// extraction), returning its value id. Cached-free: a fresh const each call is
/// fine (copy-prop/GVN dedups them in the re-run pipeline).
fn const_zero(caller: &mut TirFunction) -> ValueId {
    let v = caller.fresh_value();
    caller.value_types.insert(v, TirType::I64);
    // The const op is inserted by the caller of this fn into the pre-yield block.
    // To keep it dominating, we must actually emit it; we stash it via a thread
    // local is overkill — instead emit it directly into the entry block top.
    let entry = caller.entry_block;
    caller
        .blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .insert(0, const_int_op(v, 0));
    v
}

/// Split block `bid` after op index `idx` (the yield op was already dropped, so
/// `idx` is the position the post-yield ops begin). Returns `(pre, post)` block
/// ids; `pre` keeps the original id, `post` is fresh and takes the original
/// terminator + the ops `[idx..]`. `pre` is given a placeholder Branch to `post`
/// (the caller rewrites it).
fn split_block_at(
    caller: &mut TirFunction,
    bid: BlockId,
    idx: usize,
) -> Option<(BlockId, BlockId)> {
    let original = caller.blocks.remove(&bid)?;
    let TirBlock {
        id,
        args,
        mut ops,
        terminator,
    } = original;
    if idx > ops.len() {
        // restore and bail
        caller.blocks.insert(
            bid,
            TirBlock {
                id,
                args,
                ops,
                terminator,
            },
        );
        return None;
    }
    let post_ops = ops.split_off(idx);
    let post_id = caller.fresh_block();
    caller.blocks.insert(
        bid,
        TirBlock {
            id: bid,
            args,
            ops,
            terminator: Terminator::Branch {
                target: post_id,
                args: Vec::new(),
            },
        },
    );
    caller.blocks.insert(
        post_id,
        TirBlock {
            id: post_id,
            args: Vec::new(),
            ops: post_ops,
            terminator,
        },
    );
    Some((bid, post_id))
}

/// Remove the consumer body's leading `Index(orig_pair, 0) -> elem_val` op (it
/// now references the deleted IterNext pair; `elem_val` is rebound by the
/// yield-pre block).
fn remove_orig_elem_index(caller: &mut TirFunction, candidate: &FusionCandidate) {
    let block = caller.blocks.get_mut(&candidate.elem_block).unwrap();
    block.ops.retain(|op| {
        !(op.opcode == OpCode::Index
            && op.operands.first() == Some(&candidate.pair_val)
            && op.results.first() == Some(&candidate.elem_val))
    });
}

/// Delete the frame-creation ops (`AllocTask`, `GetIter`/`iter`, `IterNext`) and
/// seed the generator preheader's slot-init constants. The `GetIter` result is
/// replaced by a non-`None` sentinel const so the consumer's `is(iter, None)`
/// not-iterable guard folds False (the iterator never escapes after fusion).
fn delete_frame_creation_ops(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    preheader: BlockId,
    preheader_init_ops: Vec<TirOp>,
) {
    // (a) Remove the AllocTask op.
    if let Some(block) = caller.blocks.get_mut(&candidate.alloc_block) {
        block.ops.retain(|op| {
            !(op.opcode == OpCode::AllocTask && op.results.first() == Some(&candidate.alloc_val))
        });
    }
    // (b) Replace the GetIter op with ConstInt(1) producing iter_val (sentinel).
    if let Some(block) = caller.blocks.get_mut(&candidate.get_iter_block) {
        for op in block.ops.iter_mut() {
            if is_get_iter_op(op) && op.results.first() == Some(&candidate.iter_val) {
                *op = const_int_op(candidate.iter_val, 1);
                break;
            }
        }
    }
    caller.value_types.insert(candidate.iter_val, TirType::I64);
    // (c) Remove the IterNext op.
    if let Some(block) = caller.blocks.get_mut(&candidate.cond_block) {
        block.ops.retain(|op| {
            !(op.opcode == OpCode::IterNext && op.results.first() == Some(&candidate.pair_val))
        });
    }
    // (d) Prepend the preheader slot-init ops at the TOP of the cloned preheader
    //     (so they dominate the loop-header phi-arg uses).
    if !preheader_init_ops.is_empty()
        && let Some(pre) = caller.blocks.get_mut(&preheader)
    {
        for (i, op) in preheader_init_ops.into_iter().enumerate() {
            pre.ops.insert(i, op);
        }
    }
}

/// Rewire the consumer's old loop-header edges after the generator body has been
/// spliced in. The old loop header (`candidate.loop_header`, e.g. the
/// `loop_start` block; falls back to `cond_block`) has predecessors of two
/// kinds:
///   * the **continue** back-edge(s) from inside the consumer body region
///     (blocks reachable from `body_block` without leaving the loop) →
///     retargeted to `post_block` (the generator's post-yield continuation);
///   * the **entry** edge(s) from outside the loop → retargeted to `preheader`
///     (the generator's cloned entry).
///
/// Returns `false` if the header has a predecessor that is neither (an
/// unexpected irreducible shape) — a conservative bail.
fn rewire_consumer_header_edges(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    preheader: BlockId,
    post_block: BlockId,
) -> bool {
    let old_header = candidate.loop_header.unwrap_or(candidate.cond_block);

    // The consumer body region: blocks reachable from `body_block` without
    // passing through the old header or the loop exit (those bound the region).
    let body_region = reachable_avoiding(
        caller,
        candidate.body_block,
        &[old_header, candidate.exit_block],
    );

    // Every predecessor of `old_header`: classify + retarget its edge.
    let preds: Vec<BlockId> = caller
        .blocks
        .keys()
        .copied()
        .filter(|&b| block_targets(caller, b, old_header))
        .collect();
    for pred in preds {
        let new_target = if body_region.contains(&pred) {
            post_block // continue edge
        } else {
            preheader // entry edge
        };
        retarget_edges(caller, pred, old_header, new_target);
    }
    true
}

/// Retarget every edge from `block` that targets `from` so it targets `to`,
/// clearing the edge's args (the new target — preheader / post-yield — takes no
/// args from this edge; slot args are threaded separately at the header).
fn retarget_edges(caller: &mut TirFunction, block: BlockId, from: BlockId, to: BlockId) {
    if let Some(b) = caller.blocks.get_mut(&block) {
        match &mut b.terminator {
            Terminator::Branch { target, args } if *target == from => {
                *target = to;
                args.clear();
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == from {
                    *then_block = to;
                    then_args.clear();
                }
                if *else_block == from {
                    *else_block = to;
                    else_args.clear();
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                if *default == from {
                    *default = to;
                    default_args.clear();
                }
                for (_, t, a) in cases.iter_mut() {
                    if *t == from {
                        *t = to;
                        a.clear();
                    }
                }
            }
            _ => {}
        }
    }
}

/// The set of blocks reachable from `start` via terminator edges WITHOUT
/// entering any block in `barriers` (the barriers bound the search; `start`
/// itself is included even if it is a barrier).
fn reachable_avoiding(
    caller: &TirFunction,
    start: BlockId,
    barriers: &[BlockId],
) -> HashSet<BlockId> {
    let barrier: HashSet<BlockId> = barriers.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    seen.insert(start);
    while let Some(b) = stack.pop() {
        let succs: Vec<BlockId> = match caller.blocks.get(&b).map(|blk| &blk.terminator) {
            Some(Terminator::Branch { target, .. }) => vec![*target],
            Some(Terminator::CondBranch {
                then_block,
                else_block,
                ..
            }) => vec![*then_block, *else_block],
            Some(Terminator::Switch { cases, default, .. }) => {
                let mut v: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
                v.push(*default);
                v
            }
            _ => Vec::new(),
        };
        for s in succs {
            if barrier.contains(&s) {
                continue;
            }
            if seen.insert(s) {
                stack.push(s);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

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
    fn op_v(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>, value: i64) -> TirOp {
        let mut o = op(opcode, operands, results);
        o.attrs.insert("value".into(), AttrValue::Int(value));
        o
    }
    /// Allocate a fresh i64-typed value id for a constant. The matching
    /// `ConstInt` op (carrying `value`) is emitted separately by the caller; the
    /// `value` argument documents which constant this id stands for.
    fn const_int(f: &mut TirFunction, value: i64) -> ValueId {
        let _ = value;
        let id = f.fresh_value();
        f.value_types.insert(id, TirType::I64);
        id
    }

    /// Build a `counter(n)`-shaped single-yield-in-loop generator poll:
    ///   entry: i=0 (closure_store 56); br header
    ///   header: i=load56; n=load48; cond = i<n; not; br test
    ///   test: cond_br not -> exhausted, body
    ///   body: x = load56; pair=(x,false); state_yield pair,5;
    ///         (post) i2 = load56 + 1; closure_store 56, i2; br header
    ///   exhausted: closure_store 16 true; ret (None,True)
    fn counter_poll() -> TirFunction {
        let mut f = TirFunction::new("counter_poll".into(), vec![TirType::DynBox], TirType::None);
        // %0 = self
        let header = f.fresh_block();
        let test = f.fresh_block();
        let body = f.fresh_block();
        let exhausted = f.fresh_block();

        // entry
        let zero = const_int(&mut f, 0);
        {
            let e = f.blocks.get_mut(&f.entry_block).unwrap();
            e.ops.push(op_v(OpCode::ConstInt, vec![], vec![zero], 0));
            e.ops.push(op_v(
                OpCode::ClosureStore,
                vec![ValueId(0), zero],
                vec![],
                56,
            ));
            e.ops.push(op(OpCode::StateSwitch, vec![], vec![]));
            e.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }
        // header: load i, load n, cmp
        let i_h = f.fresh_value();
        f.value_types.insert(i_h, TirType::DynBox);
        let n_h = f.fresh_value();
        f.value_types.insert(n_h, TirType::DynBox);
        let cond = f.fresh_value();
        f.value_types.insert(cond, TirType::Bool);
        let notc = f.fresh_value();
        f.value_types.insert(notc, TirType::Bool);
        f.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![
                    op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![i_h], 56),
                    op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![n_h], 48),
                    op(OpCode::Lt, vec![i_h, n_h], vec![cond]),
                    op(OpCode::Not, vec![cond], vec![notc]),
                ],
                terminator: Terminator::Branch {
                    target: test,
                    args: vec![],
                },
            },
        );
        // test: cond_br not -> exhausted : body
        f.blocks.insert(
            test,
            TirBlock {
                id: test,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: notc,
                    then_block: exhausted,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        // body: x=load56; pair=(x,false); yield; post: i2=load56+1; store56; br header
        let x = f.fresh_value();
        f.value_types.insert(x, TirType::DynBox);
        let falsev = f.fresh_value();
        f.value_types.insert(falsev, TirType::Bool);
        let pair = f.fresh_value();
        f.value_types.insert(pair, TirType::DynBox);
        let i_b = f.fresh_value();
        f.value_types.insert(i_b, TirType::DynBox);
        let one = const_int(&mut f, 1);
        let i2 = f.fresh_value();
        f.value_types.insert(i2, TirType::DynBox);
        let mut pair_op = op(OpCode::Copy, vec![x, falsev], vec![pair]);
        pair_op
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("tuple_new".into()));
        f.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![x], 56),
                    {
                        let mut o = op(OpCode::ConstBool, vec![], vec![falsev]);
                        o.attrs.insert("value".into(), AttrValue::Bool(false));
                        o
                    },
                    pair_op,
                    op_v(OpCode::StateYield, vec![pair], vec![], 5),
                    op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![i_b], 56),
                    op_v(OpCode::ConstInt, vec![], vec![one], 1),
                    op(OpCode::Add, vec![i_b, one], vec![i2]),
                    op_v(OpCode::ClosureStore, vec![ValueId(0), i2], vec![], 56),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        // exhausted: store closed; ret (None, True)
        let none_v = f.fresh_value();
        f.value_types.insert(none_v, TirType::None);
        let true_v = f.fresh_value();
        f.value_types.insert(true_v, TirType::Bool);
        let donepair = f.fresh_value();
        f.value_types.insert(donepair, TirType::DynBox);
        let mut dp = op(OpCode::Copy, vec![none_v, true_v], vec![donepair]);
        dp.attrs
            .insert("_original_kind".into(), AttrValue::Str("tuple_new".into()));
        f.blocks.insert(
            exhausted,
            TirBlock {
                id: exhausted,
                args: vec![],
                ops: vec![
                    op(OpCode::ConstNone, vec![], vec![none_v]),
                    {
                        let mut o = op(OpCode::ConstBool, vec![], vec![true_v]);
                        o.attrs.insert("value".into(), AttrValue::Bool(true));
                        o
                    },
                    op_v(OpCode::ClosureStore, vec![ValueId(0), true_v], vec![], 16),
                    dp,
                ],
                terminator: Terminator::Return {
                    values: vec![donepair],
                },
            },
        );
        f
    }

    /// Build a consumer: `for x in counter(5): acc = acc + x` at function scope.
    ///   entry: n5=5; g=AllocTask(counter_poll, args=[n5], size=64);
    ///          it=iter(g); isnone=is(it,None); br guard
    ///   guard: cond_br isnone -> raise : loophdr
    ///   raise: ... br loophdr  (dead)
    ///   loophdr: br cond
    ///   cond: pair=iter_next(it); done=Index(pair,1); cond_br done -> exit : body
    ///   body: elem=Index(pair,0); ... ; br loophdr
    ///   exit: ret
    fn consumer() -> TirFunction {
        let mut f = TirFunction::new("consumer".into(), vec![], TirType::None);
        let guard = f.fresh_block();
        let loophdr = f.fresh_block();
        let condb = f.fresh_block();
        let body = f.fresh_block();
        let exit = f.fresh_block();

        let n5 = const_int(&mut f, 5);
        let g = f.fresh_value();
        f.value_types.insert(g, TirType::DynBox);
        let it = f.fresh_value();
        f.value_types.insert(it, TirType::DynBox);
        let nonev = f.fresh_value();
        f.value_types.insert(nonev, TirType::None);
        let isnone = f.fresh_value();
        f.value_types.insert(isnone, TirType::Bool);
        {
            let e = f.blocks.get_mut(&f.entry_block).unwrap();
            e.ops.push(op_v(OpCode::ConstInt, vec![], vec![n5], 5));
            let mut at = op(OpCode::AllocTask, vec![n5], vec![g]);
            at.attrs
                .insert("s_value".into(), AttrValue::Str("counter_poll".into()));
            at.attrs
                .insert("task_kind".into(), AttrValue::Str("generator".into()));
            at.attrs.insert("value".into(), AttrValue::Int(64));
            e.ops.push(at);
            let mut iter = op(OpCode::Copy, vec![g], vec![it]);
            iter.attrs
                .insert("_original_kind".into(), AttrValue::Str("iter".into()));
            e.ops.push(iter);
            e.ops.push(op(OpCode::ConstNone, vec![], vec![nonev]));
            e.ops.push(op(OpCode::Is, vec![it, nonev], vec![isnone]));
            e.terminator = Terminator::Branch {
                target: guard,
                args: vec![],
            };
        }
        f.blocks.insert(
            guard,
            TirBlock {
                id: guard,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: isnone,
                    then_block: exit,
                    then_args: vec![],
                    else_block: loophdr,
                    else_args: vec![],
                },
            },
        );
        f.blocks.insert(
            loophdr,
            TirBlock {
                id: loophdr,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: condb,
                    args: vec![],
                },
            },
        );
        let pair = f.fresh_value();
        f.value_types.insert(pair, TirType::DynBox);
        let one_c = const_int(&mut f, 1);
        let done = f.fresh_value();
        f.value_types.insert(done, TirType::Bool);
        f.blocks.insert(
            condb,
            TirBlock {
                id: condb,
                args: vec![],
                ops: vec![
                    op(OpCode::IterNext, vec![it], vec![pair]),
                    op_v(OpCode::ConstInt, vec![], vec![one_c], 1),
                    {
                        let mut o = op(OpCode::Index, vec![pair, one_c], vec![done]);
                        o.attrs
                            .insert("container_type".into(), AttrValue::Str("tuple".into()));
                        o
                    },
                ],
                terminator: Terminator::CondBranch {
                    cond: done,
                    then_block: exit,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        let zero_c = const_int(&mut f, 0);
        let elem = f.fresh_value();
        f.value_types.insert(elem, TirType::DynBox);
        let elem_use = f.fresh_value();
        f.value_types.insert(elem_use, TirType::DynBox);
        f.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    op_v(OpCode::ConstInt, vec![], vec![zero_c], 0),
                    {
                        let mut o = op(OpCode::Index, vec![pair, zero_c], vec![elem]);
                        o.attrs
                            .insert("container_type".into(), AttrValue::Str("tuple".into()));
                        o
                    },
                    // a trivial use of elem
                    op(OpCode::Copy, vec![elem], vec![elem_use]),
                ],
                terminator: Terminator::Branch {
                    target: loophdr,
                    args: vec![],
                },
            },
        );
        f.loop_roles
            .insert(loophdr, crate::tir::blocks::LoopRole::LoopHeader);
        f.loop_cond_blocks.insert(loophdr, condb);
        f.loop_pairs.insert(loophdr, exit);
        f.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        f
    }

    #[test]
    fn single_yield_in_loop_recognized_and_spliced() {
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![counter_poll(), consumer()],
        };
        let cg = CallGraph::build(&module);
        let tti = TargetInfo::native_release_fast();
        let stats = run_generator_fusion(&mut module, &cg, &tti);
        // Dump the consumer for inspection.
        let cons = module
            .functions
            .iter()
            .find(|f| f.name == "consumer")
            .unwrap();
        eprintln!(
            "=== fused consumer ===\n{}",
            crate::tir::printer::print_function(cons)
        );
        eprintln!("stats: {:?}", stats);
        assert_eq!(
            stats.frames_elided, 1,
            "the single-yield-in-loop generator must fuse"
        );
        // No AllocTask / StateYield / IterNext remain.
        let has = |op: OpCode| {
            cons.blocks
                .values()
                .any(|b| b.ops.iter().any(|o| o.opcode == op))
        };
        assert!(!has(OpCode::AllocTask), "AllocTask must be deleted");
        assert!(!has(OpCode::StateYield), "StateYield must be gone");
        assert!(!has(OpCode::IterNext), "IterNext must be deleted");
        crate::tir::verify::verify_function(cons).expect("fused consumer must verify");
    }
}
