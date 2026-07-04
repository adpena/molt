use super::super::*;

#[test]
fn preserved_copy_numeric_helpers_use_generated_fixed_runtime_selector() {
    let cases = [
        ("inplace_div", 2, "inplace_div"),
        ("inplace_floordiv", 2, "inplace_floordiv"),
        ("inplace_mod", 2, "inplace_mod"),
        ("inplace_pow", 2, "inplace_pow"),
        ("matmul", 2, "matmul"),
        ("inplace_matmul", 2, "inplace_matmul"),
        ("pow_mod", 3, "pow_mod"),
        ("round", 3, "round"),
        ("trunc", 1, "trunc"),
        ("string_eq", 2, "string_eq"),
        ("shl", 2, "lshift"),
        ("shr", 2, "rshift"),
        ("bit_not", 1, "invert"),
        ("unary_neg", 1, "neg"),
        ("unary_pos", 1, "pos"),
    ];

    for (original_kind, operand_count, runtime_call) in cases {
        let func =
            make_copy_original_kind_runtime_func(original_kind, original_kind, operand_count, true);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{original_kind} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{original_kind} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}
