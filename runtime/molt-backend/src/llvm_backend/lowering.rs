//! Core TIR -> LLVM IR lowering.
//!
//! This module converts a `TirFunction` into an LLVM `FunctionValue` using
//! type-specialized emission: when operand types are statically known (e.g.
//! I64+I64), we emit native LLVM instructions; when types are dynamic
//! (DynBox), we emit calls to the Molt runtime.

#[cfg(feature = "llvm")]
use std::{cell::RefCell, collections::HashMap};

#[cfg(feature = "llvm")]
use inkwell::basic_block::BasicBlock;
#[cfg(feature = "llvm")]
use inkwell::types::BasicType;
#[cfg(feature = "llvm")]
use inkwell::values::{BasicValueEnum, FunctionValue, PhiValue};

#[cfg(feature = "llvm")]
use crate::llvm_backend::LlvmBackend;
#[cfg(feature = "llvm")]
use crate::llvm_backend::runtime_imports::{
    RuntimeReturnAbi, classified_runtime_import_return_abi, declare_conservative_runtime_function,
    is_classified_runtime_import,
};
#[cfg(feature = "llvm")]
use crate::llvm_backend::types::lower_type;
#[cfg(feature = "llvm")]
use inkwell::FloatPredicate;
#[cfg(feature = "llvm")]
use inkwell::attributes::AttributeLoc;

#[cfg(feature = "llvm")]
use crate::tir::blocks::{BlockId, Terminator};
#[cfg(feature = "llvm")]
use crate::tir::function::TirFunction;
#[cfg(feature = "llvm")]
use crate::tir::op_kinds_generated::opcode_uses_boxed_runtime_inplace_dispatch_table;
#[cfg(feature = "llvm")]
use crate::tir::ops::{AttrValue, OpCode, TirOp};
#[cfg(feature = "llvm")]
use crate::tir::types::TirType;
#[cfg(feature = "llvm")]
use crate::tir::values::ValueId;

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmLoweringError {
    diagnostics: Vec<String>,
}

#[cfg(feature = "llvm")]
impl LlvmLoweringError {
    fn new(diagnostics: Vec<String>) -> Self {
        Self { diagnostics }
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

#[cfg(feature = "llvm")]
impl std::fmt::Display for LlvmLoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "LLVM TIR lowering failed with {} diagnostic(s):",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            writeln!(f, "- {diagnostic}")?;
        }
        Ok(())
    }
}

#[cfg(feature = "llvm")]
impl std::error::Error for LlvmLoweringError {}

#[cfg(feature = "llvm")]
fn ensure_i64_with_builder<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    context: &'ctx inkwell::context::Context,
    val: BasicValueEnum<'ctx>,
) -> inkwell::values::IntValue<'ctx> {
    let i64_ty = context.i64_type();
    match val {
        BasicValueEnum::IntValue(iv) => {
            if iv.get_type().get_bit_width() == 64 {
                iv
            } else if iv.get_type().get_bit_width() < 64 {
                builder.build_int_z_extend(iv, i64_ty, "zext_i64").unwrap()
            } else {
                builder.build_int_truncate(iv, i64_ty, "trunc_i64").unwrap()
            }
        }
        BasicValueEnum::FloatValue(fv) => builder
            .build_bit_cast(fv, i64_ty, "f2i")
            .unwrap()
            .into_int_value(),
        BasicValueEnum::PointerValue(pv) => builder.build_ptr_to_int(pv, i64_ty, "ptr2i").unwrap(),
        _ => panic!("Cannot convert {:?} to i64", val),
    }
}

/// NaN-box a raw signed `i64`, promoting to a heap BigInt when the value does
/// not fit the 47-bit inline payload.
///
/// Shared, builder-parameterized implementation of the overflow-safe integer
/// box used by both the in-function lowering path
/// ([`FunctionLowering::box_i64_overflow_safe`]) and the trampoline / direct
/// call return-boxing path ([`materialize_dynbox_bits_with_builder`]). It emits
/// a single fits-inline range check; the inline (hot) path tags the 47-bit
/// payload, the cold path calls `molt_int_from_i64`. See the method wrapper for
/// the full rationale.
#[cfg(feature = "llvm")]
fn box_i64_overflow_safe_with_builder<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    context: &'ctx inkwell::context::Context,
    module: &inkwell::module::Module<'ctx>,
    current_fn: FunctionValue<'ctx>,
    raw: inkwell::values::IntValue<'ctx>,
) -> inkwell::values::IntValue<'ctx> {
    let i64_ty = context.i64_type();

    let bias = i64_ty.const_int(1u64 << 46, false);
    let biased = builder.build_int_add(raw, bias, "int_inline_bias").unwrap();
    let limit = i64_ty.const_int(1u64 << 47, false);
    let fits = builder
        .build_int_compare(inkwell::IntPredicate::ULT, biased, limit, "int_fits_inline")
        .unwrap();

    let inline_bb = context.append_basic_block(current_fn, "box_int_inline");
    let bigint_bb = context.append_basic_block(current_fn, "box_int_bigint");
    let merge_bb = context.append_basic_block(current_fn, "box_int_merge");
    builder
        .build_conditional_branch(fits, inline_bb, bigint_bb)
        .unwrap();

    builder.position_at_end(inline_bb);
    let masked = builder
        .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "mask")
        .unwrap();
    let inline_boxed = builder
        .build_or(
            masked,
            i64_ty.const_int(nanbox::QNAN | nanbox::TAG_INT, false),
            "box_i64",
        )
        .unwrap();
    builder.build_unconditional_branch(merge_bb).unwrap();

    builder.position_at_end(bigint_bb);
    let from_i64_fn = module.get_function("molt_int_from_i64").unwrap_or_else(|| {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        module.add_function(
            "molt_int_from_i64",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        )
    });
    let bigint_boxed = builder
        .build_call(from_i64_fn, &[raw.into()], "molt_int_from_i64")
        .unwrap()
        .try_as_basic_value()
        .unwrap_basic()
        .into_int_value();
    builder.build_unconditional_branch(merge_bb).unwrap();

    builder.position_at_end(merge_bb);
    let phi = builder.build_phi(i64_ty, "boxed_int").unwrap();
    phi.add_incoming(&[(&inline_boxed, inline_bb), (&bigint_boxed, bigint_bb)]);
    phi.as_basic_value().into_int_value()
}

#[cfg(feature = "llvm")]
fn materialize_dynbox_bits_with_builder<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    context: &'ctx inkwell::context::Context,
    module: &inkwell::module::Module<'ctx>,
    current_fn: FunctionValue<'ctx>,
    operand: BasicValueEnum<'ctx>,
    operand_ty: &TirType,
) -> inkwell::values::IntValue<'ctx> {
    let i64_ty = context.i64_type();
    match operand_ty {
        TirType::I64 => {
            let raw = ensure_i64_with_builder(builder, context, operand);
            box_i64_overflow_safe_with_builder(builder, context, module, current_fn, raw)
        }
        TirType::Bool => {
            let raw = match operand {
                BasicValueEnum::IntValue(iv) if iv.get_type().get_bit_width() == 1 => {
                    builder.build_int_z_extend(iv, i64_ty, "zext_bool").unwrap()
                }
                _ => ensure_i64_with_builder(builder, context, operand),
            };
            builder
                .build_or(
                    raw,
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_BOOL, false),
                    "box_bool",
                )
                .unwrap()
        }
        TirType::None => i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
        TirType::F64 => {
            let float_val = operand.into_float_value();
            let raw_bits = builder
                .build_bit_cast(float_val, i64_ty, "f64_to_i64")
                .unwrap()
                .into_int_value();
            let is_nan = builder
                .build_float_compare(FloatPredicate::UNO, float_val, float_val, "f64_is_nan")
                .unwrap();
            builder
                .build_select(
                    is_nan,
                    i64_ty.const_int(crate::CANONICAL_NAN_BITS, false),
                    raw_bits,
                    "f64_nan_canonical_bits",
                )
                .unwrap()
                .into_int_value()
        }
        TirType::DynBox
        | TirType::BigInt
        | TirType::Str
        | TirType::Bytes
        | TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Iterator(_)
        | TirType::Set(_)
        | TirType::Tuple(_)
        | TirType::UserClass(_)
        | TirType::Ptr(_)
        | TirType::Func(_)
        | TirType::Box(_)
        | TirType::Union(_)
        | TirType::Never => ensure_i64_with_builder(builder, context, operand),
    }
}

/// Unbox a NaN-boxed (`DynBox`) value into the raw machine representation a
/// function parameter typed `target_ty` expects. The builder-parameterized
/// twin of [`FunctionLowering::unbox_from_dynbox`] — same payload decode — used
/// by the trampoline, whose entry runs on a local builder, not
/// `self.backend.builder`.
///
/// The dynamic calling convention (`molt_call_func_fast2` → trampoline →
/// args-array) carries every argument as a NaN-boxed `i64`. A target function
/// whose parameters were typed by the representation plan as raw `I64`/`Bool`/
/// `F64` reads those parameters as raw machine values (the in-body lowering
/// re-boxes an `I64` param before any boxed-domain runtime call). Passing the
/// boxed bits straight through (as the old trampoline did) made the body decode
/// a NaN-box pointer/tag as a raw integer — silently truncating a heap BigInt to
/// garbage (the trusted-unbox bug-class), or misreading a bool/float. This is
/// the exact dual of the direct-call path's [`FunctionLowering::coerce_to_tir_type`]
/// arg coercion, applied at the dynamic-dispatch boundary instead.
#[cfg(feature = "llvm")]
fn unbox_dynbox_to_param_ty_with_builder<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    context: &'ctx inkwell::context::Context,
    raw: inkwell::values::IntValue<'ctx>,
    target_ty: &TirType,
) -> inkwell::values::IntValue<'ctx> {
    let i64_ty = context.i64_type();
    match target_ty {
        TirType::I64 => {
            // Sign-extend the 47-bit inline payload back into a full i64. Mirrors
            // `unbox_from_dynbox`'s `I64` arm exactly.
            let masked = builder
                .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "payload")
                .unwrap();
            let sign_test = builder
                .build_and(
                    masked,
                    i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                    "sign_test",
                )
                .unwrap();
            let is_neg = builder
                .build_int_compare(
                    inkwell::IntPredicate::NE,
                    sign_test,
                    i64_ty.const_zero(),
                    "is_neg",
                )
                .unwrap();
            let extended = builder
                .build_or(
                    masked,
                    i64_ty.const_int(!nanbox::INT_MASK, false),
                    "sign_extend",
                )
                .unwrap();
            builder
                .build_select(is_neg, extended, masked, "unbox_i64")
                .unwrap()
                .into_int_value()
        }
        TirType::Bool => builder
            .build_and(raw, i64_ty.const_int(1, false), "bool_payload")
            .unwrap(),
        // `F64` and every reference/boxed carrier are already in the raw i64 the
        // direct ABI expects (an `F64` param's LLVM type is i64-carried bits; the
        // body bitcasts as needed). No payload decode.
        _ => raw,
    }
}

// ── LLVM fast-math flag constants (from llvm-sys LLVMFastMath* definitions) ──
//
// AllowReassoc | NoNaNs | NoInfs | NoSignedZeros | AllowReciprocal
//             | AllowContract | ApproxFunc  (= "fast" in IR text)
#[cfg(feature = "llvm")]
const LLVM_FAST_MATH_ALL: u32 = (1 << 0)  // AllowReassoc
    | (1 << 1)  // NoNaNs
    | (1 << 2)  // NoInfs
    | (1 << 3)  // NoSignedZeros
    | (1 << 4)  // AllowReciprocal
    | (1 << 5)  // AllowContract
    | (1 << 6); // ApproxFunc

/// Return `true` when `op.attrs[key]` is `AttrValue::Bool(true)`.
#[cfg(feature = "llvm")]
fn has_attr(op: &TirOp, key: &str) -> bool {
    matches!(op.attrs.get(key), Some(AttrValue::Bool(true)))
}

/// NaN-boxing constants (mirrors molt-obj-model/src/lib.rs).
#[cfg(feature = "llvm")]
mod nanbox {
    pub const QNAN: u64 = 0x7ff8_0000_0000_0000;
    pub const TAG_INT: u64 = 0x0001_0000_0000_0000;
    pub const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
    pub const TAG_NONE: u64 = 0x0003_0000_0000_0000;
    pub const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
    pub const INT_SIGN_BIT: u64 = 1 << 46;
    pub const INT_MASK: u64 = (1u64 << 47) - 1;
}

/// Holds state during lowering of a single TIR function.
#[cfg(feature = "llvm")]
struct FunctionLowering<'ctx, 'func> {
    backend: &'func LlvmBackend<'ctx>,
    func: &'func TirFunction,
    llvm_fn: FunctionValue<'ctx>,
    /// When the TIR entry block has predecessors, LLVM gets a synthetic
    /// trampoline block as the true function entry. The original TIR entry
    /// then behaves like a normal phi block and must receive parameter
    /// incoming values from this trampoline.
    entry_trampoline_bb: Option<BasicBlock<'ctx>>,
    /// Maps TIR BlockId -> LLVM BasicBlock.
    block_map: HashMap<BlockId, BasicBlock<'ctx>>,
    /// Maps TIR ValueId -> lowered LLVM value.
    values: HashMap<ValueId, BasicValueEnum<'ctx>>,
    /// Maps TIR ValueId -> its TirType (for type-specialized dispatch).
    value_types: HashMap<ValueId, TirType>,
    /// Phi nodes that need incoming values wired up after all blocks are emitted.
    /// (target_block, arg_index, phi_node)
    pending_phis: Vec<(BlockId, usize, PhiValue<'ctx>)>,
    /// Actual TIR branch edges emitted into the LLVM CFG, including the LLVM
    /// predecessor block where the edge originates. Phi wiring must follow
    /// emitted edges, not all syntactic TIR terminators, because unreachable
    /// TIR blocks are not lowered as branches.
    phi_edges: Vec<PhiIncomingEdge<'ctx>>,
    /// PGO branch weights for this function, indexed by branch counter.
    /// Loaded from profdata when PGO mode is `Use`.
    /// Consumed sequentially: each CondBranch pops two values (true, false).
    pgo_branch_weights: Option<Vec<u64>>,
    /// Index into `pgo_branch_weights` — advanced by 2 for each CondBranch.
    pgo_weight_index: usize,
    /// Counter for unique global string constant names.
    const_str_counter: usize,
    /// Counter for synthetic block names introduced during lowering.
    synthetic_block_counter: usize,
    /// Synthetic or explicit resume blocks keyed by generator/coroutine state id.
    state_resume_blocks: HashMap<i64, BasicBlock<'ctx>>,
    /// All LLVM basic blocks created during lowering (including synthetic ones),
    /// used for the final unterminated-block sweep.
    all_llvm_blocks: Vec<BasicBlock<'ctx>>,
    /// Maps each LLVM basic block to its set of LLVM predecessor blocks.
    /// Built during lowering as branches are emitted, used by
    /// `patch_incomplete_phis` to detect missing phi predecessors.
    llvm_pred_map: HashMap<BasicBlock<'ctx>, Vec<BasicBlock<'ctx>>>,
    /// Structured exception-region stack baselines for preserved TryStart/TryEnd.
    /// Stored in entry-block allocas so later TryEnd sites do not violate LLVM
    /// dominance when the region spans multiple blocks.
    try_stack_baselines: Vec<inkwell::values::PointerValue<'ctx>>,
    /// Deterministic per-function call-site numbering for IC lanes.
    call_site_counter: usize,
    /// Fatal lowering diagnostics collected before exposing the LLVM function to
    /// verification, optimization, or emission.
    diagnostics: RefCell<Vec<String>>,
    /// Shared representation facts for this function, derived from the
    /// `ScalarRepresentationPlan`. Drives the overflow-safe integer-carrier and
    /// container dispatch decisions so the LLVM backend reads the same typed
    /// facts as the native/WASM/Luau backends instead of trusting `TirType`.
    repr_facts: crate::representation_plan::LlvmReprFacts,
}

#[cfg(feature = "llvm")]
#[derive(Clone)]
struct PhiIncomingEdge<'ctx> {
    source_block: BlockId,
    source_bb: BasicBlock<'ctx>,
    target: BlockId,
    edge_name: &'static str,
    args: Vec<ValueId>,
}

/// The closed set of vectorized-reduction op kinds emitted by the frontend's
/// `_match_vector_reduction_loop` (`VEC_SUM/PROD/MIN/MAX_*`, lower-cased by the
/// SimpleIR→TIR path). Each entry is `(op_kind, arity)` where `arity` is the
/// operand count and the runtime symbol is always `molt_<op_kind>`. This list
/// is the single LLVM-side authority for the family and MUST stay in lock-step
/// with the `molt_vec_*` runtime surface (`object/ops_vec.rs`) and the native
/// dispatch (`function_compiler.rs`). The arity split is structural: the
/// `_range` forms additionally pass the `start` bound (3 operands), while the
/// plain, `_trusted`, and `_range_iter` forms pass only `(seq, acc)`.
const VEC_REDUCTION_OPS: &[(&str, usize)] = &[
    ("vec_sum_int", 2),
    ("vec_sum_int_trusted", 2),
    ("vec_sum_int_range", 3),
    ("vec_sum_int_range_trusted", 3),
    ("vec_sum_int_range_iter", 2),
    ("vec_sum_int_range_iter_trusted", 2),
    ("vec_sum_float", 2),
    ("vec_sum_float_trusted", 2),
    ("vec_sum_float_range", 3),
    ("vec_sum_float_range_trusted", 3),
    ("vec_sum_float_range_iter", 2),
    ("vec_sum_float_range_iter_trusted", 2),
    ("vec_prod_int", 2),
    ("vec_prod_int_trusted", 2),
    ("vec_prod_int_range", 3),
    ("vec_prod_int_range_trusted", 3),
    ("vec_min_int", 2),
    ("vec_min_int_trusted", 2),
    ("vec_min_int_range", 3),
    ("vec_min_int_range_trusted", 3),
    ("vec_max_int", 2),
    ("vec_max_int_trusted", 2),
    ("vec_max_int_range", 3),
    ("vec_max_int_range_trusted", 3),
];

/// Returns the `molt_*` runtime symbol for a vectorized-reduction op kind, or
/// `None` if `kind` is not a member of the family. The returned symbol is a
/// `&'static str` so it can be passed straight to `ensure_runtime_i64_fn`.
fn vec_reduction_runtime_symbol(kind: &str) -> Option<&'static str> {
    VEC_REDUCTION_RUNTIME_SYMBOLS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, sym)| *sym)
}

/// Operand count for a vectorized-reduction op kind. Panics in debug builds if
/// `kind` is not a member of the family — callers must gate on
/// [`vec_reduction_runtime_symbol`] first.
fn vec_reduction_arity(kind: &str) -> usize {
    VEC_REDUCTION_OPS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, arity)| *arity)
        .expect("vec_reduction_arity called on non-vec kind")
}

/// Static `(kind, "molt_<kind>")` table derived from [`VEC_REDUCTION_OPS`].
/// Computed once at first use so the runtime symbols are leak-free `'static`
/// strings (the lowering needs `&'static str` for `ensure_runtime_i64_fn`).
static VEC_REDUCTION_RUNTIME_SYMBOLS: std::sync::LazyLock<Vec<(&'static str, &'static str)>> =
    std::sync::LazyLock::new(|| {
        VEC_REDUCTION_OPS
            .iter()
            .map(|(kind, _)| {
                let symbol: &'static str = Box::leak(format!("molt_{kind}").into_boxed_str());
                (*kind, symbol)
            })
            .collect()
    });

/// Preserved SimpleIR ops lowered by the LLVM generic runtime-call path whose
/// runtime ABI returns `void` rather than a boxed i64 sentinel. They are still
/// real side effects and must be claimed before the terminal Copy fail-loud
/// guard, but declaring them through `ensure_runtime_i64_fn` would give LLVM the
/// wrong C ABI. Each entry is `(kind, runtime_symbol, boxed_operand_arity)`.
const PRESERVED_VOID_RUNTIME_OPS: &[(&str, &str, usize)] = &[
    ("print_newline", "molt_print_newline", 0),
    ("spawn", "molt_spawn", 1),
];

fn preserved_void_runtime_call_abi(kind: &str) -> Option<(&'static str, usize)> {
    PRESERVED_VOID_RUNTIME_OPS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, symbol, arity)| (*symbol, *arity))
}

/// Lower a TIR function to LLVM IR.
///
/// Returns the LLVM function value. The function is added to `backend.module`.
///
/// When `pgo_branch_weights` is `Some`, the lowering attaches LLVM `!prof`
/// branch-weight metadata to conditional branches.  The weights are consumed
/// sequentially: each `CondBranch` terminator pops the next two values
/// (true_count, false_count) from the front of the vector.
#[cfg(feature = "llvm")]
pub fn lower_tir_to_llvm<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> FunctionValue<'ctx> {
    try_lower_tir_to_llvm(func, backend).unwrap_or_else(|err| panic!("{err}"))
}

/// Checked lowering entry point used by production compile paths.
#[cfg(feature = "llvm")]
pub fn try_lower_tir_to_llvm<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> Result<FunctionValue<'ctx>, LlvmLoweringError> {
    try_lower_tir_to_llvm_with_pgo(func, backend, None)
}

/// Like [`lower_tir_to_llvm`] but accepts optional PGO branch weights.
#[cfg(feature = "llvm")]
pub fn lower_tir_to_llvm_with_pgo<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
    pgo_branch_weights: Option<Vec<u64>>,
) -> FunctionValue<'ctx> {
    try_lower_tir_to_llvm_with_pgo(func, backend, pgo_branch_weights)
        .unwrap_or_else(|err| panic!("{err}"))
}

/// Like [`try_lower_tir_to_llvm`] but accepts optional PGO branch weights.
#[cfg(feature = "llvm")]
pub fn try_lower_tir_to_llvm_with_pgo<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
    pgo_branch_weights: Option<Vec<u64>>,
) -> Result<FunctionValue<'ctx>, LlvmLoweringError> {
    if !func.blocks.contains_key(&func.entry_block) {
        return Err(LlvmLoweringError::new(vec![format!(
            "{}: entry block {:?} is missing from TIR block map",
            func.name, func.entry_block
        )]));
    }

    // 1. Build or reuse the LLVM function signature.
    let llvm_fn = declare_tir_function(func, backend);
    let repr_facts = backend
        .function_repr_facts
        .get(&func.name)
        .cloned()
        .unwrap_or_default();
    let mut lowering = FunctionLowering {
        backend,
        func,
        llvm_fn,
        entry_trampoline_bb: None,
        block_map: HashMap::new(),
        values: HashMap::new(),
        value_types: HashMap::new(),
        pending_phis: Vec::new(),
        phi_edges: Vec::new(),
        pgo_branch_weights,
        pgo_weight_index: 0,
        const_str_counter: 0,
        synthetic_block_counter: 0,
        state_resume_blocks: HashMap::new(),
        all_llvm_blocks: Vec::new(),
        llvm_pred_map: HashMap::new(),
        try_stack_baselines: Vec::new(),
        call_site_counter: 0,
        diagnostics: RefCell::new(Vec::new()),
        repr_facts,
    };

    // 2. Create LLVM basic blocks for each TIR block.
    //    The entry block MUST be created first so that LLVM treats it as the
    //    function entry point. HashMap iteration order is non-deterministic,
    //    so we explicitly create the entry block before all others.
    {
        let entry_bb = backend
            .context
            .append_basic_block(llvm_fn, &format!("bb{}", func.entry_block.0));
        lowering.block_map.insert(func.entry_block, entry_bb);
        lowering.all_llvm_blocks.push(entry_bb);
    }
    for block_id in func.blocks.keys() {
        if *block_id == func.entry_block {
            continue; // already created above
        }
        let bb = backend
            .context
            .append_basic_block(llvm_fn, &format!("bb{}", block_id.0));
        lowering.block_map.insert(*block_id, bb);
        lowering.all_llvm_blocks.push(bb);
    }

    // 2b. LLVM requires the entry block to have no predecessors.  If any TIR
    //     block branches back to the entry, insert a trampoline block that
    //     becomes the real LLVM entry and immediately jumps to the TIR entry.
    {
        let entry_id = func.entry_block;
        let entry_has_predecessors = func.blocks.values().any(|blk| match &blk.terminator {
            Terminator::Branch { target, .. } => *target == entry_id,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == entry_id || *else_block == entry_id,
            Terminator::Switch { cases, default, .. }
            | Terminator::StateDispatch { cases, default, .. } => {
                *default == entry_id || cases.iter().any(|(_, t, _)| *t == entry_id)
            }
            _ => false,
        });
        if entry_has_predecessors {
            let old_entry_bb = lowering.block_map[&entry_id];
            let trampoline_bb = backend
                .context
                .prepend_basic_block(old_entry_bb, "entry_trampoline");
            lowering.entry_trampoline_bb = Some(trampoline_bb);
            lowering.all_llvm_blocks.push(trampoline_bb);
            lowering.record_llvm_edge(trampoline_bb, old_entry_bb);
            // The trampoline block jumps to the real entry.
            backend.builder.position_at_end(trampoline_bb);
            backend
                .builder
                .build_unconditional_branch(old_entry_bb)
                .unwrap();
        }
    }

    // 2c. Create synthetic resume blocks for stateful generator/coroutine ops.
    lowering.initialize_state_resume_blocks();

    // 3. Compute reverse-post-order (RPO) ordering of the CFG.
    //    RPO emits each block before its non-back-edge successors, which is
    //    the order LLVM's downstream passes (and our own phi finalization)
    //    expect: dominators precede dominatees, so each block sees its
    //    operand definitions already lowered.
    let rpo = lowering.compute_rpo();

    // 4. Lower each block.
    for block_id in &rpo {
        lowering.lower_block(*block_id);
    }

    // 5. Emit `unreachable` terminators for any LLVM basic blocks that lack
    //    a terminator instruction.  This covers:
    //    - TIR blocks not visited during RPO traversal (dead/unreachable)
    //    - Synthetic blocks created by CheckException or ScfIf that ended up
    //      without a terminator (e.g., all ops after the split were in the
    //      original block's op list but the block was split mid-stream)
    //    Without this, LLVM verification fails on "basic block does not have
    //    terminator" errors.
    {
        // First pass: dead TIR blocks.
        let rpo_set: std::collections::HashSet<BlockId> = rpo.iter().copied().collect();
        for (block_id, llvm_bb) in &lowering.block_map {
            if !rpo_set.contains(block_id) && llvm_bb.get_terminator().is_none() {
                backend.builder.position_at_end(*llvm_bb);
                backend.builder.build_unreachable().unwrap();
            }
        }
        // Second pass: ALL LLVM blocks (including synthetic).  Any block
        // without a terminator gets an `unreachable` instruction.
        let mut bb_opt = llvm_fn.get_first_basic_block();
        while let Some(bb) = bb_opt {
            if bb.get_terminator().is_none() {
                backend.builder.position_at_end(bb);
                backend.builder.build_unreachable().unwrap();
            }
            bb_opt = bb.get_next_basic_block();
        }
    }

    // 6. Wire up phi incoming values.
    lowering.finalize_phis();

    let diagnostics = lowering.diagnostics.borrow().clone();
    if !diagnostics.is_empty() {
        return Err(LlvmLoweringError::new(diagnostics));
    }

    // 7. If any op in this function carries `fast_math = true`, annotate the
    //    function with `"unsafe-fp-math"="true"`.  This is the function-level
    //    fallback for LLVM passes that inspect function attributes rather than
    //    per-instruction fast-math flags.
    let has_any_fast_math = func
        .blocks
        .values()
        .any(|blk| blk.ops.iter().any(|op| has_attr(op, "fast_math")));
    if has_any_fast_math {
        let attr = backend
            .context
            .create_string_attribute("unsafe-fp-math", "true");
        llvm_fn.add_attribute(AttributeLoc::Function, attr);
    }

    Ok(llvm_fn)
}

#[cfg(feature = "llvm")]
fn require_llvm_function_type<'ctx>(
    symbol: &str,
    existing: FunctionValue<'ctx>,
    expected: inkwell::types::FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    let actual = existing.get_type();
    let same_shape = actual.get_return_type() == expected.get_return_type()
        && actual.get_param_types() == expected.get_param_types()
        && actual.is_var_arg() == expected.is_var_arg();
    if !same_shape {
        panic!(
            "LLVM function type mismatch for `{symbol}`: expected {}, actual {}",
            expected.print_to_string(),
            actual.print_to_string()
        );
    }
    existing
}

#[cfg(feature = "llvm")]
pub fn declare_tir_function<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> FunctionValue<'ctx> {
    let param_llvm_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = func
        .param_types
        .iter()
        .map(|ty| lower_type(backend.context, ty).into())
        .collect();

    let ret_ty = lower_type(backend.context, &func.return_type);
    let fn_ty = ret_ty.fn_type(&param_llvm_types, false);
    // Reuse an existing forward-declaration if present (e.g., from a prior
    // Call op that referenced this function before it was defined).
    // If not, create a new function.  This avoids LLVM appending `.1` to
    // the name when a declaration already exists.

    let llvm_fn = if let Some(existing) = backend.module.get_function(&func.name) {
        let existing = require_llvm_function_type(&func.name, existing, fn_ty);
        // Verify it's just a declaration (no basic blocks yet).
        if existing.count_basic_blocks() == 0 {
            existing
        } else {
            panic!(
                "LLVM function `{}` already has a body before TIR definition",
                func.name
            );
        }
    } else {
        backend.module.add_function(&func.name, fn_ty, None)
    };
    // All molt-compiled functions use NaN-boxed error returns (never C++
    // exceptions) and always terminate. Mark them nounwind + willreturn
    // so LLVM can omit landing pads and perform aggressive code motion.
    let nounwind_kind = inkwell::attributes::Attribute::get_named_enum_kind_id("nounwind");
    llvm_fn.add_attribute(
        AttributeLoc::Function,
        backend.context.create_enum_attribute(nounwind_kind, 0),
    );
    let willreturn_kind = inkwell::attributes::Attribute::get_named_enum_kind_id("willreturn");
    llvm_fn.add_attribute(
        AttributeLoc::Function,
        backend.context.create_enum_attribute(willreturn_kind, 0),
    );
    llvm_fn
}

/// Append the successor block ids of `term` to `out`, preserving the order
/// in which they appear in the terminator (then-before-else for conditional
/// branches; case-list order followed by default for switches).
///
/// This is the single source of truth for "what does this terminator branch
/// to" within the LLVM lowering — both RPO traversal and any future analyses
/// route through here.
///
/// Public so integration tests (under `runtime/molt-backend/tests/`) can
/// exercise it without going through an inkwell context.
#[cfg(feature = "llvm")]
pub fn append_terminator_successors(term: &Terminator, out: &mut Vec<BlockId>) {
    match term {
        Terminator::Branch { target, .. } => out.push(*target),
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => {
            out.push(*then_block);
            out.push(*else_block);
        }
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            for (_, bid, _) in cases {
                out.push(*bid);
            }
            out.push(*default);
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
}

/// Compute a reverse-post-order (RPO) traversal of `func`'s CFG starting
/// from its entry block.
///
/// Algorithm: classic Cooper/Harvey/Kennedy iterative DFS post-order, then
/// reverse. We use an explicit work stack with two-phase entries (Enter then
/// Exit markers) so deeply nested or pathologically chained CFGs cannot
/// overflow the host call stack — a hard requirement for production-grade
/// codegen.
///
/// Properties of the result:
/// - The entry block is always first (a dominator of every reachable block).
/// - For any forward CFG edge `a -> b`, `a` precedes `b` in the result.
/// - Back-edges (loop latch -> header) are the only edges that go "backwards"
///   in the resulting order, which is exactly the layout LLVM expects: it
///   minimizes branch-backwards count for downstream code-layout passes.
/// - Unreachable blocks are not included; the lowering driver emits an
///   `unreachable` terminator for them in a separate sweep.
/// - Successor order within a terminator is preserved (then-before-else,
///   case-list-before-default), so the result is deterministic given a
///   deterministic CFG construction.
///
/// CFG edge set: the traversal follows BOTH terminator successors AND the
/// implicit exception-transfer edges of `CheckException`/`TryStart` ops (the
/// same `Full` edge set the TIR analyses use). `TryEnd` carries pairing
/// metadata but is not a handler transfer. An exception-handler block is
/// reachable *only* via a mid-block `CheckException` label edge — it never
/// appears in any terminator's successor list — so without the exception edges
/// the handler would be excluded from the RPO, never lowered, and stamped with
/// a bare `unreachable` by the pass-5 sweep. LLVM's SimplifyCFG would then fold
/// the `CheckException` arm's conditional branch into an `llvm.assume` that the
/// exception is never pending, silently skipping the handler at runtime. The
/// exception-edge extraction is delegated to the single source of truth in
/// [`crate::tir::dominators::exception_successors`] (not re-derived here).
///
/// Public so integration tests (under `runtime/molt-backend/tests/`) can
/// exercise it without going through an inkwell context.
#[cfg(feature = "llvm")]
pub fn compute_function_rpo(func: &TirFunction) -> Vec<BlockId> {
    use crate::tir::dominators::{exception_label_to_block, exception_successors};

    /// Work-stack frame: either `Enter(b)` (visit `b` and schedule its
    /// successors) or `Exit(b)` (record `b` in post-order — all successors
    /// have now been fully visited).
    enum Frame {
        Enter(BlockId),
        Exit(BlockId),
    }

    let entry = func.entry_block;
    if !func.blocks.contains_key(&entry) {
        // Malformed function with no entry block. Returning empty preserves
        // the contract that callers see only blocks present in the CFG.
        return Vec::new();
    }

    // Resolve each handler label id to its owning block once; reused for every
    // block's exception-successor lookup below.
    let label_to_block = exception_label_to_block(func);

    let mut visited: std::collections::HashSet<BlockId> =
        std::collections::HashSet::with_capacity(func.blocks.len());
    let mut post_order: Vec<BlockId> = Vec::with_capacity(func.blocks.len());
    let mut stack: Vec<Frame> = Vec::with_capacity(func.blocks.len());
    let mut succ_buf: Vec<BlockId> = Vec::new();

    stack.push(Frame::Enter(entry));

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter(b) => {
                if !visited.insert(b) {
                    continue;
                }
                let Some(block) = func.blocks.get(&b) else {
                    // Terminator references a block that was deleted from
                    // the CFG. Skip rather than panic — the lowering driver
                    // will emit an unreachable terminator for any LLVM block
                    // that lacks one.
                    continue;
                };

                // Schedule the post-order Exit for this block first; it will
                // run after all successors (and their transitive successors)
                // have been fully visited.
                stack.push(Frame::Exit(b));

                // Collect successors: terminator edges first (preserving
                // then-before-else / case-before-default order), then the
                // implicit exception-handler edges. Handler blocks are placed
                // *after* the normal successors so the common fall-through path
                // keeps its natural layout, mirroring the runtime expectation
                // that the exceptional path is the unlikely one.
                succ_buf.clear();
                append_terminator_successors(&block.terminator, &mut succ_buf);
                succ_buf.extend(exception_successors(block, &label_to_block));

                // Push successors in reverse so the *first* successor is
                // popped (and thus visited) first. This makes the recursion
                // order match the natural left-to-right successor order
                // (terminator successors, then handler successors).
                for succ in succ_buf.iter().rev() {
                    if !visited.contains(succ) {
                        stack.push(Frame::Enter(*succ));
                    }
                }
            }
            Frame::Exit(b) => {
                post_order.push(b);
            }
        }
    }

    post_order.reverse();
    post_order
}

#[cfg(feature = "llvm")]
impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    fn record_fatal(&self, message: impl Into<String>) {
        self.diagnostics
            .borrow_mut()
            .push(format!("{}: {}", self.func.name, message.into()));
    }

    /// Record that `from_bb` branches to `to_bb` at the LLVM level.
    /// Used by `patch_incomplete_phis` to find missing phi predecessors.
    fn record_llvm_edge(&mut self, from_bb: BasicBlock<'ctx>, to_bb: BasicBlock<'ctx>) {
        self.llvm_pred_map.entry(to_bb).or_default().push(from_bb);
    }

    /// Compute a reverse-post-order (RPO) traversal of the CFG starting from
    /// the function's entry block.
    ///
    /// Delegates to the pure free function [`compute_function_rpo`] so the
    /// algorithm can be unit-tested without an inkwell context.
    fn compute_rpo(&self) -> Vec<BlockId> {
        compute_function_rpo(self.func)
    }

    fn lower_block(&mut self, block_id: BlockId) {
        let block = self.func.blocks.get(&block_id).unwrap().clone();
        let bb = self.block_map[&block_id];
        self.backend.builder.position_at_end(bb);

        // Entry block: map block args to function parameters.
        if block_id == self.func.entry_block && self.entry_trampoline_bb.is_none() {
            for (i, arg) in block.args.iter().enumerate() {
                let value = self.llvm_fn.get_nth_param(i as u32).unwrap_or_else(|| {
                    self.record_fatal(format!(
                        "entry block argument %{} at index {} has no corresponding function parameter",
                        arg.id.0, i
                    ));
                    self.get_undef_for_type(lower_type(self.backend.context, &arg.ty))
                });
                self.values.insert(arg.id, value);
                // An `int` parameter the plan does NOT prove overflow-safe is
                // carried BOXED (`DynBox`), exactly as a non-entry phi is (see
                // the `else` arm). The calling convention passes every argument
                // NaN-boxed; an unprovable-range int param therefore receives a
                // boxed value (an inline int OR a heap-BigInt pointer) and the
                // body must treat it as `DynBox` — using it directly in the
                // boxed-runtime arithmetic path. Declaring it raw `I64` instead
                // made the body decode the boxed bits as a raw integer and
                // re-box them, silently truncating a heap BigInt to its low 47
                // bits (the trusted-unbox bug-class, at the parameter ABI). Only
                // a value-range-proven param stays raw `I64`; for it the
                // caller/trampoline unboxes the inline payload, which is sound
                // because the range proof guarantees it fits. The LLVM parameter
                // type is i64 either way, so this changes only the semantic
                // carrier the body reasons about — matching the native lane,
                // whose `int` params are likewise boxed words.
                let effective_ty = self.effective_block_arg_type(arg.id, &arg.ty);
                self.value_types.insert(arg.id, effective_ty);
            }
        } else {
            // Non-entry blocks: create phi nodes for block arguments.
            for (i, arg) in block.args.iter().enumerate() {
                // A loop-carried integer that the plan does not prove
                // overflow-safe must travel boxed (DynBox), not as a raw i64
                // phi. Otherwise a runtime BigInt result flowing back along the
                // loop edge would be unboxed into a truncating 47-bit payload.
                // I64 and DynBox both lower to the i64 machine type, so this only
                // changes the *semantic* carrier; the incoming-edge coercion
                // boxes the raw-i64 init value to keep the bits consistent.
                let effective_ty = self.effective_block_arg_type(arg.id, &arg.ty);
                let llvm_ty = lower_type(self.backend.context, &effective_ty);
                let phi = self
                    .backend
                    .builder
                    .build_phi(llvm_ty, &format!("phi_{}", arg.id.0))
                    .unwrap();
                self.values.insert(arg.id, phi.as_basic_value());
                self.value_types.insert(arg.id, effective_ty);
                self.pending_phis.push((block_id, i, phi));
            }
        }

        // Lower each operation.
        for op in &block.ops {
            self.lower_op(block_id, op);
            let terminated = self
                .backend
                .builder
                .get_insert_block()
                .and_then(|bb| bb.get_terminator())
                .is_some();
            if terminated {
                break;
            }
        }

        // Lower terminator.
        let current_terminated = self
            .backend
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        if !current_terminated {
            self.lower_terminator(block_id, &block.terminator);
        }
    }

    fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // ── Constants ──
            OpCode::ConstInt => {
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Int(v)) => *v,
                    other => panic!("ConstInt missing integer value attribute: {:?}", other),
                };
                let result_id = op.results[0];
                let llvm_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(val as u64, val < 0)
                    .into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::I64);
            }
            OpCode::ConstFloat => {
                let val = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
                    Some(AttrValue::Float(v)) => *v,
                    other => panic!("ConstFloat missing float value attribute: {:?}", other),
                };
                let result_id = op.results[0];
                let llvm_val = self.backend.context.f64_type().const_float(val).into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::F64);
            }
            OpCode::ConstBool => {
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Bool(v)) => *v,
                    Some(AttrValue::Int(v)) => *v != 0,
                    other => panic!("ConstBool missing bool value attribute: {:?}", other),
                };
                let result_id = op.results[0];
                let llvm_val = self
                    .backend
                    .context
                    .bool_type()
                    .const_int(val as u64, false)
                    .into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::Bool);
            }
            OpCode::ConstNone => {
                let result_id = op.results[0];
                // NaN-boxed None sentinel: QNAN | TAG_NONE
                let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                let llvm_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(none_bits, false)
                    .into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::None);
            }
            OpCode::ConstStr => {
                let result_id = op.results[0];
                let i64_ty = self.backend.context.i64_type();

                // Extract the string bytes from attrs.
                let str_bytes: Vec<u8> = if let Some(AttrValue::Bytes(b)) = op.attrs.get("bytes") {
                    b.clone()
                } else if let Some(AttrValue::Str(s)) = op.attrs.get("s_value") {
                    s.as_bytes().to_vec()
                } else {
                    Vec::new()
                };

                // Create a global constant for the string data.
                let byte_array_ty = self
                    .backend
                    .context
                    .i8_type()
                    .array_type(str_bytes.len() as u32);
                let global = self.backend.module.add_global(
                    byte_array_ty,
                    None,
                    &format!("__const_str_{}", self.const_str_counter),
                );
                self.const_str_counter += 1;
                global.set_linkage(inkwell::module::Linkage::Private);
                global.set_initializer(&self.backend.context.const_string(&str_bytes, false));
                global.set_constant(true);
                global.set_unnamed_addr(true);

                // Get or declare molt_string_from_bytes.
                let sfb_fn =
                    if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
                        f
                    } else {
                        let ptr_ty = self
                            .backend
                            .context
                            .ptr_type(inkwell::AddressSpace::default());
                        let i32_ty = self.backend.context.i32_type();
                        let fn_ty =
                            i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
                        self.backend.module.add_function(
                            "molt_string_from_bytes",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    };

                // Allocate a stack slot for the output u64.
                let out_alloca = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "str_out")
                    .unwrap();

                // Call molt_string_from_bytes(ptr, len, out).
                let ptr_val = global.as_pointer_value();
                let len_val = i64_ty.const_int(str_bytes.len() as u64, false);
                self.backend
                    .builder
                    .build_call(
                        sfb_fn,
                        &[ptr_val.into(), len_val.into(), out_alloca.into()],
                        "sfb",
                    )
                    .unwrap();

                // Load the result from the output slot.
                let result = self
                    .backend
                    .builder
                    .build_load(i64_ty, out_alloca, "str_bits")
                    .unwrap();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::Str);
            }
            OpCode::ConstBigInt => {
                // Arbitrary-precision int constant: the decimal text lives in
                // s_value and is materialized through
                // `molt_bigint_from_str(ptr, len) -> bits`. The result is a
                // BOXED value (inline int or heap BigInt) — DynBox carrier,
                // never raw I64. A missing arm here is a silent miscompile:
                // the fallback `Copy` lowering left the result undefined and
                // it resolved to the None sentinel.
                let result_id = op.results[0];
                let i64_ty = self.backend.context.i64_type();

                let digits: Vec<u8> = match op.attrs.get("s_value") {
                    Some(AttrValue::Str(s)) => s.as_bytes().to_vec(),
                    other => panic!("ConstBigInt missing s_value attribute: {:?}", other),
                };

                let byte_array_ty = self
                    .backend
                    .context
                    .i8_type()
                    .array_type(digits.len() as u32);
                let global = self.backend.module.add_global(
                    byte_array_ty,
                    None,
                    &format!("__const_bigint_{}", self.const_str_counter),
                );
                self.const_str_counter += 1;
                global.set_linkage(inkwell::module::Linkage::Private);
                global.set_initializer(&self.backend.context.const_string(&digits, false));
                global.set_constant(true);
                global.set_unnamed_addr(true);

                let ptr_ty = self
                    .backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default());
                let bigint_from_str_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
                let bfs_fn =
                    if let Some(f) = self.backend.module.get_function("molt_bigint_from_str") {
                        require_llvm_function_type("molt_bigint_from_str", f, bigint_from_str_ty)
                    } else {
                        declare_conservative_runtime_function(
                            self.backend.context,
                            &self.backend.module,
                            "molt_bigint_from_str",
                            bigint_from_str_ty,
                        )
                    };

                let ptr_val = global.as_pointer_value();
                let len_val = i64_ty.const_int(digits.len() as u64, false);
                let call = self
                    .backend
                    .builder
                    .build_call(bfs_fn, &[ptr_val.into(), len_val.into()], "bigint_bits")
                    .unwrap();
                let result = call
                    .try_as_basic_value()
                    .basic()
                    .expect("molt_bigint_from_str returns i64 bits");
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::ConstBytes => {
                let result_id = op.results[0];
                let i64_ty = self.backend.context.i64_type();

                // Extract the raw bytes from attrs.
                let raw_bytes: Vec<u8> = if let Some(AttrValue::Bytes(b)) = op.attrs.get("bytes") {
                    b.clone()
                } else if let Some(AttrValue::Str(s)) = op.attrs.get("s_value") {
                    s.as_bytes().to_vec()
                } else {
                    Vec::new()
                };

                // Create a global constant for the bytes data.
                let byte_array_ty = self
                    .backend
                    .context
                    .i8_type()
                    .array_type(raw_bytes.len() as u32);
                let global = self.backend.module.add_global(
                    byte_array_ty,
                    None,
                    &format!("__const_bytes_{}", self.const_str_counter),
                );
                self.const_str_counter += 1;
                global.set_linkage(inkwell::module::Linkage::Private);
                global.set_initializer(&self.backend.context.const_string(&raw_bytes, false));
                global.set_constant(true);
                global.set_unnamed_addr(true);

                // Get or declare molt_string_from_bytes (used for bytes too — the
                // runtime creates a bytes object when the caller context is ConstBytes).
                let sfb_fn =
                    if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
                        f
                    } else {
                        let ptr_ty = self
                            .backend
                            .context
                            .ptr_type(inkwell::AddressSpace::default());
                        let i32_ty = self.backend.context.i32_type();
                        let fn_ty =
                            i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
                        self.backend.module.add_function(
                            "molt_string_from_bytes",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    };

                // Allocate a stack slot for the output u64.
                let out_alloca = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "bytes_out")
                    .unwrap();

                // Call molt_string_from_bytes(ptr, len, out).
                let ptr_val = global.as_pointer_value();
                let len_val = i64_ty.const_int(raw_bytes.len() as u64, false);
                self.backend
                    .builder
                    .build_call(
                        sfb_fn,
                        &[ptr_val.into(), len_val.into(), out_alloca.into()],
                        "bfb",
                    )
                    .unwrap();

                // Load the result from the output slot.
                let result = self
                    .backend
                    .builder
                    .build_load(i64_ty, out_alloca, "bytes_bits")
                    .unwrap();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── Arithmetic (type-specialized) ──
            OpCode::Add | OpCode::InplaceAdd => self.emit_binary_arith(op, "add"),
            OpCode::CheckedAdd => self.emit_checked_add(op),
            OpCode::CheckedMul => self.emit_checked_mul(op),
            OpCode::Sub | OpCode::InplaceSub => self.emit_binary_arith(op, "sub"),
            OpCode::Mul | OpCode::InplaceMul => self.emit_binary_arith(op, "mul"),
            OpCode::Div => self.emit_binary_arith(op, "div"),
            OpCode::FloorDiv => self.emit_binary_arith(op, "floordiv"),
            OpCode::Mod => self.emit_binary_arith(op, "mod"),
            OpCode::Pow => self.emit_binary_arith(op, "pow"),

            // ── Unary ──
            OpCode::Neg => self.emit_unary(op, "neg"),
            OpCode::Pos => {
                // Pos is identity for numeric types.
                let result_id = op.results[0];
                let operand = op.operands[0];
                let val = self.values[&operand];
                let ty = self.value_types[&operand].clone();
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, ty);
            }
            OpCode::Not => self.emit_unary(op, "not"),

            // ── Comparison (type-specialized) ──
            OpCode::Eq => self.emit_comparison(op, "eq"),
            OpCode::Ne => self.emit_comparison(op, "ne"),
            OpCode::Lt => self.emit_comparison(op, "lt"),
            OpCode::Le => self.emit_comparison(op, "le"),
            OpCode::Gt => self.emit_comparison(op, "gt"),
            OpCode::Ge => self.emit_comparison(op, "ge"),
            OpCode::Is | OpCode::IsNot => self.emit_identity(op),
            OpCode::In | OpCode::NotIn => self.emit_containment(op),

            // ── Bitwise ──
            OpCode::BitAnd => self.emit_bitwise(op, "bit_and"),
            OpCode::BitOr => self.emit_bitwise(op, "bit_or"),
            OpCode::BitXor => self.emit_bitwise(op, "bit_xor"),
            OpCode::BitNot => self.emit_unary(op, "invert"),
            OpCode::Shl => self.emit_bitwise(op, "lshift"),
            OpCode::Shr => self.emit_bitwise(op, "rshift"),

            // ── Boolean ──
            OpCode::And | OpCode::Or => {
                // Frontend BoolOp lowering uses And/Or ops to produce the
                // selected operand value inside already-structured control flow.
                // At this stage we must preserve Python operand-selection
                // semantics, not bitwise semantics.
                let result_id = op.results[0];
                let lhs = self.resolve(op.operands[0]);
                let rhs = self.resolve(op.operands[1]);
                let lhs_ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let rhs_ty = self
                    .value_types
                    .get(&op.operands[1])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let lhs_i64 = self.ensure_i64(lhs);
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[lhs_i64.into()], "truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy,
                        self.backend.context.i64_type().const_zero(),
                        "boolop_cond",
                    )
                    .unwrap();
                let lhs_bits = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_bits = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let selected = if op.opcode == OpCode::And {
                    self.backend
                        .builder
                        .build_select(cond_i1, rhs_bits, lhs_bits, "bool_and")
                        .unwrap()
                } else {
                    self.backend
                        .builder
                        .build_select(cond_i1, lhs_bits, rhs_bits, "bool_or")
                        .unwrap()
                };
                if crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
                    op.opcode,
                ) {
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    self.backend
                        .builder
                        .build_call(
                            inc_fn,
                            &[selected.into_int_value().into()],
                            "boolop_selected_inc_ref",
                        )
                        .unwrap();
                }
                self.values.insert(result_id, selected);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::Bool => {
                let result_id = op.results[0];
                let operand_id = op.operands[0];
                let operand = self.resolve(operand_id);
                let operand_ty = self
                    .value_types
                    .get(&operand_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);

                let bool_val = match operand_ty {
                    TirType::Bool => operand.into_int_value(),
                    TirType::I64 => self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            operand.into_int_value(),
                            self.backend.context.i64_type().const_zero(),
                            "bool_i64",
                        )
                        .unwrap(),
                    TirType::F64 => self
                        .backend
                        .builder
                        .build_float_compare(
                            inkwell::FloatPredicate::ONE,
                            operand.into_float_value(),
                            self.backend.context.f64_type().const_float(0.0),
                            "bool_f64",
                        )
                        .unwrap(),
                    _ => {
                        let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                        let truthy = self
                            .backend
                            .builder
                            .build_call(
                                truthy_fn,
                                &[self.ensure_i64(operand).into()],
                                "truthy_bool",
                            )
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic()
                            .into_int_value();
                        self.backend
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                truthy,
                                self.backend.context.i64_type().const_zero(),
                                "bool_dynbox",
                            )
                            .unwrap()
                    }
                };
                self.values.insert(result_id, bool_val.into());
                self.value_types.insert(result_id, TirType::Bool);
            }

            // ── Box/Unbox ──
            OpCode::BoxVal => self.emit_box(op),
            OpCode::UnboxVal => self.emit_unbox(op),
            OpCode::TypeGuard => {
                // Type guard: in lowered code, this is a no-op assertion.
                // The value passes through; if the guard fails at runtime,
                // deopt kicks in (handled elsewhere).
                let result_id = op.results[0];
                let val = self.resolve(op.operands[0]);
                self.values.insert(result_id, val);
                let ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                self.value_types.insert(result_id, ty);
            }

            // ── Refcount ──
            OpCode::IncRef => {
                let val = self.resolve(op.operands[0]);
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(inc_fn, &[bits.into()], "")
                    .unwrap();
                // IncRef has no result, but if it does, pass through.
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    self.value_types.insert(op.results[0], ty);
                }
            }
            // Python lifetime boundary (#58): when a `del`/rebind/scope-exit
            // marker survives to LLVM, it is the release authority for that
            // named-local owner. The drop phase may rewrite some markers to
            // `DecRef`; any marker left here must still lower to the same
            // runtime release, matching the native backend's direct
            // `del_boundary` arm.
            OpCode::DelBoundary => {
                let val = self.resolve(op.operands[0]);
                let bits = self.ensure_i64(val);
                let dec_fn = self.ensure_runtime_void_fn("molt_dec_ref_obj", 1);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }
            OpCode::DecRef => {
                let val = self.resolve(op.operands[0]);
                let dec_fn = self.ensure_runtime_void_fn("molt_dec_ref_obj", 1);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }

            // ── Memory / Attribute / Index ──
            OpCode::LoadAttr => {
                let result_id = op.results[0];
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if matches!(original_kind, Some("get_attr_name")) && op.operands.len() >= 2 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let name_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_name", 2);
                    let val = self
                        .backend
                        .builder
                        .build_call(
                            get_fn,
                            &[obj_bits.into(), name_bits.into()],
                            "get_attr_name_dyn",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    self.values.insert(result_id, val);
                    self.value_types.insert(result_id, TirType::DynBox);
                    return;
                }
                if matches!(original_kind, Some("load")) && !op.operands.is_empty() {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let offset = op
                        .attrs
                        .get("value")
                        .and_then(|v| match v {
                            AttrValue::Int(v) => Some(*v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                    // Inline the field load: convert ptr to pointer type,
                    // GEP by byte offset, load i64, then inc_ref.
                    // This eliminates the runtime call (GIL + debug checks).
                    let i64_ty = self.backend.context.i64_type();
                    let i8_ty = self.backend.context.i8_type();
                    let ptr_ty = self
                        .backend
                        .context
                        .ptr_type(inkwell::AddressSpace::default());
                    let raw_ptr = self
                        .backend
                        .builder
                        .build_int_to_ptr(obj_ptr_bits, ptr_ty, "obj_ptr")
                        .unwrap();
                    let offset_val = i64_ty.const_int(offset as u64, true);
                    let field_ptr = unsafe {
                        self.backend
                            .builder
                            .build_in_bounds_gep(i8_ty, raw_ptr, &[offset_val], "field_ptr")
                            .unwrap()
                    };
                    let val = self
                        .backend
                        .builder
                        .build_load(i64_ty, field_ptr, "field_val")
                        .unwrap();
                    // inc_ref the loaded value (may be a heap pointer).
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    self.backend
                        .builder
                        .build_call(inc_fn, &[val.into()], "field_load_inc_ref")
                        .unwrap();
                    self.values.insert(result_id, val);
                    self.value_types.insert(result_id, TirType::DynBox);
                    return;
                }
                if matches!(original_kind, Some("guarded_field_get")) && op.operands.len() >= 3 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let expected_version = self.materialize_dynbox_operand(op.operands[2]);
                    let attr_name = op
                        .attrs
                        .get("name")
                        .and_then(|v| {
                            if let AttrValue::Str(s) = v {
                                Some(s.as_str())
                            } else {
                                None
                            }
                        })
                        .unwrap_or("<unknown>");
                    let offset = op
                        .attrs
                        .get("value")
                        .and_then(|v| match v {
                            AttrValue::Int(v) => Some(*v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                    let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
                    let get_fn = self.ensure_runtime_i64_fn("molt_guarded_field_get_ptr", 6);
                    let val = self
                        .backend
                        .builder
                        .build_call(
                            get_fn,
                            &[
                                obj_ptr_bits.into(),
                                class_bits.into(),
                                expected_version.into(),
                                self.backend
                                    .context
                                    .i64_type()
                                    .const_int(offset as u64, true)
                                    .into(),
                                attr_ptr_bits.into(),
                                attr_len_bits.into(),
                            ],
                            "guarded_field_get",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    self.values.insert(result_id, val);
                    self.value_types.insert(result_id, TirType::DynBox);
                    return;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                // Attribute name is stored in attrs["name"], not as a second operand.
                let attr_name = op
                    .attrs
                    .get("name")
                    .and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("<unknown>");
                let runtime_name = if matches!(original_kind, Some("get_attr_generic_obj")) {
                    "molt_get_attr_object_ic"
                } else {
                    "molt_get_attr_name"
                };
                let get_fn = if runtime_name == "molt_get_attr_object_ic" {
                    self.ensure_runtime_i64_fn(runtime_name, 4)
                } else {
                    self.ensure_runtime_i64_fn(runtime_name, 2)
                };
                let name = self.intern_string_const(attr_name);
                let name_bits = self.ensure_i64(name);
                let site_bits = self.next_call_site_bits("get_attr_generic_obj");
                let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
                let call_args_generic = [
                    obj_bits.into(),
                    attr_ptr_bits.into(),
                    attr_len_bits.into(),
                    site_bits.into(),
                ];
                let call_args_name = [obj_bits.into(), name_bits.into()];
                let val = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        if runtime_name == "molt_get_attr_object_ic" {
                            &call_args_generic
                        } else {
                            &call_args_name
                        },
                        runtime_name,
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if runtime_name == "molt_get_attr_object_ic" {
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    let _ = self
                        .backend
                        .builder
                        .build_call(
                            inc_fn,
                            &[val.into_int_value().into()],
                            "get_attr_object_ic_inc_ref",
                        )
                        .unwrap();
                }
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StoreAttr => {
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if matches!(original_kind, Some("set_attr_name")) && op.operands.len() >= 3 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let name_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                    let set_fn = self.ensure_runtime_i64_fn("molt_set_attr_name", 3);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            set_fn,
                            &[obj_bits.into(), name_bits.into(), val_bits.into()],
                            "set_attr_name_dyn",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if !op.results.is_empty() {
                        self.values.insert(op.results[0], result);
                        self.value_types.insert(op.results[0], TirType::DynBox);
                    }
                    return;
                }
                if matches!(original_kind, Some("store_init")) && op.operands.len() >= 2 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let offset = op
                        .attrs
                        .get("value")
                        .and_then(|v| match v {
                            AttrValue::Int(v) => Some(*v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                    // Inline store_init: direct store for immediate values,
                    // runtime call only for heap pointers (need inc_ref + mark_has_ptrs).
                    let i64_ty = self.backend.context.i64_type();
                    let i8_ty = self.backend.context.i8_type();
                    let ptr_ty = self
                        .backend
                        .context
                        .ptr_type(inkwell::AddressSpace::default());
                    // Check if val is a heap pointer: (val & TAG_MASK) == TAG_PTR
                    let tag_mask = i64_ty.const_int(nanbox::QNAN | 0x0007_0000_0000_0000, false);
                    let tag_bits = self
                        .backend
                        .builder
                        .build_and(val_bits, tag_mask, "init_tag")
                        .unwrap();
                    let ptr_tag = i64_ty.const_int(nanbox::QNAN | 0x0004_0000_0000_0000, false);
                    let is_ptr = self
                        .backend
                        .builder
                        .build_int_compare(inkwell::IntPredicate::EQ, tag_bits, ptr_tag, "is_ptr")
                        .unwrap();
                    let current_fn = self.llvm_fn;
                    let fast_bb = self
                        .backend
                        .context
                        .append_basic_block(current_fn, "init_fast");
                    let slow_bb = self
                        .backend
                        .context
                        .append_basic_block(current_fn, "init_slow");
                    let merge_bb = self
                        .backend
                        .context
                        .append_basic_block(current_fn, "init_merge");
                    self.all_llvm_blocks.push(fast_bb);
                    self.all_llvm_blocks.push(slow_bb);
                    self.all_llvm_blocks.push(merge_bb);
                    self.backend
                        .builder
                        .build_conditional_branch(is_ptr, slow_bb, fast_bb)
                        .unwrap();
                    // Fast path: immediate value — direct store.
                    self.backend.builder.position_at_end(fast_bb);
                    let raw_ptr = self
                        .backend
                        .builder
                        .build_int_to_ptr(obj_ptr_bits, ptr_ty, "obj_ptr")
                        .unwrap();
                    let offset_val = i64_ty.const_int(offset as u64, true);
                    let field_ptr = unsafe {
                        self.backend
                            .builder
                            .build_in_bounds_gep(i8_ty, raw_ptr, &[offset_val], "field_ptr")
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(field_ptr, val_bits)
                        .unwrap();
                    self.backend
                        .builder
                        .build_unconditional_branch(merge_bb)
                        .unwrap();
                    // Slow path: pointer value — runtime call.
                    self.backend.builder.position_at_end(slow_bb);
                    let set_fn = self.ensure_runtime_i64_fn("molt_object_field_init_ptr", 3);
                    self.backend
                        .builder
                        .build_call(
                            set_fn,
                            &[
                                obj_ptr_bits.into(),
                                i64_ty.const_int(offset as u64, true).into(),
                                val_bits.into(),
                            ],
                            "field_init_slow",
                        )
                        .unwrap();
                    self.backend
                        .builder
                        .build_unconditional_branch(merge_bb)
                        .unwrap();
                    // Merge.
                    self.backend.builder.position_at_end(merge_bb);
                    if !op.results.is_empty() {
                        let none_val: BasicValueEnum<'ctx> = i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into();
                        self.values.insert(op.results[0], none_val);
                        self.value_types.insert(op.results[0], TirType::DynBox);
                    }
                    return;
                }
                if matches!(original_kind, Some("store")) && op.operands.len() >= 2 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let offset = op
                        .attrs
                        .get("value")
                        .and_then(|v| match v {
                            AttrValue::Int(v) => Some(*v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                    let set_fn = self.ensure_runtime_i64_fn("molt_object_field_set_ptr", 3);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            set_fn,
                            &[
                                obj_ptr_bits.into(),
                                self.backend
                                    .context
                                    .i64_type()
                                    .const_int(offset as u64, true)
                                    .into(),
                                val_bits.into(),
                            ],
                            "field_store",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if !op.results.is_empty() {
                        self.values.insert(op.results[0], result);
                        self.value_types.insert(op.results[0], TirType::DynBox);
                    }
                    return;
                }
                if matches!(
                    original_kind,
                    Some("guarded_field_set") | Some("guarded_field_init")
                ) && op.operands.len() >= 4
                {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let expected_version = self.materialize_dynbox_operand(op.operands[2]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[3]);
                    let attr_name = op
                        .attrs
                        .get("name")
                        .and_then(|v| {
                            if let AttrValue::Str(s) = v {
                                Some(s.as_str())
                            } else {
                                None
                            }
                        })
                        .unwrap_or("<unknown>");
                    let offset = op
                        .attrs
                        .get("value")
                        .and_then(|v| match v {
                            AttrValue::Int(v) => Some(*v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                    let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
                    let rt_name = if matches!(original_kind, Some("guarded_field_init")) {
                        "molt_guarded_field_init_ptr"
                    } else {
                        "molt_guarded_field_set_ptr"
                    };
                    let set_fn = self.ensure_runtime_i64_fn(rt_name, 7);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            set_fn,
                            &[
                                obj_ptr_bits.into(),
                                class_bits.into(),
                                expected_version.into(),
                                self.backend
                                    .context
                                    .i64_type()
                                    .const_int(offset as u64, true)
                                    .into(),
                                val_bits.into(),
                                attr_ptr_bits.into(),
                                attr_len_bits.into(),
                            ],
                            "guarded_field_set",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if !op.results.is_empty() {
                        self.values.insert(op.results[0], result);
                        self.value_types.insert(op.results[0], TirType::DynBox);
                    }
                    return;
                }
                let obj = self.resolve(op.operands[0]);
                let attr_name = op
                    .attrs
                    .get("name")
                    .and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("<unknown>");
                let name = self.intern_string_const(attr_name);
                let val = self.resolve(op.operands[1]);
                let obj_i64 = self.materialize_dynbox_bits(
                    obj,
                    &self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox),
                );
                let name_i64 = self.ensure_i64(name);
                let val_i64 = self.materialize_dynbox_bits(
                    val,
                    &self
                        .value_types
                        .get(&op.operands[1])
                        .cloned()
                        .unwrap_or(TirType::DynBox),
                );
                let set_fn = self
                    .backend
                    .module
                    .get_function("molt_set_attr_name")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[obj_i64.into(), name_i64.into(), val_i64.into()],
                        "setattr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], result);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }
            OpCode::DelAttr => {
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if matches!(original_kind, Some("del_attr_name")) && op.operands.len() >= 2 {
                    let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                    let name_bits = self.materialize_dynbox_operand(op.operands[1]);
                    let del_fn = self.ensure_runtime_i64_fn("molt_del_attr_name", 2);
                    let val = self
                        .backend
                        .builder
                        .build_call(
                            del_fn,
                            &[obj_bits.into(), name_bits.into()],
                            "del_attr_name_dyn",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if !op.results.is_empty() {
                        self.values.insert(op.results[0], val);
                        self.value_types.insert(op.results[0], TirType::DynBox);
                    }
                    return;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let attr_name = op
                    .attrs
                    .get("name")
                    .and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("<unknown>");
                let name = self.intern_string_const(attr_name);
                let name_bits = self.ensure_i64(name);
                let del_fn = self.ensure_runtime_i64_fn("molt_del_attr_name", 2);
                let val = self
                    .backend
                    .builder
                    .build_call(
                        del_fn,
                        &[obj_bits.into(), name_bits.into()],
                        "del_attr_name",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }
            OpCode::Index => {
                let result_id = op.results[0];
                // BCE: when the bounds-check elimination pass has proven the index
                // is in-range, we call `molt_getitem_unchecked` which skips the
                // runtime bounds check and associated branch entirely.
                let val = if has_attr(op, "bce_safe") {
                    self.call_runtime_2_boxed(
                        "molt_getitem_unchecked",
                        op.operands[0],
                        op.operands[1],
                    )
                } else {
                    self.call_runtime_2_boxed("molt_getitem_method", op.operands[0], op.operands[1])
                };
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StoreIndex => {
                let obj_i64 = self.materialize_dynbox_operand(op.operands[0]);
                let key_i64 = self.materialize_dynbox_operand(op.operands[1]);
                let val_i64 = self.materialize_dynbox_operand(op.operands[2]);
                let set_fn = self
                    .backend
                    .module
                    .get_function("molt_setitem_method")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[obj_i64.into(), key_i64.into(), val_i64.into()],
                        "setitem",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], result);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }
            OpCode::DelIndex => {
                let val = self.call_runtime_2_boxed(
                    "molt_delitem_method",
                    op.operands[0],
                    op.operands[1],
                );
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // ── Call ──
            OpCode::Call => {
                let i64_ty = self.backend.context.i64_type();
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });

                if matches!(original_kind, Some("call_func") | Some("call_function"))
                    && !op.operands.is_empty()
                {
                    let callable = self.resolve(op.operands[0]);
                    let result = self.emit_call_func_or_bind_runtime(callable, &op.operands[1..]);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                // Direct call by name: call_guarded stores the target function
                // name in s_value / _var, with all operands being arguments
                // (not a callable reference).  If the target already exists in
                // the LLVM module (same compilation unit), call it directly.
                let direct_target: Option<String> = op
                    .attrs
                    .get("s_value")
                    .or_else(|| op.attrs.get("_var"))
                    .and_then(|v| match v {
                        AttrValue::Str(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    });
                let direct_operands: &[ValueId] = if matches!(original_kind, Some("call_guarded")) {
                    op.operands.get(1..).unwrap_or(&[])
                } else {
                    &op.operands
                };
                let guarded_callable = if matches!(original_kind, Some("call_guarded")) {
                    op.operands.first().copied()
                } else {
                    None
                };

                if matches!(original_kind, Some("call_bind") | Some("call_indirect"))
                    && op.operands.len() >= 2
                {
                    let callable_i64 = self.ensure_i64(self.resolve(op.operands[0]));
                    let builder_bits = self.ensure_i64(self.resolve(op.operands[1]));
                    let site_bits = self.next_call_site_bits(original_kind.unwrap_or("call_bind"));
                    let runtime_name = if matches!(original_kind, Some("call_indirect")) {
                        "molt_call_indirect_ic"
                    } else {
                        "molt_call_bind_ic"
                    };
                    let runtime_fn = self.ensure_runtime_i64_fn(runtime_name, 3);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            runtime_fn,
                            &[site_bits.into(), callable_i64.into(), builder_bits.into()],
                            runtime_name,
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                if matches!(original_kind, Some("call_guarded"))
                    && let Some(callable_id) = guarded_callable
                {
                    let callable = self.resolve(callable_id);
                    let result = self.emit_call_func_runtime(callable, direct_operands);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                if let Some(ref target_name) = direct_target {
                    if let Some(target_fn) = self.backend.module.get_function(target_name) {
                        let target_return_tir_ty = self
                            .backend
                            .function_return_types
                            .get(target_name.as_str())
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        let expected_params = target_fn.count_params() as usize;
                        if expected_params != direct_operands.len()
                            && let Some(callable_id) = guarded_callable
                        {
                            let callable = self.resolve(callable_id);
                            let result = self.emit_call_bind_runtime(callable, direct_operands);
                            if let Some(&result_id) = op.results.first() {
                                self.values.insert(result_id, result);
                                self.value_types.insert(result_id, TirType::DynBox);
                            }
                            return;
                        }
                        let current_bb = self
                            .backend
                            .builder
                            .get_insert_block()
                            .expect("direct call must be emitted inside a basic block");
                        // Direct call — all operands are positional args.
                        // Every direct-call argument must be coerced from its
                        // SOURCE TirType to the CALLEE's declared param TirType
                        // (DynBox = the boxed molt ABI default). This was
                        // previously gated on `call_guarded` only — a plain
                        // `call`/`call_internal` passed an I64-typed value (or
                        // constant) RAW into a NaN-boxed parameter, where the
                        // raw bits decode as a garbage float (e.g.
                        // `compute(1000000)` received ~4.9e-318 and the loop
                        // exited after one iteration). The LLVM-type coercion
                        // below is a bitcast-level cast and cannot substitute
                        // for representation boxing.
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
                                    let source_tir_ty = self
                                        .value_types
                                        .get(&id)
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let target_tir_ty = self
                                        .backend
                                        .function_param_types
                                        .get(target_name.as_str())
                                        .and_then(|tys| tys.get(idx))
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let v = self.coerce_to_tir_type(
                                        v,
                                        &source_tir_ty,
                                        &target_tir_ty,
                                        current_bb,
                                    );
                                    let target_ty = target_fn
                                        .get_nth_param(idx as u32)
                                        .map(|param| param.get_type())
                                        .unwrap_or_else(|| self.backend.context.i64_type().into());
                                    self.coerce_to_type(v, target_ty, current_bb).into()
                                })
                                .collect();
                        let call_result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap();
                        if let Some(&result_id) = op.results.first() {
                            let raw_result =
                                call_result.try_as_basic_value().basic().unwrap_or_else(|| {
                                    i64_ty
                                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                        .into()
                                });
                            let result = if target_return_tir_ty == TirType::DynBox {
                                raw_result
                            } else {
                                materialize_dynbox_bits_with_builder(
                                    &self.backend.builder,
                                    self.backend.context,
                                    &self.backend.module,
                                    self.llvm_fn,
                                    raw_result,
                                    &target_return_tir_ty,
                                )
                                .into()
                            };
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    } else {
                        if let Some(callable_id) = guarded_callable {
                            let callable = self.resolve(callable_id);
                            let result = self.emit_call_bind_runtime(callable, direct_operands);
                            if let Some(&result_id) = op.results.first() {
                                self.values.insert(result_id, result);
                                self.value_types.insert(result_id, TirType::DynBox);
                            }
                            return;
                        }
                        // Target not yet in module — forward-declare it and call.
                        let param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                            direct_operands.iter().map(|_| i64_ty.into()).collect();
                        let fn_ty = i64_ty.fn_type(&param_types, false);
                        let target_fn = self.backend.module.add_function(
                            target_name,
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        );
                        let current_bb = self
                            .backend
                            .builder
                            .get_insert_block()
                            .expect("direct call must be emitted inside a basic block");
                        // Forward-declared target: the callee's TIR param
                        // types are unknown, so the boxed molt ABI (DynBox) is
                        // the contract — box every non-DynBox source (see the
                        // resolved-target path above for the raw-bits-as-float
                        // miscompile this prevents).
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
                                    let source_tir_ty = self
                                        .value_types
                                        .get(&id)
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let v = self.coerce_to_tir_type(
                                        v,
                                        &source_tir_ty,
                                        &TirType::DynBox,
                                        current_bb,
                                    );
                                    let target_ty = target_fn
                                        .get_nth_param(idx as u32)
                                        .map(|param| param.get_type())
                                        .unwrap_or_else(|| self.backend.context.i64_type().into());
                                    self.coerce_to_type(v, target_ty, current_bb).into()
                                })
                                .collect();
                        let result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap_or_else(|| {
                                i64_ty
                                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                    .into()
                            });
                        if let Some(&result_id) = op.results.first() {
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    }
                } else if !op.operands.is_empty() {
                    // Indirect call: operands[0] = callable, rest = positional args.
                    let callable = self.resolve(op.operands[0]);
                    let result = self.emit_call_bind_runtime(callable, &op.operands[1..]);

                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // No operands, no direct target — emit None.
                    if let Some(&result_id) = op.results.first() {
                        let none_val: BasicValueEnum<'ctx> = i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into();
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── DeleteVar local-slot transition ──
            // DeleteVar defines the local's new SSA value as the missing sentinel
            // operand. Ownership of the previous slot occupant is modeled by the
            // drop fact plane (DecRef / explicit consumed operands), not by LLVM
            // lowering.
            OpCode::DeleteVar => {
                if op.results.is_empty() {
                    // Side-effect-only legacy shapes have no SSA value to bind.
                } else if let Some(&missing) = op.operands.first() {
                    let val = self.resolve(missing);
                    let ty = self
                        .value_types
                        .get(&missing)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
                    }
                } else {
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── SSA Copy ──
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::Copy => {
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // TERMINAL FAIL-LOUD STATE (preserved-op passthrough class),
                // tied to the `CopyLowering` single source of truth.
                //
                // Reaching here means this `OpCode::Copy` carries an
                // `_original_kind` that `lower_preserved_simpleir_op` did NOT
                // handle — neither a dedicated arm nor the generic
                // `try_lower_preserved_runtime_call` (`molt_<kind>`) fallback
                // claimed it. EVERY such op is a SimpleIR op the native/WASM/Luau
                // lanes lower with dedicated semantics (a value-producing runtime
                // call, an RC adjustment, a type guard, a generator throw, or a
                // registry-owned identity fact). Passing operand 0 through by
                // default -- the historical behavior -- silently miscompiled
                // non-identity semantics: `abs(x)` returned `x`,
                // `...`/`NotImplemented` became `None`, `raise ... from ...`'s
                // `__cause__` link, `gen.throw`, special-attr loads, the
                // `borrow`/`release` refcount ops, the `guard_tag` type check, and
                // every fresh-value conversion (`int(x)`, `s[-5:]`, `dict.keys()`,
                // ...) were all DROPPED or returned operand 0. The few preserved
                // repr-identity ops whose operand-0 passthrough is correct are
                // claimed explicitly in `lower_preserved_simpleir_op`; any
                // preserved op that reaches this terminal state is therefore not a
                // sound default passthrough. Fail the build loudly here.
                //
                // SINGLE SOURCE OF TRUTH (the drift the `CopyLowering` classifier
                // forbids). The drop-insertion pass releases exactly the `Copy`s
                // whose `_original_kind` is a `CopyLowering::FreshValue`
                // (`alias_analysis::copy_kind_mints_fresh_owned_ref`). If such a
                // fresh-owned producer reached codegen as a silent operand-0
                // passthrough, the result would (a) be the wrong value AND (b)
                // alias operand 0 — which the drop pass then DOUBLE-FREES. The gate
                // therefore consults that classifier on every fatal so the table
                // and the backend cannot drift: a `FreshValue` reaching here is the
                // forbidden drift (a fresh-value op missing its explicit LLVM arm),
                // and the diagnostic names it as such; any other `_original_kind`
                // gets the general terminal message with operand/result counts.
                // Closing a kind = add an arm to `lower_preserved_simpleir_op` (or,
                // if `molt_<kind>` is a real boxed runtime intrinsic, the generic
                // fallback already covers it). See the `CopyLowering` docs and
                // `tests::copy_lowering_classes_are_total_and_disjoint`.
                if let Some(kind) = original_kind {
                    if crate::tir::passes::alias_analysis::copy_kind_reaches_no_incref_passthrough(
                        Some(kind),
                    ) {
                        // Not a `FreshValue` (a transparent-alias / inert-marker
                        // kind whose `molt_<kind>` intrinsic is also absent): the
                        // partner's general terminal state. Still fail loud — an
                        // unhandled `_original_kind` is never a sound passthrough.
                        self.record_fatal(format!(
                            "unhandled preserved SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile or drop its side effect; add a \
                             `lower_preserved_simpleir_op` arm for it (or confirm \
                             `molt_{kind}` is a boxed runtime intrinsic so the \
                             generic fallback claims it)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    } else {
                        // A `CopyLowering::FreshValue` reached the passthrough: the
                        // exact classifier↔backend drift this gate exists to catch.
                        self.record_fatal(format!(
                            "fresh-value SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile AND make the result alias operand 0 (a \
                             drop-insertion double-free); it is in \
                             `alias_analysis::copy_kind_mints_fresh_owned_ref` so it \
                             MUST have a `lower_preserved_simpleir_op` arm (the \
                             classifier and the LLVM lowering have drifted)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    }
                    return;
                }

                // `_original_kind == None`: a genuine SSA value copy
                // (`copy`/`copy_var`/`load_var`/`store_var`). Operand-0
                // passthrough is the correct lowering.
                if op.results.is_empty() {
                    // No results — nothing to bind; skip.
                } else if op.operands.is_empty() {
                    // Unknown op with no operands — produce None for each result.
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // Standard copy: pass through first operand.
                    let val = self.resolve(op.operands[0]);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
                    }
                }
            }

            // ── Allocation ──
            OpCode::Alloc => {
                let result_id = op.results[0];
                let size = self.resolve(op.operands[0]);
                let size_i64 = self.ensure_i64(size);
                let alloc_fn = self.backend.module.get_function("molt_alloc").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(alloc_fn, &[size_i64.into()], "alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── CallMethod: receiver.method(args...) ──
            // Protocol: molt_call_method(receiver, method_name_bits, args_builder) -> u64
            // operands: [receiver, method_name, arg0, arg1, ...]
            OpCode::CallMethod => {
                let i64_ty = self.backend.context.i64_type();
                if op.operands.is_empty() {
                    return;
                }
                let method_bits = self.ensure_i64(self.resolve(op.operands[0]));

                // Build positional args (operands[1..]) for the bound method object.
                let n_args = op.operands.len().saturating_sub(1) as u64;
                let new_fn = self
                    .backend
                    .module
                    .get_function("molt_callargs_new")
                    .unwrap();
                let args_builder = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            i64_ty.const_int(n_args, false).into(),
                            i64_ty.const_int(0, false).into(),
                        ],
                        "cm_args",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_callargs_push_pos")
                    .unwrap();
                for &arg_id in op.operands.get(1..).unwrap_or(&[]) {
                    // Method-call args flow through `molt_call_bind_ic` into the
                    // bound method's trampoline, which decodes each NaN-boxed
                    // `DynBox` into its parameter's raw representation. Box per the
                    // value's representation plan rather than passing raw bits (a
                    // raw `I64`/`F64` arg would be decoded as a boxed payload —
                    // the same carrier miscompile as the plain-call paths).
                    let arg_i64 = self.materialize_dynbox_operand(arg_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cm_push")
                        .unwrap();
                }
                let site_bits = self.next_call_site_bits("call_method");
                let call_bind_fn = self.ensure_runtime_i64_fn("molt_call_bind_ic", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        call_bind_fn,
                        &[site_bits.into(), method_bits.into(), args_builder.into()],
                        "call_method_bind",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── CallBuiltin: builtin_name(args...) ──
            //
            // Two patterns reach here:
            //   A) `call_builtin` from the frontend: s_value / name attr holds the
            //      builtin name, operands[0] is a ConstStr with the name bits,
            //      rest are positional args.
            //   B) `print` / `builtin_print`: the op kind IS the builtin name,
            //      stored in `_original_kind`.  ALL operands are arguments — the
            //      first is NOT a name.
            //
            // We detect (B) by checking for `_original_kind` (only set when the
            // SSA converter wraps a non-canonical kind).  For (A), the `name`
            // attr holds the builtin name string.
            OpCode::CallBuiltin => {
                let i64_ty = self.backend.context.i64_type();

                // Determine the builtin name and where positional args start.
                let (builtin_name_str, args_start): (Option<String>, usize) = {
                    let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    });
                    let name_attr = op.attrs.get("name").and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    });
                    if let Some(kind) = original_kind {
                        // Pattern B: print, builtin_print, etc.
                        // All operands are args.
                        (Some(kind.to_string()), 0)
                    } else if let Some(name) = name_attr {
                        // Pattern A: call_builtin with explicit name.
                        // operands[0] is the name ConstStr, rest are args.
                        (Some(name.to_string()), 1)
                    } else {
                        // Fallback: operands[0] is the name bits.
                        (None, 1)
                    }
                };

                if builtin_name_str.as_deref() == Some("print")
                    || builtin_name_str.as_deref() == Some("builtin_print")
                {
                    // PRINT is a dedicated frontend op. By the time it reaches
                    // backend IR, multi-argument CPython semantics have already
                    // been normalized into a single joined display string plus
                    // explicit newline behavior. Lower it directly to the
                    // runtime print surface just like the native backend.
                    let print_fn = self.ensure_runtime_void_fn("molt_print_obj", 1);
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg_i64 = self.materialize_dynbox_operand(arg_id);
                        self.backend
                            .builder
                            .build_call(print_fn, &[arg_i64.into()], "print")
                            .unwrap();
                    }
                    if let Some(&result_id) = op.results.first() {
                        let none_val: BasicValueEnum<'ctx> = i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into();
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else if builtin_name_str.as_deref() == Some("range_new") {
                    // `range(...)` is a dedicated frontend op (`RANGE_NEW`), not a
                    // generic builtin lookup. The SSA lifter folds it into
                    // `OpCode::CallBuiltin` with `_original_kind = "range_new"`
                    // (ssa.rs), but `range` is NOT registered as a runtime
                    // intrinsic and `molt_call_builtin` would fall through to the
                    // builtins module-cache path — failing at any call site reached
                    // before that cache is populated. Lower directly to the
                    // dedicated runtime constructor `molt_range_new(start, stop,
                    // step)`, exactly as the native and WASM backends do. The
                    // frontend (`_parse_range_call`) always materializes all three
                    // boxed bounds (start defaults to 0, step to 1), so operands is
                    // exactly [start, stop, step] (args_start == 0 because Pattern B
                    // was detected via `_original_kind`).
                    debug_assert_eq!(
                        op.operands.len(),
                        3,
                        "range_new must carry exactly [start, stop, step]"
                    );
                    if op.operands.len() != 3 {
                        return;
                    }
                    let range_new_fn = self.ensure_runtime_i64_fn("molt_range_new", 3);
                    let start = self.materialize_dynbox_operand(op.operands[0]).into();
                    let stop = self.materialize_dynbox_operand(op.operands[1]).into();
                    let step = self.materialize_dynbox_operand(op.operands[2]).into();
                    let result = self
                        .backend
                        .builder
                        .build_call(range_new_fn, &[start, stop, step], "range_new")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // Generic builtin call via molt_call_builtin.
                    let builtin_name_bits = if let Some(ref name) = builtin_name_str {
                        // Create a runtime string for the builtin name via
                        // molt_string_from_bytes.
                        let name_val = self.intern_string_const(name);
                        self.ensure_i64(name_val)
                    } else if args_start <= op.operands.len() && !op.operands.is_empty() {
                        let bv = self.resolve(op.operands[0]);
                        self.ensure_i64(bv)
                    } else if let Some(s_val) = op.attrs.get("s_value").and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    }) {
                        let name_val = self.intern_string_const(s_val);
                        self.ensure_i64(name_val)
                    } else {
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    };

                    let n_args = op.operands.len().saturating_sub(args_start) as u64;
                    let new_fn = self
                        .backend
                        .module
                        .get_function("molt_callargs_new")
                        .unwrap();
                    let args_builder = self
                        .backend
                        .builder
                        .build_call(
                            new_fn,
                            &[
                                i64_ty.const_int(n_args, false).into(),
                                i64_ty.const_int(0, false).into(),
                            ],
                            "cb_args",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    let push_fn = self
                        .backend
                        .module
                        .get_function("molt_callargs_push_pos")
                        .unwrap();
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg_i64 = self.materialize_dynbox_operand(arg_id);
                        self.backend
                            .builder
                            .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cb_push")
                            .unwrap();
                    }

                    let call_builtin_fn = self
                        .backend
                        .module
                        .get_function("molt_call_builtin")
                        .unwrap();
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            call_builtin_fn,
                            &[builtin_name_bits.into(), args_builder.into()],
                            "call_builtin",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── OrdAt: fused ord(container[index]) ──
            OpCode::OrdAt => {
                if op.operands.len() < 2 {
                    return;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let index_bits = self.materialize_dynbox_operand(op.operands[1]);
                let ord_at_fn = self.ensure_runtime_i64_fn("molt_ord_at", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_at_fn, &[obj_bits.into(), index_bits.into()], "ord_at")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── StackAlloc: alloca for stack-resident slots ──
            // attrs: { "type": "i64" | "dynbox" | ... }
            // result: pointer stored as i64 (ptrtoint)
            OpCode::StackAlloc => {
                let i64_ty = self.backend.context.i64_type();
                let ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "stack_slot")
                    .unwrap();
                let ptr_as_i64 = self
                    .backend
                    .builder
                    .build_ptr_to_int(ptr, i64_ty, "slot_ptr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, ptr_as_i64.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Free: stack-allocated slots are freed automatically — no-op ──
            OpCode::Free => {
                // Stack memory is reclaimed by the function epilogue; nothing to emit.
            }

            // ── BuildList: [item0, item1, ...] ──
            // Strategy: list_builder_new(capacity) + append + finish.
            OpCode::BuildList => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let list_new_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(list_new_fn, &[i64_ty.const_int(n, false).into()], "list")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "list_push")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_finish")
                    .unwrap();
                let list = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "list_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildDict: {k0: v0, k1: v1, ...} ──
            // operands: [k0, v0, k1, v1, ...]  (pairs)
            OpCode::BuildDict => {
                let i64_ty = self.backend.context.i64_type();
                let n_pairs = (op.operands.len() / 2) as u64;
                let dict_new_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        dict_new_fn,
                        &[i64_ty.const_int(n_pairs, false).into()],
                        "dict_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let dict_set_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_append")
                    .unwrap();
                let mut i = 0;
                while i + 1 < op.operands.len() {
                    let k_i64 = self.materialize_dynbox_operand(op.operands[i]);
                    let v_i64 = self.materialize_dynbox_operand(op.operands[i + 1]);
                    self.backend
                        .builder
                        .build_call(
                            dict_set_fn,
                            &[builder.into(), k_i64.into(), v_i64.into()],
                            "dict_append",
                        )
                        .unwrap();
                    i += 2;
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_finish")
                    .unwrap();
                let dict = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "dict_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, dict);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildTuple: (item0, item1, ...) ──
            OpCode::BuildTuple => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let tuple_builder_new = self
                    .backend
                    .module
                    .get_function("molt_list_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        tuple_builder_new,
                        &[i64_ty.const_int(n, false).into()],
                        "tuple_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "tup_push")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_tuple_builder_finish")
                    .unwrap();
                let tup = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "tuple_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, tup);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildSet: {item0, item1, ...} ──
            OpCode::BuildSet => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let set_new_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        set_new_fn,
                        &[i64_ty.const_int(n, false).into()],
                        "set_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "set_append")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_finish")
                    .unwrap();
                let set = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "set_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildSlice: slice(start, stop, step) ──
            // operands: [start, stop, step]   (already declared as molt_slice_new)
            OpCode::BuildSlice => {
                let i64_ty = self.backend.context.i64_type();
                let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                let none_val: BasicValueEnum<'ctx> = i64_ty.const_int(none_bits, false).into();

                let start = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };
                let stop = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };

                let slice_fn = self.backend.module.get_function("molt_slice_new").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(slice_fn, &[start.into(), stop.into(), step.into()], "slice")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── GetIter: iter(obj) ──
            OpCode::GetIter => {
                let obj = self.resolve(op.operands[0]);
                let obj_i64 = self.ensure_i64(obj);
                let get_iter_fn = self.backend.module.get_function("molt_get_iter").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(get_iter_fn, &[obj_i64.into()], "get_iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── IterNext: next(iter) -> value (or StopIteration sentinel) ──
            OpCode::IterNext => {
                let iter = self.resolve(op.operands[0]);
                let iter_i64 = self.ensure_i64(iter);
                let iter_next_fn = self.backend.module.get_function("molt_iter_next").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(iter_next_fn, &[iter_i64.into()], "iter_next")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ForIter: advance iterator, returning next value or exhaustion sentinel ──
            OpCode::ForIter => {
                // Vectorization hint: when `vectorize = true` is set on this op (by the
                // vectorize analysis pass), the enclosing loop body is safe to vectorize.
                //
                // Per-loop vectorization metadata (`!{!"llvm.loop.vectorize.enable", i1 1}`)
                // requires attaching an MDNode to the loop back-edge branch instruction.
                // The inkwell API does not expose `LLVMSetMetadata` for branch instructions
                // nor the `MDNode`/`MDString` constructors needed to build loop metadata.
                // Vectorization is still enabled at the function level via `-march=native`
                // in the target machine (which enables +neon on ARM / +avx2 on x86), so
                // LLVM's loop vectorizer will analyze and vectorize eligible loops anyway.
                // To attach per-loop metadata, a raw `llvm-sys::LLVMSetMetadata` call on
                // the back-edge `BranchInst` would be needed.
                let _ = has_attr(op, "vectorize");

                let iter = self.resolve(op.operands[0]);
                let iter_i64 = self.ensure_i64(iter);
                let for_iter_fn = self.backend.module.get_function("molt_for_iter").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(for_iter_fn, &[iter_i64.into()], "for_iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Yield: suspend generator, yield value ──
            OpCode::AllocTask => {
                let result_id = op.results[0];
                let i64_ty = self.backend.context.i64_type();
                let closure_size = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let task_kind = op
                    .attrs
                    .get("task_kind")
                    .and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .unwrap_or("future");
                let (kind_bits, payload_base) = match task_kind {
                    "generator" => (crate::TASK_KIND_GENERATOR, crate::GENERATOR_CONTROL_BYTES),
                    "future" => (crate::TASK_KIND_FUTURE, 0),
                    "coroutine" => (crate::TASK_KIND_COROUTINE, 0),
                    _ => panic!("unknown task kind: {task_kind}"),
                };
                let Some(poll_func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    panic!(
                        "alloc_task missing poll function name in {}",
                        self.func.name
                    );
                };
                let poll_fn = self.ensure_function_symbol(poll_func_name, 1, false);
                let poll_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        poll_fn.as_global_value().as_pointer_value(),
                        i64_ty,
                        "task_poll_ptr",
                    )
                    .unwrap();
                let task_new_fn = self.ensure_runtime_i64_fn("molt_task_new", 3);
                let task_bits = self
                    .backend
                    .builder
                    .build_call(
                        task_new_fn,
                        &[
                            poll_addr.into(),
                            i64_ty.const_int(closure_size as u64, true).into(),
                            i64_ty.const_int(kind_bits as u64, true).into(),
                        ],
                        "task_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let ptr_ty = self
                    .backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default());
                // `molt_task_new` returns the NaN-boxed task handle (QNAN | TAG_PTR
                // in the top 16 bits). The frame-payload stores below address raw
                // heap memory, so the boxing tag MUST be stripped first — mirroring
                // the native backend's `unbox_ptr_value(obj)` (simple_backend.rs)
                // before its `store` to the frame. Using the boxed bits directly as
                // a base address writes through `0x7FFC…`-tagged garbage → SIGSEGV
                // at generator creation. The boxed `task_bits` is still what flows
                // into the result value; only the field base pointer is unboxed.
                let task_ptr_bits = self.unbox_ptr_bits(self.ensure_i64(task_bits));
                let task_ptr = self
                    .backend
                    .builder
                    .build_int_to_ptr(task_ptr_bits, ptr_ty, "task_obj_ptr")
                    .unwrap();
                for (idx, &arg_id) in op.operands.iter().enumerate() {
                    let arg_bits = self.materialize_dynbox_operand(arg_id);
                    let field_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                task_ptr,
                                &[i64_ty
                                    .const_int(((payload_base / 8) as usize + idx) as u64, false)],
                                &format!("task_payload_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(field_ptr, arg_bits)
                        .unwrap();
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    let _ = self
                        .backend
                        .builder
                        .build_call(inc_fn, &[arg_bits.into()], "task_payload_inc_ref")
                        .unwrap();
                }
                self.values.insert(result_id, task_bits);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StateSwitch => {
                // `state_switch` is now lowered as the first-class `StateDispatch`
                // terminator (see `lower_terminator`), never as a body op: the
                // SimpleIR `state_switch` op is structural (excluded from TIR
                // block ops by `gather_defs_uses`) and the SSA terminator builder
                // emits `Terminator::StateDispatch` for the dispatch block.
                // Reaching it here as a body op means the structural-op invariant
                // broke upstream — fail loud rather than emit a second (synthetic)
                // dispatch that double-switches the saved state.
                panic!(
                    "OpCode::StateSwitch reached the LLVM op-lowering body in '{}'; \
                     state_switch must lower as the StateDispatch terminator",
                    self.func.name
                );
            }
            OpCode::ClosureLoad => {
                let result_id = op.results[0];
                let self_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let load_fn = self.ensure_runtime_i64_fn("molt_closure_load", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        load_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(offset as u64, true)
                                .into(),
                        ],
                        "closure_load",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::ClosureStore => {
                let self_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        store_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(offset as u64, true)
                                .into(),
                            val_bits.into(),
                        ],
                        "closure_store",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::StateYield => {
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let self_bits = self.generator_self_bits();
                let pair_bits = self.materialize_dynbox_operand(op.operands[0]);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(next_state_id as u64, true)
                                .into(),
                        ],
                        "state_yield_set_state",
                    )
                    .unwrap();
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(inc_fn, &[pair_bits.into()], "state_yield_inc_ref")
                    .unwrap();
                // The suspend `ret`s the yielded pair.  This `build_return`
                // terminates the suspend block; the main lowering loop detects
                // the terminator and moves on to the NEXT TIR block (the real
                // post-yield resume continuation, which the `StateDispatch`
                // terminator dispatches to).  We do NOT `position_at_end` into a
                // synthetic resume block — the continuation is a first-class TIR
                // block reached via the dispatch, and its phis were placed by the
                // SSA pass on the real `state_resume_edges`.
                self.backend.builder.build_return(Some(&pair_bits)).unwrap();
                let _ = next_state_id;
            }
            OpCode::StateTransition => {
                let (slot_id, pending_state_operand) = match op.operands.as_slice() {
                    [_, pending_state] => (None, *pending_state),
                    [_, slot, pending_state] => (Some(*slot), *pending_state),
                    other => panic!(
                        "state_transition expected 2 or 3 operands in {}: {:?}",
                        self.func.name, other
                    ),
                };
                let pending_state_id = self.const_i64_operand(pending_state_operand);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let future_bits = self.materialize_dynbox_operand(op.operands[0]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "state_transition_set_pending",
                    )
                    .unwrap();
                let poll_fn = self.ensure_runtime_i64_fn("molt_future_poll", 1);
                let res = self
                    .backend
                    .builder
                    .build_call(poll_fn, &[future_bits.into()], "state_transition_poll")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "state_transition_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("state_transition_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("state_transition_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                let sleep_fn = self.ensure_runtime_i64_fn("molt_sleep_register", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        sleep_fn,
                        &[self_bits.into(), future_bits.into()],
                        "state_transition_sleep",
                    )
                    .unwrap();
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                if let Some(slot_id) = slot_id {
                    let slot_bits = self.raw_i64_operand(slot_id, ready_path);
                    let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
                    let _ = self
                        .backend
                        .builder
                        .build_call(
                            store_fn,
                            &[self_bits.into(), slot_bits.into(), res.into()],
                            "state_transition_store",
                        )
                        .unwrap();
                } else if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "state_transition_set_next",
                    )
                    .unwrap();
                let next_bb = self.resume_block_for_state(next_state_id);
                let ready_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition ready must be in block");
                self.record_llvm_edge(ready_from_bb, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            OpCode::ChanSendYield => {
                let pending_state_id = self.const_i64_operand(op.operands[2]);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_send_yield must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "chan_send_set_pending",
                    )
                    .unwrap();
                let send_fn = self.ensure_runtime_i64_fn("molt_chan_send", 2);
                let res = self
                    .backend
                    .builder
                    .build_call(send_fn, &[chan_bits.into(), val_bits.into()], "chan_send")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "chan_send_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_send_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_send_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_send_yield branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "chan_send_set_next",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_bb = self.resume_block_for_state(next_state_id);
                self.record_llvm_edge(ready_path, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            OpCode::ChanRecvYield => {
                let pending_state_id = self.const_i64_operand(op.operands[1]);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_recv_yield must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "chan_recv_set_pending",
                    )
                    .unwrap();
                let recv_fn = self.ensure_runtime_i64_fn("molt_chan_recv", 1);
                let res = self
                    .backend
                    .builder
                    .build_call(recv_fn, &[chan_bits.into()], "chan_recv")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "chan_recv_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_recv_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_recv_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_recv_yield branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "chan_recv_set_next",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_bb = self.resume_block_for_state(next_state_id);
                self.record_llvm_edge(ready_path, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            // ── Yield: suspend generator, yield value ──
            OpCode::Yield => {
                let val = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    // yield without value yields None
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    self.backend.context.i64_type().const_int(none_bits, false)
                };
                let yield_fn = self.backend.module.get_function("molt_yield").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(yield_fn, &[val.into()], "yield")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── YieldFrom: delegate to sub-generator ──
            OpCode::YieldFrom => {
                let subiter = self.resolve(op.operands[0]);
                let subiter_i64 = self.ensure_i64(subiter);
                let yield_from_fn = self.backend.module.get_function("molt_yield_from").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(yield_from_fn, &[subiter_i64.into()], "yield_from")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Raise: raise exception ──
            OpCode::Raise => {
                let exc = self.resolve(op.operands[0]);
                let exc_i64 = self.ensure_i64(exc);
                let raise_fn = self.backend.module.get_function("molt_raise").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(raise_fn, &[exc_i64.into()], "raise")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], result);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // ── WarnStderr: side-effecting diagnostic emit ──
            OpCode::WarnStderr => {
                let msg = self.resolve(op.operands[0]);
                let msg_i64 = self.ensure_i64(msg);
                let warn_fn = self
                    .backend
                    .module
                    .get_function("molt_warn_stderr")
                    .unwrap();
                self.backend
                    .builder
                    .build_call(warn_fn, &[msg_i64.into()], "warn_stderr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ExceptionPending: read the runtime exception-pending flag as
            //    a raw i64 boolean (`molt_exception_pending() != 0`).  Produced
            //    by `loop_break_if_exception` and consumed as the condition of
            //    the loop-exit CondBranch that breaks an iterator-consumer loop
            //    on a mid-iteration raise.  Non-foldable: it observes mutable
            //    runtime state, so the value (and the break) always survive.
            OpCode::ExceptionPending => {
                let pend_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let raw = self
                    .backend
                    .builder
                    .build_call(pend_fn, &[], "exc_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // `molt_exception_pending` returns a raw i64 0/1 (NOT a
                    // NaN-boxed bool), so the consuming CondBranch must test it
                    // with `!= 0` (the TirType::I64 path) rather than routing it
                    // through `molt_is_truthy`, which would misinterpret the bit
                    // pattern of `1` as a boxed value.
                    self.value_types.insert(result_id, TirType::I64);
                }
            }

            // ── FunctionDefaultsVersion: read a function object's
            //    __defaults__/__kwdefaults__ mutation version stamp as a boxed
            //    inline int (`molt_function_defaults_version(func)`).  Produced
            //    by the compile-time defaults-devirt deopt guard and consumed by
            //    its `== 0` compare (baked literal vs live read).  Non-foldable:
            //    it observes mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => {
                let ver_fn = self.ensure_runtime_i64_fn("molt_function_defaults_version", 1);
                let func_val = op
                    .operands
                    .first()
                    .and_then(|id| self.values.get(id).copied())
                    .expect("FunctionDefaultsVersion operand not materialized");
                let raw = self
                    .backend
                    .builder
                    .build_call(ver_fn, &[func_val.into()], "func_defaults_version")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // Returns a NaN-boxed inline int; the consuming `== 0`
                    // compare routes through the boxed-int equality path.
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── CheckException: inspect the current exception state ──
            OpCode::CheckException => {
                let check_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let result = self
                    .backend
                    .builder
                    .build_call(check_fn, &[], "check_exc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let Some(target_label) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return;
                };
                let Some(target_block_id) = self
                    .func
                    .label_id_map
                    .iter()
                    .find_map(|(bid, label)| (*label == target_label).then_some(BlockId(*bid)))
                else {
                    self.record_fatal(format!(
                        "check_exception target label {} is not present in label map",
                        target_label
                    ));
                    return;
                };
                let Some(&target_bb) = self.block_map.get(&target_block_id) else {
                    self.record_fatal(format!(
                        "check_exception target block {:?} is not present in LLVM block map",
                        target_block_id
                    ));
                    return;
                };
                let continue_bb = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("check_exc_cont{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(continue_bb);
                let pending = self.ensure_i64(result);
                let has_exception = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        pending,
                        self.backend.context.i64_type().const_zero(),
                        "check_exc_pending",
                    )
                    .unwrap();
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    branch_from_bb,
                    target_block_id,
                    "check-exception-edge",
                    &op.operands,
                );
                self.record_llvm_edge(branch_from_bb, target_bb);
                self.record_llvm_edge(branch_from_bb, continue_bb);
                self.backend
                    .builder
                    .build_conditional_branch(has_exception, target_bb, continue_bb)
                    .unwrap();
                self.backend.builder.position_at_end(continue_bb);
            }

            // ── Import: import module by name ──
            OpCode::Import => {
                let result_id = op.results[0];
                let name = if let Some(&name_id) = op.operands.first() {
                    self.resolve(name_id)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("module") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("s_value") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("_var") {
                    self.intern_string_const(module_name)
                } else {
                    panic!(
                        "Import op missing module operand/attr in {}",
                        self.func.name
                    );
                };
                let name_i64 = self.ensure_i64(name);
                let import_fn = self
                    .backend
                    .module
                    .get_function("molt_module_import")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(import_fn, &[name_i64.into()], "import")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── ImportFrom: from module import name ──
            // operands: [module, attr_name]
            OpCode::ImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleCacheGet: module-cache lookup by name ──
            // operands: [module_name]
            OpCode::ModuleCacheGet => {
                let result_id = op.results[0];
                let get_fn = self.ensure_runtime_i64_fn("molt_module_cache_get", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(get_fn, &[name_bits.into()], "module_cache_get")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── ModuleCacheSet: module-cache mutation by name ──
            // operands: [module_name, module]
            OpCode::ModuleCacheSet => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_cache_set", 2);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let module_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[name_bits.into(), module_bits.into()],
                        "module_cache_set",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleCacheDel: module-cache deletion by name ──
            // operands: [module_name]
            OpCode::ModuleCacheDel => {
                let del_fn = self.ensure_runtime_i64_fn("molt_module_cache_del", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(del_fn, &[name_bits.into()], "module_cache_del")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetAttr: module attribute read ──
            // operands: [module, attr_name]
            OpCode::ModuleGetAttr => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleImportFrom: `from M import name` binding ──
            // operands: [module, attr_name]. CPython IMPORT_FROM semantics:
            // ImportError (not AttributeError) on miss, with a sys.modules
            // submodule fallback (see molt_module_import_from).
            OpCode::ModuleImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_import_from",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetGlobal: CPython-style module global lookup ──
            // operands: [module, global_name]
            OpCode::ModuleGetGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetName: module name/attribute lookup helper ──
            // operands: [module, attr_name]
            OpCode::ModuleGetName => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_name",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleSetAttr: module attribute mutation ──
            // operands: [module, attr_name, value]
            OpCode::ModuleSetAttr => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_set_attr", 3);
                let module_bits = self.materialize_dynbox_operand(op.operands[0]);
                let attr_bits = self.materialize_dynbox_operand(op.operands[1]);
                let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[module_bits.into(), attr_bits.into(), val_bits.into()],
                        "module_set_attr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleDelGlobal: CPython-style module global deletion ──
            // operands: [module, global_name]
            OpCode::ModuleDelGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ModuleDelGlobalIfPresent => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global_if_present",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── SCF dialect ops ──
            // Structured control flow ops are desugared into LLVM basic blocks.
            // ScfIf uses conditional branches to then/else blocks with a merge phi.
            // ScfFor/ScfWhile delegate to runtime helpers since full loop lowering
            // requires loop analysis infrastructure (induction variable detection,
            // trip count computation) that lives in a separate pass.
            // ScfYield maps to a runtime call that returns its value.
            OpCode::ScfIf => {
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();

                // Resolve condition and coerce to i1.
                let cond_i64 = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy_result = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[cond_i64.into()], "scf_if_truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy_result.into_int_value(),
                        i64_ty.const_int(0, false),
                        "scf_if_cond",
                    )
                    .unwrap();

                // Resolve then/else function operands.
                let then_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let else_fn_bits = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };

                // Create basic blocks for then, else, and merge.
                let current_fn = self.llvm_fn;
                let then_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_then");
                let else_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_else");
                let merge_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_merge");
                self.all_llvm_blocks.push(then_bb);
                self.all_llvm_blocks.push(else_bb);
                self.all_llvm_blocks.push(merge_bb);

                self.backend
                    .builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                // Then block: call then_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(then_bb);
                let call0_fn = self.backend.module.get_function("molt_call_0").unwrap();
                let then_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[then_fn_bits.into()], "scf_then_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let then_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Else block: call else_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(else_bb);
                let else_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[else_fn_bits.into()], "scf_else_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let else_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Merge block: phi node selects then/else result.
                self.backend.builder.position_at_end(merge_bb);
                let phi = self
                    .backend
                    .builder
                    .build_phi(i64_ty, "scf_if_phi")
                    .unwrap();
                phi.add_incoming(&[(&then_result, then_exit_bb), (&else_result, else_exit_bb)]);
                let phi_val = phi.as_basic_value();

                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, phi_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfFor => {
                // ScfFor delegates to the runtime: full loop lowering requires
                // induction variable detection and trip count analysis that runs
                // as a separate TIR pass before LLVM lowering.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let lb = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let ub = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(1, false)
                };
                let body_fn_bits = if op.operands.len() > 3 {
                    let v = self.resolve(op.operands[3]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_for_fn = self.backend.module.get_function("molt_scf_for").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_for_fn,
                        &[lb.into(), ub.into(), step.into(), body_fn_bits.into()],
                        "scf_for",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfWhile => {
                // ScfWhile delegates to the runtime: full loop lowering requires
                // condition hoisting and break/continue analysis.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let cond_fn_bits = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let body_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_while_fn = self.backend.module.get_function("molt_scf_while").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_while_fn,
                        &[cond_fn_bits.into(), body_fn_bits.into()],
                        "scf_while",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfYield => {
                // ScfYield returns its operand value (or None if no operand).
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let val = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                };
                let scf_yield_fn = self.backend.module.get_function("molt_scf_yield").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(scf_yield_fn, &[val.into()], "scf_yield")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Deopt: transfer execution back to interpreter ──
            OpCode::Deopt => {
                let i64_ty = self.backend.context.i64_type();
                let frame_bits = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let deopt_fn = self
                    .backend
                    .module
                    .get_function("molt_deopt_transfer")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(deopt_fn, &[frame_bits.into()], "deopt")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // Exception region markers. LLVM still uses polling-based Molt
            // exceptions, but the runtime expects a handler frame to be
            // established around try regions so raise/catch semantics match
            // native and wasm.
            OpCode::TryStart => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let baseline = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "try_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let baseline_slot = self.build_entry_i64_alloca("try_baseline");
                self.backend
                    .builder
                    .build_store(baseline_slot, baseline)
                    .unwrap();
                self.try_stack_baselines.push(baseline_slot);
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, baseline);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::TryEnd => {
                if let Some(baseline_slot) = self.try_stack_baselines.pop() {
                    let exit_fn = self.ensure_runtime_i64_fn("molt_exception_stack_exit", 1);
                    let baseline_bits = self
                        .backend
                        .builder
                        .build_load(
                            self.backend.context.i64_type(),
                            baseline_slot,
                            "try_baseline_load",
                        )
                        .unwrap()
                        .into_int_value();
                    let result = self
                        .backend
                        .builder
                        .build_call(exit_fn, &[baseline_bits.into()], "try_exit")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }
            OpCode::IterNextUnboxed => {
                let iter_bits = self.materialize_dynbox_operand(op.operands[0]);
                let i64_ty = self.backend.context.i64_type();
                let val_ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "iter_next_unboxed_value")
                    .unwrap();
                let val_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(val_ptr, i64_ty, "iter_next_unboxed_value_ptr")
                    .unwrap();
                let iter_next_fn = self.ensure_runtime_i64_fn("molt_iter_next_unboxed", 2);
                let done_bits = self
                    .backend
                    .builder
                    .build_call(
                        iter_next_fn,
                        &[iter_bits.into(), val_ptr_bits.into()],
                        "iter_next_unboxed",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let value_bits = self
                    .backend
                    .builder
                    .build_load(i64_ty, val_ptr, "iter_next_unboxed_value_load")
                    .unwrap();
                if let Some(&value_id) = op.results.first() {
                    self.values.insert(value_id, value_bits);
                    self.value_types.insert(value_id, TirType::DynBox);
                }
                if let Some(&done_id) = op.results.get(1) {
                    self.values.insert(done_id, done_bits);
                    self.value_types.insert(done_id, TirType::DynBox);
                }
            }
            OpCode::ObjectNewBound | OpCode::ObjectNewBoundStack => {
                let Some(&class_id) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                let class_bits = self.materialize_dynbox_operand(class_id);
                let result = if let Some(AttrValue::Int(payload_size)) = op.attrs.get("value")
                    && *payload_size > 0
                {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound_sized", 2);
                    let size_bits = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*payload_size as u64, false);
                    self.backend
                        .builder
                        .build_call(
                            new_fn,
                            &[class_bits.into(), size_bits.into()],
                            "object_new_bound_sized",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                } else {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound", 1);
                    self.backend
                        .builder
                        .build_call(new_fn, &[class_bits.into()], "object_new_bound")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }

    // ── CheckedAdd: hardware-exact signed-overflow add (overflow_peel) ──
    //
    // A TOTAL function with two lanes:
    //
    // RAW lane (both operands proven overflow-safe raw-i64 carriers):
    // `(sum, flag) = llvm.sadd.with.overflow.i64` — LLVM's canonical
    // checked-arithmetic intrinsic. The sum is the wrapping i64 result,
    // observable ONLY on the flag=0 branch (the peel's CFG enforces this;
    // the slow loop is seeded from the PRE-iteration values). The flag is
    // an i1, consumed directly by CondBranch's `TirType::Bool` path.
    //
    // BOXED lane (any operand unproven — the v1 state on LLVM, whose
    // value-keyed RawI64Safe is a 47-bit-window contract that cannot carry
    // an unbounded accumulator): dispatch through `molt_add` with NaN-boxed
    // operands — BigInt-exact, so the sum can never silently wrap and the
    // flag is CONSTANT FALSE (the peel's slow path is correctly dead; same
    // semantics, no speedup until the RawI64Full lattice extension lands).
    fn emit_checked_add(&mut self, op: &crate::tir::ops::TirOp) {
        let sum_id = op.results[0];
        let flag_id = op.results[1];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let raw_lane = matches!(lhs_ty, TirType::I64)
            && matches!(rhs_ty, TirType::I64)
            && self.repr_facts.is_overflow_safe_int(lhs_id)
            && self.repr_facts.is_overflow_safe_int(rhs_id);
        if raw_lane {
            let lhs = self.resolve(lhs_id).into_int_value();
            let rhs = self.resolve(rhs_id).into_int_value();
            let i64_ty = self.backend.context.i64_type();
            let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.sadd.with.overflow")
                .expect("llvm.sadd.with.overflow intrinsic must exist");
            let decl = intrinsic
                .get_declaration(&self.backend.module, &[i64_ty.into()])
                .expect("llvm.sadd.with.overflow.i64 declaration must succeed");
            let pair = self
                .backend
                .builder
                .build_call(decl, &[lhs.into(), rhs.into()], "checked_add")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_struct_value();
            let sum = self
                .backend
                .builder
                .build_extract_value(pair, 0, "ca_sum")
                .unwrap();
            let flag = self
                .backend
                .builder
                .build_extract_value(pair, 1, "ca_of")
                .unwrap();
            self.values.insert(sum_id, sum);
            self.value_types.insert(sum_id, TirType::I64);
            self.values.insert(flag_id, flag);
            // i1 — CondBranch's `TirType::Bool` arm uses it directly as the
            // branch condition (no truthiness call, no NaN-box round-trip).
            self.value_types.insert(flag_id, TirType::Bool);
        } else {
            let lhs = self.resolve(lhs_id);
            let rhs = self.resolve(rhs_id);
            let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
            let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
            let sum = self.call_runtime_2("molt_add", lhs_i64.into(), rhs_i64.into());
            self.values.insert(sum_id, sum);
            self.value_types.insert(sum_id, TirType::DynBox);
            let false_flag = self.backend.context.bool_type().const_zero();
            self.values.insert(flag_id, false_flag.into());
            self.value_types.insert(flag_id, TirType::Bool);
        }
    }

    // ── CheckedMul: hardware-exact signed-overflow multiply (overflow_peel) ──
    //
    // A TOTAL function with two lanes, mirroring `emit_checked_add` exactly.
    //
    // RAW lane (both operands proven overflow-safe raw-i64 carriers):
    // `(prod, flag) = llvm.smul.with.overflow.i64` — LLVM's canonical
    // checked-multiply intrinsic (the multiply analogue of
    // `llvm.sadd.with.overflow`). The product is the wrapping i64 result,
    // observable ONLY on the flag=0 branch (the peel's CFG enforces this;
    // the slow loop is seeded from the PRE-iteration values). The flag is an
    // i1, consumed directly by CondBranch's `TirType::Bool` path.
    //
    // BOXED lane (any operand unproven — the v1 state on LLVM, whose
    // value-keyed RawI64Safe is a 47-bit-window contract that cannot carry an
    // unbounded accumulator): dispatch through `molt_mul` with NaN-boxed
    // operands — BigInt-exact, so the product can never silently wrap and the
    // flag is CONSTANT FALSE (the peel's slow path is correctly dead; same
    // semantics, no speedup until the RawI64Full lattice extension lands).
    fn emit_checked_mul(&mut self, op: &crate::tir::ops::TirOp) {
        let prod_id = op.results[0];
        let flag_id = op.results[1];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let raw_lane = matches!(lhs_ty, TirType::I64)
            && matches!(rhs_ty, TirType::I64)
            && self.repr_facts.is_overflow_safe_int(lhs_id)
            && self.repr_facts.is_overflow_safe_int(rhs_id);
        if raw_lane {
            let lhs = self.resolve(lhs_id).into_int_value();
            let rhs = self.resolve(rhs_id).into_int_value();
            let i64_ty = self.backend.context.i64_type();
            let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.smul.with.overflow")
                .expect("llvm.smul.with.overflow intrinsic must exist");
            let decl = intrinsic
                .get_declaration(&self.backend.module, &[i64_ty.into()])
                .expect("llvm.smul.with.overflow.i64 declaration must succeed");
            let pair = self
                .backend
                .builder
                .build_call(decl, &[lhs.into(), rhs.into()], "checked_mul")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_struct_value();
            let prod = self
                .backend
                .builder
                .build_extract_value(pair, 0, "cm_prod")
                .unwrap();
            let flag = self
                .backend
                .builder
                .build_extract_value(pair, 1, "cm_of")
                .unwrap();
            self.values.insert(prod_id, prod);
            self.value_types.insert(prod_id, TirType::I64);
            self.values.insert(flag_id, flag);
            // i1 — CondBranch's `TirType::Bool` arm uses it directly as the
            // branch condition (no truthiness call, no NaN-box round-trip).
            self.value_types.insert(flag_id, TirType::Bool);
        } else {
            let lhs = self.resolve(lhs_id);
            let rhs = self.resolve(rhs_id);
            let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
            let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
            let prod = self.call_runtime_2("molt_mul", lhs_i64.into(), rhs_i64.into());
            self.values.insert(prod_id, prod);
            self.value_types.insert(prod_id, TirType::DynBox);
            let false_flag = self.backend.context.bool_type().const_zero();
            self.values.insert(flag_id, false_flag.into());
            self.value_types.insert(flag_id, TirType::Bool);
        }
    }

    // ── Type-specialized binary arithmetic ──

    /// Emit a divisor-zero-guarded I64 division-family op (`/`, `//`, `%`).
    ///
    /// A raw machine `sdiv`/`srem` (or float divide) by zero is a SILENT
    /// miscompile: LLVM `sdiv x, 0` is poison (observed: a garbage NaN-box bit
    /// pattern instead of CPython's `ZeroDivisionError`). The native backend
    /// already guards this with an inline runtime zero-check; this mirrors that
    /// pattern for LLVM so all backends raise byte-identically.
    ///
    /// Shape (cold slow path so the non-zero hot path stays a straight-line raw
    /// divide — no perf regression vs the unguarded code):
    /// ```text
    ///   if rhs != 0 { fast: <raw divide>        }  ──┐
    ///   else        { slow: molt_<op>(box,box)  }  ──┤→ merge: phi
    /// ```
    /// `molt_floordiv`/`molt_mod`/`molt_div` set `ZeroDivisionError` for the
    /// zero divisor, so the slow path never returns normally; its (dead) result
    /// is still unboxed to the fast lane's carrier type to keep the phi
    /// well-typed.
    fn emit_i64_divrem_zero_guarded(
        &mut self,
        op: &crate::tir::ops::TirOp,
        name: &str,
        lhs_i: inkwell::values::IntValue<'ctx>,
        rhs_i: inkwell::values::IntValue<'ctx>,
    ) -> (BasicValueEnum<'ctx>, TirType) {
        let i64_ty = self.backend.context.i64_type();
        let zero = i64_ty.const_zero();
        let rhs_nonzero = self
            .backend
            .builder
            .build_int_compare(inkwell::IntPredicate::NE, rhs_i, zero, "rhs_nonzero")
            .unwrap();
        let current_fn = self.llvm_fn;
        let fast_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_fast");
        let slow_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_zero");
        let merge_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "divrem_merge");
        self.all_llvm_blocks.push(fast_bb);
        self.all_llvm_blocks.push(slow_bb);
        self.all_llvm_blocks.push(merge_bb);
        self.backend
            .builder
            .build_conditional_branch(rhs_nonzero, fast_bb, slow_bb)
            .unwrap();

        // ── Fast path: divisor proven non-zero here, raw machine divide. ──
        self.backend.builder.position_at_end(fast_bb);
        let (fast_val, out_ty): (BasicValueEnum<'ctx>, TirType) = match name {
            "div" => {
                // Python `/` on ints returns float (7 / 2 == 3.5).
                let f64_ty = self.backend.context.f64_type();
                let lhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(lhs_i, f64_ty, "div_lhs_f")
                    .unwrap();
                let rhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(rhs_i, f64_ty, "div_rhs_f")
                    .unwrap();
                let v = self
                    .backend
                    .builder
                    .build_float_div(lhs_f, rhs_f, "div_f")
                    .unwrap();
                (v.into(), TirType::F64)
            }
            "floordiv" => {
                // Python `//`: floor toward -inf. q = sdiv; r = srem;
                // if (r != 0 && (lhs ^ rhs) < 0) q -= 1.
                let one = i64_ty.const_int(1, false);
                let q = self
                    .backend
                    .builder
                    .build_int_signed_div(lhs_i, rhs_i, "fdiv_q")
                    .unwrap();
                let r = self
                    .backend
                    .builder
                    .build_int_signed_rem(lhs_i, rhs_i, "fdiv_r")
                    .unwrap();
                let r_ne_0 = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, r, zero, "r_ne_0")
                    .unwrap();
                let xor = self
                    .backend
                    .builder
                    .build_xor(lhs_i, rhs_i, "signs_xor")
                    .unwrap();
                let signs_differ = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "signs_differ")
                    .unwrap();
                let needs_adjust = self
                    .backend
                    .builder
                    .build_and(r_ne_0, signs_differ, "needs_adj")
                    .unwrap();
                let q_minus_1 = self.backend.builder.build_int_sub(q, one, "q_m1").unwrap();
                let q_m1_basic: BasicValueEnum<'ctx> = q_minus_1.into();
                let q_basic: BasicValueEnum<'ctx> = q.into();
                let adj = self
                    .backend
                    .builder
                    .build_select(needs_adjust, q_m1_basic, q_basic, "floordiv")
                    .unwrap();
                (adj, TirType::I64)
            }
            "mod" => {
                // Python `%`: result has the sign of the divisor.
                // r = srem; if (r != 0 && (r ^ rhs) < 0) r += rhs.
                let r = self
                    .backend
                    .builder
                    .build_int_signed_rem(lhs_i, rhs_i, "mod_r")
                    .unwrap();
                let r_ne_0 = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, r, zero, "mod_r_ne_0")
                    .unwrap();
                let xor = self
                    .backend
                    .builder
                    .build_xor(r, rhs_i, "mod_signs_xor")
                    .unwrap();
                let signs_differ = self
                    .backend
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "mod_signs_differ")
                    .unwrap();
                let needs_adjust = self
                    .backend
                    .builder
                    .build_and(r_ne_0, signs_differ, "mod_adj")
                    .unwrap();
                let r_plus_rhs = self
                    .backend
                    .builder
                    .build_int_add(r, rhs_i, "mod_adjusted")
                    .unwrap();
                let r_adj_basic: BasicValueEnum<'ctx> = r_plus_rhs.into();
                let r_basic: BasicValueEnum<'ctx> = r.into();
                let result = self
                    .backend
                    .builder
                    .build_select(needs_adjust, r_adj_basic, r_basic, "pymod")
                    .unwrap();
                (result, TirType::I64)
            }
            other => unreachable!("emit_i64_divrem_zero_guarded called with {other:?}"),
        };
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let fast_pred = self.backend.builder.get_insert_block().unwrap();

        // ── Slow path: divisor == 0 ⇒ boxed runtime raises ZeroDivisionError. ──
        self.backend.builder.position_at_end(slow_bb);
        let rt_name = match name {
            "div" => "molt_div",
            "floordiv" => "molt_floordiv",
            "mod" => "molt_mod",
            other => unreachable!("emit_i64_divrem_zero_guarded called with {other:?}"),
        };
        let boxed = self
            .call_runtime_2_boxed(rt_name, op.operands[0], op.operands[1])
            .into_int_value();
        // The runtime raised; this value is unreachable-but-typed. Convert the
        // DynBox bits to the fast lane's carrier so the phi types line up.
        let slow_val: BasicValueEnum<'ctx> = match out_ty {
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(boxed, self.backend.context.f64_type(), "div_zero_f64")
                .unwrap(),
            _ => unbox_dynbox_to_param_ty_with_builder(
                &self.backend.builder,
                self.backend.context,
                boxed,
                &out_ty,
            )
            .into(),
        };
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let slow_pred = self.backend.builder.get_insert_block().unwrap();

        // ── Merge. ──
        self.backend.builder.position_at_end(merge_bb);
        let phi_ty: inkwell::types::BasicTypeEnum<'ctx> = match out_ty {
            TirType::F64 => self.backend.context.f64_type().into(),
            _ => i64_ty.into(),
        };
        let phi = self.backend.builder.build_phi(phi_ty, "divrem").unwrap();
        phi.add_incoming(&[(&fast_val, fast_pred), (&slow_val, slow_pred)]);
        (phi.as_basic_value(), out_ty)
    }

    fn emit_binary_arith(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let fast_math = has_attr(op, "fast_math");
        // When the `no_signed_wrap` attribute is set by a TIR analysis pass
        // (for example range_devirt for bounded induction increments), we
        // emit nsw-flagged integer instructions.  This enables LLVM's:
        //  - Strength reduction (e.g. `i * 4` → `i << 2` with guaranteed no wrap)
        //  - SCEV (Scalar Evolution) for loop trip count analysis
        //  - Loop vectorization with known induction variable ranges
        let nsw = has_attr(op, "no_signed_wrap");

        // Overflow-safety gate (the structural fix for the LLVM int-overflow
        // miscompile): a raw machine `add`/`sub`/`mul` may only be emitted when
        // the plan proves the *result* is an overflow-safe exact-i64 carrier
        // (interval-proven not to wrap a signed i64). `TirType::I64` alone is a
        // *semantic* int — `type_refine` assigns `add(I64, I64) -> I64` with no
        // overflow proof — so gating on the type would silently wrap and then
        // truncate to 47 bits at box time. Names outside the overflow-safe set
        // fall through to the boxed runtime path (`molt_add`/`molt_sub`/
        // `molt_mul`), which is BigInt-correct, mirroring the native and WASM
        // backends.
        let int_overflow_safe = self.repr_facts.is_overflow_safe_int(result_id);

        let (val, out_ty) = match (&lhs_ty, &rhs_ty, name) {
            // I64 + I64 -> I64 (direct machine instruction).
            // When `nsw` is set, use build_int_nsw_add to tell LLVM the
            // result is guaranteed not to overflow as a signed i64.
            (TirType::I64, TirType::I64, "add") if int_overflow_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_add(lhs_i, rhs_i, "add")
                } else {
                    self.backend.builder.build_int_add(lhs_i, rhs_i, "add")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "sub") if int_overflow_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_sub(lhs_i, rhs_i, "sub")
                } else {
                    self.backend.builder.build_int_sub(lhs_i, rhs_i, "sub")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "mul") if int_overflow_safe => {
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let v = if nsw {
                    self.backend.builder.build_int_nsw_mul(lhs_i, rhs_i, "mul")
                } else {
                    self.backend.builder.build_int_mul(lhs_i, rhs_i, "mul")
                }
                .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "div" | "floordiv" | "mod") => {
                // A raw machine divide by zero is poison (LLVM) — route through a
                // divisor-zero guard so a zero divisor raises ZeroDivisionError
                // via the boxed runtime instead of silently yielding garbage.
                self.emit_i64_divrem_zero_guarded(
                    op,
                    name,
                    lhs.into_int_value(),
                    rhs.into_int_value(),
                )
            }

            // F64 + F64 -> F64 (direct machine instruction).
            // When `fast_math = true` is set on the TIR op (injected by the
            // fast_math annotation pass), we apply LLVM's full fast-math flag
            // set to the emitted instruction via `InstructionValue::set_fast_math_flags`.
            (TirType::F64, TirType::F64, "add") => {
                let v = self
                    .backend
                    .builder
                    .build_float_add(lhs.into_float_value(), rhs.into_float_value(), "fadd")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "sub") => {
                let v = self
                    .backend
                    .builder
                    .build_float_sub(lhs.into_float_value(), rhs.into_float_value(), "fsub")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mul") => {
                let v = self
                    .backend
                    .builder
                    .build_float_mul(lhs.into_float_value(), rhs.into_float_value(), "fmul")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "div") => {
                let v = self
                    .backend
                    .builder
                    .build_float_div(lhs.into_float_value(), rhs.into_float_value(), "fdiv")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mod") => {
                let v = self
                    .backend
                    .builder
                    .build_float_rem(lhs.into_float_value(), rhs.into_float_value(), "fmod")
                    .unwrap();
                if fast_math && let Some(instr) = v.as_instruction() {
                    instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                }
                (v.into(), TirType::F64)
            }

            // Everything else: call runtime (DynBox dispatch).
            //
            // The boxed slow path must honour the in-place dunder protocol for
            // augmented assignment. An augassign op reaches here either as a
            // first-class InplaceAdd/InplaceSub/InplaceMul opcode OR as a
            // Copy-carried `inplace_floordiv`/`inplace_mod`/`inplace_pow`/... with
            // its `_original_kind` preserved (the lower_preserved arms route those
            // here via emit_binary_arith with the binary `name`). In both cases
            // CPython requires `molt_inplace_<op>` — which tries `__i<op>__`
            // BEFORE the binary `__op__`/`__rop__` chain — not the binary
            // `molt_<op>`. Selecting `molt_<op>` here was a silent miscompile:
            // a class defining only `__iadd__`/`__ifloordiv__`/… had its `+=`/
            // `//=` routed to the binary fallback dunder. The fast int/float lanes
            // above stay on the binary instruction because builtin numerics have
            // no in-place dunder (so the result is byte-identical there).
            _ => {
                let is_inplace = opcode_uses_boxed_runtime_inplace_dispatch_table(op.opcode)
                    || op
                        .attrs
                        .get("_original_kind")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .is_some_and(|k| k.starts_with("inplace_"));
                let rt_name = match (name, is_inplace) {
                    ("add", false) => "molt_add",
                    ("add", true) => "molt_inplace_add",
                    ("sub", false) => "molt_sub",
                    ("sub", true) => "molt_inplace_sub",
                    ("mul", false) => "molt_mul",
                    ("mul", true) => "molt_inplace_mul",
                    ("div", false) => "molt_div",
                    ("div", true) => "molt_inplace_div",
                    ("floordiv", false) => "molt_floordiv",
                    ("floordiv", true) => "molt_inplace_floordiv",
                    ("mod", false) => "molt_mod",
                    ("mod", true) => "molt_inplace_mod",
                    ("pow", false) => "molt_pow",
                    ("pow", true) => "molt_inplace_pow",
                    _ => unreachable!("unknown arith op: {}", name),
                };
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Type-specialized comparison ──

    fn emit_comparison(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) => {
                use inkwell::IntPredicate;
                let pred = match name {
                    "eq" => IntPredicate::EQ,
                    "ne" => IntPredicate::NE,
                    "lt" => IntPredicate::SLT,
                    "le" => IntPredicate::SLE,
                    "gt" => IntPredicate::SGT,
                    "ge" => IntPredicate::SGE,
                    _ => unreachable!(),
                };
                let v = self
                    .backend
                    .builder
                    .build_int_compare(pred, lhs.into_int_value(), rhs.into_int_value(), name)
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            (TirType::F64, TirType::F64) => {
                use inkwell::FloatPredicate;
                let pred = match name {
                    "eq" => FloatPredicate::OEQ,
                    "ne" => FloatPredicate::ONE,
                    "lt" => FloatPredicate::OLT,
                    "le" => FloatPredicate::OLE,
                    "gt" => FloatPredicate::OGT,
                    "ge" => FloatPredicate::OGE,
                    _ => unreachable!(),
                };
                let v = self
                    .backend
                    .builder
                    .build_float_compare(pred, lhs.into_float_value(), rhs.into_float_value(), name)
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            _ => {
                let rt_name = match name {
                    "eq" => "molt_eq",
                    "ne" => "molt_ne",
                    "lt" => "molt_lt",
                    "le" => "molt_le",
                    "gt" => "molt_gt",
                    "ge" => "molt_ge",
                    _ => unreachable!(),
                };
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Identity (is / is not) ──

    fn emit_identity(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let lhs = self.resolve(op.operands[0]);
        let rhs = self.resolve(op.operands[1]);
        let lhs_i64 = self.ensure_i64(lhs);
        let rhs_i64 = self.ensure_i64(rhs);
        let cmp = self
            .backend
            .builder
            .build_int_compare(inkwell::IntPredicate::EQ, lhs_i64, rhs_i64, "is")
            .unwrap();
        let val: BasicValueEnum<'ctx> = if op.opcode == OpCode::IsNot {
            self.backend
                .builder
                .build_not(cmp, "is_not")
                .unwrap()
                .into()
        } else {
            cmp.into()
        };
        self.values.insert(result_id, val);
        self.value_types.insert(result_id, TirType::Bool);
    }

    // ── Containment (in / not in) ──

    fn emit_containment(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        // `molt_contains(container, item)`. The membership op's operands are
        // [container, item] (matching the native `contains` arm and the SimpleIR
        // `contains`/`in`/`not_in` convention), so they must be passed in that
        // order — swapping them makes `3 in [1, 2, 3]` call `molt_contains(3,
        // [1, 2, 3])`, reporting `argument of type 'int' is not iterable`.
        let val = self.call_runtime_2_boxed("molt_contains", op.operands[0], op.operands[1]);
        let final_val = if op.opcode == OpCode::NotIn {
            // Invert the boolean result from molt_contains
            let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
            let item_i64 = self.ensure_i64(val);
            let truthy = self
                .backend
                .builder
                .build_call(truthy_fn, &[item_i64.into()], "truthy")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            let as_bool = self
                .backend
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    truthy.into_int_value(),
                    self.backend.context.i64_type().const_int(0, false),
                    "not_in",
                )
                .unwrap();
            as_bool.into()
        } else {
            val
        };
        self.values.insert(result_id, final_val);
        let out_ty = if op.opcode == OpCode::NotIn {
            TirType::Bool
        } else {
            TirType::DynBox
        };
        self.value_types.insert(result_id, out_ty);
    }

    // ── Bitwise ops ──

    fn emit_bitwise(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self
            .value_types
            .get(&lhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let rhs_ty = self
            .value_types
            .get(&rhs_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        // Overflow / shift-validity gate for the raw I64 lane. `bit_and`/
        // `bit_or`/`bit_xor` are unconditionally sound on raw i64 (the result
        // bits are a subset of the operand bits — no overflow, no UB), so they
        // always take the machine lane. `lshift`/`rshift` are NOT: a raw
        // `shl`/`ashr` whose count is `>= 64` is LLVM poison, and a `<<` result
        // that exceeds i64 wraps then truncates at box time — the silent
        // integer miscompile. The shift raw lane is therefore admitted only when
        // the plan proves the *result* is an overflow-safe exact-i64 carrier
        // (`RawI64Safe`), which the value-range seed grants for a shift ONLY when
        // its count is range-proven in `[0, 63]` AND the result fits the inline
        // window (single source of truth, shared with native/WASM). An unproven
        // shift falls through to the boxed `molt_lshift`/`molt_rshift` runtime —
        // BigInt-correct, negative-count `ValueError`-correct, huge-count
        // `OverflowError`-correct — exactly mirroring `emit_binary_arith`'s
        // `int_overflow_safe` gate and the native backend (which boxes every
        // shift).
        let shift_overflow_safe = self.repr_facts.is_overflow_safe_int(result_id);
        let raw_i64_lane_ok = match name {
            "bit_and" | "bit_or" | "bit_xor" => true,
            "lshift" | "rshift" => shift_overflow_safe,
            _ => unreachable!("emit_bitwise got non-bitwise name: {name}"),
        };
        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) if raw_i64_lane_ok => {
                let v = match name {
                    "bit_and" => self
                        .backend
                        .builder
                        .build_and(lhs.into_int_value(), rhs.into_int_value(), "band")
                        .unwrap(),
                    "bit_or" => self
                        .backend
                        .builder
                        .build_or(lhs.into_int_value(), rhs.into_int_value(), "bor")
                        .unwrap(),
                    "bit_xor" => self
                        .backend
                        .builder
                        .build_xor(lhs.into_int_value(), rhs.into_int_value(), "bxor")
                        .unwrap(),
                    "lshift" => self
                        .backend
                        .builder
                        .build_left_shift(lhs.into_int_value(), rhs.into_int_value(), "shl")
                        .unwrap(),
                    "rshift" => self
                        .backend
                        .builder
                        .build_right_shift(lhs.into_int_value(), rhs.into_int_value(), true, "shr")
                        .unwrap(),
                    _ => unreachable!(),
                };
                (v.into(), TirType::I64)
            }
            _ => {
                // Honour the in-place dunder for `<<=`/`>>=` (and the inplace
                // bitwise family). A Copy-carried `inplace_lshift`/`inplace_rshift`
                // /`inplace_bit_*` reaches the bitwise emitter with the BINARY
                // `name` ("lshift"/"bit_or"/…) but must dispatch the boxed slow
                // path to `molt_inplace_<op>` so `__ilshift__`/`__ior__`/… is
                // tried before the binary `__op__`/`__rop__` chain. The fast int
                // lane above is unchanged (builtin int has no in-place dunder).
                let is_inplace = op
                    .attrs
                    .get("_original_kind")
                    .and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .is_some_and(|k| k.starts_with("inplace_"));
                let rt_name = if is_inplace {
                    format!("molt_inplace_{}", name)
                } else {
                    format!("molt_{}", name)
                };
                // The runtime bitwise entries take NaN-BOXED operands. A raw
                // `TirType::I64` operand (e.g. the `4` in `x <<= 4`) must be boxed
                // via `materialize_dynbox_bits`, NOT passed through `ensure_i64`
                // (which forwards the raw i64 bit pattern — the runtime then
                // mis-reads `4` as the subnormal float 2e-323). This mirrors
                // `emit_binary_arith`'s boxed fallback; using `ensure_i64` here
                // was a latent miscompile of `<<`/`>>`/bitwise on a raw-int
                // operand, now exposed by the `<<=`/`>>=` in-place dunder path.
                let lhs_i64 = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_i64 = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let v = self.call_runtime_2(&rt_name, lhs_i64.into(), rhs_i64.into());
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Unary ops ──

    fn emit_unary(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&operand_ty, name) {
            (TirType::I64, "neg") => {
                let v = self
                    .backend
                    .builder
                    .build_int_neg(operand.into_int_value(), "neg")
                    .unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::F64, "neg") => {
                let v = self
                    .backend
                    .builder
                    .build_float_neg(operand.into_float_value(), "fneg")
                    .unwrap();
                (v.into(), TirType::F64)
            }
            (TirType::Bool, "not") => {
                let v = self
                    .backend
                    .builder
                    .build_not(operand.into_int_value(), "not")
                    .unwrap();
                (v.into(), TirType::Bool)
            }
            (TirType::I64, "invert") => {
                let v = self
                    .backend
                    .builder
                    .build_not(operand.into_int_value(), "invert")
                    .unwrap();
                (v.into(), TirType::I64)
            }
            _ => {
                let rt_name = match name {
                    "neg" => "molt_neg",
                    "not" => "molt_not",
                    "invert" => "molt_invert",
                    _ => unreachable!(),
                };
                let op_i64 = self.ensure_i64(operand);
                let func = self.backend.module.get_function(rt_name).unwrap();
                let v = self
                    .backend
                    .builder
                    .build_call(func, &[op_i64.into()], name)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                (v, TirType::DynBox)
            }
        };

        self.values.insert(result_id, val);
        self.value_types.insert(result_id, out_ty);
    }

    // ── Representation authority ──

    /// Effective semantic carrier type for a block argument (phi).
    ///
    /// The carrier is reconciled with the single `is_overflow_safe_int`
    /// representation authority (`repr_by_value`'s `RawI64Safe` view), which is
    /// derived from the value-range proof shared with native/WASM. Two
    /// directions, both keyed on that same authority so `value_types` can never
    /// diverge from the `Repr` the raw-i64 lanes gate on:
    ///
    ///   * **Demotion** `I64 -> DynBox`: a `TirType::I64` phi the plan does NOT
    ///     prove overflow-safe is carried `DynBox` (NaN-boxed). `type_refine`
    ///     assigns `add(I64, I64) -> I64` with no overflow proof, so an unproven
    ///     i64 accumulator must stay boxed across the back-edge instead of
    ///     unboxing a runtime BigInt into a truncating 47-bit payload.
    ///   * **Promotion** `DynBox -> I64`: a `DynBox`-declared phi the plan DOES
    ///     prove overflow-safe is carried as a raw `I64`. This is the masked
    ///     back-edge accumulator (`s = (s << 1) & MASK`): the value-range phi
    ///     narrowing proves `s` fits the inline window, so `is_overflow_safe_int`
    ///     mints `RawI64Safe` for it — but `type_refine` (which runs without that
    ///     value-range fact) left the phi `DynBox`. Without this promotion the
    ///     phi carries boxed, so the in-loop `<<`/`&` see a `DynBox` operand and
    ///     bail to the boxed `molt_lshift`/`molt_bit_and` runtime even though the
    ///     raw lane was proven legal — defeating the whole narrowing. The phi
    ///     incoming edges are reconciled by `coerce_to_tir_type`, which unboxes a
    ///     boxed incoming (`molt_int_from_i64` / a boxed back-edge value) into the
    ///     raw i64 the I64 phi slot expects. The promotion is sound because
    ///     `is_overflow_safe_int` is granted ONLY for values a value-range proof
    ///     places entirely within the inline-int47 window (so a heap BigInt can
    ///     never reach the raw slot); it is restricted to a `DynBox` declared
    ///     type so a non-integer carrier (`Str`/`F64`/container) is never
    ///     reinterpreted as i64.
    fn effective_block_arg_type(&self, id: ValueId, declared: &TirType) -> TirType {
        let overflow_safe = self.repr_facts.is_overflow_safe_int(id);
        match declared {
            TirType::I64 if !overflow_safe => TirType::DynBox,
            TirType::DynBox if overflow_safe => TirType::I64,
            _ => declared.clone(),
        }
    }

    /// Resolve the specialized `len` runtime function for a container operand.
    ///
    /// The container dispatch kind is taken from the shared
    /// `ScalarRepresentationPlan` (the same authority the native/WASM/Luau
    /// backends consult). When the plan has no fact for this value — for example
    /// a pipeline-introduced temporary with no stable SimpleIR name — we fall
    /// back to the refined `TirType`, which the plan itself derives from
    /// (`ScalarRepresentationFact::container_kind`), so the two never disagree
    /// where both speak.
    fn container_len_fn(&self, operand_id: ValueId) -> &'static str {
        use crate::repr::ContainerKind;
        if let Some(kind) = self.repr_facts.container_kind(operand_id) {
            return match kind {
                ContainerKind::List => "molt_len_list",
                ContainerKind::Str => "molt_len_str",
                ContainerKind::Dict => "molt_len_dict",
                ContainerKind::Tuple => "molt_len_tuple",
                ContainerKind::Set => "molt_len_set",
            };
        }
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        match operand_ty {
            TirType::List(_) => "molt_len_list",
            TirType::Str => "molt_len_str",
            TirType::Dict(_, _) => "molt_len_dict",
            TirType::Tuple(_) => "molt_len_tuple",
            TirType::Set(_) => "molt_len_set",
            _ => "molt_len",
        }
    }

    // ── Box / Unbox ──

    /// NaN-box a raw signed `i64`, promoting to a heap BigInt when the value
    /// does not fit the 47-bit inline payload.
    ///
    /// The inline integer representation is a sign-extended 47-bit payload
    /// (range `[-(1<<46), (1<<46)-1]`). An unconditional `raw & INT_MASK | TAG`
    /// silently truncates any value outside that range to 47 bits — the LLVM
    /// integer-overflow miscompile this fixes. Instead we emit a single
    /// fits-inline range check: on the hot path (fits) we box inline; on the
    /// cold path we call `molt_int_from_i64`. This mirrors the native backend's
    /// `ensure_boxed_overflow_safe` and the WASM backend's
    /// `emit_inline_int_range_check` + runtime fallback.
    ///
    /// LLVM's range analysis (SCEV / known-bits) folds the branch away whenever
    /// it can prove `raw` fits inline (e.g. bounded loop induction variables and
    /// constants), so the check is free on values that are statically small.
    ///
    /// This form splits the current block, so callers that must keep the boxed
    /// value as a single SSA value in a fixed block (phi-incoming
    /// materialization, function-return coercion) use [`Self::box_i64_branchless`]
    /// instead.
    fn box_i64_overflow_safe(
        &self,
        raw: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        box_i64_overflow_safe_with_builder(
            &self.backend.builder,
            self.backend.context,
            &self.backend.module,
            self.llvm_fn,
            raw,
        )
    }

    /// Branchless overflow-safe integer box: a single `molt_int_from_i64` call
    /// that yields one SSA value and never alters control flow. Used where the
    /// boxed value must be a single value in a fixed block (phi-incoming
    /// materialization, function-return coercion). `molt_int_from_i64` returns
    /// the inline NaN-box for values that fit the 47-bit payload and a heap
    /// BigInt otherwise, so the result matches `box_i64_overflow_safe`.
    fn box_i64_branchless(
        &self,
        raw: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let from_i64_fn = self.ensure_runtime_i64_fn("molt_int_from_i64", 1);
        self.backend
            .builder
            .build_call(from_i64_fn, &[raw.into()], "molt_int_from_i64")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    fn materialize_dynbox_bits(
        &self,
        operand: BasicValueEnum<'ctx>,
        operand_ty: &TirType,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        match operand_ty {
            TirType::I64 => {
                let raw = self.ensure_i64(operand);
                self.box_i64_overflow_safe(raw)
            }
            TirType::Bool => {
                let raw = match operand {
                    BasicValueEnum::IntValue(iv) if iv.get_type().get_bit_width() == 1 => self
                        .backend
                        .builder
                        .build_int_z_extend(iv, i64_ty, "zext_bool")
                        .unwrap(),
                    _ => self.ensure_i64(operand),
                };
                self.backend
                    .builder
                    .build_or(
                        raw,
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_BOOL, false),
                        "box_bool",
                    )
                    .unwrap()
            }
            TirType::None => i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(operand, i64_ty, "f64_to_i64")
                .unwrap()
                .into_int_value(),
            TirType::DynBox
            | TirType::BigInt
            | TirType::Str
            | TirType::Bytes
            | TirType::List(_)
            | TirType::Dict(_, _)
            | TirType::Iterator(_)
            | TirType::Set(_)
            | TirType::Tuple(_)
            | TirType::UserClass(_)
            | TirType::Ptr(_)
            | TirType::Func(_)
            | TirType::Box(_)
            | TirType::Union(_)
            | TirType::Never => self.ensure_i64(operand),
        }
    }

    fn materialize_dynbox_operand(&self, operand_id: ValueId) -> inkwell::values::IntValue<'ctx> {
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        self.materialize_dynbox_bits(operand, &operand_ty)
    }

    fn build_entry_i64_alloca(&self, name: &str) -> inkwell::values::PointerValue<'ctx> {
        let builder = self.backend.context.create_builder();
        let current_fn = self
            .backend
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_parent())
            .expect("llvm function missing while allocating try baseline");
        let entry = current_fn
            .get_first_basic_block()
            .expect("llvm function missing entry block");
        if let Some(first_instr) = entry.get_first_instruction() {
            builder.position_before(&first_instr);
        } else {
            builder.position_at_end(entry);
        }
        builder
            .build_alloca(self.backend.context.i64_type(), name)
            .unwrap()
    }

    fn call_runtime_2_boxed(
        &self,
        name: &str,
        lhs_id: ValueId,
        rhs_id: ValueId,
    ) -> BasicValueEnum<'ctx> {
        let func = self
            .backend
            .module
            .get_function(name)
            .unwrap_or_else(|| panic!("Runtime function '{}' not declared", name));
        let lhs_i64 = self.materialize_dynbox_operand(lhs_id);
        let rhs_i64 = self.materialize_dynbox_operand(rhs_id);
        self.backend
            .builder
            .build_call(func, &[lhs_i64.into(), rhs_i64.into()], name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    fn emit_box(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);
        let operand_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);

        let boxed: BasicValueEnum<'ctx> = self.materialize_dynbox_bits(operand, &operand_ty).into();

        self.values.insert(result_id, boxed);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    fn emit_unbox(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);

        // Determine target type from attrs or result type hint.
        let target_ty = if let Some(AttrValue::Str(ty_name)) = op.attrs.get("type") {
            match ty_name.as_str() {
                "i64" => TirType::I64,
                "f64" => TirType::F64,
                "bool" => TirType::Bool,
                _ => TirType::DynBox,
            }
        } else {
            TirType::I64 // default unbox target
        };

        let i64_ty = self.backend.context.i64_type();
        let raw = self.ensure_i64(operand);

        let unboxed: BasicValueEnum<'ctx> = match &target_ty {
            TirType::I64 => {
                // Extract payload: sign-extend from 47 bits
                let masked = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "payload")
                    .unwrap();
                // Sign extension: if bit 46 is set, fill upper bits
                let sign_bit = self
                    .backend
                    .builder
                    .build_and(
                        raw,
                        i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                        "sign_bit",
                    )
                    .unwrap();
                let is_neg = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        sign_bit,
                        i64_ty.const_int(0, false),
                        "is_neg",
                    )
                    .unwrap();
                let sign_extend = i64_ty.const_int(!nanbox::INT_MASK, false);
                let extended = self
                    .backend
                    .builder
                    .build_or(masked, sign_extend, "sign_extended")
                    .unwrap();
                let extended_basic: inkwell::values::BasicValueEnum = extended.into();
                let masked_basic: inkwell::values::BasicValueEnum = masked.into();

                self.backend
                    .builder
                    .build_select(is_neg, extended_basic, masked_basic, "unbox_i64")
                    .unwrap()
            }
            TirType::F64 => {
                // Bitcast i64 back to f64.
                let f64_ty = self.backend.context.f64_type();
                self.backend
                    .builder
                    .build_bit_cast(raw, f64_ty, "unbox_f64")
                    .unwrap()
            }
            TirType::Bool => {
                // Extract lowest bit
                let one = i64_ty.const_int(1, false);
                let bit = self
                    .backend
                    .builder
                    .build_and(raw, one, "bool_bit")
                    .unwrap();
                let bool_val = self
                    .backend
                    .builder
                    .build_int_truncate(bit, self.backend.context.bool_type(), "unbox_bool")
                    .unwrap();
                bool_val.into()
            }
            _ => raw.into(),
        };

        self.values.insert(result_id, unboxed);
        self.value_types.insert(result_id, target_ty);
    }

    // ── Terminators ──

    fn lower_terminator(&mut self, source_block: BlockId, term: &Terminator) {
        match term {
            Terminator::Branch { target, args } => {
                let target_bb = self.block_map[target];
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(source_block, current_bb, *target, "branch", args);
                self.record_llvm_edge(current_bb, target_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(target_bb)
                    .unwrap();
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let cond_val = self.resolve(*cond);
                let cond_ty = self
                    .value_types
                    .get(cond)
                    .cloned()
                    .unwrap_or(TirType::DynBox);

                // Convert condition to i1.
                let cond_i1 = match &cond_ty {
                    TirType::Bool => cond_val.into_int_value(),
                    TirType::I64 => self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            cond_val.into_int_value(),
                            self.backend.context.i64_type().const_int(0, false),
                            "cond_i1",
                        )
                        .unwrap(),
                    _ => {
                        // DynBox: call molt_is_truthy
                        let cond_i64 = self.ensure_i64(cond_val);
                        let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                        let result = self
                            .backend
                            .builder
                            .build_call(truthy_fn, &[cond_i64.into()], "truthy")
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic();
                        self.backend
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                result.into_int_value(),
                                self.backend.context.i64_type().const_int(0, false),
                                "cond_i1",
                            )
                            .unwrap()
                    }
                };

                let then_bb = self.block_map[then_block];
                let else_bb = self.block_map[else_block];

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *then_block,
                    "then-edge",
                    then_args,
                );
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *else_block,
                    "else-edge",
                    else_args,
                );
                self.record_llvm_edge(current_bb, then_bb);
                self.record_llvm_edge(current_bb, else_bb);

                let branch_inst = self
                    .backend
                    .builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                // Attach PGO branch weight metadata when profile data is available.
                // The weights vector is consumed sequentially: each CondBranch
                // pops two values (true_weight, false_weight).
                if let Some(ref weights) = self.pgo_branch_weights {
                    let idx = self.pgo_weight_index;
                    if idx + 1 < weights.len() {
                        let true_weight = weights[idx];
                        let false_weight = weights[idx + 1];
                        self.pgo_weight_index = idx + 2;

                        // Build !prof metadata: !{!"branch_weights", i32 T, i32 F}
                        // inkwell exposes `set_metadata(MetadataValue, kind_id)` on
                        // InstructionValue, and `metadata_node` / `metadata_string`
                        // on Context. The "prof" metadata kind ID is obtained via
                        // `context.get_kind_id("prof")`.
                        //
                        // However, inkwell's `metadata_node` API expects
                        // `&[BasicMetadataValueEnum]` which cannot hold a
                        // `MetadataValue` (the "branch_weights" string). The LLVM C
                        // API call `LLVMMDNode` with mixed operand types is not
                        // exposed through inkwell's safe wrapper. To attach !prof
                        // metadata correctly, a raw `llvm-sys` call is needed:
                        //
                        //   use llvm_sys::core::*;
                        //   let prof_kind = LLVMGetMDKindIDInContext(ctx, "prof", 4);
                        //   let bw_str = LLVMMDStringInContext(ctx, "branch_weights", 14);
                        //   let t_val = LLVMConstInt(LLVMInt32TypeInContext(ctx), true_weight, 0);
                        //   let f_val = LLVMConstInt(LLVMInt32TypeInContext(ctx), false_weight, 0);
                        //   let md_ops = [bw_str, t_val, f_val];
                        //   let md_node = LLVMMDNodeInContext(ctx, md_ops.as_ptr(), 3);
                        //   LLVMSetMetadata(branch_inst, prof_kind, md_node);
                        //
                        // This is deferred until we add `llvm-sys` as a direct
                        // dependency (currently accessed indirectly via inkwell).
                        // The PGO data is loaded and indexed correctly; only the
                        // final metadata attachment step requires the raw API.
                        let _ = (branch_inst, true_weight, false_weight);
                    }
                }
            }
            Terminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                let switch_val = self.resolve(*value);
                let switch_int = self.ensure_i64(switch_val);
                let default_bb = self.block_map[default];

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *default,
                    "switch-default",
                    default_args,
                );
                self.record_llvm_edge(current_bb, default_bb);

                let mut switch_cases: Vec<_> = Vec::with_capacity(cases.len());
                for (case_val, target, args) in cases {
                    let case_const = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*case_val as u64, *case_val < 0);
                    let target_bb = self.block_map[target];
                    self.record_branch_args(source_block, current_bb, *target, "switch-case", args);
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((case_const, target_bb));
                }

                self.backend
                    .builder
                    .build_switch(switch_int, default_bb, &switch_cases)
                    .unwrap();
            }
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                // Generator/coroutine `_poll` dispatch.  The saved resume state
                // is restored by the runtime across the suspend boundary, so the
                // dispatch value is read from the frame header here (not an SSA
                // value): `molt_obj_get_state(self)`.  State 0 (initial entry)
                // takes the `default` edge; every saved resume state dispatches
                // to the matching suspend op's REAL resume continuation block.
                //
                // This is the first-class replacement for the old synthetic
                // `state_resume_*` block machinery: the switch targets are the
                // real TIR blocks the main lowering loop emits (so their phis are
                // the phis the SSA pass placed), and `record_branch_args` supplies
                // each dispatch edge's incomings, which `finalize_phis` fills.
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let get_state_fn = self.ensure_runtime_i64_fn("molt_obj_get_state", 1);
                let state_val = self
                    .backend
                    .builder
                    .build_call(get_state_fn, &[self_bits.into()], "state_dispatch_state")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();

                let default_bb = self.block_map[default];
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    current_bb,
                    *default,
                    "state-dispatch-default",
                    default_args,
                );
                self.record_llvm_edge(current_bb, default_bb);

                let mut switch_cases: Vec<_> = Vec::with_capacity(cases.len());
                for (state_id, target, args) in cases {
                    let case_const = i64_ty.const_int(*state_id as u64, *state_id < 0);
                    let target_bb = self.block_map[target];
                    self.record_branch_args(
                        source_block,
                        current_bb,
                        *target,
                        "state-dispatch-case",
                        args,
                    );
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((case_const, target_bb));
                }

                self.backend
                    .builder
                    .build_switch(state_val, default_bb, &switch_cases)
                    .unwrap();
            }
            Terminator::Return { values } => {
                if values.is_empty() {
                    // Return void-equivalent (None sentinel for Python functions)
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let ret_val = self.backend.context.i64_type().const_int(none_bits, false);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else if values.len() == 1 {
                    let val = self.resolve(values[0]);
                    let ret_ty = lower_type(self.backend.context, &self.func.return_type);
                    let val_ty = self
                        .value_types
                        .get(&values[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    let current_bb = self
                        .backend
                        .builder
                        .get_insert_block()
                        .expect("return must be lowered inside a basic block");
                    let ret_val =
                        self.coerce_to_tir_type(val, &val_ty, &self.func.return_type, current_bb);
                    let ret_val = self.coerce_to_type(ret_val, ret_ty, current_bb);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else {
                    // Multi-value return: pack into struct.
                    // For now, just return the first value.
                    let val = self.resolve(values[0]);
                    let ret_ty = lower_type(self.backend.context, &self.func.return_type);
                    let val_ty = self
                        .value_types
                        .get(&values[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    let current_bb = self
                        .backend
                        .builder
                        .get_insert_block()
                        .expect("return must be lowered inside a basic block");
                    let ret_val =
                        self.coerce_to_tir_type(val, &val_ty, &self.func.return_type, current_bb);
                    let ret_val = self.coerce_to_type(ret_val, ret_ty, current_bb);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                }
            }
            Terminator::Unreachable => {
                self.backend.builder.build_unreachable().unwrap();
            }
        }
    }

    // ── Phi node wiring ──

    /// Record that an actually emitted branch passes `args` to `target`.
    fn record_branch_args(
        &mut self,
        source_block: BlockId,
        source_bb: BasicBlock<'ctx>,
        target: BlockId,
        edge_name: &'static str,
        args: &[ValueId],
    ) {
        self.phi_edges.push(PhiIncomingEdge {
            source_block,
            source_bb,
            target,
            edge_name,
            args: args.to_vec(),
        });
    }

    /// After all blocks are lowered, wire up phi node incoming values.
    /// Values are coerced to match the phi node's type when needed (e.g., an
    /// i1 bool flowing into an i64 phi is zero-extended).
    ///
    /// This method also handles:
    /// - Mid-block branches from CheckException (not visible in TIR terminators)
    /// - Missing predecessors: if a phi node doesn't have an incoming value for
    ///   some predecessor, record a fatal lowering diagnostic. The compile path
    ///   must not turn malformed control/data flow into verified-but-wrong IR.
    fn finalize_phis(&mut self) {
        // Collect phi info first to avoid borrow conflicts.
        let phi_info: Vec<_> = self
            .pending_phis
            .iter()
            .map(|(bid, idx, phi)| (*bid, *idx, phi.as_basic_value().get_type(), *phi))
            .collect();

        for (block_id, arg_index, phi_ty, phi) in &phi_info {
            let block = self.func.blocks.get(block_id).unwrap();
            let phi_tir_ty = block
                .args
                .get(*arg_index)
                .map(|arg| self.effective_block_arg_type(arg.id, &arg.ty))
                .unwrap_or(TirType::DynBox);

            // 1. Wire up predecessors from branches that were actually emitted
            //    into the LLVM CFG. This intentionally excludes dead TIR blocks
            //    whose terminators were not lowered and whose LLVM blocks were
            //    terminated with `unreachable`.
            let phi_edges = self.phi_edges.clone();
            for edge in phi_edges.iter().filter(|edge| edge.target == *block_id) {
                if *arg_index >= edge.args.len() {
                    self.record_fatal(format!(
                        "predecessor block {:?} {} branches to {:?} with {} argument(s), but phi argument index {} is required",
                        edge.source_block,
                        edge.edge_name,
                        block_id,
                        edge.args.len(),
                        arg_index
                    ));
                    continue;
                }
                let val_id = edge.args[*arg_index];
                let Some(val) = self.values.get(&val_id).copied() else {
                    self.record_fatal(format!(
                        "predecessor block {:?} passes undefined ValueId %{} to phi argument {} in block {:?}",
                        edge.source_block, val_id.0, arg_index, block_id
                    ));
                    continue;
                };
                let source_tir_ty = self
                    .value_types
                    .get(&val_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let coerced =
                    self.coerce_to_tir_type(val, &source_tir_ty, &phi_tir_ty, edge.source_bb);
                let coerced = self.coerce_to_type(coerced, *phi_ty, edge.source_bb);
                phi.add_incoming(&[(&coerced, edge.source_bb)]);
            }

            // 2. If the original TIR entry block was demoted behind a
            // trampoline, wire the function parameters in through that
            // synthetic predecessor. Entry args beyond the function arity are
            // true phi values and intentionally start as undef on the initial
            // call edge.
            if *block_id == self.func.entry_block
                && let Some(trampoline_bb) = self.entry_trampoline_bb
            {
                if let Some(param) = self.llvm_fn.get_nth_param(*arg_index as u32) {
                    let source_tir_ty = self
                        .func
                        .param_types
                        .get(*arg_index)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    let coerced =
                        self.coerce_to_tir_type(param, &source_tir_ty, &phi_tir_ty, trampoline_bb);
                    let coerced = self.coerce_to_type(coerced, *phi_ty, trampoline_bb);
                    phi.add_incoming(&[(&coerced, trampoline_bb)]);
                } else {
                    let undef = self.get_undef_for_type(*phi_ty);
                    phi.add_incoming(&[(&undef, trampoline_bb)]);
                }
            }
        }

        // 3. Final safety net: scan all phi nodes for missing predecessors.
        //    If any LLVM predecessor block is missing from a phi's incoming
        //    list, add an undef entry. This catches edge cases from synthetic
        //    blocks, trampoline blocks, and any other control flow that the
        //    TIR-level analysis doesn't fully capture.
        self.patch_incomplete_phis();
    }

    /// For each phi node in the function, check that every LLVM predecessor
    /// of the phi's parent block has an incoming entry. Missing entries are
    /// fatal lowering diagnostics.
    ///
    /// Uses the `llvm_pred_map` built during lowering to determine predecessors
    /// (no need to scan LLVM IR or use llvm-sys directly).
    fn patch_incomplete_phis(&self) {
        use inkwell::values::InstructionOpcode;
        use std::collections::HashSet;

        let mut bb = self.llvm_fn.get_first_basic_block();
        while let Some(current_bb) = bb {
            // Look up predecessors from our map.
            if let Some(preds) = self.llvm_pred_map.get(&current_bb) {
                // Walk instructions looking for phi nodes (they're always at the top).
                let mut inst = current_bb.get_first_instruction();
                while let Some(i) = inst {
                    if i.get_opcode() != InstructionOpcode::Phi {
                        break; // phi nodes are always at the top of the block
                    }
                    // Use inkwell's PhiValue to inspect incoming blocks.
                    use inkwell::values::AsValueRef;
                    let phi: PhiValue<'ctx> = unsafe { PhiValue::new(i.as_value_ref()) };
                    let incoming_count = phi.count_incoming();
                    let mut covered: HashSet<BasicBlock<'ctx>> = HashSet::new();
                    for idx in 0..incoming_count {
                        if let Some((_, incoming_bb)) = phi.get_incoming(idx) {
                            covered.insert(incoming_bb);
                        }
                    }
                    for pred_bb in preds {
                        if !covered.contains(pred_bb) {
                            self.record_fatal(format!(
                                "phi in LLVM block {:?} is missing incoming value from predecessor {:?}",
                                current_bb, pred_bb
                            ));
                        }
                    }
                    inst = i.get_next_instruction();
                }
            }
            bb = current_bb.get_next_basic_block();
        }
    }

    /// Return an `undef` value of the given LLVM type.
    fn get_undef_for_type(&self, ty: inkwell::types::BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        match ty {
            inkwell::types::BasicTypeEnum::IntType(it) => it.get_undef().into(),
            inkwell::types::BasicTypeEnum::FloatType(ft) => ft.get_undef().into(),
            inkwell::types::BasicTypeEnum::PointerType(pt) => pt.get_undef().into(),
            inkwell::types::BasicTypeEnum::ArrayType(at) => at.get_undef().into(),
            inkwell::types::BasicTypeEnum::StructType(st) => st.get_undef().into(),
            inkwell::types::BasicTypeEnum::VectorType(vt) => vt.get_undef().into(),
            inkwell::types::BasicTypeEnum::ScalableVectorType(svt) => svt.get_undef().into(),
        }
    }

    /// Coerce a value to a target LLVM type.  Inserts conversion instructions
    /// at the end of `in_block` (before the terminator) when the types differ.
    fn coerce_to_type(
        &self,
        val: BasicValueEnum<'ctx>,
        target_ty: inkwell::types::BasicTypeEnum<'ctx>,
        in_block: BasicBlock<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val_ty = val.get_type();
        if val_ty == target_ty {
            return val;
        }
        // Save current position and switch to the predecessor block.
        let saved_block = self.backend.builder.get_insert_block();
        // Insert BEFORE the terminator of in_block.
        if let Some(term) = in_block.get_terminator() {
            self.backend.builder.position_before(&term);
        } else {
            self.backend.builder.position_at_end(in_block);
        }
        let result = match (val, target_ty) {
            // i1 (bool) -> i64: zero-extend
            (BasicValueEnum::IntValue(iv), inkwell::types::BasicTypeEnum::IntType(target_int))
                if iv.get_type().get_bit_width() < target_int.get_bit_width() =>
            {
                self.backend
                    .builder
                    .build_int_z_extend(iv, target_int, "phi_zext")
                    .unwrap()
                    .into()
            }
            // i64 -> i1: truncate
            (BasicValueEnum::IntValue(iv), inkwell::types::BasicTypeEnum::IntType(target_int))
                if iv.get_type().get_bit_width() > target_int.get_bit_width() =>
            {
                self.backend
                    .builder
                    .build_int_truncate(iv, target_int, "phi_trunc")
                    .unwrap()
                    .into()
            }
            // f64 -> i64: bitcast
            (
                BasicValueEnum::FloatValue(fv),
                inkwell::types::BasicTypeEnum::IntType(target_int),
            ) => self
                .backend
                .builder
                .build_bit_cast(fv, target_int, "phi_f2i")
                .unwrap(),
            // i64 -> f64: bitcast
            (
                BasicValueEnum::IntValue(iv),
                inkwell::types::BasicTypeEnum::FloatType(target_float),
            ) => self
                .backend
                .builder
                .build_bit_cast(iv, target_float, "phi_i2f")
                .unwrap(),
            (
                BasicValueEnum::IntValue(iv),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr),
            ) => self
                .backend
                .builder
                .build_int_to_ptr(iv, target_ptr, "phi_i2p")
                .unwrap()
                .into(),
            (
                BasicValueEnum::PointerValue(pv),
                inkwell::types::BasicTypeEnum::IntType(target_int),
            ) => self
                .backend
                .builder
                .build_ptr_to_int(pv, target_int, "phi_p2i")
                .unwrap()
                .into(),
            (
                BasicValueEnum::PointerValue(pv),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr),
            ) => self
                .backend
                .builder
                .build_pointer_cast(pv, target_ptr, "phi_p2p")
                .unwrap()
                .into(),
            _ => {
                self.record_fatal(format!(
                    "unsupported LLVM phi coercion from {:?} to {:?} in block {:?}",
                    val_ty, target_ty, in_block
                ));
                self.get_undef_for_type(target_ty)
            }
        };
        // Restore builder position.
        if let Some(bb) = saved_block {
            self.backend.builder.position_at_end(bb);
        }
        result
    }

    fn tir_type_is_dynbox_like(ty: &TirType) -> bool {
        !matches!(
            ty,
            TirType::I64 | TirType::F64 | TirType::Bool | TirType::Never
        )
    }

    fn unbox_from_dynbox(
        &self,
        operand: BasicValueEnum<'ctx>,
        target_ty: &TirType,
    ) -> BasicValueEnum<'ctx> {
        let raw = self.ensure_i64(operand);
        let i64_ty = self.backend.context.i64_type();
        let f64_ty = self.backend.context.f64_type();
        match target_ty {
            TirType::I64 => {
                let masked = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "payload")
                    .unwrap();
                let sign_test = self
                    .backend
                    .builder
                    .build_and(
                        masked,
                        i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                        "sign_test",
                    )
                    .unwrap();
                let is_neg = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        sign_test,
                        i64_ty.const_zero(),
                        "is_neg",
                    )
                    .unwrap();
                let sign_extend = i64_ty.const_int(!nanbox::INT_MASK, false);
                let extended = self
                    .backend
                    .builder
                    .build_or(masked, sign_extend, "sign_extend")
                    .unwrap();
                self.backend
                    .builder
                    .build_select(is_neg, extended, masked, "unbox_i64")
                    .unwrap()
            }
            TirType::F64 => self
                .backend
                .builder
                .build_bit_cast(raw, f64_ty, "unbox_f64")
                .unwrap(),
            TirType::Bool => {
                let bit = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(1, false), "bool_payload")
                    .unwrap();
                self.backend
                    .builder
                    .build_int_truncate(bit, self.backend.context.bool_type(), "unbox_bool")
                    .unwrap()
                    .into()
            }
            _ => operand,
        }
    }

    fn coerce_to_tir_type(
        &self,
        val: BasicValueEnum<'ctx>,
        source_tir_ty: &TirType,
        target_tir_ty: &TirType,
        in_block: BasicBlock<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        if source_tir_ty == target_tir_ty {
            return val;
        }

        let saved_block = self.backend.builder.get_insert_block();
        if let Some(term) = in_block.get_terminator() {
            self.backend.builder.position_before(&term);
        } else {
            self.backend.builder.position_at_end(in_block);
        }

        let result = if Self::tir_type_is_dynbox_like(target_tir_ty)
            && !Self::tir_type_is_dynbox_like(source_tir_ty)
        {
            // `coerce_to_tir_type` materializes a value at a fixed position —
            // either the current block (return) or, for phi incoming edges, the
            // END of a predecessor block that already has a terminator. Both
            // restore the builder afterwards and (for phi edges) require the
            // result to be a single SSA value defined in `in_block`. The
            // overflow-safe integer box that adds a fits-inline branch would
            // split `in_block`, leaving the boxed value in a new merge block
            // that does not dominate the phi user. We therefore box integers
            // here with the branchless runtime call, which yields one SSA value
            // and never alters control flow. (`molt_int_from_i64` returns the
            // inline box for small values and a heap BigInt otherwise — the
            // same value the branch form produces.)
            if matches!(source_tir_ty, TirType::I64) {
                let raw = self.ensure_i64(val);
                self.box_i64_branchless(raw).into()
            } else {
                self.materialize_dynbox_bits(val, source_tir_ty).into()
            }
        } else if !Self::tir_type_is_dynbox_like(target_tir_ty)
            && Self::tir_type_is_dynbox_like(source_tir_ty)
        {
            self.unbox_from_dynbox(val, target_tir_ty)
        } else {
            val
        };

        if let Some(bb) = saved_block {
            self.backend.builder.position_at_end(bb);
        }
        result
    }

    // ── Helpers ──

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined, record a fatal diagnostic. The fallback
    /// value only keeps diagnostic collection moving; checked lowering refuses
    /// to expose the resulting function.
    fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        if let Some(val) = self.values.get(&id) {
            *val
        } else {
            self.record_fatal(format!(
                "ValueId %{} was used before being defined during LLVM lowering",
                id.0
            ));
            self.backend.context.i64_type().get_undef().into()
        }
    }

    /// Ensure a value is i64 (for NaN-boxed runtime calls).
    /// If it's already i64, return as-is. Otherwise, cast/extend.
    fn ensure_i64(&self, val: BasicValueEnum<'ctx>) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        match val {
            BasicValueEnum::IntValue(iv) => {
                if iv.get_type().get_bit_width() == 64 {
                    iv
                } else if iv.get_type().get_bit_width() < 64 {
                    self.backend
                        .builder
                        .build_int_z_extend(iv, i64_ty, "zext_i64")
                        .unwrap()
                } else {
                    self.backend
                        .builder
                        .build_int_truncate(iv, i64_ty, "trunc_i64")
                        .unwrap()
                }
            }
            BasicValueEnum::FloatValue(fv) => self
                .backend
                .builder
                .build_bit_cast(fv, i64_ty, "f2i")
                .unwrap()
                .into_int_value(),
            BasicValueEnum::PointerValue(pv) => self
                .backend
                .builder
                .build_ptr_to_int(pv, i64_ty, "ptr2i")
                .unwrap(),
            _ => panic!("Cannot convert {:?} to i64", val),
        }
    }

    fn ensure_runtime_decl(
        &self,
        name: &str,
        fn_ty: inkwell::types::FunctionType<'ctx>,
        param_count: usize,
        return_abi: RuntimeReturnAbi,
    ) -> FunctionValue<'ctx> {
        if let Some(func) = self.backend.module.get_function(name) {
            return require_llvm_function_type(name, func, fn_ty);
        }
        if !is_classified_runtime_import(name, param_count, return_abi) {
            panic!(
                "LLVM runtime import `{name}` has no ABI classification for conservative declaration"
            );
        }
        let func = declare_conservative_runtime_function(
            self.backend.context,
            &self.backend.module,
            name,
            fn_ty,
        );
        require_llvm_function_type(name, func, fn_ty)
    }

    fn ensure_runtime_i64_fn(&self, name: &str, param_count: usize) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        self.ensure_runtime_decl(
            name,
            i64_ty.fn_type(&params, false),
            param_count,
            RuntimeReturnAbi::I64,
        )
    }

    fn ensure_runtime_void_fn(&self, name: &str, param_count: usize) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        self.ensure_runtime_decl(
            name,
            self.backend.context.void_type().fn_type(&params, false),
            param_count,
            RuntimeReturnAbi::Void,
        )
    }

    fn unbox_ptr_bits(
        &self,
        bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let masked = self
            .backend
            .builder
            .build_and(
                bits,
                i64_ty.const_int(nanbox::POINTER_MASK, false),
                "ptr_masked",
            )
            .unwrap();
        let shifted = self
            .backend
            .builder
            .build_left_shift(masked, i64_ty.const_int(16, false), "ptr_shifted")
            .unwrap();
        self.backend
            .builder
            .build_right_shift(shifted, i64_ty.const_int(16, false), true, "ptr_signext")
            .unwrap()
    }

    fn raw_string_const_ptr_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let name_bytes = s.as_bytes();
        let global = self.backend.module.add_global(
            self.backend
                .context
                .i8_type()
                .array_type(name_bytes.len() as u32),
            None,
            &format!(
                "__guard_attr_str_{}_{}",
                self.const_str_counter,
                s.replace(|c: char| !c.is_alphanumeric(), "_")
            ),
        );
        self.const_str_counter += 1;
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(name_bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);
        let ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(global.as_pointer_value(), i64_ty, "guard_attr_ptr")
            .unwrap();
        let len_bits = i64_ty.const_int(name_bytes.len() as u64, false);
        (ptr_bits, len_bits)
    }

    fn emit_task_new_with_payload(
        &mut self,
        poll_addr: inkwell::values::IntValue<'ctx>,
        closure_size: i64,
        kind_bits: i64,
        payload_base: i32,
        payload_operands: &[ValueId],
        call_name: &str,
    ) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let task_new_fn = self.ensure_runtime_i64_fn("molt_task_new", 3);
        let task_bits = self
            .backend
            .builder
            .build_call(
                task_new_fn,
                &[
                    poll_addr.into(),
                    i64_ty.const_int(closure_size as u64, true).into(),
                    i64_ty.const_int(kind_bits as u64, true).into(),
                ],
                call_name,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        // `molt_task_new` returns a NaN-boxed task handle. Frame payload stores
        // address raw heap memory, so strip the boxing tag before writing slots,
        // matching native `unbox_ptr_value` and WASM `handle_resolve`.
        let task_ptr_bits = self.unbox_ptr_bits(self.ensure_i64(task_bits));
        let task_ptr = self
            .backend
            .builder
            .build_int_to_ptr(task_ptr_bits, ptr_ty, "task_obj_ptr")
            .unwrap();
        let payload_base_words = (payload_base / 8) as usize;
        let inc_fn = self.ensure_runtime_i64_fn("molt_inc_ref_obj", 1);
        for (idx, &arg_id) in payload_operands.iter().enumerate() {
            let arg_bits = self.materialize_dynbox_operand(arg_id);
            let field_ptr = unsafe {
                self.backend
                    .builder
                    .build_gep(
                        i64_ty,
                        task_ptr,
                        &[i64_ty.const_int((payload_base_words + idx) as u64, false)],
                        &format!("task_payload_ptr_{idx}"),
                    )
                    .unwrap()
            };
            self.backend
                .builder
                .build_store(field_ptr, arg_bits)
                .unwrap();
            let _ = self
                .backend
                .builder
                .build_call(inc_fn, &[arg_bits.into()], "task_payload_inc_ref")
                .unwrap();
        }
        task_bits
    }

    fn ensure_function_symbol(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            if let Some(param_types) = self.backend.function_param_types.get(name) {
                param_types
                    .iter()
                    .map(|ty| lower_type(self.backend.context, ty).into())
                    .collect()
            } else {
                let param_count = arity + usize::from(has_closure);
                (0..param_count).map(|_| i64_ty.into()).collect()
            };
        let return_ty = self
            .backend
            .function_return_types
            .get(name)
            .map(|ty| lower_type(self.backend.context, ty))
            .unwrap_or_else(|| i64_ty.into());
        let fn_ty = return_ty.fn_type(&params, false);
        if let Some(func) = self.backend.module.get_function(name) {
            return require_llvm_function_type(name, func, fn_ty);
        }
        self.backend
            .module
            .add_function(name, fn_ty, Some(inkwell::module::Linkage::External))
    }

    fn ensure_plain_trampoline(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let callable_arity = self
            .backend
            .function_param_types
            .get(name)
            .map(|tys| tys.len().saturating_sub(usize::from(has_closure)))
            .unwrap_or(arity);
        let target_fn = self.ensure_function_symbol(name, callable_arity, has_closure);
        let target_return_tir_ty = self
            .backend
            .function_return_types
            .get(name)
            .cloned()
            .unwrap_or(TirType::DynBox);
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let trampoline_name =
            format!("{name}__molt_llvm_trampoline_{callable_arity}{closure_suffix}");
        if let Some(func) = self.backend.module.get_function(&trampoline_name) {
            return func;
        }

        let i64_ty = self.backend.context.i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let trampoline_fn = self.backend.module.add_function(
            &trampoline_name,
            fn_ty,
            Some(inkwell::module::Linkage::Internal),
        );

        let builder = self.backend.context.create_builder();
        let entry = self
            .backend
            .context
            .append_basic_block(trampoline_fn, "entry");
        builder.position_at_end(entry);

        let closure_bits = trampoline_fn
            .get_nth_param(0)
            .expect("trampoline closure param missing")
            .into_int_value();
        let args_bits = trampoline_fn
            .get_nth_param(1)
            .expect("trampoline args param missing")
            .into_int_value();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let args_ptr = builder
            .build_int_to_ptr(args_bits, ptr_ty, "trampoline_args_ptr")
            .unwrap();

        // The target function's parameter SEMANTIC types (the representation
        // plan's `TirType` per param), used to decode each NaN-boxed argument
        // into the raw machine representation the direct ABI expects. Indexed
        // 1:1 with the LLVM params: when `has_closure`, index 0 is the closure
        // object (a boxed reference — no payload decode). A raw-`I64` param must
        // be sign-extended out of its 47-bit inline NaN-box payload; passing the
        // boxed bits straight through (as this trampoline did before) made the
        // callee body decode a NaN-box tag/pointer as a raw integer — the
        // trusted-unbox truncation bug-class for a heap-BigInt argument. This is
        // the dynamic-dispatch dual of the direct-call arg coercion
        // (`coerce_to_tir_type`).
        let param_tir_types = self.backend.function_param_types.get(name);
        let coerce_trampoline_arg = |bits: inkwell::values::IntValue<'ctx>,
                                     target_ty: inkwell::types::BasicTypeEnum<'ctx>,
                                     name: &str|
         -> inkwell::values::BasicMetadataValueEnum<'ctx> {
            match target_ty {
                inkwell::types::BasicTypeEnum::IntType(target_int) => {
                    if target_int.get_bit_width() == 64 {
                        bits.into()
                    } else if target_int.get_bit_width() < 64 {
                        builder
                            .build_int_truncate(bits, target_int, name)
                            .unwrap()
                            .into()
                    } else {
                        builder
                            .build_int_z_extend(bits, target_int, name)
                            .unwrap()
                            .into()
                    }
                }
                inkwell::types::BasicTypeEnum::FloatType(target_float) => builder
                    .build_bit_cast(bits, target_float, name)
                    .unwrap()
                    .into(),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr) => builder
                    .build_int_to_ptr(bits, target_ptr, name)
                    .unwrap()
                    .into(),
                other => panic!(
                    "unsupported trampoline argument coercion for {} to {:?}",
                    name, other
                ),
            }
        };

        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(callable_arity + usize::from(has_closure));
        if has_closure {
            let target_ty = target_fn
                .get_nth_param(0)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            call_args.push(coerce_trampoline_arg(
                closure_bits,
                target_ty,
                "trampoline_closure_arg",
            ));
        }
        for idx in 0..callable_arity {
            let elem_ptr = unsafe {
                builder
                    .build_gep(
                        i64_ty,
                        args_ptr,
                        &[i64_ty.const_int(idx as u64, false)],
                        &format!("trampoline_arg_ptr_{idx}"),
                    )
                    .unwrap()
            };
            let arg = builder
                .build_load(i64_ty, elem_ptr, &format!("trampoline_arg_{idx}"))
                .unwrap()
                .into_int_value();
            // Decode the NaN-boxed argument into the raw representation the
            // target parameter expects, BEFORE the LLVM-type cast. The args
            // array always carries `DynBox` (NaN-boxed) values; a raw-`I64`
            // param needs its 47-bit payload sign-extended back, a `Bool` its
            // low payload bit. `F64`/reference params are already the raw bits.
            let param_index = idx + usize::from(has_closure);
            let arg = match param_tir_types.and_then(|tys| tys.get(param_index)) {
                Some(param_ty) => unbox_dynbox_to_param_ty_with_builder(
                    &builder,
                    self.backend.context,
                    arg,
                    param_ty,
                ),
                None => arg,
            };
            let target_ty = target_fn
                .get_nth_param(param_index as u32)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            call_args.push(coerce_trampoline_arg(
                arg,
                target_ty,
                &format!("trampoline_arg_cast_{idx}"),
            ));
        }

        let result = builder
            .build_call(target_fn, &call_args, "trampoline_call")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| {
                i64_ty
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    .into()
            });
        let ret_bits = materialize_dynbox_bits_with_builder(
            &builder,
            self.backend.context,
            &self.backend.module,
            trampoline_fn,
            result,
            &target_return_tir_ty,
        );
        builder.build_return(Some(&ret_bits)).unwrap();
        trampoline_fn
    }

    fn lower_preserved_simpleir_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        // Vectorized reduction family (`VEC_SUM/PROD/MIN/MAX_*` from the
        // frontend's `_match_vector_reduction_loop`). These ops have no
        // dedicated `OpCode` — the SSA lifter folds them into `Copy` with
        // `_original_kind` carrying the op name (ssa.rs), so on backends that
        // round-trip TIR→SimpleIR (native/WASM/Luau) they dispatch on that
        // string. The LLVM backend consumes TIR directly, so without this arm
        // each reduction would fall through to the naive `Copy` passthrough and
        // SILENTLY return its first operand (the sequence object) instead of the
        // reduced value — a wrong-result miscompile, not an error. The mapping
        // is fully mechanical: the runtime symbol is `molt_<kind>` and the arity
        // equals the operand count (2 for seq+acc / range_iter, 3 for the range
        // forms that also carry `start`). Every operand is a boxed value; the
        // result is a single i64. The set is closed and validated against the
        // runtime surface so an unrecognized `vec_*` kind fails loud rather than
        // miscompiling.
        if let Some(symbol) = vec_reduction_runtime_symbol(kind) {
            debug_assert_eq!(
                op.operands.len(),
                vec_reduction_arity(kind),
                "vec reduction {kind} must carry exactly {} operands",
                vec_reduction_arity(kind),
            );
            if op.operands.len() != vec_reduction_arity(kind) {
                return false;
            }
            let arg_bits: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                .operands
                .iter()
                .map(|&id| self.materialize_dynbox_operand(id).into())
                .collect();
            let call_fn = self.ensure_runtime_i64_fn(symbol, op.operands.len());
            let result = self
                .backend
                .builder
                .build_call(call_fn, &arg_bits, symbol)
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            return true;
        }
        match kind {
            // Repr-identity SimpleIR ops. Native and WASM lower these as
            // operand-0 passthroughs over the same NaN-boxed value format; LLVM
            // must claim the exact same identity fact explicitly so the terminal
            // preserved-op guard remains a true fail-loud backstop rather than a
            // backend skew. No runtime call, ownership transfer, or new value is
            // introduced here.
            "cast" | "widen" | "copy_var" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_ty = self
                    .value_types
                    .get(&src_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                for &result_id in &op.results {
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, src_ty.clone());
                }
                true
            }
            "list_from_range" => {
                // `list(range(start, stop, step))` materialized eagerly by the
                // frontend (`LIST_FROM_RANGE`). Like `range_new`, it has no
                // dedicated TIR `OpCode` and survives as a `Copy` carrying
                // `_original_kind`; without this arm the LLVM Copy passthrough
                // would return operand 0 (the `start` bound) instead of the
                // built list — a silent wrong-result miscompile. Lower to the
                // dedicated runtime constructor `molt_list_from_range(start,
                // stop, step)`, mirroring the native/WASM backends.
                debug_assert_eq!(
                    op.operands.len(),
                    3,
                    "list_from_range must carry exactly [start, stop, step]"
                );
                if op.operands.len() != 3 {
                    return false;
                }
                let list_from_range_fn = self.ensure_runtime_i64_fn("molt_list_from_range", 3);
                let start = self.materialize_dynbox_operand(op.operands[0]).into();
                let stop = self.materialize_dynbox_operand(op.operands[1]).into();
                let step = self.materialize_dynbox_operand(op.operands[2]).into();
                let result = self
                    .backend
                    .builder
                    .build_call(list_from_range_fn, &[start, stop, step], "list_from_range")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "call_method_ic" => {
                // Fused instance-method dispatch (LOAD_METHOD/CALL_METHOD):
                //   operands = [recv, a0, a1, ...]  attrs.s_value = <method>
                // Lowers to a single molt_call_method_icN(site, recv, name_ptr,
                // name_len, a0..) runtime call — no bound-method / callargs
                // allocation on the IC fast path. The runtime entry is
                // target-independent extern "C"; on the native LLVM target the
                // name pointer is a real pointer cast to i64 (every arg i64),
                // matching the `4 + N`-i64 declaration in runtime_imports.rs.
                if op.operands.is_empty() {
                    return false;
                }
                let Some(method_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let recv_bits = self.materialize_dynbox_operand(op.operands[0]);
                let extra: Vec<inkwell::values::IntValue<'ctx>> = op.operands[1..]
                    .iter()
                    .map(|&id| self.materialize_dynbox_operand(id))
                    .collect();
                let site_bits = self.next_call_site_bits("call_method_ic");
                let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&method_name);
                let symbol = match extra.len() {
                    0 => "molt_call_method_ic0",
                    1 => "molt_call_method_ic1",
                    2 => "molt_call_method_ic2",
                    3 => "molt_call_method_ic3",
                    4 => "molt_call_method_ic4",
                    n => panic!(
                        "call_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
                    ),
                };
                let arity = 4 + extra.len();
                let call_fn = self.ensure_runtime_i64_fn(symbol, arity);
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
                    site_bits.into(),
                    recv_bits.into(),
                    name_ptr_bits.into(),
                    name_len_bits.into(),
                ];
                call_args.extend(
                    extra
                        .iter()
                        .map(|v| -> inkwell::values::BasicMetadataValueEnum<'ctx> { (*v).into() }),
                );
                let result = self
                    .backend
                    .builder
                    .build_call(call_fn, &call_args, symbol)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "call_super_method_ic" => {
                // Fused super().method() dispatch (no super / bound-method /
                // callargs allocation on the fast path):
                //   operands = [class, self, a0, a1, ...]  attrs.s_value = <method>
                // Lowers to molt_call_super_method_icN(site, class, self,
                // name_ptr, name_len, a0..).
                if op.operands.len() < 2 {
                    return false;
                }
                let Some(method_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let class_bits = self.materialize_dynbox_operand(op.operands[0]);
                let self_bits = self.materialize_dynbox_operand(op.operands[1]);
                let extra: Vec<inkwell::values::IntValue<'ctx>> = op.operands[2..]
                    .iter()
                    .map(|&id| self.materialize_dynbox_operand(id))
                    .collect();
                let site_bits = self.next_call_site_bits("call_super_method_ic");
                let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&method_name);
                let symbol = match extra.len() {
                    0 => "molt_call_super_method_ic0",
                    1 => "molt_call_super_method_ic1",
                    2 => "molt_call_super_method_ic2",
                    3 => "molt_call_super_method_ic3",
                    4 => "molt_call_super_method_ic4",
                    n => panic!(
                        "call_super_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
                    ),
                };
                let arity = 5 + extra.len();
                let call_fn = self.ensure_runtime_i64_fn(symbol, arity);
                let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
                    site_bits.into(),
                    class_bits.into(),
                    self_bits.into(),
                    name_ptr_bits.into(),
                    name_len_bits.into(),
                ];
                call_args.extend(
                    extra
                        .iter()
                        .map(|v| -> inkwell::values::BasicMetadataValueEnum<'ctx> { (*v).into() }),
                );
                let result = self
                    .backend
                    .builder
                    .build_call(call_fn, &call_args, symbol)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "builtin_func" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_trampoline_ptr",
                    )
                    .unwrap();
                let name_bits = self.intern_string_const(func_name).into_int_value();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new_builtin_named", 4);
                let func_bits = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            name_bits.into(),
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                        ],
                        "builtin_func_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, func_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "func_new" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_trampoline_ptr",
                    )
                    .unwrap();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                        ],
                        "func_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "func_new_closure" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let Some(&closure_id) = op.operands.first() else {
                    return false;
                };
                let closure_bits = self.ensure_i64(self.resolve(closure_id));
                let func = self.ensure_function_symbol(func_name, arity, true);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, true);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_trampoline_ptr",
                    )
                    .unwrap();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new_closure", 4);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                            closure_bits.into(),
                        ],
                        "func_new_closure",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "call_async" => {
                let Some(poll_func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let Some(&result_id) = op.results.first() else {
                    return false;
                };
                if poll_func_name == "molt_async_sleep" {
                    if op.operands.len() > 2 {
                        return false;
                    }
                    let delay_bits = op
                        .operands
                        .first()
                        .map(|&id| self.materialize_dynbox_operand(id))
                        .unwrap_or_else(|| {
                            let zero: BasicValueEnum<'ctx> =
                                self.backend.context.f64_type().const_float(0.0).into();
                            self.materialize_dynbox_bits(zero, &TirType::F64)
                        });
                    let result_bits = op
                        .operands
                        .get(1)
                        .map(|&id| self.materialize_dynbox_operand(id))
                        .unwrap_or_else(|| {
                            i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        });
                    let sleep_fn = self.ensure_runtime_i64_fn("molt_async_sleep", 2);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            sleep_fn,
                            &[delay_bits.into(), result_bits.into()],
                            "call_async_sleep",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                    return true;
                }

                let poll_fn = self.ensure_function_symbol(poll_func_name, 1, false);
                let poll_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        poll_fn.as_global_value().as_pointer_value(),
                        i64_ty,
                        "call_async_poll_ptr",
                    )
                    .unwrap();
                let task_bits = self.emit_task_new_with_payload(
                    poll_addr,
                    (op.operands.len() * 8) as i64,
                    crate::TASK_KIND_FUTURE,
                    0,
                    &op.operands,
                    "call_async_task_new",
                );
                self.values.insert(result_id, task_bits);
                self.value_types.insert(result_id, TirType::DynBox);
                true
            }
            "code_new" => {
                if op.operands.len() != 9 {
                    return false;
                }
                let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                    .operands
                    .iter()
                    .map(|&id| self.materialize_dynbox_operand(id).into())
                    .collect();
                let code_new_fn = self.ensure_runtime_i64_fn("molt_code_new", 9);
                let result = self
                    .backend
                    .builder
                    .build_call(code_new_fn, &args, "code_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "code_slot_set" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let Some(&code_bits_id) = op.operands.first() else {
                    return false;
                };
                let code_bits = self.ensure_i64(self.resolve(code_bits_id));
                let slot_set_fn = self.ensure_runtime_i64_fn("molt_code_slot_set", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        slot_set_fn,
                        &[
                            i64_ty.const_int(code_id as u64, true).into(),
                            code_bits.into(),
                        ],
                        "code_slot_set",
                    )
                    .unwrap();
                true
            }
            "code_slots_init" => {
                let count = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let init_fn = self.ensure_runtime_i64_fn("molt_code_slots_init", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        init_fn,
                        &[i64_ty.const_int(count as u64, true).into()],
                        "code_slots_init",
                    )
                    .unwrap();
                true
            }
            "classmethod_new" => {
                let Some(&func_id) = op.operands.first() else {
                    return false;
                };
                let func_bits = self.ensure_i64(self.resolve(func_id));
                let classmethod_fn = self.ensure_runtime_i64_fn("molt_classmethod_new", 1);
                let result = self
                    .backend
                    .builder
                    .build_call(classmethod_fn, &[func_bits.into()], "classmethod_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "staticmethod_new" => {
                let Some(&func_id) = op.operands.first() else {
                    return false;
                };
                let func_bits = self.ensure_i64(self.resolve(func_id));
                let staticmethod_fn = self.ensure_runtime_i64_fn("molt_staticmethod_new", 1);
                let result = self
                    .backend
                    .builder
                    .build_call(staticmethod_fn, &[func_bits.into()], "staticmethod_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "property_new" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let getter_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let setter_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let deleter_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let property_fn = self.ensure_runtime_i64_fn("molt_property_new", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        property_fn,
                        &[getter_bits.into(), setter_bits.into(), deleter_bits.into()],
                        "property_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "trace_enter_slot" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let enter_fn = self.ensure_runtime_i64_fn("molt_trace_enter_slot", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        enter_fn,
                        &[i64_ty.const_int(code_id as u64, true).into()],
                        "trace_enter_slot",
                    )
                    .unwrap();
                true
            }
            "trace_exit" => {
                let exit_fn = self.ensure_runtime_i64_fn("molt_trace_exit", 0);
                let _ = self
                    .backend
                    .builder
                    .build_call(exit_fn, &[], "trace_exit")
                    .unwrap();
                true
            }
            "frame_locals_set" => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let frame_locals_fn = self.ensure_runtime_i64_fn("molt_frame_locals_set", 1);
                let dict_bits = self.materialize_dynbox_operand(dict_id);
                let _ = self
                    .backend
                    .builder
                    .build_call(frame_locals_fn, &[dict_bits.into()], "frame_locals_set")
                    .unwrap();
                true
            }
            "line" => {
                let line = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let line_fn = self.ensure_runtime_i64_fn("molt_trace_set_line", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        line_fn,
                        &[i64_ty.const_int(line as u64, true).into()],
                        "trace_set_line",
                    )
                    .unwrap();
                true
            }
            "fn_ptr_code_set" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let Some(&code_bits_id) = op.operands.first() else {
                    return false;
                };
                let code_bits = self.ensure_i64(self.resolve(code_bits_id));
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "fn_ptr_code",
                    )
                    .unwrap();
                let set_fn = self.ensure_runtime_i64_fn("molt_fn_ptr_code_set", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[fn_ptr.into(), code_bits.into()],
                        "fn_ptr_code_set",
                    )
                    .unwrap();
                true
            }
            "callargs_new" => {
                let new_fn = self.ensure_runtime_i64_fn("molt_callargs_new", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_zero().into(), i64_ty.const_zero().into()],
                        "callargs_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "callargs_push_pos" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_pos", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        push_fn,
                        &[builder_bits.into(), val_bits.into()],
                        "callargs_push_pos",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "callargs_push_kw" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_kw", 3);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.materialize_dynbox_operand(op.operands[1]);
                let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        push_fn,
                        &[builder_bits.into(), name_bits.into(), val_bits.into()],
                        "callargs_push_kw",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "callargs_expand_star" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let expand_fn = self.ensure_runtime_i64_fn("molt_callargs_expand_star", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let iterable_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        expand_fn,
                        &[builder_bits.into(), iterable_bits.into()],
                        "callargs_expand_star",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "callargs_expand_kwstar" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let expand_fn = self.ensure_runtime_i64_fn("molt_callargs_expand_kwstar", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let mapping_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        expand_fn,
                        &[builder_bits.into(), mapping_bits.into()],
                        "callargs_expand_kwstar",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "iter_next_unboxed" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let iter_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "iter_next_unboxed_value")
                    .unwrap();
                let val_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(val_ptr, i64_ty, "iter_next_unboxed_value_ptr")
                    .unwrap();
                let iter_next_fn = self.ensure_runtime_i64_fn("molt_iter_next_unboxed", 2);
                let done_bits = self
                    .backend
                    .builder
                    .build_call(
                        iter_next_fn,
                        &[iter_bits.into(), val_ptr_bits.into()],
                        "iter_next_unboxed",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let value_bits = self
                    .backend
                    .builder
                    .build_load(i64_ty, val_ptr, "iter_next_unboxed_value_load")
                    .unwrap();
                if let Some(&value_id) = op.results.first() {
                    self.values.insert(value_id, value_bits);
                    self.value_types.insert(value_id, TirType::DynBox);
                }
                if let Some(&done_id) = op.results.get(1) {
                    self.values.insert(done_id, done_bits);
                    self.value_types.insert(done_id, TirType::DynBox);
                }
                true
            }
            "len" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let fn_name = self.container_len_fn(op.operands[0]);
                let len_fn = self.ensure_runtime_i64_fn(fn_name, 1);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(len_fn, &[obj_bits.into()], "len")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "list_new" => {
                let list_new_fn = self.ensure_runtime_i64_fn("molt_list_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        list_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "list_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.ensure_runtime_void_fn("molt_list_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_bits.into()], "list_append")
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_list_builder_finish", 1);
                let list = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "list_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "list_fill_new" => {
                let list_fill_fn = self.ensure_runtime_i64_fn("molt_list_fill_new", 2);
                let count = self.materialize_dynbox_operand(op.operands[0]);
                let fill = self.materialize_dynbox_operand(op.operands[1]);
                let list = self
                    .backend
                    .builder
                    .build_call(list_fill_fn, &[count.into(), fill.into()], "list_fill_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "list_append" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_list_append", 2);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into(), item_bits.into()], "list_append")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "list_extend" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_list_extend", 2);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let other_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into(), other_bits.into()], "list_extend")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_new" => {
                let dict_new_fn = self.ensure_runtime_i64_fn("molt_dict_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        dict_new_fn,
                        &[i64_ty
                            .const_int((op.operands.len() / 2) as u64, false)
                            .into()],
                        "dict_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let set_fn = self.ensure_runtime_void_fn("molt_dict_builder_append", 3);
                let mut idx = 0;
                while idx + 1 < op.operands.len() {
                    let key_bits = self.materialize_dynbox_operand(op.operands[idx]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[idx + 1]);
                    self.backend
                        .builder
                        .build_call(
                            set_fn,
                            &[builder.into(), key_bits.into(), val_bits.into()],
                            "dict_append",
                        )
                        .unwrap();
                    idx += 2;
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_dict_builder_finish", 1);
                let dict = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "dict_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, dict);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "tuple_new" => {
                let tuple_new_fn = self.ensure_runtime_i64_fn("molt_list_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        tuple_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "tuple_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let append_fn = self.ensure_runtime_void_fn("molt_list_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(
                            append_fn,
                            &[builder.into(), item_bits.into()],
                            "tuple_append",
                        )
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_tuple_builder_finish", 1);
                let tuple_bits = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "tuple_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, tuple_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "tuple_from_list" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_tuple_from_list", 1);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into()], "tuple_from_list")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "set_new" => {
                let set_new_fn = self.ensure_runtime_i64_fn("molt_set_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        set_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "set_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let append_fn = self.ensure_runtime_void_fn("molt_set_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(append_fn, &[builder.into(), item_bits.into()], "set_append")
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_set_builder_finish", 1);
                let set_bits = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "set_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "set_add" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_set_add", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "set_add")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "set_add_probe" => {
                // Probe-only realization: bare unhashable context on every version.
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_set_add_probe", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "set_add_probe")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "frozenset_new" => {
                // `frozenset([...])` constructor. Like `set_new`/`list_from_range`
                // it has no dedicated TIR `OpCode` — the SSA lifter folds it into a
                // `Copy` carrying `_original_kind = "frozenset_new"`. Without this
                // arm the LLVM Copy passthrough returned operand 0 (or, for the
                // common zero-operand `frozenset_new` + separate `frozenset_add`
                // shape, the None sentinel because there is no operand 0) — so
                // `frozenset([1,2,3])` evaluated to `None` entirely (#61). The
                // native/WASM/Luau backends all carry an explicit arm; this closes
                // the LLVM-only coverage gap, mirroring `fc::set_ops::handle_set_op`
                // exactly: `molt_frozenset_new(capacity)` then a `molt_frozenset_add`
                // per element (the frozenset is mutated in place during
                // construction). Any bundled elements are added inline; the
                // zero-operand shape relies on the sibling `frozenset_add` arm.
                let new_fn = self.ensure_runtime_i64_fn("molt_frozenset_new", 1);
                let set_bits = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "frozenset_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.operands.is_empty() {
                    let add_fn = self.ensure_runtime_i64_fn("molt_frozenset_add", 2);
                    for &item_id in &op.operands {
                        let item_bits = self.materialize_dynbox_operand(item_id);
                        self.backend
                            .builder
                            .build_call(
                                add_fn,
                                &[self.ensure_i64(set_bits).into(), item_bits.into()],
                                "frozenset_add",
                            )
                            .unwrap();
                    }
                }
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "frozenset_add" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_frozenset_add", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "frozenset_add")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_set" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_set", 3);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let value_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), value_bits.into()],
                        "dict_set",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_setdefault" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_setdefault", 3);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let default_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), default_bits.into()],
                        "dict_setdefault",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_setdefault_empty_list" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_setdefault_empty_list", 2);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into()],
                        "dict_setdefault_empty_list",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "aiter" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_aiter", 1);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[obj_bits.into()], "aiter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "gen_send" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_generator_send", 2);
                let gen_bits = self.materialize_dynbox_operand(op.operands[0]);
                let value_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[gen_bits.into(), value_bits.into()], "gen_send")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_exit" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_context_exit", 2);
                let ctx_bits = self.materialize_dynbox_operand(op.operands[0]);
                let exc_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[ctx_bits.into(), exc_bits.into()], "context_exit")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "super_new" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_super_new", 2);
                let type_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[type_bits.into(), obj_bits.into()], "super_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_get" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let dict_get_fn = self.ensure_runtime_i64_fn("molt_dict_get", 3);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let key_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let default_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        dict_get_fn,
                        &[dict_bits.into(), key_bits.into(), default_bits.into()],
                        "dict_get",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "iter" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let iter_fn = self.ensure_runtime_i64_fn("molt_iter_checked", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(iter_fn, &[obj_bits.into()], "iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "unpack_sequence" => {
                let Some(&seq_id) = op.operands.first() else {
                    return false;
                };
                let expected = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(op.results.len());
                let out_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(
                        i64_ty,
                        i64_ty.const_int(expected.max(1) as u64, false),
                        "unpack_out",
                    )
                    .unwrap();
                let out_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(out_alloca, i64_ty, "unpack_out_ptr")
                    .unwrap();
                let unpack_fn = self.ensure_runtime_i64_fn("molt_unpack_sequence", 3);
                let seq_bits = self.ensure_i64(self.resolve(seq_id));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        unpack_fn,
                        &[
                            seq_bits.into(),
                            i64_ty.const_int(expected as u64, false).into(),
                            out_ptr_bits.into(),
                        ],
                        "unpack_sequence",
                    )
                    .unwrap();
                for (idx, &result_id) in op.results.iter().enumerate() {
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                out_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                "unpack_elem_ptr",
                            )
                            .unwrap()
                    };
                    let elem = self
                        .backend
                        .builder
                        .build_load(i64_ty, elem_ptr, "unpack_elem")
                        .unwrap();
                    self.values.insert(result_id, elem);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_def" => {
                let Some(meta) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let mut parts = meta.split(',');
                let Some(nbases) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(nattrs) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(layout_size) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(layout_version) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(flags) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                if op.operands.is_empty() || op.operands.len() != 1 + nbases + nattrs * 2 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let bases_count = nbases.max(1) as u64;
                let attrs_count = (nattrs * 2).max(1) as u64;
                let bases_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(i64_ty, i64_ty.const_int(bases_count, false), "class_bases")
                    .unwrap();
                let attrs_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(i64_ty, i64_ty.const_int(attrs_count, false), "class_attrs")
                    .unwrap();
                for idx in 0..nbases {
                    let base_bits = self.materialize_dynbox_operand(op.operands[1 + idx]);
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                bases_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                &format!("class_base_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(elem_ptr, base_bits)
                        .unwrap();
                }
                let attrs_start = 1 + nbases;
                for idx in 0..(nattrs * 2) {
                    let value_bits =
                        self.materialize_dynbox_operand(op.operands[attrs_start + idx]);
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                attrs_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                &format!("class_attr_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(elem_ptr, value_bits)
                        .unwrap();
                }
                let bases_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(bases_alloca, i64_ty, "class_bases_ptr")
                    .unwrap();
                let attrs_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(attrs_alloca, i64_ty, "class_attrs_ptr")
                    .unwrap();
                let class_def_fn = self.ensure_runtime_i64_fn("molt_guarded_class_def", 8);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        class_def_fn,
                        &[
                            name_bits.into(),
                            bases_ptr_bits.into(),
                            i64_ty.const_int(nbases as u64, false).into(),
                            attrs_ptr_bits.into(),
                            i64_ty.const_int(nattrs as u64, false).into(),
                            i64_ty.const_int(layout_size as u64, true).into(),
                            i64_ty.const_int(layout_version as u64, true).into(),
                            i64_ty.const_int(flags as u64, true).into(),
                        ],
                        "class_def",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_new" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let module_new_fn = self.ensure_runtime_i64_fn("molt_module_new", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let result = self
                    .backend
                    .builder
                    .build_call(module_new_fn, &[name_bits.into()], "module_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_cache_get" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let get_fn = self.ensure_runtime_i64_fn("molt_module_cache_get", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let result = self
                    .backend
                    .builder
                    .build_call(get_fn, &[name_bits.into()], "module_cache_get")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_cache_set" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_module_cache_set", 2);
                let name_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let module_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[name_bits.into(), module_bits.into()],
                        "module_cache_set",
                    )
                    .unwrap();
                true
            }
            "module_cache_del" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let del_fn = self.ensure_runtime_i64_fn("molt_module_cache_del", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let _ = self
                    .backend
                    .builder
                    .build_call(del_fn, &[name_bits.into()], "module_cache_del")
                    .unwrap();
                true
            }
            "module_get_attr" | "module_import_from" => {
                if op.operands.len() != 2 {
                    return false;
                }
                // `from M import name` (module_import_from) uses CPython
                // IMPORT_FROM semantics — ImportError on miss with a sys.modules
                // submodule fallback; plain `M.name` raises AttributeError.
                let runtime_symbol = if kind == "module_import_from" {
                    "molt_module_import_from"
                } else {
                    "molt_module_get_attr"
                };
                let get_fn = self.ensure_runtime_i64_fn(runtime_symbol, 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[module_bits.into(), attr_bits.into()],
                        "module_get_attr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_get_global" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_module_get_global", 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[module_bits.into(), attr_bits.into()],
                        "module_get_global",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_set_attr" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_module_set_attr", 3);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let val_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[module_bits.into(), attr_bits.into(), val_bits.into()],
                        "module_set_attr",
                    )
                    .unwrap();
                true
            }
            "module_del_global" | "module_del_global_if_present" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let runtime_name = if kind == "module_del_global_if_present" {
                    "molt_module_del_global_if_present"
                } else {
                    "molt_module_del_global"
                };
                let del_fn = self.ensure_runtime_i64_fn(runtime_name, 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let _ = self
                    .backend
                    .builder
                    .build_call(del_fn, &[module_bits.into(), attr_bits.into()], kind)
                    .unwrap();
                true
            }
            "dict_update" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update", 2);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let other_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into(), other_bits.into()], "dict_update")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_update_missing" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update_missing", 3);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let key_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let val_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), val_bits.into()],
                        "dict_update_missing",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_update_kwstar" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update_kwstar", 2);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let mapping_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), mapping_bits.into()],
                        "dict_update_kwstar",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_clear" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_clear", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_copy" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_copy", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_copy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dict_popitem" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_popitem", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_popitem")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_class" => {
                let Some(&kind_id) = op.operands.first() else {
                    return false;
                };
                let class_fn = self.ensure_runtime_i64_fn("molt_exception_class", 1);
                let kind_bits = self.ensure_i64(self.resolve(kind_id));
                let result = self
                    .backend
                    .builder
                    .build_call(class_fn, &[kind_bits.into()], "exception_class")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new", 2);
                let kind_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let args_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[kind_bits.into(), args_bits.into()],
                        "exception_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin" => {
                let Some(&args_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin", 2);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let args_bits = self.ensure_i64(self.resolve(args_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[tag_val.into(), args_bits.into()],
                        "exception_new_builtin",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin_empty" => {
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin_empty", 1);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let result = self
                    .backend
                    .builder
                    .build_call(new_fn, &[tag_val.into()], "exception_new_builtin_empty")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin_one" => {
                let Some(&arg_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin_one", 2);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let arg_bits = self.ensure_i64(self.resolve(arg_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[tag_val.into(), arg_bits.into()],
                        "exception_new_builtin_one",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_push" => {
                let push_fn = self.ensure_runtime_i64_fn("molt_exception_push", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(push_fn, &[], "exception_push")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_enter" => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "exception_stack_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_exception_stack_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "exception_stack_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_set_depth" => {
                let Some(&depth_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_stack_set_depth", 1);
                let depth_bits = self.ensure_i64(self.resolve(depth_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[depth_bits.into()], "exception_stack_set_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_exit" => {
                let Some(&prev_id) = op.operands.first() else {
                    return false;
                };
                let exit_fn = self.ensure_runtime_i64_fn("molt_exception_stack_exit", 1);
                let prev_bits = self.ensure_i64(self.resolve(prev_id));
                let result = self
                    .backend
                    .builder
                    .build_call(exit_fn, &[prev_bits.into()], "exception_stack_exit")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_pop" => {
                let pop_fn = self.ensure_runtime_i64_fn("molt_exception_pop", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(pop_fn, &[], "exception_pop")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_stack_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_stack_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last" => {
                let last_fn = self.ensure_runtime_i64_fn("molt_exception_last", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last_pending" | "exception_finally_pending_observer" => {
                let last_fn = self.ensure_runtime_i64_fn("molt_exception_last_pending", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_active" => {
                let active_fn = self.ensure_runtime_i64_fn("molt_exception_active", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(active_fn, &[], "exception_active")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_current" => {
                let current_fn = self.ensure_runtime_i64_fn("molt_exception_current", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(current_fn, &[], "exception_current")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_enter_handler" => {
                let Some(&captured_id) = op.operands.first() else {
                    return false;
                };
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_enter_handler", 1);
                let captured_bits = self.ensure_i64(self.resolve(captured_id));
                let result = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[captured_bits.into()], "exception_enter_handler")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_resolve_captured" => {
                let Some(&captured_id) = op.operands.first() else {
                    return false;
                };
                let resolve_fn = self.ensure_runtime_i64_fn("molt_exception_resolve_captured", 1);
                let captured_bits = self.ensure_i64(self.resolve(captured_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        resolve_fn,
                        &[captured_bits.into()],
                        "exception_resolve_captured",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_set_last" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_set_last", 1);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[exc_bits.into()], "exception_set_last")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_context_set" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_context_set", 1);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[exc_bits.into()], "exception_context_set")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "builtin_type" => {
                let Some(&tag_id) = op.operands.first() else {
                    return false;
                };
                let builtin_type_fn = self.ensure_runtime_i64_fn("molt_builtin_type", 1);
                let tag_value = self.resolve(tag_id);
                let tag_ty = self
                    .value_types
                    .get(&tag_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let tag_bits = self.materialize_dynbox_bits(tag_value, &tag_ty);
                let result = self
                    .backend
                    .builder
                    .build_call(builtin_type_fn, &[tag_bits.into()], "builtin_type")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_apply_set_name" => {
                let Some(&class_id) = op.operands.first() else {
                    return false;
                };
                let apply_fn = self.ensure_runtime_i64_fn("molt_class_apply_set_name", 1);
                let class_bits = self.ensure_i64(self.resolve(class_id));
                let result = self
                    .backend
                    .builder
                    .build_call(apply_fn, &[class_bits.into()], "class_apply_set_name")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_layout_version" => {
                let Some(&class_id) = op.operands.first() else {
                    return false;
                };
                let version_fn = self.ensure_runtime_i64_fn("molt_class_layout_version", 1);
                let class_bits = self.ensure_i64(self.resolve(class_id));
                let result = self
                    .backend
                    .builder
                    .build_call(version_fn, &[class_bits.into()], "class_layout_version")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_set_layout_version" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_class_set_layout_version", 2);
                let class_bits = self.materialize_dynbox_operand(op.operands[0]);
                let version_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[class_bits.into(), version_bits.into()],
                        "class_set_layout_version",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "object_set_class" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let set_fn = self.ensure_runtime_i64_fn("molt_object_set_class", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[obj_ptr_bits.into(), class_bits.into()],
                        "object_set_class",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_merge_layout" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let merge_fn = self.ensure_runtime_i64_fn("molt_class_merge_layout", 3);
                let class_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offsets_bits = self.materialize_dynbox_operand(op.operands[1]);
                let size_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        merge_fn,
                        &[class_bits.into(), offsets_bits.into(), size_bits.into()],
                        "class_merge_layout",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "str_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let str_fn = self.ensure_runtime_i64_fn("molt_str_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(str_fn, &[src_bits.into()], "str_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── repr(x): a fresh owned string, NOT operand 0. ──
            // Mirrors WASM/Luau/native `repr_from_obj` → `molt_repr_from_obj`.
            // Without this arm the Copy fell through to the bit-passthrough,
            // silently returning `x` (a wrong-result miscompile) and aliasing it
            // (a drop-insertion double-free). One operand, owned result.
            "repr_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let repr_fn = self.ensure_runtime_i64_fn("molt_repr_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(repr_fn, &[src_bits.into()], "repr_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── int(x[, base]): a fresh owned int object, NOT operand 0. ──
            // `molt_int_from_obj(val, base, has_base)`. The frontend always emits
            // the 3-operand form (base / has_base default to the no-base sentinel).
            "int_from_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let int_fn = self.ensure_runtime_i64_fn("molt_int_from_obj", 3);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let base = self.materialize_dynbox_operand(op.operands[1]);
                let has_base = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        int_fn,
                        &[val.into(), base.into(), has_base.into()],
                        "int_from_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── float(x): a fresh owned float object, NOT operand 0. ──
            "float_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let float_fn = self.ensure_runtime_i64_fn("molt_float_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(float_fn, &[src_bits.into()], "float_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── obj[start:end] (the slice subscript): a fresh owned object, NOT
            //    operand 0. `molt_slice(obj, start, end)`. THIS is the exact
            //    adversarial-review P0 #1 double-free vector — `s[-5:]` fell
            //    through to the passthrough, returned `s`, and was double-freed. ──
            "slice" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let slice_fn = self.ensure_runtime_i64_fn("molt_slice", 3);
                let obj = self.materialize_dynbox_operand(op.operands[0]);
                let start = self.materialize_dynbox_operand(op.operands[1]);
                let end = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(slice_fn, &[obj.into(), start.into(), end.into()], "slice")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── format(val, spec) (f-string field / format()): fresh owned str. ──
            // `molt_format_builtin(val, spec)`.
            "string_format" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let fmt_fn = self.ensure_runtime_i64_fn("molt_format_builtin", 2);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let spec = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(fmt_fn, &[val.into(), spec.into()], "string_format")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // NOTE: `contains` (the `x in y` membership test) is ALSO a fresh-value
            // `Copy` kind (classified `FreshValue` in `alias_analysis`), but it is
            // already lowered explicitly further down via `emit_containment`
            // (`molt_contains` + `NotIn` negation). It therefore never reaches the
            // `Copy` passthrough fatal gate, and adding a second `"contains" =>` arm
            // here would be an unreachable duplicate. Left to its established arm.
            // ── ascii(x): fresh owned str. ──
            "ascii_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let ascii_fn = self.ensure_runtime_i64_fn("molt_ascii_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(ascii_fn, &[src_bits.into()], "ascii_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── slice(start, stop, step): a fresh owned slice object. ──
            "slice_new" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let slice_new_fn = self.ensure_runtime_i64_fn("molt_slice_new", 3);
                let start = self.materialize_dynbox_operand(op.operands[0]);
                let stop = self.materialize_dynbox_operand(op.operands[1]);
                let step = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        slice_new_fn,
                        &[start.into(), stop.into(), step.into()],
                        "slice_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── dict.keys()/values()/items(): fresh owned view objects. ──
            "dict_keys" | "dict_values" | "dict_items" => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let symbol = match kind {
                    "dict_keys" => "molt_dict_keys",
                    "dict_values" => "molt_dict_values",
                    _ => "molt_dict_items",
                };
                let view_fn = self.ensure_runtime_i64_fn(symbol, 1);
                let dict_bits = self.materialize_dynbox_operand(dict_id);
                let result = self
                    .backend
                    .builder
                    .build_call(view_fn, &[dict_bits.into()], kind)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── enumerate(iterable[, start]): a fresh owned enumerate object. ──
            "enumerate" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let enum_fn = self.ensure_runtime_i64_fn("molt_enumerate", 3);
                let iterable = self.materialize_dynbox_operand(op.operands[0]);
                let start = self.materialize_dynbox_operand(op.operands[1]);
                let has_start = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        enum_fn,
                        &[iterable.into(), start.into(), has_start.into()],
                        "enumerate",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── dict(x): a fresh owned dict. ──
            "dict_from_obj" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let dict_fn = self.ensure_runtime_i64_fn("molt_dict_from_obj", 1);
                let obj_bits = self.materialize_dynbox_operand(obj_id);
                let result = self
                    .backend
                    .builder
                    .build_call(dict_fn, &[obj_bits.into()], "dict_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── complex(real[, imag]): a fresh owned complex. ──
            "complex_from_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let complex_fn = self.ensure_runtime_i64_fn("molt_complex_from_obj", 3);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let imag = self.materialize_dynbox_operand(op.operands[1]);
                let has_imag = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        complex_fn,
                        &[val.into(), imag.into(), has_imag.into()],
                        "complex_from_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // ── object(): a fresh owned bare object. No operands. ──
            "object_new" => {
                let object_new_fn = self.ensure_runtime_i64_fn("molt_object_new", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(object_new_fn, &[], "object_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "int_from_str_of_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let int_fn = self.ensure_runtime_i64_fn("molt_int_from_str_of_obj", 3);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let base_bits = self.materialize_dynbox_operand(op.operands[1]);
                let has_base_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        int_fn,
                        &[val_bits.into(), base_bits.into(), has_base_bits.into()],
                        "int_from_str_of_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "ord" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let ord_fn = self.ensure_runtime_i64_fn("molt_ord", 1);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_fn, &[val_bits.into()], "ord")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "ord_at" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let ord_fn = self.ensure_runtime_i64_fn("molt_ord_at", 2);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let index_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_fn, &[obj_bits.into(), index_bits.into()], "ord_at")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "string_join" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let join_fn = self.ensure_runtime_i64_fn("molt_string_join", 2);
                let sep_bits = self.materialize_dynbox_operand(op.operands[0]);
                let items_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        join_fn,
                        &[sep_bits.into(), items_bits.into()],
                        "string_join",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "isinstance" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let isinstance_fn = self.ensure_runtime_i64_fn("molt_isinstance", 2);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let class_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        isinstance_fn,
                        &[obj_bits.into(), class_bits.into()],
                        "isinstance",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_match_builtin" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let match_fn = self.ensure_runtime_i64_fn("molt_exception_match_builtin", 2);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        match_fn,
                        &[exc_bits.into(), tag_val.into()],
                        "exception_match_builtin",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "issubclass" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let issubclass_fn = self.ensure_runtime_i64_fn("molt_issubclass", 2);
                let sub_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let class_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        issubclass_fn,
                        &[sub_bits.into(), class_bits.into()],
                        "issubclass",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "has_attr_name" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let has_attr_fn = self.ensure_runtime_i64_fn("molt_has_attr_name", 2);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        has_attr_fn,
                        &[obj_bits.into(), name_bits.into()],
                        "has_attr_name",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "type_of" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let type_of_fn = self.ensure_runtime_i64_fn("molt_type_of", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(type_of_fn, &[obj_bits.into()], "type_of")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "missing" => {
                let missing_fn = self.ensure_runtime_i64_fn("molt_missing", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(missing_fn, &[], "missing")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "is_callable" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let callable_fn = self.ensure_runtime_i64_fn("molt_is_callable", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(callable_fn, &[obj_bits.into()], "is_callable")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "get_attr_name_default" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_name_default", 3);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let default_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[obj_bits.into(), name_bits.into(), default_bits.into()],
                        "get_attr_name_default",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_context_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "context_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_unwind_to" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let unwind_fn = self.ensure_runtime_i64_fn("molt_context_unwind_to", 2);
                let depth_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let exc_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        unwind_fn,
                        &[depth_bits.into(), exc_bits.into()],
                        "context_unwind_to",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dataclass_new" => {
                if op.operands.len() != 4 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let field_names_bits = self.materialize_dynbox_operand(op.operands[1]);
                let values_bits = self.materialize_dynbox_operand(op.operands[2]);
                let flags_bits = self.materialize_dynbox_operand(op.operands[3]);
                let ctor_fn = self.ensure_runtime_i64_fn("molt_dataclass_new", 4);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        ctor_fn,
                        &[
                            name_bits.into(),
                            field_names_bits.into(),
                            values_bits.into(),
                            flags_bits.into(),
                        ],
                        "dataclass_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dataclass_new_values" => {
                if op.operands.len() < 3 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let field_names_bits = self.materialize_dynbox_operand(op.operands[1]);
                let flags_bits = self.materialize_dynbox_operand(op.operands[2]);
                let value_ids = &op.operands[3..];
                let values_ptr_bits = if value_ids.is_empty() {
                    i64_ty.const_zero()
                } else {
                    let values_alloca = self
                        .backend
                        .builder
                        .build_array_alloca(
                            i64_ty,
                            i64_ty.const_int(value_ids.len() as u64, false),
                            "dataclass_values",
                        )
                        .unwrap();
                    for (idx, &value_id) in value_ids.iter().enumerate() {
                        let value_bits = self.materialize_dynbox_operand(value_id);
                        let elem_ptr = unsafe {
                            self.backend
                                .builder
                                .build_gep(
                                    i64_ty,
                                    values_alloca,
                                    &[i64_ty.const_int(idx as u64, false)],
                                    &format!("dataclass_value_ptr_{idx}"),
                                )
                                .unwrap()
                        };
                        self.backend
                            .builder
                            .build_store(elem_ptr, value_bits)
                            .unwrap();
                    }
                    self.backend
                        .builder
                        .build_ptr_to_int(values_alloca, i64_ty, "dataclass_values_ptr")
                        .unwrap()
                };
                let ctor_fn = self.ensure_runtime_i64_fn("molt_dataclass_new_from_values", 5);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        ctor_fn,
                        &[
                            name_bits.into(),
                            field_names_bits.into(),
                            values_ptr_bits.into(),
                            i64_ty.const_int(value_ids.len() as u64, false).into(),
                            flags_bits.into(),
                        ],
                        "dataclass_new_values",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // ── Preserved value-producing / side-effecting ops whose runtime
            //    symbol name DIFFERS from `molt_<kind>` (so the generic
            //    `try_lower_preserved_runtime_call` fallback declines them), or
            //    which are RESULT-LESS side effects the runtime-call fallback
            //    refuses on principle. Each is the byte-for-byte LLVM analogue
            //    of the native (`function_compiler{,/fc/*}.rs`) handler, with the
            //    SAME runtime symbol and operand convention. Before these arms
            //    landed, every one of these kinds fell to the `Copy`
            //    passthrough: 0-operand singletons (`...`, `NotImplemented`)
            //    became `None`; `abs(x)` returned `x`; generator `throw`/`close`,
            //    the `__cause__` chain link, special-attr loads, the RC alias
            //    ops, and the type/layout guards were all silently DROPPED. ──

            // `abs(x)` — boxed builtin (the native int-lane branchless fast path
            // does not apply on the TIR/LLVM lane, which has no raw-int primary
            // vars; the boxed path is correct and overflow-safe for BigInt).
            // Symbol is `molt_abs_builtin`, NOT `molt_abs`.
            "abs" => {
                let Some(&x_id) = op.operands.first() else {
                    return false;
                };
                let abs_fn = self.ensure_runtime_i64_fn("molt_abs_builtin", 1);
                let x_bits = self.materialize_dynbox_operand(x_id);
                let result = self
                    .backend
                    .builder
                    .build_call(abs_fn, &[x_bits.into()], "abs")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `...` literal → the Ellipsis singleton. Symbol `molt_ellipsis`,
            // NOT `molt_const_ellipsis`. 0 operands.
            "const_ellipsis" => {
                let ell_fn = self.ensure_runtime_i64_fn("molt_ellipsis", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ell_fn, &[], "const_ellipsis")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `NotImplemented` singleton (e.g. a `__eq__` returning it). Symbol
            // `molt_not_implemented`, NOT `molt_const_not_implemented`.
            "const_not_implemented" => {
                let ni_fn = self.ensure_runtime_i64_fn("molt_not_implemented", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ni_fn, &[], "const_not_implemented")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `gen.throw(exc)` → `molt_generator_throw(gen, val)` (operands
            // [gen, val]). Symbol differs from `molt_gen_throw`.
            "gen_throw" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let throw_fn = self.ensure_runtime_i64_fn("molt_generator_throw", 2);
                let gen_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(throw_fn, &[gen_bits.into(), val_bits.into()], "gen_throw")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `gen.close()` → `molt_generator_close(gen)` (operand [gen]).
            // Symbol differs from `molt_gen_close`.
            "gen_close" => {
                let Some(&gen_id) = op.operands.first() else {
                    return false;
                };
                let close_fn = self.ensure_runtime_i64_fn("molt_generator_close", 1);
                let gen_bits = self.materialize_dynbox_operand(gen_id);
                let result = self
                    .backend
                    .builder
                    .build_call(close_fn, &[gen_bits.into()], "gen_close")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `raise X from Y` cause link → `molt_exception_set_cause(exc,
            // cause)`. Symbol matches `molt_<kind>`, but the op is frequently
            // RESULT-LESS (a pure side effect) so the runtime-call fallback
            // declines it (its `op.results.first()` early-return). Emit the call
            // unconditionally; bind the result only when present. Mirrors the
            // existing `exception_set_last` arm.
            "exception_set_cause" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_set_cause", 2);
                let exc_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let cause_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[exc_bits.into(), cause_bits.into()],
                        "exception_set_cause",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // Special-attribute load (`__class__`, `__name__`, …) →
            // `molt_get_attr_special(obj, name_ptr, name_len)`. The attribute
            // name is a compile-time string carried in `s_value`, materialized as
            // a private constant (the label-carrying convention, identical to the
            // native handler and the `call_method_ic` arm above).
            //
            // OWNERSHIP: `molt_get_attr_special` returns a BORROWED reference
            // (the value comes from `class_attr_lookup` / a descriptor / a slot
            // — not a fresh allocation). The native handler
            // (`fc/attrs.rs::get_attr_special_obj`) therefore inc_refs the result
            // via `emit_maybe_ref_adjust_v2(res, molt_inc_ref_obj)` to take owned
            // ownership; the existing LLVM `get_attr_generic_obj` arm
            // (`molt_get_attr_object_ic`) does the same. We MUST mirror that here:
            // binding the borrowed result without the inc_ref under-counts it and
            // risks a premature free / use-after-free of the attribute object.
            "get_attr_special_obj" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let Some(attr_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let obj_bits = self.materialize_dynbox_operand(obj_id);
                let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&attr_name);
                let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_special", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[obj_bits.into(), name_ptr_bits.into(), name_len_bits.into()],
                        "get_attr_special_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                // Take owned ownership of the borrowed attribute result (mirrors
                // the native get-attr ref-adjust). `molt_inc_ref_obj` is a no-op
                // for NaN-boxed immediates, so this is safe for any tag.
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                self.backend
                    .builder
                    .build_call(
                        inc_fn,
                        &[self.ensure_i64(result).into()],
                        "get_attr_special_inc_ref",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // RC-alias ops: `borrow`/retained aliases == `inc_ref` then ALIAS the
            // value through (result == source operand). The native handlers
            // (`function_compiler.rs` `inc_ref|borrow` / retained aliases) emit
            // `molt_inc_ref_obj(src)` and `def_var(out, src)` — a plain Copy
            // passthrough would skip the inc_ref (a refcount LEAK). `release` is
            // the dual and is handled in its OWN arm below because its result
            // convention differs (it does NOT alias the source — see there).
            "borrow" | "identity_alias" | "binding_alias" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_bits = self.ensure_i64(src_val);
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                self.backend
                    .builder
                    .build_call(inc_fn, &[src_bits.into()], "")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let ty = self
                        .value_types
                        .get(&src_id)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, ty);
                }
                true
            }
            // `release` == `dec_ref` the source. CRITICAL: unlike `borrow`, the
            // result must NOT alias the source — after `molt_dec_ref_obj` the
            // source may be freed, so aliasing+using it is a use-after-free. The
            // native handler (`function_compiler.rs` `dec_ref|release`) dec_refs
            // the source and, when the op carries an out var, binds it to NONE
            // (`def_var_named(out, box_none())`), never to the released source.
            // We mirror that: emit the dec_ref, then bind any result to None.
            "release" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_bits = self.ensure_i64(src_val);
                let dec_fn = self.ensure_runtime_void_fn("molt_dec_ref_obj", 1);
                self.backend
                    .builder
                    .build_call(dec_fn, &[src_bits.into()], "")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = i64_ty
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // Generator-frame locals registration (introspection support for
            // `gi_frame.f_locals` / `frame_locals_set`). Result-less side effect:
            // `molt_gen_locals_register(func_addr, names_tuple, offsets_tuple)`
            // where `func_addr` is the ADDRESS of the generator function named in
            // `s_value` (cast to i64, exactly like `func_new`), and the two
            // operands are the boxed names/offsets tuples. Dropping it (the old
            // `Copy` passthrough) silently diverges generator-frame introspection
            // from CPython. Mirrors the native handler
            // (function_compiler.rs `gen_locals_register`).
            "gen_locals_register" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(&func_name, arity, false);
                let func_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "gen_locals_func_ptr",
                    )
                    .unwrap();
                let names_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offsets_bits = self.materialize_dynbox_operand(op.operands[1]);
                let reg_fn = self.ensure_runtime_i64_fn("molt_gen_locals_register", 3);
                self.backend
                    .builder
                    .build_call(
                        reg_fn,
                        &[func_addr.into(), names_bits.into(), offsets_bits.into()],
                        "gen_locals_register",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = i64_ty
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Type/tag guard: a runtime CHECK that raises `TypeError` on
            // mismatch; the return value is discarded (the op is result-less on
            // native). `molt_guard_type(val, expected)`. A passthrough here would
            // SILENTLY ELIDE the guard — the program would not raise where
            // CPython does. (`guard_type` is the canonical kind and IS mapped to
            // a dedicated TIR `OpCode::TypeGuard`; only the `guard_tag` alias
            // reaches here as a preserved `Copy`, but we keep `guard_type` in the
            // arm for completeness/idempotence.)
            "guard_type" | "guard_tag" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_type", 2);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let expected_bits = self.materialize_dynbox_operand(op.operands[1]);
                self.backend
                    .builder
                    .build_call(
                        guard_fn,
                        &[val_bits.into(), expected_bits.into()],
                        "guard_type",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    // Guard return is the conventional sentinel; rebind only if a
                    // result was requested (native discards it).
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Layout / dict-shape guard (polymorphic-inline-cache fast path):
            // `molt_guard_layout_ptr(obj_ptr, class, expected_version)`. Both
            // `guard_layout` and `guard_dict_shape` share this single runtime
            // entry (the native handler groups them identically). Three points
            // make the generic `molt_<kind>` fallback INCAPABLE of lowering these
            // correctly — hence the dedicated arm:
            //   1. The runtime symbol is `molt_guard_layout_ptr`, not
            //      `molt_guard_layout` / `molt_guard_dict_shape`.
            //   2. The first argument is the RAW UNBOXED heap pointer of the
            //      object (`unbox_ptr_bits`), not the NaN-boxed value — mirroring
            //      the native `unbox_ptr_value(*obj)` before the call.
            //   3. The op carries a result on the IC fast path, but the guard
            //      VALUE is conventionally discarded (it raises on mismatch).
            // A `Copy` passthrough here would silently ELIDE the shape check, so
            // a stale-layout object would skip the deopt/guard and the program
            // would not raise / would read the wrong slot where CPython is
            // type-safe. operands = [obj, class, expected_version].
            "guard_layout" | "guard_dict_shape" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let version_bits = self.materialize_dynbox_operand(op.operands[2]);
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_layout_ptr", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        guard_fn,
                        &[obj_ptr.into(), class_bits.into(), version_bits.into()],
                        "guard_layout",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "guarded_field_init" => {
                if op.operands.len() != 4 {
                    return false;
                }
                let Some(attr_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let expected_version = self.materialize_dynbox_operand(op.operands[2]);
                let val_bits = self.materialize_dynbox_operand(op.operands[3]);
                let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(&attr_name);
                let init_fn = self.ensure_runtime_i64_fn("molt_guarded_field_init_ptr", 7);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        init_fn,
                        &[
                            obj_ptr_bits.into(),
                            class_bits.into(),
                            expected_version.into(),
                            i64_ty.const_int(offset as u64, true).into(),
                            val_bits.into(),
                            attr_ptr_bits.into(),
                            attr_len_bits.into(),
                        ],
                        "guarded_field_init",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Structured-data scalar parse (`json.loads`/`msgpack`/`cbor` on a
            // single scalar): `molt_<fmt>_parse_scalar_obj(value)`. The native
            // handler (`fc::parse_ops::handle_parse_op`) has a raw-pointer FAST
            // path (it reads `{arg}_ptr`/`{arg}_len` companion vars into a stack
            // out-param via `molt_<fmt>_parse_scalar`) AND this boxed SLOW path
            // for the general case. The fast path is a pure perf optimization
            // keyed on a native-only raw-string-pointer var convention the
            // TIR/LLVM lane does not carry; the slow `*_scalar_obj(value)` call is
            // the SEMANTICALLY COMPLETE lowering native falls back to whenever the
            // companion vars are absent (its `else` branch), so the LLVM lane uses
            // it unconditionally — same result, no fast-path reboxing avoidance.
            // The generic `molt_<kind>` fallback cannot claim these: the symbol is
            // `molt_<fmt>_parse_scalar_obj`, not `molt_<fmt>_parse`. operands =
            // [value]. A `Copy` passthrough would return the unparsed input.
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                let Some(&val_id) = op.operands.first() else {
                    return false;
                };
                let symbol = match kind {
                    "json_parse" => "molt_json_parse_scalar_obj",
                    "msgpack_parse" => "molt_msgpack_parse_scalar_obj",
                    "cbor_parse" => "molt_cbor_parse_scalar_obj",
                    _ => unreachable!("outer match restricts kind to the three parse ops"),
                };
                let val_bits = self.materialize_dynbox_operand(val_id);
                let parse_fn = self.ensure_runtime_i64_fn(symbol, 1);
                let result = self
                    .backend
                    .builder
                    .build_call(parse_fn, &[val_bits.into()], "parse_scalar")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // ── Arithmetic / comparison / bitwise carried as preserved `Copy`
            //    ops ──
            //
            // The SimpleIR→TIR lift (`kind_to_opcode`) maps some operator kinds
            // the frontend emits — `floordiv`, `invert`, `contains`, the
            // `inplace_bit_*` family, `matmul`, `pow_mod` — to `OpCode::Copy`
            // with `_original_kind` preserved, rather than to their dedicated
            // opcodes. The native/Cranelift and WASM lanes consume SimpleIR
            // directly (where these are real op kinds) and are unaffected, but
            // the LLVM lane lowers the TIR, where these arrive as `Copy`. Without
            // this arm the generic `Copy` handler falls through to "pass through
            // operand 0", silently replacing e.g. `a // b` with `a` (and dropping
            // any exception the operator would raise) — a silent miscompile of
            // every such operator on the LLVM lane.
            //
            // Each kind is lowered with the SAME emit helper its dedicated opcode
            // uses (the helpers take the operator name as a parameter and do not
            // read `op.opcode`; `emit_containment` checks only for `NotIn`, so the
            // `Copy`-carried `in`/`contains` correctly take the non-negated path).
            // `matmul`/`pow_mod` have no dedicated opcode or arith-specialized
            // path, so they lower to their boxed runtime calls (mirroring WASM).
            "floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "invert" => {
                self.emit_unary(op, "invert");
                true
            }
            "contains" => {
                // `x in y`. `emit_containment` negates only for `OpCode::NotIn`;
                // a `Copy`-carried `contains` is the affirmative membership test.
                self.emit_containment(op);
                true
            }
            "inplace_bit_and" => {
                self.emit_bitwise(op, "bit_and");
                true
            }
            "inplace_bit_or" => {
                self.emit_bitwise(op, "bit_or");
                true
            }
            "inplace_bit_xor" => {
                self.emit_bitwise(op, "bit_xor");
                true
            }
            // In-place augmented arithmetic for `//=`, `%=`, `**=`, `<<=`, `>>=`.
            // These ride `Copy{_original_kind}` (no first-class opcode, mirroring
            // `floordiv`/`inplace_bit_*`). We lower them with the SAME fast int/
            // float lane emitter as their binary opcode (the static int/float path
            // is byte-identical — builtin numerics have no in-place dunder), and
            // `emit_binary_arith`/`emit_bitwise` detect the `inplace_` prefix on
            // `_original_kind` to route the BOXED slow path to
            // `molt_inplace_<op>` (which tries `__i<op>__` first). `@=`/
            // `inplace_matmul` has no arith-specialized path and falls through to
            // the generic runtime-call fallback below, which emits
            // `molt_inplace_matmul`.
            "inplace_div" => {
                self.emit_binary_arith(op, "div");
                true
            }
            "inplace_floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "inplace_mod" => {
                self.emit_binary_arith(op, "mod");
                true
            }
            "inplace_pow" => {
                self.emit_binary_arith(op, "pow");
                true
            }
            "inplace_lshift" => {
                self.emit_bitwise(op, "lshift");
                true
            }
            "inplace_rshift" => {
                self.emit_bitwise(op, "rshift");
                true
            }

            // Generic preserved-op runtime-call fallback. Every other operator /
            // conversion kind the frontend emits that has no dedicated TIR opcode
            // (the `*_from_obj`/`*_from_str` conversion family, `matmul`,
            // `pow_mod`, …) is lifted to `OpCode::Copy` with `_original_kind`
            // preserved. The native/Cranelift and WASM lanes restore it to a
            // SimpleIR op and lower it as the runtime call `molt_<kind>`
            // (operands map 1:1 to the runtime ABI's positional i64 args). The
            // LLVM lane lowers the TIR directly, so it does the same here: when
            // `molt_<kind>` is a real symbol in the linked staticlib's intrinsic
            // surface, emit the boxed-operand runtime call. Anything else returns
            // `false` so the `Copy` fail-loud guard turns a genuinely unmappable
            // value-producing op into a build error rather than a silent
            // operand-0 pass-through (the `floordiv`-as-`Copy` bug-class).
            _ => self.try_lower_preserved_runtime_call(op, kind),
        }
    }

    /// Lower an unhandled preserved SimpleIR op (`Copy` with `_original_kind`)
    /// as the runtime call `molt_<kind>(boxed operands...)`, the same entry the
    /// SimpleIR-consuming backends dispatch to. Returns `false` (declining) when
    /// `molt_<kind>` is not a defined runtime intrinsic for the active profile —
    /// the op then hits the `Copy` fail-loud guard, which refuses to emit wrong
    /// code. The operand→arg mapping is positional (each operand a NaN-boxed
    /// i64), matching the runtime ABI for these `extern "C"` conversion/operator
    /// functions; the boxed return value is bound to the result when present.
    ///
    /// Covers BOTH value-producing and RESULT-LESS preserved ops. Result-less
    /// side effects (`print_newline`, `set_update`/`set_discard`/…,
    /// `dict_str_int_inc`/`dict_update`/…, `list_extend`/…) are emitted purely
    /// for their effect: the native handlers call `molt_<kind>` and bind the
    /// return only when the op carries an `out` var, exactly as we do here.
    /// Without the result-less path these ops fell to the `Copy` "1+ operands,
    /// 0 results → no-op" branch and were SILENTLY DROPPED (a missing newline, a
    /// set/dict mutation that never happened) — the same passthrough bug class as
    /// the value-producing ops, just manifesting as a dropped side effect rather
    /// than a wrong result. Ops needing a non-positional / non-boxed operand
    /// convention (unboxed pointer, compile-time string, function address) are
    /// claimed by their dedicated `match` arms BEFORE this generic fallback, so
    /// only the positional-boxed kinds reach here.
    /// `PRESERVED_VOID_RUNTIME_OPS` is checked before the default `molt_<kind>`
    /// i64-return ABI so result-less void calls are declared with the real C ABI.
    fn try_lower_preserved_runtime_call(&mut self, op: &TirOp, kind: &str) -> bool {
        if let Some((symbol, arity)) = preserved_void_runtime_call_abi(kind) {
            if op.operands.len() != arity || !op.results.is_empty() {
                return false;
            }
            if !self.backend.runtime_intrinsic_symbols.contains(symbol) {
                return false;
            }
            let arg_bits: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                .operands
                .iter()
                .map(|&id| self.materialize_dynbox_operand(id).into())
                .collect();
            let func = self.ensure_runtime_void_fn(symbol, arity);
            self.backend
                .builder
                .build_call(func, &arg_bits, symbol)
                .unwrap();
            return true;
        }

        let symbol = format!("molt_{kind}");
        if !self.backend.runtime_intrinsic_symbols.contains(&symbol) {
            return false;
        }
        let Some(return_abi) = classified_runtime_import_return_abi(&symbol, op.operands.len())
        else {
            self.record_fatal(format!(
                "preserved SimpleIR op `{kind}` maps to runtime symbol `{symbol}`, \
                 but that symbol has no LLVM ABI classification"
            ));
            return true;
        };
        let arg_bits: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
            .operands
            .iter()
            .map(|&id| self.materialize_dynbox_operand(id).into())
            .collect();
        match return_abi {
            RuntimeReturnAbi::Void => {
                if !op.results.is_empty() {
                    self.record_fatal(format!(
                        "preserved SimpleIR op `{kind}` maps to void runtime symbol `{symbol}` but has result values"
                    ));
                    return true;
                }
                let func = self.ensure_runtime_void_fn(&symbol, op.operands.len());
                self.backend
                    .builder
                    .build_call(func, &arg_bits, &symbol)
                    .unwrap();
            }
            RuntimeReturnAbi::I64 => {
                let func = self.ensure_runtime_i64_fn(&symbol, op.operands.len());
                let result = self
                    .backend
                    .builder
                    .build_call(func, &arg_bits, &symbol)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                // Bind the boxed return only when the op produces a value; a result-less
                // op was emitted purely for its side effect.
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
        }
        true
    }

    fn emit_call_bind_runtime(
        &self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let callable_i64 = self.ensure_i64(callable);
        let new_fn = self.ensure_runtime_i64_fn("molt_callargs_new", 2);
        let builder_val = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(arg_ids.len() as u64, false).into(),
                    i64_ty.const_int(0, false).into(),
                ],
                "callargs",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_pos", 2);
        for &arg_id in arg_ids {
            // The dynamic-call ABI (`molt_callargs_push_pos` -> `molt_call_bind`
            // -> trampoline) carries every argument as a NaN-boxed `DynBox`; the
            // callee trampoline then decodes each box into its parameter's raw
            // representation (`unbox_dynbox_to_param_ty_with_builder`). Passing a
            // raw scalar here (the old `ensure_i64`, a bitcast-level cast that
            // does NOT NaN-box) made the trampoline decode a raw `I64`/`F64`
            // payload as a boxed tag — e.g. a closure returning its arg, or a
            // bare `sum`/`format` result, surfaced as a denormal float / `15.0`.
            // `materialize_dynbox_operand` boxes per the value's representation
            // plan, mirroring the direct-call arg path (`coerce_to_tir_type`).
            let arg_i64 = self.materialize_dynbox_operand(arg_id);
            self.backend
                .builder
                .build_call(push_fn, &[builder_val.into(), arg_i64.into()], "push")
                .unwrap();
        }
        let bind_fn = self.ensure_runtime_i64_fn("molt_call_bind", 2);
        self.backend
            .builder
            .build_call(
                bind_fn,
                &[callable_i64.into(), builder_val.into()],
                "call_result",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    fn emit_call_func_runtime(
        &self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let callable_i64 = self.ensure_i64(callable);
        if arg_ids.len() <= 3 {
            let rt_name = format!("molt_call_func_fast{}", arg_ids.len());
            let fast_fn = self.ensure_runtime_i64_fn(&rt_name, arg_ids.len() + 1);
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                Vec::with_capacity(arg_ids.len() + 1);
            args.push(callable_i64.into());
            for &arg_id in arg_ids {
                // `molt_call_func_fast{N}` is the boxed-domain dynamic-dispatch
                // entry: it forwards each argument into the callee's trampoline,
                // which decodes a NaN-boxed `DynBox` into the parameter's raw
                // representation. A raw scalar passed here (the old `ensure_i64`)
                // is decoded as a boxed payload by the trampoline — the closure
                // call/return ABI carrier miscompile (#58/#37). Box per the
                // value's representation plan, exactly like the bind path above
                // and the direct-call path (`coerce_to_tir_type`).
                args.push(self.materialize_dynbox_operand(arg_id).into());
            }
            return self
                .backend
                .builder
                .build_call(fast_fn, &args, "call_func")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
        }
        self.emit_call_bind_runtime(callable, arg_ids)
    }

    fn emit_call_func_or_bind_runtime(
        &mut self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let callable_i64 = self.ensure_i64(callable);
        let is_func_fn = self.ensure_runtime_i64_fn("molt_is_function_obj", 1);
        let is_func_bits = self
            .backend
            .builder
            .build_call(is_func_fn, &[callable_i64.into()], "is_function_obj")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let truthy_fn = self.ensure_runtime_i64_fn("molt_is_truthy", 1);
        let is_func_truthy = self
            .backend
            .builder
            .build_call(truthy_fn, &[is_func_bits.into()], "is_function_truthy")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let cond_i1 = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                is_func_truthy,
                self.backend.context.i64_type().const_zero(),
                "call_func_fast_guard",
            )
            .unwrap();
        let current_fn = self.llvm_fn;
        let fast_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_fast");
        let bind_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_bind");
        let merge_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_merge");
        self.all_llvm_blocks.push(fast_bb);
        self.all_llvm_blocks.push(bind_bb);
        self.all_llvm_blocks.push(merge_bb);
        self.backend
            .builder
            .build_conditional_branch(cond_i1, fast_bb, bind_bb)
            .unwrap();

        self.backend.builder.position_at_end(fast_bb);
        let fast_result = self.emit_call_func_runtime(callable, arg_ids);
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let fast_exit_bb = self.backend.builder.get_insert_block().unwrap();

        self.backend.builder.position_at_end(bind_bb);
        let bind_result = self.emit_call_bind_runtime(callable, arg_ids);
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let bind_exit_bb = self.backend.builder.get_insert_block().unwrap();

        self.backend.builder.position_at_end(merge_bb);
        let phi = self
            .backend
            .builder
            .build_phi(self.backend.context.i64_type(), "call_func_or_bind_phi")
            .unwrap();
        phi.add_incoming(&[
            (&fast_result.into_int_value(), fast_exit_bb),
            (&bind_result.into_int_value(), bind_exit_bb),
        ]);
        phi.as_basic_value()
    }

    fn next_call_site_bits(&mut self, lane: &str) -> inkwell::values::IntValue<'ctx> {
        let site_id =
            crate::stable_ic_site_id(self.func.name.as_str(), self.call_site_counter, lane);
        self.call_site_counter += 1;
        let raw: BasicValueEnum<'ctx> = self
            .backend
            .context
            .i64_type()
            .const_int(site_id as u64, true)
            .into();
        self.materialize_dynbox_bits(raw, &TirType::I64)
    }

    fn generator_self_bits(&self) -> inkwell::values::IntValue<'ctx> {
        let idx = self
            .func
            .param_names
            .iter()
            .position(|name| name == "self")
            .unwrap_or(0);
        let value = self
            .llvm_fn
            .get_nth_param(idx as u32)
            .unwrap_or_else(|| self.backend.context.i64_type().const_zero().into());
        self.ensure_i64(value)
    }

    /// Map each `_poll` resume state id to the REAL TIR resume-continuation
    /// block (an entry in `block_map`), NOT a synthetic block.
    ///
    /// The single source of truth for the state → resume-block mapping is the
    /// entry block's `StateDispatch` terminator, which the SSA pass built from
    /// `cfg.state_resume_edges`: each `(state_id, resume_bid, args)` case names
    /// the real TIR block that the dispatch resumes into.  Lowering the dispatch
    /// to those real blocks (whose phis the SSA pass placed) is what makes the
    /// `_poll` state machine dominance-correct on LLVM — the old design created
    /// fresh synthetic `state_resume_*` blocks and `position_at_end`-ed the
    /// continuation into them, so the real TIR continuation block's phis were
    /// missing the dispatch incoming (the "Instruction does not dominate all
    /// uses!" class).
    ///
    /// The re-poll suspend ops (`state_transition` / `chan_*_yield`) carry a
    /// *pending* state id whose resume target is the suspend op's OWN block (it
    /// re-polls from its own position); those are also dispatch cases, so they
    /// are covered by the same `StateDispatch` case list.
    fn initialize_state_resume_blocks(&mut self) {
        let Some(entry) = self.func.blocks.get(&self.func.entry_block) else {
            return;
        };
        if let Terminator::StateDispatch { cases, .. } = &entry.terminator {
            // Clone the (state_id, block_id) pairs first to avoid borrowing
            // `self.func` while mutating `self.state_resume_blocks`.
            let pairs: Vec<(i64, BlockId)> =
                cases.iter().map(|(state, bid, _)| (*state, *bid)).collect();
            for (state_id, resume_bid) in pairs {
                if let Some(&bb) = self.block_map.get(&resume_bid) {
                    self.state_resume_blocks.insert(state_id, bb);
                }
            }
        }
    }

    fn const_i64_operand(&self, operand_id: ValueId) -> i64 {
        for block in self.func.blocks.values() {
            for op in &block.ops {
                if op.results.first() == Some(&operand_id)
                    && op.opcode == OpCode::ConstInt
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    return *v;
                }
            }
        }
        panic!(
            "expected const int operand {:?} in {}",
            operand_id, self.func.name
        );
    }

    fn raw_i64_operand(
        &self,
        operand_id: ValueId,
        current_bb: BasicBlock<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let value = self.resolve(operand_id);
        let source_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        self.coerce_to_tir_type(value, &source_ty, &TirType::I64, current_bb)
            .into_int_value()
    }

    fn resume_block_for_state(&self, state_id: i64) -> BasicBlock<'ctx> {
        *self
            .state_resume_blocks
            .get(&state_id)
            .unwrap_or_else(|| panic!("missing resume block for state {}", state_id))
    }

    /// Call a 2-argument runtime function that returns i64.
    ///
    /// The callee is declared on demand through the central runtime-import helper
    /// when it is not already in the fixed table. On-demand declarations carry
    /// only the globally valid runtime attributes; stronger facts such as
    /// `willreturn` must be promoted into `runtime_imports.rs`.
    fn call_runtime_2(
        &self,
        name: &str,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let func = self.ensure_runtime_i64_fn(name, 2);
        let lhs_i64 = self.ensure_i64(lhs);
        let rhs_i64 = self.ensure_i64(rhs);
        self.backend
            .builder
            .build_call(func, &[lhs_i64.into(), rhs_i64.into()], name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    /// Emit a global string constant and call `molt_string_from_bytes` to get
    /// a NaN-boxed string value at runtime.
    fn intern_string_const(&self, s: &str) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let sfb_fn = if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
            f
        } else {
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            let i32_ty = self.backend.context.i32_type();
            let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
            self.backend.module.add_function(
                "molt_string_from_bytes",
                fn_ty,
                Some(inkwell::module::Linkage::External),
            )
        };
        let name_bytes = s.as_bytes();
        let global = self.backend.module.add_global(
            self.backend
                .context
                .i8_type()
                .array_type(name_bytes.len() as u32),
            None,
            &format!(
                "__attr_str_{}",
                s.replace(|c: char| !c.is_alphanumeric(), "_")
            ),
        );
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(name_bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);
        let ptr = global.as_pointer_value();
        let len = i64_ty.const_int(name_bytes.len() as u64, false);
        let out_alloca = self
            .backend
            .builder
            .build_alloca(i64_ty, "intern_out")
            .unwrap();
        self.backend
            .builder
            .build_call(
                sfb_fn,
                &[ptr.into(), len.into(), out_alloca.into()],
                "intern_sfb",
            )
            .unwrap();
        self.backend
            .builder
            .build_load(i64_ty, out_alloca, "intern_bits")
            .unwrap()
    }
}

// ── Tests ──

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::*;
    use crate::llvm_backend::LlvmBackend;
    use crate::llvm_backend::runtime_imports::declare_runtime_functions;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};
    use inkwell::attributes::Attribute;
    use inkwell::context::Context;
    use inkwell::values::AnyValue;

    fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
        let backend = LlvmBackend::new(ctx, "test");
        declare_runtime_functions(ctx, &backend.module);
        backend
    }

    fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
        let kind_id = Attribute::get_named_enum_kind_id(attr_name);
        kind_id == 0
            || func
                .get_enum_attribute(AttributeLoc::Function, kind_id)
                .is_some()
    }

    fn lacks_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
        let kind_id = Attribute::get_named_enum_kind_id(attr_name);
        kind_id == 0
            || func
                .get_enum_attribute(AttributeLoc::Function, kind_id)
                .is_none()
    }

    fn assert_lowering_error_contains(err: &LlvmLoweringError, needle: &str) {
        let joined = err.diagnostics().join("\n");
        assert!(
            joined.contains(needle),
            "expected lowering diagnostic containing {needle:?}, got:\n{joined}"
        );
    }

    fn const_none_def(result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    #[test]
    fn boxed_or_retains_selected_operand_result() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "boxed_or_selected_owner".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Or,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_is_truthy"), "{ir}");
        assert!(
            ir.contains("call void @molt_inc_ref_obj(i64 %bool_or)"),
            "{ir}"
        );
    }

    fn const_int_def(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn make_dummy_lowering<'ctx, 'func>(
        backend: &'func LlvmBackend<'ctx>,
        func: &'func TirFunction,
        llvm_fn: FunctionValue<'ctx>,
    ) -> FunctionLowering<'ctx, 'func> {
        FunctionLowering {
            backend,
            func,
            llvm_fn,
            entry_trampoline_bb: None,
            block_map: HashMap::new(),
            values: HashMap::new(),
            value_types: HashMap::new(),
            pending_phis: Vec::new(),
            phi_edges: Vec::new(),
            pgo_branch_weights: None,
            pgo_weight_index: 0,
            const_str_counter: 0,
            synthetic_block_counter: 0,
            all_llvm_blocks: Vec::new(),
            llvm_pred_map: HashMap::new(),
            state_resume_blocks: HashMap::new(),
            try_stack_baselines: Vec::new(),
            call_site_counter: 0,
            diagnostics: RefCell::new(Vec::new()),
            repr_facts: crate::representation_plan::LlvmReprFacts::default(),
        }
    }

    #[test]
    #[should_panic(expected = "LLVM function type mismatch for `same_name`")]
    fn llvm_symbol_signature_mismatch_rejects_tir_forward_declaration() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        backend.module.add_function(
            "same_name",
            ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            Some(inkwell::module::Linkage::External),
        );
        let func = TirFunction::new("same_name".into(), vec![], TirType::I64);

        let _ = declare_tir_function(&func, &backend);
    }

    #[test]
    #[should_panic(expected = "LLVM function type mismatch for `molt_trace_exit`")]
    fn llvm_symbol_signature_mismatch_rejects_runtime_i64_reuse() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test");
        backend.module.add_function(
            "molt_trace_exit",
            ctx.void_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let dummy = TirFunction::new("dummy_runtime_symbol".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy_runtime_symbol",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

        let _ = lowering.ensure_runtime_i64_fn("molt_trace_exit", 0);
    }

    #[test]
    #[should_panic(expected = "LLVM function type mismatch for `molt_inc_ref_obj`")]
    fn llvm_symbol_signature_mismatch_rejects_runtime_void_reuse() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test");
        backend.module.add_function(
            "molt_inc_ref_obj",
            ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            Some(inkwell::module::Linkage::External),
        );
        let dummy = TirFunction::new("dummy_runtime_void_symbol".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy_runtime_void_symbol",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

        let _ = lowering.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
    }

    #[test]
    fn on_demand_runtime_declaration_uses_conservative_attributes() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test");
        let dummy = TirFunction::new("dummy_runtime_attrs".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy_runtime_attrs",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

        let func = lowering.ensure_runtime_i64_fn("molt_abs_builtin", 1);

        assert!(has_fn_attr(func, "nounwind"));
        assert!(
            lacks_fn_attr(func, "willreturn"),
            "ad-hoc runtime declarations must not claim termination"
        );
    }

    #[test]
    #[should_panic(
        expected = "LLVM runtime import `molt_unclassified_runtime_symbol` has no ABI classification"
    )]
    fn unclassified_runtime_declaration_rejects_new_symbol_drift() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test");
        let dummy = TirFunction::new("dummy_runtime_reject".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy_runtime_reject",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

        let _ = lowering.ensure_runtime_i64_fn("molt_unclassified_runtime_symbol", 2);
    }

    #[test]
    fn preserved_runtime_call_rejects_name_only_symbol_drift() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_intrinsic_symbols
            .insert("molt_unclassified_runtime_symbol".to_string());

        let err = lower_preserved_kind_ir(&backend, "unclassified_runtime_symbol", 2, true, None)
            .expect_err("name-only preserved runtime symbols must fail before LLVM declaration");
        assert_lowering_error_contains(&err, "has no LLVM ABI classification");
        assert_lowering_error_contains(&err, "molt_unclassified_runtime_symbol");
    }

    #[test]
    #[should_panic(expected = "LLVM function type mismatch for `gen_fn`")]
    fn llvm_symbol_signature_mismatch_rejects_function_symbol_reuse() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.module.add_function(
            "gen_fn",
            ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            Some(inkwell::module::Linkage::External),
        );
        backend
            .function_param_types
            .insert("gen_fn".to_string(), vec![TirType::DynBox, TirType::DynBox]);
        let dummy = TirFunction::new("dummy_function_symbol".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy_function_symbol",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

        let _ = lowering.ensure_function_symbol("gen_fn", 0, false);
    }

    #[test]
    fn lower_const_and_return() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn f() -> i64 { return 42 }
        let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
        let v0 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(42));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v0] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(ir.contains("const_ret"), "function name missing from IR");
        assert!(ir.contains("42"), "constant 42 missing from IR");
        assert!(ir.contains("ret "), "return instruction missing from IR");
    }

    #[test]
    fn lowers_exception_pop_then_dec_ref_from_shared_drop_shape() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new("exception_drop".into(), vec![], TirType::None);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(owned));
        let mut exception_pop = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        };
        exception_pop.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("exception_pop".into()),
        );
        entry.ops.push(exception_pop);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        let pop_pos = ir
            .find("molt_exception_pop")
            .unwrap_or_else(|| panic!("LLVM must call molt_exception_pop; IR:\n{ir}"));
        let dec_pos = ir
            .find("molt_dec_ref_obj")
            .unwrap_or_else(|| panic!("LLVM must call molt_dec_ref_obj; IR:\n{ir}"));
        assert!(
            pop_pos < dec_pos,
            "shared ExceptionRegion drops must lower after the owning exception_pop; IR:\n{ir}"
        );
    }

    #[test]
    fn missing_value_id_is_fatal_lowering_error() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new("missing_value".into(), vec![], TirType::I64);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return {
            values: vec![ValueId(99)],
        };

        let err = match try_lower_tir_to_llvm(&func, &backend) {
            Ok(_) => panic!("malformed TIR unexpectedly lowered successfully"),
            Err(err) => err,
        };
        assert_lowering_error_contains(&err, "ValueId %99 was used before being defined");
    }

    #[test]
    fn missing_phi_argument_is_fatal_lowering_error() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new("missing_phi_arg".into(), vec![], TirType::I64);
        let join_id = func.fresh_block();
        let join_arg = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: join_id,
            args: vec![],
        };
        func.blocks.insert(
            join_id,
            TirBlock {
                id: join_id,
                args: vec![TirValue {
                    id: join_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![join_arg],
                },
            },
        );

        let err = match try_lower_tir_to_llvm(&func, &backend) {
            Ok(_) => panic!("malformed phi unexpectedly lowered successfully"),
            Err(err) => err,
        };
        assert_lowering_error_contains(&err, "phi argument index 0 is required");
    }

    #[test]
    fn unreachable_predecessor_does_not_feed_phi() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new("dead_phi_pred".into(), vec![], TirType::DynBox);
        let join_id = func.fresh_block();
        let dead_id = func.fresh_block();
        let live_value = func.fresh_value();
        let join_arg = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(live_value));
        entry.terminator = Terminator::Branch {
            target: join_id,
            args: vec![live_value],
        };
        func.blocks.insert(
            join_id,
            TirBlock {
                id: join_id,
                args: vec![TirValue {
                    id: join_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![join_arg],
                },
            },
        );
        func.blocks.insert(
            dead_id,
            TirBlock {
                id: dead_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_id,
                    args: vec![ValueId(999)],
                },
            },
        );

        try_lower_tir_to_llvm(&func, &backend)
            .expect("dead TIR predecessor must not contribute to LLVM phi incoming values");
        backend
            .module
            .verify()
            .expect("dead predecessor phi lowering should verify");
    }

    #[test]
    fn check_exception_edge_feeds_handler_phi() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new("check_exception_phi".into(), vec![], TirType::DynBox);
        let exit_id = func.fresh_block();
        let handler_id = func.fresh_block();
        let live_value = func.fresh_value();
        let exit_value = func.fresh_value();
        let handler_arg = func.fresh_value();

        let mut handler_attrs = AttrDict::new();
        handler_attrs.insert("value".into(), AttrValue::Int(100));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(live_value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![live_value],
            results: vec![],
            attrs: handler_attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Branch {
            target: exit_id,
            args: vec![],
        };
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![const_none_def(exit_value)],
                terminator: Terminator::Return {
                    values: vec![exit_value],
                },
            },
        );
        func.blocks.insert(
            handler_id,
            TirBlock {
                id: handler_id,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![handler_arg],
                },
            },
        );
        func.has_exception_handling = true;
        func.label_id_map.insert(handler_id.0, 100);

        try_lower_tir_to_llvm(&func, &backend)
            .expect("check_exception operands must feed handler block phi args");
        backend
            .module
            .verify()
            .expect("check_exception handler phi lowering should verify");
    }

    /// Build the trivial `fn add(a: i64, b: i64) -> i64 { return a + b }` TIR
    /// used by the overflow-safety gating tests below.
    fn build_i64_add_func() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "add_i64".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let v_sum = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v_sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![v_sum],
        };
        (func, v_sum)
    }

    #[test]
    fn lower_i64_add_overflow_safe_uses_native_add() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);

        // Build: fn add(a: i64, b: i64) -> i64 { return a + b }, with the result
        // marked overflow-safe by the representation plan. The backend may then
        // emit a raw machine `add` instead of routing through the runtime.
        let (func, v_sum) = build_i64_add_func();
        let mut facts = crate::representation_plan::LlvmReprFacts::default();
        // A native machine `add` is sound only when BOTH operands and the result
        // are value-range-proven exact-i64 carriers. The two `i64` parameters
        // (entry args %0/%1) carry as boxed `DynBox` unless proven overflow-safe
        // (the parameter-ABI carrier rule), so prove all three here — the
        // realistic shape under which the plan admits raw machine arithmetic.
        for v in [ValueId(0), ValueId(1), v_sum] {
            facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
        }
        backend.function_repr_facts.insert(func.name.clone(), facts);

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("add i64"),
            "expected native i64 add for an overflow-safe result: {}",
            ir
        );
        assert!(
            !ir.contains("call") || !ir.contains("molt_add"),
            "overflow-safe i64+i64 add must NOT call the runtime: {}",
            ir
        );
    }

    #[test]
    fn lower_i64_add_not_overflow_safe_routes_to_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Same add, but with NO overflow-safety proof (empty plan facts). The
        // structural fix for the LLVM int-overflow miscompile requires this to
        // route through `molt_add` (BigInt-correct) rather than emit a raw
        // machine `add` that would silently wrap and truncate at box time.
        let (func, _v_sum) = build_i64_add_func();

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("call i64 @molt_add"),
            "non-overflow-safe i64+i64 add must route through molt_add: {}",
            ir
        );
    }

    #[test]
    fn lower_f64_add() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn fadd(a: f64, b: f64) -> f64 { return a + b }
        let mut func = TirFunction::new(
            "add_f64".into(),
            vec![TirType::F64, TirType::F64],
            TirType::F64,
        );
        let v_sum = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v_sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![v_sum],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("fadd double"),
            "expected native f64 add in IR: {}",
            ir
        );
    }

    #[test]
    fn lower_dynbox_add_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn dyn_add(a: DynBox, b: DynBox) -> DynBox { return a + b }
        let mut func = TirFunction::new(
            "dyn_add".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let v_sum = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v_sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![v_sum],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("molt_add"),
            "expected runtime call to molt_add in IR: {}",
            ir
        );
    }

    #[test]
    fn lower_conditional_branch() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn cond(flag: Bool) -> i64 { if flag: return 1 else: return 0 }
        let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

        let then_id = func.fresh_block();
        let else_id = func.fresh_block();
        let v_one = func.fresh_value();
        let v_zero = func.fresh_value();

        // Entry: cond branch on param 0
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };

        // Then block: return 1
        func.blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v_one],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(1));
                        m
                    },
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![v_one],
                },
            },
        );

        // Else block: return 0
        func.blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v_zero],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(0));
                        m
                    },
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![v_zero],
                },
            },
        );

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        // Should have 3 blocks and a conditional branch
        assert!(
            ir.contains("br i1"),
            "expected conditional branch in IR: {}",
            ir
        );
        assert!(ir.contains("bb1"), "expected then block in IR: {}", ir);
        assert!(ir.contains("bb2"), "expected else block in IR: {}", ir);
    }

    #[test]
    fn plain_trampoline_boxes_bool_return_into_i64_abi() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        let _target = backend.module.add_function(
            "helper_bool",
            ctx.bool_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        backend
            .function_return_types
            .insert("helper_bool".to_string(), TirType::Bool);
        let dummy = TirFunction::new("dummy".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);
        let trampoline = lowering.ensure_plain_trampoline("helper_bool", 0, false);

        assert_eq!(
            trampoline.get_type().get_return_type(),
            Some(ctx.i64_type().into())
        );
        backend.module.verify().expect("llvm module should verify");
        let ir = trampoline.print_to_string().to_string();
        assert!(ir.contains("box_bool") || ir.contains("zext_bool"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn plain_trampoline_boxes_f64_return_into_i64_abi() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        let _target = backend.module.add_function(
            "helper_f64",
            ctx.f64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        backend
            .function_return_types
            .insert("helper_f64".to_string(), TirType::F64);
        let dummy = TirFunction::new("dummy".into(), vec![], TirType::DynBox);
        let dummy_fn = backend.module.add_function(
            "dummy",
            ctx.i64_type().fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);
        let trampoline = lowering.ensure_plain_trampoline("helper_f64", 0, false);

        assert_eq!(
            trampoline.get_type().get_return_type(),
            Some(ctx.i64_type().into())
        );
        backend.module.verify().expect("llvm module should verify");
        let ir = trampoline.print_to_string().to_string();
        assert!(
            ir.contains("f64_to_i64") || ir.contains("bitcast double"),
            "{ir}"
        );
        assert!(ir.contains("fcmp uno"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn lower_call_guarded_uses_runtime_callable_dispatch_even_with_known_target() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        let _target = backend.module.add_function(
            "guarded_target",
            ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            Some(inkwell::module::Linkage::External),
        );
        backend
            .function_param_types
            .insert("guarded_target".to_string(), vec![TirType::DynBox]);
        backend
            .function_return_types
            .insert("guarded_target".to_string(), TirType::DynBox);

        let mut func = TirFunction::new("guarded_call_abi".into(), vec![], TirType::DynBox);
        let callable = func.fresh_value();
        let arg0 = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(callable), const_none_def(arg0)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![callable, arg0],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("call_guarded".into()),
                );
                attrs.insert("s_value".into(), AttrValue::Str("guarded_target".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(ir.contains("molt_call_func_fast1"), "{ir}");
        assert!(!ir.contains("call i64 @guarded_target"), "{ir}");
    }

    #[test]
    fn lower_import_uses_var_attr_fallback_for_module_name() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("import_var_fallback".into(), vec![], TirType::DynBox);
        let imported = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("_var".into(), AttrValue::Str("pathlib".into()));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Import,
            operands: vec![],
            results: vec![imported],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![imported],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_module_import"), "{ir}");
    }

    #[test]
    fn lower_direct_container_builders_box_raw_i64_elements() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("container_builder_boxing".into(), vec![], TirType::DynBox);
        let raw = func.fresh_value();
        let key = func.fresh_value();
        let list = func.fresh_value();
        let tuple = func.fresh_value();
        let set = func.fresh_value();
        let dict = func.fresh_value();
        let ret = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, 2));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![key],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("s_value".into(), AttrValue::Str("k".into()));
                attrs
            },
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildList,
            operands: vec![raw],
            results: vec![list],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildTuple,
            operands: vec![raw],
            results: vec![tuple],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildSet,
            operands: vec![raw],
            results: vec![set],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildDict,
            operands: vec![key, raw],
            results: vec![dict],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(const_none_def(ret));
        entry.terminator = Terminator::Return { values: vec![ret] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        let boxed_two = "9221401712017801218";
        assert!(
            ir.matches(boxed_two).count() >= 4,
            "each direct container builder must append boxed int bits; IR:\n{ir}"
        );
        assert!(
            !ir.contains("molt_list_builder_append(i64 %list, i64 2)"),
            "{ir}"
        );
        assert!(
            !ir.contains("molt_set_builder_append(i64 %set_builder, i64 2)"),
            "{ir}"
        );
        assert!(
            !ir.contains("molt_dict_builder_append(i64 %dict_builder, i64 %str_bits, i64 2)"),
            "{ir}"
        );
    }

    #[test]
    fn lower_preserved_container_builders_use_void_append_abi() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "preserved_container_builder_append_abi".into(),
            vec![],
            TirType::DynBox,
        );
        let raw = func.fresh_value();
        let key = func.fresh_value();
        let list = func.fresh_value();
        let tuple = func.fresh_value();
        let set = func.fresh_value();
        let dict = func.fresh_value();
        let ret = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, 2));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![key],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("s_value".into(), AttrValue::Str("k".into()));
                attrs
            },
            source_span: None,
        });
        for (kind, operands, result) in [
            ("list_new", vec![raw], list),
            ("tuple_new", vec![raw], tuple),
            ("set_new", vec![raw], set),
            ("dict_new", vec![key, raw], dict),
        ] {
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands,
                results: vec![result],
                attrs: {
                    let mut attrs = AttrDict::new();
                    attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
                    attrs
                },
                source_span: None,
            });
        }
        entry.ops.push(const_none_def(ret));
        entry.terminator = Terminator::Return { values: vec![ret] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend.module.verify().expect("module should verify");
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("call void @molt_list_builder_append"), "{ir}");
        assert!(ir.contains("call void @molt_dict_builder_append"), "{ir}");
        assert!(ir.contains("call void @molt_set_builder_append"), "{ir}");
    }

    #[test]
    #[should_panic(expected = "call_method_ic supports at most 4 positional args")]
    fn lower_call_method_ic_rejects_over_ic4_arity() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "call_method_ic_too_many_args".into(),
            vec![],
            TirType::DynBox,
        );
        let mut operands = Vec::new();
        for _ in 0..6 {
            let value = func.fresh_value();
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(const_none_def(value));
            operands.push(value);
        }
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("call_method_ic".into()),
                );
                attrs.insert("s_value".into(), AttrValue::Str("m".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let _ = lower_tir_to_llvm(&func, &backend);
    }

    #[test]
    fn lower_call_method_ic_preserves_central_no_willreturn_declaration() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func =
            TirFunction::new("call_method_ic_attr_reuse".into(), vec![], TirType::DynBox);
        let recv = func.fresh_value();
        let arg = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(recv));
        entry.ops.push(const_none_def(arg));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![recv, arg],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("call_method_ic".into()),
                );
                attrs.insert("s_value".into(), AttrValue::Str("m".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_call_method_ic1"), "{ir}");
        let runtime_fn = backend
            .module
            .get_function("molt_call_method_ic1")
            .expect("central method IC runtime import should exist");
        assert!(has_fn_attr(runtime_fn, "nounwind"));
        assert!(
            lacks_fn_attr(runtime_fn, "willreturn"),
            "method IC dispatch executes arbitrary user code"
        );
    }

    #[test]
    #[should_panic(expected = "call_super_method_ic supports at most 4 positional args")]
    fn lower_call_super_method_ic_rejects_over_ic4_arity() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "call_super_method_ic_too_many_args".into(),
            vec![],
            TirType::DynBox,
        );
        let mut operands = Vec::new();
        for _ in 0..7 {
            let value = func.fresh_value();
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(const_none_def(value));
            operands.push(value);
        }
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("call_super_method_ic".into()),
                );
                attrs.insert("s_value".into(), AttrValue::Str("m".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let _ = lower_tir_to_llvm(&func, &backend);
    }

    #[test]
    fn lower_class_def_boxes_raw_i64_attribute_values() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("class_def_boxed_attrs".into(), vec![], TirType::DynBox);
        let name = func.fresh_value();
        let base = func.fresh_value();
        let attr_key = func.fresh_value();
        let attr_value = func.fresh_value();
        let class_obj = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![name],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("s_value".into(), AttrValue::Str("C".into()));
                attrs
            },
            source_span: None,
        });
        entry.ops.push(const_none_def(base));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![attr_key],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("s_value".into(), AttrValue::Str("y".into()));
                attrs
            },
            source_span: None,
        });
        entry.ops.push(const_int_def(attr_value, 2));
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("class_def".into()));
        attrs.insert("s_value".into(), AttrValue::Str("1,1,0,0,0".into()));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![name, base, attr_key, attr_value],
            results: vec![class_obj],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![class_obj],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_guarded_class_def"), "{ir}");
        assert!(
            ir.contains("9221401712017801218"),
            "class_def attr values must be boxed before array storage; IR:\n{ir}"
        );
        assert!(!ir.contains("store i64 2, ptr %class_attr_ptr_1"), "{ir}");
    }

    #[test]
    fn lower_preserved_dict_update_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("dict_update_preserved".into(), vec![], TirType::DynBox);
        let dict_bits = func.fresh_value();
        let other_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(dict_bits), const_none_def(other_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![dict_bits, other_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("dict_update".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_dict_update"), "{ir}");
    }

    /// Build a single-block function whose only op is a preserved `Copy`
    /// carrying `_original_kind = kind` with `n_operands` ConstNone operands and
    /// (optionally) a result, then lower it and return the printed IR. Shared by
    /// the preserved-op passthrough-class regressions below.
    #[cfg(feature = "llvm")]
    fn lower_preserved_kind_ir(
        backend: &LlvmBackend<'_>,
        kind: &str,
        n_operands: usize,
        with_result: bool,
        s_value: Option<&str>,
    ) -> Result<String, LlvmLoweringError> {
        let mut func = TirFunction::new(format!("preserved_{kind}"), vec![], TirType::DynBox);
        let operands: Vec<ValueId> = (0..n_operands).map(|_| func.fresh_value()).collect();
        let result = with_result.then(|| func.fresh_value());
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &o in &operands {
            entry.ops.push(const_none_def(o));
        }
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
        if let Some(s) = s_value {
            attrs.insert("s_value".into(), AttrValue::Str(s.to_string()));
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results: result.into_iter().collect(),
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result.into_iter().collect(),
        };
        try_lower_tir_to_llvm(&func, backend).map(|f| f.print_to_string().to_string())
    }

    /// The preserved-op passthrough-class closure: each kind that previously
    /// fell to the `Copy` operand-0 passthrough (a silent miscompile / dropped
    /// side effect) must now lower to its dedicated runtime call. This pins the
    /// specific dedicated arms whose runtime symbol DIFFERS from `molt_<kind>`
    /// (so the generic fallback would have declined) or which are result-less.
    #[test]
    fn lower_preserved_passthrough_class_routes_to_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        // (kind, n_operands, with_result, s_value, expected runtime symbol)
        let cases: &[(&str, usize, bool, Option<&str>, &str)] = &[
            ("abs", 1, true, None, "molt_abs_builtin"),
            ("const_ellipsis", 0, true, None, "molt_ellipsis"),
            (
                "const_not_implemented",
                0,
                true,
                None,
                "molt_not_implemented",
            ),
            ("gen_throw", 2, true, None, "molt_generator_throw"),
            ("gen_close", 1, true, None, "molt_generator_close"),
            (
                "exception_set_cause",
                2,
                false,
                None,
                "molt_exception_set_cause",
            ),
            (
                "get_attr_special_obj",
                1,
                true,
                Some("__class__"),
                "molt_get_attr_special",
            ),
            ("borrow", 1, true, None, "molt_inc_ref_obj"),
            ("identity_alias", 1, true, None, "molt_inc_ref_obj"),
            ("binding_alias", 1, true, None, "molt_inc_ref_obj"),
            ("release", 1, true, None, "molt_dec_ref_obj"),
            ("guard_tag", 2, false, None, "molt_guard_type"),
            ("guard_layout", 3, true, None, "molt_guard_layout_ptr"),
            ("guard_dict_shape", 3, true, None, "molt_guard_layout_ptr"),
            ("dataclass_new", 4, true, None, "molt_dataclass_new"),
            ("json_parse", 1, true, None, "molt_json_parse_scalar_obj"),
            (
                "msgpack_parse",
                1,
                true,
                None,
                "molt_msgpack_parse_scalar_obj",
            ),
            ("cbor_parse", 1, true, None, "molt_cbor_parse_scalar_obj"),
            (
                "gen_locals_register",
                2,
                false,
                Some("gen_fn"),
                "molt_gen_locals_register",
            ),
        ];
        for &(kind, nops, with_result, s_value, sym) in cases {
            let ir = lower_preserved_kind_ir(&backend, kind, nops, with_result, s_value)
                .unwrap_or_else(|e| {
                    panic!(
                        "preserved `{kind}` must lower, got error: {:?}",
                        e.diagnostics()
                    )
                });
            assert!(
                ir.contains(sym),
                "preserved `{kind}` must lower to `{sym}` (not an operand-0 \
                 passthrough); IR:\n{ir}"
            );
        }
    }

    /// Repr-identity preserved ops (`cast`, `widen`, `copy_var`) are the
    /// explicit exception to the terminal preserved-op fail-loud rule: they
    /// carry no runtime semantics and must alias operand 0 exactly, matching
    /// native/WASM identity lowering over the NaN-boxed value format.
    #[test]
    fn lower_preserved_repr_identity_ops_pass_operand_through() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        for kind in ["cast", "widen", "copy_var"] {
            let mut func = TirFunction::new(
                format!("preserved_{kind}_identity"),
                vec![TirType::DynBox],
                TirType::DynBox,
            );
            let src = func
                .blocks
                .get(&func.entry_block)
                .and_then(|block| block.args.first())
                .map(|arg| arg.id)
                .expect("identity test function must have one entry argument");
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![src],
                results: vec![result],
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![result],
            };

            let ir = try_lower_tir_to_llvm(&func, &backend)
                .map(|f| f.print_to_string().to_string())
                .unwrap_or_else(|e| {
                    panic!(
                        "repr-identity preserved `{kind}` must lower as operand-0 \
                         passthrough, got error: {:?}",
                        e.diagnostics()
                    )
                });
            assert!(
                !ir.contains("call "),
                "repr-identity preserved `{kind}` must not lower through a runtime call:\n{ir}"
            );
            assert!(
                ir.contains("ret i64 %0"),
                "repr-identity preserved `{kind}` must return operand 0 exactly:\n{ir}"
            );
        }
    }

    /// Terminal fail-loud state: a preserved `Copy` carrying an `_original_kind`
    /// that NO arm and NO `molt_<kind>` runtime intrinsic claims must be a hard
    /// `record_fatal` lowering error — never a silent operand-0 passthrough.
    /// `__ppaudit_unmapped__` is a synthetic kind that cannot resolve to any
    /// `molt_*` symbol, so it must reach the terminal guard.
    #[test]
    fn lower_preserved_unmapped_kind_fails_loud() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let err = lower_preserved_kind_ir(&backend, "__ppaudit_unmapped__", 1, true, None)
            .expect_err(
                "an unhandled preserved op must fail the lowering, not silently \
                 pass operand 0 through",
            );
        assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
        assert_lowering_error_contains(&err, "__ppaudit_unmapped__");
    }

    /// RESULT-LESS preserved side-effect ops (`print_newline`, `set_update`,
    /// `dict_str_int_inc`, …) whose `molt_<kind>` symbol IS in the linked
    /// intrinsic surface must lower to that runtime call via the generic
    /// fallback — NOT be dropped as a `Copy` "0 results → no-op". The
    /// passthrough enumeration found these reaching the no-op branch (a missing
    /// newline / a set or dict mutation that never happened). This pins the
    /// result-less generic-fallback path; the symbols are injected because the
    /// unit-test backend has an empty intrinsic surface by default.
    #[test]
    fn lower_preserved_resultless_side_effect_routes_to_runtime() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        // (kind, n_operands, expected runtime symbol). All result-less (res=0).
        let cases: &[(&str, usize, &str)] = &[
            ("print_newline", 0, "molt_print_newline"),
            ("set_update", 2, "molt_set_update"),
            ("dict_str_int_inc", 3, "molt_dict_str_int_inc"),
            ("spawn", 1, "molt_spawn"),
        ];
        for &(_, _, sym) in cases {
            backend.runtime_intrinsic_symbols.insert(sym.to_string());
        }
        for &(kind, nops, sym) in cases {
            let ir =
                lower_preserved_kind_ir(&backend, kind, nops, false, None).unwrap_or_else(|e| {
                    panic!(
                        "result-less preserved `{kind}` must lower, got error: {:?}",
                        e.diagnostics()
                    )
                });
            assert!(
                ir.contains(sym),
                "result-less preserved `{kind}` must lower to `{sym}` (not a \
                 dropped no-op); IR:\n{ir}"
            );
            if sym == "molt_print_newline" {
                assert!(
                    ir.contains("call void @molt_print_newline()"),
                    "print_newline must use the runtime's void ABI; IR:\n{ir}"
                );
            }
        }
    }

    #[test]
    fn lower_preserved_void_runtime_result_shape_fails_loud() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_intrinsic_symbols
            .insert("molt_spawn".to_string());
        let err = lower_preserved_kind_ir(&backend, "spawn", 1, true, None)
            .expect_err("void preserved runtime ops must not bind a boxed result");
        assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
        assert_lowering_error_contains(&err, "spawn");
    }

    /// The dual safety check: a result-less preserved op whose `molt_<kind>`
    /// symbol is ABSENT from the intrinsic surface must STILL fail loud (never a
    /// silent dropped side effect). Without the symbol the generic fallback
    /// declines and the terminal guard must fire.
    #[test]
    fn lower_preserved_resultless_unmapped_fails_loud() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let err = lower_preserved_kind_ir(&backend, "__ppaudit_resultless__", 2, false, None)
            .expect_err("an unhandled result-less preserved op must fail the lowering");
        assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
        assert_lowering_error_contains(&err, "__ppaudit_resultless__");
    }

    /// A bare `Copy` (no `_original_kind` — a genuine SSA value copy such as
    /// `copy`/`load_var`/`store_var`) must STILL take the benign operand-0
    /// passthrough. The terminal fail-loud guard keys on `_original_kind`, so it
    /// must not fire here.
    #[test]
    fn lower_bare_copy_without_original_kind_passes_through() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("bare_copy".into(), vec![], TirType::DynBox);
        let src = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(src));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![src],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        // Must lower cleanly (no fatal); the result aliases the source.
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .map(|f| f.print_to_string().to_string())
            .expect("a bare Copy without _original_kind must lower as a passthrough");
        assert!(
            !ir.contains("unhandled preserved"),
            "bare Copy must not trigger the preserved-op fail-loud: {ir}"
        );
    }

    #[test]
    fn lower_preserved_len_ignores_transport_container_type() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("len_preserved".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(obj));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![obj],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
                attrs.insert("container_type".into(), AttrValue::Str("tuple".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("call i64 @molt_len("), "{ir}");
        assert!(!ir.contains("call i64 @molt_len_tuple("), "{ir}");
    }

    #[test]
    fn lower_preserved_len_uses_tir_tuple_fact() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "len_typed_tuple".into(),
            vec![TirType::Tuple(vec![TirType::DynBox, TirType::DynBox])],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![obj],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("call i64 @molt_len_tuple("), "{ir}");
    }

    #[test]
    fn lower_preserved_list_append_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("list_append_preserved".into(), vec![], TirType::DynBox);
        let list_bits = func.fresh_value();
        let item_bits = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(list_bits), const_none_def(item_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![list_bits, item_bits],
            results: vec![],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("list_append".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_list_append"), "{ir}");
    }

    #[test]
    fn lower_del_boundary_calls_dec_ref_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::DynBox);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(owned));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DelBoundary,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_dec_ref_obj"), "{ir}");
    }

    #[test]
    fn lower_preserved_list_pop_calls_runtime() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_intrinsic_symbols
            .insert("molt_list_pop".to_string());
        let ir = lower_preserved_kind_ir(&backend, "list_pop", 2, true, None)
            .expect("list_pop must lower through the boxed runtime call");
        assert!(ir.contains("molt_list_pop"), "{ir}");
    }

    #[test]
    fn lower_preserved_dataclass_new_values_calls_runtime_with_value_slice() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let ir = lower_preserved_kind_ir(&backend, "dataclass_new_values", 5, true, None)
            .expect("dataclass_new_values must lower through its value-slice runtime call");
        assert!(ir.contains("molt_dataclass_new_from_values"), "{ir}");
        assert!(ir.contains("alloca i64, i64 2"), "{ir}");
    }

    #[test]
    fn lower_preserved_tuple_from_list_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func =
            TirFunction::new("tuple_from_list_preserved".into(), vec![], TirType::DynBox);
        let list_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(list_bits));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![list_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("tuple_from_list".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_tuple_from_list"), "{ir}");
    }

    #[test]
    fn lower_preserved_set_add_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("set_add_preserved".into(), vec![], TirType::DynBox);
        let set_bits = func.fresh_value();
        let item_bits = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(set_bits), const_none_def(item_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![set_bits, item_bits],
            results: vec![],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("set_add".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_set_add"), "{ir}");
    }

    #[test]
    fn lower_preserved_list_extend_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("list_extend_preserved".into(), vec![], TirType::DynBox);
        let list_bits = func.fresh_value();
        let other_bits = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(list_bits), const_none_def(other_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![list_bits, other_bits],
            results: vec![],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("list_extend".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_list_extend"), "{ir}");
    }

    #[test]
    fn lower_preserved_aiter_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("aiter_preserved".into(), vec![], TirType::DynBox);
        let obj_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(obj_bits));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![obj_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("aiter".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_aiter"), "{ir}");
    }

    #[test]
    fn lower_preserved_gen_send_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("gen_send_preserved".into(), vec![], TirType::DynBox);
        let gen_bits = func.fresh_value();
        let send_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(gen_bits), const_none_def(send_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![gen_bits, send_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("gen_send".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_generator_send"), "{ir}");
    }

    #[test]
    fn lower_preserved_context_exit_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("context_exit_preserved".into(), vec![], TirType::DynBox);
        let ctx_bits = func.fresh_value();
        let exc_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(ctx_bits), const_none_def(exc_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![ctx_bits, exc_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("context_exit".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_context_exit"), "{ir}");
    }

    #[test]
    fn lower_preserved_super_new_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("super_new_preserved".into(), vec![], TirType::DynBox);
        let type_bits = func.fresh_value();
        let obj_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(type_bits), const_none_def(obj_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![type_bits, obj_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("super_new".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_super_new"), "{ir}");
    }

    #[test]
    fn lower_dynamic_get_attr_name_uses_operand_name() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "dynamic_get_attr_name".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::LoadAttr,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("get_attr_name".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_get_attr_name"), "{ir}");
        assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
    }

    #[test]
    fn lower_dynamic_set_attr_name_uses_operand_name() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "dynamic_set_attr_name".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StoreAttr,
            operands: vec![ValueId(0), ValueId(1), ValueId(2)],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("set_attr_name".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_set_attr_name"), "{ir}");
        assert!(ir.contains("i64 %0, i64 %1, i64 %2"), "{ir}");
    }

    #[test]
    fn lower_dynamic_del_attr_name_uses_operand_name() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "dynamic_del_attr_name".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DelAttr,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("del_attr_name".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_del_attr_name"), "{ir}");
        assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
    }

    #[test]
    fn lower_preserved_has_attr_name_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("has_attr_name_preserved".into(), vec![], TirType::DynBox);
        let obj_bits = func.fresh_value();
        let name_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(obj_bits), const_none_def(name_bits)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![obj_bits, name_bits],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("has_attr_name".into()),
                );
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_has_attr_name"), "{ir}");
    }

    #[test]
    fn lower_call_method_uses_call_bind_ic_abi() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("call_method_abi".into(), vec![], TirType::DynBox);
        let callable = func.fresh_value();
        let arg0 = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(callable), const_none_def(arg0)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallMethod,
            operands: vec![callable, arg0],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_call_bind_ic"), "{ir}");
        assert!(!ir.contains("molt_call_method"), "{ir}");
    }

    #[test]
    fn lower_call_bind_preserves_callargs_builder_abi() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("call_bind_abi".into(), vec![], TirType::DynBox);
        let callable = func.fresh_value();
        let builder = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(callable), const_none_def(builder)]);
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![callable, builder],
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
                attrs
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains("molt_call_bind_ic"), "{ir}");
    }

    #[test]
    fn lower_i64_comparison() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);

        // Build: fn lt(a: i64, b: i64) -> bool { return a < b }
        let mut func = TirFunction::new(
            "cmp_lt".into(),
            vec![TirType::I64, TirType::I64],
            TirType::Bool,
        );
        let v_result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Lt,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v_result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![v_result],
        };

        // The raw `icmp slt` path needs both operands proven exact-i64 carriers;
        // an unproven `i64` parameter carries boxed (`DynBox`) and dispatches the
        // comparison through the runtime. Prove the two parameters here.
        let mut facts = crate::representation_plan::LlvmReprFacts::default();
        for v in [ValueId(0), ValueId(1)] {
            facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
        }
        backend.function_repr_facts.insert(func.name.clone(), facts);

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("icmp slt"),
            "expected signed less-than comparison in IR: {}",
            ir
        );
    }

    #[test]
    fn lower_box_i64() {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);

        // Build: fn box_it(x: i64) -> DynBox { return box(x) }
        let mut func = TirFunction::new("box_i64".into(), vec![TirType::I64], TirType::DynBox);
        let v_boxed = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BoxVal,
            operands: vec![ValueId(0)],
            results: vec![v_boxed],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![v_boxed],
        };

        // `box(x)` emits the NaN-boxing arithmetic only when `x` is a RAW i64.
        // An unproven `i64` parameter carries already-boxed (`DynBox`), for which
        // `box` is a no-op; prove the parameter raw so the box path is exercised.
        let mut facts = crate::representation_plan::LlvmReprFacts::default();
        facts
            .repr_by_value
            .insert(ValueId(0), crate::Repr::RawI64Safe);
        backend.function_repr_facts.insert(func.name.clone(), facts);

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        // Should contain the NaN-boxing OR operations
        assert!(
            ir.contains("or i64"),
            "expected NaN-boxing OR in IR: {}",
            ir
        );
        assert!(
            ir.contains("and i64"),
            "expected NaN-boxing AND mask in IR: {}",
            ir
        );
    }

    #[test]
    fn masked_shift_loop_phi_promoted_to_raw_i64_lane() {
        // #43 end-to-end (the perf payoff the value-range phi narrowing exists
        // for): a `DynBox`-declared loop-header phi that the representation plan
        // proves `RawI64Safe` must be carried as a raw `I64` so the in-loop
        // `<<`/`&` emit raw machine `shl`/`and` instead of the boxed
        // `molt_lshift`/`molt_bit_and` runtime. `type_refine` leaves the masked
        // accumulator `DynBox` (its inline-window fit is a value-range-only fact),
        // so without `effective_block_arg_type`'s DynBox->I64 promotion the phi
        // carries boxed and every iteration round-trips through the runtime — the
        // exact regression this guards.
        //
        // Shape:  s_phi: DynBox = phi[ 1 (preheader), band (back-edge) ]
        //         shl  = s_phi << 1
        //         band = shl & MASK            (MASK = 2**32 - 1)
        //         -> header(band)
        // with the plan proving s_phi / shl / band all RawI64Safe.
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);

        let mut func = TirFunction::new("masked_shift".into(), vec![], TirType::None);
        let s_start = func.fresh_value(); // ConstInt 1
        let mask_c = func.fresh_value(); // ConstInt (2**32 - 1)
        let one_c = func.fresh_value(); // ConstInt 1 (shift count)
        let s_phi = func.fresh_value(); // header phi (DynBox-declared)
        let shl = func.fresh_value(); // s_phi << 1
        let band = func.fresh_value(); // shl & MASK

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();

        let mk_int = |result: ValueId, v: i64| TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(v));
                m
            },
            source_span: None,
        };
        let mk_bin = |opcode: OpCode, a: ValueId, b: ValueId, r: ValueId| TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![a, b],
            results: vec![r],
            attrs: AttrDict::new(),
            source_span: None,
        };
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                mk_int(s_start, 1),
                mk_int(mask_c, (1i64 << 32) - 1),
                mk_int(one_c, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![s_start],
            };
        }
        // The phi is DECLARED DynBox (as type_refine leaves the masked accumulator).
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: s_phi,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: body,
                    args: vec![],
                },
            },
        );
        func.loop_roles
            .insert(header, crate::tir::blocks::LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    mk_bin(OpCode::Shl, s_phi, one_c, shl),
                    mk_bin(OpCode::BitAnd, shl, mask_c, band),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![band],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles
            .insert(exit, crate::tir::blocks::LoopRole::LoopEnd);

        // The plan proves the masked accumulator chain RawI64Safe (what the
        // value-range phi narrowing yields end to end). The ConstInts are I64 by
        // their own lowering; the proof here is for the phi + the two op results.
        let mut facts = crate::representation_plan::LlvmReprFacts::default();
        for v in [s_phi, shl, band] {
            facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
        }
        backend.function_repr_facts.insert(func.name.clone(), facts);

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        assert!(
            ir.contains("shl i64"),
            "masked accumulator shift must lower to a RAW machine `shl i64`, not \
             the boxed runtime. IR:\n{ir}"
        );
        assert!(
            !ir.contains("@molt_lshift"),
            "a RawI64Safe-proven masked shift must NOT call the boxed `molt_lshift`. \
             IR:\n{ir}"
        );
        assert!(
            !ir.contains("@molt_bit_and"),
            "a RawI64Safe-proven masked `& MASK` must NOT call the boxed \
             `molt_bit_and`. IR:\n{ir}"
        );
        // The header phi must be a raw `i64` phi (promoted from its DynBox
        // declaration) so the back-edge carries the raw masked value.
        assert!(
            ir.contains("phi i64"),
            "the RawI64Safe masked accumulator phi must be a raw `i64` phi. IR:\n{ir}"
        );
    }

    // ── RPO algorithm tests ──
    //
    // The RPO algorithm is exercised end-to-end by the integration tests in
    // `runtime/molt-backend/tests/llvm_rpo.rs`, which call into
    // [`super::compute_function_rpo`] directly with synthetic CFGs covering
    // diamonds, loops, switches, deep chains, self-loops, and unreachable
    // blocks. Those tests live in a separate test binary and so are not
    // blocked by drift in the wider lib test suite.

    /// Helper: build a function with `num_blocks` empty blocks (terminators
    /// initialized to `Unreachable`; tests overwrite them).
    fn make_func_with_blocks(name: &str, num_blocks: u32) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
        for _ in 1..num_blocks {
            let bid = func.fresh_block();
            func.blocks.insert(
                bid,
                TirBlock {
                    id: bid,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Unreachable,
                },
            );
        }
        func
    }

    fn set_term(func: &mut TirFunction, b: BlockId, term: Terminator) {
        func.blocks.get_mut(&b).unwrap().terminator = term;
    }

    fn position_of(rpo: &[BlockId], b: BlockId) -> usize {
        rpo.iter()
            .position(|x| *x == b)
            .unwrap_or_else(|| panic!("BlockId {:?} not present in RPO {:?}", b, rpo))
    }

    #[test]
    fn rpo_diamond_cfg_orders_entry_first_then_arms_then_merge() {
        // CFG:
        //   entry -> A, B   (cond branch)
        //   A     -> merge
        //   B     -> merge
        //   merge -> return
        //
        // Valid RPOs: [entry, A, B, merge] OR [entry, B, A, merge].
        let mut func = make_func_with_blocks("diamond", 4);
        let entry = func.entry_block; // BlockId(0)
        let a = BlockId(1);
        let b = BlockId(2);
        let merge = BlockId(3);

        // We allocate ValueId(0) as the conditional value. We never actually
        // evaluate it — RPO walks terminators, not ops.
        let cond = func.fresh_value();
        set_term(
            &mut func,
            entry,
            Terminator::CondBranch {
                cond,
                then_block: a,
                then_args: vec![],
                else_block: b,
                else_args: vec![],
            },
        );
        set_term(
            &mut func,
            a,
            Terminator::Branch {
                target: merge,
                args: vec![],
            },
        );
        set_term(
            &mut func,
            b,
            Terminator::Branch {
                target: merge,
                args: vec![],
            },
        );
        set_term(&mut func, merge, Terminator::Return { values: vec![] });

        let rpo = compute_function_rpo(&func);

        assert_eq!(
            rpo.len(),
            4,
            "all four blocks must appear in RPO: {:?}",
            rpo
        );
        assert_eq!(rpo[0], entry, "entry must be first: {:?}", rpo);
        assert_eq!(rpo[3], merge, "merge must be last: {:?}", rpo);

        let pos_entry = position_of(&rpo, entry);
        let pos_a = position_of(&rpo, a);
        let pos_b = position_of(&rpo, b);
        let pos_merge = position_of(&rpo, merge);

        assert!(pos_entry < pos_a, "entry must precede A: {:?}", rpo);
        assert!(pos_entry < pos_b, "entry must precede B: {:?}", rpo);
        assert!(pos_a < pos_merge, "A must precede merge: {:?}", rpo);
        assert!(pos_b < pos_merge, "B must precede merge: {:?}", rpo);

        // The two valid orderings are exactly these two.
        let valid_a_first = rpo == vec![entry, a, b, merge];
        let valid_b_first = rpo == vec![entry, b, a, merge];
        assert!(
            valid_a_first || valid_b_first,
            "RPO must be one of the two valid diamond orderings, got {:?}",
            rpo
        );
    }

    #[test]
    fn rpo_simple_loop_orders_entry_before_header_before_body() {
        // CFG:
        //   entry  -> header
        //   header -> body, exit  (cond branch)
        //   body   -> header      (back-edge — does NOT change RPO order)
        //   exit   -> return
        //
        // Required: entry < header < body in RPO. The back-edge body->header
        // is the only edge that runs "backwards" in the resulting layout.
        let mut func = make_func_with_blocks("loop", 4);
        let entry = func.entry_block; // BlockId(0)
        let header = BlockId(1);
        let body = BlockId(2);
        let exit = BlockId(3);

        let cond = func.fresh_value();
        set_term(
            &mut func,
            entry,
            Terminator::Branch {
                target: header,
                args: vec![],
            },
        );
        set_term(
            &mut func,
            header,
            Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        );
        set_term(
            &mut func,
            body,
            Terminator::Branch {
                target: header,
                args: vec![],
            },
        );
        set_term(&mut func, exit, Terminator::Return { values: vec![] });

        let rpo = compute_function_rpo(&func);

        assert_eq!(
            rpo.len(),
            4,
            "all four blocks must appear in RPO: {:?}",
            rpo
        );

        let pos_entry = position_of(&rpo, entry);
        let pos_header = position_of(&rpo, header);
        let pos_body = position_of(&rpo, body);
        let pos_exit = position_of(&rpo, exit);

        assert_eq!(pos_entry, 0, "entry must be first: {:?}", rpo);
        assert!(
            pos_entry < pos_header,
            "entry must precede header: {:?}",
            rpo
        );
        assert!(
            pos_header < pos_body,
            "header must precede body (back-edge does not flip order): {:?}",
            rpo
        );
        assert!(
            pos_header < pos_exit,
            "header must precede exit (then is forward edge): {:?}",
            rpo
        );
    }

    #[test]
    fn rpo_unreachable_blocks_are_excluded() {
        // CFG:
        //   entry -> exit (return)
        //   dead  -> return  (no predecessor — unreachable)
        let mut func = make_func_with_blocks("dead_block", 3);
        let entry = func.entry_block;
        let exit = BlockId(1);
        let dead = BlockId(2);

        set_term(
            &mut func,
            entry,
            Terminator::Branch {
                target: exit,
                args: vec![],
            },
        );
        set_term(&mut func, exit, Terminator::Return { values: vec![] });
        set_term(&mut func, dead, Terminator::Return { values: vec![] });

        let rpo = compute_function_rpo(&func);

        assert_eq!(rpo, vec![entry, exit]);
        assert!(
            !rpo.contains(&dead),
            "unreachable block must be excluded from RPO: {:?}",
            rpo
        );
    }

    #[test]
    fn rpo_switch_terminator_visits_all_cases_and_default() {
        // CFG:
        //   entry -> switch on v: case 0 -> A, case 1 -> B, default -> C
        //   A, B, C -> merge -> return
        let mut func = make_func_with_blocks("switch_cfg", 5);
        let entry = func.entry_block;
        let a = BlockId(1);
        let b = BlockId(2);
        let c = BlockId(3);
        let merge = BlockId(4);

        let v = func.fresh_value();
        set_term(
            &mut func,
            entry,
            Terminator::Switch {
                value: v,
                cases: vec![(0, a, vec![]), (1, b, vec![])],
                default: c,
                default_args: vec![],
            },
        );
        for case_block in [a, b, c] {
            set_term(
                &mut func,
                case_block,
                Terminator::Branch {
                    target: merge,
                    args: vec![],
                },
            );
        }
        set_term(&mut func, merge, Terminator::Return { values: vec![] });

        let rpo = compute_function_rpo(&func);

        assert_eq!(rpo.len(), 5, "all five blocks must appear: {:?}", rpo);
        assert_eq!(rpo[0], entry);
        assert_eq!(rpo[4], merge);
        for case_block in [a, b, c] {
            let p = position_of(&rpo, case_block);
            assert!(p > 0, "case block must follow entry");
            assert!(p < 4, "case block must precede merge");
        }
    }

    #[test]
    fn rpo_deeply_chained_cfg_does_not_overflow_stack() {
        // Build a chain of 5,000 blocks: entry -> b1 -> b2 -> ... -> b4999 -> return.
        // The original recursive implementation overflowed at this depth on
        // default thread stack sizes; the iterative version handles it
        // without issue.
        const N: u32 = 5_000;
        let mut func = make_func_with_blocks("deep_chain", N);
        for i in 0..N - 1 {
            set_term(
                &mut func,
                BlockId(i),
                Terminator::Branch {
                    target: BlockId(i + 1),
                    args: vec![],
                },
            );
        }
        set_term(
            &mut func,
            BlockId(N - 1),
            Terminator::Return { values: vec![] },
        );

        let rpo = compute_function_rpo(&func);

        assert_eq!(rpo.len(), N as usize);
        for (i, bid) in rpo.iter().enumerate() {
            assert_eq!(
                *bid,
                BlockId(i as u32),
                "deep chain RPO must be entry, b1, b2, ... in order"
            );
        }
    }

    #[test]
    fn rpo_terminator_successor_helper_preserves_order() {
        // The order in which `append_terminator_successors` records successors
        // is part of the algorithm's contract: it determines tie-breaking
        // when multiple valid RPOs exist. Pin it explicitly.
        let mut buf = Vec::new();

        buf.clear();
        append_terminator_successors(
            &Terminator::Branch {
                target: BlockId(7),
                args: vec![],
            },
            &mut buf,
        );
        assert_eq!(buf, vec![BlockId(7)]);

        buf.clear();
        append_terminator_successors(
            &Terminator::CondBranch {
                cond: ValueId(0),
                then_block: BlockId(11),
                then_args: vec![],
                else_block: BlockId(13),
                else_args: vec![],
            },
            &mut buf,
        );
        assert_eq!(
            buf,
            vec![BlockId(11), BlockId(13)],
            "then must precede else"
        );

        buf.clear();
        append_terminator_successors(
            &Terminator::Switch {
                value: ValueId(0),
                cases: vec![(0, BlockId(20), vec![]), (1, BlockId(21), vec![])],
                default: BlockId(22),
                default_args: vec![],
            },
            &mut buf,
        );
        assert_eq!(
            buf,
            vec![BlockId(20), BlockId(21), BlockId(22)],
            "switch cases in declaration order, then default"
        );

        buf.clear();
        append_terminator_successors(&Terminator::Return { values: vec![] }, &mut buf);
        assert!(buf.is_empty(), "Return has no successors");

        buf.clear();
        append_terminator_successors(&Terminator::Unreachable, &mut buf);
        assert!(buf.is_empty(), "Unreachable has no successors");
    }
}
