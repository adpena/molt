use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ValueRangeConstFoldRule, ValueRangeContainerLengthRule,
    opcode_value_range_const_fold_rule_table, opcode_value_range_container_length_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode};

use super::ValueRangeResult;
use super::result::KnownLength;

/// Collect `ConstInt` values, container lengths, and `len(c)` symbols.
pub(super) fn collect_constants_and_lengths(func: &TirFunction, result: &mut ValueRangeResult) {
    // First pass: literal constants.
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &r in &op.results {
                    result.const_int.insert(r, *v);
                }
            }
        }
    }
    // Constant-fold integer `Add`/`Sub`/`Mul`/`Shl`/`Shr`/bitwise of known
    // constants to a fixpoint, so derived lengths like `n + 1` (the
    // `[True] * (n + 1)` sieve shape) AND constant bit-masks like `(1 << 32) - 1`
    // resolve to numeric bounds. The mask case is load-bearing for the masked
    // back-edge accumulator (`s = (s << 1) & MASK`): `bit_and`'s constant-mask
    // rule (`a & m, m >= 0 ⇒ [0, m]` for ANY `a`) requires `MASK` to be a known
    // constant — without folding `(1 << k) - 1`, the mask stays a non-constant
    // and `bit_and` falls back to the both-non-negative rule, which fails on the
    // FULL (negative-`lo`) shift result, leaving the masked value FULL and the
    // accumulator off the raw lane. All arithmetic is CHECKED in i64 — an
    // overflow (`1 << 70` would exceed i64) drops the value (left unknown,
    // correctly forcing the boxed BigInt path), never wraps. A negative shift
    // count yields no fold (it is a runtime `ValueError`, no static value).
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                let const_fold_rule = opcode_value_range_const_fold_rule_table(op.opcode);
                if const_fold_rule == ValueRangeConstFoldRule::None || op.operands.len() != 2 {
                    continue;
                }
                let Some(&a) = result.const_int.get(&result.resolve(op.operands[0])) else {
                    continue;
                };
                let Some(&b) = result.const_int.get(&result.resolve(op.operands[1])) else {
                    continue;
                };
                let folded = match const_fold_rule {
                    ValueRangeConstFoldRule::Add => a.checked_add(b),
                    ValueRangeConstFoldRule::Sub => a.checked_sub(b),
                    ValueRangeConstFoldRule::Mul => a.checked_mul(b),
                    // `a << b`: only fold a non-negative, in-i64-range count whose
                    // result fits i64 (checked). A count `>= 64`, `< 0`, or an
                    // overflowing result yields no constant (boxed BigInt path).
                    ValueRangeConstFoldRule::Shl => {
                        if (0..64).contains(&b) {
                            a.checked_shl(b as u32).filter(|&v| (v >> b) == a)
                        } else {
                            None
                        }
                    }
                    // `a >> b`: arithmetic floor shift; only a non-negative,
                    // in-range count. `a >> b` never overflows i64.
                    ValueRangeConstFoldRule::Shr => {
                        if (0..64).contains(&b) {
                            Some(a >> b)
                        } else {
                            None
                        }
                    }
                    ValueRangeConstFoldRule::BitAnd => Some(a & b),
                    ValueRangeConstFoldRule::BitOr => Some(a | b),
                    ValueRangeConstFoldRule::BitXor => Some(a ^ b),
                    ValueRangeConstFoldRule::None => None,
                };
                if let Some(v) = folded {
                    for &r in &op.results {
                        if result.const_int.insert(r, v).is_none() {
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    // Second pass: container lengths (depends on constants for list-repeat).
    for block in func.blocks.values() {
        for op in &block.ops {
            match opcode_value_range_container_length_rule_table(op.opcode) {
                ValueRangeContainerLengthRule::FixedLiteral => {
                    let len = op.operands.len() as i64;
                    for &r in &op.results {
                        result
                            .container_length
                            .insert(r, KnownLength::Constant(len));
                    }
                }
                ValueRangeContainerLengthRule::ListRepeat => {
                    if op.operands.len() == 2 {
                        // list-repeat: Mul(list_of_1, count) → length == count.
                        // Resolve operands through copies to reach the BuildList /
                        // const sources.
                        let (a, b) = (
                            result.resolve(op.operands[0]),
                            result.resolve(op.operands[1]),
                        );
                        let count = if result
                            .container_length
                            .get(&a)
                            .is_some_and(|l| matches!(l, KnownLength::Constant(1)))
                        {
                            Some(b)
                        } else if result
                            .container_length
                            .get(&b)
                            .is_some_and(|l| matches!(l, KnownLength::Constant(1)))
                        {
                            Some(a)
                        } else {
                            None
                        };
                        if let Some(count_val) = count {
                            for &r in &op.results {
                                if let Some(&c) = result.const_int.get(&count_val) {
                                    result.container_length.insert(r, KnownLength::Constant(c));
                                } else {
                                    result
                                        .container_length
                                        .insert(r, KnownLength::SameAs(count_val));
                                }
                            }
                        }
                    }
                }
                ValueRangeContainerLengthRule::LenCall => {
                    let name = op
                        .attrs
                        .get("name")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    if name == "len" && op.operands.len() == 1 {
                        let container = result.resolve(op.operands[0]);
                        for &r in &op.results {
                            result.len_of.insert(r, container);
                        }
                    }
                }
                ValueRangeContainerLengthRule::None => {}
            }
        }
    }
}
