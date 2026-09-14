use super::super::value_range::ValueRangeResult;
use crate::tir::op_kinds_generated::{
    opcode_requires_i64_shift_count_guard_table, opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::TirOp;
/// Returns `true` if the op is pure and safe to hoist out of a loop.
///
/// The operand-aware purity decision is delegated to the shared effects
/// authority. LICM additionally permits a structural SSA value copy, which is
/// a property of the op *instance* (its operand/result arity and empty attrs),
/// not of the opcode, so that check stays here.
///
/// Hoisting requires the FULL pure-movable property (including `nothrow`):
/// moving an op above the loop guard changes whether/when it would raise, so a
/// may-throw op (e.g. `Div`) must not be hoisted even though it is CSE-safe -
/// UNLESS its specific throw condition is *disproven* at the hoist site, which
/// [`throw_condition_disproven`] decides per-instance from the value-range proof
/// (an exact-integer shift whose count is in `[0, 63]`, or exact-integer floor
/// division/modulo whose divisor is non-zero). True division remains may-throw.
pub(super) fn is_hoistable(
    op: &TirOp,
    vr: &ValueRangeResult,
    value_types: &std::collections::HashMap<
        crate::tir::values::ValueId,
        crate::tir::types::TirType,
    >,
) -> bool {
    let effects = super::super::effects::op_effects_with_types(op, value_types);
    (effects.consistent && effects.effect_free && effects.nothrow)
        || op.is_plain_value_copy()
        || (effects.consistent
            && effects.effect_free
            && !effects.nothrow
            && throw_condition_disproven(op, vr, value_types))
}

/// True when an operand-proven `pure_may_throw` operator instance is
/// PROVEN not to raise on its operands - so hoisting it above the loop guard
/// cannot move an observable raise earlier (it would never have raised). This is
/// the domain-aware shared "throw-condition disproven" gate, with this pass
/// supplying only its value-range evidence.
///
///   * **`Shl` / `Shr`**: a negative shift count raises `ValueError`, and a
///     count `>= 64` is a wrong-value machine shift on the raw lane. The op is
///     nothrow-and-well-defined iff the count operand is range-proven in
///     `[0, 63]` - the exact gate the raw-i64 shift seed
///     (`representation_plan::raw_i64_safe_value_seed`) applies. We DO NOT
///     additionally require the result to fit the inline window: hoisting is a
///     *position* change, not a representation change - a hoisted `x << k` whose
///     result is a heap BigInt is still computed (boxed) in the preheader,
///     correctly, exactly once. The only property hoisting needs is that the
///     shift does not *raise* where the loop guard used to protect it, i.e. a
///     non-negative, in-machine-range count.
///   * **`FloorDiv` / `Mod`**: exact builtin integer operands plus a divisor
///     whose range `proves_nonzero()` discharge `ZeroDivisionError`. Python
///     integer floor division and modulo do not overflow at `i64::MIN / -1`;
///     the semantic result may be a BigInt and remains a representation concern.
///   * **`Div`**: REFUSED. Even exact semantic integers can overflow while being
///     converted to the float result, so a nonzero divisor is not a complete
///     exception proof. Mixed integer/float instances retain the same conversion
///     risk.
///   * **`Pow`**: REFUSED. `x ** y` raises `ZeroDivisionError` for `0 ** -1` and
///     returns a float for a negative integer exponent, so the nothrow
///     condition couples base AND exponent ranges (and the int/float result
///     repr); it is not trivially range-provable. We never hoist `Pow` here -
///     documenting the refusal rather than shipping an unsound or fragile gate.
///     CSE of `Pow` (under dominance) is unaffected; only the hoist is withheld.
pub(super) fn throw_condition_disproven(
    op: &TirOp,
    vr: &ValueRangeResult,
    value_types: &std::collections::HashMap<
        crate::tir::values::ValueId,
        crate::tir::types::TirType,
    >,
) -> bool {
    let shift_count_valid = opcode_requires_i64_shift_count_guard_table(op.opcode)
        && op
            .operands
            .get(1)
            .is_some_and(|&count| vr.range_of(count).proves_i64_shift_count());
    let divisor_nonzero = opcode_requires_i64_zero_divisor_guard_table(op.opcode)
        && op
            .operands
            .get(1)
            .is_some_and(|&divisor| vr.range_of(divisor).proves_nonzero());
    super::super::effects::guarded_throw_condition_disproven(
        op,
        value_types,
        shift_count_valid,
        divisor_nonzero,
    )
}
