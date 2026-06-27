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
use molt_codegen_abi as nanbox;

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
mod call_lowering;
#[cfg(feature = "llvm")]
mod constant_ops;
#[cfg(feature = "llvm")]
mod container_ops;
#[cfg(feature = "llvm")]
mod numeric_ops;
#[cfg(feature = "llvm")]
mod object_ops;
#[cfg(feature = "llvm")]
mod op_dispatch;
#[cfg(feature = "llvm")]
mod preserved_ops;
#[cfg(feature = "llvm")]
mod runtime_helpers;
#[cfg(feature = "llvm")]
mod state_machine_ops;
#[cfg(feature = "llvm")]
mod value_materialization;

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

    let bias = i64_ty.const_int(nanbox::INLINE_INT_BIAS as u64, false);
    let biased = builder.build_int_add(raw, bias, "int_inline_bias").unwrap();
    let limit = i64_ty.const_int(nanbox::INLINE_INT_LIMIT as u64, false);
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
                    i64_ty.const_int(nanbox::CANONICAL_NAN_BITS, false),
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
}

// ?? Tests ??

#[cfg(all(test, feature = "llvm"))]
mod tests;
