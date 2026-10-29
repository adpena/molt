use super::has_non_linear_control_flow;
use crate::wasm::function_frame::{WasmFrameControlMode, WasmFunctionFramePlan};
use crate::{FunctionIR, OpIR, SimpleIR};
use molt_ir::simple_verify::{
    EdgeRole, simple_ir_executable_flow, validate_simple_ir_control_flow,
};

fn op(kind: molt_ir::OpKind, value: Option<i64>) -> OpIR {
    OpIR {
        value,
        ..OpIR::new(kind)
    }
}

#[test]
fn dispatch_control_uses_canonical_forward_label_identity() {
    let ops = vec![
        op(molt_ir::OpKind::Jump, Some(7)),
        op(molt_ir::OpKind::ConstNone, None),
        op(molt_ir::OpKind::Label, Some(7)),
    ];
    let flow = simple_ir_executable_flow(&ops);
    assert_eq!(flow.edges[0][0].target, 2);
    assert_eq!(flow.edges[0][0].role, EdgeRole::Taken);
}

#[test]
fn path_local_region_closes_preserve_canonical_if_loop_and_handler_edges() {
    for stateful in [false, true] {
        let mut ops = vec![
            op(molt_ir::OpKind::TryStart, Some(7)),
            op(molt_ir::OpKind::LoopStart, None),
            op_with_io(molt_ir::OpKind::If, Some(vec!["condition"]), None),
            op(molt_ir::OpKind::TryEnd, Some(7)),
            op(molt_ir::OpKind::LoopBreak, None),
            op(molt_ir::OpKind::EndIf, None),
            op(molt_ir::OpKind::LoopEnd, None),
            op(molt_ir::OpKind::TryEnd, Some(7)),
            op(molt_ir::OpKind::Label, Some(7)),
            op(molt_ir::OpKind::RetVoid, None),
        ];
        if stateful {
            ops.insert(
                0,
                OpIR {
                    state_targets: Some(vec![]),
                    ..op(molt_ir::OpKind::StateSwitch, None)
                },
            );
        }
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "path_local_context_close".into(),
            params: vec!["self".into(), "condition".into()],
            ops,
            ..FunctionIR::default()
        };
        let (_, frame) =
            WasmFunctionFramePlan::for_codegen_fixture(&function).into_function_and_frame();
        assert!(
            frame.control_mode()
                == if stateful {
                    WasmFrameControlMode::Stateful
                } else {
                    WasmFrameControlMode::Jumpful
                }
        );
        let flow = simple_ir_executable_flow(&function.ops);
        let offset = usize::from(stateful);
        assert_eq!(
            flow.edges[2 + offset]
                .iter()
                .map(|edge| (edge.role, edge.target))
                .collect::<Vec<_>>(),
            vec![
                (EdgeRole::BranchTrue, 3 + offset),
                (EdgeRole::BranchFalse, 5 + offset)
            ]
        );
        assert_eq!(flow.edges[4 + offset][0].target, 7 + offset);
        assert_eq!(
            flow.edges[6 + offset]
                .iter()
                .map(|edge| edge.target)
                .collect::<Vec<_>>(),
            vec![1 + offset]
        );
    }
}

#[test]
fn malformed_control_is_rejected_by_shared_admission() {
    for (name, ops) in [
        ("missing_jump_label", vec![op(molt_ir::OpKind::Jump, None)]),
        (
            "orphan_jump_label",
            vec![op(molt_ir::OpKind::Jump, Some(99))],
        ),
        ("unbalanced_if", vec![op(molt_ir::OpKind::If, None)]),
        (
            "break_without_loop",
            vec![op(molt_ir::OpKind::LoopBreak, None)],
        ),
    ] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: name.into(),
                ops,
                ..FunctionIR::default()
            }],
            profile: None,
        };
        let error = validate_simple_ir_control_flow(&ir)
            .expect_err("malformed control must fail admission");
        assert!(error.contains(name), "{error}");
    }
}

fn op_with_io(kind: molt_ir::OpKind, args: Option<Vec<&str>>, out: Option<&str>) -> OpIR {
    OpIR {
        args: args.map(|a| a.into_iter().map(String::from).collect()),
        out: out.map(String::from),
        ..OpIR::new(kind)
    }
}

#[test]
fn non_linear_control_flow_detection_handles_jumpful_functions() {
    let ops = vec![
        op_with_io(molt_ir::OpKind::Const, None, Some("v0")),
        op_with_io(molt_ir::OpKind::CheckException, None, None),
        op_with_io(molt_ir::OpKind::Jump, None, None),
        op_with_io(molt_ir::OpKind::Label, None, None),
    ];
    assert!(has_non_linear_control_flow(&ops));
}

#[test]
fn non_linear_control_flow_detection_ignores_straight_line_ops() {
    let ops = vec![
        op_with_io(molt_ir::OpKind::Const, None, Some("v0")),
        op_with_io(molt_ir::OpKind::Add, Some(vec!["v0", "v1"]), Some("v2")),
        op_with_io(molt_ir::OpKind::TupleNew, Some(vec!["v2"]), Some("v3")),
    ];
    assert!(!has_non_linear_control_flow(&ops));
}
