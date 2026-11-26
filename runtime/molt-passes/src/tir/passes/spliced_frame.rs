//! Spliced frame states: the one builder of the operations that keep a fused
//! or inlined activation's frame observable (docs/design/foundation/
//! spliced_frames.md).
//!
//! A splice copies a callee body into its caller and drops the callee's frame
//! record. Each point of the spliced body where an observer can run gets a
//! frame state: per spliced level (outermost first) the activation's function
//! object, its `LocationId` in the callee's own location table and the value
//! of each binding there. The splice computes the bindings from its own facts
//! (the fusion slot plan, the inliner's home stores); this module turns a
//! state into runtime calls every backend already lowers. The state links one
//! record per level (`molt_spliced_frame_link`) and binds its locals
//! (`molt_spliced_frame_bind`); no call allocates or refuses a pending
//! exception, so the state can be built on an exception edge:
//!
//! * an Origin check keeps its kind (the backend captures the record-owning
//!   caller at the call site) and its exceptional successor becomes a cold
//!   block that links the state and inserts its entries beneath that caller's
//!   entry (`molt_spliced_frame_origin`, [`origin_cold_block`]);
//! * an explicit raise's Origin is followed by the same insertion
//!   ([`raise_ops`]);
//! * an operation that may run Python code is bracketed by the state's links
//!   and `molt_spliced_frame_unlink` ([`bracket_ops`]); after the caller is
//!   re-optimized, [`prune_brackets`] deletes each bracket whose operations
//!   typed facts have since proved callback-free, so a proven hot path pays
//!   nothing.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// How a level's binding keys name its bindings (the runtime's key spaces).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeySpace {
    /// A synchronous home is indexed by its code object's localsplus slot.
    CodeSlot = 0,
    /// A payload offset of a generator or coroutine binding.
    StatefulOffset = 1,
}

/// A frame home at a publication point. The value is read only when the
/// Boolean `present` is true, so a present Python None remains a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) key: i64,
    pub(crate) value: ValueId,
    pub(crate) present: ValueId,
}

/// One spliced activation at one observer point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Level {
    /// The caller value holding the activation's function object.
    pub(crate) function: ValueId,
    pub(crate) identity: ValueId,
    /// The activation's location; `None` when no single publication reaches.
    pub(crate) location: Option<i64>,
    pub(crate) key_space: KeySpace,
    /// The homes this frame state may bind.
    pub(crate) bindings: Vec<Binding>,
}

pub(crate) fn const_int(func: &mut TirFunction, value: i64, ops: &mut Vec<TirOp>) -> ValueId {
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::I64);
    let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    ops.push(op);
    result
}

/// The runtime-call form both final-TIR and SimpleIR targets lower: a `Call`
/// whose typed runtime entry its symbol names.
fn runtime_call(
    func: &mut TirFunction,
    runtime: RuntimeFn,
    operands: Vec<ValueId>,
    span: Option<(u32, u32)>,
    ops: &mut Vec<TirOp>,
) {
    let results = if matches!(
        runtime,
        RuntimeFn::SplicedFrameLink | RuntimeFn::SplicedFrameFinish
    ) {
        Vec::new()
    } else {
        let result = func.fresh_value();
        func.value_types.insert(result, TirType::DynBox);
        vec![result]
    };
    let mut op = TirOp::new(OpCode::Call, operands, results);
    op.runtime = Some(runtime);
    // The call never raises: it keeps the selected exception and abandons
    // its own failures. It is no exception event of its own.
    op.mark_nothrow_proof();
    op.attrs.insert(
        "exception_source".into(),
        AttrValue::Int(crate::tir::op_kinds_generated::ExceptionEdgeKind::Unwind as i64),
    );
    op.attrs.insert(
        "s_value".into(),
        AttrValue::Str(runtime.symbol().to_string()),
    );
    op.source_span = span;
    ops.push(op);
}

/// The operations that link one record per level of `levels` over the
/// executing frame, outermost first, each with its location and bindings;
/// and the constant that counts them.
fn link_ops(
    func: &mut TirFunction,
    levels: &[Level],
    span: Option<(u32, u32)>,
) -> (Vec<TirOp>, ValueId) {
    let mut ops = Vec::new();
    for level in levels {
        link_level(func, level, span, &mut ops);
    }
    let count = const_int(func, levels.len() as i64, &mut ops);
    (ops, count)
}

/// Prepend an enclosing level to an already authored sequence of links.
pub(crate) fn link_level(
    func: &mut TirFunction,
    level: &Level,
    span: Option<(u32, u32)>,
    ops: &mut Vec<TirOp>,
) {
    let location = const_int(func, level.location.unwrap_or(-1), ops);
    let key_space = const_int(func, level.key_space as i64, ops);
    runtime_call(
        func,
        RuntimeFn::SplicedFrameLink,
        vec![level.function, location, level.identity],
        span,
        ops,
    );
    for binding in &level.bindings {
        let key = const_int(func, binding.key, ops);
        runtime_call(
            func,
            RuntimeFn::SplicedFrameBind,
            vec![key_space, key, binding.value, binding.present],
            span,
            ops,
        );
    }
}

/// Whether `op` is one of this facility's runtime calls.
pub(crate) fn is_spliced_frame_call(op: &TirOp) -> bool {
    matches!(
        op.runtime,
        Some(
            RuntimeFn::SplicedFrameLink
                | RuntimeFn::SplicedFrameBind
                | RuntimeFn::SplicedFrameOrigin
                | RuntimeFn::SplicedFrameUnlink
                | RuntimeFn::SplicedFrameFinish
        )
    )
}

/// A fresh cold block, to be the exceptional successor of an Origin check
/// inside a spliced body, that links the records of `levels`, inserts their
/// entries beneath the caller's entry and pops them, and then branches to
/// `target` with `args`: the caller's handler of the call site and what
/// the call site's check passes it. The caller inserts the block under a
/// fresh label and points the check at that label.
pub(crate) fn origin_cold_block(
    func: &mut TirFunction,
    levels: &[Level],
    target: BlockId,
    args: Vec<ValueId>,
    span: Option<(u32, u32)>,
) -> TirBlock {
    let cold = func.fresh_block();
    let ops = raise_ops(func, None, levels, span);
    TirBlock {
        id: cold,
        args: Vec::new(),
        ops,
        terminator: Terminator::Branch { target, args },
    }
}

/// A fresh cold block, to be the exceptional successor of a bracketed
/// operation that leaves with the bracket's `count` records still linked (an
/// asynchronous-work poll whose handler raised): the backend's capture found a
/// spliced record executing and captured nothing, so the runtime captures the
/// caller with the records, pops them and the block branches to `target`
/// with `args`.
/// With `capture` false (an Unwind poll, whose pending exception an earlier
/// Origin already recorded) the block only pops the records.
pub(crate) fn linked_origin_block(
    func: &mut TirFunction,
    count: i64,
    capture: bool,
    target: BlockId,
    args: Vec<ValueId>,
    span: Option<(u32, u32)>,
) -> TirBlock {
    let cold = func.fresh_block();
    let mut ops = Vec::new();
    if !capture {
        let count = const_int(func, count, &mut ops);
        runtime_call(
            func,
            RuntimeFn::SplicedFrameUnlink,
            vec![count],
            span,
            &mut ops,
        );
        return TirBlock {
            id: cold,
            args: Vec::new(),
            ops,
            terminator: Terminator::Branch { target, args },
        };
    }
    let none = func.fresh_value();
    func.value_types.insert(none, TirType::None);
    ops.push(TirOp::new(OpCode::ConstNone, vec![], vec![none]));
    let count = const_int(func, count, &mut ops);
    runtime_call(
        func,
        RuntimeFn::SplicedFrameOrigin,
        vec![none, count],
        span,
        &mut ops,
    );
    TirBlock {
        id: cold,
        args: Vec::new(),
        ops,
        terminator: Terminator::Branch { target, args },
    }
}

/// The operations that link the records of `levels`, insert their entries
/// beneath the entry the record-owning caller's Origin captured and pop them.
/// `exception` is an explicit raise's exception, whose Origin
/// (`molt_exception_trace_prepend`) they follow, or `None` for the pending
/// exception of an Origin edge.
pub(crate) fn raise_ops(
    func: &mut TirFunction,
    exception: Option<ValueId>,
    levels: &[Level],
    span: Option<(u32, u32)>,
) -> Vec<TirOp> {
    let (mut ops, count) = link_ops(func, levels, span);
    let exception = exception.unwrap_or_else(|| {
        let none = func.fresh_value();
        func.value_types.insert(none, TirType::None);
        ops.push(TirOp::new(OpCode::ConstNone, vec![], vec![none]));
        none
    });
    runtime_call(
        func,
        RuntimeFn::SplicedFrameOrigin,
        vec![exception, count],
        span,
        &mut ops,
    );
    ops
}

/// The operations that link the records of `levels` before an operation that
/// may run Python code, and those that pop them after it.
pub(crate) fn bracket_ops(
    func: &mut TirFunction,
    levels: &[Level],
    span: Option<(u32, u32)>,
) -> (Vec<TirOp>, Vec<TirOp>) {
    let (enter, count) = link_ops(func, levels, span);
    let mut exit = Vec::new();
    runtime_call(
        func,
        RuntimeFn::SplicedFrameUnlink,
        vec![count],
        span,
        &mut exit,
    );
    (enter, exit)
}

pub(crate) fn finish_op(func: &mut TirFunction, identity: ValueId) -> TirOp {
    let mut ops = Vec::with_capacity(1);
    runtime_call(
        func,
        RuntimeFn::SplicedFrameFinish,
        vec![identity],
        None,
        &mut ops,
    );
    ops.pop().unwrap()
}

/// Whether a callback or poll may first expose a frame from the current splice.
pub(crate) fn has_observer_bracket(func: &TirFunction, first_new_value: u32) -> bool {
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    func.blocks.values().any(|block| {
        let mut linked = false;
        for op in &block.ops {
            match op.runtime {
                Some(RuntimeFn::SplicedFrameLink) => {
                    linked |= op.operands[2].0 >= first_new_value;
                }
                Some(RuntimeFn::SplicedFrameUnlink | RuntimeFn::SplicedFrameOrigin) => {
                    linked = false;
                }
                _ if linked && may_observe_frames(op, &exact) => return true,
                _ => {}
            }
        }
        false
    })
}

/// Whether Python code that can observe the executing frame may run at `op`,
/// whose operand facts are `exact`: the operations a splice brackets
/// (spliced_frames.md 4.4). A check, a raise and an explicit raise's Origin
/// are not: an exception takes the Origin edge (4.2, 4.3); this facility's
/// own calls are not either.
pub(crate) fn may_observe_frames(op: &TirOp, exact: &HashMap<ValueId, TirType>) -> bool {
    if op.opcode == OpCode::CheckException {
        // A poll services asynchronous work: a signal handler runs there.
        return op.is_async_work_poll();
    }
    if op.opcode == OpCode::Raise
        || is_spliced_frame_call(op)
        || op.runtime == Some(RuntimeFn::ExceptionTracePrepend)
    {
        return false;
    }
    super::frame_elision::op_reads_frame(op)
        || super::effects::op_effects_with_types(op, exact).may_call_python
}

fn is_link_op(op: &TirOp) -> bool {
    matches!(
        op.runtime,
        Some(RuntimeFn::SplicedFrameLink | RuntimeFn::SplicedFrameBind)
    ) || op.opcode == OpCode::ConstInt
}

/// A pre-existing async-work poll in its published Python frame at a CFG
/// boundary. Splicing and checked-loop versioning can put the back edge on
/// either side of this bracket; the check still runs at its original source
/// and exceptional successor.
fn poll_and_unlink(ops: &[TirOp]) -> bool {
    matches!(ops, [poll, unlink, ..] if poll.opcode == OpCode::CheckException
        && poll.is_async_work_poll()
        && matches!(poll.attrs.get("value"), Some(AttrValue::Int(_)))
        && unlink.runtime == Some(RuntimeFn::SplicedFrameUnlink))
}

/// The complete cold service arm built by generator frame elision. Nothing
/// else may run between the pending-work test and the delivery of that work.
pub(super) fn poll_service(ops: &[TirOp]) -> Option<&TirOp> {
    // A deferred slot-read credit may cover the entire publication. Retaining
    // the already-held slot cannot call Python, and its matching retirement
    // follows the original service and unlink.
    let ops = match ops {
        [retain, bracket @ .., close, retire]
            if retain.opcode == OpCode::Copy
                && retain.origin == Some(molt_ir::OpKind::BindingAlias)
                && retain.operands.len() == 1
                && retain.results.len() == 1
                && retire.opcode == OpCode::DecRef
                && close.opcode == OpCode::Copy
                && close.origin == Some(molt_ir::OpKind::BindingAlias)
                && close.operands == retain.results
                && close.results.len() == 1
                && retire.operands == close.results
                && retire.results.is_empty()
                && bracket.iter().any(|op| {
                    op.runtime == Some(RuntimeFn::SplicedFrameBind)
                        && op.operands.get(2) == retain.operands.first()
                }) =>
        {
            bracket
        }
        _ => ops,
    };
    let [setup @ .., poll, _unlink] = ops else {
        return None;
    };
    let mut calls = setup.iter().filter(|op| op.opcode != OpCode::ConstInt);
    let link = calls.next()?;
    (link.runtime == Some(RuntimeFn::SplicedFrameLink)
        && calls.all(|op| op.runtime == Some(RuntimeFn::SplicedFrameBind))
        && poll.exception_edge_kind()
            == Some(crate::tir::op_kinds_generated::ExceptionEdgeKind::Unwind)
        && poll_and_unlink(&ops[setup.len()..]))
    .then_some(poll)
}

pub(super) fn ends_with_poll_bracket(ops: &[TirOp]) -> bool {
    let Some(check) = ops.len().checked_sub(2) else {
        return false;
    };
    poll_and_unlink(&ops[check..])
        && ops[..check]
            .iter()
            .rev()
            .take_while(|op| is_link_op(op))
            .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
}

pub(super) fn starts_with_poll_bracket(ops: &[TirOp]) -> bool {
    // A joined continuation may close the preceding operation's frame and
    // bind its result into SSA before publishing the poll's frame.
    let ops = match ops {
        [close, rest @ ..] if close.runtime == Some(RuntimeFn::SplicedFrameUnlink) => rest,
        _ => ops,
    };
    let aliases = ops
        .iter()
        .take_while(|op| {
            op.opcode == OpCode::Copy && op.origin == Some(molt_ir::OpKind::BindingAlias)
        })
        .count();
    let ops = &ops[aliases..];
    let links = ops.iter().take_while(|op| is_link_op(op)).count();
    ops[..links]
        .iter()
        .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
        && poll_and_unlink(&ops[links..])
}

/// Delete each bracket whose operations can no longer run an observer, and
/// the state it linked; return how many. A bracket is a run of link and bind
/// calls (with their constants), the operations it covers, and the unlink
/// call that closes it in the same block. A re-optimized caller's facts often
/// prove the covered operation callback-free (an integer multiply once its
/// operand is the loop counter).
pub(crate) fn prune_brackets(func: &mut TirFunction) -> usize {
    let plan = crate::tir::type_refine::ExactScalarPlan::new(func);
    let native_steps = super::native_iterator::callback_free_steps(func, &plan);
    let exact = crate::tir::type_refine::exact_scalar_map_from_plan(plan);
    let snapshots: HashMap<_, _> = func
        .blocks
        .iter()
        .filter_map(|(&id, block)| {
            let Terminator::CondBranch {
                cond, then_block, ..
            } = block.terminator
            else {
                return None;
            };
            let observed = block
                .ops
                .iter()
                .find(|op| op.opcode == OpCode::SplicedFrameObserved && op.results == [cond])?;
            Some((then_block, (id, observed.operands[0])))
        })
        .collect();
    let mut pruned = 0;
    let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for block in blocks {
        if snapshots.contains_key(&block) {
            continue;
        }
        let ops = &func.blocks[&block].ops;
        let mut dead: Vec<usize> = Vec::new();
        let mut index = 0;
        while index < ops.len() {
            if ops[index].runtime != Some(RuntimeFn::SplicedFrameLink) {
                index += 1;
                continue;
            }
            // The run of links, binds and their constants around this link.
            let mut start = index;
            while start > 0 && is_link_op(&ops[start - 1]) {
                start -= 1;
            }
            let mut end = index;
            while end < ops.len() && is_link_op(&ops[end]) {
                end += 1;
            }
            let Some(close) =
                (end..ops.len()).find(|&at| ops[at].runtime == Some(RuntimeFn::SplicedFrameUnlink))
            else {
                break;
            };
            let covered = &ops[end..close];
            // A check inside a live bracket can branch to a cold Origin that
            // consumes its still-linked records, even if the covered operation
            // later proves callback-free. Removing its links alone would pop
            // the physical caller on that edge.
            let bracketed = !covered
                .iter()
                .any(|op| is_spliced_frame_call(op) || op.opcode == OpCode::CheckException);
            if bracketed
                && !covered.iter().any(|op| {
                    may_observe_frames(op, &exact)
                        && !(op.opcode == OpCode::IterNextUnboxed
                            && op
                                .results
                                .first()
                                .is_some_and(|item| native_steps.contains(item)))
                })
            {
                dead.extend(start..end);
                dead.push(close);
                pruned += 1;
            }
            index = close + 1;
        }
        if dead.is_empty() {
            continue;
        }
        // A constant of the run that any other operation reads stays.
        let dead_set: molt_ir::deterministic_hash::HashSet<usize> = dead.iter().copied().collect();
        let mut live_reads: molt_ir::deterministic_hash::HashSet<ValueId> = Default::default();
        for (&id, body) in &func.blocks {
            for (at, op) in body.ops.iter().enumerate() {
                if id != block || !dead_set.contains(&at) {
                    live_reads.extend(op.operands.iter().copied());
                }
            }
            body.terminator.for_each_value(|value| {
                live_reads.insert(value);
            });
        }
        let block_ops = &mut func.blocks.get_mut(&block).expect("block").ops;
        let mut at = 0usize;
        block_ops.retain(|op| {
            let remove = dead_set.contains(&at)
                && !(op.opcode == OpCode::ConstInt
                    && op.results.iter().any(|result| live_reads.contains(result)));
            at += 1;
            !remove
        });
    }
    let active: HashSet<_> = func
        .blocks
        .iter()
        .filter(|(id, _)| !snapshots.contains_key(id))
        .flat_map(|(_, block)| &block.ops)
        .filter(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
        .map(|op| op.operands[2])
        .collect();
    for (_, (source, identity)) in snapshots {
        if active.contains(&identity) {
            continue;
        }
        let block = func.blocks.get_mut(&source).unwrap();
        let Terminator::CondBranch {
            else_block,
            else_args,
            ..
        } = &mut block.terminator
        else {
            unreachable!("snapshot source is conditional");
        };
        block.terminator = Terminator::Branch {
            target: *else_block,
            args: std::mem::take(else_args),
        };
        pruned += 1;
    }
    // The guards and releases borrow a place only the remaining links can
    // populate. A callback-free splice has no identity instructions at all.
    for block in func.blocks.values_mut() {
        block.ops.retain(|op| {
            if op.opcode == OpCode::SplicedFrameSlot {
                return active.contains(&op.results[0]);
            }
            op.spliced_frame_identity_operand()
                .is_none_or(|index| active.contains(&op.operands[index]))
        });
    }
    if pruned != 0 {
        super::native_iterator::project_successful_items(func, &native_steps);
        let retained = super::reachability::metadata_preserving_reachable_blocks(func);
        func.retain_blocks(&retained)
            .expect("pruning splice snapshots preserves live edges");
    }
    pruned
}

/// Refresh escaped frames at a suspension or return. The common unobserved
/// path proceeds directly to the continuation.
pub(crate) fn publish_if_observed(caller: &mut TirFunction, source: BlockId, frame: &Level) {
    let ops = snapshot_ops(caller, frame);
    if_frame_observed(caller, source, frame.identity, ops);
}

/// Retire an observed inner frame under its virtual callers. Both the final
/// snapshot's back chain and cleanup that can run Python use that chain.
pub(crate) fn retire_if_observed_under(
    caller: &mut TirFunction,
    source: BlockId,
    identity: ValueId,
    frame: Option<&Level>,
    callers: &[Level],
) {
    let (mut ops, unlink) = if callers.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        bracket_ops(caller, callers, None)
    };
    if let Some(frame) = frame {
        ops.extend(snapshot_ops(caller, frame));
    }
    ops.push(finish_op(caller, identity));
    ops.extend(unlink);
    if_frame_observed(caller, source, identity, ops);
}

fn snapshot_ops(caller: &mut TirFunction, frame: &Level) -> Vec<TirOp> {
    let (mut ops, unlink) = bracket_ops(caller, std::slice::from_ref(frame), None);
    ops.extend(unlink);
    ops
}

fn if_frame_observed(
    caller: &mut TirFunction,
    source: BlockId,
    identity: ValueId,
    ops: Vec<TirOp>,
) {
    let Terminator::Branch { target, args } = caller.blocks[&source].terminator.clone() else {
        unreachable!("a frame publication continues along one edge");
    };
    let snapshot = caller.fresh_block();
    caller.blocks.insert(
        snapshot,
        TirBlock {
            id: snapshot,
            args: Vec::new(),
            ops,
            terminator: Terminator::Branch {
                target,
                args: args.clone(),
            },
        },
    );
    let observed = caller.fresh_value();
    caller.value_types.insert(observed, TirType::Bool);
    let block = caller.blocks.get_mut(&source).unwrap();
    block.ops.push(TirOp::new(
        OpCode::SplicedFrameObserved,
        vec![identity],
        vec![observed],
    ));
    block.terminator = Terminator::CondBranch {
        cond: observed,
        then_block: snapshot,
        then_args: Vec::new(),
        else_block: target,
        else_args: args,
    };
}
