use molt_backend::{FunctionIR, OpIR, SimpleIR, validate_simple_ir};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn test_func(name: &str, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: Vec::new(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
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
        functions: vec![test_func("molt_test_validate_ok", vec![c0, ret])],
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
        functions: vec![test_func("molt_test_validate_missing", vec![idx])],
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
        functions: vec![test_func(
            "molt_test_validate_dict_receiver_placeholder",
            vec![k, v, dict_set],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_accepts_fast_int_flags_on_arithmetic_ops() {
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

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_fast_int",
            vec![lhs, rhs, add],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_accepts_fast_int_flags_on_division_transport_ops() {
    let mut lhs = op("const");
    lhs.value = Some(7);
    lhs.out = Some("v0".to_string());

    let mut rhs = op("const");
    rhs.value = Some(3);
    rhs.out = Some("v1".to_string());

    let mut div = op("div");
    div.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    div.out = Some("v2".to_string());
    div.fast_int = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_fast_int_div",
            vec![lhs, rhs, div],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_rejects_param_type_arity_mismatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_param_types".to_string(),
            params: vec!["x".to_string()],
            ops: vec![op("ret_void")],
            param_types: Some(vec!["int".to_string(), "bool".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected param type arity rejection");
    assert!(err.contains("has 1 params but 2 param_types"));
}

#[test]
fn validate_simple_ir_rejects_conflicting_fast_scalar_flags() {
    let mut scalar = op("add");
    scalar.args = Some(vec!["lhs".to_string(), "rhs".to_string()]);
    scalar.out = Some("sum".to_string());
    scalar.fast_int = Some(true);
    scalar.fast_float = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_conflicting_scalar_flags".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![scalar],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected scalar flag rejection");
    assert!(err.contains("cannot set both fast_int and fast_float"));
}

#[test]
fn validate_simple_ir_rejects_fast_int_on_non_scalar_owner() {
    let mut call = op("call");
    call.s_value = Some("opaque".to_string());
    call.out = Some("v0".to_string());
    call.fast_int = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func("molt_test_validate_fast_int_owner", vec![call])],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected fast_int owner rejection");
    assert!(err.contains("does not own fast_int scalar specialization"));
}

#[test]
fn validate_simple_ir_rejects_unknown_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.container_type = Some("vectorish".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_container_type".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected container type rejection");
    assert!(err.contains("unsupported container_type `vectorish`"));
}

#[test]
fn validate_simple_ir_rejects_legacy_list_int_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.container_type = Some("list_int".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_legacy_list_int_container_type".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected list_int container type rejection");
    assert!(err.contains("unsupported container_type `list_int`"));
}

#[test]
fn validate_simple_ir_accepts_bce_safe_without_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.bce_safe = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_bce_container".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    validate_simple_ir(&ir).expect("bce_safe is an independent bounds proof");
}

#[test]
fn validate_simple_ir_rejects_arena_eligible_on_non_allocation() {
    let mut add = op("add");
    add.args = Some(vec!["lhs".to_string(), "rhs".to_string()]);
    add.out = Some("sum".to_string());
    add.arena_eligible = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_validate_arena_owner".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![add],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected arena owner rejection");
    assert!(err.contains("cannot carry arena_eligible"));
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
            test_func("molt_main", vec![main_runtime_init, main_init, main_ret]),
            test_func("molt_init___main__", vec![init_sys, user_call, init_ret]),
            test_func("molt_runtime_init", vec![helper_ret.clone()]),
            test_func("molt_init_sys", vec![helper_ret.clone()]),
            test_func("user_kernel", vec![user_ret]),
            test_func("unused_helper", vec![helper_ret]),
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
