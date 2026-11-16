use super::*;

#[test]
fn exception_elision_requires_clean_fallthrough_and_generated_nothrow_facts() {
    let check = || OpIR {
        kind: molt_ir::OpKind::CheckException,
        value: Some(100),
        ..OpIR::new(molt_ir::OpKind::CheckException)
    };
    for kind in [
        molt_ir::OpKind::ConstStr,
        molt_ir::OpKind::ConstBytes,
        molt_ir::OpKind::ConstBigint,
        molt_ir::OpKind::Box,
        molt_ir::OpKind::BoxFromRawInt,
        molt_ir::OpKind::CallFunc,
        molt_ir::OpKind::TraceEnterSlot,
        molt_ir::OpKind::Label,
        molt_ir::OpKind::StateLabel,
    ] {
        let mut candidate = make_op(kind);
        if kind == molt_ir::OpKind::TraceEnterSlot {
            candidate.s_value = Some("m".into());
        }
        let mut func = manifest_func(vec![
            check(),
            candidate,
            make_op(molt_ir::OpKind::ConstNone),
            check(),
        ]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops
                .iter()
                .filter(|op| op.kind == molt_ir::OpKind::CheckException)
                .count(),
            2,
            "a pure trailing operation cannot hide pending errors or a join after {kind}"
        );
    }
    for kind in [
        molt_ir::OpKind::ConstNone,
        molt_ir::OpKind::ConstBool,
        molt_ir::OpKind::ConstFloat,
        molt_ir::OpKind::IncRef,
        molt_ir::OpKind::DecRef,
    ] {
        let mut func = manifest_func(vec![check(), make_op(kind), check()]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            2,
            "clean {kind} fallthrough needs no repeated check"
        );
    }
    for value in [0, i64::MIN, i64::MAX] {
        let mut func = manifest_func(vec![check(), make_const_int("v", value), check()]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(func.ops.len(), if value == 0 { 2 } else { 3 });
    }
}

#[test]
fn exception_elision_preserves_polling_observers_and_unproven_entry_state() {
    for (kind, poll, out, target) in [
        (molt_ir::OpKind::CheckException, true, None, Some(100)),
        (molt_ir::OpKind::AsyncWorkPoll, false, None, Some(100)),
        (
            molt_ir::OpKind::CheckException,
            false,
            Some("observed"),
            Some(100),
        ),
        (molt_ir::OpKind::CheckException, false, None, None),
    ] {
        let mut func = manifest_func(vec![
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(100),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            make_op(molt_ir::OpKind::ConstNone),
            OpIR {
                kind,
                async_work_poll: poll,
                out: out.map(str::to_string),
                value: target,
                ..OpIR::new(kind)
            },
        ]);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            3,
            "observer/polling semantics are not redundant"
        );
    }
    let mut func = manifest_func(vec![
        make_op(molt_ir::OpKind::ConstNone),
        OpIR {
            kind: molt_ir::OpKind::CheckException,
            value: Some(100),
            ..OpIR::new(molt_ir::OpKind::CheckException)
        },
    ]);
    elide_safe_exception_checks(&mut func);
    assert_eq!(
        func.ops.len(),
        2,
        "a constant does not establish clean entry state"
    );
}

#[test]
fn untargeted_observers_cannot_hide_pending_failures_from_later_checks() {
    for (kind, poll, literal) in [
        (
            molt_ir::OpKind::CheckException,
            false,
            Some(molt_ir::OpKind::ConstStr),
        ),
        (
            molt_ir::OpKind::CheckException,
            false,
            Some(molt_ir::OpKind::ConstBytes),
        ),
        (
            molt_ir::OpKind::CheckException,
            false,
            Some(molt_ir::OpKind::ConstBigint),
        ),
        (molt_ir::OpKind::CheckException, true, None),
        (molt_ir::OpKind::AsyncWorkPoll, false, None),
    ] {
        let check = || OpIR {
            kind: molt_ir::OpKind::CheckException,
            value: Some(100),
            ..OpIR::new(molt_ir::OpKind::CheckException)
        };
        let mut ops = vec![check()];
        if let Some(literal) = literal {
            ops.push(make_op(literal));
        }
        ops.push(OpIR {
            kind,
            async_work_poll: poll,
            ..OpIR::new(kind)
        });
        ops.push(check());
        let expected_len = ops.len();
        let mut func = manifest_func(ops);
        elide_safe_exception_checks(&mut func);
        assert_eq!(
            func.ops.len(),
            expected_len,
            "untargeted {kind} poll={poll} must preserve the final failure transfer"
        );
        assert_eq!(func.ops.last().unwrap().value, Some(100));
    }
}

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
