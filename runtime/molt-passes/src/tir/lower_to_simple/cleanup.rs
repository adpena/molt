use std::collections::HashSet;

use crate::ir::OpIR;
use crate::tir::dominators::is_simple_exception_transfer_kind;

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
            if let Some(targets) = &op.state_targets {
                branch_targets.extend(targets.iter().map(|&(_, label)| label));
            }
            let kind = op.kind;
            if (kind.is_verifier_label_reference()
                || is_simple_exception_transfer_kind(kind)
                || matches!(kind, molt_ir::OpKind::BrIf | molt_ir::OpKind::LoopContinue)
                || kind.is_suspend())
                && let Some(id) = op.value
            {
                branch_targets.insert(id);
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
        let mut open_loops = 0usize;
        let mut keep = vec![true; ops.len()];

        for i in 0..ops.len() {
            let kind = ops[i].kind;
            match kind {
                kind if kind.is_terminator() && kind != molt_ir::OpKind::LoopContinue => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Closed;
                    }
                }
                molt_ir::OpKind::LoopContinue => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::LoopContinue;
                    }
                }
                molt_ir::OpKind::Label | molt_ir::OpKind::StateLabel => {
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
                // A retained loop opener owns its lexical closer even when
                // the final body path returned or jumped away. The loop's
                // conditional break can still reach the code after it.
                molt_ir::OpKind::LoopEnd => {
                    let paired = open_loops != 0;
                    open_loops = open_loops.saturating_sub(1);
                    if !paired
                        && filled_state == FilledState::Closed
                        && !current_block_started_at_live_label
                    {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Open;
                        current_block_started_at_live_label = false;
                    }
                }
                molt_ir::OpKind::If | molt_ir::OpKind::Else | molt_ir::OpKind::EndIf => {
                    // Structured if markers remain live even when the
                    // immediately preceding textual branch returned or raised.
                    // If a dead labeled path falls into a structured `if`,
                    // stripping only the opening `if` while preserving
                    // `else` / `end_if` corrupts the control stack.
                    filled_state = FilledState::Open;
                    current_block_started_at_live_label = false;
                }
                molt_ir::OpKind::TryStart | molt_ir::OpKind::TryEnd => {
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
                molt_ir::OpKind::LoopStart
                | molt_ir::OpKind::LoopBreakIfFalse
                | molt_ir::OpKind::LoopBreakIfTrue
                | molt_ir::OpKind::LoopBreakIfException
                | molt_ir::OpKind::LoopIndexStart => {
                    // These are control-flow markers that don't terminate blocks.
                    if kind == molt_ir::OpKind::LoopStart {
                        open_loops += 1;
                        current_block_started_at_live_label = false;
                    }
                }
                molt_ir::OpKind::BrIf => {
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

/// Validate that every label referenced by jump/br_if/check_exception exists
/// as a label op in the output.  Returns false if any reference is dangling.
pub fn validate_labels(ops: &[crate::ir::OpIR]) -> bool {
    missing_label_references(ops).is_empty()
}

pub(super) fn missing_label_references(ops: &[crate::ir::OpIR]) -> Vec<i64> {
    let mut defined_labels: HashSet<i64> = HashSet::new();
    let mut referenced_labels: HashSet<i64> = HashSet::new();
    for op in ops {
        if let Some(targets) = &op.state_targets {
            referenced_labels.extend(targets.iter().map(|&(_, label)| label));
        }
        let kind = op.kind;
        if kind.is_verifier_label_definition() {
            if let Some(id) = op.value {
                defined_labels.insert(id);
            }
        } else if (kind.is_verifier_label_reference()
            || is_simple_exception_transfer_kind(kind)
            || kind == molt_ir::OpKind::BrIf)
            && let Some(id) = op.value
        {
            referenced_labels.insert(id);
        }
    }
    let mut missing: Vec<i64> = referenced_labels
        .difference(&defined_labels)
        .copied()
        .collect();
    missing.sort_unstable();
    missing
}
