use super::*;

#[test]
fn native_backend_compiles_float_primary_tuple_escape_before_exception_cleanup() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
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
                    var: Some("loads".to_string()),
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
                    var: Some("none_ret".to_string()),
                    args: Some(vec!["none_ret".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

fn compile_retained_alias_after_source_dec_ref(alias_kind: &str) {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: format!("{alias_kind}_after_dec_ref"),
            params: vec![],
            ops: vec![
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
                OpIR {
                    kind: "dec_ref".to_string(),
                    args: Some(vec!["src".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("alias".to_string()),
                    args: Some(vec!["alias".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn native_backend_compiles_identity_alias_after_source_dec_ref() {
    compile_retained_alias_after_source_dec_ref("identity_alias");
}

#[test]
fn native_backend_compiles_binding_alias_after_source_dec_ref() {
    compile_retained_alias_after_source_dec_ref("binding_alias");
}
