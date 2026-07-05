use super::super::ConstVal;
use crate::tir::numeric_facts::{python_range_is_non_empty, python_range_len};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::effects;

/// Try to concrete-eval a `CallBuiltin` op when all operands are constant
/// and the callee is a known pure builtin.
pub(in crate::tir::passes::sccp) fn evaluate_builtin_call(
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

/// CPython `repr(str)` for the subset whose single-quoted rendering needs no
/// quote selection or escaping. All other strings stay with the runtime.
fn fold_repr_str(s: &str) -> Option<ConstVal> {
    if s.bytes().all(|b| (0x20..=0x7e).contains(&b)) && !s.contains('\'') && !s.contains('\\') {
        Some(ConstVal::Str(format!("'{}'", s)))
    } else {
        None
    }
}

/// Concrete evaluation of known pure builtins.
pub(super) fn eval_concrete_builtin(
    name: &str,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    match name {
        "len" => {
            let a = operands.first().copied().flatten()?;
            match a {
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
                // Rust's f64 formatter is not byte-for-byte CPython.
                ConstVal::Float(_) => None,
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
                ConstVal::Float(_) => None,
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
