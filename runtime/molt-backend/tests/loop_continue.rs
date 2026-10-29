use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

#[test]
fn loop_continue_in_if_compiles() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut const_start = op(molt_ir::OpKind::Const);
    const_start.value = Some(0);
    const_start.out = Some("v0".to_string());
    ops.push(const_start);

    let mut const_stop = op(molt_ir::OpKind::Const);
    const_stop.value = Some(3);
    const_stop.out = Some("v1".to_string());
    ops.push(const_stop);

    let mut const_step = op(molt_ir::OpKind::Const);
    const_step.value = Some(1);
    const_step.out = Some("v2".to_string());
    ops.push(const_step);

    ops.push(op(molt_ir::OpKind::LoopStart));
    let mut loop_index_start = op(molt_ir::OpKind::LoopIndexStart);
    loop_index_start.args = Some(vec!["v0".to_string()]);
    loop_index_start.out = Some("v3".to_string());
    ops.push(loop_index_start);

    let mut lt = op(molt_ir::OpKind::Lt);
    lt.args = Some(vec!["v3".to_string(), "v1".to_string()]);
    lt.out = Some("v4".to_string());
    ops.push(lt);

    let mut loop_break_if_false = op(molt_ir::OpKind::LoopBreakIfFalse);
    loop_break_if_false.args = Some(vec!["v4".to_string()]);
    ops.push(loop_break_if_false);

    let mut const_true = op(molt_ir::OpKind::ConstBool);
    const_true.value = Some(1);
    const_true.out = Some("v5".to_string());
    ops.push(const_true);

    let mut if_op = op(molt_ir::OpKind::If);
    if_op.args = Some(vec!["v5".to_string()]);
    ops.push(if_op);

    let mut add_continue = op(molt_ir::OpKind::Add);
    add_continue.args = Some(vec!["v3".to_string(), "v2".to_string()]);
    add_continue.out = Some("v6".to_string());
    ops.push(add_continue);

    let mut loop_index_next = op(molt_ir::OpKind::LoopIndexNext);
    loop_index_next.args = Some(vec!["v6".to_string()]);
    loop_index_next.out = Some("v3".to_string());
    ops.push(loop_index_next);

    ops.push(op(molt_ir::OpKind::LoopContinue));
    ops.push(op(molt_ir::OpKind::EndIf));

    let mut add_fallthrough = op(molt_ir::OpKind::Add);
    add_fallthrough.args = Some(vec!["v3".to_string(), "v2".to_string()]);
    add_fallthrough.out = Some("v7".to_string());
    ops.push(add_fallthrough);

    let mut loop_index_next_fallthrough = op(molt_ir::OpKind::LoopIndexNext);
    loop_index_next_fallthrough.args = Some(vec!["v7".to_string()]);
    loop_index_next_fallthrough.out = Some("v3".to_string());
    ops.push(loop_index_next_fallthrough);

    ops.push(op(molt_ir::OpKind::LoopContinue));
    ops.push(op(molt_ir::OpKind::LoopEnd));
    ops.push(op(molt_ir::OpKind::RetVoid));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_loop_continue".to_string(),
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
        // Standalone codegen object — not linked into a final binary, so it
        // must not emit the per-app callable resolver (which would require the
        // runtime staticlib's callable-symbol set and panic without it,
        // masking the control-flow behavior this test actually exercises).
        let mut backend = SimpleBackend::new();
        backend.emit_app_callable_resolver = false;
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok());
}
