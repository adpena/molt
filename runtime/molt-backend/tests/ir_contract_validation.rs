use molt_backend::{FunctionIR, OpIR, SimpleIR, validate_simple_ir};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
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
            param_types: None,
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
            param_types: None,
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
            param_types: None,
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
            param_types: None,
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
            param_types: None,
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
            param_types: None,
        }],
        profile: None,
    };

    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn tree_shake_luau_rewrites_main_and_drops_runtime_bootstrap_helpers() {
    let mut main_runtime_init = op("call");
    main_runtime_init.s_value = Some("molt_runtime_init".to_string());
    main_runtime_init.out = Some("v0".to_string());

    let mut main_init = op("call");
    main_init.s_value = Some("molt_init___main__".to_string());
    main_init.out = Some("v1".to_string());

    let main_ret = op("ret_void");

    let mut init_sys = op("call");
    init_sys.s_value = Some("molt_init_sys".to_string());
    init_sys.out = Some("v2".to_string());

    let mut user_call = op("call");
    user_call.s_value = Some("user_kernel".to_string());
    user_call.out = Some("v3".to_string());

    let init_ret = op("ret_void");

    let user_ret = op("ret_void");
    let helper_ret = op("ret_void");

    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                ops: vec![main_runtime_init, main_init, main_ret],
                param_types: None,
            },
            FunctionIR {
                name: "molt_init___main__".to_string(),
                params: Vec::new(),
                ops: vec![init_sys, user_call, init_ret],
                param_types: None,
            },
            FunctionIR {
                name: "molt_runtime_init".to_string(),
                params: Vec::new(),
                ops: vec![helper_ret.clone()],
                param_types: None,
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: Vec::new(),
                ops: vec![helper_ret.clone()],
                param_types: None,
            },
            FunctionIR {
                name: "user_kernel".to_string(),
                params: Vec::new(),
                ops: vec![user_ret],
                param_types: None,
            },
            FunctionIR {
                name: "unused_helper".to_string(),
                params: Vec::new(),
                ops: vec![helper_ret],
                param_types: None,
            },
        ],
        profile: None,
    };

    ir.tree_shake_luau();

    let names = ir
        .functions
        .iter()
        .map(|func| func.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["molt_main", "molt_init___main__", "user_kernel"]
    );

    let main = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_main")
        .expect("molt_main should remain");
    assert_eq!(main.ops.len(), 2);
    assert_eq!(main.ops[0].kind, "call");
    assert_eq!(main.ops[0].s_value.as_deref(), Some("molt_init___main__"));
    assert_eq!(main.ops[1].kind, "ret_void");

    let init_main = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_init___main__")
        .expect("molt_init___main__ should remain");
    assert_eq!(init_main.ops[0].kind, "nop");
    assert_eq!(init_main.ops[1].kind, "call");
    assert_eq!(init_main.ops[1].s_value.as_deref(), Some("user_kernel"));
}
