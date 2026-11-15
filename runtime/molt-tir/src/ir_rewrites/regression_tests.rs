use super::rewrite_phi_to_store_load;
use crate::OpIR;

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
