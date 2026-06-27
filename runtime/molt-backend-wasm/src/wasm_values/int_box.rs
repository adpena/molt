use super::constant_cache::ConstantCache;
use crate::wasm_values::{INT_MASK, box_int};
use molt_codegen_abi::{INT_MAX_INLINE, INT_MIN_INLINE, INT_SHIFT, QNAN, TAG_BOOL, TAG_INT};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

/// Trusted unbox: when we *know* the value is a NaN-boxed integer (from IR
/// type information / `fast_int`), we can skip the `AND INT_MASK` step.
/// The left-shift by `INT_SHIFT` (17) already discards the upper QNAN+tag
/// bits, so the mask is redundant.  Saves 2 instructions per operand.
fn emit_unbox_int_local_trusted(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
) {
    func.instruction(&Instruction::LocalGet(src_local));
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64Shl);
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalSet(dst_local));
}

/// Like [`emit_unbox_int_local_trusted`] but uses `local.tee` instead of
/// `local.set`, leaving the unboxed value on the operand stack.  This
/// eliminates a subsequent `local.get` when the caller needs the value
/// immediately after storing it.
fn emit_unbox_int_local_trusted_tee(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
) {
    func.instruction(&Instruction::LocalGet(src_local));
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64Shl);
    if let Some(shift) = cc.int_shift {
        func.instruction(&Instruction::LocalGet(shift));
    } else {
        func.instruction(&Instruction::I64Const(INT_SHIFT));
    }
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalTee(dst_local));
}

// ---------------------------------------------------------------------------
// Peephole optimization: known-value unbox/box elimination
//
// When we know at compile time that a WASM local holds a NaN-boxed integer
// whose raw value is `v`, we can replace the 4-instruction unbox sequence
// with a single `i64.const v`, and the 4-instruction box sequence with a
// single `i64.const box_int(v)`.  This eliminates redundant box/unbox
// round-trips that commonly occur when a `const` op feeds into a `fast_int`
// arithmetic op.
// ---------------------------------------------------------------------------

/// Peephole-optimized unbox: if `src_local` has a known raw int value in
/// `known_raw`, emit `i64.const <raw>` + `local.set dst` (2 instructions)
/// instead of the 5-instruction shift-based unbox.  Returns `true` if the
/// optimization fired.
pub(crate) fn emit_unbox_int_local_trusted_opt(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
    known_raw: &BTreeMap<u32, i64>,
) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(raw));
        func.instruction(&Instruction::LocalSet(dst_local));
    } else {
        emit_unbox_int_local_trusted(func, src_local, dst_local, cc);
    }
}

/// Peephole-optimized unbox with tee: like [`emit_unbox_int_local_trusted_opt`]
/// but leaves the value on the operand stack (`local.tee`).
pub(crate) fn emit_unbox_int_local_trusted_tee_opt(
    func: &mut Function,
    src_local: u32,
    dst_local: u32,
    cc: &ConstantCache,
    known_raw: &BTreeMap<u32, i64>,
) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(raw));
        func.instruction(&Instruction::LocalTee(dst_local));
    } else {
        emit_unbox_int_local_trusted_tee(func, src_local, dst_local, cc);
    }
}

/// Peephole-optimized box: if `src_local` has a known raw int value in
/// `known_raw`, emit `i64.const <boxed>` (1 instruction) instead of the
/// 4-instruction mask+or boxing sequence.
pub(crate) fn emit_box_int_from_local_opt(
    func: &mut Function,
    src_local: u32,
    known_raw: &BTreeMap<u32, i64>,
) {
    if let Some(&raw) = known_raw.get(&src_local) {
        func.instruction(&Instruction::I64Const(box_int(raw)));
    } else {
        emit_box_int_from_local(func, src_local);
    }
}

fn emit_box_int_from_local(func: &mut Function, src_local: u32) {
    func.instruction(&Instruction::LocalGet(src_local));
    func.instruction(&Instruction::I64Const(INT_MASK as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const((QNAN | TAG_INT) as i64));
    func.instruction(&Instruction::I64Or);
}

pub(crate) fn emit_inline_int_range_check(func: &mut Function, val_local: u32, cc: &ConstantCache) {
    func.instruction(&Instruction::LocalGet(val_local));
    if let Some(min_local) = cc.int_min {
        func.instruction(&Instruction::LocalGet(min_local));
    } else {
        func.instruction(&Instruction::I64Const(INT_MIN_INLINE));
    }
    func.instruction(&Instruction::I64GeS);
    func.instruction(&Instruction::LocalGet(val_local));
    if let Some(max_local) = cc.int_max {
        func.instruction(&Instruction::LocalGet(max_local));
    } else {
        func.instruction(&Instruction::I64Const(INT_MAX_INLINE));
    }
    func.instruction(&Instruction::I64LeS);
    func.instruction(&Instruction::I32And);
}

pub(crate) fn emit_box_bool_from_i32(func: &mut Function) {
    func.instruction(&Instruction::I64ExtendI32U);
    func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
    func.instruction(&Instruction::I64Or);
}
