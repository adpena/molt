#![cfg(feature = "wasm-backend")]

use molt_backend::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

#[test]
fn jumpful_else_without_end_if_does_not_panic() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut cond = op(molt_ir::OpKind::ConstBool);
    cond.value = Some(1);
    cond.out = Some("v0".to_string());
    ops.push(cond);

    let mut if_op = op(molt_ir::OpKind::If);
    if_op.args = Some(vec!["v0".to_string()]);
    ops.push(if_op);

    let mut one = op(molt_ir::OpKind::Const);
    one.value = Some(1);
    one.out = Some("v1".to_string());
    ops.push(one);

    // Intentionally malformed control metadata: ELSE with no END_IF.
    ops.push(op(molt_ir::OpKind::Else));
    ops.push(op(molt_ir::OpKind::RetVoid));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_jumpful_malformed_else".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
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
