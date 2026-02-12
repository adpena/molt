use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};

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
    }
}

#[test]
fn jumpful_else_without_end_if_does_not_panic() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut cond = op("const_bool");
    cond.value = Some(1);
    cond.out = Some("v0".to_string());
    ops.push(cond);

    let mut if_op = op("if");
    if_op.args = Some(vec!["v0".to_string()]);
    ops.push(if_op);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("v1".to_string());
    ops.push(one);

    // Intentionally malformed control metadata: ELSE with no END_IF.
    ops.push(op("else"));
    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_jumpful_malformed_else".to_string(),
            params: Vec::new(),
            ops,
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        let backend = WasmBackend::new();
        let wasm = backend.compile(ir);
        assert!(!wasm.is_empty());
    });
    assert!(result.is_ok());
}
