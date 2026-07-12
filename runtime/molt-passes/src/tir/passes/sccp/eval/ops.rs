use super::super::{ConstVal, MAX_COMPOUND_ELEMENTS};
use crate::tir::numeric_facts::{py_i64_floordiv, py_i64_mod};
use crate::tir::op_kinds_generated::{SccpConstantEvalRule, opcode_sccp_constant_eval_rule_table};
use crate::tir::ops::OpCode;

/// Try to evaluate a binary/unary op on constant operands.
pub(in crate::tir::passes::sccp) fn evaluate_op(
    opcode: OpCode,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    match opcode_sccp_constant_eval_rule_table(opcode) {
        // Binary arithmetic
        // Use checked arithmetic to avoid panic on overflow in debug / silent wrap in release.
        // On overflow, return None → value stays as Bottom (unfoldable), matching Python's BigInt.
        SccpConstantEvalRule::Add => {
            // Try string concatenation first, then numeric addition.
            eval_str_concat(operands)
                .or_else(|| eval_list_concat(operands))
                .or_else(|| eval_binary(operands, |a, b| a.checked_add(b), |a, b| Some(a + b)))
        }
        SccpConstantEvalRule::Sub => {
            eval_binary(operands, |a, b| a.checked_sub(b), |a, b| Some(a - b))
        }
        SccpConstantEvalRule::Mul => {
            // Try string/list repeat first, then numeric multiplication.
            eval_str_repeat(operands)
                .or_else(|| eval_list_repeat(operands))
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
            match a {
                ConstVal::Bool(v) => Some(ConstVal::Bool(!v)),
                _ => None,
            }
        }

        // Container construction with all-constant elements.
        SccpConstantEvalRule::BuildList => eval_build_list(operands),
        SccpConstantEvalRule::BuildDict => eval_build_dict(operands),
        // Tuples fold to List for SCCP purposes.
        SccpConstantEvalRule::BuildTupleAsList => eval_build_list(operands),

        SccpConstantEvalRule::None => None,
    }
}

/// Fold string concatenation: "a" + "b" → "ab".
fn eval_str_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Str(x), ConstVal::Str(y)) => {
            let result = format!("{}{}", x, y);
            if result.len() <= MAX_COMPOUND_ELEMENTS {
                Some(ConstVal::Str(result))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Fold list concatenation: [1,2] + [3,4] → [1,2,3,4].
fn eval_list_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::List(x), ConstVal::List(y)) => {
            let total = x.len() + y.len();
            if total <= MAX_COMPOUND_ELEMENTS {
                let mut result = x.clone();
                result.extend(y.iter().cloned());
                Some(ConstVal::List(result))
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
            if *n <= 0 {
                Some(ConstVal::Str(String::new()))
            } else {
                let count = *n as usize;
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

/// Fold list repeat: [1,2] * 3 → [1,2,1,2,1,2].
fn eval_list_repeat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    let (list, n) = match (a, b) {
        (ConstVal::List(l), ConstVal::Int(n)) => (l, *n),
        (ConstVal::Int(n), ConstVal::List(l)) => (l, *n),
        _ => return None,
    };
    if n <= 0 {
        return Some(ConstVal::List(Vec::new()));
    }
    let count = n as usize;
    let total = list.len().checked_mul(count)?;
    if total > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let mut result = Vec::with_capacity(total);
    for _ in 0..count {
        result.extend(list.iter().cloned());
    }
    Some(ConstVal::List(result))
}

/// Fold BuildList with all-constant operands to ConstVal::List.
fn eval_build_list(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    if operands.len() > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let elements: Vec<ConstVal> = operands
        .iter()
        .map(|o| o.map(|v| (*v).clone()))
        .collect::<Option<Vec<_>>>()?;
    Some(ConstVal::List(elements))
}

/// Fold BuildDict with all-constant operands to ConstVal::Dict.
/// Dict operands are laid out as [k1, v1, k2, v2, ...].
fn eval_build_dict(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    if !operands.len().is_multiple_of(2) {
        return None;
    }
    let n_entries = operands.len() / 2;
    if n_entries > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let mut entries = Vec::with_capacity(n_entries);
    for i in 0..n_entries {
        let k = operands[i * 2]?.clone();
        let v = operands[i * 2 + 1]?.clone();
        entries.push((k, v));
    }
    Some(ConstVal::Dict(entries))
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
        (ConstVal::Float(x), ConstVal::Float(y)) => float_op(*x, *y).map(ConstVal::Float),
        _ => None,
    }
}

fn eval_binary_div(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python `/` on ints returns float
            Some(ConstVal::Float(*x as f64 / *y as f64))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => Some(ConstVal::Float(*x / *y)),
        _ => None,
    }
}

fn eval_binary_floordiv(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => py_i64_floordiv(*x, *y).map(ConstVal::Int),
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            Some(ConstVal::Float((*x / *y).floor()))
        }
        _ => None,
    }
}

fn eval_binary_mod(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => py_i64_mod(*x, *y).map(ConstVal::Int),
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            // Python modulo semantics
            let r = *x % *y;
            let result = if r != 0.0 && r.signum() != y.signum() {
                r + *y
            } else {
                r
            };
            Some(ConstVal::Float(result))
        }
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
        (ConstVal::Float(x), ConstVal::Float(y)) => {
            // `float ** float` may diverge from a real, finite float — and SCCP's
            // `ConstVal` lattice cannot represent those results, so folding them
            // would be a silent miscompile. CPython's `float.__pow__`:
            //   * `0.0 ** negative`  → raises ZeroDivisionError (observable)
            //   * `negative ** non-integer` → returns `complex` (NOT a float)
            //   * any result that is inf/NaN (overflow / domain edge) likewise
            //     cannot be trusted to match CPython's value/exception contract.
            // Refuse to fold in every one of those cases (return None → the op
            // stays as Bottom and the runtime evaluates it). Only a finite real
            // float result that the IEEE `powf` reproduces exactly is folded.
            if *x == 0.0 && *y < 0.0 {
                return None; // ZeroDivisionError at runtime
            }
            if *x < 0.0 && y.fract() != 0.0 {
                return None; // complex result at runtime
            }
            let result = x.powf(*y);
            if result.is_finite() {
                Some(ConstVal::Float(result))
            } else {
                None
            }
        }
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
