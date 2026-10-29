use super::{
    elide_useless_try_blocks, elide_useless_try_blocks_for_function, rewrite_phi_to_store_load,
};
use crate::{FunctionIR, OpIR};

fn op_shapes(
    ops: &[OpIR],
) -> Vec<(
    molt_ir::OpKind,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<String>>,
)> {
    ops.iter()
        .map(|op| {
            (
                op.kind,
                op.value,
                op.out.clone(),
                op.var.clone(),
                op.s_value.clone(),
                op.args.clone(),
            )
        })
        .collect()
}

#[test]
fn try_elision_preserves_try_finally_cleanup_shape() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::ExceptionPush,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPush)
        },
        OpIR {
            kind: molt_ir::OpKind::TryStart,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryStart)
        },
        OpIR {
            kind: molt_ir::OpKind::Line,
            value: Some(1),
            ..OpIR::new(molt_ir::OpKind::Line)
        },
        OpIR {
            kind: molt_ir::OpKind::CheckException,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::CheckException)
        },
        OpIR {
            kind: molt_ir::OpKind::TryEnd,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryEnd)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::ConstNone,
            out: Some("finally_value".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstNone)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionPop,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPop)
        },
    ];

    let original = ops.clone();
    elide_useless_try_blocks(&mut ops);

    assert_eq!(
        op_shapes(&ops),
        op_shapes(&original),
        "try/finally must not use the try/except-only elision path"
    );
}

#[test]
fn try_except_elision_preserves_raising_bodies_and_removes_only_safe_checks() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::ExceptionPush,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPush)
        },
        OpIR {
            kind: molt_ir::OpKind::TryStart,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryStart)
        },
        OpIR {
            kind: molt_ir::OpKind::Const,
            value: Some(1),
            out: Some("x".to_string()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            kind: molt_ir::OpKind::CheckException,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::CheckException)
        },
        OpIR {
            kind: molt_ir::OpKind::StoreVar,
            var: Some("total".to_string()),
            args: Some(vec!["x".to_string()]),
            ..OpIR::new(molt_ir::OpKind::StoreVar)
        },
        OpIR {
            kind: molt_ir::OpKind::TryEnd,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryEnd)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionLast,
            out: Some("exc".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionLast)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionMatchBuiltin,
            value: Some(3),
            s_value: Some("ValueError".to_string()),
            args: Some(vec!["exc".to_string()]),
            out: Some("matched".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionMatchBuiltin)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionPop,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPop)
        },
    ];

    // Frame entry may initialize builtins and fail. A structurally owned
    // frontend check is not a proof that its producer cannot throw.
    let mut checked_entry = ops.clone();
    checked_entry[2] = OpIR {
        kind: molt_ir::OpKind::TraceEnterSlot,
        s_value: Some("checked_entry".into()),
        ..OpIR::new(molt_ir::OpKind::TraceEnterSlot)
    };
    checked_entry.remove(4); // The scalar-only fixture's store has no producer here.
    assert!(
        !crate::passes::simple_ir_op_is_provably_nonthrowing_with_facts(None, &checked_entry[2],)
    );
    let original_entry = checked_entry.clone();
    elide_useless_try_blocks(&mut checked_entry);
    assert_eq!(op_shapes(&checked_entry), op_shapes(&original_entry));

    elide_useless_try_blocks(&mut ops);

    let kinds: Vec<molt_ir::OpKind> = ops.iter().map(|op| op.kind).collect();
    assert_eq!(
        kinds,
        vec![molt_ir::OpKind::Const, molt_ir::OpKind::StoreVar]
    );
    assert!(
        ops.iter()
            .all(|op| !(op.kind == molt_ir::OpKind::CheckException && op.value == Some(10))),
        "eliding a safe try/except body must not leave stale handler checks: {ops:?}"
    );
}

#[test]
fn try_except_elision_keeps_transport_hinted_unknown_add() {
    let add = OpIR {
        kind: molt_ir::OpKind::Add,
        args: Some(vec!["left".to_string(), "right".to_string()]),
        out: Some("sum".to_string()),
        ..OpIR::new(molt_ir::OpKind::Add)
    };
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "transport_hint_try_body".to_string(),
        params: vec!["left".to_string(), "right".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
        ops: vec![
            OpIR {
                kind: molt_ir::OpKind::ExceptionPush,
                out: Some("none".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionPush)
            },
            OpIR {
                kind: molt_ir::OpKind::TryStart,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::TryStart)
            },
            add,
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some("result".to_string()),
                args: Some(vec!["sum".to_string()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
            OpIR {
                kind: molt_ir::OpKind::TryEnd,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::TryEnd)
            },
            OpIR {
                kind: molt_ir::OpKind::Jump,
                value: Some(11),
                ..OpIR::new(molt_ir::OpKind::Jump)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionLast,
                out: Some("exc".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionLast)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionMatchBuiltin,
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionMatchBuiltin)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(11),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionPop,
                out: Some("none".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionPop)
            },
        ],
    };

    elide_useless_try_blocks_for_function(
        &mut func,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );

    assert!(
        func.ops
            .iter()
            .any(|op| op.kind == molt_ir::OpKind::ExceptionPush)
            && func.ops.iter().any(|op| op.kind == molt_ir::OpKind::Add),
        "transport hints alone must not elide try/except around Python arithmetic: {:?}",
        func.ops
    );
}

#[test]
fn try_except_elision_uses_typed_int_body_without_transport_hints() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "typed_int_try_body".to_string(),
        params: vec!["left".to_string(), "right".to_string()],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
        ops: vec![
            OpIR {
                kind: molt_ir::OpKind::ExceptionPush,
                out: Some("none".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionPush)
            },
            OpIR {
                kind: molt_ir::OpKind::TryStart,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::TryStart)
            },
            OpIR {
                kind: molt_ir::OpKind::Add,
                args: Some(vec!["left".to_string(), "right".to_string()]),
                out: Some("sum".to_string()),
                ..OpIR::new(molt_ir::OpKind::Add)
            },
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some("result".to_string()),
                args: Some(vec!["sum".to_string()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
            OpIR {
                kind: molt_ir::OpKind::TryEnd,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::TryEnd)
            },
            OpIR {
                kind: molt_ir::OpKind::Jump,
                value: Some(11),
                ..OpIR::new(molt_ir::OpKind::Jump)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(10),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionLast,
                out: Some("exc".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionLast)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionMatchBuiltin,
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionMatchBuiltin)
            },
            OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(11),
                ..OpIR::new(molt_ir::OpKind::Label)
            },
            OpIR {
                kind: molt_ir::OpKind::ExceptionPop,
                out: Some("none".to_string()),
                ..OpIR::new(molt_ir::OpKind::ExceptionPop)
            },
        ],
    };

    elide_useless_try_blocks_for_function(
        &mut func,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );

    let kinds: Vec<molt_ir::OpKind> = func.ops.iter().map(|op| op.kind).collect();
    assert_eq!(kinds, vec![molt_ir::OpKind::Add, molt_ir::OpKind::StoreVar]);
}

#[test]
fn try_except_elision_aborts_when_body_branches_to_removed_wrapper_label() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::ExceptionPush,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPush)
        },
        OpIR {
            kind: molt_ir::OpKind::TryStart,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryStart)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::TryEnd,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::TryEnd)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(10),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionLast,
            out: Some("exc".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionLast)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionMatchBuiltin,
            value: Some(3),
            s_value: Some("ValueError".to_string()),
            args: Some(vec!["exc".to_string()]),
            out: Some("matched".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionMatchBuiltin)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(11),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::ExceptionPop,
            out: Some("none".to_string()),
            ..OpIR::new(molt_ir::OpKind::ExceptionPop)
        },
    ];

    let original = ops.clone();
    elide_useless_try_blocks(&mut ops);

    assert_eq!(
        op_shapes(&ops),
        op_shapes(&original),
        "try/except elision must be CFG-closed when wrapper-local labels are removed"
    );
}

#[test]
fn rewrite_phi_to_store_load_rewrites_merge_phi() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::ConstBool,
            value: Some(1),
            out: Some("cond".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstBool)
        },
        OpIR {
            kind: molt_ir::OpKind::If,
            args: Some(vec!["cond".to_string()]),
            ..OpIR::new(molt_ir::OpKind::If)
        },
        OpIR {
            kind: molt_ir::OpKind::Const,
            value: Some(1),
            out: Some("then_val".to_string()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            kind: molt_ir::OpKind::Else,
            ..OpIR::new(molt_ir::OpKind::Else)
        },
        OpIR {
            kind: molt_ir::OpKind::Const,
            value: Some(2),
            out: Some("else_val".to_string()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            kind: molt_ir::OpKind::EndIf,
            ..OpIR::new(molt_ir::OpKind::EndIf)
        },
        OpIR {
            kind: molt_ir::OpKind::Phi,
            out: Some("merged".to_string()),
            args: Some(vec!["then_val".to_string(), "else_val".to_string()]),
            ..OpIR::new(molt_ir::OpKind::Phi)
        },
        OpIR {
            kind: molt_ir::OpKind::Ret,
            args: Some(vec!["merged".to_string()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
    ];

    rewrite_phi_to_store_load(&mut ops);

    assert!(
        ops.iter().all(|op| op.kind != molt_ir::OpKind::Phi),
        "phi should be eliminated: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == molt_ir::OpKind::StoreVar
                && op.var.as_deref() == Some("_phi_merged")
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.len() == 1 && args[0] == "then_val")
        }),
        "then branch should store merged value"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == molt_ir::OpKind::StoreVar
                && op.var.as_deref() == Some("_phi_merged")
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.len() == 1 && args[0] == "else_val")
        }),
        "else branch should store merged value"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == molt_ir::OpKind::LoadVar
                && op.var.as_deref() == Some("_phi_merged")
                && op.out.as_deref() == Some("merged")
        }),
        "merged phi should become load_var"
    );
}
