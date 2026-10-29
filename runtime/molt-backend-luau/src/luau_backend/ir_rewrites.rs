use crate::OpIR;
use std::collections::BTreeSet;

#[inline]
fn is_exception_observer_kind(kind: molt_ir::OpKind) -> bool {
    kind.is_exception_check()
}

pub(super) fn observes_pending_exception(ops: &[OpIR]) -> bool {
    // Without a local pending-state observer, host errors can unwind directly
    // to the caller's capture. Such functions need no extra operation scopes.
    ops.iter().any(|op| {
        is_exception_observer_kind(op.kind)
            || matches!(
                op.kind,
                molt_ir::OpKind::TryStart
                    | molt_ir::OpKind::LoopBreakIfException
                    | molt_ir::OpKind::ExceptionPending
                    | molt_ir::OpKind::ExceptionCurrent
                    | molt_ir::OpKind::ExceptionLast
                    | molt_ir::OpKind::ExceptionLastPending
                    | molt_ir::OpKind::ExceptionFinallyPendingObserver
            )
    })
}

/// Capture host throws one executable operation at a time, never across a
/// lexical control boundary. Explicit state/CFG operations still own routing;
/// outputs are declared just outside their capture in the original scope.
pub(super) fn lower_exception_captures(ops: &[OpIR]) -> Vec<OpIR> {
    lower_exception_captures_with_counter(ops, observes_pending_exception(ops), &mut 0)
}

pub(super) fn lower_exception_captures_with_counter(
    ops: &[OpIR],
    observes_pending: bool,
    counter: &mut i64,
) -> Vec<OpIR> {
    use molt_ir::tir::op_kinds_generated::{
        copy_kind_is_inert_marker_table, kind_to_opcode_table, opcode_may_throw_table,
    };
    use molt_ir::tir::simple_def_use::visit_simple_ir_defined_names;

    let mut result = Vec::with_capacity(ops.len());
    for op in ops {
        let kind = op.kind;
        if matches!(kind, molt_ir::OpKind::TryStart | molt_ir::OpKind::TryEnd) {
            continue;
        }
        // This target projects pending/handled state directly, without host
        // throws. Constructors and match/value/cause operations remain subject
        // to the shared may-throw oracle. The only target-local control kinds
        // are the synthetic loops and captures introduced by these rewrites.
        let protocol_or_control = matches!(
            kind,
            molt_ir::OpKind::ExceptionPush
                | molt_ir::OpKind::ExceptionPop
                | molt_ir::OpKind::ExceptionStackEnter
                | molt_ir::OpKind::ExceptionStackExit
                | molt_ir::OpKind::ExceptionStackDepth
                | molt_ir::OpKind::ExceptionStackSetDepth
                | molt_ir::OpKind::ExceptionStackClear
                | molt_ir::OpKind::ExceptionContextSet
                | molt_ir::OpKind::ExceptionClear
                | molt_ir::OpKind::ExceptionSetLast
                | molt_ir::OpKind::ExceptionLast
                | molt_ir::OpKind::ExceptionLastPending
                | molt_ir::OpKind::ExceptionFinallyPendingObserver
                | molt_ir::OpKind::ExceptionActive
                | molt_ir::OpKind::ExceptionCurrent
                | molt_ir::OpKind::ExceptionPending
                | molt_ir::OpKind::Raise
                | molt_ir::OpKind::ForRange
                | molt_ir::OpKind::ForIter
                | molt_ir::OpKind::EndFor
                | molt_ir::OpKind::PcallWrapBegin
                | molt_ir::OpKind::PcallWrapEnd
        ) || is_exception_observer_kind(kind)
            || kind.is_cfg_or_ssa_consumed()
            || kind.is_suspend()
            || copy_kind_is_inert_marker_table(kind);
        let may_throw = observes_pending
            && !protocol_or_control
            && kind_to_opcode_table(kind).is_none_or(opcode_may_throw_table);
        if may_throw {
            let mut outputs = Vec::new();
            visit_simple_ir_defined_names(op, |name| {
                if name != "none" {
                    outputs.push(name.to_string());
                }
            });
            result.push(OpIR {
                args: Some(outputs),
                value: Some(*counter),
                ..OpIR::new(molt_ir::OpKind::PcallWrapBegin)
            });
        }
        result.push(op.clone());
        if may_throw {
            result.push(OpIR {
                value: Some(*counter),
                ..OpIR::new(molt_ir::OpKind::PcallWrapEnd)
            });
            *counter += 1;
        }
    }
    result
}

pub(super) fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None; // depth at which we became dead
    let referenced_labels: BTreeSet<i64> = ops
        .iter()
        .filter_map(|op| {
            matches!(
                op.kind,
                molt_ir::OpKind::Jump
                    | molt_ir::OpKind::Goto
                    | molt_ir::OpKind::BrIf
                    | molt_ir::OpKind::Branch
                    | molt_ir::OpKind::BranchFalse
                    | molt_ir::OpKind::CheckException
                    | molt_ir::OpKind::AsyncWorkPoll
                    | molt_ir::OpKind::TryStart
                    | molt_ir::OpKind::StateBlockStart
                    | molt_ir::OpKind::LoopBreakIfTrue
                    | molt_ir::OpKind::LoopBreakIfFalse
                    | molt_ir::OpKind::LoopBreakIfException
            )
            .then_some(op.value)
            .flatten()
        })
        .collect();

    for op in ops {
        let kind = op.kind;

        // Track structured nesting.
        let is_open = matches!(
            kind,
            molt_ir::OpKind::If
                | molt_ir::OpKind::LoopStart
                | molt_ir::OpKind::ForRange
                | molt_ir::OpKind::ForIter
        );
        let is_mid = matches!(kind, molt_ir::OpKind::Else);
        let is_close = matches!(
            kind,
            molt_ir::OpKind::EndIf | molt_ir::OpKind::LoopEnd | molt_ir::OpKind::EndFor
        );

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            // `else` doesn't change depth but resets dead state if we're
            // dead at this depth (the other branch may not have returned).
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            // Closing a block may bring us back to a reachable state.
            if let Some(d) = dead_at_depth
                && d > depth
            {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        // Keep path-local metadata intact until the exception capture projection
        // consumes it; these markers never adjust lexical reachability depth.
        if matches!(kind, molt_ir::OpKind::TryStart | molt_ir::OpKind::TryEnd) {
            result.push(op.clone());
            continue;
        }

        // Out-of-line exception handlers and branch targets can legally appear
        // after a return in the linearized stream. A live label starts a new
        // reachable block even when the preceding block is closed.
        if matches!(kind, molt_ir::OpKind::Label | molt_ir::OpKind::StateLabel)
            && op
                .value
                .is_some_and(|label| referenced_labels.contains(&label))
        {
            dead_at_depth = None;
            result.push(op.clone());
            continue;
        }

        // If we're in dead code, skip this op.
        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            // We're at a shallower depth now — no longer dead.
            dead_at_depth = None;
        }

        // Check if this op is an unconditional return.
        let is_return = kind.is_return_terminator();
        result.push(op.clone());

        if is_return {
            dead_at_depth = Some(depth);
        }
    }

    result
}
