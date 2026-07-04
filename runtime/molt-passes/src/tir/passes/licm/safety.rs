use super::super::value_range::ValueRangeResult;
use crate::tir::op_kinds_generated::{
    opcode_requires_i64_shift_count_guard_table, opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::TirOp;
/// Returns `true` if the op is pure and safe to hoist out of a loop.
///
/// The opcode-level purity decision is delegated to the single source of truth
/// in `effects::opcode_is_pure_movable` (deterministic + side-effect-free +
/// never-throwing). LICM additionally permits a structural SSA value copy,
/// which is a property of the op *instance* (its operand/result arity and empty
/// attrs), not of the opcode, so that check stays here.
///
/// Hoisting requires the FULL pure-movable property (including `nothrow`):
/// moving an op above the loop guard changes whether/when it would raise, so a
/// may-throw op (e.g. `Div`) must not be hoisted even though it is CSE-safe -
/// UNLESS its specific throw condition is *disproven* at the hoist site, which
/// [`throw_condition_disproven`] decides per-instance from the value-range proof
/// (a shift whose count is in `[0, 63]`, a divide whose divisor is non-zero).
pub(super) fn is_hoistable(op: &TirOp, vr: &ValueRangeResult) -> bool {
    super::super::effects::opcode_is_pure_movable(op.opcode)
        || op.is_plain_value_copy()
        || (super::super::effects::opcode_is_pure_may_throw(op.opcode)
            && throw_condition_disproven(op, vr))
}

/// True when a `pure_may_throw` op (`{Div, FloorDiv, Mod, Pow, Shl, Shr}`) is
/// PROVEN not to raise on its operands - so hoisting it above the loop guard
/// cannot move an observable raise earlier (it would never have raised). This is
/// the honest generalization of the hoist gate: "throw-condition disproven",
/// parameterized per opcode, each arm reusing the SINGLE value-range proof the
/// raw-i64 lane already uses (no duplicated proof logic).
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
///   * **`Div` / `FloorDiv` / `Mod`**: a zero divisor raises
///     `ZeroDivisionError`. The op is nothrow iff the divisor operand
///     `proves_nonzero()` - the same predicate the WASM raw `sdiv`/`srem` lane
///     uses (#42). (Integer `i64::MIN / -1` overflow is a separate concern that
///     does not raise in Python - it produces a bigint - and is handled by the
///     boxed lowering, not a raise, so it does not block the hoist.)
///   * **`Pow`**: REFUSED. `x ** y` raises `ZeroDivisionError` for `0 ** -1` and
///     returns a float for a negative integer exponent, so the nothrow
///     condition couples base AND exponent ranges (and the int/float result
///     repr); it is not trivially range-provable. We never hoist `Pow` here -
///     documenting the refusal rather than shipping an unsound or fragile gate.
///     CSE of `Pow` (under dominance) is unaffected; only the hoist is withheld.
pub(super) fn throw_condition_disproven(op: &TirOp, vr: &ValueRangeResult) -> bool {
    if opcode_requires_i64_shift_count_guard_table(op.opcode) {
        // Count operand proven in the valid machine-shift range [0, 63].
        return op
            .operands
            .get(1)
            .is_some_and(|&count| vr.range_of(count).proves_i64_shift_count());
    }
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode) {
        // Divisor proven to exclude zero.
        return op
            .operands
            .get(1)
            .is_some_and(|&divisor| vr.range_of(divisor).proves_nonzero());
    }
    // Pow's throw condition is not a single-operand range fact - refuse.
    false
}
