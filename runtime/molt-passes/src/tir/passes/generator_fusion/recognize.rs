use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::super::super::blocks::{BlockId, Terminator};
use super::super::super::call_graph::CallGraph;
use super::super::super::function::TirFunction;
use super::super::super::op_kinds_generated::{
    ExceptionEdgeKind, GeneratorFusionIterUseRole, opcode_generator_fusion_iter_use_role_table,
    opcode_generator_fusion_poll_role_table,
};
use super::super::super::ops::OpCode;
use super::super::super::values::ValueId;
use super::{FusionCandidate, attr_task_kind, is_get_iter_op, poll_callee};

/// Whether `poll` is a generator `_poll` body that may be fused (Phase 1).
///
/// Conservative-correct exclusions — any one keeps the generator at Tier D:
/// * not a generator at all (no `StateYield`).
/// * `YieldFrom` (delegation; cannot be linearized in Phase 1).
/// * `StateBlockStart`/`StateBlockEnd` (async generator state region) or a real
///   `try`/`except` HANDLER ([`has_exception_handlers`](TirFunction::has_exception_handlers)).
/// * on a static call cycle (a self-edge or a multi-node SCC): an unbounded
///   splice. An opaque call is no cycle: the splice copies the body once.
/// * the entry block has a predecessor — the splice assumes the entry is the
///   single linear start (no branch targets it).
///
/// Returns the first exclusion `poll` meets, or None when it may be fused.
pub(in crate::tir::passes::generator_fusion) fn poll_fusion_refusal(
    poll_id: molt_ir::FunctionId,
    poll: &TirFunction,
    call_graph: &CallGraph,
) -> Option<PollRefusal> {
    let mut has_yield = false;
    let mut rejected = None;
    let mut frame_writer = None;
    for block in poll.blocks.values() {
        for op in &block.ops {
            let role = opcode_generator_fusion_poll_role_table(op.opcode);
            if role.rejects_fusion() && rejected.is_none() {
                rejected = Some(op.opcode);
            }
            if role.is_required_yield() {
                has_yield = true;
            }
            frame_writer = frame_writer.or_else(|| foreign_frame_access(op));
        }
    }
    if !has_yield {
        return Some(PollRefusal::NotAGenerator);
    }
    if let Some(opcode) = rejected {
        return Some(PollRefusal::Op(opcode));
    }
    if let Some(refusal) = frame_writer {
        return Some(refusal);
    }
    if poll.has_exception_handlers() {
        return Some(PollRefusal::ExceptionHandler);
    }
    if call_graph.in_static_cycle(poll_id) {
        return Some(PollRefusal::Recursive);
    }
    if entry_has_predecessor(poll) {
        return Some(PollRefusal::EntryHasPredecessor);
    }
    None
}

/// The exclusion that keeps a generator poll body from fusion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollRefusal {
    /// No `StateYield`: not a generator body.
    NotAGenerator,
    /// An op whose fusion role rejects fusion (`op_kinds.toml`).
    Op(OpCode),
    /// A real `try`/`except` handler.
    ExceptionHandler,
    /// The poll lies on a static call cycle.
    Recursive,
    /// A branch targets the entry block.
    EntryHasPredecessor,
    /// A runtime call writes a frame slot behind the slot plan's back: a
    /// prologue cell publication (`molt_frame_cell_publish`).
    CellPublication,
    /// A yield or the state dispatch does not take the shape fusion
    /// linearizes: a yield ending its block, each case resumed by a yield.
    ResumeShape,
    /// A callback or signal poll can expose a frame whose observer may
    /// rebind its locals or close the suspended generator.
    MutableFrame,
    /// A new backedge in the merged body lacks a certified async-work observation.
    MissingObservation,
    /// The body already holds a spliced frame state (an inlined observable
    /// callee); fusing it would nest frame states.
    NestedSplice,
}

/// The refusal an operation of a poll body earns because it touches the
/// frame outside the slot accesses the plan promotes, or holds a frame state.
fn foreign_frame_access(op: &crate::tir::ops::TirOp) -> Option<PollRefusal> {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    if super::super::spliced_frame::is_spliced_frame_call(op) {
        return Some(PollRefusal::NestedSplice);
    }
    (op.runtime == Some(RuntimeFn::FrameCellPublish)).then_some(PollRefusal::CellPublication)
}

impl std::fmt::Display for PollRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAGenerator => f.write_str("not a generator body"),
            Self::Op(opcode) => write!(f, "op {opcode:?} rejects fusion"),
            Self::ExceptionHandler => f.write_str("a try/except handler"),
            Self::Recursive => f.write_str("recursive"),
            Self::EntryHasPredecessor => f.write_str("a branch targets the entry block"),
            Self::CellPublication => f.write_str("a prologue publishes a cell into a frame slot"),
            Self::ResumeShape => f.write_str("a yield or the state dispatch has no linear resume"),
            Self::MutableFrame => {
                f.write_str("an observer can rebind frame locals or close the suspended generator")
            }
            Self::MissingObservation => {
                f.write_str("a fused boundary lacks an async-work observation")
            }
            Self::NestedSplice => f.write_str("the body holds a spliced frame state"),
        }
    }
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
        Terminator::Return { .. } | Terminator::Unwind | Terminator::Unreachable => false,
    })
}

/// Build a use-count map over the whole function (ops + terminators), so a
/// "single use" recognition test is exact.
fn build_use_counts(func: &TirFunction) -> HashMap<ValueId, usize> {
    let mut counts: HashMap<ValueId, usize> = HashMap::default();
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
            Terminator::Unwind | Terminator::Unreachable => {}
        }
    }
    counts
}

/// Find the first eligible allocation that has not already declined its splice.
/// Recompute after each splice, in deterministic order:
/// blocks sorted by id, ops in index order.
///
/// Bail table (each leaves the IR unchanged):
/// * `AllocTask` is not a `generator` (future/coroutine) — out of scope.
/// * the poll body is not in `poll_bodies` (not fusable, or external).
/// * the frame value has any use other than the single `GetIter` (a `.send`/
///   `.throw`/`.close` method call, an escape into a container, a store).
/// * the `GetIter` result has any use other than the single `IterNextUnboxed`
///   and the not-iterable guard.
/// * the `IterNextUnboxed` is not followed by its iteration's Origin check, or
///   its done result does not drive the loop's `CondBranch` (through empty
///   blocks only).
pub(in crate::tir::passes::generator_fusion) fn next_fusion_candidate(
    caller: &TirFunction,
    poll_bodies: &HashMap<molt_ir::FunctionId, TirFunction>,
    declined: &HashSet<ValueId>,
) -> Option<FusionCandidate> {
    let use_counts = build_use_counts(caller);

    // An Is use is an identity observation unless its other value is None.
    // The splice replaces each non-escaping iterator with the same sentinel.
    let none_values: HashSet<_> = caller
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::ConstNone)
        .flat_map(|op| op.results.iter().copied())
        .collect();

    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &alloc_block in &block_ids {
        let block = &caller.blocks[&alloc_block];
        for (alloc_idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::AllocTask || attr_task_kind(op) != Some("generator") {
                continue;
            }
            let Some(poll) = poll_callee(op) else {
                continue;
            };
            if !poll_bodies.contains_key(&poll) {
                continue;
            }
            let Some(&alloc_val) = op.results.first() else {
                continue;
            };
            if declined.contains(&alloc_val) {
                continue;
            }
            let (&function, _) = crate::tir::ops::alloc_task_operands(&op.operands);

            // The frame value must have exactly one use: the GetIter.
            if use_counts.get(&alloc_val).copied().unwrap_or(0) != 1 {
                continue;
            }
            let Some((get_iter_block, iter_val)) = find_single_get_iter_use(caller, alloc_val)
            else {
                continue;
            };

            // The iterator value's uses must be exactly: the `IterNextUnboxed`,
            // plus (optionally) the consumer's `is(iter, None)` not-iterable
            // guard (the frontend emits `if iter is None: raise TypeError`
            // around `for x in <expr>`; that `Is` use is benign — fusion
            // replaces the iterator with a non-None sentinel so the guard
            // folds False).
            if !iter_uses_are_next_and_optional_none_guard(caller, iter_val, &none_values) {
                continue;
            }
            let Some(next) = recognize_iteration(caller, iter_val) else {
                continue;
            };
            // The done flag drives the loop alone: it retires with the test.
            if use_counts.get(&next.done).copied() != Some(1) {
                continue;
            }
            let Some(handler) = caller
                .label_id_map
                .iter()
                .find_map(|(&block, &label)| (label == next.check_label).then_some(BlockId(block)))
            else {
                continue;
            };
            let call_location = super::super::frame_location_placement::reaching_locations(caller)
                .get(&(next.block, next.index))
                .copied();

            let loop_header = caller.loop_pairs.keys().copied().find(|h| {
                caller
                    .loop_cond_blocks
                    .get(h)
                    .is_some_and(|cond| *cond == next.block || next.chain.contains(cond))
            });

            return Some(FusionCandidate {
                alloc_block,
                alloc_idx,
                alloc_val,
                function,
                poll,
                get_iter_block,
                iter_val,
                cond_block: next.block,
                elem_val: next.elem,
                chain: next.chain,
                handler,
                handler_args: next.check_args.clone(),
                call_location,
                exit_block: next.exit_block,
                body_block: next.body_block,
                loop_header,
            });
        }
    }

    None
}

/// Find the single `GetIter`/`iter` use of `frame_val`, returning
/// `(block, iter_val)`.
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
/// `IterNextUnboxed`, an `Is(iter, None)` not-iterable guard, or a
/// `GetIter`/`iter` op that produced it. Any other use (an escape, a
/// `.send`/`.throw`/`.close` method dispatch, a store) disqualifies fusion.
fn iter_uses_are_next_and_optional_none_guard(
    caller: &TirFunction,
    iter_val: ValueId,
    none_values: &HashSet<ValueId>,
) -> bool {
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
                GeneratorFusionIterUseRole::NextUse
                    if op.operands.first() == Some(&iter_val) && !saw_next =>
                {
                    saw_next = true
                }
                GeneratorFusionIterUseRole::NoneGuard
                    if matches!(op.operands.as_slice(), [a, b]
                        if (*a == iter_val && none_values.contains(b))
                            || (*b == iter_val && none_values.contains(a))) => {}
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
pub(super) fn terminator_uses(term: &Terminator, v: ValueId) -> bool {
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
        Terminator::Unwind | Terminator::Unreachable => false,
    }
}

/// The consumer's iteration over the generator: one `IterNextUnboxed` step.
struct Iteration {
    /// The block holding the step and its index there.
    block: BlockId,
    index: usize,
    /// The element and done flag the step produces.
    elem: ValueId,
    done: ValueId,
    /// The label of the step's Origin check.
    check_label: i64,
    /// The values the check passes its handler.
    check_args: Vec<ValueId>,
    /// The empty blocks between the step's block and the done test.
    chain: Vec<BlockId>,
    /// The `done == true` (loop exit) and `done == false` (loop body) targets.
    exit_block: BlockId,
    body_block: BlockId,
}

/// Recognize the consumer's iteration over `iter_val`:
///
/// ```text
///   %elem, %done = iter_next_unboxed %it     ; the only ops are the step,
///   check_exception (Origin) -> handler       ; its check and location
///   br chain...                               ; publications
///   cond_br %done, exit, body                 ; in an empty chain block
/// ```
fn recognize_iteration(caller: &TirFunction, iter_val: ValueId) -> Option<Iteration> {
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    let (block, index) = block_ids.iter().find_map(|&bid| {
        caller.blocks[&bid]
            .ops
            .iter()
            .position(|op| {
                opcode_generator_fusion_iter_use_role_table(op.opcode)
                    == GeneratorFusionIterUseRole::NextUse
                    && op.operands.first() == Some(&iter_val)
            })
            .map(|index| (bid, index))
    })?;
    let ops = &caller.blocks[&block].ops;
    let step = &ops[index];
    let [elem, done] = step.results[..] else {
        return None;
    };
    // Every other op of the step's block is a location publication, before
    // the step, or the step's Origin check right after it.
    let check = ops.get(index + 1)?;
    if check.opcode != OpCode::CheckException
        || check.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
        || ops.len() != index + 2
        || ops[..index]
            .iter()
            .any(|op| op.opcode != OpCode::FrameSetLocation)
    {
        return None;
    }
    let check_label = crate::tir::clone_support::exception_label_of(check)?;
    // Follow argument-less branches through empty blocks to the done test.
    let mut chain = Vec::new();
    let mut current = block;
    loop {
        match &caller.blocks[&current].terminator {
            Terminator::Branch { target, args } if args.is_empty() => {
                let next = &caller.blocks.get(target)?;
                if !next.ops.is_empty() || !next.args.is_empty() || chain.contains(target) {
                    return None;
                }
                chain.push(*target);
                current = *target;
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } if *cond == done && then_args.is_empty() && else_args.is_empty() => {
                return Some(Iteration {
                    block,
                    index,
                    elem,
                    done,
                    check_label,
                    check_args: check.operands.clone(),
                    chain,
                    exit_block: *then_block,
                    body_block: *else_block,
                });
            }
            // A branch with arguments, a test of another value, and every
            // other terminator end the chain before the done test.
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Return { .. }
            | Terminator::Unwind
            | Terminator::Unreachable => return None,
        }
    }
}
