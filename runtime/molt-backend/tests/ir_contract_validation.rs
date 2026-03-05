use molt_backend::{FunctionIR, OpIR, SimpleIR, validate_simple_ir};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        value: None,
        f_value: None,
        s_value: None,
        bytes: None,
        var: None,
        args: None,
        out: None,
        fast_int: None,
        task_kind: None,
        container_type: None,
        stack_eligible: None,
        fast_float: None,
        raw_int: None,
        type_hint: None,
    }
}

#[test]
fn validate_simple_ir_accepts_well_formed_value_uses() {
    let mut c0 = op("const");
    c0.value = Some(1);
    c0.out = Some("v0".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["v0".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_ok".to_string(),
            params: Vec::new(),
            ops: vec![c0, ret],
        }],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_rejects_missing_value_definition() {
    let mut idx = op("index");
    idx.args = Some(vec!["v0".to_string(), "v9999".to_string()]);
    idx.out = Some("v1".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_missing".to_string(),
            params: Vec::new(),
            ops: vec![idx],
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected undefined value rejection");
    assert!(err.contains("uses undefined value `v9999`"));
}

#[test]
fn validate_simple_ir_allows_dict_receiver_merge_placeholders() {
    let mut k = op("const_str");
    k.s_value = Some("key".to_string());
    k.out = Some("v0".to_string());

    let mut v = op("const");
    v.value = Some(1);
    v.out = Some("v1".to_string());

    let mut dict_set = op("dict_set");
    dict_set.args = Some(vec![
        "v9999".to_string(),
        "v0".to_string(),
        "v1".to_string(),
    ]);
    dict_set.out = Some("none".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_dict_receiver_placeholder".to_string(),
            params: Vec::new(),
            ops: vec![k, v, dict_set],
        }],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}
