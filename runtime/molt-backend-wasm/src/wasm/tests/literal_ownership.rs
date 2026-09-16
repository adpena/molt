use super::support::*;

fn compile_literal_body(params: Vec<&str>, ops: Vec<OpIR>) -> (Vec<String>, BTreeMap<String, u32>) {
    let ir = SimpleIR {
        functions: vec![wasm_test_function("molt_main", params, None, ops)],
        profile: None,
    };
    let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
    (
        wasm_operator_debug_for_export(&output.wasm, "molt_main"),
        wasm_function_import_indices(&output.wasm),
    )
}

fn call_count(operators: &[String], function_index: u32) -> usize {
    let call = format!("Call {{ function_index: {function_index} }}");
    operators
        .iter()
        .filter(|operator| operator.as_str() == call.as_str())
        .count()
}

pub(super) fn assert_every_return_releases_anchor(
    operators: &[String],
    dec_ref_index: u32,
    expected_minimum_returns: usize,
) -> usize {
    let release = format!("Call {{ function_index: {dec_ref_index} }}");
    let return_positions: Vec<usize> = operators
        .iter()
        .enumerate()
        .filter_map(|(index, operator)| (operator == "Return").then_some(index))
        .collect();
    assert!(
        return_positions.len() >= expected_minimum_returns,
        "expected at least {expected_minimum_returns} return paths; operators={operators:?}"
    );
    for &return_index in &return_positions {
        assert_eq!(
            operators.get(return_index.wrapping_sub(1)),
            Some(&release),
            "every anchored function return must release its unique anchor immediately before returning; operators={operators:?}"
        );
    }
    return_positions.len()
}

#[test]
fn jumpful_literals_share_one_anchor_and_mint_each_dynamic_result_owner() {
    let mut first = wasm_test_op("const_str", Some("first"), vec![]);
    first.s_value = Some("shared-payload".to_string());
    let mut branch = wasm_test_op("br_if", None, vec!["cond"]);
    branch.value = Some(7);
    let mut second = wasm_test_op("const_str", Some("second"), vec![]);
    second.s_value = Some("shared-payload".to_string());
    let mut label = wasm_test_op("label", None, vec![]);
    label.value = Some(7);

    let (operators, imports) = compile_literal_body(
        vec!["cond"],
        vec![
            first,
            wasm_test_op("dec_ref", None, vec!["first"]),
            branch,
            second,
            wasm_test_op("dec_ref", None, vec!["second"]),
            label,
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["string_from_bytes"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(
        call_count(&operators, imports["inc_ref_obj"]),
        2,
        "each original literal op must mint its own result owner from the shared anchor; operators={operators:?}"
    );
    assert_every_return_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}

#[test]
fn full_i64_const_uses_fallible_anchor_instead_of_inline_47_truncation() {
    let mut wide = wasm_test_op("const", Some("wide"), vec![]);
    wide.value = Some(i64::MAX);
    let (operators, imports) = compile_literal_body(
        vec![],
        vec![
            wide,
            wasm_test_op("dec_ref", None, vec!["wide"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["int_from_i64"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(call_count(&operators, imports["inc_ref_obj"]), 1);
    assert!(
        operators
            .iter()
            .any(|operator| operator == &format!("I64Const {{ value: {} }}", i64::MAX)),
        "full-width constant must reach int_from_i64 without a 47-bit mask; operators={operators:?}"
    );
    assert_every_return_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}
