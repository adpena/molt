use super::super::{ConstVal, MAX_COMPOUND_ELEMENTS};
use crate::tir::numeric_facts::{py_i64_floordiv, py_i64_mod, python_range_is_non_empty};
use crate::tir::op_kinds_generated::{
    SccpConstantEvalRule, opcode_accepts_shape, opcode_sccp_constant_eval_rule_table,
};
use crate::tir::ops::OpCode;

/// Try to evaluate a binary/unary op on constant operands.
pub(in crate::tir::passes::sccp) fn evaluate_op(
    opcode: OpCode,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if !opcode_accepts_shape(opcode, operands.len(), 1) {
        return None;
    }
    match opcode_sccp_constant_eval_rule_table(opcode) {
        // Binary arithmetic
        // Use checked arithmetic to avoid panic on overflow in debug / silent wrap in release.
        // On overflow, return None → value stays as Bottom (unfoldable), matching Python's BigInt.
        SccpConstantEvalRule::Add => {
            // Try string concatenation first, then numeric addition.
            eval_str_concat(operands)
                .or_else(|| eval_tuple_concat(operands))
                .or_else(|| eval_binary(operands, |a, b| a.checked_add(b), |a, b| Some(a + b)))
        }
        SccpConstantEvalRule::Sub => {
            eval_binary(operands, |a, b| a.checked_sub(b), |a, b| Some(a - b))
        }
        SccpConstantEvalRule::Mul => {
            // Try immutable sequence repeat first, then numeric multiplication.
            eval_str_repeat(operands)
                .or_else(|| eval_tuple_repeat(operands))
                .or_else(|| eval_binary(operands, |a, b| a.checked_mul(b), |a, b| Some(a * b)))
        }
        SccpConstantEvalRule::Div => eval_binary_div(operands),
        SccpConstantEvalRule::FloorDiv => eval_binary_floordiv(operands),
        SccpConstantEvalRule::Mod => eval_binary_mod(operands),
        SccpConstantEvalRule::Pow => eval_binary_pow(operands),

        // Comparisons
        SccpConstantEvalRule::Eq => eval_cmp(operands, |a, b| a == b, |a, b| a == b, |a, b| a == b),
        SccpConstantEvalRule::Ne => eval_cmp(operands, |a, b| a != b, |a, b| a != b, |a, b| a != b),
        SccpConstantEvalRule::Lt => eval_cmp(operands, |a, b| a < b, |a, b| a < b, |a, b| !a & b),
        SccpConstantEvalRule::Le => eval_cmp(operands, |a, b| a <= b, |a, b| a <= b, |a, b| a <= b),
        SccpConstantEvalRule::Gt => eval_cmp(operands, |a, b| a > b, |a, b| a > b, |a, b| a & !b),
        SccpConstantEvalRule::Ge => eval_cmp(operands, |a, b| a >= b, |a, b| a >= b, |a, b| a >= b),

        // Unary
        SccpConstantEvalRule::Neg => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_neg().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(-v)),
                _ => None,
            }
        }
        SccpConstantEvalRule::Not => {
            let a = operands.first().copied().flatten()?;
            constant_truth_value(a).map(|value| ConstVal::Bool(!value))
        }
        SccpConstantEvalRule::Bool => {
            constant_truth_value(operands.first().copied().flatten()?).map(ConstVal::Bool)
        }

        // Only recursively immutable containers belong to the value lattice.
        SccpConstantEvalRule::BuildTuple => eval_build_tuple(operands),

        SccpConstantEvalRule::None => None,
    }
}

/// Truthiness of exact immutable values. This belongs to Bool/Not operations,
/// not a generic callable whose lookup name happens to be "bool".
fn constant_truth_value(value: &ConstVal) -> Option<bool> {
    Some(match value {
        ConstVal::Int(value) => *value != 0,
        ConstVal::Float(value) => *value != 0.0,
        ConstVal::Bool(value) => *value,
        ConstVal::Str(value) => !value.is_empty(),
        ConstVal::None => false,
        ConstVal::Tuple(elements) => !elements.is_empty(),
        ConstVal::Range { start, stop, step } => python_range_is_non_empty(*start, *stop, *step)?,
    })
}

/// Fold string concatenation: "a" + "b" → "ab".
fn eval_str_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Str(x), ConstVal::Str(y)) => {
            if x.len().checked_add(y.len())? <= MAX_COMPOUND_ELEMENTS {
                Some(ConstVal::Str(format!("{}{}", x, y)))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Fold immutable tuple concatenation: (1, 2) + (3, 4) → (1, 2, 3, 4).
fn eval_tuple_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Tuple(x), ConstVal::Tuple(y)) => {
            let cost = a
                .materialization_cost()?
                .checked_add(b.materialization_cost()?)?
                - 1;
            if cost <= MAX_COMPOUND_ELEMENTS {
                let mut result = x.to_vec();
                result.extend(y.iter().cloned());
                Some(ConstVal::Tuple(result.into()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Fold string repeat: "ab" * 3 → "ababab".
fn eval_str_repeat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Str(s), ConstVal::Int(n)) | (ConstVal::Int(n), ConstVal::Str(s)) => {
            if *n <= 0 || s.is_empty() {
                Some(ConstVal::Str(String::new()))
            } else {
                let count = usize::try_from(*n).ok()?;
                let result_len = s.len().checked_mul(count)?;
                if result_len <= MAX_COMPOUND_ELEMENTS {
                    Some(ConstVal::Str(s.repeat(count)))
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

/// Fold immutable tuple repetition: (1, 2) * 3 → (1, 2, 1, 2, 1, 2).
fn eval_tuple_repeat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    let (value, tuple, n) = match (a, b) {
        (ConstVal::Tuple(t), ConstVal::Int(n)) => (a, t, *n),
        (ConstVal::Int(n), ConstVal::Tuple(t)) => (b, t, *n),
        _ => return None,
    };
    if n <= 0 || tuple.is_empty() {
        return Some(ConstVal::Tuple(Vec::new().into()));
    }
    let count = usize::try_from(n).ok()?;
    let cost = (value.materialization_cost()? - 1)
        .checked_mul(count)?
        .checked_add(1)?;
    let total = tuple.len().checked_mul(count)?;
    if cost > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let mut result = Vec::with_capacity(total);
    for _ in 0..count {
        result.extend(tuple.iter().cloned());
    }
    Some(ConstVal::Tuple(result.into()))
}

/// Fold BuildTuple with recursively immutable operands to ConstVal::Tuple.
fn eval_build_tuple(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    operands.iter().try_fold(1usize, |cost, operand| {
        let cost = cost.checked_add((*operand)?.materialization_cost()?)?;
        (cost <= MAX_COMPOUND_ELEMENTS).then_some(cost)
    })?;
    let elements: Vec<ConstVal> = operands
        .iter()
        .map(|o| o.map(|v| (*v).clone()))
        .collect::<Option<Vec<_>>>()?;
    Some(ConstVal::Tuple(elements.into()))
}

/// Evaluate a binary arithmetic op on int or float operands.
/// Int operations use checked arithmetic — returns None on overflow
/// (matching Python's BigInt promotion behavior: we can't fold it, so leave it unfoldable).
fn eval_binary(
    operands: &[Option<&ConstVal>],
    int_op: impl Fn(i64, i64) -> Option<i64>,
    float_op: impl Fn(f64, f64) -> Option<f64>,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => int_op(*x, *y).map(ConstVal::Int),
        (ConstVal::Float(x), ConstVal::Float(y)) => {
            float_op(*x, *y).and_then(float_arithmetic_result)
        }
        _ => None,
    }
}

fn float_arithmetic_result(value: f64) -> Option<ConstVal> {
    // IEEE arithmetic does not establish a cross-target NaN sign/payload.
    // Keep those computations executable; comparisons and bit-preserving
    // operand selections can still fold without inventing a new NaN value.
    (!value.is_nan()).then_some(ConstVal::Float(value))
}

fn eval_binary_div(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y))
            if *y != 0 && exactly_representable_i64(*x) && exactly_representable_i64(*y) =>
        {
            // Python rounds the exact integer ratio, not separately rounded
            // operands. Host division is admitted only when both casts are exact.
            Some(ConstVal::Float(*x as f64 / *y as f64))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => float_arithmetic_result(*x / *y),
        _ => None,
    }
}

fn exactly_representable_i64(value: i64) -> bool {
    // Widen the round-trip comparison: an i64 cast would saturate 2^63 back
    // to i64::MAX and incorrectly admit that rounded endpoint.
    (value as f64) as i128 == i128::from(value)
}

fn eval_binary_floordiv(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => py_i64_floordiv(*x, *y).map(ConstVal::Int),
        // Python float divmod uses a shared quotient/remainder correction;
        // floor(x / y) is not equivalent (for example, 1.0 // 0.1 is 9.0).
        // Preserve float evaluation until a target-semantic primitive exists.
        _ => None,
    }
}

fn eval_binary_mod(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => py_i64_mod(*x, *y).map(ConstVal::Int),
        // A host remainder plus sign adjustment is not Python float divmod:
        // signed zero and quotient correction require the shared primitive.
        _ => None,
    }
}

fn eval_binary_pow(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(base), ConstVal::Int(exp)) => {
            if *exp >= 0 && *exp <= 63 {
                // Safe small exponent — use checked pow to avoid overflow panic.
                // A negative exponent (`2 ** -1` → float `0.5`, `0 ** -1` →
                // ZeroDivisionError) is intentionally NOT folded here: it leaves
                // the int domain, so the runtime `Pow` op (which is float- and
                // exception-correct) handles it. `exp == 0` is `1` for any base.
                base.checked_pow(*exp as u32).map(ConstVal::Int)
            } else {
                None
            }
        }
        // Finite host powf output does not prove target rounding, Python's
        // exception behavior, or complex-result selection. Preserve float Pow
        // until a shared target semantic primitive establishes those facts.
        _ => None,
    }
}

/// Evaluate a comparison op.
fn eval_cmp(
    operands: &[Option<&ConstVal>],
    int_cmp: impl Fn(i64, i64) -> bool,
    float_cmp: impl Fn(f64, f64) -> bool,
    bool_cmp: impl Fn(bool, bool) -> bool,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Bool(int_cmp(*x, *y))),
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Bool(float_cmp(*x, *y))),
        (ConstVal::Bool(x), ConstVal::Bool(y)) => Some(ConstVal::Bool(bool_cmp(*x, *y))),
        _ => None,
    }
}
