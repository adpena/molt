use super::super::*;

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
