use crate::OpIR;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(in crate::wasm) enum ControlKind {
    Block,
    Loop,
    If,
    Try,
}

pub(in crate::wasm) fn loop_break_depth(control_stack: &[ControlKind]) -> Option<u32> {
    let mut depth = 0u32;
    let mut found_loop = false;
    for entry in control_stack.iter().rev() {
        match entry {
            ControlKind::Block if found_loop => return Some(depth),
            ControlKind::Loop => {
                found_loop = true;
            }
            _ => {}
        }
        depth += 1;
    }
    None
}

pub(in crate::wasm) fn loop_continue_depth(control_stack: &[ControlKind]) -> Option<u32> {
    let mut depth = 0u32;
    for entry in control_stack.iter().rev() {
        if matches!(entry, ControlKind::Loop) {
            return Some(depth);
        }
        depth += 1;
    }
    None
}

pub(in crate::wasm) fn has_non_linear_control_flow(ops: &[OpIR]) -> bool {
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
    fn op_with_io(kind: &str, args: Option<Vec<&str>>, out: Option<&str>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: args.map(|a| a.into_iter().map(String::from).collect()),
            out: out.map(String::from),
            ..OpIR::default()
        }
    }

    #[test]
    fn non_linear_control_flow_detection_handles_jumpful_functions() {
        let ops = vec![
            op_with_io("const", None, Some("v0")),
            op_with_io("check_exception", None, None),
            op_with_io("jump", None, None),
            op_with_io("label", None, None),
        ];
        assert!(has_non_linear_control_flow(&ops));
    }

    #[test]
    fn non_linear_control_flow_detection_ignores_straight_line_ops() {
        let ops = vec![
            op_with_io("const", None, Some("v0")),
            op_with_io("add", Some(vec!["v0", "v1"]), Some("v2")),
            op_with_io("tuple_new", Some(vec!["v2"]), Some("v3")),
        ];
        assert!(!has_non_linear_control_flow(&ops));
    }
}
