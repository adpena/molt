use super::*;

#[test]
fn direct_raise_edge_canonicalization_removes_duplicate_handler_edges() {
    let mut func = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "direct_raise".to_string(),
        params: vec!["acc".into()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
        ops: vec![
            OpIR {
                kind: molt_ir::OpKind::ExceptionNewBuiltinEmpty,
                out: Some("exc".to_string()),
                value: Some(5),
                ..OpIR::new(molt_ir::OpKind::ExceptionNewBuiltinEmpty)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::Raise,
                args: Some(vec!["exc".to_string()]),
                ..OpIR::new(molt_ir::OpKind::Raise)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::Jump,
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::Jump)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
        ],
    };

    let mut typed = crate::tir::lower_from_simple::lower_to_tir(&func);
    crate::tir::drop_phase::prepare_final_backend_lowering(
        &mut typed,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );
    func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);

    assert!(
        !func
            .ops
            .iter()
            .any(|op| op.kind == molt_ir::OpKind::CheckException),
        "direct raise-to-handler edge must not keep redundant polls: {:?}",
        func.ops
    );
    let raise_idx = func
        .ops
        .iter()
        .position(|op| op.kind == molt_ir::OpKind::Raise)
        .expect("raise must remain");
    assert_eq!(func.ops[raise_idx + 1].kind, molt_ir::OpKind::Jump);
    assert!(crate::tir::lower_to_simple::validate_labels(&func.ops));
}
