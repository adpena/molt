use crate::OpIR;
use std::collections::{BTreeMap, BTreeSet};

fn is_stateful_dispatch_terminator(kind: &str) -> bool {
    matches!(
        kind,
        "state_switch"
            | "state_transition"
            | "state_yield"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "if"
            | "else"
            | "end_if"
            | "loop_start"
            | "loop_index_start"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break"
            | "loop_continue"
            | "loop_end"
            | "jump"
            | "try_start"
            | "try_end"
            | "label"
            | "state_label"
            | "check_exception"
            | "ret"
            | "ret_void"
    )
}

pub(crate) fn has_non_linear_control_flow(ops: &[OpIR]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "if" | "else"
                | "end_if"
                | "loop_start"
                | "loop_index_start"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_break_if_exception"
                | "loop_break"
                | "loop_continue"
                | "loop_end"
                | "for_iter_start"
                | "for_iter_end"
                | "while_start"
                | "while_end"
                | "try_start"
                | "try_end"
                | "async_for_start"
                | "async_for_end"
                | "jump"
                | "br_if"
                | "label"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
                | "state_label"
                | "check_exception"
                | "ret"
                | "ret_void"
        )
    })
}

pub(crate) fn build_dispatch_blocks(ops: &[OpIR]) -> (Vec<usize>, Vec<usize>) {
    let op_count = ops.len();
    if op_count == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut is_start = vec![false; op_count];
    is_start[0] = true;
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" | "state_label" | "loop_start" | "loop_index_start" | "loop_end" => {
                is_start[idx] = true;
            }
            _ => {}
        }
        if is_stateful_dispatch_terminator(op.kind.as_str()) && idx + 1 < op_count {
            is_start[idx + 1] = true;
        }
    }

    let mut starts = Vec::new();
    for (idx, start) in is_start.iter().enumerate() {
        if *start {
            starts.push(idx);
        }
    }

    let mut block_for_op = vec![0; op_count];
    let mut block_idx = 0usize;
    let mut next_start = starts.get(1).copied().unwrap_or(op_count);
    for (idx, block_slot) in block_for_op.iter_mut().enumerate().take(op_count) {
        if idx == next_start {
            block_idx += 1;
            next_start = starts.get(block_idx + 1).copied().unwrap_or(op_count);
        }
        *block_slot = block_idx;
    }

    (starts, block_for_op)
}

pub(crate) fn build_dispatch_block_map(block_for_op: &[usize]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(block_for_op.len() * 4);
    for &block_idx in block_for_op {
        bytes.extend_from_slice(&(block_idx as u32).to_le_bytes());
    }
    bytes
}

#[derive(Default)]
pub(crate) struct DispatchControlMaps {
    pub(crate) label_to_index: BTreeMap<i64, usize>,
    pub(crate) else_for_if: BTreeMap<usize, usize>,
    pub(crate) end_for_if: BTreeMap<usize, usize>,
    pub(crate) end_for_else: BTreeMap<usize, usize>,
    pub(crate) loop_continue_target: BTreeMap<usize, usize>,
    pub(crate) loop_break_target: BTreeMap<usize, usize>,
}

pub(crate) fn dispatch_control_panic(
    function_name: &str,
    op_idx: usize,
    message: impl std::fmt::Display,
) -> ! {
    panic!("invalid WASM dispatch control in function `{function_name}` op {op_idx}: {message}")
}

pub(crate) fn build_dispatch_control_maps(
    ops: &[OpIR],
    include_state_labels: bool,
    function_name: &str,
) -> DispatchControlMaps {
    struct LoopFrame {
        start_idx: usize,
        break_ops: Vec<usize>,
    }

    let mut valid_labels: BTreeSet<i64> = BTreeSet::new();
    for op in ops {
        match op.kind.as_str() {
            "label" | "state_label" if include_state_labels => {
                if let Some(label_id) = op.value {
                    valid_labels.insert(label_id);
                }
            }
            "label" => {
                if let Some(label_id) = op.value {
                    valid_labels.insert(label_id);
                }
            }
            _ => {}
        }
    }

    let mut maps = DispatchControlMaps::default();
    let mut if_stack: Vec<usize> = Vec::new();
    let mut loop_stack: Vec<LoopFrame> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "jump" | "br_if" => {
                let Some(label_id) = op.value else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!("{} missing target label id", op.kind),
                    );
                };
                if !valid_labels.contains(&label_id) {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!(
                            "{} target label {} is not present in dispatch label map",
                            op.kind, label_id
                        ),
                    );
                }
            }
            "label" => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "state_label" if include_state_labels => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "if" => if_stack.push(idx),
            "else" => {
                let Some(if_idx) = if_stack.last().copied() else {
                    dispatch_control_panic(function_name, idx, "else without matching if");
                };
                maps.else_for_if.insert(if_idx, idx);
            }
            "end_if" => {
                let Some(if_idx) = if_stack.pop() else {
                    dispatch_control_panic(function_name, idx, "end_if without matching if");
                };
                maps.end_for_if.insert(if_idx, idx);
                if let Some(else_idx) = maps.else_for_if.get(&if_idx).copied() {
                    maps.end_for_else.insert(else_idx, idx);
                }
            }
            "loop_start" => {
                loop_stack.push(LoopFrame {
                    start_idx: idx,
                    break_ops: Vec::new(),
                });
            }
            "loop_index_start" => {
                // loop_index_start is always preceded by loop_start,
                // which already pushed a LoopFrame. Update the
                // start_idx to point here (the actual loop body start)
                // instead of pushing a duplicate frame.
                let Some(frame) = loop_stack.last_mut() else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        "loop_index_start without matching loop_start",
                    );
                };
                frame.start_idx = idx;
            }
            "loop_continue" => {
                let Some(frame) = loop_stack.last() else {
                    dispatch_control_panic(function_name, idx, "loop_continue without loop");
                };
                maps.loop_continue_target.insert(idx, frame.start_idx);
            }
            "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break" => {
                let Some(frame) = loop_stack.last_mut() else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!("{} without loop", op.kind),
                    );
                };
                frame.break_ops.push(idx);
            }
            "loop_end" => {
                let Some(frame) = loop_stack.pop() else {
                    dispatch_control_panic(function_name, idx, "loop_end without loop_start");
                };
                for break_idx in frame.break_ops {
                    maps.loop_break_target.insert(break_idx, idx);
                }
            }
            _ => {}
        }
    }
    if let Some(if_idx) = if_stack.last().copied() {
        dispatch_control_panic(function_name, if_idx, "if without matching end_if");
    }
    if let Some(frame) = loop_stack.last() {
        dispatch_control_panic(
            function_name,
            frame.start_idx,
            "loop_start without matching loop_end",
        );
    }

    maps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str, value: Option<i64>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value,
            ..OpIR::default()
        }
    }

    #[test]
    fn dispatch_control_accepts_forward_jump_labels() {
        let maps = build_dispatch_control_maps(
            &[
                op("jump", Some(7)),
                op("const_none", None),
                op("label", Some(7)),
            ],
            false,
            "forward_jump",
        );

        assert_eq!(maps.label_to_index.get(&7), Some(&2));
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `missing_jump_label` op 0: jump missing target label id"
    )]
    fn dispatch_control_rejects_jump_without_label() {
        build_dispatch_control_maps(&[op("jump", None)], false, "missing_jump_label");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `orphan_jump_label` op 0: jump target label 99 is not present in dispatch label map"
    )]
    fn dispatch_control_rejects_unknown_jump_label() {
        build_dispatch_control_maps(&[op("jump", Some(99))], false, "orphan_jump_label");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `unbalanced_if` op 0: if without matching end_if"
    )]
    fn dispatch_control_rejects_unbalanced_if() {
        build_dispatch_control_maps(&[op("if", None)], false, "unbalanced_if");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `break_without_loop` op 0: loop_break without loop"
    )]
    fn dispatch_control_rejects_loop_break_without_loop() {
        build_dispatch_control_maps(&[op("loop_break", None)], false, "break_without_loop");
    }
}
