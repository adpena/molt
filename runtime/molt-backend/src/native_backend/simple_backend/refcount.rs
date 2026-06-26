use super::*;

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn emit_maybe_ref_adjust(
    builder: &mut FunctionBuilder,
    val: Value,
    obj_ref_fn: FuncRef,
) {
    // Keep ref-adjust control flow linear. Hidden branch blocks here can invalidate
    // block-local tracked-value carry if callers do not explicitly propagate tracking.
    // The runtime ref helpers already no-op for non-pointer boxed values.
    let _ = builder.ins().call(obj_ref_fn, &[val]);
}

// ---------------------------------------------------------------------------
// Phase 1: Inline inc_ref_obj as Cranelift IR
//
// Eliminates function-call overhead for the hottest runtime operation (~73
// calls per compiled function). The inlined sequence:
//
//   1. Check if `val` is a heap pointer (NaN-boxed TAG_PTR).
//   2. Extract the raw data pointer from the NaN-box.
//   3. Load the flags field from MoltHeader; skip if IMMORTAL.
//   4. Load the 32-bit refcount, add 1, store back.
//
// Gated by MOLT_INLINE_RC=1 env var so we can A/B test vs call-based RC.
// dec_ref is left as a function call (needs the free/destructor path).
// ---------------------------------------------------------------------------

/// Returns `true` if inline RC codegen is enabled.
///
/// Re-enabled: the inline RC path now uses atomic_rmw (AtomicRmwOp::Add)
/// instead of non-atomic load/iadd/store, which is correct for the
/// AtomicU32 refcount field.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn inline_rc_enabled() -> bool {
    // Disabled: inline RC codegen (even single-branch) causes memory corruption
    // when inc_ref blocks fragment the control flow inside tuple_new. The root
    // cause is Cranelift's handling of SSA values across the brif boundary
    // between the inc_ref blocks and subsequent list_builder_append calls.
    // The function-call path (molt_inc_ref_obj) is both correct and fast
    // enough - it matches Swift's ARC pattern of opaque retain/release calls.
    false
}

/// Emit an inlined `inc_ref_obj` as Cranelift IR.
///
/// Single-branch architecture: only one brif (is_ptr -> inc, else -> merge).
/// The immortal check uses branchless conditional select to compute the
/// increment delta (0 for immortal, 1 for mortal), avoiding the extra
/// block that caused the Cranelift block-fragmentation corruption bug.
///
/// Equivalent to:
/// ```text
/// if (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR):
///     ptr = sign_extend(val & POINTER_MASK)
///     flags = *(ptr - 8) as u32
///     delta = ((flags & IMMORTAL) == 0) ? 1 : 0
///     atomic_add(*(ptr - 12), delta)  // no-op when delta=0
/// ```
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn emit_inline_inc_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) {
    // Single-branch: only split on is_ptr to avoid block fragmentation.
    let inc_block = builder.create_block();
    let merge_block = builder.create_block();

    // 1. Check if val is a heap pointer: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    builder.ins().brif(is_ptr, inc_block, &[], merge_block, &[]);

    // 2. Inc block: extract pointer, check immortal branchlessly, atomic inc
    builder.switch_to_block(inc_block);
    let raw_ptr = unbox_ptr_value(builder, val, nbc);

    // Load flags and compute delta branchlessly:
    // delta = (flags & IMMORTAL) == 0 ? 1 : 0
    let flags = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        raw_ptr,
        HEADER_FLAGS_OFFSET,
    );
    let immortal_mask = builder
        .ins()
        .iconst(types::I32, HEADER_FLAG_IMMORTAL as i64);
    let immortal_bits = builder.ins().band(flags, immortal_mask);
    let zero_i32 = builder.ins().iconst(types::I32, 0);
    let is_mortal = builder.ins().icmp(IntCC::Equal, immortal_bits, zero_i32);
    let one_i32 = builder.ins().iconst(types::I32, 1);
    // Branchless: delta = select(is_mortal, 1, 0)
    let delta = builder.ins().select(is_mortal, one_i32, zero_i32);

    // Atomic add of delta (0 for immortal = no-op, 1 for mortal = inc)
    let rc_offset = builder
        .ins()
        .iconst(types::I64, HEADER_REFCOUNT_OFFSET as i64);
    let rc_addr = builder.ins().iadd(raw_ptr, rc_offset);
    builder.ins().atomic_rmw(
        types::I32,
        MemFlagsData::trusted(),
        AtomicRmwOp::Add,
        rc_addr,
        delta,
    );
    builder.ins().jump(merge_block, &[]);

    // 3. Merge
    builder.switch_to_block(merge_block);
    builder.seal_block(inc_block);
    builder.seal_block(merge_block);
}

/// Emit an inc_ref_obj - either inlined or as a function call depending on
/// the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_inc_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a ref-adjust (inc_ref_obj) - either inlined or as a function call
/// depending on the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_maybe_ref_adjust_v2(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        let _ = builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a dec_ref_obj with an inlined tag check: if the value is not a heap
/// pointer (e.g. NaN-boxed int/float/bool/none), skip the dec_ref call
/// entirely. This eliminates function-call + GIL overhead for the common case
/// where cleanup values are immediate integers.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn emit_dec_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if !inline_rc_enabled() {
        builder.ins().call(call_ref, &[val]);
        return;
    }
    // Inline tag check: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let call_block = builder.create_block();
    let merge_block = builder.create_block();

    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    brif_block(builder, is_ptr, call_block, &[], merge_block, &[]);

    // Only call dec_ref_obj for actual heap pointers.
    builder.switch_to_block(call_block);
    builder.ins().call(call_ref, &[val]);
    jump_block(builder, merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(call_block);
    builder.seal_block(merge_block);
}
