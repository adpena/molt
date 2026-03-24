//! TIR → WASM type-specialized lowering.
//!
//! Converts a [`TirFunction`] into WASM instructions using the `wasm-encoder` crate.
//! The key insight: TIR carries refined type information from optimization passes,
//! so we can emit **native WASM arithmetic** for unboxed scalars instead of falling
//! back to runtime dispatch calls for every operation.
//!
//! ## Type mapping
//!
//! | TirType     | WASM ValType | Notes                          |
//! |-------------|-------------|--------------------------------|
//! | I64         | i64         | Native 64-bit integer          |
//! | F64         | f64         | Native 64-bit float            |
//! | Bool        | i32         | 0 or 1                         |
//! | None        | i64         | Sentinel constant              |
//! | DynBox      | i64         | NaN-boxed runtime value        |
//! | Str/List/…  | i64         | Heap pointer as i64            |
//!
//! ## SSA → stack machine
//!
//! TIR is register-based SSA; WASM is a stack machine. We allocate one WASM local
//! per SSA value and emit explicit local.get/local.set around each operation.
//! A later peephole pass (not in this module) could eliminate redundant get/set pairs.

#[cfg(feature = "wasm-backend")]
use wasm_encoder::{Ieee64, Instruction, ValType};

#[cfg(feature = "wasm-backend")]
use std::collections::HashMap;

#[cfg(feature = "wasm-backend")]
use super::blocks::{BlockId, Terminator};
#[cfg(feature = "wasm-backend")]
use super::function::TirFunction;
#[cfg(feature = "wasm-backend")]
use super::ops::{AttrValue, OpCode};
#[cfg(feature = "wasm-backend")]
use super::types::TirType;
#[cfg(feature = "wasm-backend")]
use super::values::ValueId;

// ---------------------------------------------------------------------------
// Output struct
// ---------------------------------------------------------------------------

/// The result of lowering a single TIR function to WASM.
#[cfg(feature = "wasm-backend")]
#[derive(Debug)]
pub struct WasmFunctionOutput {
    /// WASM parameter types.
    pub param_types: Vec<ValType>,
    /// WASM result types.
    pub result_types: Vec<ValType>,
    /// Local variable types (excludes parameters).
    pub locals: Vec<ValType>,
    /// WASM instruction sequence (function body).
    pub instructions: Vec<Instruction<'static>>,
}

// ---------------------------------------------------------------------------
// Type mapping
// ---------------------------------------------------------------------------

/// Map a TIR type to its WASM representation.
#[cfg(feature = "wasm-backend")]
fn tir_type_to_val(ty: &TirType) -> ValType {
    match ty {
        TirType::I64 => ValType::I64,
        TirType::F64 => ValType::F64,
        TirType::Bool => ValType::I32,
        // Everything else is represented as i64 (NaN-boxed or heap pointer).
        _ => ValType::I64,
    }
}

// ---------------------------------------------------------------------------
// Lowering context
// ---------------------------------------------------------------------------

/// Internal state for the lowering pass.
#[cfg(feature = "wasm-backend")]
struct LowerCtx<'a> {
    func: &'a TirFunction,
    /// Map SSA ValueId → WASM local index.
    value_locals: HashMap<ValueId, u32>,
    /// Map SSA ValueId → its TIR type (for type-specialized emission).
    value_types: HashMap<ValueId, TirType>,
    /// Total number of WASM locals allocated so far (params + locals).
    next_local: u32,
    /// Emitted instructions.
    instructions: Vec<Instruction<'static>>,
    /// Block ordering (reverse post-order).
    rpo: Vec<BlockId>,
    /// Map BlockId → index in `rpo` (used for branch targets).
    block_index: HashMap<BlockId, usize>,
}

#[cfg(feature = "wasm-backend")]
impl<'a> LowerCtx<'a> {
    fn new(func: &'a TirFunction) -> Self {
        let rpo = compute_rpo(func);
        let block_index: HashMap<BlockId, usize> = rpo
            .iter()
            .enumerate()
            .map(|(i, &bid)| (bid, i))
            .collect();

        Self {
            func,
            value_locals: HashMap::new(),
            value_types: HashMap::new(),
            next_local: 0,
            instructions: Vec::new(),
            rpo,
            block_index,
        }
    }

    /// Ensure an SSA value has a WASM local allocated; return its index.
    fn local_for(&mut self, vid: ValueId, ty: &TirType) -> u32 {
        if let Some(&idx) = self.value_locals.get(&vid) {
            return idx;
        }
        let idx = self.next_local;
        self.next_local += 1;
        self.value_locals.insert(vid, idx);
        self.value_types.insert(vid, ty.clone());
        idx
    }

    /// Look up the local index for an already-allocated value.
    fn get_local(&self, vid: ValueId) -> u32 {
        self.value_locals[&vid]
    }

    /// Emit a `local.get` for the given SSA value.
    fn emit_get(&mut self, vid: ValueId) {
        let idx = self.get_local(vid);
        self.instructions.push(Instruction::LocalGet(idx));
    }

    /// Emit a `local.set` for the given SSA value.
    fn emit_set(&mut self, vid: ValueId) {
        let idx = self.get_local(vid);
        self.instructions.push(Instruction::LocalSet(idx));
    }

    /// Get the TIR type of a value (defaults to DynBox if unknown).
    fn type_of(&self, vid: ValueId) -> TirType {
        self.value_types.get(&vid).cloned().unwrap_or(TirType::DynBox)
    }
}

// ---------------------------------------------------------------------------
// RPO computation
// ---------------------------------------------------------------------------

/// Compute reverse post-order of the CFG for structured WASM emission.
#[cfg(feature = "wasm-backend")]
fn compute_rpo(func: &TirFunction) -> Vec<BlockId> {
    let mut visited = HashMap::new();
    let mut order = Vec::new();
    rpo_visit(func, func.entry_block, &mut visited, &mut order);
    order.reverse();
    order
}

#[cfg(feature = "wasm-backend")]
fn rpo_visit(
    func: &TirFunction,
    block_id: BlockId,
    visited: &mut HashMap<BlockId, bool>,
    order: &mut Vec<BlockId>,
) {
    if visited.contains_key(&block_id) {
        return;
    }
    visited.insert(block_id, true);

    if let Some(block) = func.blocks.get(&block_id) {
        match &block.terminator {
            Terminator::Branch { target, .. } => {
                rpo_visit(func, *target, visited, order);
            }
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                rpo_visit(func, *then_block, visited, order);
                rpo_visit(func, *else_block, visited, order);
            }
            Terminator::Switch {
                cases, default, ..
            } => {
                for (_, target, _) in cases {
                    rpo_visit(func, *target, visited, order);
                }
                rpo_visit(func, *default, visited, order);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    order.push(block_id);
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` → `wasm i64`, `F64` → `wasm f64`, `DynBox` → runtime call.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm(func: &TirFunction) -> WasmFunctionOutput {
    let mut ctx = LowerCtx::new(func);

    // --- Allocate locals for parameters (entry block args). ---
    let param_types: Vec<ValType> = func.param_types.iter().map(tir_type_to_val).collect();
    let result_types: Vec<ValType> = vec![tir_type_to_val(&func.return_type)];

    if let Some(entry) = func.blocks.get(&func.entry_block) {
        for arg in &entry.args {
            ctx.local_for(arg.id, &arg.ty);
        }
    }

    // --- Pre-scan: allocate locals for every SSA result in every block. ---
    for &bid in &ctx.rpo.clone() {
        if let Some(block) = func.blocks.get(&bid) {
            // Block arguments (for non-entry blocks).
            for arg in &block.args {
                ctx.local_for(arg.id, &arg.ty);
            }
            // Op results.
            for op in &block.ops {
                for &result_id in &op.results {
                    // Infer result type from the op (simplified heuristic).
                    let ty = infer_result_type(op, &ctx);
                    ctx.local_for(result_id, &ty);
                }
            }
        }
    }

    // --- Compute non-parameter locals. ---
    let num_params = func.param_types.len() as u32;
    let total_locals = ctx.next_local;
    let mut locals = Vec::new();
    for idx in num_params..total_locals {
        // Find the type for this local.
        let ty = ctx
            .value_locals
            .iter()
            .find(|&(_, &local_idx)| local_idx == idx)
            .and_then(|(vid, _)| ctx.value_types.get(vid))
            .map(tir_type_to_val)
            .unwrap_or(ValType::I64);
        locals.push(ty);
    }

    // --- Emit blocks. ---
    // Simple strategy: for a single-block function, emit ops inline.
    // For multi-block, use WASM block/loop/br structure.
    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();

    if num_blocks <= 1 {
        // Single block — emit ops directly, no control flow.
        if let Some(block) = func.blocks.get(&func.entry_block) {
            emit_block_ops(&mut ctx, block);
            emit_terminator(&mut ctx, &block.terminator);
        }
    } else {
        // Multi-block: wrap each block in a WASM `block` and use `br` for jumps.
        // Strategy: nest blocks so that forward branches can target outer blocks.
        //
        //   block $b0
        //     block $b1
        //       block $b2
        //         ... entry ops ...
        //         br $bN  (branch to block N)
        //       end
        //       ... block 2 ops ...
        //     end
        //     ... block 1 ops ...
        //   end
        //   ... block 0 (last in RPO) ops ...
        //
        // For forward-only CFGs this works well. Back-edges (loops) would need
        // `loop` blocks, which we handle below.

        // Detect back-edges (target block appears before source in RPO).
        let back_edge_targets: HashMap<BlockId, bool> = {
            let mut targets = HashMap::new();
            for (src_idx, &bid) in rpo.iter().enumerate() {
                if let Some(block) = func.blocks.get(&bid) {
                    for succ in terminator_successors(&block.terminator) {
                        if let Some(&tgt_idx) = ctx.block_index.get(&succ) {
                            if tgt_idx <= src_idx {
                                targets.insert(succ, true);
                            }
                        }
                    }
                }
            }
            targets
        };

        // Open nested blocks/loops for all but the last RPO block.
        // Block at RPO index i can be targeted by `br (num_blocks - 1 - i)`.
        for (i, &bid) in rpo.iter().enumerate() {
            if i < num_blocks - 1 {
                if back_edge_targets.contains_key(&bid) {
                    ctx.instructions.push(Instruction::Loop(
                        wasm_encoder::BlockType::Empty,
                    ));
                } else {
                    ctx.instructions.push(Instruction::Block(
                        wasm_encoder::BlockType::Empty,
                    ));
                }
            }
        }

        // Emit each block's ops + terminator.
        for (i, &bid) in rpo.iter().enumerate() {
            if let Some(block) = func.blocks.get(&bid) {
                emit_block_ops(&mut ctx, block);
                emit_terminator_multiblock(&mut ctx, &block.terminator, num_blocks);
            }
            // Close the block (all but last have an open block/loop).
            if i < num_blocks - 1 {
                ctx.instructions.push(Instruction::End);
            }
        }
    }

    // Final `end` for the function body.
    ctx.instructions.push(Instruction::End);

    WasmFunctionOutput {
        param_types,
        result_types,
        locals,
        instructions: ctx.instructions,
    }
}

// ---------------------------------------------------------------------------
// Op emission
// ---------------------------------------------------------------------------

/// Emit WASM instructions for all ops in a block.
#[cfg(feature = "wasm-backend")]
fn emit_block_ops(ctx: &mut LowerCtx, block: &super::blocks::TirBlock) {
    // Copy block args into their locals (for non-entry blocks, the
    // branch source should have already stored values — block args
    // are handled at the branch site via local.set).
    for op in &block.ops {
        emit_op(ctx, op);
    }
}

/// Emit WASM instructions for a single TIR operation.
#[cfg(feature = "wasm-backend")]
fn emit_op(ctx: &mut LowerCtx, op: &super::ops::TirOp) {
    match op.opcode {
        // --- Constants ---
        OpCode::ConstInt => {
            let val = match op.attrs.get("value") {
                Some(AttrValue::Int(v)) => *v,
                _ => 0,
            };
            if let Some(&result) = op.results.first() {
                let ty = ctx.type_of(result);
                match ty {
                    TirType::F64 => {
                        ctx.instructions.push(Instruction::F64Const(Ieee64::from(val as f64)));
                    }
                    _ => {
                        ctx.instructions.push(Instruction::I64Const(val));
                    }
                }
                ctx.emit_set(result);
            }
        }
        OpCode::ConstFloat => {
            let val = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
                Some(AttrValue::Float(v)) => *v,
                _ => 0.0,
            };
            if let Some(&result) = op.results.first() {
                ctx.instructions.push(Instruction::F64Const(Ieee64::from(val)));
                ctx.emit_set(result);
            }
        }
        OpCode::ConstBool => {
            let val = match op.attrs.get("value") {
                Some(AttrValue::Bool(v)) => *v,
                _ => false,
            };
            if let Some(&result) = op.results.first() {
                ctx.instructions
                    .push(Instruction::I32Const(if val { 1 } else { 0 }));
                ctx.emit_set(result);
            }
        }
        OpCode::ConstNone => {
            if let Some(&result) = op.results.first() {
                // None is represented as a sentinel i64 constant.
                // NaN-boxed None: QNAN | TAG_NONE
                const QNAN: u64 = 0x7ff8_0000_0000_0000;
                const TAG_NONE: u64 = 0x0003_0000_0000_0000;
                ctx.instructions
                    .push(Instruction::I64Const((QNAN | TAG_NONE) as i64));
                ctx.emit_set(result);
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            // String/bytes constants need runtime support (heap allocation).
            // For now emit a placeholder i64 constant 0 — the integration layer
            // will need to patch these with actual string table offsets.
            if let Some(&result) = op.results.first() {
                ctx.instructions.push(Instruction::I64Const(0));
                ctx.emit_set(result);
            }
        }

        // --- Arithmetic (type-specialized) ---
        OpCode::Add => emit_binary_arith(ctx, op, ArithOp::Add),
        OpCode::Sub => emit_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul => emit_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Neg => emit_unary_arith(ctx, op, UnaryOp::Neg),
        OpCode::Pos => {
            // Pos is identity for numeric types.
            if let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) {
                ctx.emit_get(src);
                ctx.emit_set(dst);
            }
        }

        // --- Comparison (type-specialized) ---
        OpCode::Eq => emit_comparison(ctx, op, CmpOp::Eq),
        OpCode::Ne => emit_comparison(ctx, op, CmpOp::Ne),
        OpCode::Lt => emit_comparison(ctx, op, CmpOp::Lt),
        OpCode::Le => emit_comparison(ctx, op, CmpOp::Le),
        OpCode::Gt => emit_comparison(ctx, op, CmpOp::Gt),
        OpCode::Ge => emit_comparison(ctx, op, CmpOp::Ge),

        // --- Bitwise ---
        OpCode::BitAnd => emit_bitwise(ctx, op, BitwiseOp::And),
        OpCode::BitOr => emit_bitwise(ctx, op, BitwiseOp::Or),
        OpCode::BitXor => emit_bitwise(ctx, op, BitwiseOp::Xor),
        OpCode::BitNot => {
            if let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) {
                ctx.emit_get(src);
                ctx.instructions.push(Instruction::I64Const(-1));
                ctx.instructions.push(Instruction::I64Xor);
                ctx.emit_set(dst);
            }
        }
        OpCode::Shl => {
            if op.operands.len() >= 2 {
                if let Some(&dst) = op.results.first() {
                    ctx.emit_get(op.operands[0]);
                    ctx.emit_get(op.operands[1]);
                    ctx.instructions.push(Instruction::I64Shl);
                    ctx.emit_set(dst);
                }
            }
        }
        OpCode::Shr => {
            if op.operands.len() >= 2 {
                if let Some(&dst) = op.results.first() {
                    ctx.emit_get(op.operands[0]);
                    ctx.emit_get(op.operands[1]);
                    ctx.instructions.push(Instruction::I64ShrS);
                    ctx.emit_set(dst);
                }
            }
        }

        // --- Boolean ---
        OpCode::Not => {
            if let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) {
                ctx.emit_get(src);
                ctx.instructions.push(Instruction::I32Eqz);
                ctx.emit_set(dst);
            }
        }
        OpCode::And | OpCode::Or => {
            // Short-circuit boolean ops — for typed booleans, just use bitwise.
            if op.operands.len() >= 2 {
                if let Some(&dst) = op.results.first() {
                    ctx.emit_get(op.operands[0]);
                    ctx.emit_get(op.operands[1]);
                    let instr = if op.opcode == OpCode::And {
                        Instruction::I32And
                    } else {
                        Instruction::I32Or
                    };
                    ctx.instructions.push(instr);
                    ctx.emit_set(dst);
                }
            }
        }

        // --- SSA Copy ---
        OpCode::Copy => {
            if let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) {
                ctx.emit_get(src);
                ctx.emit_set(dst);
            }
        }

        // --- Box/Unbox ---
        OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            // Boxing/unboxing requires NaN-boxing logic. For now, treat as a copy
            // since the runtime representation is the same width (i64).
            if let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) {
                ctx.emit_get(src);
                ctx.emit_set(dst);
            }
        }

        // --- Refcount (no-op in WASM — GC is external) ---
        OpCode::IncRef | OpCode::DecRef => {}

        // --- Calls ---
        OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin => {
            // Calls require function index resolution from the module context.
            // Emit a placeholder: push all operands, call index 0, store result.
            // The integration layer will patch the call target.
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            // Placeholder call index — will be patched during module assembly.
            let func_idx = match op.attrs.get("func_index") {
                Some(AttrValue::Int(v)) => *v as u32,
                _ => 0,
            };
            ctx.instructions.push(Instruction::Call(func_idx));
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Container builders ---
        OpCode::BuildList | OpCode::BuildDict | OpCode::BuildTuple
        | OpCode::BuildSet | OpCode::BuildSlice => {
            // Container construction requires runtime calls.
            // Push operand count + operands, call runtime builder.
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            // Placeholder — runtime will handle.
            ctx.instructions
                .push(Instruction::I64Const(op.operands.len() as i64));
            ctx.instructions.push(Instruction::Call(0)); // patched later
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Memory ops (attribute access, indexing) ---
        OpCode::LoadAttr | OpCode::StoreAttr | OpCode::DelAttr
        | OpCode::Index | OpCode::StoreIndex | OpCode::DelIndex => {
            // These require runtime dispatch. Emit operands + call placeholder.
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0)); // patched later
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Allocation ---
        OpCode::Alloc | OpCode::StackAlloc | OpCode::Free => {
            // Runtime memory management — placeholder.
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0)); // patched later
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Iteration ---
        OpCode::GetIter | OpCode::IterNext | OpCode::ForIter => {
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0)); // patched later
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Import ---
        OpCode::Import | OpCode::ImportFrom => {
            for &operand in &op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0)); // patched later
            if let Some(&result) = op.results.first() {
                ctx.emit_set(result);
            }
        }

        // --- Pow (runtime call even for numeric — no native WASM pow) ---
        OpCode::Pow => {
            if op.operands.len() >= 2 {
                ctx.emit_get(op.operands[0]);
                ctx.emit_get(op.operands[1]);
                ctx.instructions.push(Instruction::Call(0)); // $molt_pow
                if let Some(&result) = op.results.first() {
                    ctx.emit_set(result);
                }
            }
        }

        // --- Identity / containment checks (runtime) ---
        OpCode::Is | OpCode::IsNot | OpCode::In | OpCode::NotIn => {
            if op.operands.len() >= 2 {
                ctx.emit_get(op.operands[0]);
                ctx.emit_get(op.operands[1]);
                ctx.instructions.push(Instruction::Call(0)); // runtime
                if let Some(&result) = op.results.first() {
                    ctx.emit_set(result);
                }
            }
        }

        // --- Exception / Generator / SCF / Deopt — emit runtime calls ---
        OpCode::Raise => {
            // Call molt_raise runtime function
            if let Some(&func_idx) = ctx.func_indices.get("molt_raise") {
                for &operand in &op.operands {
                    ctx.emit_local_get(operand);
                }
                ctx.instructions.push(Instruction::Call(func_idx));
            }
            ctx.instructions.push(Instruction::Unreachable);
        }
        OpCode::CheckException => {
            // Call molt_check_exception runtime function
            if let Some(&func_idx) = ctx.func_indices.get("molt_check_exception") {
                ctx.instructions.push(Instruction::Call(func_idx));
            }
            if let Some(&result) = op.results.first() {
                ctx.emit_local_set(result);
            }
        }
        OpCode::Yield | OpCode::YieldFrom => {
            // Generator yield: emit runtime call to molt_yield
            let fn_name = if op.opcode == OpCode::Yield { "molt_yield" } else { "molt_yield_from" };
            if let Some(&func_idx) = ctx.func_indices.get(fn_name) {
                for &operand in &op.operands {
                    ctx.emit_local_get(operand);
                }
                ctx.instructions.push(Instruction::Call(func_idx));
            }
            if let Some(&result) = op.results.first() {
                ctx.emit_local_set(result);
            }
        }
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => {
            // SCF ops should be lowered to block/loop/br before reaching WASM emission.
            // If they reach here, emit a nop (they were already handled by block structure).
        }
        OpCode::Deopt => {
            // Deoptimization: call molt_deopt_transfer and unreachable
            if let Some(&func_idx) = ctx.func_indices.get("molt_deopt_transfer") {
                for &operand in &op.operands {
                    ctx.emit_local_get(operand);
                }
                ctx.instructions.push(Instruction::Call(func_idx));
            }
            ctx.instructions.push(Instruction::Unreachable);
        }
    }
}

// ---------------------------------------------------------------------------
// Arithmetic helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-backend")]
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
}

#[cfg(feature = "wasm-backend")]
#[derive(Clone, Copy)]
enum UnaryOp {
    Neg,
}

#[cfg(feature = "wasm-backend")]
#[derive(Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[cfg(feature = "wasm-backend")]
#[derive(Clone, Copy)]
enum BitwiseOp {
    And,
    Or,
    Xor,
}

/// Emit a binary arithmetic operation, type-specialized.
#[cfg(feature = "wasm-backend")]
fn emit_binary_arith(ctx: &mut LowerCtx, op: &super::ops::TirOp, arith: ArithOp) {
    if op.operands.len() < 2 || op.results.is_empty() {
        return;
    }
    let lhs = op.operands[0];
    let rhs = op.operands[1];
    let dst = op.results[0];
    let lhs_ty = ctx.type_of(lhs);
    let rhs_ty = ctx.type_of(rhs);

    ctx.emit_get(lhs);
    ctx.emit_get(rhs);

    match (&lhs_ty, &rhs_ty) {
        (TirType::I64, TirType::I64) => {
            ctx.instructions.push(match arith {
                ArithOp::Add => Instruction::I64Add,
                ArithOp::Sub => Instruction::I64Sub,
                ArithOp::Mul => Instruction::I64Mul,
                ArithOp::Div | ArithOp::FloorDiv => Instruction::I64DivS,
                ArithOp::Mod => Instruction::I64RemS,
            });
        }
        (TirType::F64, TirType::F64) => {
            ctx.instructions.push(match arith {
                ArithOp::Add => Instruction::F64Add,
                ArithOp::Sub => Instruction::F64Sub,
                ArithOp::Mul => Instruction::F64Mul,
                ArithOp::Div | ArithOp::FloorDiv => Instruction::F64Div,
                ArithOp::Mod => {
                    // WASM has no f64.rem — need runtime call.
                    // Drop the two operands already on the stack and re-emit as call.
                    ctx.instructions.push(Instruction::Call(0)); // $molt_fmod
                    ctx.emit_set(dst);
                    return;
                }
            });
        }
        _ => {
            // DynBox or mixed types — fall back to runtime dispatch.
            ctx.instructions.push(Instruction::Call(0)); // $molt_arith, patched later
            ctx.emit_set(dst);
            return;
        }
    }

    ctx.emit_set(dst);
}

/// Emit a unary arithmetic operation.
#[cfg(feature = "wasm-backend")]
fn emit_unary_arith(ctx: &mut LowerCtx, op: &super::ops::TirOp, _unary: UnaryOp) {
    if op.operands.is_empty() || op.results.is_empty() {
        return;
    }
    let src = op.operands[0];
    let dst = op.results[0];
    let ty = ctx.type_of(src);

    match ty {
        TirType::I64 => {
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Sub);
        }
        TirType::F64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::F64Neg);
        }
        _ => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::Call(0)); // $molt_neg
            ctx.emit_set(dst);
            return;
        }
    }

    ctx.emit_set(dst);
}

/// Emit a comparison, type-specialized.
#[cfg(feature = "wasm-backend")]
fn emit_comparison(ctx: &mut LowerCtx, op: &super::ops::TirOp, cmp: CmpOp) {
    if op.operands.len() < 2 || op.results.is_empty() {
        return;
    }
    let lhs = op.operands[0];
    let rhs = op.operands[1];
    let dst = op.results[0];
    let lhs_ty = ctx.type_of(lhs);
    let rhs_ty = ctx.type_of(rhs);

    ctx.emit_get(lhs);
    ctx.emit_get(rhs);

    match (&lhs_ty, &rhs_ty) {
        (TirType::I64, TirType::I64) => {
            ctx.instructions.push(match cmp {
                CmpOp::Eq => Instruction::I64Eq,
                CmpOp::Ne => Instruction::I64Ne,
                CmpOp::Lt => Instruction::I64LtS,
                CmpOp::Le => Instruction::I64LeS,
                CmpOp::Gt => Instruction::I64GtS,
                CmpOp::Ge => Instruction::I64GeS,
            });
        }
        (TirType::F64, TirType::F64) => {
            ctx.instructions.push(match cmp {
                CmpOp::Eq => Instruction::F64Eq,
                CmpOp::Ne => Instruction::F64Ne,
                CmpOp::Lt => Instruction::F64Lt,
                CmpOp::Le => Instruction::F64Le,
                CmpOp::Gt => Instruction::F64Gt,
                CmpOp::Ge => Instruction::F64Ge,
            });
        }
        _ => {
            ctx.instructions.push(Instruction::Call(0)); // runtime cmp
            ctx.emit_set(dst);
            return;
        }
    }

    ctx.emit_set(dst);
}

/// Emit a bitwise operation (always i64).
#[cfg(feature = "wasm-backend")]
fn emit_bitwise(ctx: &mut LowerCtx, op: &super::ops::TirOp, bw: BitwiseOp) {
    if op.operands.len() < 2 || op.results.is_empty() {
        return;
    }
    ctx.emit_get(op.operands[0]);
    ctx.emit_get(op.operands[1]);
    ctx.instructions.push(match bw {
        BitwiseOp::And => Instruction::I64And,
        BitwiseOp::Or => Instruction::I64Or,
        BitwiseOp::Xor => Instruction::I64Xor,
    });
    ctx.emit_set(op.results[0]);
}

// ---------------------------------------------------------------------------
// Terminator emission
// ---------------------------------------------------------------------------

/// Emit a terminator for a single-block function.
#[cfg(feature = "wasm-backend")]
fn emit_terminator(ctx: &mut LowerCtx, term: &Terminator) {
    match term {
        Terminator::Return { values } => {
            if let Some(&val) = values.first() {
                ctx.emit_get(val);
            }
            ctx.instructions.push(Instruction::Return);
        }
        Terminator::Unreachable => {
            ctx.instructions.push(Instruction::Unreachable);
        }
        _ => {
            // Single-block function shouldn't have branches, but handle gracefully.
            ctx.instructions.push(Instruction::Unreachable);
        }
    }
}

/// Emit a terminator for multi-block functions using WASM structured control flow.
#[cfg(feature = "wasm-backend")]
fn emit_terminator_multiblock(ctx: &mut LowerCtx, term: &Terminator, num_blocks: usize) {
    match term {
        Terminator::Return { values } => {
            if let Some(&val) = values.first() {
                ctx.emit_get(val);
            }
            ctx.instructions.push(Instruction::Return);
        }
        Terminator::Unreachable => {
            ctx.instructions.push(Instruction::Unreachable);
        }
        Terminator::Branch { target, args } => {
            // Store block args into the target block's argument locals.
            store_block_args(ctx, *target, args);
            // Compute branch depth: target is at rpo index `tgt_idx`.
            // We're inside nested blocks; the depth to reach block at index i
            // from the current nesting level is: current nesting depth - i.
            if let Some(&tgt_idx) = ctx.block_index.get(target) {
                // The outermost open block is index 0, innermost is num_blocks-2.
                // Block at RPO index i corresponds to nesting depth (num_blocks - 1 - i) - 1.
                // Actually with our structure: block at RPO[i] has its `block`/`loop`
                // instruction at nesting position i (0-indexed from outermost).
                // From inside the innermost position, to branch to block i we need
                // depth = (num_blocks - 2) - i.
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            // Emit: if cond, branch to then_block; else branch to else_block.
            let cond_ty = ctx.type_of(*cond);

            // Convert condition to i32 for br_if.
            match cond_ty {
                TirType::Bool => {
                    ctx.emit_get(*cond);
                }
                TirType::I64 => {
                    // i64 → i32 (nonzero = true).
                    ctx.emit_get(*cond);
                    ctx.instructions.push(Instruction::I64Const(0));
                    ctx.instructions.push(Instruction::I64Ne);
                }
                _ => {
                    // DynBox — wrap to i32 (nonzero = true).
                    ctx.emit_get(*cond);
                    ctx.instructions.push(Instruction::I64Const(0));
                    ctx.instructions.push(Instruction::I64Ne);
                }
            }

            // Store then-args, then br_if to then_block.
            // If not taken, fall through to else branch.
            store_block_args(ctx, *then_block, then_args);

            if let Some(&tgt_idx) = ctx.block_index.get(then_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::BrIf(depth as u32));
            }

            // Else path: store else-args and branch.
            store_block_args(ctx, *else_block, else_args);
            if let Some(&tgt_idx) = ctx.block_index.get(else_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            // Emit a br_table for switch.
            // For now, fall back to a chain of if/else.
            for (case_val, target, args) in cases {
                ctx.emit_get(*value);
                ctx.instructions.push(Instruction::I64Const(*case_val));
                ctx.instructions.push(Instruction::I64Eq);
                store_block_args(ctx, *target, args);
                if let Some(&tgt_idx) = ctx.block_index.get(target) {
                    let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                    ctx.instructions.push(Instruction::BrIf(depth as u32));
                }
            }
            // Default case.
            store_block_args(ctx, *default, default_args);
            if let Some(&tgt_idx) = ctx.block_index.get(default) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
    }
}

/// Store values into the target block's argument locals.
#[cfg(feature = "wasm-backend")]
fn store_block_args(ctx: &mut LowerCtx, target: BlockId, args: &[ValueId]) {
    if let Some(block) = ctx.func.blocks.get(&target) {
        for (arg_val, &src_val) in block.args.iter().zip(args.iter()) {
            ctx.emit_get(src_val);
            let dst_local = ctx.get_local(arg_val.id);
            ctx.instructions.push(Instruction::LocalSet(dst_local));
        }
    }
}

/// Collect successor block IDs from a terminator.
#[cfg(feature = "wasm-backend")]
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
            let mut succs: Vec<BlockId> = cases.iter().map(|(_, bid, _)| *bid).collect();
            succs.push(*default);
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

// ---------------------------------------------------------------------------
// Type inference helper
// ---------------------------------------------------------------------------

/// Infer the result type of a TIR op (simplified heuristic).
#[cfg(feature = "wasm-backend")]
fn infer_result_type(op: &super::ops::TirOp, ctx: &LowerCtx) -> TirType {
    match op.opcode {
        OpCode::ConstInt => TirType::I64,
        OpCode::ConstFloat => TirType::F64,
        OpCode::ConstBool => TirType::Bool,
        OpCode::ConstNone => TirType::None,
        OpCode::ConstStr => TirType::Str,
        OpCode::ConstBytes => TirType::Bytes,
        OpCode::Not => TirType::Bool,
        OpCode::And | OpCode::Or => TirType::Bool,
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge
        | OpCode::Is | OpCode::IsNot | OpCode::In | OpCode::NotIn => TirType::Bool,
        OpCode::Copy => {
            if let Some(&src) = op.operands.first() {
                ctx.type_of(src)
            } else {
                TirType::DynBox
            }
        }
        OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Div | OpCode::FloorDiv
        | OpCode::Mod | OpCode::Neg | OpCode::Pos | OpCode::Pow => {
            // Inherit type from first operand.
            if let Some(&src) = op.operands.first() {
                ctx.type_of(src)
            } else {
                TirType::DynBox
            }
        }
        OpCode::BoxVal | OpCode::TypeGuard => TirType::DynBox,
        OpCode::UnboxVal => {
            // Unbox result type might be in attrs.
            TirType::DynBox
        }
        _ => TirType::DynBox,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "wasm-backend")]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    /// Build a trivial function: returns a constant i64.
    fn make_const_return_func(val: i64) -> TirFunction {
        let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result_id],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(val));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };
        func
    }

    #[test]
    fn trivial_const_return() {
        let func = make_const_return_func(42);
        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, vec![]);
        assert_eq!(output.result_types, vec![ValType::I64]);

        // Should contain i64.const 42 somewhere.
        let has_const = output.instructions.iter().any(|i| {
            matches!(i, Instruction::I64Const(42))
        });
        assert!(has_const, "expected i64.const 42 in output");

        // Should end with `end`.
        assert!(matches!(
            output.instructions.last(),
            Some(Instruction::End)
        ));
    }

    #[test]
    fn add_two_i64s() {
        let mut func = TirFunction::new(
            "add_i64".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let result_id = func.fresh_value(); // ValueId(2)
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, vec![ValType::I64, ValType::I64]);

        // Should contain i64.add.
        let has_add = output.instructions.iter().any(|i| {
            matches!(i, Instruction::I64Add)
        });
        assert!(has_add, "expected i64.add instruction");
    }

    #[test]
    fn add_two_f64s() {
        let mut func = TirFunction::new(
            "add_f64".into(),
            vec![TirType::F64, TirType::F64],
            TirType::F64,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
        let has_f64_add = output.instructions.iter().any(|i| {
            matches!(i, Instruction::F64Add)
        });
        assert!(has_f64_add, "expected f64.add instruction");
    }

    #[test]
    fn conditional_branch() {
        let mut func = TirFunction::new(
            "cond_branch".into(),
            vec![TirType::Bool],
            TirType::I64,
        );

        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let ret_then = func.fresh_value();
        let ret_else = func.fresh_value();

        // Patch entry block to branch on param.
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };

        let then_block = TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_then],
            },
        };

        let else_block = TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_else],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_else],
            },
        };

        func.blocks.insert(then_id, then_block);
        func.blocks.insert(else_id, else_block);

        let output = lower_tir_to_wasm(&func);

        // Should contain br_if for the conditional branch.
        let has_br_if = output.instructions.iter().any(|i| {
            matches!(i, Instruction::BrIf(_))
        });
        assert!(has_br_if, "expected br_if instruction for conditional branch");
    }

    #[test]
    fn comparison_i64_emits_native() {
        let mut func = TirFunction::new(
            "cmp_i64".into(),
            vec![TirType::I64, TirType::I64],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Lt,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        let has_lt = output.instructions.iter().any(|i| {
            matches!(i, Instruction::I64LtS)
        });
        assert!(has_lt, "expected i64.lt_s instruction");
    }

    #[test]
    fn dynbox_add_falls_back_to_call() {
        let mut func = TirFunction::new(
            "add_dyn".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        // DynBox add should emit a Call (runtime dispatch), not i64.add.
        let has_call = output.instructions.iter().any(|i| {
            matches!(i, Instruction::Call(_))
        });
        assert!(has_call, "expected runtime call for DynBox add");

        let has_i64_add = output.instructions.iter().any(|i| {
            matches!(i, Instruction::I64Add)
        });
        assert!(!has_i64_add, "should NOT emit i64.add for DynBox operands");
    }
}
