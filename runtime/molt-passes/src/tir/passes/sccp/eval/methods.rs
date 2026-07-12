use super::super::ConstVal;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::effects;

/// Translate a UTF-8 byte offset into a Python code-point index.
fn byte_offset_to_char_index(s: &str, byte_off: usize) -> i64 {
    s[..byte_off].chars().count() as i64
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
pub(in crate::tir::passes::sccp) fn evaluate_method_call(
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

/// Concrete evaluation of known pure methods on constant receivers.
pub(super) fn eval_concrete_method(
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
