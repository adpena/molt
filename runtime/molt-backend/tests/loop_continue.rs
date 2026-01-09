use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

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
    }
}

#[test]
fn loop_continue_in_if_compiles() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut const_start = op("const");
    const_start.value = Some(0);
    const_start.out = Some("v0".to_string());
    ops.push(const_start);

    let mut const_stop = op("const");
    const_stop.value = Some(3);
    const_stop.out = Some("v1".to_string());
    ops.push(const_stop);

    let mut const_step = op("const");
    const_step.value = Some(1);
    const_step.out = Some("v2".to_string());
    ops.push(const_step);

    let mut loop_index_start = op("loop_index_start");
    loop_index_start.args = Some(vec!["v0".to_string()]);
    loop_index_start.out = Some("v3".to_string());
    ops.push(loop_index_start);

    let mut lt = op("lt");
    lt.args = Some(vec!["v3".to_string(), "v1".to_string()]);
    lt.out = Some("v4".to_string());
    ops.push(lt);

    let mut loop_break_if_false = op("loop_break_if_false");
    loop_break_if_false.args = Some(vec!["v4".to_string()]);
    ops.push(loop_break_if_false);

    let mut const_true = op("const_bool");
    const_true.value = Some(1);
    const_true.out = Some("v5".to_string());
    ops.push(const_true);

    let mut if_op = op("if");
    if_op.args = Some(vec!["v5".to_string()]);
    ops.push(if_op);

    let mut add_continue = op("add");
    add_continue.args = Some(vec!["v3".to_string(), "v2".to_string()]);
    add_continue.out = Some("v6".to_string());
    ops.push(add_continue);

    let mut loop_index_next = op("loop_index_next");
    loop_index_next.args = Some(vec!["v6".to_string()]);
    loop_index_next.out = Some("v6".to_string());
    ops.push(loop_index_next);

    ops.push(op("loop_continue"));
    ops.push(op("end_if"));

    let mut add_fallthrough = op("add");
    add_fallthrough.args = Some(vec!["v3".to_string(), "v2".to_string()]);
    add_fallthrough.out = Some("v7".to_string());
    ops.push(add_fallthrough);

    let mut loop_index_next_fallthrough = op("loop_index_next");
    loop_index_next_fallthrough.args = Some(vec!["v7".to_string()]);
    loop_index_next_fallthrough.out = Some("v7".to_string());
    ops.push(loop_index_next_fallthrough);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));
    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_loop_continue".to_string(),
            params: Vec::new(),
            ops,
        }],
    };

    let result = std::panic::catch_unwind(|| {
        let backend = SimpleBackend::new();
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok());
}
