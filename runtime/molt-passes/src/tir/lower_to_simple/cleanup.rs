use std::collections::HashSet;

use crate::ir::OpIR;

/// Remove dead `label` ops from the linearised op stream.
///
/// A "dead label" is a `label`/`state_label` op whose label id is never
/// the target of any `jump`, `br_if`, or `check_exception` op AND whose
/// preceding instruction has already terminated the block (i.e., the label
/// is not reachable via fallthrough either).
///
/// The Cranelift backend creates a block for every label it sees in its
/// pre-scan.  If that block ends up with no predecessors (no branch targets
/// it AND no fallthrough), Cranelift's alias_analysis and block ordering
/// panic with `Option::unwrap() on None`.
///
/// This pass strips only the dead label ops themselves.  The code following
/// a dead label is kept: it may be reachable via structured control flow
/// (e.g., `loop_end` switches to an `after_block` and the following ops
/// are emitted into that block).
pub(super) fn eliminate_dead_labels(ops: &mut Vec<OpIR>) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FilledState {
        Open,
        Closed,
        LoopContinue,
    }

    loop {
        // Phase 1: collect all label ids that are explicit branch targets.
        let mut branch_targets: HashSet<i64> = HashSet::new();
        for op in ops.iter() {
            match op.kind.as_str() {
                "jump" | "br_if" | "check_exception" | "try_start" | "loop_continue" => {
                    if let Some(id) = op.value {
                        branch_targets.insert(id);
                    }
                }
                "state_yield" | "state_transition" | "chan_send_yield" | "chan_recv_yield" => {
                    if let Some(id) = op.value {
                        branch_targets.insert(id);
                    }
                }
                _ => {}
            }
        }

        // Phase 2: walk ops, detecting dead labels.
        // `is_filled` tracks whether the current block has been terminated
        // (by jump/ret/loop_continue) without a subsequent label starting a
        // new live block. `raise` is intentionally not a SimpleIR terminator:
        // host-call EH models it as "set pending exception", then an explicit
        // check/jump edge carries block arguments into the handler/cleanup
        // block. Treating `raise` as filled deletes that edge and drops the
        // carrier state.
        let mut filled_state = FilledState::Open;
        let mut current_block_started_at_live_label = false;
        let mut keep = vec![true; ops.len()];

        for i in 0..ops.len() {
            let kind = ops[i].kind.as_str();
            match kind {
                "jump" | "ret" | "loop_break" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Closed;
                    }
                }
                "loop_continue" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::LoopContinue;
                    }
                }
                "label" | "state_label" => {
                    let label_id = ops[i].value.unwrap_or(-1);
                    if filled_state != FilledState::Open && !branch_targets.contains(&label_id) {
                        // Dead label: preceded by a terminator and not a
                        // branch target.  Remove the label op but keep the
                        // code that follows (it may be reachable via
                        // structured control flow like loop_end → after_block).
                        keep[i] = false;
                    } else {
                        // Live label (reachable via fallthrough or branch).
                        filled_state = FilledState::Open;
                        current_block_started_at_live_label = true;
                    }
                }
                // loop_end resets the filled state only when the current block
                // is still live, or when it closes the implicit break path of
                // a structured loop after a textual `loop_continue`.
                "loop_end" => {
                    if filled_state == FilledState::Closed && !current_block_started_at_live_label {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Open;
                        current_block_started_at_live_label = false;
                    }
                }
                "if" | "else" | "end_if" => {
                    // Structured if markers remain live even when the
                    // immediately preceding textual branch returned or raised.
                    // If a dead labeled path falls into a structured `if`,
                    // stripping only the opening `if` while preserving
                    // `else` / `end_if` corrupts the control stack.
                    filled_state = FilledState::Open;
                    current_block_started_at_live_label = false;
                }
                "try_start" | "try_end" => {
                    // Try markers are codegen-region boundaries, not executable
                    // straight-line work.  A protected body may end in a raise
                    // followed by the explicit handler jump, but the following
                    // try_end is still the shared fact that closes the protected
                    // body for text backends such as Luau.  Keeping try_end live
                    // also re-opens the synthetic post-pcall dispatch path so the
                    // success jump is not discarded as unreachable text.
                    filled_state = FilledState::Open;
                    current_block_started_at_live_label = false;
                }
                // loop_start, loop_break_if_false/true/exception do not fill:
                // each has a fall-through path (the non-break edge continues the
                // loop body), so they are control-flow markers, not block-fillers.
                "loop_start"
                | "loop_break_if_false"
                | "loop_break_if_true"
                | "loop_break_if_exception"
                | "loop_index_start" => {
                    // These are control-flow markers that don't terminate blocks.
                    if kind == "loop_start" {
                        current_block_started_at_live_label = false;
                    }
                }
                "br_if" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        // br_if has a fallthrough path — does not fill.
                        filled_state = FilledState::Open;
                    }
                }
                _ => {
                    if filled_state != FilledState::Open {
                        // Once a block is terminated, any straight-line ops that
                        // follow before the next live label are unreachable. Keep
                        // only the structural boundary ops handled above.
                        keep[i] = false;
                    }
                }
            }
        }

        // Phase 3: compact — remove dead ops.
        let old_len = ops.len();
        let mut write_idx = 0;
        for read_idx in 0..ops.len() {
            if keep[read_idx] {
                if write_idx != read_idx {
                    ops.swap(write_idx, read_idx);
                }
                write_idx += 1;
            }
        }
        ops.truncate(write_idx);
        if ops.len() == old_len {
            break;
        }
    }
}

pub(super) fn close_try_regions_before_handler_labels(ops: &mut Vec<OpIR>) {
    let mut active_handlers: Vec<i64> = Vec::new();
    let mut result = Vec::with_capacity(ops.len());
    let mut changed = false;

    for op in ops.iter() {
        if matches!(op.kind.as_str(), "label" | "state_label")
            && let Some(label) = op.value
            && let Some(pos) = active_handlers
                .iter()
                .rposition(|&handler| handler == label)
        {
            let drained: Vec<i64> = active_handlers.drain(pos..).collect();
            for handler in drained.into_iter().rev() {
                result.push(OpIR {
                    kind: "try_end".to_string(),
                    value: Some(handler),
                    ..OpIR::default()
                });
                changed = true;
            }
        }

        match op.kind.as_str() {
            "try_start" => {
                if let Some(handler) = op.value {
                    active_handlers.push(handler);
                }
            }
            "try_end" => {
                if let Some(handler) = op.value
                    && let Some(pos) = active_handlers
                        .iter()
                        .rposition(|&active| active == handler)
                {
                    active_handlers.drain(pos..);
                }
            }
            _ => {}
        }

        result.push(op.clone());
    }

    if changed {
        *ops = result;
    }
}

pub(super) fn validate_structured_if_markers(ops: &[OpIR]) -> Result<(), String> {
    #[derive(Clone, Copy)]
    struct IfFrame {
        if_idx: usize,
        saw_else: bool,
    }

    let mut stack: Vec<IfFrame> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "if" => stack.push(IfFrame {
                if_idx: idx,
                saw_else: false,
            }),
            "else" => {
                let Some(frame) = stack.last_mut() else {
                    return Err(format!("orphan else at op {idx}"));
                };
                if frame.saw_else {
                    return Err(format!(
                        "duplicate else at op {idx} for if starting at op {}",
                        frame.if_idx
                    ));
                }
                frame.saw_else = true;
            }
            "end_if" => {
                let Some(_frame) = stack.pop() else {
                    return Err(format!("orphan end_if at op {idx}"));
                };
            }
            _ => {}
        }
    }
    if let Some(frame) = stack.last() {
        return Err(format!("unterminated if starting at op {}", frame.if_idx));
    }
    Ok(())
}

/// Validate that every label referenced by jump/br_if/check_exception exists
/// as a label op in the output.  Returns false if any reference is dangling.
pub fn validate_labels(ops: &[crate::ir::OpIR]) -> bool {
    missing_label_references(ops).is_empty()
}

pub(super) fn missing_label_references(ops: &[crate::ir::OpIR]) -> Vec<i64> {
    let mut defined_labels: HashSet<i64> = HashSet::new();
    let mut referenced_labels: HashSet<i64> = HashSet::new();
    for op in ops {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    defined_labels.insert(id);
                }
            }
            "jump" | "br_if" | "check_exception" => {
                if let Some(id) = op.value {
                    referenced_labels.insert(id);
                }
            }
            _ => {}
        }
    }
    let mut missing: Vec<i64> = referenced_labels
        .difference(&defined_labels)
        .copied()
        .collect();
    missing.sort_unstable();
    missing
}
