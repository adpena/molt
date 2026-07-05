use std::collections::HashMap;

use super::super::super::blocks::{BlockId, Terminator};
use super::super::super::call_graph::CallGraph;
use super::super::super::function::TirFunction;
use super::super::super::op_kinds_generated::{
    GeneratorFusionIterUseRole, opcode_generator_fusion_iter_use_role_table,
    opcode_generator_fusion_poll_role_table,
};
use super::super::super::ops::OpCode;
use super::super::super::values::ValueId;
use super::{FusionCandidate, attr_s_value, attr_task_kind, attr_value_int, is_get_iter_op};

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
pub(in crate::tir::passes::generator_fusion) fn is_poll_fusable(
    poll: &TirFunction,
    call_graph: &CallGraph,
) -> bool {
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
pub(in crate::tir::passes::generator_fusion) fn collect_fusion_candidates(
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
