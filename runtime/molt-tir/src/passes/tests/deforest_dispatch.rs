use super::*;

fn op_with(kind: molt_ir::OpKind, out: Option<&str>, s_value: Option<&str>, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind,
        out: out.map(str::to_string),
        s_value: s_value.map(str::to_string),
        args: if args.is_empty() {
            None
        } else {
            Some(args.iter().map(|a| a.to_string()).collect())
        },
        ..OpIR::new(kind)
    }
}

fn assert_source_site(op: &OpIR, line: i64, col: i64, end_col: i64) {
    assert_eq!(op.source_line, Some(line), "source_line for {}", op.kind);
    assert_eq!(op.col_offset, Some(col), "col_offset for {}", op.kind);
    assert_eq!(
        op.end_col_offset,
        Some(end_col),
        "end_col_offset for {}",
        op.kind
    );
}

#[test]
fn split_field_deforestation_preserves_source_site() {
    let mut field = op_with(
        molt_ir::OpKind::StringSplitField,
        Some("field"),
        None,
        &["hay", "sep", "idx"],
    );
    field.source_line = Some(31);
    field.col_offset = Some(2);
    field.end_col_offset = Some(12);
    let mut len = op_with(molt_ir::OpKind::Len, Some("n"), None, &["field"]);
    len.source_line = Some(32);
    len.col_offset = Some(4);
    len.end_col_offset = Some(9);
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "f".to_string(),
        params: vec!["hay".into(), "sep".into(), "idx".into()],
        ops: vec![
            field,
            OpIR {
                value: Some(9),
                ..op_with(molt_ir::OpKind::CheckException, None, None, &[])
            },
            len,
            op_with(molt_ir::OpKind::RetVoid, None, None, &[]),
            OpIR {
                value: Some(9),
                ..op_with(molt_ir::OpKind::Label, None, None, &[])
            },
            op_with(molt_ir::OpKind::RetVoid, None, None, &[]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let mut typed = crate::tir::lower_from_simple::lower_to_tir(&func);
    crate::tir::drop_phase::prepare_final_backend_lowering(
        &mut typed,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );
    func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);

    let start = func
        .ops
        .iter()
        .find(|op| op.kind == molt_ir::OpKind::StringSplitFieldStart)
        .unwrap_or_else(|| {
            panic!(
                "field source op should be split into property reads: {:?}",
                func.ops
            )
        });
    assert_source_site(start, 31, 2, 12);
    let len_from_bounds = func
        .ops
        .iter()
        .find(|op| op.kind == molt_ir::OpKind::StringSplitFieldLenFromBounds)
        .expect("len consumer should be rewritten through bounds");
    assert_source_site(len_from_bounds, 32, 4, 9);
}

#[test]
fn split_field_guards_require_matching_tags_and_no_observed_alias() {
    for kind in [molt_ir::OpKind::GuardTag, molt_ir::OpKind::GuardType] {
        for (tag, output, removable) in
            [(5, None, true), (1, None, false), (5, Some("alias"), false)]
        {
            let mut check = op_with(molt_ir::OpKind::CheckException, None, None, &[]);
            check.value = Some(9);
            let mut handler = op_with(molt_ir::OpKind::Label, None, None, &[]);
            handler.value = Some(9);
            let mut func = FunctionIR {
                name: "split_field_guard".into(),
                params: vec!["hay".into(), "sep".into(), "idx".into()],
                ops: vec![
                    make_const_int("tag", tag),
                    op_with(
                        molt_ir::OpKind::StringSplitField,
                        Some("field"),
                        None,
                        &["hay", "sep", "idx"],
                    ),
                    check,
                    op_with(kind, output, None, &["field", "tag"]),
                    op_with(molt_ir::OpKind::Len, Some("n"), None, &["field"]),
                    op_with(molt_ir::OpKind::RetVoid, None, None, &[]),
                    handler,
                    op_with(molt_ir::OpKind::RetVoid, None, None, &[]),
                ],
                return_abi: molt_ir::FunctionReturnAbi::Void,
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: vec![],
                execution_context: Default::default(),
            };
            let mut typed = crate::tir::lower_from_simple::lower_to_tir(&func);
            crate::tir::drop_phase::prepare_final_backend_lowering(
                &mut typed,
                &crate::tir::target_info::TargetInfo::native_release_fast(),
            );
            func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);
            assert_eq!(
                func.ops.iter().any(|op| op.kind == kind),
                !removable,
                "{kind}/{tag}/{output:?}"
            );
            assert_eq!(
                func.ops
                    .iter()
                    .any(|op| op.kind == molt_ir::OpKind::StringSplitField),
                !removable
            );
        }
    }
}
