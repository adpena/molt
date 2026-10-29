use super::super::*;

#[test]
fn preserved_copy_numeric_helpers_use_generated_fixed_runtime_selector() {
    let cases = [
        (molt_ir::OpKind::InplaceDiv, 2, "inplace_div"),
        (molt_ir::OpKind::InplaceFloordiv, 2, "inplace_floordiv"),
        (molt_ir::OpKind::InplaceMod, 2, "inplace_mod"),
        (molt_ir::OpKind::InplacePow, 2, "inplace_pow"),
        (molt_ir::OpKind::Matmul, 2, "matmul"),
        (molt_ir::OpKind::InplaceMatmul, 2, "inplace_matmul"),
        (molt_ir::OpKind::PowMod, 3, "pow_mod"),
        (molt_ir::OpKind::Round, 3, "round"),
        (molt_ir::OpKind::Trunc, 1, "trunc"),
        (molt_ir::OpKind::StringEq, 2, "string_eq"),
        (molt_ir::OpKind::Shl, 2, "lshift"),
        (molt_ir::OpKind::Shr, 2, "rshift"),
        (molt_ir::OpKind::BitNot, 1, "invert"),
        (molt_ir::OpKind::UnaryNeg, 1, "neg"),
        (molt_ir::OpKind::UnaryPos, 1, "pos"),
    ];

    for (original_kind, operand_count, runtime_call) in cases {
        let func = make_copy_original_kind_runtime_func(
            original_kind.as_str(),
            original_kind,
            operand_count,
            true,
        );
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
