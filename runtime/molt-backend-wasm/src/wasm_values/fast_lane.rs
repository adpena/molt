use crate::wasm_binary::emit_call;
use molt_codegen_abi::{QNAN, QNAN_TAG_MASK_I64, TAG_BOOL, TAG_INT};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

/// Which NaN-box tags an integer scalar fast path can correctly consume on its
/// raw (unboxed) lane. This is dictated by what the fast body *does* with the
/// operand bits, not by the operand's Python type:
///
/// - [`IntFastLane::IntOrBool`] â€” the body shift-unboxes the payload
///   (`emit_unbox_int_local_trusted`), which is value-exact for both `TAG_INT`
///   and `TAG_BOOL` (`True`â†’1, `False`â†’0). Used by every arithmetic / bitwise /
///   shift op.
/// - [`IntFastLane::IntOnly`] â€” the body compares the *boxed* representations
///   directly (`==`/`!=` via `i64.eq`), which is value-correct only when both
///   operands share the canonical inline-int encoding. A `bool` has a distinct
///   tag, so `True == 1` would wrongly compare unequal on the raw lane; `bool`
///   operands must fall to the runtime helper. Used by `eq` / `ne`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntFastLane {
    IntOrBool,
    IntOnly,
}

/// Push an `i32` boolean that is `1` iff `val_local` holds a NaN-boxed value the
/// integer scalar fast path may consume on its raw lane for `lane` (see
/// [`IntFastLane`]).
///
/// This deliberately rejects heap pointers (`TAG_PTR`), which is the load-bearing
/// correctness case. The integer scalar fast path classifies operands by their
/// Python *type* (`int`), and a Python `int` whose magnitude exceeds the 47-bit
/// inline range is a heap-allocated BigInt carried as a `TAG_PTR` NaN-box â€” not
/// an inline int. The trusted unbox would `(<<17)>>17`-truncate that pointer's
/// low bits (and the boxed-identity compare would test pointer identity instead
/// of value), yielding wrong results. Guarding on this predicate routes BigInt
/// (and float / None / pending) operands to the boxed runtime helper.
fn emit_is_trusted_inline_int_i32(func: &mut Function, val_local: u32, lane: IntFastLane) {
    func.instruction(&Instruction::LocalGet(val_local));
    func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const((QNAN | TAG_INT) as i64));
    func.instruction(&Instruction::I64Eq);
    if lane == IntFastLane::IntOrBool {
        func.instruction(&Instruction::LocalGet(val_local));
        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
        func.instruction(&Instruction::I64And);
        func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
        func.instruction(&Instruction::I64Eq);
        func.instruction(&Instruction::I32Or);
    }
}

/// Open a runtime tag guard for an integer scalar fast path.
///
/// For each operand local that is *not* already a compile-time-proven inline int
/// (i.e. not in `known_raw_ints`), this pushes the
/// [`emit_is_trusted_inline_int_i32`] predicate for `lane` and `AND`s them
/// together, then opens an `If(Result(I64))`. The caller emits the existing
/// trusted raw fast body as the `If` arm, then must call
/// [`emit_trusted_int_fast_path_guard_close`] to emit the `Else` (boxed runtime
/// fallback) and `End`.
///
/// Returns `true` when a guard (and `If`) was emitted. Returns `false` when every
/// operand is compile-time-proven inline (no `TAG_PTR` is possible), in which case
/// no guard is emitted and the caller emits the raw fast body unwrapped â€” keeping
/// the constant-folded fast path allocation-free and branch-free.
#[must_use]
pub(crate) fn emit_trusted_int_fast_path_guard_open(
    func: &mut Function,
    operands: &[u32],
    known_raw_ints: &BTreeMap<u32, i64>,
    lane: IntFastLane,
) -> bool {
    let mut emitted = 0usize;
    for &val in operands {
        if known_raw_ints.contains_key(&val) {
            continue;
        }
        emit_is_trusted_inline_int_i32(func, val, lane);
        if emitted > 0 {
            func.instruction(&Instruction::I32And);
        }
        emitted += 1;
    }
    if emitted == 0 {
        return false;
    }
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    true
}

/// Close a guard opened by [`emit_trusted_int_fast_path_guard_open`].
///
/// Emits the `Else` arm â€” the boxed runtime call that correctly handles BigInt /
/// float / mixed operands â€” followed by `End`. The runtime helper receives the
/// original NaN-boxed operand locals (in order) and leaves one `I64` result on
/// the stack, matching the `If` arm's result and the surrounding op's contract.
pub(crate) fn emit_trusted_int_fast_path_guard_close(
    func: &mut Function,
    reloc_enabled: bool,
    operands: &[u32],
    runtime_import: u32,
) {
    func.instruction(&Instruction::Else);
    for &val in operands {
        func.instruction(&Instruction::LocalGet(val));
    }
    emit_call(func, reloc_enabled, runtime_import);
    func.instruction(&Instruction::End);
}
