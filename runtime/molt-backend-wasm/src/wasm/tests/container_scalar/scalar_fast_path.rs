use super::*;

fn wasm_const_int_op(out: &str, value: i64) -> OpIR {
    OpIR {
        kind: "const".to_string(),
        out: Some(out.to_string()),
        value: Some(value),
        ..OpIR::default()
    }
}

fn wasm_copy_var_op(out: &str, var: &str) -> OpIR {
    let mut copy = wasm_test_op("copy_var", Some(out), Vec::<&str>::new());
    copy.var = Some(var.to_string());
    copy
}

fn wasm_store_var_op(var: &str, value: &str) -> OpIR {
    let mut store = wasm_test_op("store_var", None, vec![value]);
    store.var = Some(var.to_string());
    store
}

fn wasm_load_var_op(out: &str, var: &str) -> OpIR {
    let mut load = wasm_test_op("load_var", Some(out), Vec::<&str>::new());
    load.var = Some(var.to_string());
    load
}

fn wasm_ret_op(name: &str) -> OpIR {
    let mut ret = wasm_test_op("ret", None, vec![name]);
    ret.args = Some(vec![name.to_string()]);
    ret
}

fn compile_final_numeric_function_with_diagnostics(
    params: Vec<&str>,
    param_types: Option<Vec<&str>>,
    ops: Vec<OpIR>,
) -> WasmCompileOutput {
    let ir = SimpleIR {
        functions: vec![wasm_test_function("molt_main", params, param_types, ops)],
        profile: None,
    };
    wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir)
}

fn compile_final_numeric_ops_with_diagnostics(ops: Vec<OpIR>) -> WasmCompileOutput {
    compile_final_numeric_function_with_diagnostics(vec![], None, ops)
}

fn assert_no_direct_call_to_import(wasm: &[u8], import_name: &str) {
    let import_indices = wasm_function_import_indices(wasm);
    if let Some(import_index) = import_indices.get(import_name) {
        let call_indices = wasm_direct_call_indices_for_export(wasm, "molt_main");
        assert!(
            !call_indices.contains(import_index),
            "{import_name} must not be called from proven direct numeric path; calls={call_indices:?}"
        );
    }
}

fn assert_molt_main_has_operator(wasm: &[u8], expected: &str) {
    let operators = wasm_operator_debug_for_export(wasm, "molt_main");
    assert!(
        operators.iter().any(|op| op == expected),
        "molt_main must contain {expected}; operators={operators:?}"
    );
}

#[test]
fn scalar_fast_path_ignores_transport_hints() {
    let mut add = wasm_test_op("add", Some("sum"), vec!["lhs", "rhs"]);
    add.fast_int = Some(true);
    add.type_hint = Some("int".to_string());
    let func = wasm_test_function("hinted", vec!["lhs", "rhs"], None, vec![add.clone()]);
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(!wasm_scalar_integer_fast_path_for_op(&plan, &add));
}

#[test]
fn scalar_fast_path_uses_typed_operands_without_transport_hints() {
    let add = wasm_test_op("add", Some("sum"), vec!["lhs", "rhs"]);
    let mul = wasm_test_op("mul", Some("product"), vec!["lhs", "rhs"]);
    let div = wasm_test_op("div", Some("quot"), vec!["lhs", "rhs"]);
    let func = wasm_test_function(
        "typed",
        vec!["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![add.clone(), mul.clone(), div.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &add));
    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &mul));
    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &div));
    assert!(wasm_scalar_truthiness_fast_path_for_name(&plan, "lhs"));
}

#[test]
fn proven_inline_int_add_lowers_without_boxed_add_call() {
    let add = wasm_test_op("add", Some("i_next"), vec!["i_cur", "one"]);
    let output = compile_final_numeric_ops_with_diagnostics(vec![
        wasm_const_int_op("init", 0),
        wasm_const_int_op("one", 1),
        wasm_const_int_op("stop", 1_000_000),
        wasm_store_var_op("i", "init"),
        wasm_test_op("loop_start", None, Vec::<&str>::new()),
        wasm_load_var_op("i_cur", "i"),
        wasm_test_op("lt", Some("keep_going"), vec!["i_cur", "stop"]),
        wasm_test_op("loop_break_if_false", None, vec!["keep_going"]),
        add,
        wasm_store_var_op("i", "i_next"),
        wasm_test_op("loop_continue", None, Vec::<&str>::new()),
        wasm_test_op("loop_end", None, Vec::<&str>::new()),
        wasm_load_var_op("i_after", "i"),
        wasm_ret_op("i_after"),
    ]);

    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .expect("valid wasm");
    assert_molt_main_has_operator(&output.wasm, "I64Add");
    assert_no_direct_call_to_import(&output.wasm, "add");
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_inline_int_raw_sites,
        1
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_boxed_runtime_sites,
        0
    );
}

#[test]
fn proven_float_add_lowers_without_boxed_add_call() {
    let add = wasm_test_op("add", Some("sum"), vec!["lhs", "rhs"]);
    let output = compile_final_numeric_function_with_diagnostics(
        vec!["param_lhs", "param_rhs"],
        Some(vec!["float", "float"]),
        vec![
            wasm_copy_var_op("lhs", "param_lhs"),
            wasm_copy_var_op("rhs", "param_rhs"),
            add,
            wasm_ret_op("sum"),
        ],
    );

    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .expect("valid wasm");
    assert_molt_main_has_operator(&output.wasm, "F64Add");
    assert_no_direct_call_to_import(&output.wasm, "add");
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_float_raw_sites,
        1
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_boxed_runtime_sites,
        0
    );
}

#[test]
fn typed_int_floor_div_records_division_guarded_site() {
    let floor_div = wasm_test_op("floordiv", Some("quot"), vec!["lhs", "rhs"]);
    let output = compile_final_numeric_function_with_diagnostics(
        vec!["param_lhs", "param_rhs"],
        Some(vec!["int", "int"]),
        vec![
            wasm_copy_var_op("lhs", "param_lhs"),
            wasm_copy_var_op("rhs", "param_rhs"),
            floor_div,
            wasm_ret_op("quot"),
        ],
    );

    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .expect("valid wasm");
    assert_molt_main_has_operator(&output.wasm, "I64DivS");
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_division_guarded_int_sites,
        1
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_division_boxed_runtime_sites,
        0
    );
}

#[test]
fn proven_inline_int_bitwise_lowers_without_boxed_bit_or_call() {
    let bit_or = wasm_test_op("bit_or", Some("masked"), vec!["i_cur", "one"]);
    let add = wasm_test_op("add", Some("i_next"), vec!["i_cur", "one"]);
    let output = compile_final_numeric_ops_with_diagnostics(vec![
        wasm_const_int_op("init", 0),
        wasm_const_int_op("one", 1),
        wasm_const_int_op("stop", 10),
        wasm_store_var_op("i", "init"),
        wasm_store_var_op("last_mask", "init"),
        wasm_test_op("loop_start", None, Vec::<&str>::new()),
        wasm_load_var_op("i_cur", "i"),
        wasm_test_op("lt", Some("keep_going"), vec!["i_cur", "stop"]),
        wasm_test_op("loop_break_if_false", None, vec!["keep_going"]),
        bit_or,
        wasm_store_var_op("last_mask", "masked"),
        add,
        wasm_store_var_op("i", "i_next"),
        wasm_test_op("loop_continue", None, Vec::<&str>::new()),
        wasm_test_op("loop_end", None, Vec::<&str>::new()),
        wasm_load_var_op("out", "last_mask"),
        wasm_ret_op("out"),
    ]);

    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .expect("valid wasm");
    assert_molt_main_has_operator(&output.wasm, "I64Or");
    assert_no_direct_call_to_import(&output.wasm, "bit_or");
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_bitwise_inline_int_raw_sites,
        1
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_inline_int_raw_sites,
        1
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_bitwise_boxed_runtime_sites,
        0
    );
    assert_eq!(
        output
            .diagnostics
            .numeric_lanes
            .op_loop_additive_boxed_runtime_sites,
        0
    );
}

#[test]
fn scalar_fast_path_keeps_list_repeat_on_runtime_mul() {
    let list_new = wasm_test_op("list_new", Some("items"), vec!["item"]);
    let repeat = wasm_test_op("mul", Some("repeated"), vec!["items", "count"]);
    let func = wasm_test_function(
        "list_repeat",
        vec!["item", "count"],
        Some(vec!["bool", "int"]),
        vec![list_new, repeat.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(!wasm_scalar_integer_fast_path_for_op(&plan, &repeat));
}
