use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

#[test]
fn entry_block_params_compile_with_int_shadow_targets() {
    let mut const_one = op("const");
    const_one.value = Some(1);
    const_one.out = Some("tmp".to_string());

    let mut store_slot = op("store_var");
    store_slot.var = Some("loop_slot".to_string());
    store_slot.args = Some(vec!["tmp".to_string()]);

    let mut ret_arg = op("ret");
    ret_arg.var = Some("arg".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "entry_param_shadow_regression".to_string(),
            params: vec!["arg".to_string()],
            ops: vec![const_one, store_slot, ret_arg],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let bytes = SimpleBackend::new().compile(ir);
    assert!(!bytes.is_empty());
}

#[test]
fn structured_if_phi_merges_compile() {
    let mut cond = op("const_bool");
    cond.value = Some(1);
    cond.out = Some("cond".to_string());

    let mut if_op = op("if");
    if_op.args = Some(vec!["cond".to_string()]);

    let mut then_val = op("const");
    then_val.value = Some(1);
    then_val.out = Some("then_val".to_string());

    let else_op = op("else");

    let mut else_val = op("const");
    else_val.value = Some(2);
    else_val.out = Some("else_val".to_string());

    let end_if = op("end_if");

    let mut phi = op("phi");
    phi.out = Some("joined".to_string());
    phi.args = Some(vec!["then_val".to_string(), "else_val".to_string()]);

    let mut ret_joined = op("ret");
    ret_joined.var = Some("joined".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "structured_if_phi_regression".to_string(),
            params: Vec::new(),
            ops: vec![cond, if_op, then_val, else_op, else_val, end_if, phi, ret_joined],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let bytes = SimpleBackend::new().compile(ir);
    assert!(!bytes.is_empty());
}
