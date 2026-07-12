use crate::OpIR;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(in crate::wasm) struct DispatchControlMaps {
    pub(in crate::wasm) label_to_index: BTreeMap<i64, usize>,
    pub(in crate::wasm) else_for_if: BTreeMap<usize, usize>,
    pub(in crate::wasm) end_for_if: BTreeMap<usize, usize>,
    pub(in crate::wasm) end_for_else: BTreeMap<usize, usize>,
    pub(in crate::wasm) loop_continue_target: BTreeMap<usize, usize>,
    pub(in crate::wasm) loop_break_target: BTreeMap<usize, usize>,
}

pub(in crate::wasm) fn dispatch_control_panic(
    function_name: &str,
    op_idx: usize,
    message: impl std::fmt::Display,
) -> ! {
    panic!("invalid WASM dispatch control in function `{function_name}` op {op_idx}: {message}")
}

pub(in crate::wasm) fn build_dispatch_control_maps(
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
