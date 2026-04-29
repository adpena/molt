//! Core TIR -> LLVM IR lowering.
//!
//! This module converts a `TirFunction` into an LLVM `FunctionValue` using
//! type-specialized emission: when operand types are statically known (e.g.
//! I64+I64), we emit native LLVM instructions; when types are dynamic
//! (DynBox), we emit calls to the Molt runtime.

#[cfg(feature = "llvm")]
use std::collections::HashMap;

#[cfg(feature = "llvm")]
use inkwell::basic_block::BasicBlock;
#[cfg(feature = "llvm")]
use inkwell::types::BasicType;
#[cfg(feature = "llvm")]
use inkwell::values::{BasicValueEnum, FunctionValue, PhiValue};

#[cfg(feature = "llvm")]
use crate::llvm_backend::LlvmBackend;
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
use crate::tir::ops::{AttrValue, OpCode, TirOp};
#[cfg(feature = "llvm")]
use crate::tir::types::TirType;
#[cfg(feature = "llvm")]
use crate::tir::values::ValueId;

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

#[cfg(feature = "llvm")]
fn materialize_dynbox_bits_with_builder<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    context: &'ctx inkwell::context::Context,
    operand: BasicValueEnum<'ctx>,
    operand_ty: &TirType,
) -> inkwell::values::IntValue<'ctx> {
    let i64_ty = context.i64_type();
    match operand_ty {
        TirType::I64 => {
            let raw = ensure_i64_with_builder(builder, context, operand);
            let masked = builder
                .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "mask")
                .unwrap();
            builder
                .build_or(
                    masked,
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_INT, false),
                    "box_i64",
                )
                .unwrap()
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
        | TirType::Set(_)
        | TirType::Tuple(_)
        | TirType::Ptr(_)
        | TirType::Func(_)
        | TirType::Box(_)
        | TirType::Union(_)
        | TirType::Never => ensure_i64_with_builder(builder, context, operand),
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
    /// Maps TIR BlockId -> the LLVM block where the explicit terminator was emitted.
    /// This differs from `block_map` when mid-block control flow splits (for example
    /// `check_exception` creating a synthetic fallthrough block).
    exit_block_map: HashMap<BlockId, BasicBlock<'ctx>>,
    /// Maps TIR ValueId -> lowered LLVM value.
    values: HashMap<ValueId, BasicValueEnum<'ctx>>,
    /// Maps TIR ValueId -> its TirType (for type-specialized dispatch).
    value_types: HashMap<ValueId, TirType>,
    /// Phi nodes that need incoming values wired up after all blocks are emitted.
    /// (target_block, arg_index, phi_node)
    pending_phis: Vec<(BlockId, usize, PhiValue<'ctx>)>,
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
    /// Mid-block branches created by CheckException.  Each entry records the
    /// LLVM basic block that the branch originates from and the TIR BlockId
    /// it targets.  These are NOT visible in TIR terminators, so
    /// `finalize_phis` must account for them separately.
    mid_block_branches: Vec<(BasicBlock<'ctx>, BlockId)>,
    /// Synthetic or explicit resume blocks keyed by generator/coroutine state id.
    state_resume_blocks: HashMap<i64, BasicBlock<'ctx>>,
    /// All LLVM basic blocks created during lowering (including synthetic ones),
    /// used for the final unterminated-block sweep.
    all_llvm_blocks: Vec<BasicBlock<'ctx>>,
    /// Maps each LLVM basic block to its set of LLVM predecessor blocks.
    /// Built during lowering as branches are emitted, used by
    /// `patch_incomplete_phis` to add undef entries for missing predecessors.
    llvm_pred_map: HashMap<BasicBlock<'ctx>, Vec<BasicBlock<'ctx>>>,
    /// Structured exception-region stack baselines for preserved TryStart/TryEnd.
    /// Stored in entry-block allocas so later TryEnd sites do not violate LLVM
    /// dominance when the region spans multiple blocks.
    try_stack_baselines: Vec<inkwell::values::PointerValue<'ctx>>,
    /// Deterministic per-function call-site numbering for IC lanes.
    call_site_counter: usize,
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
    lower_tir_to_llvm_with_pgo(func, backend, None)
}

/// Like [`lower_tir_to_llvm`] but accepts optional PGO branch weights.
#[cfg(feature = "llvm")]
pub fn lower_tir_to_llvm_with_pgo<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
    pgo_branch_weights: Option<Vec<u64>>,
) -> FunctionValue<'ctx> {
    // 1. Build or reuse the LLVM function signature.
    let llvm_fn = declare_tir_function(func, backend);
    let mut lowering = FunctionLowering {
        backend,
        func,
        llvm_fn,
        entry_trampoline_bb: None,
        block_map: HashMap::new(),
        exit_block_map: HashMap::new(),
        values: HashMap::new(),
        value_types: HashMap::new(),
        pending_phis: Vec::new(),
        pgo_branch_weights,
        pgo_weight_index: 0,
        const_str_counter: 0,
        synthetic_block_counter: 0,
        mid_block_branches: Vec::new(),
        state_resume_blocks: HashMap::new(),
        all_llvm_blocks: Vec::new(),
        llvm_pred_map: HashMap::new(),
        try_stack_baselines: Vec::new(),
        call_site_counter: 0,
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
            Terminator::Switch { cases, default, .. } => {
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

    llvm_fn
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
        // Verify it's just a declaration (no basic blocks yet).
        if existing.count_basic_blocks() == 0 {
            existing
        } else {
            // Already defined — create with unique name (shouldn't happen).
            backend.module.add_function(&func.name, fn_ty, None)
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
        Terminator::Switch { cases, default, .. } => {
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
/// Public so integration tests (under `runtime/molt-backend/tests/`) can
/// exercise it without going through an inkwell context.
#[cfg(feature = "llvm")]
pub fn compute_function_rpo(func: &TirFunction) -> Vec<BlockId> {
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

                // Push successors in reverse so the *first* successor is
                // popped (and thus visited) first. This makes the recursion
                // order match the natural left-to-right successor order
                // recorded by `append_terminator_successors`.
                succ_buf.clear();
                append_terminator_successors(&block.terminator, &mut succ_buf);
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
                let value =
                    self.llvm_fn.get_nth_param(i as u32).unwrap_or_else(|| {
                        match lower_type(self.backend.context, &arg.ty) {
                            inkwell::types::BasicTypeEnum::IntType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::FloatType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::PointerType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::ArrayType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::StructType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::VectorType(ty) => ty.get_undef().into(),
                            inkwell::types::BasicTypeEnum::ScalableVectorType(ty) => {
                                ty.get_undef().into()
                            }
                        }
                    });
                self.values.insert(arg.id, value);
                self.value_types.insert(arg.id, arg.ty.clone());
            }
        } else {
            // Non-entry blocks: create phi nodes for block arguments.
            for (i, arg) in block.args.iter().enumerate() {
                let llvm_ty = lower_type(self.backend.context, &arg.ty);
                let phi = self
                    .backend
                    .builder
                    .build_phi(llvm_ty, &format!("phi_{}", arg.id.0))
                    .unwrap();
                self.values.insert(arg.id, phi.as_basic_value());
                self.value_types.insert(arg.id, arg.ty.clone());
                self.pending_phis.push((block_id, i, phi));
            }
        }

        // Lower each operation.
        for op in &block.ops {
            self.lower_op(op);
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
            self.lower_terminator(&block.terminator);
        }
        let exit_bb = self.backend.builder.get_insert_block().unwrap_or(bb);
        self.exit_block_map.insert(block_id, exit_bb);
    }

    fn lower_op(&mut self, op: &crate::tir::ops::TirOp) {
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
                let val = if op.opcode == OpCode::And {
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
                self.values.insert(result_id, val);
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
                let inc_fn = self
                    .backend
                    .module
                    .get_function("molt_inc_ref_obj")
                    .unwrap();
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
            OpCode::DecRef => {
                let val = self.resolve(op.operands[0]);
                let dec_fn = self
                    .backend
                    .module
                    .get_function("molt_dec_ref_obj")
                    .unwrap();
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
                    let ptr_ty = self.backend.context.ptr_type(
                        inkwell::AddressSpace::default(),
                    );
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
                    let inc_fn = self
                        .backend
                        .module
                        .get_function("molt_inc_ref_obj")
                        .unwrap();
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
                    let inc_fn = self.ensure_runtime_i64_fn("molt_inc_ref_obj", 1);
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
                if matches!(original_kind, Some("store_init"))
                    && op.operands.len() >= 2
                {
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
                    let ptr_ty = self.backend.context.ptr_type(
                        inkwell::AddressSpace::default(),
                    );
                    // Check if val is a heap pointer: (val & TAG_MASK) == TAG_PTR
                    let tag_mask = i64_ty.const_int(
                        nanbox::QNAN | 0x0007_0000_0000_0000,
                        false,
                    );
                    let tag_bits = self
                        .backend
                        .builder
                        .build_and(val_bits, tag_mask, "init_tag")
                        .unwrap();
                    let ptr_tag = i64_ty.const_int(
                        nanbox::QNAN | 0x0004_0000_0000_0000,
                        false,
                    );
                    let is_ptr = self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::EQ,
                            tag_bits,
                            ptr_tag,
                            "is_ptr",
                        )
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
                if matches!(original_kind, Some("store"))
                    && op.operands.len() >= 2
                {
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
                    Some("guarded_field_set") | Some("guarded_field_set_init")
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
                    let rt_name = if matches!(original_kind, Some("guarded_field_set_init")) {
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
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
                                    let v = if matches!(original_kind, Some("call_guarded")) {
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
                                        self.coerce_to_tir_type(
                                            v,
                                            &source_tir_ty,
                                            &target_tir_ty,
                                            current_bb,
                                        )
                                    } else {
                                        v
                                    };
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
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
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

            // ── SSA Copy ──
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::Copy => {
                if let Some(kind) = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
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
                    let arg = self.resolve(arg_id);
                    let arg_i64 = self.ensure_i64(arg);
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
                    let print_fn =
                        if let Some(f) = self.backend.module.get_function("molt_print_obj") {
                            f
                        } else {
                            let void_ty = self.backend.context.void_type();
                            let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
                            let f = self.backend.module.add_function(
                                "molt_print_obj",
                                fn_ty,
                                Some(inkwell::module::Linkage::External),
                            );
                            let nounwind_kind =
                                inkwell::attributes::Attribute::get_named_enum_kind_id("nounwind");
                            f.add_attribute(
                                AttributeLoc::Function,
                                self.backend.context.create_enum_attribute(nounwind_kind, 0),
                            );
                            f
                        };
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
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
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
                    let k = self.resolve(op.operands[i]);
                    let v = self.resolve(op.operands[i + 1]);
                    let k_i64 = self.ensure_i64(k);
                    let v_i64 = self.ensure_i64(v);
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
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
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
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
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
                let task_ptr = self
                    .backend
                    .builder
                    .build_int_to_ptr(self.ensure_i64(task_bits), ptr_ty, "task_obj_ptr")
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
                    let inc_fn = self.ensure_runtime_i64_fn("molt_inc_ref_obj", 1);
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
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let get_state_fn = self.ensure_runtime_i64_fn("molt_obj_get_state", 1);
                let state_val = self
                    .backend
                    .builder
                    .build_call(get_state_fn, &[self_bits.into()], "state_switch_state")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let fallback_bb = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("state_switch_cont{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(fallback_bb);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_switch must be inside a block");
                self.record_llvm_edge(current_bb, fallback_bb);
                let resume_cases: Vec<_> = self
                    .state_resume_blocks
                    .iter()
                    .map(|(&state_id, &target_bb)| (state_id, target_bb))
                    .collect();
                let mut switch_cases: Vec<_> = Vec::with_capacity(resume_cases.len());
                for (state_id, target_bb) in resume_cases {
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((i64_ty.const_int(state_id as u64, state_id < 0), target_bb));
                }
                switch_cases.sort_by_key(|(state, _)| state.get_zero_extended_constant());
                self.backend
                    .builder
                    .build_switch(state_val, fallback_bb, &switch_cases)
                    .unwrap();
                self.backend.builder.position_at_end(fallback_bb);
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
                let set_state_fn = self.ensure_runtime_i64_fn("molt_obj_set_state", 2);
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
                let inc_fn = self.ensure_runtime_i64_fn("molt_inc_ref_obj", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(inc_fn, &[pair_bits.into()], "state_yield_inc_ref")
                    .unwrap();
                self.backend.builder.build_return(Some(&pair_bits)).unwrap();
                let resume_bb = *self
                    .state_resume_blocks
                    .get(&next_state_id)
                    .unwrap_or_else(|| panic!("missing resume block for state {}", next_state_id));
                self.backend.builder.position_at_end(resume_bb);
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
                let set_state_fn = self.ensure_runtime_i64_fn("molt_obj_set_state", 2);
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
                let set_state_fn = self.ensure_runtime_i64_fn("molt_obj_set_state", 2);
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
                let set_state_fn = self.ensure_runtime_i64_fn("molt_obj_set_state", 2);
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
                    eprintln!(
                        "LLVM lowering warning: unknown check_exception target {} in {}",
                        target_label, self.func.name
                    );
                    return;
                };
                let Some(&target_bb) = self.block_map.get(&target_block_id) else {
                    eprintln!(
                        "LLVM lowering warning: check_exception target block {:?} not in block_map in {}",
                        target_block_id, self.func.name
                    );
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
                // Record the current LLVM block as a mid-block branch source
                // to the handler target. This is needed so finalize_phis can
                // wire up phi incoming values for the handler block.
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.mid_block_branches
                    .push((branch_from_bb, target_block_id));
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
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }

    // ── Type-specialized binary arithmetic ──

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
        // (e.g. loop_narrow for bounded loop counters), we emit nsw-flagged
        // integer instructions.  This enables LLVM's:
        //  - Strength reduction (e.g. `i * 4` → `i << 2` with guaranteed no wrap)
        //  - SCEV (Scalar Evolution) for loop trip count analysis
        //  - Loop vectorization with known induction variable ranges
        let nsw = has_attr(op, "no_signed_wrap");

        let (val, out_ty) = match (&lhs_ty, &rhs_ty, name) {
            // I64 + I64 -> I64 (direct machine instruction).
            // When `nsw` is set, use build_int_nsw_add to tell LLVM the
            // result is guaranteed not to overflow as a signed i64.
            (TirType::I64, TirType::I64, "add") => {
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
            (TirType::I64, TirType::I64, "sub") => {
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
            (TirType::I64, TirType::I64, "mul") => {
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
            (TirType::I64, TirType::I64, "div") => {
                // Python `/` on ints always returns float (7 / 2 == 3.5)
                let f64_ty = self.backend.context.f64_type();
                let lhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(lhs.into_int_value(), f64_ty, "div_lhs_f")
                    .unwrap();
                let rhs_f = self
                    .backend
                    .builder
                    .build_signed_int_to_float(rhs.into_int_value(), f64_ty, "div_rhs_f")
                    .unwrap();
                let v = self
                    .backend
                    .builder
                    .build_float_div(lhs_f, rhs_f, "div_f")
                    .unwrap();
                (v.into(), TirType::F64)
            }
            (TirType::I64, TirType::I64, "floordiv") => {
                // Python `//`: rounds toward negative infinity (not toward zero like C sdiv).
                // Emit: q = sdiv(lhs, rhs); r = srem(lhs, rhs);
                //       if (r != 0 && (lhs ^ rhs) < 0) q -= 1
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let i64_ty = self.backend.context.i64_type();
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
                let zero = i64_ty.const_zero();
                let one = i64_ty.const_int(1, false);
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
                let q_m1_basic: inkwell::values::BasicValueEnum = q_minus_1.into();
                let q_basic: inkwell::values::BasicValueEnum = q.into();
                let adj = self
                    .backend
                    .builder
                    .build_select(needs_adjust, q_m1_basic, q_basic, "floordiv")
                    .unwrap();
                (adj, TirType::I64)
            }
            (TirType::I64, TirType::I64, "mod") => {
                // Python `%`: result has sign of the divisor (not dividend like C srem).
                // Emit: r = srem(lhs, rhs);
                //       if (r != 0 && (r ^ rhs) < 0) r += rhs
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let i64_ty = self.backend.context.i64_type();
                let zero = i64_ty.const_zero();
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
                let r_adj_basic: inkwell::values::BasicValueEnum = r_plus_rhs.into();
                let r_basic: inkwell::values::BasicValueEnum = r.into();
                let result = self
                    .backend
                    .builder
                    .build_select(needs_adjust, r_adj_basic, r_basic, "pymod")
                    .unwrap();
                (result, TirType::I64)
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

            // Everything else: call runtime (DynBox dispatch)
            _ => {
                let rt_name = match name {
                    "add" => "molt_add",
                    "sub" => "molt_sub",
                    "mul" => "molt_mul",
                    "div" => "molt_div",
                    "floordiv" => "molt_floordiv",
                    "mod" => "molt_mod",
                    "pow" => "molt_pow",
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
        let val = self.call_runtime_2_boxed("molt_contains", op.operands[1], op.operands[0]);
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

        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) => {
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
                let rt_name = format!("molt_{}", name);
                let lhs_i64 = self.ensure_i64(lhs);
                let rhs_i64 = self.ensure_i64(rhs);
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

    // ── Box / Unbox ──

    fn materialize_dynbox_bits(
        &self,
        operand: BasicValueEnum<'ctx>,
        operand_ty: &TirType,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        match operand_ty {
            TirType::I64 => {
                let raw = self.ensure_i64(operand);
                let masked = self
                    .backend
                    .builder
                    .build_and(raw, i64_ty.const_int(nanbox::INT_MASK, false), "mask")
                    .unwrap();
                self.backend
                    .builder
                    .build_or(
                        masked,
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_INT, false),
                        "box_i64",
                    )
                    .unwrap()
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
            | TirType::Set(_)
            | TirType::Tuple(_)
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

    fn lower_terminator(&mut self, term: &Terminator) {
        match term {
            Terminator::Branch { target, args } => {
                let target_bb = self.block_map[target];
                // Record args for phi resolution.
                self.record_branch_args(*target, args);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
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

                self.record_branch_args(*then_block, then_args);
                self.record_branch_args(*else_block, else_args);

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
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
                self.record_branch_args(*default, default_args);

                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_llvm_edge(current_bb, default_bb);

                let mut switch_cases: Vec<_> = Vec::with_capacity(cases.len());
                for (case_val, target, args) in cases {
                    let case_const = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*case_val as u64, *case_val < 0);
                    self.record_branch_args(*target, args);
                    let target_bb = self.block_map[target];
                    self.record_llvm_edge(current_bb, target_bb);
                    switch_cases.push((case_const, target_bb));
                }

                self.backend
                    .builder
                    .build_switch(switch_int, default_bb, &switch_cases)
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

    /// Record that a branch from the current block passes `args` to `target`.
    fn record_branch_args(&mut self, _target: BlockId, _args: &[ValueId]) {
        // Phi incoming values are wired up in finalize_phis using
        // the predecessor information from the TIR blocks.
    }

    /// After all blocks are lowered, wire up phi node incoming values.
    /// Values are coerced to match the phi node's type when needed (e.g., an
    /// i1 bool flowing into an i64 phi is zero-extended).
    ///
    /// This method also handles:
    /// - Mid-block branches from CheckException (not visible in TIR terminators)
    /// - Missing predecessors: if a phi node doesn't have an incoming value for
    ///   some predecessor, an `undef` entry is added so LLVM verification passes
    fn finalize_phis(&mut self) {
        // Collect phi info first to avoid borrow conflicts.
        let phi_info: Vec<_> = self
            .pending_phis
            .iter()
            .map(|(bid, idx, phi)| (*bid, *idx, phi.as_basic_value().get_type(), *phi))
            .collect();

        // Snapshot mid-block branches to avoid borrow conflicts.
        let mid_block_branches: Vec<_> = self.mid_block_branches.clone();

        for (block_id, arg_index, phi_ty, phi) in &phi_info {
            let block = self.func.blocks.get(block_id).unwrap();
            let phi_tir_ty = block
                .args
                .get(*arg_index)
                .map(|arg| arg.ty.clone())
                .unwrap_or(TirType::DynBox);

            // 1. Wire up predecessors from TIR terminators.
            for (pred_id, pred_block) in &self.func.blocks {
                let branch_args = self.get_branch_args_to(&pred_block.terminator, *block_id);
                if let Some(args) = branch_args
                    && *arg_index < args.len()
                {
                    let val_id = args[*arg_index];
                    if let Some(val) = self.values.get(&val_id) {
                        let pred_bb = self
                            .exit_block_map
                            .get(pred_id)
                            .copied()
                            .unwrap_or(self.block_map[pred_id]);
                        let source_tir_ty = self
                            .value_types
                            .get(&val_id)
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        let coerced =
                            self.coerce_to_tir_type(*val, &source_tir_ty, &phi_tir_ty, pred_bb);
                        let coerced = self.coerce_to_type(coerced, *phi_ty, pred_bb);
                        phi.add_incoming(&[(&coerced, pred_bb)]);
                    }
                }
            }

            // 2. Wire up mid-block branches (CheckException -> handler block).
            //    These branches target `block_id` but aren't recorded in any
            //    TIR terminator, so the loop above misses them.
            for (src_bb, target_bid) in &mid_block_branches {
                if *target_bid == *block_id {
                    // The handler block's phi expects a value from this predecessor.
                    // We don't have specific args for mid-block branches, so use undef.
                    let undef = self.get_undef_for_type(*phi_ty);
                    phi.add_incoming(&[(&undef, *src_bb)]);
                }
            }

            // 3. If the original TIR entry block was demoted behind a
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

        // 4. Final safety net: scan all phi nodes for missing predecessors.
        //    If any LLVM predecessor block is missing from a phi's incoming
        //    list, add an undef entry. This catches edge cases from synthetic
        //    blocks, trampoline blocks, and any other control flow that the
        //    TIR-level analysis doesn't fully capture.
        self.patch_incomplete_phis();
    }

    /// For each phi node in the function, check that every LLVM predecessor
    /// of the phi's parent block has an incoming entry.  Add `undef` entries
    /// for any that are missing.
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
                    let phi_ty = phi.as_basic_value().get_type();
                    for pred_bb in preds {
                        if !covered.contains(pred_bb) {
                            let undef = self.get_undef_for_type(phi_ty);
                            phi.add_incoming(&[(&undef, *pred_bb)]);
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
            _ => panic!(
                "unsupported LLVM phi coercion from {:?} to {:?} in block {:?}",
                val_ty, target_ty, in_block
            ),
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
            self.materialize_dynbox_bits(val, source_tir_ty).into()
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

    /// If `term` branches to `target`, return the args it passes; otherwise None.
    fn get_branch_args_to<'a>(
        &self,
        term: &'a Terminator,
        target: BlockId,
    ) -> Option<&'a Vec<ValueId>> {
        match term {
            Terminator::Branch {
                target: t, args, ..
            } if *t == target => Some(args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == target {
                    Some(then_args)
                } else if *else_block == target {
                    Some(else_args)
                } else {
                    None
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, bid, args) in cases {
                    if *bid == target {
                        return Some(args);
                    }
                }
                if *default == target {
                    Some(default_args)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    // ── Helpers ──

    /// Resolve a ValueId to its LLVM value.
    ///
    /// If the value was never defined (e.g. the defining block was unreachable,
    /// or a mid-block split made a value invisible), return an `undef i64`
    /// sentinel instead of panicking.  The resulting IR may be semantically
    /// wrong, but it will pass LLVM verification — which is the goal for
    /// graceful degradation on complex programs.
    fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        if let Some(val) = self.values.get(&id) {
            *val
        } else {
            eprintln!(
                "LLVM lowering warning: ValueId %{} not found — inserting undef i64",
                id.0
            );
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

    fn ensure_runtime_i64_fn(&self, name: &str, param_count: usize) -> FunctionValue<'ctx> {
        if let Some(func) = self.backend.module.get_function(name) {
            return func;
        }
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
        let func = self
            .backend
            .module
            .add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        // All molt runtime functions use explicit error returns (NaN-boxed
        // sentinels) and catch_unwind at FFI boundaries — no C++ exceptions
        // escape.  Adding nounwind + willreturn lets LLVM omit landing pads,
        // perform aggressive code motion, and inline/CSE through call sites.
        let nounwind_kind =
            inkwell::attributes::Attribute::get_named_enum_kind_id("nounwind");
        func.add_attribute(
            AttributeLoc::Function,
            self.backend
                .context
                .create_enum_attribute(nounwind_kind, 0),
        );
        let willreturn_kind =
            inkwell::attributes::Attribute::get_named_enum_kind_id("willreturn");
        func.add_attribute(
            AttributeLoc::Function,
            self.backend
                .context
                .create_enum_attribute(willreturn_kind, 0),
        );
        func
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

    fn ensure_function_symbol(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let param_count = self
            .backend
            .function_param_types
            .get(name)
            .map(|tys| tys.len())
            .unwrap_or(arity + usize::from(has_closure));
        if let Some(func) = self.backend.module.get_function(name) {
            return func;
        }
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..param_count).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
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
            let target_ty = target_fn
                .get_nth_param((idx + usize::from(has_closure)) as u32)
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
            result,
            &target_return_tir_ty,
        );
        builder.build_return(Some(&ret_bits)).unwrap();
        trampoline_fn
    }

    fn lower_preserved_simpleir_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
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
            "code_new" => {
                if op.operands.len() != 8 {
                    return false;
                }
                let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                    .operands
                    .iter()
                    .map(|&id| self.materialize_dynbox_operand(id).into())
                    .collect();
                let code_new_fn = self.ensure_runtime_i64_fn("molt_code_new", 8);
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
                let fn_name = match op.attrs.get("container_type").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) {
                    Some("list") | Some("list_int") => "molt_len_list",
                    Some("str") => "molt_len_str",
                    Some("dict") => "molt_len_dict",
                    Some("tuple") => "molt_len_tuple",
                    Some("set") | Some("frozenset") => "molt_len_set",
                    _ => "molt_len",
                };
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
                let push_fn = self.ensure_runtime_i64_fn("molt_list_builder_append", 2);
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
                let set_fn = self.ensure_runtime_i64_fn("molt_dict_builder_append", 3);
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
                let append_fn = self.ensure_runtime_i64_fn("molt_list_builder_append", 2);
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
                let append_fn = self.ensure_runtime_i64_fn("molt_set_builder_append", 2);
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
                let name_bits = self.ensure_i64(self.resolve(op.operands[0]));
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
                    let base_bits = self.ensure_i64(self.resolve(op.operands[1 + idx]));
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
                    let value_bits = self.ensure_i64(self.resolve(op.operands[attrs_start + idx]));
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
            "module_get_attr" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_module_get_attr", 2);
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
            _ => false,
        }
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
            let arg = self.resolve(arg_id);
            let arg_i64 = self.ensure_i64(arg);
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
                let arg = self.resolve(arg_id);
                args.push(self.ensure_i64(arg).into());
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

    fn initialize_state_resume_blocks(&mut self) {
        let mut const_ints = std::collections::HashMap::new();
        for block in self.func.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && let Some(result) = op.results.first()
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    const_ints.insert(*result, *v);
                }
            }
        }
        let mut resume_ids = std::collections::BTreeSet::new();
        for block in self.func.blocks.values() {
            for op in &block.ops {
                match op.opcode {
                    OpCode::StateYield => {
                        if let Some(AttrValue::Int(state_id)) = op.attrs.get("value") {
                            resume_ids.insert(*state_id);
                        }
                    }
                    OpCode::StateTransition => {
                        if let Some(AttrValue::Int(state_id)) = op.attrs.get("value") {
                            resume_ids.insert(*state_id);
                        }
                        let pending_idx = if op.operands.len() == 2 { 1 } else { 2 };
                        if let Some(state_id) = op
                            .operands
                            .get(pending_idx)
                            .and_then(|id| const_ints.get(id))
                        {
                            resume_ids.insert(*state_id);
                        }
                    }
                    OpCode::ChanSendYield => {
                        if let Some(AttrValue::Int(state_id)) = op.attrs.get("value") {
                            resume_ids.insert(*state_id);
                        }
                        if let Some(state_id) = op.operands.get(2).and_then(|id| const_ints.get(id))
                        {
                            resume_ids.insert(*state_id);
                        }
                    }
                    OpCode::ChanRecvYield => {
                        if let Some(AttrValue::Int(state_id)) = op.attrs.get("value") {
                            resume_ids.insert(*state_id);
                        }
                        if let Some(state_id) = op.operands.get(1).and_then(|id| const_ints.get(id))
                        {
                            resume_ids.insert(*state_id);
                        }
                    }
                    _ => {}
                }
            }
        }
        if resume_ids.is_empty() {
            return;
        }
        for state_id in resume_ids {
            let bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, &format!("state_resume_{}", state_id));
            self.all_llvm_blocks.push(bb);
            self.state_resume_blocks.insert(state_id, bb);
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
    fn call_runtime_2(
        &self,
        name: &str,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let func = self
            .backend
            .module
            .get_function(name)
            .unwrap_or_else(|| panic!("Runtime function '{}' not declared", name));
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
    use crate::tir::values::ValueId;
    use inkwell::context::Context;
    use inkwell::values::AnyValue;

    fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
        let backend = LlvmBackend::new(ctx, "test");
        declare_runtime_functions(ctx, &backend.module);
        backend
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
            exit_block_map: HashMap::new(),
            values: HashMap::new(),
            value_types: HashMap::new(),
            pending_phis: Vec::new(),
            pgo_branch_weights: None,
            pgo_weight_index: 0,
            const_str_counter: 0,
            synthetic_block_counter: 0,
            mid_block_branches: Vec::new(),
            all_llvm_blocks: Vec::new(),
            llvm_pred_map: HashMap::new(),
            state_resume_blocks: HashMap::new(),
            try_stack_baselines: Vec::new(),
            call_site_counter: 0,
        }
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
    fn lower_i64_add() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn add(a: i64, b: i64) -> i64 { return a + b }
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

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        let ir = llvm_fn.print_to_string().to_string();

        // Should contain a native `add` instruction, NOT a call to molt_add
        assert!(
            ir.contains("add i64"),
            "expected native i64 add in IR: {}",
            ir
        );
        assert!(
            !ir.contains("call") || !ir.contains("molt_add"),
            "should NOT call runtime for i64+i64 add"
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
    fn lower_preserved_dict_update_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("dict_update_preserved".into(), vec![], TirType::DynBox);
        let dict_bits = func.fresh_value();
        let other_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
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

    #[test]
    fn lower_preserved_len_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("len_preserved".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
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
        assert!(ir.contains("molt_len_tuple"), "{ir}");
    }

    #[test]
    fn lower_preserved_list_append_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new("list_append_preserved".into(), vec![], TirType::DynBox);
        let list_bits = func.fresh_value();
        let item_bits = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
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
    fn lower_preserved_tuple_from_list_calls_runtime() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func =
            TirFunction::new("tuple_from_list_preserved".into(), vec![], TirType::DynBox);
        let list_bits = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
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
        let backend = make_backend(&ctx);

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
        let backend = make_backend(&ctx);

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

        assert_eq!(rpo.len(), 4, "all four blocks must appear in RPO: {:?}", rpo);
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

        assert_eq!(rpo.len(), 4, "all four blocks must appear in RPO: {:?}", rpo);

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
