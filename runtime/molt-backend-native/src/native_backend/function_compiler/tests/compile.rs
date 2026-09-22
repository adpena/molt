use super::*;

#[test]
fn native_backend_compiles_float_primary_tuple_escape_before_exception_cleanup() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "float_primary_tuple_cleanup".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_a".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_a".to_string()),
                    args: Some(vec!["src_a".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_b".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_b".to_string()),
                    args: Some(vec!["src_b".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("src_c".to_string()),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "float_from_obj".to_string(),
                    out: Some("flt_c".to_string()),
                    args: Some(vec!["src_c".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    out: Some("loads".to_string()),
                    args: Some(vec![
                        "flt_a".to_string(),
                        "flt_b".to_string(),
                        "flt_c".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["loads".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("none_ret".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["none_ret".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

fn compile_alias_owner_transfer(alias_kind: &str, release_source: bool) {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: format!("{alias_kind}_owner_transfer"),
            params: vec![],
            ops: {
                let mut ops = vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("src".to_string()),
                        s_value: Some("owned".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: alias_kind.to_string(),
                        args: Some(vec!["src".to_string()]),
                        out: Some("alias".to_string()),
                        ..OpIR::default()
                    },
                ];
                if release_source {
                    ops.push(OpIR {
                        kind: "dec_ref".to_string(),
                        args: Some(vec!["src".to_string()]),
                        ..OpIR::default()
                    });
                }
                ops.push(OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["alias".to_string()]),
                    ..OpIR::default()
                });
                ops
            },
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn native_backend_compiles_identity_alias_transferring_the_source_owner() {
    // A transparent identity has no independent retain. Its return transfers
    // the source owner; releasing that owner first would be use-after-free.
    compile_alias_owner_transfer("identity_alias", false);
}

#[test]
fn native_backend_compiles_binding_alias_after_source_dec_ref() {
    compile_alias_owner_transfer("binding_alias", true);
}
