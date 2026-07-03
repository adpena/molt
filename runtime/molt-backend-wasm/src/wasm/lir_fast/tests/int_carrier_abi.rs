use super::*;

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

/// Full-range raw carriers must box through the OVERFLOW-SAFE path: a
/// full-range raw value without an inline-window range proof (the CheckedAdd
/// sum / overflow_peel accumulator case) boxed at a runtime-call or
/// return site must emit the fits-check + named `int_from_i64` cold
/// call, never the bare 47-bit mask (which truncates mod 2^47).
#[test]
fn full_range_raw_carrier_boxes_overflow_safe_with_named_call() {
    let func = make_add_two_params_func();
    // The values are raw full-deopt carriers, and the value-range proof for
    // opaque params does not prove the 47-bit inline window. The checked
    // triple is therefore refused and the add takes the boxed runtime path,
    // boxing both raw operands through the overflow-safe cold call.
    let repr: HashMap<ValueId, Repr> = HashMap::from([
        (ValueId(0), Repr::RawI64FullDeopt),
        (ValueId(1), Repr::RawI64FullDeopt),
        (ValueId(2), Repr::RawI64FullDeopt),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = crate::tir::lower_to_lir::lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    // Triple refused without an inline-window proof: no op carries
    // lir.checked_overflow.
    let has_triple = lir.blocks.values().flat_map(|b| b.ops.iter()).any(|op| {
        matches!(
            op.tir_op.attrs.get("lir.checked_overflow"),
            Some(crate::tir::ops::AttrValue::Bool(true))
        )
    });
    assert!(
        !has_triple,
        "checked-i64 triple must be refused without a value-range proof"
    );

    let output = lower_lir_to_wasm(&lir).test_view();
    // The raw operands are boxed overflow-safely: the cold arm is a
    // NAMED int_from_i64 runtime call recorded in runtime_calls.
    assert!(
        output
            .runtime_calls
            .iter()
            .filter(|name| **name == "int_from_i64")
            .count()
            >= 2,
        "both full-range raw operands must box through the int_from_i64 cold path; got {:?}",
        output.runtime_calls
    );
}

/// THE regression guard for finding #3: an integer `add` with one proven
/// `RawI64Safe` operand and one `MaybeBigInt` operand must NOT emit a bare
/// `i64.add` (the unsound op on a NaN-boxed word). Both operands must be
/// NaN-boxed before the runtime `Call` (`molt_add`): the proven operand via
/// the inline-int box, the unproven operand passed through already-boxed.
#[test]
fn mixed_repr_int_add_boxes_both_operands_no_bare_i64_add() {
    let func = make_add_two_params_func();
    // a (ValueId 0) is proven RawI64Safe; b (ValueId 1) is an unproven
    // MaybeBigInt; the result (ValueId 2) is therefore MaybeBigInt too (it
    // cannot be proven from an unproven operand). This forces the generic
    // boxed path (NOT the checked-overflow triple, which requires all three
    // to be RawI64Safe).
    let repr: HashMap<ValueId, Repr> = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (ValueId(1), Repr::MaybeBigInt),
        (ValueId(2), Repr::MaybeBigInt),
    ]);
    let lir = lower_function_to_lir_with_inline_proof(
        &func,
        &repr,
        &crate::representation_plan::value_range_for(&func),
    );
    let output = lower_lir_to_wasm(&lir).test_view();

    // No bare OPERAND i64.add: a raw machine add on a possibly-heap-BigInt
    // operand is exactly the truncation bug-class this phase makes
    // un-emittable. The overflow-safe box legitimately contains an
    // `i64.add` (the `src + 2^46` fits-inline bias), so the precise
    // invariant is: every I64Add in the stream is a fits-check add —
    // immediately followed by the `2^47` window-limit const — never an
    // operand-pair add.
    for (idx, inst) in output.instructions.iter().enumerate() {
        if matches!(inst, Instruction::I64Add) {
            assert!(
                matches!(
                    output.instructions.get(idx + 1),
                    Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                ),
                "mixed-repr add emitted a bare operand i64.add at {idx} (operand may be a heap BigInt)"
            );
        }
    }
    // Runtime dispatch through the typed boxed helper import.
    assert!(
        output.runtime_calls.contains(&"add"),
        "mixed-repr add must dispatch through the boxed runtime helper"
    );
    // The proven RawI64Safe operand `a` is NaN-boxed (inline-int box) before
    // the call. (`b` is already a DynBox word and passes through, so exactly
    // one inline-int box is emitted for the operands of this add.)
    assert!(
        count_inline_int_boxes(&output.instructions) >= 1,
        "the proven raw-i64 operand must be NaN-boxed before the runtime call"
    );
}

#[test]
fn checked_overflow_triples_use_opcode_specific_runtime_helpers_without_generic_bail() {
    let cases = [
        ("checked_add_overflow_triple", OpCode::Add, 20, 22, "add"),
        ("checked_sub_overflow_triple", OpCode::Sub, 42, 20, "sub"),
        ("checked_mul_overflow_triple", OpCode::Mul, 6, 7, "mul"),
    ];

    for (name, opcode, lhs, rhs, runtime_call) in cases {
        let func = make_binary_two_consts_func(name, opcode, lhs, rhs);
        let output = lower_tir_to_wasm(&func).test_view();

        assert_eq!(
            output.bail_to_generic_reason, None,
            "{name} must stay in the LIR fast body"
        );
        assert!(
            has_native_binary_instruction(&output.instructions, opcode),
            "{name} must emit the hot raw WASM instruction for {opcode:?}"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} overflow side channel must dispatch through {runtime_call}; got {:?}",
            output.runtime_calls
        );
        for other in ["add", "sub", "mul"] {
            if other != runtime_call {
                assert!(
                    !output.runtime_calls.contains(&other),
                    "{name} must not call the {other} helper for a {opcode:?} overflow side channel; got {:?}",
                    output.runtime_calls
                );
            }
        }
    }
}

/// The perf-preservation direction: when BOTH operands are proven
/// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
/// triple). The cold overflow-box side channel is a typed runtime call, not a
/// generic body bail.
#[test]
fn proven_raw_i64_add_still_emits_native_i64_add() {
    let func = make_add_two_consts_func(20, 22);
    let output = lower_tir_to_wasm(&func).test_view();

    let has_operand_add = output.instructions.iter().enumerate().any(|(idx, inst)| {
        matches!(inst, Instruction::I64Add)
            && !matches!(
                output.instructions.get(idx + 1),
                Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
            )
    });
    assert!(
        has_operand_add,
        "range-proven const add must emit an operand-pair native i64.add, got {:?}",
        output.instructions
    );
    assert_eq!(output.bail_to_generic_reason, None);
    assert!(
        output.runtime_calls.contains(&"add"),
        "checked-overflow cold side channel must use the typed add helper"
    );
}

#[test]
fn checked_mul_raw_i64_emits_exact_wasm_overflow_flag_without_generic_bail() {
    let func = make_checked_mul_two_consts_func(6, 7);
    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(
        output.bail_to_generic_reason, None,
        "raw checked_mul must stay in the LIR fast body"
    );
    assert!(
        !output.runtime_calls.contains(&"mul"),
        "raw CheckedMul produces a raw carrier and must not route through boxed molt_mul"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Mul)),
        "raw CheckedMul must emit the wrapping i64.mul product"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64DivS)),
        "raw CheckedMul must emit the exact product/lhs overflow check"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(value) if *value == i64::MIN)
        ),
        "raw CheckedMul must guard the i64::MIN / -1 division trap"
    );
}

/// On the production boxed-i64 ABI path, a function whose integer params are
/// proven `RawI64Safe` keeps the fast path (entry args lower to `I64`); a
/// `MaybeBigInt` param forces the entry arg to `DynBox`, so the boxed-i64 ABI
/// (which requires all-`I64` entry args) bails to `None` — falling back to
/// the IntFastLane-guarded slow path. This is the structural gate that keeps
/// the unsound bare op un-emittable for unproven ints.
#[test]
fn boxed_i64_abi_bails_when_param_is_maybe_bigint() {
    let proven = make_add_two_consts_func(20, 22);
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&proven).is_some(),
        "range-proven raw-i64 values keep the boxed-i64 ABI fast path"
    );

    let unproven = make_add_two_params_func();
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&unproven).is_none(),
        "a MaybeBigInt param must bail the boxed-i64 ABI (entry arg is DynBox)"
    );
}
