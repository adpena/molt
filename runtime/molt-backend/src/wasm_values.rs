use crate::wasm_binary::emit_call;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
pub(crate) const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const QNAN_TAG_MASK_I64: i64 = (QNAN | TAG_MASK) as i64;
const QNAN_TAG_PTR_I64: i64 = (QNAN | TAG_PTR) as i64;
pub(crate) const INT_MASK: u64 = (1 << 47) - 1;
const INT_SHIFT: i64 = 17;
const INT_MIN_INLINE: i64 = -(1 << 46);
const INT_MAX_INLINE: i64 = (1 << 46) - 1;

pub(crate) fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

pub(crate) fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        CANONICAL_NAN_BITS as i64
    } else {
        val.to_bits() as i64
    }
}

pub(crate) fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

pub(crate) fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

pub(crate) fn box_pending() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

/// Emit WASM instructions to convert an f64 on the stack to a NaN-canonicalized i64.
/// Uses `scratch_local` (an i64 local) as temporary storage.
/// Expects: stack = [..., f64_val]
/// Produces: stack = [..., i64_boxed] where NaN is replaced with CANONICAL_NAN_BITS.
pub(crate) fn emit_f64_to_i64_canonical(func: &mut wasm_encoder::Function, scratch_local: u32) {
    // Reinterpret f64 to i64 raw bits, save in scratch
    func.instruction(&Instruction::I64ReinterpretF64);
    func.instruction(&Instruction::LocalTee(scratch_local));
    // Check if raw bits have QNAN prefix: (raw & QNAN) == QNAN
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64Eq);
    // select(canonical, raw, is_nan) — if is_nan is true (nonzero), picks canonical
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::I64Const(CANONICAL_NAN_BITS as i64));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(scratch_local));
    func.instruction(&Instruction::End);
}

pub(crate) fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for b in func_name
        .as_bytes()
        .iter()
        .chain(lane.as_bytes().iter())
        .copied()
    {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= op_idx as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

/// Cache of WASM local indices holding frequently-used i64 constants.
/// When a function body contains 3+ fast_int operations, these locals are
/// pre-allocated and initialized once at function entry, replacing repeated
/// `i64.const` immediates with cheaper `local.get` instructions.
#[derive(Clone, Copy, Default)]
pub(crate) struct ConstantCache {
    pub(crate) int_shift: Option<u32>,
    pub(crate) int_min: Option<u32>,
    pub(crate) int_max: Option<u32>,
    pub(crate) none_bits: Option<u32>,
    pub(crate) qnan_tag_mask: Option<u32>,
    pub(crate) qnan_tag_ptr: Option<u32>,
}

impl ConstantCache {
    /// Emit the initialization sequence for all cached constants.
    /// Must be called once, right after the WASM `Function` is created and
    /// before any op emission.
    pub(crate) fn emit_init(&self, func: &mut Function) {
        if let Some(local) = self.int_shift {
            func.instruction(&Instruction::I64Const(INT_SHIFT));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_min {
            func.instruction(&Instruction::I64Const(INT_MIN_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_max {
            func.instruction(&Instruction::I64Const(INT_MAX_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
    }

    /// Emit `box_none()` — uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_none(&self, func: &mut Function) {
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(box_none()));
        }
    }

    /// Emit `QNAN_TAG_MASK_I64` — uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_qnan_tag_mask(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
        }
    }

    /// Emit `QNAN_TAG_PTR_I64` — uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_qnan_tag_ptr(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
        }
    }
}

/// Trusted unbox: when we *know* the value is a NaN-boxed integer (from IR
/// type information / `fast_int`), we can skip the `AND INT_MASK` step.
/// The left-shift by `INT_SHIFT` (17) already discards the upper QNAN+tag
/// bits, so the mask is redundant.  Saves 2 instructions per operand.
pub(crate) fn emit_unbox_int_local_trusted(
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
pub(crate) fn emit_unbox_int_local_trusted_tee(
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

pub(crate) fn emit_box_int_from_local(func: &mut Function, src_local: u32) {
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

/// Push an `i32` boolean (`1` = truthy, `0` = falsy) for `cond_local` to be
/// consumed by a control-flow branch (`br_if` / `if` / `loop_break_if_*`).
///
/// For a NaN-boxed **bool** this reads bit 0 directly; for everything else it
/// falls back to the runtime `molt_is_truthy`.  This mirrors the native
/// backend's `br_if` truthiness dispatch (which checks the bool tag and reads
/// bit 0 inline) and is the load-bearing correctness fix for the exception
/// break:
///
/// `molt_is_truthy` returns **false** whenever an exception is pending
/// (CPython truthiness can never be evaluated with an exception in flight).
/// The iterator-consumer exception break is gated on
/// `box_bool(molt_exception_pending())`; routing that boxed bool through
/// `is_truthy` while the very exception it checks is pending would make the
/// break unconditionally not-taken — the loop would spin forever (OOM).
/// Reading bit 0 of a boxed bool is exception-independent and value-exact
/// (`True`→1, `False`→0), so the break fires correctly.  For non-bool
/// conditions the behaviour is unchanged (the runtime helper is still called).
pub(crate) fn emit_branch_truthiness_i32(
    func: &mut Function,
    cond_local: u32,
    is_truthy_import: u32,
    reloc_enabled: bool,
) {
    // is_boxed_bool = (cond & QNAN_TAG_MASK) == (QNAN | TAG_BOOL)
    func.instruction(&Instruction::LocalGet(cond_local));
    func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    // Boxed bool: truthiness is bit 0 (no GIL/exception dependence).
    func.instruction(&Instruction::LocalGet(cond_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I32Const(1));
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::Else);
    // Non-bool: defer to the runtime truthiness helper (`!= 0`).
    func.instruction(&Instruction::LocalGet(cond_local));
    emit_call(func, reloc_enabled, is_truthy_import);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::End);
}

/// Which NaN-box tags an integer scalar fast path can correctly consume on its
/// raw (unboxed) lane. This is dictated by what the fast body *does* with the
/// operand bits, not by the operand's Python type:
///
/// - [`IntFastLane::IntOrBool`] — the body shift-unboxes the payload
///   (`emit_unbox_int_local_trusted`), which is value-exact for both `TAG_INT`
///   and `TAG_BOOL` (`True`→1, `False`→0). Used by every arithmetic / bitwise /
///   shift op.
/// - [`IntFastLane::IntOnly`] — the body compares the *boxed* representations
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
/// inline range is a heap-allocated BigInt carried as a `TAG_PTR` NaN-box — not
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
/// no guard is emitted and the caller emits the raw fast body unwrapped — keeping
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
/// Emits the `Else` arm — the boxed runtime call that correctly handles BigInt /
/// float / mixed operands — followed by `End`. The runtime helper receives the
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
