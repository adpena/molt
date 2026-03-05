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

fn single_func_ir(op: OpIR, params: Vec<&str>) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_schema_scaffold".to_string(),
            params: params.into_iter().map(str::to_string).collect(),
            ops: vec![op],
        }],
        profile: None,
    }
}

#[test]
fn validate_simple_ir_rejects_list_repeat_range_missing_out() {
    let mut repeat = op("list_repeat_range");
    repeat.args = Some(vec![
        "v0".to_string(),
        "v1".to_string(),
        "v2".to_string(),
        "v3".to_string(),
    ]);
    let ir = single_func_ir(repeat, vec!["v0", "v1", "v2", "v3"]);
    let err = validate_simple_ir(&ir).expect_err("expected missing out rejection");
    assert!(err.contains("requires non-`none` `out` destination"));
}

#[test]
fn validate_simple_ir_rejects_bytearray_fill_range_wrong_arity() {
    let mut fill = op("bytearray_fill_range");
    fill.args = Some(vec!["v0".to_string(), "v1".to_string(), "v2".to_string()]);
    fill.out = Some("v4".to_string());
    let ir = single_func_ir(fill, vec!["v0", "v1", "v2"]);
    let err = validate_simple_ir(&ir).expect_err("expected args arity rejection");
    assert!(err.contains("requires `args` length 4"));
}

#[test]
fn validate_simple_ir_accepts_range_fill_family_when_shape_is_valid() {
    let mut repeat = op("list_repeat_range");
    repeat.args = Some(vec![
        "v0".to_string(),
        "v1".to_string(),
        "v2".to_string(),
        "v3".to_string(),
    ]);
    repeat.out = Some("v4".to_string());
    let ir = single_func_ir(repeat, vec!["v0", "v1", "v2", "v3"]);
    assert!(validate_simple_ir(&ir).is_ok());
}
