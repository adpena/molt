//! Raw numeric lanes: the machine operation a numeric op is when its operands
//! are held raw and the representation plan proves the result.
//!
//! Each function answers `None` when no raw lane is proven; the caller then
//! takes the boxed runtime entry (operation-local custody, which joins the
//! walker with the value-materialization family). A raw integer lane needs a
//! proof about the *result*: `TirType::I64` is a semantic int with no range,
//! so a machine `add` selected on the type alone would wrap and then truncate
//! to 47 bits when boxed.

use molt_ir::OpKind;

use super::builder::{CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison};
use super::value_table::HeldCarrier;
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::values::ValueId;

/// One operand of a numeric op as the backend holds it.
#[derive(Clone, Copy)]
pub(crate) struct Operand<V> {
    pub(crate) id: ValueId,
    pub(crate) value: V,
    pub(crate) carrier: HeldCarrier,
}

/// A raw lane's result and the carrier it is held in.
pub(crate) type RawResult<V> = Option<(V, HeldCarrier)>;

fn flag(op: &TirOp, key: &str) -> bool {
    matches!(op.attrs.get(key), Some(AttrValue::Bool(true)))
}

fn raw_ints<B: CodegenBuilder>(
    b: &B,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> Option<(B::Word, B::Word)> {
    (lhs.carrier == HeldCarrier::RawI64 && rhs.carrier == HeldCarrier::RawI64)
        .then(|| (b.expect_word(lhs.value), b.expect_word(rhs.value)))
}

fn raw_floats<V: Copy>(lhs: Operand<V>, rhs: Operand<V>) -> Option<(V, V)> {
    (lhs.carrier == HeldCarrier::RawF64 && rhs.carrier == HeldCarrier::RawF64)
        .then_some((lhs.value, rhs.value))
}

/// `+`, `-` and `*` on two raw integers or two raw floats. Division and
/// modulo are not here: their zero-divisor guard raises through the runtime.
pub(crate) fn binary_arith<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    op: &TirOp,
    name: OpKind,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> RawResult<B::Value> {
    let result = op.results[0];
    let (int_op, float_op) = match name {
        OpKind::Add => (IntBinary::Add, FloatBinary::Add),
        OpKind::Sub => (IntBinary::Sub, FloatBinary::Sub),
        OpKind::Mul => (IntBinary::Mul, FloatBinary::Mul),
        _ => return None,
    };
    if let Some((left, right)) = raw_ints(b, lhs, rhs) {
        // The result fits the inline window, so the instruction cannot wrap.
        let inline_safe = plan.is_inline_safe_int(result);
        // The exact result lies in i64, so the plan carries it as the
        // full-range raw tier and it boxes through the full-range path: the
        // sum or difference of two inline-safe operands (HF-821), or an op
        // whose operand ranges checked integer loop versioning proved (a
        // bounded accumulator in its fast loop).
        let exact = plan.is_full_deopt_int(result)
            && (op.has_i64_exact_proof()
                || (matches!(name, OpKind::Add | OpKind::Sub)
                    && plan.is_inline_safe_int(lhs.id)
                    && plan.is_inline_safe_int(rhs.id)));
        if !(inline_safe || exact) {
            return None;
        }
        // `no_signed_wrap` comes from a TIR range analysis (bounded induction
        // increments); it lets the backend reason about trip counts.
        let no_signed_wrap = flag(op, "no_signed_wrap") || exact;
        let value = b.int_binary(int_op, left, right, no_signed_wrap);
        return Some((b.word_value(value), HeldCarrier::RawI64));
    }
    let (left, right) = raw_floats(lhs, rhs)?;
    let value = b.float_binary(float_op, left, right, flag(op, "fast_math"));
    Some((value, HeldCarrier::RawF64))
}

/// A comparison of two raw integers or two raw floats, as a raw bool.
pub(crate) fn comparison<B: CodegenBuilder>(
    b: &mut B,
    name: OpKind,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> RawResult<B::Value> {
    let (int_cmp, float_cmp) = match name {
        OpKind::Eq => (IntComparison::Equal, FloatComparison::OrderedEqual),
        // Python: `nan != nan` is True, so `!=` is unordered-or-unequal.
        OpKind::Ne => (
            IntComparison::NotEqual,
            FloatComparison::UnorderedOrNotEqual,
        ),
        OpKind::Lt => (IntComparison::SignedLess, FloatComparison::OrderedLess),
        OpKind::Le => (
            IntComparison::SignedLessOrEqual,
            FloatComparison::OrderedLessOrEqual,
        ),
        OpKind::Gt => (
            IntComparison::SignedGreater,
            FloatComparison::OrderedGreater,
        ),
        OpKind::Ge => (
            IntComparison::SignedGreaterOrEqual,
            FloatComparison::OrderedGreaterOrEqual,
        ),
        other => unreachable!("{other} is not a comparison"),
    };
    let bit = if let Some((left, right)) = raw_ints(b, lhs, rhs) {
        b.int_compare(int_cmp, left, right)
    } else {
        let (left, right) = raw_floats(lhs, rhs)?;
        b.float_compare(float_cmp, left, right)
    };
    Some((b.bit_value(bit), HeldCarrier::RawBool))
}

/// `&`, `|`, `^`, `<<` and `>>` on two raw integers. The three bitwise ops
/// cannot overflow. A shift takes the raw lane only when the plan proves the
/// result inline-safe, which the range seed grants only for a count proven in
/// `[0, 63]`: a wider count is undefined in machine code, and Python raises
/// or widens to a BigInt.
pub(crate) fn bitwise<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    op: &TirOp,
    name: OpKind,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> RawResult<B::Value> {
    let (int_op, needs_range_proof) = match name {
        OpKind::BitAnd => (IntBinary::And, false),
        OpKind::BitOr => (IntBinary::Or, false),
        OpKind::BitXor => (IntBinary::Xor, false),
        OpKind::Lshift => (IntBinary::ShiftLeft, true),
        OpKind::Rshift => (IntBinary::ShiftRightArithmetic, true),
        other => unreachable!("{other} is not a bitwise op"),
    };
    if needs_range_proof && !plan.is_inline_safe_int(op.results[0]) {
        return None;
    }
    let (left, right) = raw_ints(b, lhs, rhs)?;
    let value = b.int_binary(int_op, left, right, false);
    Some((b.word_value(value), HeldCarrier::RawI64))
}

/// A unary op on a raw operand.
pub(crate) fn unary<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    op: &TirOp,
    name: OpKind,
    operand: Operand<B::Value>,
) -> RawResult<B::Value> {
    match (operand.carrier, name) {
        // Negating i64::MIN needs a BigInt: the same result-range proof as
        // binary arithmetic authorizes the machine negation.
        (HeldCarrier::RawI64, OpKind::Neg) if plan.is_inline_safe_int(op.results[0]) => {
            let word = b.expect_word(operand.value);
            let value = b.int_negate(word);
            Some((b.word_value(value), HeldCarrier::RawI64))
        }
        (HeldCarrier::RawF64, OpKind::Neg) => {
            Some((b.float_negate(operand.value), HeldCarrier::RawF64))
        }
        // Unary plus is the identity only for an exact numeric carrier: a
        // bool becomes an int and a boxed value dispatches `__pos__`.
        (HeldCarrier::RawI64 | HeldCarrier::RawF64, OpKind::Pos) => {
            Some((operand.value, operand.carrier))
        }
        (HeldCarrier::RawBool, OpKind::Not) => {
            let bit = b.expect_bit(operand.value);
            let value = b.bit_not(bit);
            Some((b.bit_value(value), HeldCarrier::RawBool))
        }
        (HeldCarrier::RawI64, OpKind::Not) => {
            let word = b.expect_word(operand.value);
            let zero = b.constant(0);
            let bit = b.int_compare(IntComparison::Equal, word, zero);
            Some((b.bit_value(bit), HeldCarrier::RawBool))
        }
        (HeldCarrier::RawI64, OpKind::Invert) => {
            let word = b.expect_word(operand.value);
            let value = b.int_invert(word);
            Some((b.word_value(value), HeldCarrier::RawI64))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! The raw lanes on the interpreting builder. The oracle is Python: a raw
    //! lane exists only where the machine result equals Python's for every
    //! input the plan admits, and is refused where it could wrap.

    use super::super::scalar::tests::{Interpreter, Value};
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use molt_ir::repr::Repr;

    const LHS: ValueId = ValueId(0);
    const RHS: ValueId = ValueId(1);
    const RESULT: ValueId = ValueId(2);

    fn op() -> TirOp {
        TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![LHS, RHS],
            results: vec![RESULT],
            attrs: AttrDict::default(),
            source_span: None,
        }
    }

    fn plan(facts: &[(ValueId, Repr)]) -> ProducerCarrierPlan {
        let mut plan = ProducerCarrierPlan::default();
        plan.repr_by_value.extend(facts.iter().copied());
        plan
    }

    fn int(id: ValueId, raw: i64) -> Operand<Value> {
        Operand {
            id,
            value: Value::Word(raw as u64),
            carrier: HeldCarrier::RawI64,
        }
    }

    fn float(id: ValueId, raw: f64) -> Operand<Value> {
        Operand {
            id,
            value: Value::Float(raw),
            carrier: HeldCarrier::RawF64,
        }
    }

    fn raw_int(result: RawResult<Value>) -> Option<i64> {
        result.map(|(value, carrier)| {
            assert_eq!(carrier, HeldCarrier::RawI64);
            match value {
                Value::Word(word) => word as i64,
                other => panic!("{other:?} is not a raw integer"),
            }
        })
    }

    #[test]
    fn integer_arithmetic_is_raw_only_under_a_result_range_proof() {
        let mut b = Interpreter::default();
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        for name in [OpKind::Add, OpKind::Sub, OpKind::Mul] {
            let lanes = binary_arith(&mut b, &unproven, &op(), name, int(LHS, 7), int(RHS, 5));
            assert!(lanes.is_none(), "{name}: an unproven result could wrap");
        }
        for (name, expected) in [(OpKind::Add, 12), (OpKind::Sub, 2), (OpKind::Mul, 35)] {
            let lanes = binary_arith(&mut b, &safe, &op(), name, int(LHS, 7), int(RHS, 5));
            assert_eq!(raw_int(lanes), Some(expected), "{name}");
        }
        // The exact-sum tier: inline-safe operands, a full-range result.
        let exact = plan(&[
            (LHS, Repr::RawI64Safe),
            (RHS, Repr::RawI64Safe),
            (RESULT, Repr::RawI64FullDeopt),
        ]);
        let big = (1 << 46) - 1;
        let sum = binary_arith(
            &mut b,
            &exact,
            &op(),
            OpKind::Add,
            int(LHS, big),
            int(RHS, big),
        );
        assert_eq!(
            raw_int(sum),
            Some(2 * big),
            "the sum leaves the inline window, exactly"
        );
        let product = binary_arith(
            &mut b,
            &exact,
            &op(),
            OpKind::Mul,
            int(LHS, big),
            int(RHS, big),
        );
        assert!(
            product.is_none(),
            "a product of inline operands can exceed 64 bits"
        );
        // One full-range operand voids the exactness proof.
        let one_wide = plan(&[(LHS, Repr::RawI64Safe), (RESULT, Repr::RawI64FullDeopt)]);
        let lanes = binary_arith(
            &mut b,
            &one_wide,
            &op(),
            OpKind::Add,
            int(LHS, 1),
            int(RHS, 1),
        );
        assert!(lanes.is_none());
        // Division and modulo raise on zero: never a bare machine instruction.
        for name in [OpKind::Div, OpKind::Floordiv, OpKind::Mod] {
            assert!(binary_arith(&mut b, &safe, &op(), name, int(LHS, 7), int(RHS, 5)).is_none());
        }
    }

    #[test]
    fn float_arithmetic_is_raw_and_a_mixed_pair_is_not() {
        let mut b = Interpreter::default();
        let unproven = plan(&[]);
        for (name, expected) in [(OpKind::Add, 2.0), (OpKind::Sub, 1.0), (OpKind::Mul, 0.75)] {
            let lanes = binary_arith(
                &mut b,
                &unproven,
                &op(),
                name,
                float(LHS, 1.5),
                float(RHS, 0.5),
            );
            assert_eq!(
                lanes,
                Some((Value::Float(expected), HeldCarrier::RawF64)),
                "{name}"
            );
        }
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        let mixed = binary_arith(
            &mut b,
            &safe,
            &op(),
            OpKind::Add,
            int(LHS, 1),
            float(RHS, 0.5),
        );
        assert!(
            mixed.is_none(),
            "int + float promotes in the runtime or the late stage"
        );
    }

    #[test]
    fn comparisons_follow_python_for_signed_integers_and_nan() {
        let mut b = Interpreter::default();
        let truth = |result: RawResult<Value>| match result {
            Some((Value::Bit(bit), HeldCarrier::RawBool)) => bit,
            other => panic!("{other:?} is not a raw bool"),
        };
        for (name, expected) in [
            (OpKind::Eq, false),
            (OpKind::Ne, true),
            (OpKind::Lt, true),
            (OpKind::Le, true),
            (OpKind::Gt, false),
            (OpKind::Ge, false),
        ] {
            // -1 < 1 as signed integers; as unsigned words the order flips.
            assert_eq!(
                truth(comparison(&mut b, name, int(LHS, -1), int(RHS, 1))),
                expected
            );
            assert_eq!(
                truth(comparison(&mut b, name, float(LHS, -1.0), float(RHS, 1.0))),
                expected
            );
            // Every comparison with NaN is False except `!=`.
            assert_eq!(
                truth(comparison(
                    &mut b,
                    name,
                    float(LHS, f64::NAN),
                    float(RHS, f64::NAN)
                )),
                name == OpKind::Ne,
                "{name}"
            );
        }
        let boxed = Operand {
            id: RHS,
            value: Value::Word(0),
            carrier: HeldCarrier::Boxed,
        };
        assert!(comparison(&mut b, OpKind::Eq, int(LHS, 0), boxed).is_none());
    }

    #[test]
    fn a_shift_needs_a_range_proof_and_the_bitwise_ops_do_not() {
        let mut b = Interpreter::default();
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        for (name, expected) in [
            (OpKind::BitAnd, 4),
            (OpKind::BitOr, -1),
            (OpKind::BitXor, -5),
        ] {
            let lanes = bitwise(&mut b, &unproven, &op(), name, int(LHS, -4), int(RHS, 7));
            assert_eq!(raw_int(lanes), Some(expected), "{name}");
        }
        for name in [OpKind::Lshift, OpKind::Rshift] {
            let lanes = bitwise(&mut b, &unproven, &op(), name, int(LHS, 1), int(RHS, 70));
            assert!(
                lanes.is_none(),
                "{name}: a count of 70 is not a machine shift"
            );
        }
        let left = bitwise(
            &mut b,
            &safe,
            &op(),
            OpKind::Lshift,
            int(LHS, -3),
            int(RHS, 4),
        );
        assert_eq!(raw_int(left), Some(-48));
        // Python's `>>` floors: -9 >> 1 == -5.
        let right = bitwise(
            &mut b,
            &safe,
            &op(),
            OpKind::Rshift,
            int(LHS, -9),
            int(RHS, 1),
        );
        assert_eq!(raw_int(right), Some(-5));
    }

    #[test]
    fn unary_ops_take_a_raw_lane_only_where_python_agrees() {
        let mut b = Interpreter::default();
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        let bit = |value: bool| Operand {
            id: LHS,
            value: Value::Bit(value),
            carrier: HeldCarrier::RawBool,
        };
        // -(i64::MIN) is a BigInt: negation needs the result proof.
        assert!(unary(&mut b, &unproven, &op(), OpKind::Neg, int(LHS, 5)).is_none());
        assert_eq!(
            raw_int(unary(&mut b, &safe, &op(), OpKind::Neg, int(LHS, 5))),
            Some(-5)
        );
        assert_eq!(
            unary(&mut b, &unproven, &op(), OpKind::Neg, float(LHS, 0.0)),
            Some((Value::Float(-0.0), HeldCarrier::RawF64))
        );
        assert_eq!(
            raw_int(unary(&mut b, &unproven, &op(), OpKind::Invert, int(LHS, 5))),
            Some(-6)
        );
        assert_eq!(
            raw_int(unary(&mut b, &unproven, &op(), OpKind::Pos, int(LHS, 5))),
            Some(5)
        );
        assert_eq!(
            unary(&mut b, &unproven, &op(), OpKind::Not, int(LHS, 0)),
            Some((Value::Bit(true), HeldCarrier::RawBool))
        );
        assert_eq!(
            unary(&mut b, &unproven, &op(), OpKind::Not, bit(true)),
            Some((Value::Bit(false), HeldCarrier::RawBool))
        );
        // +True is 1 and ~True is -2: a bool is not its own integer carrier.
        for name in [OpKind::Pos, OpKind::Neg, OpKind::Invert] {
            assert!(
                unary(&mut b, &safe, &op(), name, bit(true)).is_none(),
                "{name}"
            );
        }
        // `not 0.0` and `~1.5` go to the runtime.
        assert!(unary(&mut b, &safe, &op(), OpKind::Not, float(LHS, 0.0)).is_none());
        assert!(unary(&mut b, &safe, &op(), OpKind::Invert, float(LHS, 1.5)).is_none());
    }
}
