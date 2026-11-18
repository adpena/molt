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

use molt_ir::deterministic_hash::HashMap;
use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// How a level's binding keys name its bindings (the runtime's key spaces).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeySpace {
    /// A payload offset of a generator or coroutine binding. The runtime's
    /// key space 0 names a synchronous frame's code slot.
    StatefulOffset = 1,
}

/// One spliced activation at one observer point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Level {
    /// The caller value holding the activation's function object.
    pub(crate) function: ValueId,
    /// The activation's location; `None` when no single publication reaches.
    pub(crate) location: Option<i64>,
    pub(crate) key_space: KeySpace,
    /// Each binding that holds a value at the point: key and value.
    pub(crate) bindings: Vec<(i64, ValueId)>,
}

fn const_int(func: &mut TirFunction, value: i64, ops: &mut Vec<TirOp>) -> ValueId {
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
) -> ValueId {
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let mut op = TirOp::new(OpCode::Call, operands, vec![result]);
    op.runtime = Some(runtime);
    op.attrs.insert(
        "s_value".into(),
        AttrValue::Str(runtime.symbol().to_string()),
    );
    op.source_span = span;
    ops.push(op);
    result
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
        let location = const_int(func, level.location.unwrap_or(-1), &mut ops);
        let key_space = const_int(func, level.key_space as i64, &mut ops);
        runtime_call(
            func,
            RuntimeFn::SplicedFrameLink,
            vec![level.function, location, key_space],
            span,
            &mut ops,
        );
        for &(key, value) in &level.bindings {
            let key = const_int(func, key, &mut ops);
            runtime_call(
                func,
                RuntimeFn::SplicedFrameBind,
                vec![key_space, key, value],
                span,
                &mut ops,
            );
        }
    }
    let count = const_int(func, levels.len() as i64, &mut ops);
    (ops, count)
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

/// Whether Python code that can observe the executing frame may run at `op`,
/// whose operand facts are `exact`: the operations a splice brackets
/// (spliced_frames.md 4.4). A check, a raise and an explicit raise's Origin
/// are not: an exception takes the Origin edge (4.2, 4.3); this facility's
/// own calls are not either.
pub(crate) fn may_observe_frames(op: &TirOp, exact: &HashMap<ValueId, TirType>) -> bool {
    if matches!(op.opcode, OpCode::CheckException | OpCode::Raise)
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

/// Delete each bracket whose operations can no longer run an observer, and
/// the state it linked; return how many. A bracket is a run of link and bind
/// calls (with their constants), the operations it covers, and the unlink
/// call that closes it in the same block. A re-optimized caller's facts often
/// prove the covered operation callback-free (an integer multiply once its
/// operand is the loop counter).
pub(crate) fn prune_brackets(func: &mut TirFunction) -> usize {
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    let mut pruned = 0;
    let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for block in blocks {
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
            let bracketed = !covered.iter().any(is_spliced_frame_call);
            if bracketed && !covered.iter().any(|op| may_observe_frames(op, &exact)) {
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
    pruned
}
