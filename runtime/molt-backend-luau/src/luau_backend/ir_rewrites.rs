use crate::OpIR;
use std::collections::BTreeSet;

#[inline]
fn is_exception_observer_kind(kind: &str) -> bool {
    molt_ir::tir::op_kinds_generated::simpleir_kind_is_exception_check(kind)
}

pub(super) fn observes_pending_exception(ops: &[OpIR]) -> bool {
    // Without a local pending-state observer, host errors can unwind directly
    // to the caller's capture. Such functions need no extra operation scopes.
    ops.iter().any(|op| {
        is_exception_observer_kind(&op.kind)
            || matches!(
                op.kind.as_str(),
                "try_start"
                    | "loop_break_if_exception"
                    | "exception_pending"
                    | "exception_current"
                    | "exception_last"
                    | "exception_last_pending"
                    | "exception_finally_pending_observer"
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
        simpleir_kind_is_cfg_or_ssa_consumed, simpleir_kind_is_suspend,
    };
    use molt_ir::tir::simple_def_use::visit_simple_ir_defined_names;

    let mut result = Vec::with_capacity(ops.len());
    for op in ops {
        let kind = op.kind.as_str();
        if matches!(kind, "try_start" | "try_end") {
            continue;
        }
        // This target projects pending/handled state directly, without host
        // throws. Constructors and match/value/cause operations remain subject
        // to the shared may-throw oracle. The only target-local control kinds
        // are the synthetic loops and captures introduced by these rewrites.
        let protocol_or_control = matches!(
            kind,
            "exception_push"
                | "exception_pop"
                | "exception_stack_enter"
                | "exception_stack_exit"
                | "exception_stack_depth"
                | "exception_stack_set_depth"
                | "exception_stack_clear"
                | "exception_context_set"
                | "exception_clear"
                | "exception_set_last"
                | "exception_last"
                | "exception_last_pending"
                | "exception_finally_pending_observer"
                | "exception_active"
                | "exception_current"
                | "exception_pending"
                | "raise"
                | "for_range"
                | "for_iter"
                | "end_for"
                | "pcall_wrap_begin"
                | "pcall_wrap_end"
        ) || is_exception_observer_kind(kind)
            || simpleir_kind_is_cfg_or_ssa_consumed(kind)
            || simpleir_kind_is_suspend(kind)
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
                kind: "pcall_wrap_begin".into(),
                args: Some(outputs),
                value: Some(*counter),
                ..OpIR::default()
            });
        }
        result.push(op.clone());
        if may_throw {
            result.push(OpIR {
                kind: "pcall_wrap_end".into(),
                value: Some(*counter),
                ..OpIR::default()
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
                op.kind.as_str(),
                "jump"
                    | "goto"
                    | "br_if"
                    | "branch"
                    | "branch_false"
                    | "check_exception"
                    | "async_work_poll"
                    | "try_start"
                    | "state_block_start"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_break_if_exception"
            )
            .then_some(op.value)
            .flatten()
        })
        .collect();

    for op in ops {
        let kind = op.kind.as_str();

        // Track structured nesting.
        let is_open = matches!(kind, "if" | "loop_start" | "for_range" | "for_iter");
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "end_for");

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
        if matches!(kind, "try_start" | "try_end") {
            result.push(op.clone());
            continue;
        }

        // Out-of-line exception handlers and branch targets can legally appear
        // after a return in the linearized stream. A live label starts a new
        // reachable block even when the preceding block is closed.
        if matches!(kind, "label" | "state_label")
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
        let is_return = molt_ir::tir::op_kinds_generated::simpleir_kind_is_return_terminator(kind);
        result.push(op.clone());

        if is_return {
            dead_at_depth = Some(depth);
        }
    }

    result
}
