use crate::{FunctionIR, OpIR};
use std::collections::BTreeMap;

/// Rewrite loops that contain `state_yield` in stateful (generator/async)
/// functions so the native backend can resume inside the loop body.
///
/// Problem: the native Cranelift backend tracks loop context on a runtime
/// `loop_stack`.  When a generator yields inside a loop and later resumes,
/// the `loop_start` that pushed the frame is skipped (the state machine
/// jumps directly to the resume block).  Any subsequent `loop_continue`
/// finds an empty `loop_stack` and falls through, ending the generator
/// after a single yield.
///
/// Fix: for every loop that encloses a `state_yield`, insert a
/// `state_label` at the loop body start and rewrite `loop_continue` ops
/// to `jump` ops targeting that label.  The native backend treats
/// `state_label` as a resume-eligible block and `jump` as an
/// unconditional branch — both work correctly across state-machine
/// boundaries.
pub fn rewrite_stateful_loops(func_ir: &mut FunctionIR) {
    // Only transform stateful functions (generators / async).
    let is_stateful = func_ir.ops.iter().any(|op| {
        matches!(
            op.kind,
            molt_ir::OpKind::StateSwitch
                | molt_ir::OpKind::StateTransition
                | molt_ir::OpKind::StateYield
        )
    });
    if !is_stateful {
        return;
    }

    // Find the maximum state ID already in use so we can allocate fresh IDs.
    let mut max_state_id: i64 = 0;
    for op in &func_ir.ops {
        if let Some(id) = op.value
            && matches!(
                op.kind,
                molt_ir::OpKind::StateYield
                    | molt_ir::OpKind::StateTransition
                    | molt_ir::OpKind::StateLabel
                    | molt_ir::OpKind::Label
            )
        {
            max_state_id = max_state_id.max(id);
        }
    }
    let mut next_state_id = max_state_id + 100; // leave headroom

    // Build a stack of loop start indices and find which loops contain yields.
    struct LoopInfo {
        start_idx: usize,
        end_idx: usize,
        has_yield: bool,
        continues: Vec<usize>,
        breaks: Vec<usize>,
        break_if_trues: Vec<usize>,
        break_if_falses: Vec<usize>,
    }
    let mut loop_stack: Vec<LoopInfo> = Vec::new();
    let mut finished_loops: Vec<LoopInfo> = Vec::new();

    for (idx, op) in func_ir.ops.iter().enumerate() {
        match op.kind {
            molt_ir::OpKind::LoopStart | molt_ir::OpKind::LoopIndexStart => {
                loop_stack.push(LoopInfo {
                    start_idx: idx,
                    end_idx: 0,
                    has_yield: false,
                    continues: Vec::new(),
                    breaks: Vec::new(),
                    break_if_trues: Vec::new(),
                    break_if_falses: Vec::new(),
                });
            }
            molt_ir::OpKind::StateYield => {
                for frame in loop_stack.iter_mut() {
                    frame.has_yield = true;
                }
            }
            molt_ir::OpKind::LoopContinue => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.continues.push(idx);
                }
            }
            molt_ir::OpKind::LoopBreak => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.breaks.push(idx);
                }
            }
            molt_ir::OpKind::LoopBreakIfTrue => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_trues.push(idx);
                }
            }
            molt_ir::OpKind::LoopBreakIfFalse => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_falses.push(idx);
                }
            }
            molt_ir::OpKind::LoopBreakIfException => {
                // `loop_break_if_exception` is emitted by the frontend ONLY in
                // functions WITHOUT the exception stack (function_exception_label
                // == None).  Generator/coroutine bodies — the only functions
                // lowered to a yield state machine here — are always compiled
                // with the exception stack (generator polls default to
                // needs_exception_stack=True), so this op can never appear inside
                // a yield-bearing loop.  Assert the invariant: if it is ever
                // violated, the state-machine rewrite below would silently drop
                // the exception break and re-introduce the infinite-loop/OOM bug.
                debug_assert!(
                    false,
                    "loop_break_if_exception inside a yield state-machine loop \
                     ({}@op{}) — generator polls must be needs_exception_stack=True",
                    func_ir.name, idx
                );
            }
            molt_ir::OpKind::LoopEnd => {
                if let Some(mut frame) = loop_stack.pop() {
                    frame.end_idx = idx;
                    finished_loops.push(frame);
                }
            }
            _ => {}
        }
    }

    let yield_loops: Vec<LoopInfo> = finished_loops.into_iter().filter(|l| l.has_yield).collect();

    if yield_loops.is_empty() {
        return;
    }

    // For each yield-containing loop, allocate TWO state labels:
    //   body_label  — at loop body start (continue / back-edge target)
    //   after_label — after loop_end (break target)
    // Replace ALL structured loop ops with labels and jumps so the
    // state machine works correctly on resume.
    let mut body_label_for_start: BTreeMap<usize, i64> = BTreeMap::new();
    let mut after_label_for_end: BTreeMap<usize, i64> = BTreeMap::new();
    let mut continue_target: BTreeMap<usize, i64> = BTreeMap::new();
    let mut break_target: BTreeMap<usize, i64> = BTreeMap::new();

    for info in &yield_loops {
        let body_label = next_state_id;
        next_state_id += 1;
        let after_label = next_state_id;
        next_state_id += 1;

        body_label_for_start.insert(info.start_idx, body_label);
        after_label_for_end.insert(info.end_idx, after_label);

        for &ci in &info.continues {
            continue_target.insert(ci, body_label);
        }
        for &bi in &info.breaks {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_trues {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_falses {
            break_target.insert(bi, after_label);
        }
    }

    // Rebuild the ops list, replacing structured loop ops with labels/jumps.
    let old_ops = std::mem::take(&mut func_ir.ops);
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(old_ops.len() + yield_loops.len() * 4);

    for (idx, op) in old_ops.into_iter().enumerate() {
        if let Some(&body_label) = body_label_for_start.get(&idx) {
            // Replace loop_start with state_label (loop body entry).
            new_ops.push(OpIR {
                kind: molt_ir::OpKind::StateLabel,
                value: Some(body_label),
                ..OpIR::new(molt_ir::OpKind::StateLabel)
            });
        } else if let Some(&after_label) = after_label_for_end.get(&idx) {
            // Replace loop_end with state_label (break target).
            new_ops.push(OpIR {
                kind: molt_ir::OpKind::StateLabel,
                value: Some(after_label),
                ..OpIR::new(molt_ir::OpKind::StateLabel)
            });
        } else if let Some(&target) = continue_target.get(&idx) {
            // Replace loop_continue with jump to body label.
            new_ops.push(OpIR {
                kind: molt_ir::OpKind::Jump,
                value: Some(target),
                ..OpIR::new(molt_ir::OpKind::Jump)
            });
        } else if let Some(&target) = break_target.get(&idx) {
            match op.kind {
                molt_ir::OpKind::LoopBreak => {
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::Jump,
                        value: Some(target),
                        ..OpIR::new(molt_ir::OpKind::Jump)
                    });
                }
                molt_ir::OpKind::LoopBreakIfTrue => {
                    // Expand: if(cond) { jump(after_label) } end_if
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::If,
                        args: op.args.clone(),
                        ..OpIR::new(molt_ir::OpKind::If)
                    });
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::Jump,
                        value: Some(target),
                        ..OpIR::new(molt_ir::OpKind::Jump)
                    });
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::EndIf,
                        ..OpIR::new(molt_ir::OpKind::EndIf)
                    });
                }
                molt_ir::OpKind::LoopBreakIfFalse => {
                    let cond = op
                        .args
                        .as_ref()
                        .and_then(|a| a.first().cloned())
                        .unwrap_or_default();
                    let not_var = format!("__slr_not_{idx}");
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::Not,
                        args: Some(vec![cond]),
                        out: Some(not_var.clone()),
                        ..OpIR::new(molt_ir::OpKind::Not)
                    });
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::If,
                        args: Some(vec![not_var]),
                        ..OpIR::new(molt_ir::OpKind::If)
                    });
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::Jump,
                        value: Some(target),
                        ..OpIR::new(molt_ir::OpKind::Jump)
                    });
                    new_ops.push(OpIR {
                        kind: molt_ir::OpKind::EndIf,
                        ..OpIR::new(molt_ir::OpKind::EndIf)
                    });
                }
                _ => new_ops.push(op),
            }
        } else {
            new_ops.push(op);
        }
    }

    func_ir.ops = new_ops;
}
