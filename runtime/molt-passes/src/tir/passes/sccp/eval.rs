//! Constant-folding evaluation for SCCP.
//!
//! Pure functions that concretely evaluate a TIR op (arithmetic, comparison,
//! container construction, pure builtin/method calls) on already-constant
//! operands, returning the folded [`ConstVal`] or `None` when the op cannot be
//! soundly folded. Split out of `sccp.rs` as a move-only decomposition; the
//! lattice driver and rewrite live in the parent [`super`] module.

use super::{ConstVal, MAX_COMPOUND_ELEMENTS};
use crate::tir::numeric_facts::{
    py_i64_floordiv, py_i64_mod, python_range_is_non_empty, python_range_len,
};
use crate::tir::op_kinds_generated::{SccpConstantEvalRule, opcode_sccp_constant_eval_rule_table};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::effects;

/// Translate a UTF-8 byte offset into a Python code-point index.
///
/// Rust's `str::find`/`rfind` return byte offsets, but Python string index
/// APIs (`find`, `rfind`, `index`) are defined over code points. Folding the
/// raw byte offset silently miscompiles any receiver containing a non-ASCII
/// character before the match. `byte_off` is assumed to be a valid char
/// boundary within `s` (it always is when produced by `str::find`/`rfind`).
fn byte_offset_to_char_index(s: &str, byte_off: usize) -> i64 {
    s[..byte_off].chars().count() as i64
}

/// Try to evaluate a binary/unary op on constant operands.
pub(super) fn evaluate_op(opcode: OpCode, operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
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

/// Try to concrete-eval a `CallBuiltin` op when all operands are constant
/// and the callee is a known pure builtin.
pub(super) fn evaluate_builtin_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallBuiltin {
        return None;
    }
    let name = match op.attrs.get("name") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    let fx = effects::builtin_effects(name)?;
    if !fx.is_pure() {
        return None;
    }
    eval_concrete_builtin(name, operands)
}

/// Try to concrete-eval a `CallMethod` op when the receiver is a known
/// constant and the method is pure.
///
/// `op.attrs["method"]` may carry either:
///   - the full frontend `BoundMethod:<receiver>:<method>` string
///     (production canonical form, set by the SSA lift from the
///     SimpleIR `call_method` op's `s_value` field), OR
///   - a bare method name (test / future-refined form).
///
/// We strip the `BoundMethod:` prefix and discard the embedded
/// receiver name, keeping the bare method.  The actual receiver
/// type for the effects lookup comes from the constant operand's
/// runtime type — that's stricter than what's encoded in the hint
/// (the hint reflects the frontend's static guess; the constant
/// operand reflects what's actually flowing through SCCP).
pub(super) fn evaluate_method_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallMethod {
        return None;
    }
    let method_attr = match op.attrs.get("method") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    // Strip the `BoundMethod:<rcv>:` prefix when present.  We keep
    // only the bare method name; the receiver type comes from the
    // constant operand below.  Empty-component guards mirror the
    // escape-analysis parse to keep the contract symmetric.
    let method = if let Some(rest) = method_attr.strip_prefix("BoundMethod:") {
        let mut parts = rest.splitn(2, ':');
        match (parts.next(), parts.next()) {
            (Some(_rcv), Some(mthd)) if !mthd.is_empty() => mthd,
            _ => return None,
        }
    } else {
        method_attr
    };
    let receiver = operands.first().copied().flatten()?;
    let receiver_type = match receiver {
        ConstVal::Str(_) => "str",
        ConstVal::Int(_) => "int",
        ConstVal::Float(_) => "float",
        ConstVal::Bool(_) => "bool",
        ConstVal::List(_) => "list",
        ConstVal::Dict(_) => "dict",
        ConstVal::Range { .. } => return None,
        ConstVal::None => return None,
    };
    let fx = effects::method_effects(receiver_type, method)?;
    if !fx.is_pure() {
        return None;
    }
    eval_concrete_method(receiver_type, method, operands)
}

/// Concrete evaluation of known pure builtins.
/// CPython `str(float)` == `repr(float)` (identical since Py3). Fold ONLY where
/// Rust's shortest-round-trip `Display` provably matches CPython: the
/// non-scientific finite regime. CPython switches to exponential notation for a
/// decimal exponent < -4 or >= 16 (i.e. `abs >= 1e16` or `0 < abs < 1e-4`) and
/// spells non-finite values `nan`/`inf`/`-inf` — in those cases Rust's `Display`
/// diverges, so we DON'T fold and defer to the correct runtime formatter. Whole
/// finite values keep the trailing `.0` (`{:.1}`) exactly as CPython does.
fn fold_float_str(v: f64) -> Option<ConstVal> {
    let av = v.abs();
    if !v.is_finite() || (av != 0.0 && (av >= 1e16 || av < 1e-4)) {
        return None;
    }
    let s = if v.fract() == 0.0 {
        format!("{:.1}", v)
    } else {
        format!("{}", v)
    };
    Some(ConstVal::Str(s))
}

/// CPython `repr(str)`. Fold ONLY the case where a single-quoted, unescaped
/// rendering is byte-for-byte what CPython produces: printable-ASCII content
/// (0x20..=0x7e) with no single quote (which flips CPython to double quotes) and
/// no backslash (which needs escaping). Anything else — a `'`, a `\`, control
/// chars, or non-ASCII needing `\x`/`\u` escapes — is deferred to the runtime,
/// which reproduces CPython's quote-selection and escaping.
fn fold_repr_str(s: &str) -> Option<ConstVal> {
    if s.bytes().all(|b| (0x20..=0x7e).contains(&b)) && !s.contains('\'') && !s.contains('\\') {
        Some(ConstVal::Str(format!("'{}'", s)))
    } else {
        None
    }
}

fn eval_concrete_builtin(name: &str, operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    match name {
        "len" => {
            let a = operands.first().copied().flatten()?;
            match a {
                // Python `len(str)` counts Unicode code points, NOT UTF-8
                // bytes. Folding `s.len()` here silently miscompiles every
                // non-ASCII constant (e.g. len("café") == 4, not 5).
                ConstVal::Str(s) => Some(ConstVal::Int(s.chars().count() as i64)),
                ConstVal::List(elems) => Some(ConstVal::Int(elems.len() as i64)),
                ConstVal::Dict(entries) => Some(ConstVal::Int(entries.len() as i64)),
                ConstVal::Range { start, stop, step } => {
                    // Python: len(range(start, stop, step))
                    python_range_len(*start, *stop, *step).map(ConstVal::Int)
                }
                _ => None,
            }
        }
        "abs" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_abs().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(v.abs())),
                _ => None,
            }
        }
        "bool" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Bool(*v != 0)),
                ConstVal::Float(v) => Some(ConstVal::Bool(*v != 0.0)),
                ConstVal::Bool(v) => Some(ConstVal::Bool(*v)),
                ConstVal::Str(s) => Some(ConstVal::Bool(!s.is_empty())),
                ConstVal::None => Some(ConstVal::Bool(false)),
                ConstVal::List(elems) => Some(ConstVal::Bool(!elems.is_empty())),
                ConstVal::Dict(entries) => Some(ConstVal::Bool(!entries.is_empty())),
                ConstVal::Range { start, stop, step } => {
                    python_range_is_non_empty(*start, *stop, *step).map(ConstVal::Bool)
                }
            }
        }
        "int" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                ConstVal::Float(v) => Some(ConstVal::Int(*v as i64)),
                ConstVal::Bool(v) => Some(ConstVal::Int(if *v { 1 } else { 0 })),
                _ => None,
            }
        }
        "float" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Float(*v as f64)),
                ConstVal::Float(v) => Some(ConstVal::Float(*v)),
                ConstVal::Bool(v) => Some(ConstVal::Float(if *v { 1.0 } else { 0.0 })),
                _ => None,
            }
        }
        "str" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Str(v.to_string())),
                ConstVal::Float(v) => fold_float_str(*v),
                ConstVal::Bool(v) => {
                    Some(ConstVal::Str(if *v { "True" } else { "False" }.to_string()))
                }
                ConstVal::Str(s) => Some(ConstVal::Str(s.clone())),
                ConstVal::None => Some(ConstVal::Str("None".to_string())),
                _ => None, // compound types don't fold to str
            }
        }
        "repr" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Str(v.to_string())),
                ConstVal::Float(v) => fold_float_str(*v),
                ConstVal::Bool(v) => {
                    Some(ConstVal::Str(if *v { "True" } else { "False" }.to_string()))
                }
                ConstVal::Str(s) => fold_repr_str(s),
                ConstVal::None => Some(ConstVal::Str("None".to_string())),
                _ => None, // compound types don't fold to repr
            }
        }
        "chr" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                if *v >= 0 && *v <= 0x10FFFF {
                    char::from_u32(*v as u32).map(|c| ConstVal::Str(c.to_string()))
                } else {
                    None
                }
            } else {
                None
            }
        }
        "ord" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Str(s) = a {
                let mut chars = s.chars();
                let first = chars.next()?;
                if chars.next().is_none() {
                    Some(ConstVal::Int(first as i64))
                } else {
                    None
                }
            } else {
                None
            }
        }
        "hex" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0x{:x}", -v)
                } else {
                    format!("0x{:x}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "oct" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0o{:o}", -v)
                } else {
                    format!("0o{:o}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "bin" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0b{:b}", -v)
                } else {
                    format!("0b{:b}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "range" => {
            // range(stop), range(start, stop), range(start, stop, step)
            match operands.len() {
                1 => {
                    let stop = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    Some(ConstVal::Range {
                        start: 0,
                        stop,
                        step: 1,
                    })
                }
                2 => {
                    let start = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let stop = match operands[1]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    Some(ConstVal::Range {
                        start,
                        stop,
                        step: 1,
                    })
                }
                3 => {
                    let start = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let stop = match operands[1]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let step = match operands[2]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    if step == 0 {
                        return None; // ValueError in Python
                    }
                    Some(ConstVal::Range { start, stop, step })
                }
                _ => None,
            }
        }
        "sorted" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::List(elems) => {
                    // Only sort homogeneous int lists (Python raises TypeError
                    // on mixed types like int < str).
                    let mut ints = Vec::with_capacity(elems.len());
                    for e in elems {
                        match e {
                            ConstVal::Int(v) => ints.push(*v),
                            _ => return None,
                        }
                    }
                    ints.sort();
                    Some(ConstVal::List(
                        ints.into_iter().map(ConstVal::Int).collect(),
                    ))
                }
                _ => None,
            }
        }
        "sum" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::List(elems) => {
                    // sum([int, int, ...]) → int
                    let mut total: i64 = 0;
                    for elem in elems {
                        match elem {
                            ConstVal::Int(v) => {
                                total = total.checked_add(*v)?;
                            }
                            _ => return None,
                        }
                    }
                    Some(ConstVal::Int(total))
                }
                _ => None,
            }
        }
        "min" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::min(*x, *y))),
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.min(*y))),
                _ => None,
            }
        }
        "max" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::max(*x, *y))),
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.max(*y))),
                _ => None,
            }
        }
        "math.sqrt" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) if *v >= 0.0 => Some(ConstVal::Float(v.sqrt())),
                ConstVal::Int(v) if *v >= 0 => Some(ConstVal::Float((*v as f64).sqrt())),
                _ => None,
            }
        }
        "math.floor" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.floor() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.ceil" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.ceil() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.log" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) if *v > 0.0 => Some(ConstVal::Float(v.ln())),
                ConstVal::Int(v) if *v > 0 => Some(ConstVal::Float((*v as f64).ln())),
                _ => None,
            }
        }
        "math.exp" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Float(v.exp())),
                ConstVal::Int(v) => Some(ConstVal::Float((*v as f64).exp())),
                _ => None,
            }
        }
        "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan" => {
            let a = operands.first().copied().flatten()?;
            let v = match a {
                ConstVal::Float(v) => *v,
                ConstVal::Int(v) => *v as f64,
                _ => return None,
            };
            let result = match name {
                "math.sin" => v.sin(),
                "math.cos" => v.cos(),
                "math.tan" => v.tan(),
                "math.asin" => v.asin(),
                "math.acos" => v.acos(),
                "math.atan" => v.atan(),
                _ => unreachable!(),
            };
            Some(ConstVal::Float(result))
        }
        "math.fabs" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Float(v.abs())),
                ConstVal::Int(v) => Some(ConstVal::Float((*v as f64).abs())),
                _ => None,
            }
        }
        "math.trunc" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.trunc() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.isfinite" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_finite())),
                ConstVal::Int(_) => Some(ConstVal::Bool(true)),
                _ => None,
            }
        }
        "math.isinf" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_infinite())),
                ConstVal::Int(_) => Some(ConstVal::Bool(false)),
                _ => None,
            }
        }
        "math.isnan" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_nan())),
                ConstVal::Int(_) => Some(ConstVal::Bool(false)),
                _ => None,
            }
        }
        "math.copysign" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.copysign(*y))),
                _ => None,
            }
        }
        "math.pow" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.powf(*y))),
                _ => None,
            }
        }
        "math.atan2" | "math.hypot" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => {
                    let result = if name == "math.atan2" {
                        x.atan2(*y)
                    } else {
                        x.hypot(*y)
                    };
                    Some(ConstVal::Float(result))
                }
                _ => None,
            }
        }
        "math.gcd" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            if let (ConstVal::Int(x), ConstVal::Int(y)) = (a, b) {
                fn gcd(mut a: i64, mut b: i64) -> i64 {
                    a = a.abs();
                    b = b.abs();
                    while b != 0 {
                        let t = b;
                        b = a % b;
                        a = t;
                    }
                    a
                }
                Some(ConstVal::Int(gcd(*x, *y)))
            } else {
                None
            }
        }
        "math.lcm" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            if let (ConstVal::Int(x), ConstVal::Int(y)) = (a, b) {
                if *x == 0 || *y == 0 {
                    Some(ConstVal::Int(0))
                } else {
                    fn gcd(mut a: i64, mut b: i64) -> i64 {
                        a = a.abs();
                        b = b.abs();
                        while b != 0 {
                            let t = b;
                            b = a % b;
                            a = t;
                        }
                        a
                    }
                    let g = gcd(*x, *y);
                    x.checked_div(g)
                        .and_then(|q| q.checked_mul(*y))
                        .map(|v| ConstVal::Int(v.abs()))
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Concrete evaluation of known pure methods on constant receivers.
fn eval_concrete_method(
    receiver_type: &str,
    method: &str,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    let receiver = operands.first().copied().flatten()?;
    match receiver_type {
        "str" => {
            let s = if let ConstVal::Str(s) = receiver {
                s
            } else {
                return None;
            };
            match method {
                "upper" => Some(ConstVal::Str(s.to_uppercase())),
                "lower" => Some(ConstVal::Str(s.to_lowercase())),
                "strip" => Some(ConstVal::Str(s.trim().to_string())),
                "lstrip" => Some(ConstVal::Str(s.trim_start().to_string())),
                "rstrip" => Some(ConstVal::Str(s.trim_end().to_string())),
                "title" => {
                    let mut result = String::with_capacity(s.len());
                    let mut prev_is_boundary = true;
                    for c in s.chars() {
                        if prev_is_boundary && c.is_alphabetic() {
                            for uc in c.to_uppercase() {
                                result.push(uc);
                            }
                            prev_is_boundary = false;
                        } else if !c.is_alphabetic() {
                            result.push(c);
                            prev_is_boundary = true;
                        } else {
                            for lc in c.to_lowercase() {
                                result.push(lc);
                            }
                            prev_is_boundary = false;
                        }
                    }
                    Some(ConstVal::Str(result))
                }
                "capitalize" => {
                    let mut chars = s.chars();
                    let result = match chars.next() {
                        Some(c) => {
                            let upper: String = c.to_uppercase().collect();
                            let lower: String = chars.flat_map(|c| c.to_lowercase()).collect();
                            format!("{}{}", upper, lower)
                        }
                        None => String::new(),
                    };
                    Some(ConstVal::Str(result))
                }
                "swapcase" => {
                    let result: String = s
                        .chars()
                        .flat_map(|c| {
                            if c.is_uppercase() {
                                c.to_lowercase().collect::<Vec<_>>()
                            } else if c.is_lowercase() {
                                c.to_uppercase().collect::<Vec<_>>()
                            } else {
                                vec![c]
                            }
                        })
                        .collect();
                    Some(ConstVal::Str(result))
                }
                "isalpha" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_alphabetic()),
                )),
                "isdigit" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()),
                )),
                "isalnum" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()),
                )),
                "isspace" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_whitespace()),
                )),
                "isupper" => Some(ConstVal::Bool(
                    s.chars().any(|c| c.is_uppercase()) && !s.chars().any(|c| c.is_lowercase()),
                )),
                "islower" => Some(ConstVal::Bool(
                    s.chars().any(|c| c.is_lowercase()) && !s.chars().any(|c| c.is_uppercase()),
                )),
                "startswith" => {
                    let prefix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = prefix {
                        Some(ConstVal::Bool(s.starts_with(p.as_str())))
                    } else {
                        None
                    }
                }
                "endswith" => {
                    let suffix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = suffix {
                        Some(ConstVal::Bool(s.ends_with(p.as_str())))
                    } else {
                        None
                    }
                }
                "find" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        // Python `str.find` returns a code-point index; Rust's
                        // `str::find` returns a UTF-8 byte offset. Translate so
                        // non-ASCII receivers ("héllo".find("llo") == 2, not 3)
                        // are not silently miscompiled.
                        let idx = s
                            .find(n.as_str())
                            .map(|byte_off| byte_offset_to_char_index(s, byte_off))
                            .unwrap_or(-1);
                        Some(ConstVal::Int(idx))
                    } else {
                        None
                    }
                }
                "rfind" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        let idx = s
                            .rfind(n.as_str())
                            .map(|byte_off| byte_offset_to_char_index(s, byte_off))
                            .unwrap_or(-1);
                        Some(ConstVal::Int(idx))
                    } else {
                        None
                    }
                }
                "count" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        if n.is_empty() {
                            // Python: "abc".count("") == 4 (code-point len + 1).
                            // Must count code points, not UTF-8 bytes, so
                            // "café".count("") == 5 (not 6).
                            Some(ConstVal::Int(s.chars().count() as i64 + 1))
                        } else {
                            Some(ConstVal::Int(s.matches(n.as_str()).count() as i64))
                        }
                    } else {
                        None
                    }
                }
                "replace" => {
                    if operands.len() < 3 {
                        return None;
                    }
                    let old = operands[1]?;
                    let new = operands[2]?;
                    if let (ConstVal::Str(o), ConstVal::Str(n)) = (old, new) {
                        Some(ConstVal::Str(s.replace(o.as_str(), n.as_str())))
                    } else {
                        None
                    }
                }
                "removeprefix" => {
                    let prefix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = prefix {
                        let result = s.strip_prefix(p.as_str()).unwrap_or(s);
                        Some(ConstVal::Str(result.to_string()))
                    } else {
                        None
                    }
                }
                "removesuffix" => {
                    let suffix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = suffix {
                        let result = s.strip_suffix(p.as_str()).unwrap_or(s);
                        Some(ConstVal::Str(result.to_string()))
                    } else {
                        None
                    }
                }
                "zfill" => {
                    let width = operands.get(1).copied().flatten()?;
                    if let ConstVal::Int(w) = width {
                        // Python pads to `width` CODE POINTS, not bytes, and a
                        // width <= current length (including a negative width)
                        // returns the string unchanged. Using `s.len()` bytes
                        // both miscompiled non-ASCII ("é".zfill(3) == "00é") and
                        // `*w as usize` on a negative width wrapped to a huge
                        // value, driving `"0".repeat(...)` into a compile-time
                        // OOM/panic.
                        let char_len = s.chars().count() as i64;
                        if *w <= char_len {
                            Some(ConstVal::Str(s.clone()))
                        } else {
                            let (prefix, body) = if s.starts_with('-') || s.starts_with('+') {
                                (&s[..1], &s[1..])
                            } else {
                                ("", s.as_str())
                            };
                            let fill = (*w - char_len) as usize;
                            Some(ConstVal::Str(format!(
                                "{}{}{}",
                                prefix,
                                "0".repeat(fill),
                                body
                            )))
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        "int" => {
            let v = if let ConstVal::Int(v) = receiver {
                *v
            } else {
                return None;
            };
            match method {
                "bit_length" => {
                    if v == 0 {
                        Some(ConstVal::Int(0))
                    } else {
                        Some(ConstVal::Int(64 - v.abs().leading_zeros() as i64))
                    }
                }
                "bit_count" => Some(ConstVal::Int(v.unsigned_abs().count_ones() as i64)),
                _ => None,
            }
        }
        "float" => {
            let v = if let ConstVal::Float(v) = receiver {
                *v
            } else {
                return None;
            };
            match method {
                "is_integer" => Some(ConstVal::Bool(v.fract() == 0.0 && v.is_finite())),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod unicode_fold_tests {
    //! Teeth for finding #5: SCCP folding of str builtins/methods must use
    //! CPython code-point semantics, never Rust UTF-8 byte offsets. Every
    //! expected value below was captured from the CPython 3.12 reference
    //! interpreter. A regression to byte semantics flips these and fails here.
    use super::{ConstVal, eval_concrete_builtin, eval_concrete_method};

    fn s(v: &str) -> ConstVal {
        ConstVal::Str(v.to_string())
    }

    fn builtin(name: &str, args: &[ConstVal]) -> Option<ConstVal> {
        let ops: Vec<Option<&ConstVal>> = args.iter().map(Some).collect();
        eval_concrete_builtin(name, &ops)
    }

    fn method(recv_ty: &str, m: &str, args: &[ConstVal]) -> Option<ConstVal> {
        let ops: Vec<Option<&ConstVal>> = args.iter().map(Some).collect();
        eval_concrete_method(recv_ty, m, &ops)
    }

    #[test]
    fn len_counts_code_points_not_bytes() {
        // CPython: len("café") == 4, len("héllo") == 5, len("a😀b") == 3.
        assert_eq!(builtin("len", &[s("café")]), Some(ConstVal::Int(4)));
        assert_eq!(builtin("len", &[s("héllo")]), Some(ConstVal::Int(5)));
        assert_eq!(builtin("len", &[s("a😀b")]), Some(ConstVal::Int(3)));
        // ASCII fast path unchanged.
        assert_eq!(builtin("len", &[s("abc")]), Some(ConstVal::Int(3)));
    }

    #[test]
    fn str_repr_fold_matches_cpython_or_refuses() {
        // Float str/repr: FOLD the non-scientific finite regime to CPython's exact
        // output; REFUSE (return None → defer to the correct runtime formatter) for
        // scientific-notation and non-finite values, where Rust's Display diverges.
        // Expectations captured from the CPython 3.12 reference interpreter.
        for (v, expect) in [
            (0.0_f64, "0.0"),
            (-0.0, "-0.0"),
            (1.5, "1.5"),
            (100.0, "100.0"),
            (0.1, "0.1"),
            (1234.5678, "1234.5678"),
            (0.0001, "0.0001"),
            (9.5e15, "9500000000000000.0"),
        ] {
            let want = Some(ConstVal::Str(expect.to_string()));
            assert_eq!(builtin("str", &[ConstVal::Float(v)]), want, "str({v})");
            assert_eq!(builtin("repr", &[ConstVal::Float(v)]), want, "repr({v})");
        }
        for v in [1e-5_f64, 1e16, 1e17, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert_eq!(builtin("str", &[ConstVal::Float(v)]), None, "str({v}) must defer");
            assert_eq!(builtin("repr", &[ConstVal::Float(v)]), None, "repr({v}) must defer");
        }
        // repr(str): FOLD only simple printable-ASCII with no `'` and no `\`; REFUSE
        // (defer) the quote-selection / escaping cases. CPython 3.12 ground truth:
        // repr("it's") == "\"it's\"" (double-quoted), repr("a\\b") escapes, etc.
        for (input, expect) in [
            ("abc", "'abc'"),
            ("a b c", "'a b c'"),
            ("a\"b", "'a\"b'"),
            ("x!@#$%", "'x!@#$%'"),
        ] {
            assert_eq!(
                builtin("repr", &[s(input)]),
                Some(ConstVal::Str(expect.to_string())),
                "repr({input:?})"
            );
        }
        for input in ["it's", "a\\b", "a\nb", "café"] {
            assert_eq!(builtin("repr", &[s(input)]), None, "repr({input:?}) must defer");
        }
        // str(str) is identity — unchanged.
        assert_eq!(builtin("str", &[s("café")]), Some(s("café")));
    }

    #[test]
    fn find_returns_code_point_index() {
        // CPython: "héllo".find("llo") == 2 (byte offset would be 3).
        assert_eq!(
            method("str", "find", &[s("héllo"), s("llo")]),
            Some(ConstVal::Int(2))
        );
        // Astral needle placement: "a😀b".find("b") == 2 (byte offset 5).
        assert_eq!(
            method("str", "find", &[s("a😀b"), s("b")]),
            Some(ConstVal::Int(2))
        );
        // Not found stays -1; ASCII unchanged.
        assert_eq!(
            method("str", "find", &[s("héllo"), s("z")]),
            Some(ConstVal::Int(-1))
        );
        assert_eq!(
            method("str", "find", &[s("abc"), s("bc")]),
            Some(ConstVal::Int(1))
        );
    }

    #[test]
    fn rfind_returns_code_point_index() {
        // CPython: "héllo".rfind("l") == 3 (byte offset would be 4).
        assert_eq!(
            method("str", "rfind", &[s("héllo"), s("l")]),
            Some(ConstVal::Int(3))
        );
        assert_eq!(
            method("str", "rfind", &[s("héllo"), s("z")]),
            Some(ConstVal::Int(-1))
        );
    }

    #[test]
    fn count_empty_needle_is_code_point_len_plus_one() {
        // CPython: "café".count("") == 5 (byte-len would give 6).
        assert_eq!(
            method("str", "count", &[s("café"), s("")]),
            Some(ConstVal::Int(5))
        );
        assert_eq!(
            method("str", "count", &[s("abc"), s("")]),
            Some(ConstVal::Int(4))
        );
    }

    #[test]
    fn zfill_pads_to_code_point_width() {
        // CPython: "é".zfill(3) == "00é"; "-é".zfill(4) == "-00é".
        assert_eq!(
            method("str", "zfill", &[s("é"), ConstVal::Int(3)]),
            Some(s("00é"))
        );
        assert_eq!(
            method("str", "zfill", &[s("-é"), ConstVal::Int(4)]),
            Some(s("-00é"))
        );
    }

    #[test]
    fn zfill_non_positive_width_returns_unchanged_without_panic() {
        // Negative width previously wrapped `*w as usize` to a huge value and
        // drove "0".repeat(..) into a compile-time OOM/panic. CPython returns
        // the string unchanged.
        assert_eq!(
            method("str", "zfill", &[s("5"), ConstVal::Int(-3)]),
            Some(s("5"))
        );
        assert_eq!(
            method("str", "zfill", &[s("café"), ConstVal::Int(0)]),
            Some(s("café"))
        );
    }
}
