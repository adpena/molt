use crate::tir::numeric_facts::IntRange;
use crate::tir::op_kinds_generated::{
    ValueRangeTransferRule, opcode_value_range_transfer_rule_table,
};
use crate::tir::ops::TirOp;

use super::ValueRangeResult;
/// The transfer function for one op: the range of its result computed from the
/// (already-proven) ranges of its operands. `None` when the opcode is not a
/// modeled integer operation; `Some(FULL_I64)` when modeled but unprovable.
///
/// Every rule here is sound over the **full i64 domain including negatives**.
/// A false (too-tight) range feeds `fits_inline_int47` → `RawI64Safe` promotion,
/// so an unsound bound is a silent BigInt-truncation miscompile. When in doubt,
/// return `FULL_I64`.
pub(super) fn transfer_op_range(op: &TirOp, result: &ValueRangeResult) -> Option<IntRange> {
    // Operand range / constant helpers (resolve through plain copies).
    let r = |i: usize| -> IntRange {
        op.operands
            .get(i)
            .map(|&v| result.range_of(v))
            .unwrap_or(IntRange::FULL_I64)
    };
    let c = |i: usize| -> Option<i64> { op.operands.get(i).and_then(|&v| result.const_int_of(v)) };
    match opcode_value_range_transfer_rule_table(op.opcode) {
        ValueRangeTransferRule::Add if op.operands.len() == 2 => Some(r(0).add(r(1))),
        ValueRangeTransferRule::Sub if op.operands.len() == 2 => Some(r(0).sub(r(1))),
        ValueRangeTransferRule::Mul if op.operands.len() == 2 => {
            let (a, b) = (r(0), r(1));
            // A FULL operand makes the product FULL; guard to avoid i64::MIN ·
            // huge corner-product noise (still sound, just no information).
            if a.is_full() || b.is_full() {
                Some(IntRange::FULL_I64)
            } else {
                Some(a.mul(b))
            }
        }
        ValueRangeTransferRule::Neg if op.operands.len() == 1 => Some(r(0).neg()),
        ValueRangeTransferRule::BitAnd if op.operands.len() == 2 => {
            Some(r(0).bit_and(r(1), c(0), c(1)))
        }
        ValueRangeTransferRule::BitOr if op.operands.len() == 2 => {
            Some(r(0).bit_or_xor(r(1), true))
        }
        ValueRangeTransferRule::BitXor if op.operands.len() == 2 => {
            Some(r(0).bit_or_xor(r(1), false))
        }
        ValueRangeTransferRule::Mod if op.operands.len() == 2 => {
            // Constant divisor (Python sign-of-divisor semantics); else a
            // sign-uniform, non-zero divisor range.
            if let Some(cd) = c(1) {
                if cd == 0 {
                    Some(IntRange::FULL_I64) // ZeroDivisionError — no value.
                } else {
                    Some(IntRange::mod_const(cd))
                }
            } else {
                Some(IntRange::mod_range(r(1)))
            }
        }
        ValueRangeTransferRule::FloorDiv if op.operands.len() == 2 => {
            // Constant divisor (the common `i // 3` loop-IV case) takes Python
            // sign-of-divisor floor semantics; else a sign-uniform, non-zero
            // divisor range. `//` rounds toward -inf, so the dividend's whole
            // range — not just its magnitude — drives the bound.
            if let Some(cd) = c(1) {
                if cd == 0 {
                    Some(IntRange::FULL_I64) // ZeroDivisionError — no value.
                } else {
                    Some(r(0).floordiv_const(cd))
                }
            } else {
                Some(r(0).floordiv_range(r(1)))
            }
        }
        ValueRangeTransferRule::Shr if op.operands.len() == 2 => match c(1) {
            Some(s) => Some(r(0).shr_const(s)),
            None => Some(IntRange::FULL_I64),
        },
        ValueRangeTransferRule::Shl if op.operands.len() == 2 => match c(1) {
            Some(s) => Some(r(0).shl_const(s)),
            None => Some(IntRange::FULL_I64),
        },
        _ => None,
    }
}
