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

#[test]
fn validate_simple_ir_rejects_unsupported_raw_int_kinds() {
    let mut lhs = op("const");
    lhs.value = Some(7);
    lhs.out = Some("v0".to_string());
    lhs.raw_int = Some(true);

    let mut rhs = op("const");
    rhs.value = Some(3);
    rhs.out = Some("v1".to_string());
    rhs.raw_int = Some(true);

    let mut sub = op("sub");
    sub.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    sub.out = Some("v2".to_string());
    sub.raw_int = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_raw_int_kind".to_string(),
            params: Vec::new(),
            ops: vec![lhs, rhs, sub],
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected raw_int kind rejection");
    assert!(err.contains("does not support `raw_int`"));
}

#[test]
fn validate_simple_ir_rejects_conflicting_specialized_int_flags() {
    let mut lhs = op("const");
    lhs.value = Some(7);
    lhs.out = Some("v0".to_string());

    let mut rhs = op("const");
    rhs.value = Some(3);
    rhs.out = Some("v1".to_string());

    let mut add = op("add");
    add.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    add.out = Some("v2".to_string());
    add.fast_int = Some(true);
    add.raw_int = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_specialized_int_conflict".to_string(),
            params: Vec::new(),
            ops: vec![lhs, rhs, add],
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected specialized-int conflict rejection");
    assert!(err.contains("cannot set both `fast_int` and `raw_int`"));
}

#[test]
fn validate_simple_ir_accepts_raw_loop_index_carriers() {
    let mut start = op("const");
    start.value = Some(0);
    start.out = Some("v0".to_string());
    start.raw_int = Some(true);

    let mut loop_index_start = op("loop_index_start");
    loop_index_start.args = Some(vec!["v0".to_string()]);
    loop_index_start.out = Some("idx".to_string());

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("v1".to_string());
    one.raw_int = Some(true);

    let mut add = op("add");
    add.args = Some(vec!["idx".to_string(), "v1".to_string()]);
    add.out = Some("idx_plus_1".to_string());
    add.raw_int = Some(true);

    let mut loop_index_next = op("loop_index_next");
    loop_index_next.args = Some(vec!["idx_plus_1".to_string()]);
    loop_index_next.out = Some("idx2".to_string());

    let mut stop = op("const");
    stop.value = Some(10);
    stop.out = Some("v2".to_string());
    stop.raw_int = Some(true);

    let mut lt = op("lt");
    lt.args = Some(vec!["idx2".to_string(), "v2".to_string()]);
    lt.out = Some("cmp".to_string());
    lt.raw_int = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_raw_loop_index_carriers".to_string(),
            params: Vec::new(),
            ops: vec![start, loop_index_start, one, add, loop_index_next, stop, lt],
        }],
        profile: None,
    };

    assert!(validate_simple_ir(&ir).is_ok());
}
