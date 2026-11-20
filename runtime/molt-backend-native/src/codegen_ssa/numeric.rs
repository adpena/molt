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

use super::builder::{
    CheckedBinary, CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison,
};
use super::value_table::HeldCarrier;
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;
use molt_codegen_abi as nanbox;
use molt_ir::runtime_fn_generated::RuntimeFn;

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

/// The operands of an int-valued operator (`+ - *` and comparisons): raw
/// ints, or raw bools, each the int 0 or 1 since Python's bool subclasses
/// int. Bitwise operators keep [`raw_ints`]: `True & True` is the bool True.
fn raw_int_valued<B: CodegenBuilder>(
    b: &mut B,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> Option<(B::Word, B::Word)> {
    let admitted = |operand: &Operand<B::Value>| {
        matches!(operand.carrier, HeldCarrier::RawI64 | HeldCarrier::RawBool)
    };
    if !(admitted(&lhs) && admitted(&rhs)) {
        return None;
    }
    let word = |b: &mut B, operand: Operand<B::Value>| match operand.carrier {
        HeldCarrier::RawBool => b.zero_extend(operand.value),
        _ => b.expect_word(operand.value),
    };
    let left = word(b, lhs);
    Some((left, word(b, rhs)))
}

/// A raw bool operand lies in the inline-int window, whatever the plan
/// records for its producer.
fn inline_safe_operand<V>(plan: &ProducerCarrierPlan, operand: &Operand<V>) -> bool {
    operand.carrier == HeldCarrier::RawBool || plan.is_inline_safe_int(operand.id)
}

fn raw_floats<V: Copy>(lhs: Operand<V>, rhs: Operand<V>) -> Option<(V, V)> {
    (lhs.carrier == HeldCarrier::RawF64 && rhs.carrier == HeldCarrier::RawF64)
        .then_some((lhs.value, rhs.value))
}

/// `+`, `-` and `*` on two raw integers or two raw floats. Division and
/// modulo are [`divide`]: their zero-divisor arm raises through the runtime.
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
    if let Some((left, right)) = raw_int_valued(b, lhs, rhs) {
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
                    && inline_safe_operand(plan, &lhs)
                    && inline_safe_operand(plan, &rhs)));
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
    let bit = if let Some((left, right)) = raw_int_valued(b, lhs, rhs) {
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

/// `CheckedAdd` and `CheckedMul` (the overflow peel's ops) on two raw
/// integers: the wrapping machine result and the signed-overflow bit. The
/// result is observable only where the bit is clear; the peel's control flow
/// enforces that. `None` when an operand is not a proven raw carrier: the
/// boxed runtime entry is exact and its overflow bit is constant false.
pub(crate) fn checked_arith<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    opcode: OpCode,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> Option<(B::Value, B::Value)> {
    let checked = match opcode {
        OpCode::CheckedAdd => CheckedBinary::Add,
        OpCode::CheckedMul => CheckedBinary::Mul,
        other => unreachable!("{other:?} is not a checked op"),
    };
    let raw_int = |operand: &Operand<B::Value>| {
        operand.carrier == HeldCarrier::RawBool || plan.is_raw_int_carrier(operand.id)
    };
    if !(raw_int(&lhs) && raw_int(&rhs)) {
        return None;
    }
    let (left, right) = raw_int_valued(b, lhs, rhs)?;
    let (value, overflow) = b.int_checked(checked, left, right);
    Some((b.word_value(value), b.bit_value(overflow)))
}

/// `/`, `//` and `%` on two raw integers, `/` and `%` on two raw floats. A
/// machine divide by zero is undefined (LLVM poison), so the machine
/// operation runs behind a zero-divisor test. The zero arm calls the op's
/// boxed runtime entry, which raises ZeroDivisionError; it never returns
/// normally, and its word only completes the join.
///
/// `i64::MIN // -1` is `2**63` in Python, which no raw carrier holds, and
/// `i64::MIN // -1` and `i64::MIN % -1` are undefined machine divisions. So
/// `//` takes the integer lane only when the dividend is inline-safe (it is
/// not `i64::MIN`) or the plan proves the quotient a raw carrier: inline-safe
/// by its range, or full-range because the divisor is a constant other than
/// 0 and -1 (`ReprRawI64FullDeoptSeedRule::QuotientByConstantDivisor`). `%` always fits: a full-range dividend divides by 1 where the
/// divisor is -1, since `x % -1 == x % 1 == 0`. `/` converts both operands
/// to floats, which is exact only below 2**53, so both must be inline-safe.
pub(crate) fn divide<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    name: OpKind,
    result: ValueId,
    lhs: Operand<B::Value>,
    rhs: Operand<B::Value>,
) -> RawResult<B::Value> {
    let runtime = molt_ir::runtime_fn_generated::numeric_runtime_fn(name)?;
    if let Some((left, right)) = raw_ints(b, lhs, rhs) {
        let dividend_safe = plan.is_inline_safe_int(lhs.id);
        let admitted = match name {
            OpKind::Div => dividend_safe && plan.is_inline_safe_int(rhs.id),
            OpKind::Floordiv => dividend_safe || plan.is_raw_int_carrier(result),
            OpKind::Mod => true,
            _ => false,
        };
        if !admitted {
            return None;
        }
        let zero = b.constant(0);
        let nonzero = b.int_compare(IntComparison::NotEqual, right, zero);
        let word = b.choose(
            nonzero,
            |b| {
                let right = if name == OpKind::Mod && !dividend_safe {
                    let minus_one = b.constant(u64::MAX);
                    let is_minus_one = b.int_compare(IntComparison::Equal, right, minus_one);
                    let one = b.constant(1);
                    let (one, right) = (b.word_value(one), b.word_value(right));
                    let divisor = b.select(is_minus_one, one, right);
                    b.expect_word(divisor)
                } else {
                    right
                };
                integer_quotient(b, name, left, right)
            },
            |b| {
                // An integer ZeroDivisionError names no operand value, so
                // the entry takes the zero divisor twice: an inline box that
                // mints no owner, whatever the dividend's width.
                let zero = nanbox::boxed_word::generate_box_inline_int(b, right);
                raise_zero_division(b, runtime, zero, zero)
            },
        );
        return Some(match name {
            OpKind::Div => (b.bits_to_float(word), HeldCarrier::RawF64),
            _ => (b.word_value(word), HeldCarrier::RawI64),
        });
    }
    let (left, right) = raw_floats(lhs, rhs)?;
    if !matches!(name, OpKind::Div | OpKind::Mod) {
        return None;
    }
    let zero = b.float_constant(0.0);
    let nonzero = b.float_compare(FloatComparison::UnorderedOrNotEqual, right, zero);
    let word = b.choose(
        nonzero,
        |b| {
            let value = float_quotient(b, name, left, right);
            b.float_to_bits(value)
        },
        |b| {
            // A float is its own boxed word.
            let left = b.float_to_bits(left);
            let right = b.float_to_bits(right);
            raise_zero_division(b, runtime, left, right)
        },
    );
    Some((b.bits_to_float(word), HeldCarrier::RawF64))
}

/// The boxed entry of a division whose divisor is zero: it raises and its
/// word is dead.
fn raise_zero_division<B: CodegenBuilder>(
    b: &mut B,
    runtime: RuntimeFn,
    left: B::Word,
    right: B::Word,
) -> B::Word {
    b.call_runtime(runtime, &[left, right])
        .unwrap_or_else(|| panic!("{runtime:?} returns a word"))
}

/// Python's quotient of two integers, the divisor nonzero. `/` is the float
/// quotient's bits; `//` floors toward negative infinity; `%` takes the
/// divisor's sign.
fn integer_quotient<B: CodegenBuilder>(
    b: &mut B,
    name: OpKind,
    left: B::Word,
    right: B::Word,
) -> B::Word {
    let zero = b.constant(0);
    match name {
        OpKind::Div => {
            let left = b.int_to_float(left);
            let right = b.int_to_float(right);
            let value = b.float_binary(FloatBinary::Divide, left, right, false);
            b.float_to_bits(value)
        }
        OpKind::Floordiv => {
            // q -= 1 when the division is inexact and the signs differ.
            let quotient = b.int_binary(IntBinary::SignedQuotient, left, right, false);
            let remainder = b.int_binary(IntBinary::SignedRemainder, left, right, false);
            let inexact = b.int_compare(IntComparison::NotEqual, remainder, zero);
            let signs = b.int_binary(IntBinary::Xor, left, right, false);
            let differ = b.int_compare(IntComparison::SignedLess, signs, zero);
            let adjust = b.both(inexact, differ);
            let lower = b.add_immediate(quotient, -1);
            let (lower, quotient) = (b.word_value(lower), b.word_value(quotient));
            let value = b.select(adjust, lower, quotient);
            b.expect_word(value)
        }
        OpKind::Mod => {
            // r += divisor when r is nonzero and its sign is not the divisor's.
            let remainder = b.int_binary(IntBinary::SignedRemainder, left, right, false);
            let inexact = b.int_compare(IntComparison::NotEqual, remainder, zero);
            let signs = b.int_binary(IntBinary::Xor, remainder, right, false);
            let differ = b.int_compare(IntComparison::SignedLess, signs, zero);
            let adjust = b.both(inexact, differ);
            let adjusted = b.int_binary(IntBinary::Add, remainder, right, false);
            let (adjusted, remainder) = (b.word_value(adjusted), b.word_value(remainder));
            let value = b.select(adjust, adjusted, remainder);
            b.expect_word(value)
        }
        other => unreachable!("{other} is not a division"),
    }
}

/// Python's `/` or `%` of two floats, the divisor nonzero. `%` takes the
/// divisor's sign, and a zero remainder is a zero of the divisor's sign.
fn float_quotient<B: CodegenBuilder>(
    b: &mut B,
    name: OpKind,
    left: B::Value,
    right: B::Value,
) -> B::Value {
    match name {
        OpKind::Div => b.float_binary(FloatBinary::Divide, left, right, false),
        OpKind::Mod => {
            let zero = b.float_constant(0.0);
            let remainder = b.float_binary(FloatBinary::Remainder, left, right, false);
            let nonzero = b.float_compare(FloatComparison::UnorderedOrNotEqual, remainder, zero);
            let negative = b.float_compare(FloatComparison::OrderedLess, remainder, zero);
            let divisor_negative = b.float_compare(FloatComparison::OrderedLess, right, zero);
            let differ = b.bit_xor(negative, divisor_negative);
            let adjusted = b.float_binary(FloatBinary::Add, remainder, right, false);
            let signed = b.select(differ, adjusted, remainder);
            let divisor_bits = b.float_to_bits(right);
            let sign = b.and(divisor_bits, 1 << 63);
            let signed_zero = b.bits_to_float(sign);
            b.select(nonzero, signed, signed_zero)
        }
        other => unreachable!("{other} is not a float division"),
    }
}

#[cfg(test)]
mod tests {
    //! The raw lanes on the interpreting builder and as Cranelift machine
    //! code. The oracle is Python: a raw lane exists only where the machine
    //! result equals Python's for every input the plan admits, and is refused
    //! where it could wrap.

    use super::super::differential::{Run, both, kernel};
    use super::super::scalar::tests::Value;
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

    /// An operand as the test passes it: its held carrier and its value.
    type Input = (HeldCarrier, Value);

    fn int(raw: i64) -> Input {
        (HeldCarrier::RawI64, Value::Word(raw as u64))
    }

    fn float(raw: f64) -> Input {
        (HeldCarrier::RawF64, Value::Float(raw))
    }

    fn bit(raw: bool) -> Input {
        (HeldCarrier::RawBool, Value::Bit(raw))
    }

    /// The numeric lane under test.
    #[derive(Clone, Copy)]
    enum Lane {
        Arith(OpKind),
        Compare(OpKind),
        Bitwise(OpKind),
        Unary(OpKind),
        Checked(OpCode),
        Divide(OpKind),
    }

    /// Run `lane` on both machines over operands %0 (and %1), result %2.
    /// The fact is the result's carrier, or None where the lane refused.
    fn run(plan: &ProducerCarrierPlan, lane: Lane, operands: &[Input]) -> Run<Option<HeldCarrier>> {
        let carriers: Vec<HeldCarrier> = operands.iter().map(|&(carrier, _)| carrier).collect();
        let inputs: Vec<Value> = operands.iter().map(|&(_, value)| value).collect();
        let kernel = kernel!(
            plan: ProducerCarrierPlan = plan.clone(),
            lane: Lane = lane,
            carriers: Vec<HeldCarrier> = carriers;
            |b, inputs| -> Option<HeldCarrier> {
                let operand = |index: usize| Operand {
                    id: ValueId(index as u32),
                    value: inputs[index],
                    carrier: carriers[index],
                };
                let result = match *lane {
                    Lane::Arith(name) => binary_arith(b, plan, &op(), name, operand(0), operand(1)),
                    Lane::Compare(name) => comparison(b, name, operand(0), operand(1)),
                    Lane::Bitwise(name) => bitwise(b, plan, &op(), name, operand(0), operand(1)),
                    Lane::Unary(name) => unary(b, plan, &op(), name, operand(0)),
                    Lane::Divide(name) => divide(b, plan, name, RESULT, operand(0), operand(1)),
                    Lane::Checked(opcode) => {
                        return match checked_arith(b, plan, opcode, operand(0), operand(1)) {
                            Some((value, overflow)) => {
                                (Some(HeldCarrier::RawI64), vec![value, overflow])
                            }
                            None => (None, Vec::new()),
                        };
                    }
                };
                match result {
                    Some((value, carrier)) => (Some(carrier), vec![value]),
                    None => (None, Vec::new()),
                }
            }
        );
        both(&kernel, &inputs)
    }

    /// The lane's raw result and carrier, or None where it refused.
    fn lane(plan: &ProducerCarrierPlan, lane: Lane, operands: &[Input]) -> RawResult<Value> {
        let run = run(plan, lane, operands);
        run.fact.map(|carrier| (run.results[0], carrier))
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
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        for name in [OpKind::Add, OpKind::Sub, OpKind::Mul] {
            let lanes = lane(&unproven, Lane::Arith(name), &[int(7), int(5)]);
            assert!(lanes.is_none(), "{name}: an unproven result could wrap");
        }
        for (name, expected) in [(OpKind::Add, 12), (OpKind::Sub, 2), (OpKind::Mul, 35)] {
            let lanes = lane(&safe, Lane::Arith(name), &[int(7), int(5)]);
            assert_eq!(raw_int(lanes), Some(expected), "{name}");
        }
        // The exact-sum tier: inline-safe operands, a full-range result.
        let exact = plan(&[
            (LHS, Repr::RawI64Safe),
            (RHS, Repr::RawI64Safe),
            (RESULT, Repr::RawI64FullDeopt),
        ]);
        let big = (1 << 46) - 1;
        let sum = lane(&exact, Lane::Arith(OpKind::Add), &[int(big), int(big)]);
        assert_eq!(
            raw_int(sum),
            Some(2 * big),
            "the sum leaves the inline window, exactly"
        );
        let product = lane(&exact, Lane::Arith(OpKind::Mul), &[int(big), int(big)]);
        assert!(
            product.is_none(),
            "a product of inline operands can exceed 64 bits"
        );
        // One full-range operand voids the exactness proof.
        let one_wide = plan(&[(LHS, Repr::RawI64Safe), (RESULT, Repr::RawI64FullDeopt)]);
        assert!(lane(&one_wide, Lane::Arith(OpKind::Add), &[int(1), int(1)]).is_none());
        // Division and modulo raise on zero: never a bare machine instruction.
        for name in [OpKind::Div, OpKind::Floordiv, OpKind::Mod] {
            assert!(lane(&safe, Lane::Arith(name), &[int(7), int(5)]).is_none());
        }
    }

    #[test]
    fn float_arithmetic_is_raw_and_a_mixed_pair_is_not() {
        let unproven = plan(&[]);
        for (name, expected) in [(OpKind::Add, 2.0), (OpKind::Sub, 1.0), (OpKind::Mul, 0.75)] {
            let lanes = lane(&unproven, Lane::Arith(name), &[float(1.5), float(0.5)]);
            assert_eq!(
                lanes,
                Some((Value::Float(expected), HeldCarrier::RawF64)),
                "{name}"
            );
        }
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        let mixed = lane(&safe, Lane::Arith(OpKind::Add), &[int(1), float(0.5)]);
        assert!(
            mixed.is_none(),
            "int + float promotes in the runtime or the late stage"
        );
    }

    #[test]
    fn comparisons_follow_python_for_signed_integers_and_nan() {
        let unproven = plan(&[]);
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
            let compare =
                |operands: &[Input]| truth(lane(&unproven, Lane::Compare(name), operands));
            // -1 < 1 as signed integers; as unsigned words the order flips.
            assert_eq!(compare(&[int(-1), int(1)]), expected);
            assert_eq!(compare(&[float(-1.0), float(1.0)]), expected);
            // Every comparison with NaN is False except `!=`.
            assert_eq!(
                compare(&[float(f64::NAN), float(f64::NAN)]),
                name == OpKind::Ne,
                "{name}"
            );
        }
        let boxed = (HeldCarrier::Boxed, Value::Word(0));
        assert!(lane(&unproven, Lane::Compare(OpKind::Eq), &[int(0), boxed]).is_none());
    }

    #[test]
    fn a_shift_needs_a_range_proof_and_the_bitwise_ops_do_not() {
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        for (name, expected) in [
            (OpKind::BitAnd, 4),
            (OpKind::BitOr, -1),
            (OpKind::BitXor, -5),
        ] {
            let lanes = lane(&unproven, Lane::Bitwise(name), &[int(-4), int(7)]);
            assert_eq!(raw_int(lanes), Some(expected), "{name}");
        }
        for name in [OpKind::Lshift, OpKind::Rshift] {
            let lanes = lane(&unproven, Lane::Bitwise(name), &[int(1), int(70)]);
            assert!(
                lanes.is_none(),
                "{name}: a count of 70 is not a machine shift"
            );
        }
        let left = lane(&safe, Lane::Bitwise(OpKind::Lshift), &[int(-3), int(4)]);
        assert_eq!(raw_int(left), Some(-48));
        // Python's `>>` floors: -9 >> 1 == -5.
        let right = lane(&safe, Lane::Bitwise(OpKind::Rshift), &[int(-9), int(1)]);
        assert_eq!(raw_int(right), Some(-5));
    }

    #[test]
    fn checked_ops_answer_the_wrapping_result_and_python_overflow() {
        let raw = plan(&[(LHS, Repr::RawI64FullDeopt), (RHS, Repr::RawI64Safe)]);
        let checked = |opcode, l: i64, r: i64| {
            let run = run(&raw, Lane::Checked(opcode), &[int(l), int(r)]);
            match run.results[..] {
                [Value::Word(word), Value::Bit(overflow)] => (word as i64, overflow),
                ref other => panic!("{other:?} is not a checked result"),
            }
        };
        assert_eq!(checked(OpCode::CheckedAdd, 40, 2), (42, false));
        assert_eq!(checked(OpCode::CheckedAdd, i64::MAX, 1), (i64::MIN, true));
        assert_eq!(checked(OpCode::CheckedAdd, i64::MIN, -1), (i64::MAX, true));
        assert_eq!(
            checked(OpCode::CheckedAdd, -5, i64::MIN + 5),
            (i64::MIN, false)
        );
        assert_eq!(checked(OpCode::CheckedMul, -6, 7), (-42, false));
        assert_eq!(
            checked(OpCode::CheckedMul, 1 << 32, 1 << 31),
            (i64::MIN, true)
        );
        assert_eq!(
            checked(OpCode::CheckedMul, -(1 << 32), 1 << 31),
            (i64::MIN, false)
        );
        assert_eq!(checked(OpCode::CheckedMul, i64::MIN, -1), (i64::MIN, true));
        // An operand that is not a proven raw carrier takes the exact runtime.
        let unproven = plan(&[(LHS, Repr::RawI64Safe)]);
        assert!(
            run(
                &unproven,
                Lane::Checked(OpCode::CheckedAdd),
                &[int(1), int(1)]
            )
            .fact
            .is_none()
        );
    }

    #[test]
    fn integer_division_floors_like_python_and_raises_on_zero() {
        let safe = plan(&[(LHS, Repr::RawI64Safe), (RHS, Repr::RawI64Safe)]);
        // (dividend, divisor, Python's a // b, a % b, a / b)
        for (l, r, floor, modulo, quotient) in [
            (7, 2, 3, 1, 3.5),
            (-7, 2, -4, 1, -3.5),
            (7, -2, -4, -1, -3.5),
            (-7, -2, 3, -1, 3.5),
            (6, -3, -2, 0, -2.0),
            (0, -5, 0, 0, -0.0),
            (
                (1 << 46) - 1,
                3,
                23_456_248_059_221,
                0,
                23_456_248_059_221.0,
            ),
        ] {
            let lanes = |name| run(&safe, Lane::Divide(name), &[int(l), int(r)]);
            let floor_run = lanes(OpKind::Floordiv);
            assert_eq!(
                raw_int(floor_run.fact.map(|c| (floor_run.results[0], c))),
                Some(floor),
                "{l} // {r}"
            );
            let mod_run = lanes(OpKind::Mod);
            assert_eq!(
                raw_int(mod_run.fact.map(|c| (mod_run.results[0], c))),
                Some(modulo),
                "{l} % {r}"
            );
            let div_run = lanes(OpKind::Div);
            match (div_run.fact, &div_run.results[..]) {
                (Some(HeldCarrier::RawF64), &[Value::Float(value)]) => {
                    assert_eq!(value.to_bits(), f64::to_bits(quotient), "{l} / {r}");
                }
                other => panic!("{other:?} is not a raw float"),
            }
            for run in [floor_run, mod_run, div_run] {
                assert!(
                    run.runtime_calls.is_empty(),
                    "a nonzero divisor stays inline"
                );
            }
        }
        for (name, runtime) in [
            (OpKind::Floordiv, RuntimeFn::Floordiv),
            (OpKind::Mod, RuntimeFn::Mod),
            (OpKind::Div, RuntimeFn::Div),
        ] {
            let zero = run(&safe, Lane::Divide(name), &[int(7), int(0)]);
            assert!(zero.fact.is_some());
            assert_eq!(
                zero.runtime_calls,
                [runtime],
                "{name}: a zero divisor raises"
            );
        }
        // i64::MIN // -1 is 2**63 in Python and undefined in a machine: a
        // full-range dividend keeps the runtime for `//` and `/`, unless the
        // plan proves the quotient inline-safe.
        let wide = plan(&[(LHS, Repr::RawI64FullDeopt), (RHS, Repr::RawI64Safe)]);
        for name in [OpKind::Floordiv, OpKind::Div] {
            assert!(lane(&wide, Lane::Divide(name), &[int(i64::MIN), int(-1)]).is_none());
        }
        let proven_quotient = plan(&[
            (LHS, Repr::RawI64FullDeopt),
            (RHS, Repr::RawI64Safe),
            (RESULT, Repr::RawI64Safe),
        ]);
        let quotient = lane(
            &proven_quotient,
            Lane::Divide(OpKind::Floordiv),
            &[int(-(1 << 50)), int(1 << 10)],
        );
        assert_eq!(raw_int(quotient), Some(-(1 << 40)));
        // A full-range quotient (a constant divisor other than 0 and -1).
        let full_quotient = plan(&[
            (LHS, Repr::RawI64FullDeopt),
            (RHS, Repr::RawI64Safe),
            (RESULT, Repr::RawI64FullDeopt),
        ]);
        let quotient = lane(
            &full_quotient,
            Lane::Divide(OpKind::Floordiv),
            &[int(i64::MIN), int(3)],
        );
        assert_eq!(raw_int(quotient), Some(i64::MIN.div_euclid(3)));
        // `%` of any dividend fits; `x % -1` is 0, never a machine trap.
        for (l, r, modulo) in [
            (i64::MIN, -1, 0),
            (i64::MIN, 7, 6),
            (i64::MAX, -10, -3),
            (-(1 << 50), -1, 0),
        ] {
            let run = run(&wide, Lane::Divide(OpKind::Mod), &[int(l), int(r)]);
            assert_eq!(
                raw_int(run.fact.map(|c| (run.results[0], c))),
                Some(modulo),
                "{l} % {r}"
            );
            assert!(run.runtime_calls.is_empty());
        }
        let wide_divisor = plan(&[(LHS, Repr::RawI64Safe), (RHS, Repr::RawI64FullDeopt)]);
        assert!(lane(&wide_divisor, Lane::Divide(OpKind::Div), &[int(1), int(3)]).is_none());
        assert_eq!(
            raw_int(lane(
                &wide_divisor,
                Lane::Divide(OpKind::Floordiv),
                &[int(7), int(i64::MIN)]
            )),
            Some(-1)
        );
    }

    #[test]
    fn float_division_keeps_python_remainder_sign_and_ieee_values() {
        let unproven = plan(&[]);
        let bits = |run: &Run<Option<HeldCarrier>>| match (run.fact, &run.results[..]) {
            (Some(HeldCarrier::RawF64), &[Value::Float(value)]) => value.to_bits(),
            other => panic!("{other:?} is not a raw float"),
        };
        // (dividend, divisor, Python's a % b, a / b)
        for (l, r, modulo, quotient) in [
            (7.5, 2.0, 1.5, 3.75),
            (-7.5, 2.0, 0.5, -3.75),
            (7.5, -2.0, -0.5, -3.75),
            (-7.5, -2.0, -1.5, 3.75),
            (6.0, -3.0, -0.0, -2.0),
            (-6.0, 3.0, 0.0, -2.0),
            (5.0, f64::INFINITY, 5.0, 0.0),
            (-5.0, f64::INFINITY, f64::INFINITY, -0.0),
        ] {
            let modulo_lane = run(&unproven, Lane::Divide(OpKind::Mod), &[float(l), float(r)]);
            assert_eq!(bits(&modulo_lane), f64::to_bits(modulo), "{l} % {r}");
            let quotient_lane = run(&unproven, Lane::Divide(OpKind::Div), &[float(l), float(r)]);
            assert_eq!(bits(&quotient_lane), f64::to_bits(quotient), "{l} / {r}");
            assert!(modulo_lane.runtime_calls.is_empty());
            assert!(quotient_lane.runtime_calls.is_empty());
        }
        let nan = run(
            &unproven,
            Lane::Divide(OpKind::Div),
            &[float(f64::NAN), float(2.0)],
        );
        assert!(f64::from_bits(bits(&nan)).is_nan());
        for zero in [0.0, -0.0] {
            let zero_run = run(
                &unproven,
                Lane::Divide(OpKind::Mod),
                &[float(1.0), float(zero)],
            );
            assert!(zero_run.fact.is_some());
            assert_eq!(
                zero_run.runtime_calls,
                [RuntimeFn::Mod],
                "{zero}: a zero divisor raises"
            );
        }
        // `//` on floats and a mixed pair stay with the runtime.
        assert!(
            lane(
                &unproven,
                Lane::Divide(OpKind::Floordiv),
                &[float(1.0), float(2.0)]
            )
            .is_none()
        );
        let safe = plan(&[(LHS, Repr::RawI64Safe), (RHS, Repr::RawI64Safe)]);
        assert!(lane(&safe, Lane::Divide(OpKind::Div), &[int(1), float(2.0)]).is_none());
    }

    #[test]
    fn unary_ops_take_a_raw_lane_only_where_python_agrees() {
        let unproven = plan(&[]);
        let safe = plan(&[(RESULT, Repr::RawI64Safe)]);
        let unary =
            |plan: &ProducerCarrierPlan, name, operand| lane(plan, Lane::Unary(name), &[operand]);
        // -(i64::MIN) is a BigInt: negation needs the result proof.
        assert!(unary(&unproven, OpKind::Neg, int(5)).is_none());
        assert_eq!(raw_int(unary(&safe, OpKind::Neg, int(5))), Some(-5));
        assert_eq!(
            unary(&unproven, OpKind::Neg, float(0.0)),
            Some((Value::Float(-0.0), HeldCarrier::RawF64))
        );
        assert_eq!(raw_int(unary(&unproven, OpKind::Invert, int(5))), Some(-6));
        assert_eq!(raw_int(unary(&unproven, OpKind::Pos, int(5))), Some(5));
        assert_eq!(
            unary(&unproven, OpKind::Not, int(0)),
            Some((Value::Bit(true), HeldCarrier::RawBool))
        );
        assert_eq!(
            unary(&unproven, OpKind::Not, bit(true)),
            Some((Value::Bit(false), HeldCarrier::RawBool))
        );
        // +True is 1 and ~True is -2: a bool is not its own integer carrier.
        for name in [OpKind::Pos, OpKind::Neg, OpKind::Invert] {
            assert!(unary(&safe, name, bit(true)).is_none(), "{name}");
        }
        // `not 0.0` and `~1.5` go to the runtime.
        assert!(unary(&safe, OpKind::Not, float(0.0)).is_none());
        assert!(unary(&safe, OpKind::Invert, float(1.5)).is_none());
    }
}
