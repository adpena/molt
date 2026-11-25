use crate::tir::blocks::BlockId;
use crate::tir::numeric_facts::{INLINE_INT47_HI, INLINE_INT47_LO, IntRange};
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
pub(in crate::tir::passes) fn transfer_op_range(
    op: &TirOp,
    block: BlockId,
    result: &ValueRangeResult,
) -> Option<IntRange> {
    // Operand range / constant helpers (resolve through plain copies).
    let r = |i: usize| -> IntRange {
        op.operands
            .get(i)
            .map(|&v| result.range_at(block, v))
            .unwrap_or(IntRange::FULL_I64)
    };
    let c = |i: usize| -> Option<i64> { op.operands.get(i).and_then(|&v| result.const_int_of(v)) };
    // At a saturated endpoint the actual Python magnitude can exceed i64.
    // Only a literal can prove that an extreme denotes precisely MIN or MAX.
    let finite = |i| {
        let range = r(i);
        if range.is_bounded() {
            Some(range)
        } else {
            c(i).map(IntRange::point)
        }
    };
    let finite_binary = |transfer: fn(IntRange, IntRange) -> IntRange| {
        finite(0)
            .zip(finite(1))
            .map_or(IntRange::FULL_I64, |(a, b)| transfer(a, b))
    };
    match opcode_value_range_transfer_rule_table(op.opcode) {
        // ModuleRegistry admission bounds the dense row count to u32. The
        // actual final-application ID remains unknown to reusable functions.
        ValueRangeTransferRule::ModuleSlot if op.operands.is_empty() => {
            Some(IntRange::new(0, i64::from(u32::MAX)))
        }
        ValueRangeTransferRule::Add if op.operands.len() == 2 => Some(finite_binary(IntRange::add)),
        ValueRangeTransferRule::Sub if op.operands.len() == 2 => Some(finite_binary(IntRange::sub)),
        ValueRangeTransferRule::Mul if op.operands.len() == 2 => Some(finite_binary(IntRange::mul)),
        ValueRangeTransferRule::Neg if op.operands.len() == 1 => {
            Some(finite(0).map_or(IntRange::FULL_I64, IntRange::neg))
        }
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
                Some(finite(1).map_or(IntRange::FULL_I64, IntRange::mod_range))
            }
        }
        ValueRangeTransferRule::FloorDiv if op.operands.len() == 2 => {
            // Constant divisor (the common `i // 3` loop-IV case) takes Python
            // sign-of-divisor floor semantics; else a sign-uniform, non-zero
            // divisor range. `//` rounds toward -inf, so the dividend's whole
            // range — not just its magnitude — drives the bound.
            let Some(dividend) = finite(0) else {
                return Some(IntRange::FULL_I64);
            };
            if let Some(cd) = c(1) {
                if cd == 0 {
                    Some(IntRange::FULL_I64) // ZeroDivisionError — no value.
                } else {
                    Some(dividend.floordiv_const(cd))
                }
            } else {
                Some(finite(1).map_or(IntRange::FULL_I64, |divisor| {
                    dividend.floordiv_range(divisor)
                }))
            }
        }
        ValueRangeTransferRule::Shr if op.operands.len() == 2 => {
            let Some(value) = finite(0) else {
                return Some(IntRange::FULL_I64);
            };
            let count = c(1).map_or_else(|| r(1), IntRange::point);
            Some(value.shr_range(count))
        }
        ValueRangeTransferRule::Shl if op.operands.len() == 2 => match c(1) {
            Some(s) => Some(r(0).shl_const(s)),
            None => Some(IntRange::FULL_I64),
        },
        // A sign-extended 47-bit inline-int payload (HF-821's fast arm).
        ValueRangeTransferRule::InlineIntPayload if op.operands.len() == 1 => {
            Some(IntRange::new(INLINE_INT47_LO, INLINE_INT47_HI))
        }
        _ => None,
    }
}
