#[cfg(feature = "native-backend")]
use cranelift_codegen::Context;
#[cfg(feature = "native-backend")]
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
#[cfg(feature = "native-backend")]
use cranelift_codegen::ir::{
    AbiParam, AtomicRmwOp, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlags,
    StackSlotData, StackSlotKind, Value, types,
};
#[cfg(feature = "native-backend")]
use cranelift_codegen::isa;
#[cfg(feature = "native-backend")]
use cranelift_codegen::settings;
#[cfg(feature = "native-backend")]
use cranelift_codegen::settings::Configurable;
#[cfg(feature = "native-backend")]
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
#[cfg(feature = "native-backend")]
use cranelift_module::{DataDescription, Linkage, Module};
#[cfg(feature = "native-backend")]
use cranelift_native::builder_with_options as native_isa_builder_with_options;
#[cfg(feature = "native-backend")]
use cranelift_object::{ObjectBuilder, ObjectModule};
#[cfg(feature = "native-backend")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "native-backend")]
use std::collections::HashSet;
#[cfg(feature = "native-backend")]
use std::fmt::Write as _;
#[cfg(feature = "native-backend")]
use std::sync::OnceLock;

mod ir;
mod ir_schema;
mod json_boundary;
pub mod tir;
pub mod luau_ir;
pub mod luau_lower;
#[cfg(feature = "llvm")]
pub mod llvm_backend;
#[cfg(feature = "native-backend")]
mod native_backend;
mod passes;
pub use crate::ir::{
    FunctionIR, OpIR, PgoProfileIR, SimpleIR,
    validate_simple_ir,
};
#[cfg(feature = "native-backend")]
use crate::native_backend::TrampolineKey;
pub use crate::passes::{
    apply_profile_order, build_const_int_map, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, escape_analysis,
    fold_constants, fold_constants_cross_block, hoist_loop_invariants,
    inline_functions, propagate_loop_fast_int, rc_coalescing,
    split_megafunctions,
};

#[cfg(feature = "luau-backend")]
pub mod luau;
#[cfg(feature = "rust-backend")]
pub mod rust;
#[cfg(feature = "wasm-backend")]
mod wasm_imports;
#[cfg(feature = "wasm-backend")]
pub mod wasm;

#[cfg(feature = "egraphs")]
pub mod egraph_simplify;

#[cfg(feature = "native-backend")]
mod native_backend_consts {
    pub(super) const QNAN: u64 = 0x7ff8_0000_0000_0000;
    pub(super) const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
    pub(super) const TAG_INT: u64 = 0x0001_0000_0000_0000;
    pub(super) const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
    pub(super) const TAG_NONE: u64 = 0x0003_0000_0000_0000;
    pub(super) const TAG_PTR: u64 = 0x0004_0000_0000_0000;
    pub(super) const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
    pub(super) const TAG_MASK: u64 = 0x0007_0000_0000_0000;
    pub(super) const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
    pub(super) const INT_WIDTH: u64 = 47;
    pub(super) const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
    pub(super) const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;
    pub(super) const GENERATOR_CONTROL_BYTES: i32 = 48;
    pub(super) const TASK_KIND_FUTURE: i64 = 0;
    pub(super) const TASK_KIND_GENERATOR: i64 = 1;
    pub(super) const TASK_KIND_COROUTINE: i64 = 2;
    // FUNC_DEFAULT_* constants moved to the runtime (molt_call_func_dispatch).
    // Kept as dead_code in case the WASM backend needs them during outlining.
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_NONE: i64 = 1;
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_DICT_POP: i64 = 2;
    #[allow(dead_code)]
    pub(super) const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
    pub(super) const HEADER_SIZE_BYTES: i32 = 40;
    pub(super) const HEADER_STATE_OFFSET: i32 = -(HEADER_SIZE_BYTES - 16);
    pub(super) const HEADER_REFCOUNT_OFFSET: i32 = -(HEADER_SIZE_BYTES - 4);
    pub(super) const HEADER_FLAGS_OFFSET: i32 = -(HEADER_SIZE_BYTES - 32);
    pub(super) const HEADER_FLAG_IMMORTAL: u64 = 1 << 15;
}

#[cfg(feature = "native-backend")]
use native_backend_consts::*;

#[cfg(feature = "native-backend")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ImportSignatureShape {
    params: Vec<String>,
    returns: Vec<String>,
}

#[cfg(feature = "native-backend")]
impl ImportSignatureShape {
    fn from_types(params: &[types::Type], returns: &[types::Type]) -> Self {
        Self {
            params: params.iter().map(ToString::to_string).collect(),
            returns: returns.iter().map(ToString::to_string).collect(),
        }
    }
}

#[cfg(feature = "native-backend")]
struct NativeBackendIrAnalysis {
    defined_functions: BTreeSet<String>,
    closure_functions: BTreeSet<String>,
    task_kinds: BTreeMap<String, TrampolineKind>,
    task_closure_sizes: BTreeMap<String, i64>,
    needs_inlining: bool,
}

#[cfg(feature = "native-backend")]
fn analyze_native_backend_ir(ir: &SimpleIR) -> NativeBackendIrAnalysis {
    let defined_functions: BTreeSet<String> =
        ir.functions.iter().map(|func| func.name.clone()).collect();
    let mut closure_functions: BTreeSet<String> = BTreeSet::new();
    let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
    let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
    let mut needs_inlining = false;
    let mut has_task_attrs = false;

    for func_ir in &ir.functions {
        for op in &func_ir.ops {
            match op.kind.as_str() {
                "call_internal" => needs_inlining = true,
                "func_new_closure" => {
                    if let Some(name) = op.s_value.as_ref() {
                        closure_functions.insert(name.clone());
                    }
                }
                "set_attr_generic_obj" => {
                    if matches!(
                        op.s_value.as_deref(),
                        Some(
                            "__molt_is_generator__"
                                | "__molt_is_coroutine__"
                                | "__molt_is_async_generator__"
                                | "__molt_closure_size__"
                        )
                    ) {
                        has_task_attrs = true;
                    }
                }
                _ => {}
            }
        }
    }

    if has_task_attrs {
        for func_ir in &ir.functions {
            let mut func_obj_names: BTreeMap<String, String> = BTreeMap::new();
            let mut const_values: BTreeMap<String, i64> = BTreeMap::new();
            let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "const" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0);
                        const_values.insert(out.clone(), val);
                    }
                    "const_bool" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0) != 0;
                        const_bools.insert(out.clone(), val);
                    }
                    "func_new" | "func_new_closure" => {
                        let Some(name) = op.s_value.as_ref() else {
                            continue;
                        };
                        if let Some(out) = op.out.as_ref() {
                            func_obj_names.insert(out.clone(), name.clone());
                        }
                    }
                    _ => {}
                }
            }
            for op in &func_ir.ops {
                if op.kind != "set_attr_generic_obj" {
                    continue;
                }
                let Some(attr) = op.s_value.as_deref() else {
                    continue;
                };
                if attr != "__molt_is_generator__"
                    && attr != "__molt_is_coroutine__"
                    && attr != "__molt_is_async_generator__"
                    && attr != "__molt_closure_size__"
                {
                    continue;
                }
                let args = op.args.as_ref().expect("set_attr_generic_obj args missing");
                let Some(func_name) = func_obj_names.get(&args[0]) else {
                    continue;
                };
                match attr {
                    "__molt_is_generator__"
                    | "__molt_is_coroutine__"
                    | "__molt_is_async_generator__" => {
                        let val_name = &args[1];
                        let is_true = const_bools
                            .get(val_name)
                            .copied()
                            .or_else(|| const_values.get(val_name).map(|val| *val != 0))
                            .unwrap_or(false);
                        if is_true {
                            if !func_name.ends_with("_poll") {
                                continue;
                            }
                            let kind = match attr {
                                "__molt_is_generator__" => TrampolineKind::Generator,
                                "__molt_is_coroutine__" => TrampolineKind::Coroutine,
                                "__molt_is_async_generator__" => TrampolineKind::AsyncGen,
                                _ => TrampolineKind::Plain,
                            };
                            if let Some(prev) = task_kinds.insert(func_name.clone(), kind)
                                && prev != kind
                            {
                                panic!(
                                    "conflicting task kinds for {func_name}: {:?} vs {:?}",
                                    prev, kind
                                );
                            }
                        }
                    }
                    "__molt_closure_size__" => {
                        let val_name = &args[1];
                        if let Some(size) = const_values.get(val_name) {
                            task_closure_sizes.insert(func_name.clone(), *size);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    NativeBackendIrAnalysis {
        defined_functions,
        closure_functions,
        task_kinds,
        task_closure_sizes,
        needs_inlining,
    }
}

#[cfg(feature = "native-backend")]
fn find_zero_pred_blocks(func: &Function) -> Vec<Block> {
    let mut preds: BTreeMap<Block, usize> = BTreeMap::new();
    for block in func.layout.blocks() {
        preds.entry(block).or_insert(0);
    }
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for dest in func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
            {
                let dest_block = dest.block(&func.dfg.value_lists);
                *preds.entry(dest_block).or_insert(0) += 1;
            }
        }
    }
    let entry = func.layout.entry_block();
    preds
        .into_iter()
        .filter(|(block, count)| Some(*block) != entry && *count == 0)
        .map(|(block, _)| block)
        .collect()
}

#[cfg(feature = "native-backend")]
fn ensure_block_in_layout(builder: &mut FunctionBuilder, block: Block) {
    if builder.func.layout.is_block_inserted(block) {
        return;
    }
    if let Some(current) = builder.current_block()
        && builder.func.layout.is_block_inserted(current)
    {
        builder.insert_block_after(block, current);
        return;
    }
    builder.func.layout.append_block(block);
}

#[cfg(feature = "native-backend")]
fn block_has_terminator(builder: &FunctionBuilder, block: Block) -> bool {
    builder
        .func
        .layout
        .last_inst(block)
        .map(|inst| builder.func.dfg.insts[inst].opcode().is_terminator())
        .unwrap_or(false)
}

#[cfg(feature = "native-backend")]
fn sync_block_filled(builder: &FunctionBuilder, is_block_filled: &mut bool) {
    if let Some(block) = builder.current_block()
        && block_has_terminator(builder, block)
    {
        *is_block_filled = true;
    }
}

#[cfg(feature = "native-backend")]
fn switch_to_block_tracking(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    builder.switch_to_block(block);
    *is_block_filled = block_has_terminator(builder, block);
}

#[cfg(feature = "native-backend")]
fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

#[cfg(feature = "native-backend")]
fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        0x7ff0_0000_0000_0001_u64 as i64
    } else {
        val.to_bits() as i64
    }
}

#[cfg(feature = "native-backend")]
fn pending_bits() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

#[cfg(feature = "native-backend")]
fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

#[cfg(feature = "native-backend")]
fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

#[cfg(feature = "native-backend")]
fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
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
    // Keep the id within inline-int payload range and avoid zero.
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

#[cfg(feature = "native-backend")]
fn unbox_int(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Debug-mode guard: verify the value actually carries the int tag before
    // unboxing.  In release builds this is a no-op; in debug builds an illegal
    // trap fires immediately if a non-int value reaches this path.
    #[cfg(debug_assertions)]
    {
        let mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
        let expected = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
        let masked = builder.ins().band(val, mask);
        let is_int = builder
            .ins()
            .icmp(IntCC::Equal, masked, expected);
        builder
            .ins()
            .trapz(is_int, cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    // The ishl by INT_SHIFT (17) shifts out the upper 17 tag bits (QNAN+TAG),
    // then sshr sign-extends the 47-bit payload. No separate band with INT_MASK
    // is needed — the shift pair implicitly strips the tag.
    let shift = builder.ins().iconst(types::I64, INT_SHIFT);
    let shifted = builder.ins().ishl(val, shift);
    builder.ins().sshr(shifted, shift)
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn is_int_tag(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
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
fn fused_tag_check_and_unbox_int(builder: &mut FunctionBuilder, val: Value) -> (Value, Value) {
    let expected_tag = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
    let xored = builder.ins().bxor(val, expected_tag);
    let shift = builder.ins().iconst(types::I64, INT_SHIFT);
    let shifted = builder.ins().ishl(xored, shift);
    let unboxed = builder.ins().sshr(shifted, shift);
    (xored, unboxed)
}

/// Check that two XOR'd values both represent NaN-boxed ints.
///
/// Takes the `xored` outputs from two `fused_tag_check_and_unbox_int` calls
/// and checks that both had their tag bits zeroed (i.e., both were ints).
/// Uses BOR to combine the two values, then checks that the upper 17 bits
/// of the combined result are zero — true iff both inputs were ints.
#[cfg(feature = "native-backend")]
fn fused_both_int_check(
    builder: &mut FunctionBuilder,
    lhs_xored: Value,
    rhs_xored: Value,
) -> Value {
    let combined = builder.ins().bor(lhs_xored, rhs_xored);
    let tag_shift = builder.ins().iconst(types::I64, INT_WIDTH as i64);
    let upper = builder.ins().ushr(combined, tag_shift);
    builder.ins().icmp_imm(IntCC::Equal, upper, 0)
}

/// Check whether a NaN-boxed value is a special tagged type (int/bool/none/ptr/pending)
/// rather than a plain f64.
///
/// All NaN-boxed specials have bits 62..48 in the range `0x7FF9..=0x7FFD`.
/// Returns true if the value IS a special (i.e., NOT a float).
#[cfg(feature = "native-backend")]
fn is_nanboxed_special(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Shift right by 48 to isolate the tag field, then check range [0x7FF9, 0x7FFD].
    let shift48 = builder.ins().iconst(types::I64, 48);
    let tag16 = builder.ins().ushr(val, shift48);
    // tag16 - 0x7FF9; result < 5 means it's a tagged special
    let base = builder.ins().iconst(types::I64, 0x7FF9);
    let adjusted = builder.ins().isub(tag16, base);
    let limit = builder.ins().iconst(types::I64, 5);
    builder.ins().icmp(IntCC::UnsignedLessThan, adjusted, limit)
}

/// Check that both NaN-boxed values are plain f64 (not tagged specials).
#[cfg(feature = "native-backend")]
fn both_float_check(builder: &mut FunctionBuilder, lhs: Value, rhs: Value) -> Value {
    let lhs_special = is_nanboxed_special(builder, lhs);
    let rhs_special = is_nanboxed_special(builder, rhs);
    let either_special = builder.ins().bor(lhs_special, rhs_special);
    // both_float = !(lhs_special || rhs_special)
    // Since is_nanboxed_special returns an i8 (0 or 1), we check either_special == 0
    builder.ins().icmp_imm(IntCC::Equal, either_special, 0)
}

#[cfg(feature = "native-backend")]
fn box_int_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, INT_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_INT) as i64);
    builder.ins().bor(tag, masked)
}

#[cfg(feature = "native-backend")]
fn box_float_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Canonicalize NaN: if the f64 value is NaN, replace with CANONICAL_NAN_BITS
    // to avoid collision with the QNAN tag prefix used by NaN-boxing.
    let raw_bits = builder.ins().bitcast(types::I64, MemFlags::new(), val);
    let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
    let canonical = builder.ins().iconst(types::I64, CANONICAL_NAN_BITS as i64);
    builder.ins().select(is_nan, canonical, raw_bits)
}

#[cfg(feature = "native-backend")]
fn int_value_fits_inline(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Inline ints are 47-bit signed payloads. Round-trip through box/unbox to
    // guard against silent wrap in fast arithmetic lowering.
    let boxed = box_int_value(builder, val);
    let unboxed = unbox_int(builder, boxed);
    builder.ins().icmp(IntCC::Equal, val, unboxed)
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
#[cfg(feature = "native-backend")]
fn imul_checked_inline(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value) {
    let prod = builder.ins().imul(lhs, rhs);
    // smulhi gives the upper 64 bits of the signed 128-bit product.
    let hi = builder.ins().smulhi(lhs, rhs);
    // If there was no 64-bit overflow, hi must be the sign-extension of prod,
    // i.e. hi == prod >> 63 (arithmetic).
    let sixty_three = builder.ins().iconst(types::I64, 63);
    let sign = builder.ins().sshr(prod, sixty_three);
    let no_overflow_64 = builder.ins().icmp(IntCC::Equal, hi, sign);
    // Also check the result fits in 47-bit signed payload.
    let fits_47 = int_value_fits_inline(builder, prod);
    let both_ok = builder.ins().band(no_overflow_64, fits_47);
    (prod, both_ok)
}

#[cfg(feature = "native-backend")]
fn box_bool_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    let bool_val = builder.ins().select(val, one, zero);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_BOOL) as i64);
    builder.ins().bor(tag, bool_val)
}

#[cfg(feature = "native-backend")]
fn unbox_ptr_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, POINTER_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, 16);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

#[cfg(feature = "native-backend")]
fn box_ptr_value(builder: &mut FunctionBuilder, val: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, POINTER_MASK as i64);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, (QNAN | TAG_PTR) as i64);
    builder.ins().bor(tag, masked)
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn emit_maybe_ref_adjust(builder: &mut FunctionBuilder, val: Value, obj_ref_fn: FuncRef) {
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
fn inline_rc_enabled() -> bool {
    true
}

/// Emit an inlined `inc_ref_obj` as Cranelift IR instead of a function call.
///
/// The emitted IR is equivalent to:
/// ```text
/// if (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR):
///     ptr = sign_extend(val & POINTER_MASK)
///     flags = *(ptr - 8) as u64
///     if (flags & HEADER_FLAG_IMMORTAL) == 0:
///         rc = *(ptr - 36) as u32
///         *(ptr - 36) = rc + 1
/// ```
#[cfg(feature = "native-backend")]
fn emit_inline_inc_ref_obj(builder: &mut FunctionBuilder, val: Value) {
    let current_block = builder.current_block().expect("no current block");

    // --- Block layout: current → check_immortal → do_inc → merge ---
    let check_immortal_block = builder.create_block();
    let do_inc_block = builder.create_block();
    let merge_block = builder.create_block();

    // 1. Check if val is a heap pointer: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let tag_check_mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, (QNAN | TAG_PTR) as i64);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    builder
        .ins()
        .brif(is_ptr, check_immortal_block, &[], merge_block, &[]);

    // 2. Extract raw data pointer and check immortal flag
    builder.switch_to_block(check_immortal_block);
    let raw_ptr = unbox_ptr_value(builder, val);

    // Load flags (u64 at ptr + HEADER_FLAGS_OFFSET)
    let flags = builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        raw_ptr,
        HEADER_FLAGS_OFFSET,
    );
    let immortal_mask = builder
        .ins()
        .iconst(types::I64, HEADER_FLAG_IMMORTAL as i64);
    let immortal_bits = builder.ins().band(flags, immortal_mask);
    let zero = builder.ins().iconst(types::I64, 0);
    let is_mortal = builder.ins().icmp(IntCC::Equal, immortal_bits, zero);
    builder
        .ins()
        .brif(is_mortal, do_inc_block, &[], merge_block, &[]);

    // 3. Increment refcount atomically using atomic_rmw (Add)
    builder.switch_to_block(do_inc_block);
    let rc_offset = builder
        .ins()
        .iconst(types::I64, HEADER_REFCOUNT_OFFSET as i64);
    let rc_addr = builder.ins().iadd(raw_ptr, rc_offset);
    let one_i32 = builder.ins().iconst(types::I32, 1);
    builder
        .ins()
        .atomic_rmw(types::I32, MemFlags::trusted(), AtomicRmwOp::Add, rc_addr, one_i32);
    builder.ins().jump(merge_block, &[]);

    // 4. Merge — continue in the merge block
    builder.switch_to_block(merge_block);
    // Seal the blocks we created (they have known predecessors now)
    builder.seal_block(check_immortal_block);
    builder.seal_block(do_inc_block);
    builder.seal_block(merge_block);

    // Note: caller must NOT seal current_block before calling this function
    // if it hasn't been sealed yet. The merge_block becomes the new "current"
    // block for subsequent instruction emission.
    let _ = current_block; // suppress unused warning
}

/// Emit an inc_ref_obj — either inlined or as a function call depending on
/// the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
fn emit_inc_ref_obj(builder: &mut FunctionBuilder, val: Value, call_ref: FuncRef) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val);
    } else {
        builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a ref-adjust (inc_ref_obj) — either inlined or as a function call
/// depending on the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
fn emit_maybe_ref_adjust_v2(builder: &mut FunctionBuilder, val: Value, call_ref: FuncRef) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val);
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
fn emit_dec_ref_obj(builder: &mut FunctionBuilder, val: Value, call_ref: FuncRef) {
    if !inline_rc_enabled() {
        builder.ins().call(call_ref, &[val]);
        return;
    }
    // Inline tag check: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let call_block = builder.create_block();
    let merge_block = builder.create_block();

    let tag_check_mask = builder.ins().iconst(types::I64, (QNAN | TAG_MASK) as i64);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, (QNAN | TAG_PTR) as i64);
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

#[derive(Clone, Copy)]
#[cfg(feature = "native-backend")]
struct VarValue(Value);

#[cfg(feature = "native-backend")]
impl std::ops::Deref for VarValue {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

#[cfg(feature = "native-backend")]
fn var_get(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: &str,
) -> Option<VarValue> {
    vars.get(name).map(|var| VarValue(builder.use_var(*var)))
}

#[cfg(feature = "native-backend")]
fn def_var_named(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: impl AsRef<str>,
    val: Value,
) {
    let name_ref = name.as_ref();
    if name_ref == "none" {
        return;
    }
    let var = *vars
        .get(name_ref)
        .unwrap_or_else(|| panic!("Var not found: {name_ref}"));
    builder.def_var(var, val);
}

#[cfg(feature = "native-backend")]
fn jump_block(builder: &mut FunctionBuilder, target: Block, args: &[Value]) {
    let block_args: Vec<BlockArg> = args.iter().copied().map(BlockArg::from).collect();
    builder.ins().jump(target, &block_args);
}

#[cfg(feature = "native-backend")]
fn brif_block(
    builder: &mut FunctionBuilder,
    cond: Value,
    then_block: Block,
    then_args: &[Value],
    else_block: Block,
    else_args: &[Value],
) {
    let then_block_args: Vec<BlockArg> = then_args.iter().copied().map(BlockArg::from).collect();
    let else_block_args: Vec<BlockArg> = else_args.iter().copied().map(BlockArg::from).collect();
    builder.ins().brif(
        cond,
        then_block,
        &then_block_args,
        else_block,
        &else_block_args,
    );
}

#[cfg(feature = "native-backend")]
fn parse_inst_id(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"inst") {
            let mut j = i + 4;
            let mut value: usize = 0;
            let mut found = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                found = true;
                value = value * 10 + (bytes[j] - b'0') as usize;
                j += 1;
            }
            if found {
                return Some(value);
            }
        }
        i += 1;
    }
    None
}

#[cfg(feature = "native-backend")]
struct DumpIrConfig {
    mode: String,
    filter: Option<String>,
}

#[cfg(feature = "native-backend")]
fn should_dump_ir() -> Option<DumpIrConfig> {
    let raw = std::env::var("MOLT_DUMP_IR").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let (mode, filter) = if let Some((left, right)) = trimmed.split_once(':') {
        let left_trim = left.trim();
        let right_trim = right.trim();
        let mode = if left_trim.eq_ignore_ascii_case("full") {
            "full"
        } else {
            "control"
        };
        let filter = if right_trim.is_empty() {
            None
        } else {
            Some(right_trim.to_string())
        };
        (mode.to_string(), filter)
    } else if lower == "full" || lower == "control" || lower == "1" || lower == "all" {
        let mode = if lower == "full" { "full" } else { "control" };
        (mode.to_string(), None)
    } else {
        ("control".to_string(), Some(trimmed.to_string()))
    };
    Some(DumpIrConfig { mode, filter })
}

#[cfg(feature = "native-backend")]
fn dump_ir_matches(config: &DumpIrConfig, func_name: &str) -> bool {
    let Some(filter) = config.filter.as_ref() else {
        return true;
    };
    if filter == "1" || filter.eq_ignore_ascii_case("all") {
        return true;
    }
    func_name == filter || func_name.contains(filter)
}

#[cfg(feature = "native-backend")]
struct TraceOpsConfig {
    stride: usize,
}

#[cfg(feature = "native-backend")]
fn should_trace_ops(func_name: &str) -> Option<TraceOpsConfig> {
    static RAW: OnceLock<Option<String>> = OnceLock::new();
    let raw = RAW
        .get_or_init(|| {
            std::env::var("MOLT_TRACE_OP_PROGRESS")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .as_ref()?;
    let (filter_part, stride_part) = match raw.split_once(':') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (raw.as_str(), None),
    };
    let stride = stride_part
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5_000);
    let matches = filter_part == "1"
        || filter_part.eq_ignore_ascii_case("all")
        || func_name == filter_part
        || func_name.contains(filter_part);
    if matches {
        Some(TraceOpsConfig { stride })
    } else {
        None
    }
}

#[cfg(feature = "native-backend")]
fn dump_ir_ops(func_ir: &FunctionIR, mode: &str) {
    let mut out = String::new();
    let full = mode.eq_ignore_ascii_case("full");
    let mut last_written = 0usize;
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !full {
            let kind = op.kind.as_str();
            let is_control = matches!(
                kind,
                "if" | "else"
                    | "end_if"
                    | "phi"
                    | "label"
                    | "state_label"
                    | "jump"
                    | "br_if"
                    | "loop_start"
                    | "loop_end"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_break"
                    | "loop_continue"
                    | "ret"
            );
            if !is_control {
                continue;
            }
        }
        let mut detail = Vec::new();
        if let Some(out_name) = &op.out {
            detail.push(format!("out={out_name}"));
        }
        if let Some(var) = &op.var {
            detail.push(format!("var={var}"));
        }
        if let Some(args) = &op.args {
            detail.push(format!("args=[{}]", args.join(", ")));
        }
        if let Some(val) = op.value {
            detail.push(format!("value={val}"));
        }
        if let Some(val) = op.f_value {
            detail.push(format!("f_value={val}"));
        }
        if let Some(val) = &op.s_value {
            detail.push(format!("s_value={val}"));
        }
        if let Some(bytes) = &op.bytes {
            detail.push(format!("bytes_len={}", bytes.len()));
        }
        if let Some(fast_int) = op.fast_int {
            detail.push(format!("fast_int={fast_int}"));
        }
        let _ = writeln!(out, "{idx:04}: {:<20} {}", op.kind, detail.join(" "));
        last_written = idx;
    }
    if last_written == 0 && func_ir.ops.is_empty() {
        return;
    }
    eprintln!("IR ops for {} (mode={}):\n{}", func_ir.name, mode, out);
}

#[cfg(feature = "native-backend")]
fn drain_cleanup_tracked(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    let mut cleanup = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        // If not in last_use, default to MAX (keep alive) — NOT op_idx.
        // Using op_idx as default causes premature cleanup of variables
        // that are used later but not yet tracked in last_use.
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            cleanup.push(name.clone());
            return false;
        }
        true
    });
    cleanup
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn collect_cleanup_tracked(
    names: &[String],
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| skip != Some(name.as_str()))
        .filter(|name| last_use.get(*name).copied().unwrap_or(op_idx) <= op_idx)
        .cloned()
        .collect()
}

#[cfg(feature = "native-backend")]
fn extend_unique_tracked(dst: &mut Vec<String>, src: Vec<String>) {
    if src.is_empty() {
        return;
    }
    if dst.is_empty() {
        dst.extend(src);
        return;
    }
    // Dedup by `name` so multi-predecessor merges don't create double-decref hazards.
    let mut seen: BTreeSet<String> = dst.iter().cloned().collect();
    for name in src {
        if seen.insert(name.clone()) {
            dst.push(name);
        }
    }
}

/// Propagate tracked objects to ALL branch target blocks.
/// Prevents use-after-free when exception handlers access freed objects.
#[cfg(feature = "native-backend")]
pub(crate) fn propagate_tracked_to_branches(
    block_tracked: &mut BTreeMap<cranelift_codegen::ir::Block, Vec<String>>,
    targets: &[cranelift_codegen::ir::Block],
    carry: Vec<String>,
) {
    if carry.is_empty() || targets.is_empty() {
        return;
    }
    if targets.len() == 1 {
        extend_unique_tracked(block_tracked.entry(targets[0]).or_default(), carry);
        return;
    }
    let last_idx = targets.len() - 1;
    for (i, &target) in targets.iter().enumerate() {
        if i == last_idx {
            extend_unique_tracked(block_tracked.entry(target).or_default(), carry);
            return;
        }
        extend_unique_tracked(
            block_tracked.entry(target).or_default(),
            carry.clone(),
        );
    }
}

#[cfg(feature = "native-backend")]
fn drain_cleanup_entry_tracked(
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
) -> Vec<Value> {
    let mut cleanup = Vec::new();
    let mut to_remove = Vec::new();
    names.retain(|name| {
        // If not in last_use, default to MAX (keep alive) — NOT op_idx.
        // Using op_idx as default causes premature cleanup of variables
        // that are used later but not yet tracked in last_use.
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            if let Some(val) = entry_vars.get(name) {
                cleanup.push(*val);
            }
            // Mark for removal from entry_vars so no other cleanup path
            // (exception handler, finalize block) can double dec-ref.
            to_remove.push(name.clone());
            return false;
        }
        true
    });
    for name in to_remove {
        entry_vars.remove(&name);
    }
    cleanup
}

// ---------------------------------------------------------------------------
// RC coalescing: eliminate redundant inc_ref / dec_ref pairs.
// ---------------------------------------------------------------------------

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
const CONTROL_FLOW_OPS: &[&str] = &[
    "if", "else", "end_if", "loop_start", "loop_end", "loop_for_start",
    "loop_for_end", "label", "state_label", "jump", "return", "state_yield",
    "check_exception", "raise",
];

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    let cf_set: HashSet<&str> = CONTROL_FLOW_OPS.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) { continue; }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec { continue; }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) { break; }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release")
                    && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow")
                    && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b.args.as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var { break; }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) { continue; }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") { continue; }
        let out_name = match op.out.as_deref() {
            Some(name) if name != "none" => name,
            _ => continue,
        };
        let last = last_use.get(out_name).copied().unwrap_or(idx);
        if last <= idx {
            skip_ops.insert(idx);
            skip_dec_ref.insert(out_name.to_string());
        }
    }

    if !skip_ops.is_empty() || !skip_dec_ref.is_empty() {
        static RC_COALESCE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *RC_COALESCE_TRACE.get_or_init(|| {
            std::env::var("MOLT_RC_COALESCE_TRACE").as_deref() == Ok("1")
        });
        if trace {
            eprintln!(
                "[rc-coalesce] eliminated {} RC ops, {} dec_ref skips",
                skip_ops.len(), skip_dec_ref.len()
            );
        }
    }

    (skip_ops, skip_dec_ref)
}

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) enum TrampolineKind {
    Plain,
    Generator,
    Coroutine,
    AsyncGen,
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct TrampolineSpec {
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
}

#[cfg(feature = "native-backend")]
pub struct SimpleBackend {
    module: ObjectModule,
    ctx: Context,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    trampoline_ids: BTreeMap<TrampolineKey, cranelift_module::FuncId>,
    import_ids: BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    pub skip_ir_passes: bool,
    /// Function names that exist in other batches — use Linkage::Import, not trap stubs.
    pub external_function_names: std::collections::BTreeSet<String>,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    data_pool: BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: u64,
    // Track the arity each user-defined function was declared with so that
    // call sites that reference the same function (potentially with a
    // different number of actual arguments, e.g. kwargs expansion) can
    // construct a matching Cranelift signature for `declare_function`.
    declared_func_arities: BTreeMap<String, usize>,
    /// Track which functions have been given a body (defined), so we can
    /// emit trap stubs for declared-but-undefined `__ov` variants after
    /// all functions are compiled.
    defined_func_names: std::collections::BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
struct IfFrame {
    else_block: Option<Block>,
    merge_block: Block,
    has_else: bool,
    then_terminal: bool,
    else_terminal: bool,
    phi_ops: Vec<(String, String, String)>,
    phi_params: Vec<Value>,
}

#[cfg(feature = "native-backend")]
struct LoopFrame {
    loop_block: Block,
    body_block: Block,
    after_block: Block,
    index_name: Option<String>,
    next_index: Option<Value>,
}

#[cfg(feature = "native-backend")]
fn parse_truthy_env(raw: &str) -> bool {
    let norm = raw.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

#[cfg(feature = "native-backend")]
impl Default for SimpleBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub fn new() -> Self {
        Self::new_with_target(None)
    }

    pub fn new_with_target(target: Option<&str>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        // Cranelift optimization level: "none", "speed", or "speed_and_size".
        // Default to "speed" for production quality codegen.  Override with
        // MOLT_BACKEND_OPT_LEVEL=none for fast dev-loop compilation (~3-5x
        // faster compile times at the cost of ~30-50% slower generated code).
        let opt_level = env_setting("MOLT_BACKEND_OPT_LEVEL")
            .unwrap_or_else(|| "speed".to_string());
        flag_builder.set("opt_level", &opt_level).unwrap_or_else(|err| {
            panic!("invalid MOLT_BACKEND_OPT_LEVEL={opt_level:?}: {err:?}")
        });
        let regalloc_algorithm =
            env_setting("MOLT_BACKEND_REGALLOC_ALGORITHM").unwrap_or_else(|| {
                // When opt_level=none, default to the fast single-pass
                // allocator regardless of build profile — the user has
                // explicitly asked for compile-time speed.
                if cfg!(debug_assertions) || opt_level == "none" {
                    "single_pass".to_string()
                } else {
                    "backtracking".to_string()
                }
            });
        flag_builder
            .set("regalloc_algorithm", &regalloc_algorithm)
            .unwrap_or_else(|err| {
                panic!("invalid MOLT_BACKEND_REGALLOC_ALGORITHM={regalloc_algorithm:?}: {err:?}")
            });
        // Cranelift 0.128 adds explicit minimum function alignment tuning.
        // Default to 16-byte release alignment for better i-cache/branch
        // behavior on hot call-heavy kernels; keep debug/dev unchanged.
        let min_alignment_log2 = env_setting("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2")
            .unwrap_or_else(|| {
                if cfg!(debug_assertions) {
                    "0".to_string()
                } else {
                    "4".to_string()
                }
            });
        flag_builder
            .set("log2_min_function_alignment", &min_alignment_log2)
            .unwrap_or_else(|err| {
                panic!(
                    "invalid MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2={min_alignment_log2:?}: {err:?}"
                )
            });
        if let Some(libcall_call_conv) = env_setting("MOLT_BACKEND_LIBCALL_CALL_CONV") {
            flag_builder
                .set("libcall_call_conv", &libcall_call_conv)
                .unwrap_or_else(|err| {
                    panic!("invalid MOLT_BACKEND_LIBCALL_CALL_CONV={libcall_call_conv:?}: {err:?}")
                });
        }
        // Cranelift verifier catches IR invariant violations (type mismatches,
        // dominator tree bugs). Enable in debug builds; disable in release for
        // speed. Override with MOLT_BACKEND_ENABLE_VERIFIER=0|1.
        let default_enable_verifier = cfg!(debug_assertions);
        let enable_verifier = env_setting("MOLT_BACKEND_ENABLE_VERIFIER")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(default_enable_verifier);
        flag_builder
            .set(
                "enable_verifier",
                if enable_verifier { "true" } else { "false" },
            )
            .unwrap();
        // Cranelift alias analysis: enables redundant-load elimination across
        // memory operations within a basic block. Safe for our codegen because
        // we never emit raw pointer aliasing between different object fields.
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        // Emit CFG metadata in machine code output — enables downstream tools
        // and profilers to reconstruct control-flow graphs from compiled objects.
        flag_builder.set("machine_code_cfg_info", "true").unwrap();
        // Use colocated libcalls: our generated code and runtime libcalls live
        // in the same link unit — colocated calls skip GOT/PLT indirection and
        // use direct PC-relative calls instead.
        flag_builder.set("use_colocated_libcalls", "true").unwrap();
        // Detect whether we are targeting aarch64 — either because we are
        // compiling natively on aarch64, or because an explicit cross-compile
        // target triple was supplied that contains "aarch64".
        let targeting_aarch64 = match target {
            Some(t) => t.contains("aarch64"),
            None => cfg!(target_arch = "aarch64"),
        };
        // Frame pointers: always preserve on aarch64 to ensure correct stack
        // frame layout for large functions (>16KB frames).  Cranelift 0.128 can
        // generate incorrect SP-relative accesses on aarch64 when frame pointers
        // are omitted and the frame exceeds the immediate offset range, leading
        // to SIGTRAP (exit 133) in generated code.  On x86_64 the cost is one
        // register (rbp); on aarch64 x29 is conventionally reserved anyway.
        // Debug builds always preserve for profiler/debugger support.
        flag_builder
            .set(
                "preserve_frame_pointers",
                if cfg!(debug_assertions) || targeting_aarch64 {
                    "true"
                } else {
                    "false"
                },
            )
            .unwrap();
        // Spectre mitigations: Molt compiles trusted user code (not sandboxed
        // plugins), so Spectre v1 heap/table mitigations add unnecessary overhead.
        flag_builder
            .set("enable_heap_access_spectre_mitigation", "false")
            .unwrap();
        flag_builder
            .set("enable_table_access_spectre_mitigation", "false")
            .unwrap();
        // Stack probing strategy: use outline (call-based) probes on aarch64
        // to avoid a Cranelift 0.128 bug where inline probe loops generate
        // incorrect touch sequences for frames >16KB, causing SIGTRAP.
        // On x86_64, inline probes are safe and faster for deep recursion.
        flag_builder
            .set(
                "probestack_strategy",
                if targeting_aarch64 {
                    "outline"
                } else {
                    "inline"
                },
            )
            .unwrap();
        // MOLT_PORTABLE=1 forces baseline ISA (no host-specific features like AVX2).
        // This ensures reproducible codegen across different machines at the cost of
        // ~5-15% runtime performance on modern CPUs with advanced features.
        let portable = env_setting("MOLT_PORTABLE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let isa_builder = if let Some(triple) = target {
            isa::lookup_by_name(triple).unwrap_or_else(|msg| {
                panic!("target {} is not supported: {}", triple, msg);
            })
        } else if portable {
            // Baseline ISA: no auto-detected host features. Produces portable
            // binaries that run on any CPU supporting the base architecture.
            native_isa_builder_with_options(false).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        } else {
            // Auto-detect host CPU features (AVX2, SSE4.2, BMI2, POPCNT on x86;
            // NEON, AES, CRC on aarch64). Allows Cranelift to emit feature-specific
            // instructions like vpmovmskb, popcnt, tzcnt, etc.
            native_isa_builder_with_options(true).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        };
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder = ObjectBuilder::new(
            isa,
            "molt_output",
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        // Emit each function into its own object section so the linker can
        // discard unreferenced runtime functions via -dead_strip / --gc-sections.
        builder.per_function_section(true);
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self {
            module,
            ctx,
            trampoline_ids: BTreeMap::new(),
            import_ids: BTreeMap::new(),
            skip_ir_passes: false,
            external_function_names: std::collections::BTreeSet::new(),
            data_pool: BTreeMap::new(),
            next_data_id: 0,
            declared_func_arities: BTreeMap::new(),
            defined_func_names: std::collections::BTreeSet::new(),
        }
    }

    /// Retry compiling a function at `opt_level=none` after the optimizing
    /// pipeline panicked.  Builds a throwaway ISA that matches the module's
    /// target but disables all optimization passes (which avoids the
    /// `remove_constant_phis` assertion and similar upstream Cranelift bugs).
    /// The compiled bytes are installed via `define_function_bytes` so the
    /// module's own ISA is never consulted for code generation.
    fn retry_define_at_opt_none(
        module: &mut ObjectModule,
        func_id: cranelift_module::FuncId,
        func: cranelift_codegen::ir::Function,
        func_name: &str,
    ) -> Result<(), String> {
        use cranelift_codegen::control::ControlPlane;

        // Build a fallback ISA with opt_level=none — identical flags to the
        // primary ISA except the optimization level.
        let mut fb = settings::builder();
        fb.set("opt_level", "none").unwrap();
        fb.set("is_pic", "true").unwrap();
        // Carry forward the same safety-critical settings used by
        // new_with_target so the emitted code is ABI-compatible.
        fb.set("use_colocated_libcalls", "true").unwrap();
        fb.set("enable_alias_analysis", "false").unwrap();
        let targeting_aarch64 = cfg!(target_arch = "aarch64");
        fb.set(
            "preserve_frame_pointers",
            if cfg!(debug_assertions) || targeting_aarch64 {
                "true"
            } else {
                "false"
            },
        )
        .unwrap();
        fb.set("enable_heap_access_spectre_mitigation", "false")
            .unwrap();
        fb.set("enable_table_access_spectre_mitigation", "false")
            .unwrap();
        fb.set(
            "probestack_strategy",
            if targeting_aarch64 {
                "outline"
            } else {
                "inline"
            },
        )
        .unwrap();
        let fallback_isa = native_isa_builder_with_options(true)
            .map_err(|e| format!("ISA builder: {e}"))?
            .finish(settings::Flags::new(fb))
            .map_err(|e| format!("ISA finish: {e}"))?;

        let mut retry_ctx = Context::for_function(func);
        let mut ctrl = ControlPlane::default();
        retry_ctx
            .compile(&*fallback_isa, &mut ctrl)
            .map_err(|e| format!("compile at O0: {e:?}"))?;
        let compiled = retry_ctx.compiled_code().unwrap();
        let alignment = compiled.buffer.alignment as u64;
        let code = compiled.buffer.data().to_vec();
        let relocs: Vec<cranelift_module::ModuleReloc> = compiled
            .buffer
            .relocs()
            .iter()
            .map(|r| {
                cranelift_module::ModuleReloc::from_mach_reloc(
                    r,
                    &retry_ctx.func,
                    func_id,
                )
            })
            .collect();
        module
            .define_function_bytes(func_id, alignment, &code, &relocs)
            .map_err(|e| format!("define_function_bytes for {func_name}: {e}"))?;
        Ok(())
    }

    /// Emit a minimal function body that immediately traps.  Used as a
    /// fallback when a function is too large for Cranelift to compile
    /// (even at opt_level=none).  The stub lets the rest of the object
    /// file link successfully; if the function is called at runtime,
    /// it will abort.
    fn emit_trap_stub(
        module: &mut ObjectModule,
        func_id: cranelift_module::FuncId,
        sig: &cranelift_codegen::ir::Signature,
        func_name: &str,
    ) -> Result<(), String> {
        use cranelift_codegen::control::ControlPlane;
        use cranelift_codegen::ir::{Function, TrapCode};
        use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

        let mut func = Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::default(),
            sig.clone(),
        );
        let mut fbc = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut fbc);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_all_blocks();
        builder.ins().trap(TrapCode::user(1).unwrap());
        builder.finalize();

        // Build a minimal ISA for compilation
        let mut fb = settings::builder();
        fb.set("opt_level", "none").unwrap();
        fb.set("is_pic", "true").unwrap();
        fb.set("use_colocated_libcalls", "true").unwrap();
        fb.set("enable_alias_analysis", "false").unwrap();
        let targeting_aarch64 = cfg!(target_arch = "aarch64");
        fb.set(
            "preserve_frame_pointers",
            if cfg!(debug_assertions) || targeting_aarch64 { "true" } else { "false" },
        ).unwrap();
        fb.set("enable_heap_access_spectre_mitigation", "false").unwrap();
        fb.set("enable_table_access_spectre_mitigation", "false").unwrap();
        fb.set(
            "probestack_strategy",
            if targeting_aarch64 { "outline" } else { "inline" },
        ).unwrap();
        let fallback_isa = native_isa_builder_with_options(true)
            .map_err(|e| format!("ISA builder: {e}"))?
            .finish(settings::Flags::new(fb))
            .map_err(|e| format!("ISA finish: {e}"))?;

        let mut ctx = Context::for_function(func);
        let mut ctrl = ControlPlane::default();
        ctx.compile(&*fallback_isa, &mut ctrl)
            .map_err(|e| format!("compile trap stub: {e:?}"))?;
        let compiled = ctx.compiled_code().unwrap();
        let alignment = compiled.buffer.alignment as u64;
        let code = compiled.buffer.data().to_vec();
        let relocs: Vec<cranelift_module::ModuleReloc> = compiled
            .buffer
            .relocs()
            .iter()
            .map(|r| {
                cranelift_module::ModuleReloc::from_mach_reloc(
                    r,
                    &ctx.func,
                    func_id,
                )
            })
            .collect();
        module
            .define_function_bytes(func_id, alignment, &code, &relocs)
            .map_err(|e| format!("define_function_bytes trap stub for {func_name}: {e}"))?;
        Ok(())
    }

    fn intern_data_segment(
        module: &mut ObjectModule,
        data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
        next_data_id: &mut u64,
        bytes: &[u8],
    ) -> cranelift_module::DataId {
        if let Some(existing) = data_pool.get(bytes) {
            return *existing;
        }
        let name = format!("data_pool_{}", *next_data_id);
        *next_data_id += 1;
        let data_id = module
            .declare_data(&name, Linkage::Local, false, false)
            .unwrap();
        let mut data_ctx = DataDescription::new();
        data_ctx.define(bytes.to_vec().into_boxed_slice());
        module.define_data(data_id, &data_ctx).unwrap();
        data_pool.insert(bytes.to_vec(), data_id);
        data_id
    }

    /// Walk backwards from `before_idx` to find a `"const"` op whose `out`
    /// matches `var_name` and return its integer value.  Used by the
    /// iter_next peephole to resolve constant index arguments.
    fn resolve_const_int(ops: &[OpIR], before_idx: usize, var_name: &str) -> Option<i64> {
        for i in (0..before_idx).rev() {
            let op = &ops[i];
            if op.kind == "const" {
                if let Some(ref out) = op.out {
                    if out == var_name {
                        return op.value;
                    }
                }
            }
        }
        None
    }

    #[cfg(test)]
    fn import_func_id(
        &mut self,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        let shape = ImportSignatureShape::from_types(params, returns);
        if let Some((func_id, cached_shape)) = self.import_ids.get(name) {
            assert_eq!(
                cached_shape, &shape,
                "import signature mismatch for {name}: {:?} vs {:?}",
                cached_shape, shape
            );
            return *func_id;
        }

        let mut sig = self.module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        for ret in returns {
            sig.returns.push(AbiParam::new(*ret));
        }
        let func_id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap();
        self.import_ids.insert(name, (func_id, shape));
        func_id
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let timing = env_setting("MOLT_BACKEND_TIMING")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        // Backend selection: MOLT_BACKEND=llvm routes through LLVM when the feature is
        // available; otherwise falls back to Cranelift with a warning.
        let _use_llvm = env_setting("MOLT_BACKEND").as_deref() == Some("llvm");
        #[cfg(not(feature = "llvm"))]
        if _use_llvm {
            eprintln!("[molt] WARNING: MOLT_BACKEND=llvm requested but llvm feature is not compiled in; falling back to Cranelift");
        }
        apply_profile_order(&mut ir);
        for func_ir in &mut ir.functions {
            elide_dead_struct_allocs(func_ir);
        }
        for func_ir in &mut ir.functions {
            escape_analysis(func_ir);
        }
        for func_ir in &mut ir.functions {
            if std::env::var("MOLT_DISABLE_RC_COALESCING").as_deref() != Ok("1") {
                rc_coalescing(func_ir);
            }
        }
        for func_ir in &mut ir.functions {
            fold_constants(&mut func_ir.ops);
            fold_constants_cross_block(&mut func_ir.ops);
        }
        for func_ir in &mut ir.functions {
            elide_safe_exception_checks(func_ir);
        }
        for func_ir in &mut ir.functions {
            hoist_loop_invariants(func_ir);
        }
        for func_ir in &mut ir.functions {
            propagate_loop_fast_int(func_ir);
        }
        // ── GPU kernel detection ──
        // Functions containing GPU intrinsic ops (gpu_thread_id, gpu_block_id,
        // etc.) are GPU kernels.  Flag them in metadata so the GPU pipeline can
        // handle them separately.  For now we log detection and skip TIR
        // optimization on these functions — the GPU pipeline handles lowering.
        let mut gpu_kernel_names: Vec<String> = Vec::new();
        for func_ir in &ir.functions {
            let is_gpu = func_ir.ops.iter().any(|op| {
                matches!(
                    op.kind.as_str(),
                    "gpu_thread_id"
                        | "gpu_block_id"
                        | "gpu_block_dim"
                        | "gpu_grid_dim"
                        | "gpu_barrier"
                )
            });
            if is_gpu {
                gpu_kernel_names.push(func_ir.name.clone());
            }
        }
        if !gpu_kernel_names.is_empty() {
            eprintln!(
                "[molt-gpu] Detected {} GPU kernel function(s): {:?}",
                gpu_kernel_names.len(),
                gpu_kernel_names
            );
        }

        // ── TIR optimization pipeline (default ON; set MOLT_TIR_OPT=0 to disable) ──
        if env_setting("MOLT_TIR_OPT").as_deref() == Some("1") {
            let tir_dump = env_setting("TIR_DUMP").as_deref() == Some("1");
            let tir_stats = env_setting("TIR_OPT_STATS").as_deref() == Some("1");
            let mut tir_cache = crate::tir::cache::CompilationCache::open(
                std::path::PathBuf::from(".molt-cache"),
            );
            for func_ir in &mut ir.functions {
                // GPU kernel functions are handled by the GPU pipeline — skip
                // normal TIR optimization to avoid lowering GPU intrinsic ops
                // that the standard passes do not understand.
                if gpu_kernel_names.contains(&func_ir.name) {
                    continue;
                }
                let func_name = func_ir.name.clone();
                let original_ops = func_ir.ops.clone(); // backup for fallback

                // Compute a stable content hash from the function name + a
                // serialization of its input ops. This is the cache key.
                let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
                let content_hash = crate::tir::cache::CompilationCache::compute_hash(
                    &func_ir.name,
                    &body_bytes,
                );

                // Cache hit: if we already have optimized ops for this function,
                // restore them directly and skip the TIR pipeline entirely.
                if let Some(cached_bytes) = tir_cache.get(&content_hash) {
                    if let Some(cached_ops) =
                        crate::tir::serialize::deserialize_ops(&cached_bytes)
                    {
                        func_ir.ops = cached_ops;
                        continue;
                    }
                }

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
                    crate::tir::type_refine::refine_types(&mut tir_func);
                    let type_map = crate::tir::type_refine::extract_type_map(&tir_func);
                    let stats = crate::tir::passes::run_pipeline(&mut tir_func);
                    if tir_dump {
                        eprintln!("{}", crate::tir::printer::print_function(&tir_func));
                    }
                    if tir_stats {
                        for s in &stats {
                            eprintln!(
                                "[TIR] {}: {} changed, {} removed, {} added",
                                s.name, s.values_changed, s.ops_removed, s.ops_added
                            );
                        }
                    }
                    crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map)
                }));

                match result {
                    Ok(optimized_ops) => {
                        // Validate: every label referenced by jump/br_if/check_exception
                        // must exist as a label op.  If not, fall back to original ops.
                        let valid = crate::tir::lower_to_simple::validate_labels(&optimized_ops);
                        if valid {
                            let serialized =
                                crate::tir::serialize::serialize_ops(&optimized_ops);
                            tir_cache.put(&content_hash, &serialized, vec![]);
                            func_ir.ops = optimized_ops;
                        } else {
                            eprintln!(
                                "[TIR] WARNING: label validation failed on function \'{}\' — falling back to unoptimized.",
                                func_name
                            );
                            func_ir.ops = original_ops;
                        }
                    }
                    Err(_panic) => {
                        // TIR failed on this function — fall back to unoptimized ops.
                        // Log the failure so it's visible, not silent.
                        eprintln!(
                            "[TIR] WARNING: optimization panicked on function '{}' — falling back to unoptimized. \
                             Set MOLT_TIR_OPT=0 to disable TIR, or report this bug.",
                            func_name
                        );
                        func_ir.ops = original_ops;
                    }
                }
            }
            // Persist the updated cache index so future runs benefit from
            // the newly stored entries.
            tir_cache.save_index();
        }
        // Post-TIR: analysis + inlining (from main)
        {
            let analysis = analyze_native_backend_ir(&ir);
            if analysis.needs_inlining && !self.skip_ir_passes {
                inline_functions(&mut ir);
            }
        }
        // Dead function elimination: remove functions that are unreachable from
        // the entry point after inlining.  This reduces code size for both the
        // native object and the downstream linker's work.
        if !self.skip_ir_passes { eliminate_dead_functions(&mut ir); }
        // Megafunction splitting: break up functions with >4000 ops (or
        // MOLT_MAX_FUNCTION_OPS) into private chunk functions to avoid
        // Cranelift's O(n²) register allocator blowup.
        split_megafunctions(&mut ir);
        // Replace __annotate__ functions with trivial ret_void stubs.
        // These are typing metadata that we don't need to compile fully,
        // but their symbols must exist for trampolines and def_function refs.
        // We keep the params so the Cranelift signature matches the declaration.
        for func_ir in ir.functions.iter_mut() {
            if func_ir.name.contains("__annotate__") {
                func_ir.ops.clear();
                func_ir.ops.push(crate::OpIR {
                    kind: "ret_void".to_string(),
                    ..crate::OpIR::default()
                });
            }
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }
        // Re-analyze after dead function elimination and megafunction
        // splitting so defined_functions/closure_functions reflect only the
        // surviving (and newly created chunk) functions.
        let ir_analysis = analyze_native_backend_ir(&ir);
        // Conditional trace elimination: skip emitting trace_enter/trace_exit calls
        // when tracing is disabled. Each guarded call site emits 2 trace function calls
        // (enter + exit); eliminating them saves codegen work on cache misses and
        // keeps the default native backend lane focused on production semantics.
        // Trace emission is opt-in via MOLT_BACKEND_EMIT_TRACES=1.
        let emit_traces = env_setting("MOLT_BACKEND_EMIT_TRACES")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        // Compile functions. For large modules (>128 functions), use the
        // Cranelift catch_unwind resilience path that retries failing
        // functions at opt_level=none.  The single-module approach is
        // retained (no batching) because Cranelift 0.130's ObjectModule
        // handles large function counts efficiently when individual
        // function compilations are bounded.
        let func_count = ir.functions.len();
        let total_ops: usize = ir.functions.iter().map(|f| f.ops.len()).sum();
        eprintln!("MOLT_BACKEND: compiling {func_count} functions ({total_ops} total ops)");
        let codegen_start = std::time::Instant::now();
        let mut compiled = 0u32;
        let failed = 0u32;
        let mut slowest_func: Option<(String, std::time::Duration)> = None;
        // Progress reporting: pick interval based on function count so the
        // user sees roughly 20 updates during a long build, but at least
        // every 50 functions.
        let progress_interval = (func_count / 20).max(1).min(50);
        let mut last_progress = std::time::Instant::now();
        for func_ir in ir.functions {
            let func_name = func_ir.name.clone();
            let func_start = std::time::Instant::now();
            self.compile_func(
                func_ir,
                &ir_analysis.task_kinds,
                &ir_analysis.task_closure_sizes,
                &ir_analysis.defined_functions,
                &ir_analysis.closure_functions,
                emit_traces,
            );
            let func_elapsed = func_start.elapsed();
            if timing && func_elapsed.as_millis() > 500 {
                eprintln!(
                    "MOLT_BACKEND_TIMING: function `{func_name}` took {func_elapsed:.2?}"
                );
            }
            if slowest_func
                .as_ref()
                .map_or(true, |(_, d)| func_elapsed > *d)
            {
                slowest_func = Some((func_name, func_elapsed));
            }
            compiled += 1;
            // Print progress at regular intervals, or every 500ms for
            // slow builds where individual functions take a long time.
            if compiled as usize % progress_interval == 0
                || last_progress.elapsed().as_millis() >= 500
            {
                let pct = (compiled as f64 / func_count as f64 * 100.0) as u32;
                let elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: [{pct:3}%] compiled {compiled}/{func_count} functions ({elapsed:.1?} elapsed)"
                );
                last_progress = std::time::Instant::now();
            }
        }
        if timing {
            let codegen_elapsed = codegen_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: Cranelift codegen took {codegen_elapsed:.2?}");
            if let Some((name, dur)) = &slowest_func {
                eprintln!("MOLT_BACKEND_TIMING: slowest function: `{name}` ({dur:.2?})");
            }
        }
        if failed > 0 {
            eprintln!("MOLT_BACKEND: {failed} functions failed, {compiled} succeeded");
        }
        // ── Post-compilation: define trap stubs for declared-but-undefined
        // functions.  This covers `__ov{N}` variants created when a function
        // is referenced with different arities, and functions that were skipped
        // due to signature mismatches or compilation failures.
        let mut stubs_emitted = 0u32;
        let declared: Vec<(cranelift_module::FuncId, String, cranelift_codegen::ir::Signature)> =
            self.module
                .declarations()
                .get_functions()
                .filter_map(|(fid, decl)| {
                    let name = decl.name.clone()?;
                    if decl.linkage == cranelift_module::Linkage::Export
                        && !self.defined_func_names.contains(&name)
                    {
                        Some((fid, name, decl.signature.clone()))
                    } else {
                        None
                    }
                })
                .collect();
        for (fid, name, sig) in declared {
            // In batched compilation, skip trap stubs for functions that
            // exist in other batches — ld -r will resolve them at merge
            // time.  But functions that don't exist in ANY batch (like
            // __ov variants or internally-generated names) still need
            // stubs to avoid Cranelift "Export must be defined" panics.
            if !self.external_function_names.is_empty()
                && self.external_function_names.contains(&name)
            {
                continue;
            }
            if let Err(e) = Self::emit_trap_stub(&mut self.module, fid, &sig, &name) {
                eprintln!("WARNING: failed to emit trap stub for `{}`: {}", name, e);
            } else {
                stubs_emitted += 1;
            }
        }
        if stubs_emitted > 0 {
            eprintln!(
                "WARNING: emitted {} trap stub(s) for declared-but-undefined functions",
                stubs_emitted
            );
        }

        let emit_start = std::time::Instant::now();
        let mut product = self.module.finish();
        // Set MachO platform load command so ld doesn't emit
        // "no platform load command found" warnings on macOS.
        #[cfg(target_os = "macos")]
        {
            use cranelift_object::object::write::MachOBuildVersion;
            // Encode macOS 11.0.0 as minimum deployment target.
            // Version encoding: xxxx.yy.zz nibbles => 0x000B0000 = 11.0.0
            let mut bv = MachOBuildVersion::default();
            bv.platform = cranelift_object::object::macho::PLATFORM_MACOS;
            bv.minos = 0x000B_0000; // macOS 11.0.0
            bv.sdk = 0;             // no SDK constraint
            product.object.set_macho_build_version(bv);
        }
        let bytes = product.emit().unwrap();
        if timing {
            let emit_elapsed = emit_start.elapsed();
            let total_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: object emit took {emit_elapsed:.2?}");
            eprintln!(
                "MOLT_BACKEND_TIMING: total backend compile: {total_elapsed:.2?} \
                 ({func_count} functions, {total_ops} ops, {} bytes)",
                bytes.len()
            );
        }
        bytes
    }

    fn ensure_trampoline(
        module: &mut ObjectModule,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        func_name: &str,
        linkage: Linkage,
        spec: TrampolineSpec,
    ) -> cranelift_module::FuncId {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
        } = spec;
        let is_import = matches!(linkage, Linkage::Import);
        let key = TrampolineKey {
            name: func_name.to_string(),
            arity,
            has_closure,
            is_import,
            kind,
            closure_size,
        };
        if let Some(id) = trampoline_ids.get(&key) {
            return *id;
        }
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let import_suffix = if is_import { "_import" } else { "" };
        let kind_suffix = match kind {
            TrampolineKind::Plain => "",
            TrampolineKind::Generator => "_gen",
            TrampolineKind::Coroutine => "_coro",
            TrampolineKind::AsyncGen => "_asyncgen",
        };
        let trampoline_name = format!(
            "{func_name}__molt_trampoline_{arity}{closure_suffix}{kind_suffix}{import_suffix}"
        );
        let mut ctx = module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let closure_bits = builder.block_params(entry_block)[0];
        let args_ptr = builder.block_params(entry_block)[1];
        let _args_len = builder.block_params(entry_block)[2];

        let poll_target = if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            if func_name.ends_with("_poll") {
                func_name.to_string()
            } else {
                format!("{func_name}_poll")
            }
        } else {
            String::new()
        };

        match kind {
            TrampolineKind::Generator => {
                if closure_size < 0 {
                    panic!("generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, linkage, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlags::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }
                builder.ins().return_(&[obj]);
            }
            TrampolineKind::Coroutine => {
                if closure_size < 0 {
                    panic!("coroutine closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("coroutine closure size too small for trampoline");
                }

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, linkage, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_COROUTINE);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                if payload_slots > 0 {
                    let mut inc_ref_obj_sig = module.make_signature();
                    inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                    let inc_ref_obj_callee = module
                        .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                        .unwrap();
                    let local_inc_ref_obj =
                        module.declare_func_in_func(inc_ref_obj_callee, builder.func);
                    let obj_ptr = unbox_ptr_value(&mut builder, obj);

                    let mut offset = 0i32;
                    if has_closure {
                        builder
                            .ins()
                            .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                        builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                        offset += 8;
                    }
                    for idx in 0..arity {
                        let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                        let arg_val = builder.ins().load(
                            types::I64,
                            MemFlags::trusted(),
                            args_ptr,
                            arg_offset,
                        );
                        builder.ins().store(
                            MemFlags::trusted(),
                            arg_val,
                            obj_ptr,
                            offset + arg_offset,
                        );
                        builder.ins().call(local_inc_ref_obj, &[arg_val]);
                    }
                }

                let mut get_sig = module.make_signature();
                get_sig.returns.push(AbiParam::new(types::I64));
                let get_callee = module
                    .declare_function("molt_cancel_token_get_current", Linkage::Import, &get_sig)
                    .unwrap();
                let get_local = module.declare_func_in_func(get_callee, builder.func);
                let get_call = builder.ins().call(get_local, &[]);
                let current_token = builder.inst_results(get_call)[0];

                let mut reg_sig = module.make_signature();
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.returns.push(AbiParam::new(types::I64));
                let reg_callee = module
                    .declare_function("molt_task_register_token_owned", Linkage::Import, &reg_sig)
                    .unwrap();
                let reg_local = module.declare_func_in_func(reg_callee, builder.func);
                builder.ins().call(reg_local, &[obj, current_token]);

                builder.ins().return_(&[obj]);
            }
            TrampolineKind::AsyncGen => {
                if closure_size < 0 {
                    panic!("async generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("async generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, linkage, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlags::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlags::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }

                let mut asyncgen_sig = module.make_signature();
                asyncgen_sig.params.push(AbiParam::new(types::I64));
                asyncgen_sig.returns.push(AbiParam::new(types::I64));
                let asyncgen_callee = module
                    .declare_function("molt_asyncgen_new", Linkage::Import, &asyncgen_sig)
                    .unwrap();
                let asyncgen_local = module.declare_func_in_func(asyncgen_callee, builder.func);
                let asyncgen_call = builder.ins().call(asyncgen_local, &[obj]);
                let asyncgen_obj = builder.inst_results(asyncgen_call)[0];
                builder.ins().return_(&[asyncgen_obj]);
            }
            TrampolineKind::Plain => {
                let mut call_args = Vec::with_capacity(arity + if has_closure { 1 } else { 0 });
                if has_closure {
                    call_args.push(closure_bits);
                }
                for idx in 0..arity {
                    let offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), args_ptr, offset);
                    call_args.push(arg_val);
                }

                let mut target_sig = module.make_signature();
                if has_closure {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                for _ in 0..arity {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                target_sig.returns.push(AbiParam::new(types::I64));
                let target_id = module
                    .declare_function(func_name, linkage, &target_sig)
                    .unwrap();
                let target_ref = module.declare_func_in_func(target_id, builder.func);
                let call = builder.ins().call(target_ref, &call_args);
                let res = builder.inst_results(call)[0];
                builder.ins().return_(&[res]);
            }
        }

        builder.seal_all_blocks();
        builder.finalize();

        let trampoline_id = module
            .declare_function(&trampoline_name, Linkage::Local, &ctx.func.signature)
            .unwrap();
        if let Err(err) = module.define_function(trampoline_id, &mut ctx) {
            panic!("Failed to define trampoline {trampoline_name}: {err:?}");
        }
        trampoline_ids.insert(key, trampoline_id);
        trampoline_id
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::{
        FunctionIR, OpIR, SimpleBackend, SimpleIR, TrampolineKind, analyze_native_backend_ir,
    };
    use cranelift_codegen::ir::types;
    use std::sync::{Mutex, OnceLock};

    fn backend_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn compile_trace_probe_object(emit_traces_env: Option<&str>) -> Vec<u8> {
        let _guard = backend_env_lock().lock().expect("env lock poisoned");
        match emit_traces_env {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_EMIT_TRACES", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") },
        }
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
            }],
            profile: None,
        };
        let bytes = SimpleBackend::new().compile(ir);
        unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") };
        bytes
    }

    #[test]
    fn native_backend_skips_trace_imports_by_default() {
        let bytes = compile_trace_probe_object(None);

        assert!(
            !bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            !bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_can_opt_in_trace_imports() {
        let bytes = compile_trace_probe_object(Some("1"));

        assert!(
            bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir);

        assert!(!analysis.needs_inlining);
        assert!(analysis.defined_functions.contains("molt_main"));
    }

    #[test]
    fn native_backend_ir_analysis_collects_task_metadata_once_needed() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        out: Some("flag".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("closure_size".to_string()),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new_closure".to_string(),
                        out: Some("poll_obj".to_string()),
                        s_value: Some("worker_poll".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_is_coroutine__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "flag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_closure_size__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "closure_size".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir);

        assert!(analysis.closure_functions.contains("worker_poll"));
        assert_eq!(
            analysis.task_kinds.get("worker_poll"),
            Some(&TrampolineKind::Coroutine)
        );
        assert_eq!(analysis.task_closure_sizes.get("worker_poll"), Some(&3));
    }

    #[test]
    fn native_backend_import_ids_are_cached_by_symbol() {
        let mut backend = SimpleBackend::new();

        let first = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);
        let second = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);

        assert_eq!(first, second);
        assert_eq!(backend.import_ids.len(), 1);
    }

    #[test]
    fn native_backend_skips_profile_store_imports_when_function_has_no_store_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(
            !bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            !bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    #[test]
    fn native_backend_keeps_profile_store_imports_when_function_has_store_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("obj".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("value".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store".to_string(),
                        args: Some(vec!["obj".to_string(), "value".to_string()]),
                        value: Some(8),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(
            bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    #[test]
    fn native_backend_compiles_exception_label_guard_if_without_else() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hello_regress____molt_globals_builtin__".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "exception_stack_enter".to_string(),
                        out: Some("v74".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_depth".to_string(),
                        out: Some("v75".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v76".to_string()),
                        s_value: Some("hello_regress".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        out: Some("v77".to_string()),
                        args: Some(vec!["v76".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v78".to_string()),
                        s_value: Some("__dict__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        out: Some("v79".to_string()),
                        args: Some(vec!["v77".to_string(), "v78".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v79".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_set_depth".to_string(),
                        args: Some(vec!["v75".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_exit".to_string(),
                        args: Some(vec!["v74".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("v80".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v81".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        out: Some("v82".to_string()),
                        args: Some(vec!["v80".to_string(), "v81".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "not".to_string(),
                        out: Some("v83".to_string()),
                        args: Some(vec!["v82".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["v83".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".to_string(),
                        args: Some(vec!["v80".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
            }],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir);

        assert!(!bytes.is_empty());
    }
}
