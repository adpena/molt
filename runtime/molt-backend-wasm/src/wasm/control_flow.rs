mod depth;
mod dispatch_maps;

pub(in crate::wasm) use self::depth::{
    ControlKind, has_non_linear_control_flow, loop_break_depth, loop_continue_depth,
};
pub(in crate::wasm) use self::dispatch_maps::{
    DispatchControlMaps, build_dispatch_control_maps, dispatch_control_panic,
};

#[cfg(test)]
mod tests {
    use super::{build_dispatch_control_maps, has_non_linear_control_flow};
    use crate::wasm::function_frame::{WasmFrameControlMode, WasmFunctionFramePlan};
    use crate::{FunctionIR, OpIR};

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
    fn path_local_region_closes_route_through_dispatch_without_closing_if_or_loop() {
        for stateful in [false, true] {
            let mut ops = vec![
                op("try_start", Some(7)),
                op("loop_start", None),
                op_with_io("if", Some(vec!["condition"]), None),
                op("try_end", Some(7)),
                op("loop_break", None),
                op("end_if", None),
                op("loop_end", None),
                op("try_end", Some(7)),
                op("label", Some(7)),
                op("ret_void", None),
            ];
            if stateful {
                ops.push(op("state_switch", None));
            }
            let function = FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "path_local_context_close".to_string(),
                params: vec!["self".to_string(), "condition".to_string()],
                ops,
                ..FunctionIR::default()
            };
            let (_, frame) =
                WasmFunctionFramePlan::for_function(&function).into_function_and_frame();
            assert!(
                frame.control_mode()
                    == if stateful {
                        WasmFrameControlMode::Stateful
                    } else {
                        WasmFrameControlMode::Jumpful
                    }
            );
            let maps = build_dispatch_control_maps(&function.ops, stateful, &function.name);
            assert_eq!(maps.end_for_if.get(&2), Some(&5));
            assert_eq!(maps.loop_break_target.get(&4), Some(&6));
            assert_eq!(maps.label_to_index.get(&7), Some(&8));
        }
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
