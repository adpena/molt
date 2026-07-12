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
    use crate::OpIR;

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
