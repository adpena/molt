use super::*;

/// Pre-computed NaN-box tag mask constants materialized at each helper site.
///
/// These values are plain immediates, not Cranelift `Variable`s. Keeping
/// representation constants out of SSA repair prevents label/exception CFG
/// stitching from turning immutable tag facts into block parameters.
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(crate) struct NanBoxConsts {
    /// `(QNAN | TAG_MASK) as i64`
    pub(crate) qnan_tag_mask: i64,
    /// `(QNAN | TAG_INT) as i64`
    pub(crate) qnan_tag_int: i64,
    /// `(QNAN | TAG_PTR) as i64`
    pub(crate) qnan_tag_ptr: i64,
    /// `INT_SHIFT` (17)
    int_shift: i64,
    /// `POINTER_MASK as i64`
    pub(crate) pointer_mask: i64,
    /// `(QNAN | TAG_BOOL) as i64`
    pub(crate) qnan_tag_bool: i64,
    /// `INT_WIDTH as i64` (47)  used in fused_both_int_check
    int_width: i64,
    /// `48i64`  shift to isolate tag field for nanboxed-special / int checks
    shift_48: i64,
    /// `0x7FF9i64`  base of special-tag range
    special_base: i64,
    /// `5i64`  width of special-tag range
    special_limit: i64,
    /// `((QNAN | TAG_INT) >> 48) as i64`  16-bit tag for nanboxed int check
    int_tag_16: i64,
    /// `INT_MASK as i64`  mask for box_int_value
    pub(crate) int_mask: i64,
    /// `16i64`  sign-extension shift for unbox_ptr_value
    shift_16: i64,
    /// `CANONICAL_NAN_BITS as i64`  canonical NaN for box_float_value
    canonical_nan: i64,
}

#[cfg(feature = "native-backend")]
impl NanBoxConsts {
    pub(crate) fn new(_builder: &mut FunctionBuilder) -> Self {
        Self {
            qnan_tag_mask: (QNAN | TAG_MASK) as i64,
            qnan_tag_int: (QNAN | TAG_INT) as i64,
            qnan_tag_ptr: (QNAN | TAG_PTR) as i64,
            int_shift: INT_SHIFT,
            pointer_mask: POINTER_MASK as i64,
            qnan_tag_bool: (QNAN | TAG_BOOL) as i64,
            int_width: INT_WIDTH as i64,
            shift_48: 48,
            special_base: 0x7FF9,
            special_limit: 5,
            int_tag_16: ((QNAN | TAG_INT) >> 48) as i64,
            int_mask: INT_MASK as i64,
            shift_16: 16,
            canonical_nan: CANONICAL_NAN_BITS as i64,
        }
    }
}

pub(crate) fn box_int(val: i64) -> i64 {
    // Use INT_MASK (47 bits) not POINTER_MASK (48 bits) to match the
    // sign-extending unbox path (ishl/sshr by INT_SHIFT=17).
    let masked = (val as u64) & INT_MASK;
    (QNAN | TAG_INT | masked) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        0x7ff0_0000_0000_0001_u64 as i64
    } else {
        val.to_bits() as i64
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn unbox_int(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Debug-mode guard: verify the value actually carries the int tag before
    // unboxing.  In release builds this is a no-op; in debug builds an illegal
    // trap fires immediately if a non-int value reaches this path.
    #[cfg(debug_assertions)]
    {
        let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
        let expected = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
        let masked = builder.ins().band(val, mask);
        let is_int = builder.ins().icmp(IntCC::Equal, masked, expected);
        builder
            .ins()
            .trapz(is_int, cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    // The ishl by INT_SHIFT (17) shifts out the upper 17 tag bits (QNAN+TAG),
    // then sshr sign-extends the 47-bit payload. No separate band with INT_MASK
    // is needed  the shift pair implicitly strips the tag.
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    builder.ins().sshr(shifted, shift)
}

/// Unbox a NaN-boxed value that is either TAG_INT or TAG_BOOL to an i64.
///
/// Booleans are coerced to 0/1 (matching Python's `bool` subclass of `int`).
/// This is needed in `fast_int` arithmetic paths where the TIR optimizer may
/// mark an op as `fast_int` even when one or both operands are booleans.
#[cfg(feature = "native-backend")]
pub(crate) fn unbox_int_or_bool(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let masked = builder.ins().band(val, mask);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);

    let bool_block = builder.create_block();
    let int_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    builder.ins().brif(is_bool, bool_block, &[], int_block, &[]);

    // Bool path: extract bit 0 as the integer value (False=0, True=1).
    builder.switch_to_block(bool_block);
    builder.seal_block(bool_block);
    let one = builder.ins().iconst(types::I64, 1);
    let bool_val = builder.ins().band(val, one);
    jump_block(builder, merge_block, &[bool_val]);

    // Int path: normal unbox_int shift pair.
    builder.switch_to_block(int_block);
    builder.seal_block(int_block);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    let int_val = builder.ins().sshr(shifted, shift);
    jump_block(builder, merge_block, &[int_val]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    builder.block_params(merge_block)[0]
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn is_int_tag(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let masked = builder.ins().band(val, mask);
    builder.ins().icmp(IntCC::Equal, masked, tag)
}

/// Fused tag-check-and-unbox for a single NaN-boxed value.
///
/// XORs the value against the expected int tag pattern `(QNAN | TAG_INT)`.
/// If the value is an int, the XOR zeros out the upper 17 tag bits, leaving
/// only the 47-bit payload.
///
/// Returns `(xored, unboxed)` where:
///   - `xored` can be used for the tag check: `(xored >> 47) == 0` iff the
///     value was a NaN-boxed int.
///   - `unboxed` is the sign-extended 47-bit integer payload (valid only when
///     the tag check passes).
#[cfg(feature = "native-backend")]
pub(crate) fn fused_tag_check_and_unbox_int(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> (Value, Value) {
    let expected_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let xored = builder.ins().bxor(val, expected_tag);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(xored, shift);
    let unboxed = builder.ins().sshr(shifted, shift);
    (xored, unboxed)
}

/// Check that two XOR'd values both represent NaN-boxed ints.
///
/// Takes the `xored` outputs from two `fused_tag_check_and_unbox_int` calls
/// and checks that both had their tag bits zeroed (i.e., both were ints).
/// Uses BOR to combine the two values, then checks that the upper 17 bits
/// of the combined result are zero  true iff both inputs were ints.
#[cfg(feature = "native-backend")]
pub(crate) fn fused_both_int_check(
    builder: &mut FunctionBuilder,
    lhs_xored: Value,
    rhs_xored: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let combined = builder.ins().bor(lhs_xored, rhs_xored);
    let tag_shift = builder.ins().iconst(types::I64, nbc.int_width);
    let upper = builder.ins().ushr(combined, tag_shift);
    builder.ins().icmp_imm(IntCC::Equal, upper, 0)
}

/// Returns true (i8 `1`) iff `val` is an inline NaN-boxed integer (`TAG_INT`) or
/// boolean (`TAG_BOOL`).
///
/// These are exactly the tags for which the trusted shift-unbox `(v << s) >> s`
/// (`unbox_int`) recovers the operand's integer value (`False`0, `True`1).
/// Crucially, this rejects heap pointers (`TAG_PTR`): a Python `int` whose
/// magnitude exceeds the 47-bit inline range is a BigInt carried as a `TAG_PTR`
/// NaN-box, and unboxing it would truncate the pointer to garbage. Callers use
/// this to keep the raw-int fast path correct while still accepting `bool`
/// operands (which are `int`-typed but tagged `TAG_BOOL`).
#[cfg(feature = "native-backend")]
pub(crate) fn fused_is_int_or_bool(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let masked = builder.ins().band(val, mask);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let is_int = builder.ins().icmp(IntCC::Equal, masked, int_tag);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);
    builder.ins().bor(is_int, is_bool)
}

/// Check whether a NaN-boxed value is a special tagged type (int/bool/none/ptr/pending)
/// rather than a plain f64.
///
/// All NaN-boxed specials have bits 62..48 in the range `0x7FF9..=0x7FFD`.
/// Returns true if the value IS a special (i.e., NOT a float).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn is_nanboxed_special(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    // Shift right by 48 to isolate the tag field, then check range [0x7FF9, 0x7FFD].
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    // tag16 - 0x7FF9; result < 5 means it's a tagged special
    let base = builder.ins().iconst(types::I64, nbc.special_base);
    let adjusted = builder.ins().isub(tag16, base);
    let limit = builder.ins().iconst(types::I64, nbc.special_limit);
    builder.ins().icmp(IntCC::UnsignedLessThan, adjusted, limit)
}

/// Check that both NaN-boxed values are plain f64 (not tagged specials).
#[cfg(feature = "native-backend")]
pub(crate) fn both_float_check(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let either_special = builder.ins().bor(lhs_special, rhs_special);
    // both_float = !(lhs_special || rhs_special)
    // Since is_nanboxed_special returns an i8 (0 or 1), we check either_special == 0
    builder.ins().icmp_imm(IntCC::Equal, either_special, 0)
}

/// Check whether a NaN-boxed value carries the int tag.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn is_nanboxed_int(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    let expected = builder.ins().iconst(types::I64, nbc.int_tag_16);
    builder.ins().icmp(IntCC::Equal, tag16, expected)
}

/// Emit inline mixed int+float arithmetic.  When exactly one operand is a
/// NaN-boxed int and the other is a plain f64, convert the int to f64 via
/// `fcvt_from_sint` and perform the requested float operation inline.
///
/// `f_op`: 0 = fadd, 1 = fsub, 2 = fmul.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_mixed_int_float_op(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
    f_op: u8,
    merge_block: Block,
) {
    let lhs_is_int = is_nanboxed_int(builder, lhs, nbc);
    let rhs_is_int = is_nanboxed_int(builder, rhs, nbc);
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let rhs_not_special = builder.ins().icmp_imm(IntCC::Equal, rhs_special, 0);
    let lhs_not_special = builder.ins().icmp_imm(IntCC::Equal, lhs_special, 0);
    let case_a = builder.ins().band(lhs_is_int, rhs_not_special);
    let case_b = builder.ins().band(rhs_is_int, lhs_not_special);
    let lhs_int_block = builder.create_block();
    let check_rhs_block = builder.create_block();
    let rhs_int_block = builder.create_block();
    let not_mixed_block = builder.create_block();
    builder.set_cold_block(not_mixed_block);
    builder
        .ins()
        .brif(case_a, lhs_int_block, &[], check_rhs_block, &[]);
    // LHS is int, RHS is float
    builder.switch_to_block(lhs_int_block);
    builder.seal_block(lhs_int_block);
    let lhs_int_val = unbox_int(builder, lhs, nbc);
    let lhs_conv = builder.ins().fcvt_from_sint(types::F64, lhs_int_val);
    let rhs_flt = builder.ins().bitcast(types::F64, MemFlagsData::new(), rhs);
    let res_a = match f_op {
        0 => builder.ins().fadd(lhs_conv, rhs_flt),
        1 => builder.ins().fsub(lhs_conv, rhs_flt),
        2 => builder.ins().fmul(lhs_conv, rhs_flt),
        _ => unreachable!(),
    };
    let boxed_a = box_float_value(builder, res_a, nbc);
    jump_block(builder, merge_block, &[boxed_a]);
    // Check case_b
    builder.switch_to_block(check_rhs_block);
    builder.seal_block(check_rhs_block);
    builder
        .ins()
        .brif(case_b, rhs_int_block, &[], not_mixed_block, &[]);
    // RHS is int, LHS is float
    builder.switch_to_block(rhs_int_block);
    builder.seal_block(rhs_int_block);
    let rhs_int_val = unbox_int(builder, rhs, nbc);
    let rhs_conv = builder.ins().fcvt_from_sint(types::F64, rhs_int_val);
    let lhs_flt = builder.ins().bitcast(types::F64, MemFlagsData::new(), lhs);
    let res_b = match f_op {
        0 => builder.ins().fadd(lhs_flt, rhs_conv),
        1 => builder.ins().fsub(lhs_flt, rhs_conv),
        2 => builder.ins().fmul(lhs_flt, rhs_conv),
        _ => unreachable!(),
    };
    let boxed_b = box_float_value(builder, res_b, nbc);
    jump_block(builder, merge_block, &[boxed_b]);
    // Not mixed: caller emits slow path
    builder.switch_to_block(not_mixed_block);
    builder.seal_block(not_mixed_block);
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_int_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    builder.ins().bor(tag, masked)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_float_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    // Canonicalize NaN: if the f64 value is NaN, replace with CANONICAL_NAN_BITS
    // to avoid collision with the QNAN tag prefix used by NaN-boxing.
    let raw_bits = builder.ins().bitcast(types::I64, MemFlagsData::new(), val);
    let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
    let canonical = builder.ins().iconst(types::I64, nbc.canonical_nan);
    builder.ins().select(is_nan, canonical, raw_bits)
}

#[cfg(feature = "native-backend")]
pub(crate) fn int_value_fits_inline(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Inline ints are 47-bit signed payloads: range [-(1<<46), (1<<46)-1].
    // Bias the value by +2^46 so the valid range maps to [0, 2^47-1],
    // then do a single unsigned comparison against 2^47.
    // This is a single-comparison range check that Cranelift cannot fold away.
    let bias = builder.ins().iconst(types::I64, 1_i64 << 46);
    let biased = builder.ins().iadd(val, bias);
    let limit = builder.ins().iconst(types::I64, 1_i64 << 47);
    builder.ins().icmp(IntCC::UnsignedLessThan, biased, limit)
}

/// Raw pieces of a signed 64-bit multiply with the `smulhi` overflow witness
/// (Cranelift 0.131 has NO `smul_overflow`, unlike `sadd_overflow`).
///
/// Returns `(product, hi, sign)`:
///   * `product`  the low 64 bits of `lhs * rhs` (the wrapping result),
///   * `hi`       the upper 64 bits of the signed 128-bit product (`smulhi`),
///   * `sign`     the arithmetic sign-extension of `product` (`product >> 63`).
///
/// The signed 64-bit multiply overflows iff `hi != sign`. Both
/// [`imul_overflow64`] and [`imul_checked_inline`] derive their boolean flag
/// from this single source of truth (no duplicated `smulhi` pattern), each
/// forming its own polarity with a direct `icmp` so the result stays a clean
/// `I8` 0/1 (Cranelift folded booleans into `I8`, so a `bnot` would yield
/// `0xFE` and silently corrupt a downstream `band`).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn imul_smulhi_pieces(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value, Value) {
    let prod = builder.ins().imul(lhs, rhs);
    // smulhi gives the upper 64 bits of the signed 128-bit product.
    let hi = builder.ins().smulhi(lhs, rhs);
    // If there was no 64-bit overflow, hi must be the sign-extension of prod,
    // i.e. hi == prod >> 63 (arithmetic).
    let sixty_three = builder.ins().iconst(types::I64, 63);
    let sign = builder.ins().sshr(prod, sixty_three);
    (prod, hi, sign)
}

/// Perform `imul` with hardware-exact 64-bit signed-overflow detection via the
/// `smulhi` pattern.
///
/// Returns `(product, overflow64)` where `product` is the low 64 bits of the
/// signed multiplication and `overflow64` is an `I8` boolean Value that is
/// **true iff the signed product overflowed i64**  i.e. the full 128-bit
/// product does not fit in 64 bits. The flag polarity matches Cranelift's
/// `sadd_overflow` second result (true = overflowed), so the `checked_mul`
/// lowering mirrors the `checked_add` `(sum, of)` shape exactly.
///
/// This is a FULL 64-bit-exact overflow flag, NOT a 47-bit-inline-window test:
/// the overflow-peel accumulator is a genuine full-width i64 carrier, so it
/// must deopt to the boxed BigInt slow loop only at the true 2^63 boundary.
/// (Reusing the `fits_47`-ANDing `imul_checked_inline` here would deopt the
/// accumulator 2^16- too early  a perf bug, not a correctness bug.)
#[cfg(feature = "native-backend")]
pub(crate) fn imul_overflow64(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value) {
    let (prod, hi, sign) = imul_smulhi_pieces(builder, lhs, rhs);
    // Overflow iff the high half differs from the low half's sign-extension.
    let overflow64 = builder.ins().icmp(IntCC::NotEqual, hi, sign);
    (prod, overflow64)
}

/// Perform `imul` with 64-bit overflow detection via `smulhi`.
///
/// Two 47-bit signed values can produce a product exceeding 64 bits (up to ~93
/// bits).  Plain `imul` silently wraps at 64 bits, and the truncated result may
/// happen to pass `int_value_fits_inline` even though it is wrong.
///
/// Returns `(product, fits)` where `product` is the low 64 bits of the
/// multiplication and `fits` is a boolean Value that is true only when:
///   1. The full 128-bit product equals the 64-bit `imul` result (no 64-bit
///      overflow), AND
///   2. The 64-bit result fits in a 47-bit signed inline integer.
///
/// Shares the `smulhi` pattern with [`imul_overflow64`] via
/// [`imul_smulhi_pieces`] (single source of truth); this variant additionally
/// ANDs in the 47-bit inline-window test, which the full-range `checked_mul`
/// carrier must NOT do.
#[cfg(feature = "native-backend")]
pub(crate) fn imul_checked_inline(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value) {
    let (prod, hi, sign) = imul_smulhi_pieces(builder, lhs, rhs);
    // No 64-bit overflow iff hi == prod's sign-extension (direct icmp keeps the
    // result a clean I8 0/1 for the band below).
    let no_overflow_64 = builder.ins().icmp(IntCC::Equal, hi, sign);
    // Also check the result fits in 47-bit signed payload.
    let fits_47 = int_value_fits_inline(builder, prod);
    let both_ok = builder.ins().band(no_overflow_64, fits_47);
    (prod, both_ok)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_bool_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    let bool_val = builder.ins().select(val, one, zero);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    builder.ins().bor(tag, bool_val)
}

#[cfg(feature = "native-backend")]
pub(crate) fn unbox_ptr_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, nbc.shift_16);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_ptr_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    builder.ins().bor(tag, masked)
}

/// Fully inline list_int bounds check  zero FFI calls.
///
/// Extracts the raw heap pointer from the NaN-boxed list value, then
/// dereferences the object layout directly:
///
///   obj_ptr  = unbox_ptr(list_bits)   // past MoltHeader
///   vec_ptr  = *(obj_ptr as *const *const Vec<i64>)   // offset 0
///   data_ptr = *(vec_ptr + 0)         // Vec::ptr  (offset 0)
///   len      = *(vec_ptr + 8)         // Vec::len  (offset 8)
///
/// Returns (data_ptr, in_bounds)  the caller must branch on in_bounds
/// BEFORE loading/storing the element.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(in crate::native_backend::simple_backend) fn emit_list_int_bounds_check(
    builder: &mut FunctionBuilder,
    list_bits: Value,
    index_raw: Value,
    _nbc: &NanBoxConsts,
) -> (Value, Value) {
    // Step 1: extract raw pointer from NaN-boxed value.
    //
    // The NaN-boxed pointer layout is: QNAN | TAG_PTR | (addr & POINTER_MASK).
    // To extract the address: mask off the top 16 bits (QNAN+tag), then
    // sign-extend from bit 47 to reconstruct canonical aarch64 addresses.
    //
    // Use _imm variants to avoid introducing SSA variable dependencies that
    // could interact with Cranelift's block sealing in complex control flow.
    let masked = builder.ins().band_imm(list_bits, POINTER_MASK as i64);
    // Sign-extend from bit 47: shift left 16, arithmetic shift right 16.
    let shifted = builder.ins().ishl_imm(masked, 16);
    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
    // Step 2: load *mut Vec<i64> from offset 0 of the object payload
    let vec_ptr = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
    // Step 3: load data pointer from Vec (offset 0) and length (offset 8)
    let data_ptr = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), vec_ptr, 0);
    let len = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), vec_ptr, 8);
    // Step 4: unsigned compare index < length
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index_raw, len);
    (data_ptr, in_bounds)
}

/// Load element from list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(in crate::native_backend::simple_backend) fn emit_list_int_load(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    let raw_val = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);
    box_int_value(builder, raw_val, nbc)
}

/// Store element into list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(in crate::native_backend::simple_backend) fn emit_list_int_store(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    value_raw: Value,
) {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    builder
        .ins()
        .store(MemFlagsData::trusted(), value_raw, elem_addr, 0);
}
