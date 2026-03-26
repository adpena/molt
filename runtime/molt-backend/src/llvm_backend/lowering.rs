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
use crate::llvm_backend::types::lower_type;
#[cfg(feature = "llvm")]
use crate::llvm_backend::LlvmBackend;
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
    pub const TAG_PTR: u64 = 0x0004_0000_0000_0000;
    pub const INT_SIGN_BIT: u64 = 1 << 46;
    pub const INT_MASK: u64 = (1u64 << 47) - 1;
}

/// Holds state during lowering of a single TIR function.
#[cfg(feature = "llvm")]
struct FunctionLowering<'ctx, 'func> {
    backend: &'func LlvmBackend<'ctx>,
    func: &'func TirFunction,
    llvm_fn: FunctionValue<'ctx>,
    /// Maps TIR BlockId -> LLVM BasicBlock.
    block_map: HashMap<BlockId, BasicBlock<'ctx>>,
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
    // 1. Build the LLVM function signature.
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

    let mut lowering = FunctionLowering {
        backend,
        func,
        llvm_fn,
        block_map: HashMap::new(),
        values: HashMap::new(),
        value_types: HashMap::new(),
        pending_phis: Vec::new(),
        pgo_branch_weights,
        pgo_weight_index: 0,
        const_str_counter: 0,
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
    }
    for block_id in func.blocks.keys() {
        if *block_id == func.entry_block {
            continue; // already created above
        }
        let bb = backend
            .context
            .append_basic_block(llvm_fn, &format!("bb{}", block_id.0));
        lowering.block_map.insert(*block_id, bb);
    }

    // 2b. LLVM requires the entry block to have no predecessors.  If any TIR
    //     block branches back to the entry, insert a trampoline block that
    //     becomes the real LLVM entry and immediately jumps to the TIR entry.
    {
        let entry_id = func.entry_block;
        let entry_has_predecessors = func.blocks.values().any(|blk| {
            match &blk.terminator {
                Terminator::Branch { target, .. } => *target == entry_id,
                Terminator::CondBranch { then_block, else_block, .. } => {
                    *then_block == entry_id || *else_block == entry_id
                }
                Terminator::Switch { cases, default, .. } => {
                    *default == entry_id || cases.iter().any(|(_, t, _)| *t == entry_id)
                }
                _ => false,
            }
        });
        if entry_has_predecessors {
            let old_entry_bb = lowering.block_map[&entry_id];
            let trampoline_bb = backend
                .context
                .prepend_basic_block(old_entry_bb, "entry_trampoline");
            // The trampoline block jumps to the real entry.
            backend.builder.position_at_end(trampoline_bb);
            backend.builder.build_unconditional_branch(old_entry_bb).unwrap();
        }
    }

    // 3. Compute RPO ordering (simple BFS from entry for now).
    let rpo = lowering.compute_rpo();

    // 4. Lower each block.
    for block_id in &rpo {
        lowering.lower_block(*block_id);
    }

    // 5. Emit `unreachable` terminators for any LLVM basic blocks that were
    //    created for TIR blocks but not visited during RPO traversal (dead
    //    blocks unreachable from the entry).  Without this, LLVM verification
    //    fails because every basic block must have a terminator instruction.
    {
        let rpo_set: std::collections::HashSet<BlockId> = rpo.iter().copied().collect();
        for (block_id, llvm_bb) in &lowering.block_map {
            if !rpo_set.contains(block_id) {
                backend.builder.position_at_end(*llvm_bb);
                backend.builder.build_unreachable().unwrap();
            }
        }
    }

    // 6. Wire up phi incoming values.
    lowering.finalize_phis();

    // 7. If any op in this function carries `fast_math = true`, annotate the
    //    function with `"unsafe-fp-math"="true"`.  This is the function-level
    //    fallback for LLVM passes that inspect function attributes rather than
    //    per-instruction fast-math flags.
    let has_any_fast_math = func.blocks.values().any(|blk| {
        blk.ops.iter().any(|op| has_attr(op, "fast_math"))
    });
    if has_any_fast_math {
        let attr = backend
            .context
            .create_string_attribute("unsafe-fp-math", "true");
        llvm_fn.add_attribute(AttributeLoc::Function, attr);
    }

    llvm_fn
}

#[cfg(feature = "llvm")]
impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Compute a reverse-post-order traversal of blocks starting from entry.
    fn compute_rpo(&self) -> Vec<BlockId> {
        let mut visited = std::collections::HashSet::new();
        let mut post_order = Vec::new();
        self.dfs_post_order(self.func.entry_block, &mut visited, &mut post_order);
        post_order.reverse();
        post_order
    }

    fn dfs_post_order(
        &self,
        block_id: BlockId,
        visited: &mut std::collections::HashSet<BlockId>,
        post_order: &mut Vec<BlockId>,
    ) {
        if !visited.insert(block_id) {
            return;
        }
        if let Some(block) = self.func.blocks.get(&block_id) {
            for succ in Self::terminator_successors(&block.terminator) {
                self.dfs_post_order(succ, visited, post_order);
            }
        }
        post_order.push(block_id);
    }

    fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
        match term {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch {
                cases, default, ..
            } => {
                let mut succs: Vec<BlockId> =
                    cases.iter().map(|(_, bid, _)| *bid).collect();
                succs.push(*default);
                succs
            }
            Terminator::Return { .. } | Terminator::Unreachable => vec![],
        }
    }

    fn lower_block(&mut self, block_id: BlockId) {
        let block = self.func.blocks.get(&block_id).unwrap().clone();
        let bb = self.block_map[&block_id];
        self.backend.builder.position_at_end(bb);

        // Entry block: map block args to function parameters.
        if block_id == self.func.entry_block {
            for (i, arg) in block.args.iter().enumerate() {
                let param = self.llvm_fn.get_nth_param(i as u32).unwrap();
                self.values.insert(arg.id, param);
                self.value_types.insert(arg.id, arg.ty.clone());
            }
        } else {
            // Non-entry blocks: create phi nodes for block arguments.
            for (i, arg) in block.args.iter().enumerate() {
                let llvm_ty = lower_type(self.backend.context, &arg.ty);
                let phi = self.backend.builder.build_phi(llvm_ty, &format!("phi_{}", arg.id.0)).unwrap();
                self.values.insert(arg.id, phi.as_basic_value());
                self.value_types.insert(arg.id, arg.ty.clone());
                self.pending_phis.push((block_id, i, phi));
            }
        }

        // Lower each operation.
        for op in &block.ops {
            self.lower_op(op);
        }

        // Lower terminator.
        self.lower_terminator(&block.terminator);
    }

    fn lower_op(&mut self, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // ── Constants ──
            OpCode::ConstInt => {
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Int(v)) => *v,
                    _ => 0, // graceful fallback for malformed TIR
                };
                let result_id = op.results[0];
                // NaN-box the integer for the runtime ABI.
                // Small integers (fits in 47 bits): QNAN | TAG_INT | (val & INT_MASK)
                // Sign extension: if negative, set the sign bit.
                let boxed = {
                    let masked = (val as u64) & nanbox::INT_MASK;
                    let bits = nanbox::QNAN | nanbox::TAG_INT | masked;
                    bits
                };
                let llvm_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(boxed, false)
                    .into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::ConstFloat => {
                let val = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
                    Some(AttrValue::Float(v)) => *v,
                    _ => 0.0, // graceful fallback for malformed TIR
                };
                let result_id = op.results[0];
                let llvm_val = self
                    .backend
                    .context
                    .f64_type()
                    .const_float(val)
                    .into();
                self.values.insert(result_id, llvm_val);
                self.value_types.insert(result_id, TirType::F64);
            }
            OpCode::ConstBool => {
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Bool(v)) => *v,
                    _ => false, // graceful fallback for malformed TIR
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
                global.set_initializer(
                    &self.backend.context.const_string(&str_bytes, false),
                );
                global.set_constant(true);
                global.set_unnamed_addr(true);

                // Get or declare molt_string_from_bytes.
                let sfb_fn = if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
                    f
                } else {
                    let ptr_ty = self.backend.context.ptr_type(inkwell::AddressSpace::default());
                    let i32_ty = self.backend.context.i32_type();
                    let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
                    self.backend.module.add_function(
                        "molt_string_from_bytes",
                        fn_ty,
                        Some(inkwell::module::Linkage::External),
                    )
                };

                // Allocate a stack slot for the output u64.
                let out_alloca = self.backend.builder.build_alloca(i64_ty, "str_out").unwrap();

                // Call molt_string_from_bytes(ptr, len, out).
                let ptr_val = global.as_pointer_value();
                let len_val = i64_ty.const_int(str_bytes.len() as u64, false);
                self.backend
                    .builder
                    .build_call(sfb_fn, &[ptr_val.into(), len_val.into(), out_alloca.into()], "sfb")
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
                global.set_initializer(
                    &self.backend.context.const_string(&raw_bytes, false),
                );
                global.set_constant(true);
                global.set_unnamed_addr(true);

                // Get or declare molt_string_from_bytes (used for bytes too — the
                // runtime creates a bytes object when the caller context is ConstBytes).
                let sfb_fn = if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
                    f
                } else {
                    let ptr_ty = self.backend.context.ptr_type(inkwell::AddressSpace::default());
                    let i32_ty = self.backend.context.i32_type();
                    let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
                    self.backend.module.add_function(
                        "molt_string_from_bytes",
                        fn_ty,
                        Some(inkwell::module::Linkage::External),
                    )
                };

                // Allocate a stack slot for the output u64.
                let out_alloca = self.backend.builder.build_alloca(i64_ty, "bytes_out").unwrap();

                // Call molt_string_from_bytes(ptr, len, out).
                let ptr_val = global.as_pointer_value();
                let len_val = i64_ty.const_int(raw_bytes.len() as u64, false);
                self.backend
                    .builder
                    .build_call(sfb_fn, &[ptr_val.into(), len_val.into(), out_alloca.into()], "bfb")
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
            OpCode::Add => self.emit_binary_arith(op, "add"),
            OpCode::Sub => self.emit_binary_arith(op, "sub"),
            OpCode::Mul => self.emit_binary_arith(op, "mul"),
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
                // Short-circuit boolean ops: in SSA these are already lowered
                // to branches, but if they appear as ops, use runtime dispatch.
                let result_id = op.results[0];
                let lhs = self.resolve(op.operands[0]);
                let rhs = self.resolve(op.operands[1]);
                let rt_name = if op.opcode == OpCode::And {
                    "molt_bit_and"
                } else {
                    "molt_bit_or"
                };
                let val = self.call_runtime_2(rt_name, lhs, rhs);
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
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
                let ty = self.value_types.get(&op.operands[0]).cloned().unwrap_or(TirType::DynBox);
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
                    let ty = self.value_types.get(&op.operands[0]).cloned().unwrap_or(TirType::DynBox);
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
                let obj = self.resolve(op.operands[0]);
                // Attribute name is stored in attrs["name"], not as a second operand.
                let attr_name = op.attrs.get("name")
                    .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                    .unwrap_or("<unknown>");
                let name = self.intern_string_const(attr_name);
                let val = self.call_runtime_2("molt_get_attr_name", obj, name);
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StoreAttr => {
                let obj = self.resolve(op.operands[0]);
                let attr_name = op.attrs.get("name")
                    .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                    .unwrap_or("<unknown>");
                let name = self.intern_string_const(attr_name);
                let val = self.resolve(op.operands[1]);
                let obj_i64 = self.ensure_i64(obj);
                let name_i64 = self.ensure_i64(name);
                let val_i64 = self.ensure_i64(val);
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
                let obj = self.resolve(op.operands[0]);
                let attr_name = op.attrs.get("name")
                    .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                    .unwrap_or("<unknown>");
                let name = self.intern_string_const(attr_name);
                let val = self.call_runtime_2("molt_del_attr_name", obj, name);
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }
            OpCode::Index => {
                let result_id = op.results[0];
                let obj = self.resolve(op.operands[0]);
                let key = self.resolve(op.operands[1]);
                // BCE: when the bounds-check elimination pass has proven the index
                // is in-range, we call `molt_getitem_unchecked` which skips the
                // runtime bounds check and associated branch entirely.
                let val = if has_attr(op, "bce_safe") {
                    self.call_runtime_2("molt_getitem_unchecked", obj, key)
                } else {
                    self.call_runtime_2("molt_getitem_method", obj, key)
                };
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StoreIndex => {
                let obj = self.resolve(op.operands[0]);
                let key = self.resolve(op.operands[1]);
                let val = self.resolve(op.operands[2]);
                let obj_i64 = self.ensure_i64(obj);
                let key_i64 = self.ensure_i64(key);
                let val_i64 = self.ensure_i64(val);
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
                let obj = self.resolve(op.operands[0]);
                let key = self.resolve(op.operands[1]);
                let val = self.call_runtime_2("molt_delitem_method", obj, key);
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // ── Call ──
            OpCode::Call => {
                let i64_ty = self.backend.context.i64_type();

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

                if let Some(ref target_name) = direct_target {
                    if let Some(target_fn) = self.backend.module.get_function(target_name) {
                        // Direct call — all operands are positional args.
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                            .operands
                            .iter()
                            .map(|&id| {
                                let v = self.resolve(id);
                                let v_i64 = self.ensure_i64(v);
                                v_i64.into()
                            })
                            .collect();
                        let call_result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap();
                        if let Some(&result_id) = op.results.first() {
                            let result = call_result
                                .try_as_basic_value()
                                .basic()
                                .unwrap_or_else(|| i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false).into());
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    } else {
                        // Target not yet in module — forward-declare it and call.
                        let param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                            op.operands.iter().map(|_| i64_ty.into()).collect();
                        let fn_ty = i64_ty.fn_type(&param_types, false);
                        let target_fn = self.backend.module.add_function(
                            target_name,
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        );
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                            .operands
                            .iter()
                            .map(|&id| {
                                let v = self.resolve(id);
                                let v_i64 = self.ensure_i64(v);
                                v_i64.into()
                            })
                            .collect();
                        let result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap_or_else(|| i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false).into());
                        if let Some(&result_id) = op.results.first() {
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    }
                } else if !op.operands.is_empty() {
                    // Indirect call: operands[0] = callable, rest = positional args.
                    let callable = self.resolve(op.operands[0]);
                    let callable_i64 = self.ensure_i64(callable);

                    let n_args = (op.operands.len() - 1) as u64;

                    let new_fn = self.backend.module.get_function("molt_callargs_new").unwrap();
                    let builder_val = self
                        .backend
                        .builder
                        .build_call(
                            new_fn,
                            &[
                                i64_ty.const_int(n_args, false).into(),
                                i64_ty.const_int(0, false).into(),
                            ],
                            "callargs",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();

                    let push_fn = self.backend.module.get_function("molt_callargs_push_pos").unwrap();
                    for &arg_id in &op.operands[1..] {
                        let arg = self.resolve(arg_id);
                        let arg_i64 = self.ensure_i64(arg);
                        self.backend
                            .builder
                            .build_call(push_fn, &[builder_val.into(), arg_i64.into()], "push")
                            .unwrap();
                    }

                    let bind_fn = self.backend.module.get_function("molt_call_bind").unwrap();
                    let result = self
                        .backend
                        .builder
                        .build_call(bind_fn, &[callable_i64.into(), builder_val.into()], "call_result")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();

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
                    let ty = self.value_types.get(&op.operands[0]).cloned().unwrap_or(TirType::DynBox);
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
                let receiver = self.resolve(op.operands[0]);
                let receiver_i64 = self.ensure_i64(receiver);

                let method_name = if op.operands.len() > 1 {
                    let mv = self.resolve(op.operands[1]);
                    self.ensure_i64(mv)
                } else if let Some(method_str) = op.attrs.get("method")
                    .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                {
                    // Method name stored in attrs (from SSA s_value), not as an operand.
                    let name_val = self.intern_string_const(method_str);
                    self.ensure_i64(name_val)
                } else if let Some(name_str) = op.attrs.get("name")
                    .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                {
                    let name_val = self.intern_string_const(name_str);
                    self.ensure_i64(name_val)
                } else {
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                };

                // Build positional args (operands[2..])
                let n_args = op.operands.len().saturating_sub(2) as u64;
                let new_fn = self.backend.module.get_function("molt_callargs_new").unwrap();
                let args_builder = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_int(n_args, false).into(), i64_ty.const_int(0, false).into()],
                        "cm_args",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.backend.module.get_function("molt_callargs_push_pos").unwrap();
                for &arg_id in op.operands.get(2..).unwrap_or(&[]) {
                    let arg = self.resolve(arg_id);
                    let arg_i64 = self.ensure_i64(arg);
                    self.backend
                        .builder
                        .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cm_push")
                        .unwrap();
                }

                let call_method_fn = self.backend.module.get_function("molt_call_method").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        call_method_fn,
                        &[receiver_i64.into(), method_name.into(), args_builder.into()],
                        "call_method",
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

                // For "print", use the dedicated molt_print_obj runtime
                // function which handles printing + newline in one call.
                if builtin_name_str.as_deref() == Some("print")
                    || builtin_name_str.as_deref() == Some("builtin_print")
                {
                    let print_fn = if let Some(f) = self.backend.module.get_function("molt_print_obj") {
                        f
                    } else {
                        // Forward-declare molt_print_obj(u64) -> void
                        let void_ty = self.backend.context.void_type();
                        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
                        self.backend.module.add_function(
                            "molt_print_obj",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    };
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg = self.resolve(arg_id);
                        let arg_i64 = self.ensure_i64(arg);
                        self.backend
                            .builder
                            .build_call(print_fn, &[arg_i64.into()], "print")
                            .unwrap();
                    }
                    // print returns None
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
                    } else if let Some(s_val) = op.attrs.get("s_value")
                        .and_then(|v| if let AttrValue::Str(s) = v { Some(s.as_str()) } else { None })
                    {
                        let name_val = self.intern_string_const(s_val);
                        self.ensure_i64(name_val)
                    } else {
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    };

                    let n_args = op.operands.len().saturating_sub(args_start) as u64;
                    let new_fn = self.backend.module.get_function("molt_callargs_new").unwrap();
                    let args_builder = self
                        .backend
                        .builder
                        .build_call(
                            new_fn,
                            &[i64_ty.const_int(n_args, false).into(), i64_ty.const_int(0, false).into()],
                            "cb_args",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    let push_fn = self.backend.module.get_function("molt_callargs_push_pos").unwrap();
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg = self.resolve(arg_id);
                        let arg_i64 = self.ensure_i64(arg);
                        self.backend
                            .builder
                            .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cb_push")
                            .unwrap();
                    }

                    let call_builtin_fn = self.backend.module.get_function("molt_call_builtin").unwrap();
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
            // Strategy: molt_list_new(capacity) then molt_list_push for each item.
            OpCode::BuildList => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let list_new_fn = self.backend.module.get_function("molt_list_new").unwrap();
                let list = self
                    .backend
                    .builder
                    .build_call(list_new_fn, &[i64_ty.const_int(n, false).into()], "list")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.backend.module.get_function("molt_list_push").unwrap();
                for &item_id in &op.operands {
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
                    self.backend
                        .builder
                        .build_call(push_fn, &[list.into(), item_i64.into()], "list_push")
                        .unwrap();
                }
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
                let dict_new_fn = self.backend.module.get_function("molt_dict_new").unwrap();
                let dict = self
                    .backend
                    .builder
                    .build_call(dict_new_fn, &[i64_ty.const_int(n_pairs, false).into()], "dict")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let dict_set_fn = self.backend.module.get_function("molt_dict_set").unwrap();
                let mut i = 0;
                while i + 1 < op.operands.len() {
                    let k = self.resolve(op.operands[i]);
                    let v = self.resolve(op.operands[i + 1]);
                    let k_i64 = self.ensure_i64(k);
                    let v_i64 = self.ensure_i64(v);
                    self.backend
                        .builder
                        .build_call(dict_set_fn, &[dict.into(), k_i64.into(), v_i64.into()], "dict_set")
                        .unwrap();
                    i += 2;
                }
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, dict);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildTuple: (item0, item1, ...) ──
            OpCode::BuildTuple => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let tup_new_fn = self.backend.module.get_function("molt_tuple_new").unwrap();
                let tup = self
                    .backend
                    .builder
                    .build_call(tup_new_fn, &[i64_ty.const_int(n, false).into()], "tuple")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.backend.module.get_function("molt_tuple_push").unwrap();
                for &item_id in &op.operands {
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
                    self.backend
                        .builder
                        .build_call(push_fn, &[tup.into(), item_i64.into()], "tup_push")
                        .unwrap();
                }
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, tup);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildSet: {item0, item1, ...} ──
            OpCode::BuildSet => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let set_new_fn = self.backend.module.get_function("molt_set_new").unwrap();
                let set = self
                    .backend
                    .builder
                    .build_call(set_new_fn, &[i64_ty.const_int(n, false).into()], "set")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.backend.module.get_function("molt_set_push").unwrap();
                for &item_id in &op.operands {
                    let item = self.resolve(item_id);
                    let item_i64 = self.ensure_i64(item);
                    self.backend
                        .builder
                        .build_call(push_fn, &[set.into(), item_i64.into()], "set_push")
                        .unwrap();
                }
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

                let start = if op.operands.len() > 0 {
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

            // ── CheckException: inspect the current exception state ──
            // The runtime exposes `molt_exception_pending` (returns u64 bool).
            // For MVP, we call it and ignore the result (no branch to handler).
            OpCode::CheckException => {
                let check_fn = self.backend.module.get_function("molt_exception_pending").unwrap_or_else(|| {
                    let i64_ty = self.backend.context.i64_type();
                    let fn_ty = i64_ty.fn_type(&[], false);
                    self.backend.module.add_function("molt_exception_pending", fn_ty, Some(inkwell::module::Linkage::External))
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
            }

            // ── Import: import module by name ──
            OpCode::Import => {
                let result_id = op.results[0];
                let name = self.resolve(op.operands[0]);
                let name_i64 = self.ensure_i64(name);
                let import_fn = self.backend.module.get_function("molt_module_import").unwrap();
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
                let module_val = self.resolve(op.operands[0]);
                let attr_val = self.resolve(op.operands[1]);
                let result = self.call_runtime_2("molt_module_get_attr", module_val, attr_val);
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
                let cond_i1 = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::NE,
                    truthy_result.into_int_value(),
                    i64_ty.const_int(0, false),
                    "scf_if_cond",
                ).unwrap();

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
                let then_bb = self.backend.context.append_basic_block(current_fn, "scf_if_then");
                let else_bb = self.backend.context.append_basic_block(current_fn, "scf_if_else");
                let merge_bb = self.backend.context.append_basic_block(current_fn, "scf_if_merge");

                self.backend.builder.build_conditional_branch(cond_i1, then_bb, else_bb).unwrap();

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
                self.backend.builder.build_unconditional_branch(merge_bb).unwrap();
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
                self.backend.builder.build_unconditional_branch(merge_bb).unwrap();
                let else_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Merge block: phi node selects then/else result.
                self.backend.builder.position_at_end(merge_bb);
                let phi = self.backend.builder.build_phi(i64_ty, "scf_if_phi").unwrap();
                phi.add_incoming(&[
                    (&then_result, then_exit_bb),
                    (&else_result, else_exit_bb),
                ]);
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
                    let v = self.resolve(op.operands[0]); self.ensure_i64(v)
                } else { i64_ty.const_int(0, false) };
                let ub = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]); self.ensure_i64(v)
                } else { i64_ty.const_int(0, false) };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]); self.ensure_i64(v)
                } else { i64_ty.const_int(1, false) };
                let body_fn_bits = if op.operands.len() > 3 {
                    let v = self.resolve(op.operands[3]); self.ensure_i64(v)
                } else { i64_ty.const_int(0, false) };
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
                    let v = self.resolve(op.operands[0]); self.ensure_i64(v)
                } else { i64_ty.const_int(0, false) };
                let body_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]); self.ensure_i64(v)
                } else { i64_ty.const_int(0, false) };
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
                    let v = self.resolve(op.operands[0]); self.ensure_i64(v)
                } else { i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false) };
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
                let deopt_fn = self.backend.module.get_function("molt_deopt_transfer").unwrap();
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

            // Structural ops that don't produce values — ignored by the LLVM lowering
            // because they are handled at the block/terminator level.
            OpCode::TryStart | OpCode::TryEnd
            | OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }

    // ── Type-specialized binary arithmetic ──

    fn emit_binary_arith(&mut self, op: &crate::tir::ops::TirOp, name: &str) {
        let result_id = op.results[0];
        let lhs_id = op.operands[0];
        let rhs_id = op.operands[1];
        let lhs = self.resolve(lhs_id);
        let rhs = self.resolve(rhs_id);
        let lhs_ty = self.value_types.get(&lhs_id).cloned().unwrap_or(TirType::DynBox);
        let rhs_ty = self.value_types.get(&rhs_id).cloned().unwrap_or(TirType::DynBox);
        let fast_math = has_attr(op, "fast_math");

        let (val, out_ty) = match (&lhs_ty, &rhs_ty, name) {
            // I64 + I64 -> I64 (direct machine instruction)
            (TirType::I64, TirType::I64, "add") => {
                let v = self.backend.builder.build_int_add(
                    lhs.into_int_value(), rhs.into_int_value(), "add",
                ).unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "sub") => {
                let v = self.backend.builder.build_int_sub(
                    lhs.into_int_value(), rhs.into_int_value(), "sub",
                ).unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "mul") => {
                let v = self.backend.builder.build_int_mul(
                    lhs.into_int_value(), rhs.into_int_value(), "mul",
                ).unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::I64, TirType::I64, "div") => {
                // Python `/` on ints always returns float (7 / 2 == 3.5)
                let f64_ty = self.backend.context.f64_type();
                let lhs_f = self.backend.builder.build_signed_int_to_float(
                    lhs.into_int_value(), f64_ty, "div_lhs_f",
                ).unwrap();
                let rhs_f = self.backend.builder.build_signed_int_to_float(
                    rhs.into_int_value(), f64_ty, "div_rhs_f",
                ).unwrap();
                let v = self.backend.builder.build_float_div(lhs_f, rhs_f, "div_f").unwrap();
                (v.into(), TirType::F64)
            }
            (TirType::I64, TirType::I64, "floordiv") => {
                // Python `//`: rounds toward negative infinity (not toward zero like C sdiv).
                // Emit: q = sdiv(lhs, rhs); r = srem(lhs, rhs);
                //       if (r != 0 && (lhs ^ rhs) < 0) q -= 1
                let lhs_i = lhs.into_int_value();
                let rhs_i = rhs.into_int_value();
                let i64_ty = self.backend.context.i64_type();
                let q = self.backend.builder.build_int_signed_div(lhs_i, rhs_i, "fdiv_q").unwrap();
                let r = self.backend.builder.build_int_signed_rem(lhs_i, rhs_i, "fdiv_r").unwrap();
                let zero = i64_ty.const_zero();
                let one = i64_ty.const_int(1, false);
                let r_ne_0 = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::NE, r, zero, "r_ne_0",
                ).unwrap();
                let xor = self.backend.builder.build_xor(lhs_i, rhs_i, "signs_xor").unwrap();
                let signs_differ = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::SLT, xor, zero, "signs_differ",
                ).unwrap();
                let needs_adjust = self.backend.builder.build_and(r_ne_0, signs_differ, "needs_adj").unwrap();
                let q_minus_1 = self.backend.builder.build_int_sub(q, one, "q_m1").unwrap();
                let q_m1_basic: inkwell::values::BasicValueEnum = q_minus_1.into();
                let q_basic: inkwell::values::BasicValueEnum = q.into();
                let adj = self.backend.builder.build_select(
                    needs_adjust, q_m1_basic, q_basic, "floordiv",
                ).unwrap();
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
                let r = self.backend.builder.build_int_signed_rem(lhs_i, rhs_i, "mod_r").unwrap();
                let r_ne_0 = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::NE, r, zero, "mod_r_ne_0",
                ).unwrap();
                let xor = self.backend.builder.build_xor(r, rhs_i, "mod_signs_xor").unwrap();
                let signs_differ = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::SLT, xor, zero, "mod_signs_differ",
                ).unwrap();
                let needs_adjust = self.backend.builder.build_and(r_ne_0, signs_differ, "mod_adj").unwrap();
                let r_plus_rhs = self.backend.builder.build_int_add(r, rhs_i, "mod_adjusted").unwrap();
                let r_adj_basic: inkwell::values::BasicValueEnum = r_plus_rhs.into();
                let r_basic: inkwell::values::BasicValueEnum = r.into();
                let result = self.backend.builder.build_select(
                    needs_adjust, r_adj_basic, r_basic, "pymod",
                ).unwrap();
                (result, TirType::I64)
            }

            // F64 + F64 -> F64 (direct machine instruction).
            // When `fast_math = true` is set on the TIR op (injected by the
            // fast_math annotation pass), we apply LLVM's full fast-math flag
            // set to the emitted instruction via `InstructionValue::set_fast_math_flags`.
            (TirType::F64, TirType::F64, "add") => {
                let v = self.backend.builder.build_float_add(
                    lhs.into_float_value(), rhs.into_float_value(), "fadd",
                ).unwrap();
                if fast_math {
                    if let Some(instr) = v.as_instruction() {
                        instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                    }
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "sub") => {
                let v = self.backend.builder.build_float_sub(
                    lhs.into_float_value(), rhs.into_float_value(), "fsub",
                ).unwrap();
                if fast_math {
                    if let Some(instr) = v.as_instruction() {
                        instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                    }
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mul") => {
                let v = self.backend.builder.build_float_mul(
                    lhs.into_float_value(), rhs.into_float_value(), "fmul",
                ).unwrap();
                if fast_math {
                    if let Some(instr) = v.as_instruction() {
                        instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                    }
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "div") => {
                let v = self.backend.builder.build_float_div(
                    lhs.into_float_value(), rhs.into_float_value(), "fdiv",
                ).unwrap();
                if fast_math {
                    if let Some(instr) = v.as_instruction() {
                        instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                    }
                }
                (v.into(), TirType::F64)
            }
            (TirType::F64, TirType::F64, "mod") => {
                let v = self.backend.builder.build_float_rem(
                    lhs.into_float_value(), rhs.into_float_value(), "fmod",
                ).unwrap();
                if fast_math {
                    if let Some(instr) = v.as_instruction() {
                        instr.set_fast_math_flags(LLVM_FAST_MATH_ALL);
                    }
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
                let lhs_i64 = self.ensure_i64(lhs);
                let rhs_i64 = self.ensure_i64(rhs);
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
        let lhs_ty = self.value_types.get(&lhs_id).cloned().unwrap_or(TirType::DynBox);
        let rhs_ty = self.value_types.get(&rhs_id).cloned().unwrap_or(TirType::DynBox);

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
                let v = self.backend.builder.build_int_compare(
                    pred, lhs.into_int_value(), rhs.into_int_value(), name,
                ).unwrap();
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
                let v = self.backend.builder.build_float_compare(
                    pred, lhs.into_float_value(), rhs.into_float_value(), name,
                ).unwrap();
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
                let lhs_i64 = self.ensure_i64(lhs);
                let rhs_i64 = self.ensure_i64(rhs);
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
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                lhs_i64,
                rhs_i64,
                "is",
            )
            .unwrap();
        let val: BasicValueEnum<'ctx> = if op.opcode == OpCode::IsNot {
            self.backend.builder.build_not(cmp, "is_not").unwrap().into()
        } else {
            cmp.into()
        };
        self.values.insert(result_id, val);
        self.value_types.insert(result_id, TirType::Bool);
    }

    // ── Containment (in / not in) ──

    fn emit_containment(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let item = self.resolve(op.operands[0]);
        let container = self.resolve(op.operands[1]);
        let val = self.call_runtime_2("molt_contains", container, item);
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
        let lhs_ty = self.value_types.get(&lhs_id).cloned().unwrap_or(TirType::DynBox);
        let rhs_ty = self.value_types.get(&rhs_id).cloned().unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&lhs_ty, &rhs_ty) {
            (TirType::I64, TirType::I64) => {
                let v = match name {
                    "bit_and" => self.backend.builder.build_and(
                        lhs.into_int_value(), rhs.into_int_value(), "band",
                    ).unwrap(),
                    "bit_or" => self.backend.builder.build_or(
                        lhs.into_int_value(), rhs.into_int_value(), "bor",
                    ).unwrap(),
                    "bit_xor" => self.backend.builder.build_xor(
                        lhs.into_int_value(), rhs.into_int_value(), "bxor",
                    ).unwrap(),
                    "lshift" => self.backend.builder.build_left_shift(
                        lhs.into_int_value(), rhs.into_int_value(), "shl",
                    ).unwrap(),
                    "rshift" => self.backend.builder.build_right_shift(
                        lhs.into_int_value(), rhs.into_int_value(), true, "shr",
                    ).unwrap(),
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
        let operand_ty = self.value_types.get(&operand_id).cloned().unwrap_or(TirType::DynBox);

        let (val, out_ty) = match (&operand_ty, name) {
            (TirType::I64, "neg") => {
                let v = self.backend.builder.build_int_neg(
                    operand.into_int_value(), "neg",
                ).unwrap();
                (v.into(), TirType::I64)
            }
            (TirType::F64, "neg") => {
                let v = self.backend.builder.build_float_neg(
                    operand.into_float_value(), "fneg",
                ).unwrap();
                (v.into(), TirType::F64)
            }
            (TirType::Bool, "not") => {
                let v = self.backend.builder.build_not(
                    operand.into_int_value(), "not",
                ).unwrap();
                (v.into(), TirType::Bool)
            }
            (TirType::I64, "invert") => {
                let v = self.backend.builder.build_not(
                    operand.into_int_value(), "invert",
                ).unwrap();
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

    fn emit_box(&mut self, op: &crate::tir::ops::TirOp) {
        let result_id = op.results[0];
        let operand_id = op.operands[0];
        let operand = self.resolve(operand_id);
        let operand_ty = self.value_types.get(&operand_id).cloned().unwrap_or(TirType::DynBox);

        let i64_ty = self.backend.context.i64_type();

        let boxed: BasicValueEnum<'ctx> = match &operand_ty {
            TirType::I64 => {
                // NaN-box an i64: QNAN | TAG_INT | (value & INT_MASK)
                let raw = operand.into_int_value();
                let masked = self.backend.builder.build_and(
                    raw,
                    i64_ty.const_int(nanbox::INT_MASK, false),
                    "mask",
                ).unwrap();
                let tagged = self.backend.builder.build_or(
                    masked,
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_INT, false),
                    "box_i64",
                ).unwrap();
                tagged.into()
            }
            TirType::Bool => {
                // NaN-box a bool: QNAN | TAG_BOOL | (val as u64)
                let extended = self.backend.builder.build_int_z_extend(
                    operand.into_int_value(),
                    i64_ty,
                    "zext",
                ).unwrap();
                let tagged = self.backend.builder.build_or(
                    extended,
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_BOOL, false),
                    "box_bool",
                ).unwrap();
                tagged.into()
            }
            TirType::None => {
                // Already a NaN-boxed sentinel
                i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false).into()
            }
            TirType::F64 => {
                // Float boxing: bitcast f64 to i64 — but need to handle NaN canonicalization.
                // For now, just bitcast.
                let as_i64 = self.backend.builder.build_bit_cast(
                    operand, i64_ty, "f64_to_i64",
                ).unwrap();
                as_i64
            }
            _ => {
                // Already DynBox or reference type — pass through as i64.
                self.ensure_i64(operand).into()
            }
        };

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
                let masked = self.backend.builder.build_and(
                    raw,
                    i64_ty.const_int(nanbox::INT_MASK, false),
                    "payload",
                ).unwrap();
                // Sign extension: if bit 46 is set, fill upper bits
                let sign_bit = self.backend.builder.build_and(
                    raw,
                    i64_ty.const_int(nanbox::INT_SIGN_BIT, false),
                    "sign_bit",
                ).unwrap();
                let is_neg = self.backend.builder.build_int_compare(
                    inkwell::IntPredicate::NE,
                    sign_bit,
                    i64_ty.const_int(0, false),
                    "is_neg",
                ).unwrap();
                let sign_extend = i64_ty.const_int(!nanbox::INT_MASK, false);
                let extended = self.backend.builder.build_or(
                    masked,
                    sign_extend,
                    "sign_extended",
                ).unwrap();
                let extended_basic: inkwell::values::BasicValueEnum = extended.into();
                let masked_basic: inkwell::values::BasicValueEnum = masked.into();
                let val = self.backend.builder.build_select(
                    is_neg, extended_basic, masked_basic, "unbox_i64",
                ).unwrap();
                val
            }
            TirType::F64 => {
                // Bitcast i64 back to f64.
                let f64_ty = self.backend.context.f64_type();
                self.backend.builder.build_bit_cast(raw, f64_ty, "unbox_f64").unwrap()
            }
            TirType::Bool => {
                // Extract lowest bit
                let one = i64_ty.const_int(1, false);
                let bit = self.backend.builder.build_and(raw, one, "bool_bit").unwrap();
                let bool_val = self.backend.builder.build_int_truncate(
                    bit,
                    self.backend.context.bool_type(),
                    "unbox_bool",
                ).unwrap();
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
                self.backend.builder.build_unconditional_branch(target_bb).unwrap();
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let cond_val = self.resolve(*cond);
                let cond_ty = self.value_types.get(cond).cloned().unwrap_or(TirType::DynBox);

                // Convert condition to i1.
                let cond_i1 = match &cond_ty {
                    TirType::Bool => cond_val.into_int_value(),
                    TirType::I64 => {
                        self.backend.builder.build_int_compare(
                            inkwell::IntPredicate::NE,
                            cond_val.into_int_value(),
                            self.backend.context.i64_type().const_int(0, false),
                            "cond_i1",
                        ).unwrap()
                    }
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
                        self.backend.builder.build_int_compare(
                            inkwell::IntPredicate::NE,
                            result.into_int_value(),
                            self.backend.context.i64_type().const_int(0, false),
                            "cond_i1",
                        ).unwrap()
                    }
                };

                let then_bb = self.block_map[then_block];
                let else_bb = self.block_map[else_block];

                self.record_branch_args(*then_block, then_args);
                self.record_branch_args(*else_block, else_args);

                let branch_inst = self.backend
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

                let switch_cases: Vec<_> = cases
                    .iter()
                    .map(|(case_val, target, args)| {
                        let case_const = self
                            .backend
                            .context
                            .i64_type()
                            .const_int(*case_val as u64, *case_val < 0);
                        self.record_branch_args(*target, args);
                        (case_const, self.block_map[target])
                    })
                    .collect();

                self.backend
                    .builder
                    .build_switch(switch_int, default_bb, &switch_cases)
                    .unwrap();
            }
            Terminator::Return { values } => {
                if values.is_empty() {
                    // Return void-equivalent (None sentinel for Python functions)
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let ret_val = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false);
                    self.backend.builder.build_return(Some(&ret_val)).unwrap();
                } else if values.len() == 1 {
                    let val = self.resolve(values[0]);
                    self.backend.builder.build_return(Some(&val)).unwrap();
                } else {
                    // Multi-value return: pack into struct.
                    // For now, just return the first value.
                    let val = self.resolve(values[0]);
                    self.backend.builder.build_return(Some(&val)).unwrap();
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
    fn finalize_phis(&mut self) {
        // Collect phi info first to avoid borrow conflicts.
        let phi_info: Vec<_> = self.pending_phis.iter().map(|(bid, idx, phi)| {
            (*bid, *idx, phi.as_basic_value().get_type(), phi.clone())
        }).collect();

        for (block_id, arg_index, phi_ty, phi) in &phi_info {
            let _block = self.func.blocks.get(block_id).unwrap();
            // Find all predecessors that branch to this block.
            for (pred_id, pred_block) in &self.func.blocks {
                let branch_args = self.get_branch_args_to(&pred_block.terminator, *block_id);
                if let Some(args) = branch_args {
                    if *arg_index < args.len() {
                        let val_id = args[*arg_index];
                        if let Some(val) = self.values.get(&val_id) {
                            let pred_bb = self.block_map[pred_id];
                            // Coerce value to phi type if they differ (e.g. i1 -> i64).
                            let coerced = self.coerce_to_type(*val, phi_ty.clone(), pred_bb);
                            phi.add_incoming(&[(&coerced, pred_bb)]);
                        }
                    }
                }
            }
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
            (BasicValueEnum::FloatValue(fv), inkwell::types::BasicTypeEnum::IntType(target_int)) => {
                self.backend
                    .builder
                    .build_bit_cast(fv, target_int, "phi_f2i")
                    .unwrap()
            }
            // i64 -> f64: bitcast
            (BasicValueEnum::IntValue(iv), inkwell::types::BasicTypeEnum::FloatType(target_float)) => {
                self.backend
                    .builder
                    .build_bit_cast(iv, target_float, "phi_i2f")
                    .unwrap()
            }
            // Fallback: pass through (may cause verification warning)
            _ => val,
        };
        // Restore builder position.
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
    fn resolve(&self, id: ValueId) -> BasicValueEnum<'ctx> {
        *self.values.get(&id).unwrap_or_else(|| {
            panic!(
                "ValueId %{} not found in lowered values — possible use-before-def",
                id.0
            )
        })
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
            BasicValueEnum::FloatValue(fv) => {
                self.backend
                    .builder
                    .build_bit_cast(fv, i64_ty, "f2i")
                    .unwrap()
                    .into_int_value()
            }
            BasicValueEnum::PointerValue(pv) => {
                self.backend
                    .builder
                    .build_ptr_to_int(pv, i64_ty, "ptr2i")
                    .unwrap()
            }
            _ => panic!("Cannot convert {:?} to i64", val),
        }
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
            let ptr_ty = self.backend.context.ptr_type(inkwell::AddressSpace::default());
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
            self.backend.context.i8_type().array_type(name_bytes.len() as u32),
            None,
            &format!("__attr_str_{}", s.replace(|c: char| !c.is_alphanumeric(), "_")),
        );
        global.set_initializer(
            &self.backend.context.const_string(name_bytes, false),
        );
        global.set_constant(true);
        global.set_unnamed_addr(true);
        let ptr = global.as_pointer_value();
        let len = i64_ty.const_int(name_bytes.len() as u64, false);
        let out_alloca = self.backend.builder.build_alloca(i64_ty, "intern_out").unwrap();
        self.backend
            .builder
            .build_call(sfb_fn, &[ptr.into(), len.into(), out_alloca.into()], "intern_sfb")
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
    use crate::llvm_backend::runtime_imports::declare_runtime_functions;
    use crate::llvm_backend::LlvmBackend;
    use crate::tir::blocks::{BlockId, TirBlock, Terminator};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};
    use inkwell::context::Context;

    fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
        let backend = LlvmBackend::new(ctx, "test");
        declare_runtime_functions(ctx, &backend.module);
        backend
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
        let ir = backend.dump_ir();

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
        let ir = backend.dump_ir();

        // Should contain a native `add` instruction, NOT a call to molt_add
        assert!(ir.contains("add i64"), "expected native i64 add in IR: {}", ir);
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
        let ir = backend.dump_ir();

        assert!(ir.contains("fadd double"), "expected native f64 add in IR: {}", ir);
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
        let ir = backend.dump_ir();

        assert!(ir.contains("molt_add"), "expected runtime call to molt_add in IR: {}", ir);
    }

    #[test]
    fn lower_conditional_branch() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn cond(flag: Bool) -> i64 { if flag: return 1 else: return 0 }
        let mut func = TirFunction::new(
            "cond_branch".into(),
            vec![TirType::Bool],
            TirType::I64,
        );

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
        let ir = backend.dump_ir();

        // Should have 3 blocks and a conditional branch
        assert!(ir.contains("br i1"), "expected conditional branch in IR: {}", ir);
        assert!(ir.contains("bb1"), "expected then block in IR: {}", ir);
        assert!(ir.contains("bb2"), "expected else block in IR: {}", ir);
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
        let ir = backend.dump_ir();

        assert!(ir.contains("icmp slt"), "expected signed less-than comparison in IR: {}", ir);
    }

    #[test]
    fn lower_box_i64() {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        // Build: fn box_it(x: i64) -> DynBox { return box(x) }
        let mut func = TirFunction::new(
            "box_i64".into(),
            vec![TirType::I64],
            TirType::DynBox,
        );
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
        let ir = backend.dump_ir();

        // Should contain the NaN-boxing OR operations
        assert!(ir.contains("or i64"), "expected NaN-boxing OR in IR: {}", ir);
        assert!(ir.contains("and i64"), "expected NaN-boxing AND mask in IR: {}", ir);
    }
}
