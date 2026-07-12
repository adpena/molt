use super::super::*;

#[test]
fn generic_tir_to_wasm_uses_value_repr_not_type_floor_for_int_params() {
    let func = make_add_two_params_func();
    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.runtime_calls.contains(&"add"),
        "unproven int params must lower through boxed runtime dispatch, not a type-floor raw i64 op"
    );
    for (idx, inst) in output.instructions.iter().enumerate() {
        if matches!(inst, Instruction::I64Add) {
            assert!(
                matches!(
                    output.instructions.get(idx + 1),
                    Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                ),
                "generic lower_tir_to_wasm emitted a bare operand i64.add at {idx}"
            );
        }
    }
}
