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

    let output = SimpleBackend::new().compile(ir);
    assert!(!output.bytes.is_empty());
    assert!(
        output.trap_stub_names.is_empty(),
        "unexpected trap stubs: {:?}",
        output.trap_stub_names
    );
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
            ops: vec![
                cond, if_op, then_val, else_op, else_val, end_if, phi, ret_joined,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);
    assert!(!output.bytes.is_empty());
    assert!(
        output.trap_stub_names.is_empty(),
        "unexpected trap stubs: {:?}",
        output.trap_stub_names
    );
}

#[test]
fn nested_structured_if_phi_merges_compile() {
    let mut outer_cond = op("const_bool");
    outer_cond.value = Some(1);
    outer_cond.out = Some("outer_cond".to_string());

    let mut inner_cond = op("const_bool");
    inner_cond.value = Some(1);
    inner_cond.out = Some("inner_cond".to_string());

    let mut base = op("const");
    base.value = Some(0);
    base.out = Some("base".to_string());

    let mut outer_if = op("if");
    outer_if.args = Some(vec!["outer_cond".to_string()]);

    let mut inner_if = op("if");
    inner_if.args = Some(vec!["inner_cond".to_string()]);

    let mut inner_then = op("const");
    inner_then.value = Some(1);
    inner_then.out = Some("inner_then".to_string());

    let inner_else = op("else");

    let mut inner_else_val = op("const");
    inner_else_val.value = Some(2);
    inner_else_val.out = Some("inner_else".to_string());

    let inner_end_if = op("end_if");

    let mut inner_phi = op("phi");
    inner_phi.out = Some("outer_then".to_string());
    inner_phi.args = Some(vec!["inner_then".to_string(), "inner_else".to_string()]);

    let outer_end_if = op("end_if");

    let mut outer_phi = op("phi");
    outer_phi.out = Some("joined".to_string());
    outer_phi.args = Some(vec!["outer_then".to_string(), "base".to_string()]);

    let mut ret_joined = op("ret");
    ret_joined.var = Some("joined".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "nested_structured_if_phi_regression".to_string(),
            params: Vec::new(),
            ops: vec![
                outer_cond,
                inner_cond,
                base,
                outer_if,
                inner_if,
                inner_then,
                inner_else,
                inner_else_val,
                inner_end_if,
                inner_phi,
                outer_end_if,
                outer_phi,
                ret_joined,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);
    assert!(!output.bytes.is_empty());
    assert!(
        output.trap_stub_names.is_empty(),
        "unexpected trap stubs: {:?}",
        output.trap_stub_names
    );
}
