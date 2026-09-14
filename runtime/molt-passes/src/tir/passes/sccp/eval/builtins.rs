use super::super::{ConstVal, MAX_COMPOUND_ELEMENTS, admits_constant_result};
use crate::tir::numeric_facts::python_range_len;
use crate::tir::ops::{BuiltinCallTarget, OpCode, builtin_call_view};

/// Try to concrete-eval a `CallBuiltin` op when all operands are constant
/// and the concrete typed evaluator admits its complete argument form.
pub(in crate::tir::passes::sccp) fn evaluate_builtin_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallBuiltin
        || !admits_constant_result(op)
        || operands.len() != op.operands.len()
    {
        return None;
    }
    let call = builtin_call_view(op.opcode, &op.attrs, operands)?;
    let name = if call.wire_kind == "range_new" {
        "range_new"
    } else {
        let name = match call.target {
            BuiltinCallTarget::Named(name) => name,
            BuiltinCallTarget::Dynamic(Some(ConstVal::Str(name))) => name.as_str(),
            _ => return None,
        };
        // A generic lookup does not acquire the dedicated primitive's identity.
        if name == "range_new" {
            return None;
        }
        name
    };
    eval_concrete_builtin(name, call.arguments)
}

/// CPython `repr(str)` for the subset whose single-quoted rendering needs no
/// quote selection or escaping. All other strings stay with the runtime.
fn fold_repr_str(s: &str) -> Option<ConstVal> {
    if s.len().checked_add(2)? <= MAX_COMPOUND_ELEMENTS
        && s.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !s.contains('\'')
        && !s.contains('\\')
    {
        Some(ConstVal::Str(format!("'{}'", s)))
    } else {
        None
    }
}

/// Concrete evaluation of implemented pure builtin argument forms.
/// Arm guards describe the forms evaluated below, not the builtin's entire
/// Python signature. Unimplemented optional/variadic arguments must reach the
/// runtime unchanged; silently ignoring them changes values or hides TypeError.
pub(super) fn eval_concrete_builtin(
    name: &str,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    for operand in operands {
        (*operand)?.materialization_cost()?;
    }
    match name {
        "len" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Str(s) => Some(ConstVal::Int(s.chars().count() as i64)),
                ConstVal::Tuple(elems) => Some(ConstVal::Int(elems.len() as i64)),
                ConstVal::Range { start, stop, step } => {
                    // Python: len(range(start, stop, step))
                    python_range_len(*start, *stop, *step).map(ConstVal::Int)
                }
                _ => None,
            }
        }
        "abs" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_abs().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(v.abs())),
                _ => None,
            }
        }
        "repr" if operands.len() == 1 => {
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
        "chr" if operands.len() == 1 => {
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
        "ord" if operands.len() == 1 => {
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
        "hex" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0x{:x}", v.unsigned_abs())
                } else {
                    format!("0x{:x}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "oct" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0o{:o}", v.unsigned_abs())
                } else {
                    format!("0o{:o}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "bin" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0b{:b}", v.unsigned_abs())
                } else {
                    format!("0b{:b}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        // Only the dedicated frontend primitive bypasses mutable builtin
        // lookup. A generic call named "range" is not this constructor.
        "range_new" if operands.len() == 3 => {
            let (ConstVal::Int(start), ConstVal::Int(stop), ConstVal::Int(step)) =
                (operands[0]?, operands[1]?, operands[2]?)
            else {
                return None;
            };
            (*step != 0).then_some(ConstVal::Range {
                start: *start,
                stop: *stop,
                step: *step,
            })
        }
        "sum" if operands.len() == 1 => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Tuple(elems) => {
                    // sum((int, int, ...)) → int
                    let mut total: i64 = 0;
                    for elem in elems.iter() {
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
        "min" if operands.len() == 2 => {
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::min(*x, *y))),
                // Python retains the first operand unless the next compares
                // strictly smaller, including unordered NaN and signed zero.
                (ConstVal::Float(x), ConstVal::Float(y)) => {
                    Some(ConstVal::Float(if y < x { *y } else { *x }))
                }
                _ => None,
            }
        }
        "max" if operands.len() == 2 => {
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::max(*x, *y))),
                (ConstVal::Float(x), ConstVal::Float(y)) => {
                    Some(ConstVal::Float(if y > x { *y } else { *x }))
                }
                _ => None,
            }
        }
        // Dotted module names do not identify runtime intrinsics. Constructor
        // names such as int/str/bool/float/range resolve through the mutable
        // builtins module and do not establish callable identity either.
        _ => None,
    }
}
