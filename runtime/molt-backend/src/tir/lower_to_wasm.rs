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
//! A peephole pass (`peephole_set_get_to_tee`) runs after emission to collapse
//! `local.set X; local.get X` pairs into `local.tee X`, eliminating redundant
//! stack traffic.

#[cfg(feature = "wasm-backend")]
use wasm_encoder::{BlockType, Ieee64, Instruction, ValType};

#[cfg(feature = "wasm-backend")]
use std::collections::HashMap;

#[cfg(feature = "wasm-backend")]
use super::blocks::BlockId;
#[cfg(feature = "wasm-backend")]
use super::function::TirFunction;
#[cfg(feature = "wasm-backend")]
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
#[cfg(feature = "wasm-backend")]
use super::lower_to_lir::lower_function_to_lir;
#[cfg(feature = "wasm-backend")]
use super::ops::{AttrValue, OpCode};
#[cfg(feature = "wasm-backend")]
use super::values::ValueId;

#[cfg(feature = "wasm-backend")]
const QNAN: i64 = 0x7ff8_0000_0000_0000u64 as i64;
#[cfg(feature = "wasm-backend")]
const TAG_INT: i64 = 0x0001_0000_0000_0000u64 as i64;
#[cfg(feature = "wasm-backend")]
const TAG_NONE: i64 = 0x0003_0000_0000_0000u64 as i64;
#[cfg(feature = "wasm-backend")]
const INT_MASK: i64 = ((1u64 << 47) - 1) as i64;
#[cfg(feature = "wasm-backend")]
const INT_SHIFT_BITS: i64 = 17;
#[cfg(feature = "wasm-backend")]
const INLINE_INT_MIN: i64 = -(1i64 << 46);
#[cfg(feature = "wasm-backend")]
const INLINE_INT_MAX: i64 = (1i64 << 46) - 1;

// ---------------------------------------------------------------------------
// Output struct
// ---------------------------------------------------------------------------

/// The result of lowering a single TIR function to WASM.
#[cfg(feature = "wasm-backend")]
#[derive(Debug, Clone)]
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
// Main entry point
// ---------------------------------------------------------------------------

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` → `wasm i64`, `F64` → `wasm f64`, `DynBox` → runtime call.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm(func: &TirFunction) -> WasmFunctionOutput {
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm(&lir)
}

#[cfg(feature = "wasm-backend")]
fn lir_repr_to_val(repr: LirRepr) -> ValType {
    match repr {
        LirRepr::I64 => ValType::I64,
        LirRepr::F64 => ValType::F64,
        LirRepr::Bool1 => ValType::I32,
        LirRepr::DynBox => ValType::I64,
    }
}

#[cfg(feature = "wasm-backend")]
struct LirLowerCtx<'a> {
    func: &'a LirFunction,
    value_locals: HashMap<ValueId, u32>,
    value_reprs: HashMap<ValueId, LirRepr>,
    /// Reverse map: local index -> ValType. Built during allocation so the
    /// locals vector can be constructed in O(N) instead of O(N^2).
    local_types: HashMap<u32, ValType>,
    next_local: u32,
    instructions: Vec<Instruction<'static>>,
    rpo: Vec<BlockId>,
    block_index: HashMap<BlockId, usize>,
}

#[cfg(feature = "wasm-backend")]
impl<'a> LirLowerCtx<'a> {
    fn new(func: &'a LirFunction) -> Self {
        Self::new_with_local_base(func, 0)
    }

    fn new_with_local_base(func: &'a LirFunction, local_base: u32) -> Self {
        let rpo = compute_lir_rpo(func);
        let block_index = rpo.iter().enumerate().map(|(i, &bid)| (bid, i)).collect();
        Self {
            func,
            value_locals: HashMap::new(),
            value_reprs: HashMap::new(),
            local_types: HashMap::new(),
            next_local: local_base,
            instructions: Vec::new(),
            rpo,
            block_index,
        }
    }

    fn local_for(&mut self, value: &LirValue) -> u32 {
        if let Some(&idx) = self.value_locals.get(&value.id) {
            return idx;
        }
        let idx = self.next_local;
        self.next_local += 1;
        self.value_locals.insert(value.id, idx);
        self.value_reprs.insert(value.id, value.repr);
        self.local_types.insert(idx, lir_repr_to_val(value.repr));
        idx
    }

    fn get_local(&self, vid: ValueId) -> u32 {
        self.value_locals[&vid]
    }

    fn emit_get(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalGet(self.get_local(vid)));
    }

    fn emit_set(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalSet(self.get_local(vid)));
    }

    fn repr_of(&self, vid: ValueId) -> LirRepr {
        self.value_reprs
            .get(&vid)
            .copied()
            .unwrap_or(LirRepr::DynBox)
    }
}

#[cfg(feature = "wasm-backend")]
fn compute_lir_rpo(func: &LirFunction) -> Vec<BlockId> {
    let mut visited = HashMap::new();
    let mut order = Vec::new();
    rpo_visit_lir(func, func.entry_block, &mut visited, &mut order);
    order.reverse();
    order
}

#[cfg(feature = "wasm-backend")]
fn rpo_visit_lir(
    func: &LirFunction,
    block_id: BlockId,
    visited: &mut HashMap<BlockId, bool>,
    order: &mut Vec<BlockId>,
) {
    if visited.contains_key(&block_id) {
        return;
    }
    visited.insert(block_id, true);
    if let Some(block) = func.blocks.get(&block_id) {
        for succ in lir_terminator_successors(&block.terminator) {
            rpo_visit_lir(func, succ, visited, order);
        }
    }
    order.push(block_id);
}

#[cfg(feature = "wasm-backend")]
fn lir_terminator_successors(term: &LirTerminator) -> Vec<BlockId> {
    match term {
        LirTerminator::Branch { target, .. } => vec![*target],
        LirTerminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        LirTerminator::Switch { cases, default, .. } => {
            let mut succs: Vec<BlockId> = cases.iter().map(|(_, bid, _)| *bid).collect();
            succs.push(*default);
            succs
        }
        LirTerminator::Return { .. } | LirTerminator::Unreachable => vec![],
    }
}

#[cfg(feature = "wasm-backend")]
pub fn lower_lir_to_wasm(func: &LirFunction) -> WasmFunctionOutput {
    let mut ctx = LirLowerCtx::new(func);

    let param_types: Vec<ValType> = func
        .blocks
        .get(&func.entry_block)
        .map(|entry| {
            entry
                .args
                .iter()
                .map(|arg| lir_repr_to_val(arg.repr))
                .collect()
        })
        .unwrap_or_default();
    let result_types: Vec<ValType> = func
        .return_types
        .iter()
        .map(|ty| lir_repr_to_val(LirRepr::for_type(ty)))
        .collect();

    if let Some(entry) = func.blocks.get(&func.entry_block) {
        for arg in &entry.args {
            ctx.local_for(arg);
        }
    }
    for &bid in &ctx.rpo.clone() {
        if let Some(block) = func.blocks.get(&bid) {
            for arg in &block.args {
                ctx.local_for(arg);
            }
            for op in &block.ops {
                for value in &op.result_values {
                    ctx.local_for(value);
                }
            }
        }
    }

    let num_params = param_types.len() as u32;
    let total_locals = ctx.next_local;
    let mut locals = Vec::with_capacity((total_locals - num_params) as usize);
    for idx in num_params..total_locals {
        let ty = ctx.local_types.get(&idx).copied().unwrap_or(ValType::I64);
        locals.push(ty);
    }

    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = func.blocks.get(&func.entry_block) {
            emit_lir_block_ops(&mut ctx, block);
            emit_lir_terminator(&mut ctx, &block.terminator);
        }
    } else {
        let back_edge_targets: HashMap<BlockId, bool> = {
            let mut targets = HashMap::new();
            for (src_idx, &bid) in rpo.iter().enumerate() {
                if let Some(block) = func.blocks.get(&bid) {
                    for succ in lir_terminator_successors(&block.terminator) {
                        if let Some(&tgt_idx) = ctx.block_index.get(&succ)
                            && tgt_idx <= src_idx
                        {
                            targets.insert(succ, true);
                        }
                    }
                }
            }
            targets
        };

        for (i, &bid) in rpo.iter().enumerate() {
            if i < num_blocks - 1 {
                if back_edge_targets.contains_key(&bid) {
                    ctx.instructions.push(Instruction::Loop(BlockType::Empty));
                } else {
                    ctx.instructions.push(Instruction::Block(BlockType::Empty));
                }
            }
        }

        for (i, &bid) in rpo.iter().enumerate() {
            if let Some(block) = func.blocks.get(&bid) {
                emit_lir_block_ops(&mut ctx, block);
                emit_lir_terminator_multiblock(&mut ctx, &block.terminator, num_blocks);
            }
            if i < num_blocks - 1 {
                ctx.instructions.push(Instruction::End);
            }
        }
    }

    ctx.instructions.push(Instruction::End);
    let instructions = peephole_set_get_to_tee(ctx.instructions);
    WasmFunctionOutput {
        param_types,
        result_types,
        locals,
        instructions,
    }
}

#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm_boxed_i64_abi(func: &TirFunction) -> Option<WasmFunctionOutput> {
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm_boxed_i64_abi(&lir)
}

#[cfg(feature = "wasm-backend")]
pub fn lower_lir_to_wasm_boxed_i64_abi(func: &LirFunction) -> Option<WasmFunctionOutput> {
    if func
        .param_types
        .iter()
        .any(|ty| *ty != super::types::TirType::I64)
    {
        return None;
    }
    if func.return_types.len() != 1 || func.return_types[0] != super::types::TirType::I64 {
        return None;
    }
    let entry = func.blocks.get(&func.entry_block)?;
    if entry.args.iter().any(|arg| arg.repr != LirRepr::I64) {
        return None;
    }

    let param_count = entry.args.len() as u32;
    let mut ctx = LirLowerCtx::new_with_local_base(func, param_count);

    for arg in &entry.args {
        ctx.local_for(arg);
    }
    for &bid in &ctx.rpo.clone() {
        if let Some(block) = func.blocks.get(&bid) {
            for arg in &block.args {
                ctx.local_for(arg);
            }
            for op in &block.ops {
                for value in &op.result_values {
                    ctx.local_for(value);
                }
            }
        }
    }

    let param_types = vec![ValType::I64; param_count as usize];
    let result_types = vec![ValType::I64];
    let total_locals = ctx.next_local;
    let mut locals = Vec::new();
    for idx in param_count..total_locals {
        let ty = ctx
            .value_locals
            .iter()
            .find(|&(_, &local_idx)| local_idx == idx)
            .and_then(|(vid, _)| ctx.value_reprs.get(vid))
            .copied()
            .map(lir_repr_to_val)
            .unwrap_or(ValType::I64);
        locals.push(ty);
    }

    for (idx, arg) in entry.args.iter().enumerate() {
        ctx.instructions.push(Instruction::LocalGet(idx as u32));
        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
        ctx.instructions.push(Instruction::I64Shl);
        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
        ctx.instructions.push(Instruction::I64ShrS);
        ctx.emit_set(arg.id);
    }

    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = func.blocks.get(&func.entry_block) {
            emit_lir_block_ops(&mut ctx, block);
            emit_lir_terminator_boxed_i64_abi(&mut ctx, &block.terminator);
        }
    } else {
        let back_edge_targets: HashMap<BlockId, bool> = {
            let mut targets = HashMap::new();
            for (src_idx, &bid) in rpo.iter().enumerate() {
                if let Some(block) = func.blocks.get(&bid) {
                    for succ in lir_terminator_successors(&block.terminator) {
                        if let Some(&tgt_idx) = ctx.block_index.get(&succ)
                            && tgt_idx <= src_idx
                        {
                            targets.insert(succ, true);
                        }
                    }
                }
            }
            targets
        };

        for (i, &bid) in rpo.iter().enumerate() {
            if i < num_blocks - 1 {
                if back_edge_targets.contains_key(&bid) {
                    ctx.instructions.push(Instruction::Loop(BlockType::Empty));
                } else {
                    ctx.instructions.push(Instruction::Block(BlockType::Empty));
                }
            }
        }

        for (i, &bid) in rpo.iter().enumerate() {
            if let Some(block) = func.blocks.get(&bid) {
                emit_lir_block_ops(&mut ctx, block);
                emit_lir_terminator_multiblock_boxed_i64_abi(
                    &mut ctx,
                    &block.terminator,
                    num_blocks,
                );
            }
            if i < num_blocks - 1 {
                ctx.instructions.push(Instruction::End);
            }
        }
    }

    ctx.instructions.push(Instruction::End);
    let instructions = peephole_set_get_to_tee(ctx.instructions);
    Some(WasmFunctionOutput {
        param_types,
        result_types,
        locals,
        instructions,
    })
}

// ---------------------------------------------------------------------------
// Op emission
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
#[cfg(feature = "wasm-backend")]
fn emit_lir_block_ops(ctx: &mut LirLowerCtx, block: &LirBlock) {
    for op in &block.ops {
        emit_lir_op(ctx, op);
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_op(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    match tir_op.opcode {
        OpCode::ConstInt => {
            let val = match tir_op.attrs.get("value") {
                Some(AttrValue::Int(v)) => *v,
                _ => 0,
            };
            if let Some(result) = op.result_values.first() {
                match result.repr {
                    LirRepr::F64 => ctx
                        .instructions
                        .push(Instruction::F64Const(Ieee64::from(val as f64))),
                    _ => ctx.instructions.push(Instruction::I64Const(val)),
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstFloat => {
            let val = match tir_op
                .attrs
                .get("f_value")
                .or_else(|| tir_op.attrs.get("value"))
            {
                Some(AttrValue::Float(v)) => *v,
                _ => 0.0,
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::F64Const(Ieee64::from(val)));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstBool => {
            let val = match tir_op.attrs.get("value") {
                Some(AttrValue::Bool(v)) => *v,
                _ => false,
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I32Const(if val { 1 } else { 0 }));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstNone => {
            if let Some(result) = op.result_values.first() {
                const QNAN: u64 = 0x7ff8_0000_0000_0000;
                const TAG_NONE: u64 = 0x0003_0000_0000_0000;
                ctx.instructions
                    .push(Instruction::I64Const((QNAN | TAG_NONE) as i64));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            if let Some(result) = op.result_values.first() {
                ctx.instructions.push(Instruction::I64Const(0));
                ctx.emit_set(result.id);
            }
        }
        OpCode::Add | OpCode::InplaceAdd => emit_lir_binary_arith(ctx, op, ArithOp::Add),
        OpCode::Sub | OpCode::InplaceSub => emit_lir_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul | OpCode::InplaceMul => emit_lir_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_lir_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_lir_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_lir_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Neg => emit_lir_unary_arith(ctx, op, UnaryOp::Neg),
        OpCode::Pos | OpCode::Copy | OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                ctx.emit_get(src);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Eq => emit_lir_comparison(ctx, op, CmpOp::Eq),
        OpCode::Ne => emit_lir_comparison(ctx, op, CmpOp::Ne),
        OpCode::Lt => emit_lir_comparison(ctx, op, CmpOp::Lt),
        OpCode::Le => emit_lir_comparison(ctx, op, CmpOp::Le),
        OpCode::Gt => emit_lir_comparison(ctx, op, CmpOp::Gt),
        OpCode::Ge => emit_lir_comparison(ctx, op, CmpOp::Ge),
        OpCode::BitAnd => emit_lir_bitwise(ctx, op, BitwiseOp::And),
        OpCode::BitOr => emit_lir_bitwise(ctx, op, BitwiseOp::Or),
        OpCode::BitXor => emit_lir_bitwise(ctx, op, BitwiseOp::Xor),
        OpCode::BitNot => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                ctx.emit_get(src);
                ctx.instructions.push(Instruction::I64Const(-1));
                ctx.instructions.push(Instruction::I64Xor);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Shl => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                ctx.emit_get(tir_op.operands[0]);
                ctx.emit_get(tir_op.operands[1]);
                ctx.instructions.push(Instruction::I64Shl);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Shr => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                ctx.emit_get(tir_op.operands[0]);
                ctx.emit_get(tir_op.operands[1]);
                ctx.instructions.push(Instruction::I64ShrS);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Not => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                ctx.emit_get(src);
                ctx.instructions.push(Instruction::I32Eqz);
                ctx.emit_set(result.id);
            }
        }
        OpCode::And | OpCode::Or => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                ctx.emit_get(tir_op.operands[0]);
                ctx.emit_get(tir_op.operands[1]);
                ctx.instructions.push(if tir_op.opcode == OpCode::And {
                    Instruction::I32And
                } else {
                    Instruction::I32Or
                });
                ctx.emit_set(result.id);
            }
        }
        OpCode::Bool => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                match ctx.repr_of(src) {
                    LirRepr::Bool1 => ctx.emit_get(src),
                    LirRepr::F64 => {
                        ctx.emit_get(src);
                        ctx.instructions
                            .push(Instruction::F64Const(Ieee64::from(0.0)));
                        ctx.instructions.push(Instruction::F64Ne);
                    }
                    _ => {
                        ctx.emit_get(src);
                        ctx.instructions.push(Instruction::Call(0));
                    }
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::CallBuiltin
            if matches!(
                tir_op.attrs.get("lir.truthy_cond"),
                Some(AttrValue::Bool(true))
            ) =>
        {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                match ctx.repr_of(src) {
                    LirRepr::Bool1 => ctx.emit_get(src),
                    LirRepr::F64 => {
                        ctx.emit_get(src);
                        ctx.instructions
                            .push(Instruction::F64Const(Ieee64::from(0.0)));
                        ctx.instructions.push(Instruction::F64Ne);
                    }
                    _ => {
                        ctx.emit_get(src);
                        ctx.instructions.push(Instruction::Call(0));
                    }
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallBuiltin
        | OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildTuple
        | OpCode::BuildSet
        | OpCode::BuildSlice
        | OpCode::LoadAttr
        | OpCode::StoreAttr
        | OpCode::DelAttr
        | OpCode::Index
        | OpCode::StoreIndex
        | OpCode::DelIndex
        | OpCode::Alloc
        | OpCode::StackAlloc
        | OpCode::ObjectNewBound
        | OpCode::ObjectNewBoundStack
        | OpCode::Free
        | OpCode::GetIter
        | OpCode::IterNext
        | OpCode::IterNextUnboxed
        | OpCode::ForIter
        | OpCode::StateSwitch
        | OpCode::StateTransition
        | OpCode::StateYield
        | OpCode::ChanSendYield
        | OpCode::ChanRecvYield
        | OpCode::ClosureLoad
        | OpCode::ClosureStore
        | OpCode::Import
        | OpCode::ImportFrom
        | OpCode::ModuleCacheGet
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::Pow
        | OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::AllocTask
        | OpCode::Yield
        | OpCode::YieldFrom
        | OpCode::ScfIf
        | OpCode::ScfFor
        | OpCode::ScfWhile
        | OpCode::ScfYield
        | OpCode::Deopt
        | OpCode::TryStart
        | OpCode::TryEnd
        | OpCode::StateBlockStart
        | OpCode::StateBlockEnd
        | OpCode::WarnStderr
        | OpCode::IncRef
        | OpCode::DecRef => {
            for &operand in &tir_op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0));
            if let Some(result) = op.result_values.first() {
                ctx.emit_set(result.id);
            }
        }
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_binary_arith(ctx: &mut LirLowerCtx, op: &LirOp, arith: ArithOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    if matches!(
        tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    ) {
        let main = op.result_values[0].id;
        let overflow_box = op.result_values[1].id;
        let overflow_flag = op.result_values[2].id;

        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(Instruction::I64Add);
        ctx.emit_set(main);

        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MIN));
        ctx.instructions.push(Instruction::I64GeS);
        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MAX));
        ctx.instructions.push(Instruction::I64LeS);
        ctx.instructions.push(Instruction::I32And);
        ctx.instructions.push(Instruction::If(BlockType::Empty));
        emit_box_none(ctx);
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(0));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::Else);
        emit_box_inline_i64(ctx, lhs);
        emit_box_inline_i64(ctx, rhs);
        ctx.instructions.push(Instruction::Call(0));
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(1));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::End);
        return;
    }
    let lhs_repr = ctx.repr_of(lhs);
    let rhs_repr = ctx.repr_of(rhs);
    ctx.emit_get(lhs);
    ctx.emit_get(rhs);
    match (lhs_repr, rhs_repr) {
        (LirRepr::I64, LirRepr::I64) => ctx.instructions.push(match arith {
            ArithOp::Add => Instruction::I64Add,
            ArithOp::Sub => Instruction::I64Sub,
            ArithOp::Mul => Instruction::I64Mul,
            ArithOp::Div | ArithOp::FloorDiv => Instruction::I64DivS,
            ArithOp::Mod => Instruction::I64RemS,
        }),
        (LirRepr::F64, LirRepr::F64) => match arith {
            ArithOp::Add => ctx.instructions.push(Instruction::F64Add),
            ArithOp::Sub => ctx.instructions.push(Instruction::F64Sub),
            ArithOp::Mul => ctx.instructions.push(Instruction::F64Mul),
            ArithOp::Div => ctx.instructions.push(Instruction::F64Div),
            ArithOp::FloorDiv => {
                // Python // on floats: floor(a / b)
                ctx.instructions.push(Instruction::F64Div);
                ctx.instructions.push(Instruction::F64Floor);
                // Result already on stack, fall through to emit_set.
            }
            ArithOp::Mod => {
                // Python fmod: a - floor(a / b) * b
                // Stack: [lhs, rhs]. We need both values twice.
                // Allocate scratch locals for the operands.
                let scratch_a = ctx.next_local;
                ctx.next_local += 1;
                ctx.local_types.insert(scratch_a, ValType::F64);
                let scratch_b = ctx.next_local;
                ctx.next_local += 1;
                ctx.local_types.insert(scratch_b, ValType::F64);
                // Pop rhs, pop lhs into scratches.
                ctx.instructions.push(Instruction::LocalSet(scratch_b));
                ctx.instructions.push(Instruction::LocalSet(scratch_a));
                // Compute: a - floor(a / b) * b
                ctx.instructions.push(Instruction::LocalGet(scratch_a));
                ctx.instructions.push(Instruction::LocalGet(scratch_a));
                ctx.instructions.push(Instruction::LocalGet(scratch_b));
                ctx.instructions.push(Instruction::F64Div);
                ctx.instructions.push(Instruction::F64Floor);
                ctx.instructions.push(Instruction::LocalGet(scratch_b));
                ctx.instructions.push(Instruction::F64Mul);
                ctx.instructions.push(Instruction::F64Sub);
                // Result on stack, fall through to emit_set.
            }
        },
        _ => {
            ctx.instructions.push(Instruction::Call(0));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_unary_arith(ctx: &mut LirLowerCtx, op: &LirOp, _unary: UnaryOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.is_empty() || op.result_values.is_empty() {
        return;
    }
    let src = tir_op.operands[0];
    let dst = op.result_values[0].id;
    match ctx.repr_of(src) {
        LirRepr::I64 => {
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Sub);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::F64Neg);
        }
        _ => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::Call(0));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_comparison(ctx: &mut LirLowerCtx, op: &LirOp, cmp: CmpOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    ctx.emit_get(lhs);
    ctx.emit_get(rhs);
    match (ctx.repr_of(lhs), ctx.repr_of(rhs)) {
        (LirRepr::I64, LirRepr::I64) => ctx.instructions.push(match cmp {
            CmpOp::Eq => Instruction::I64Eq,
            CmpOp::Ne => Instruction::I64Ne,
            CmpOp::Lt => Instruction::I64LtS,
            CmpOp::Le => Instruction::I64LeS,
            CmpOp::Gt => Instruction::I64GtS,
            CmpOp::Ge => Instruction::I64GeS,
        }),
        (LirRepr::F64, LirRepr::F64) => ctx.instructions.push(match cmp {
            CmpOp::Eq => Instruction::F64Eq,
            CmpOp::Ne => Instruction::F64Ne,
            CmpOp::Lt => Instruction::F64Lt,
            CmpOp::Le => Instruction::F64Le,
            CmpOp::Gt => Instruction::F64Gt,
            CmpOp::Ge => Instruction::F64Ge,
        }),
        _ => {
            ctx.instructions.push(Instruction::Call(0));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_bitwise(ctx: &mut LirLowerCtx, op: &LirOp, bw: BitwiseOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    ctx.emit_get(tir_op.operands[0]);
    ctx.emit_get(tir_op.operands[1]);
    ctx.instructions.push(match bw {
        BitwiseOp::And => Instruction::I64And,
        BitwiseOp::Or => Instruction::I64Or,
        BitwiseOp::Xor => Instruction::I64Xor,
    });
    ctx.emit_set(op.result_values[0].id);
}

#[cfg(feature = "wasm-backend")]
fn emit_box_inline_i64(ctx: &mut LirLowerCtx, src: ValueId) {
    ctx.emit_get(src);
    ctx.instructions.push(Instruction::I64Const(INT_MASK));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions.push(Instruction::I64Const(QNAN | TAG_INT));
    ctx.instructions.push(Instruction::I64Or);
}

#[cfg(feature = "wasm-backend")]
fn emit_box_none(ctx: &mut LirLowerCtx) {
    ctx.instructions
        .push(Instruction::I64Const(QNAN | TAG_NONE));
}

#[cfg(feature = "wasm-backend")]
fn emit_return_boxed_i64(ctx: &mut LirLowerCtx, value: ValueId) {
    match ctx.repr_of(value) {
        LirRepr::I64 => emit_box_inline_i64(ctx, value),
        LirRepr::DynBox => ctx.emit_get(value),
        LirRepr::Bool1 => {
            ctx.emit_get(value);
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions.push(Instruction::I64Const(
                QNAN | 0x0002_0000_0000_0000u64 as i64,
            ));
            ctx.instructions.push(Instruction::I64Or);
        }
        LirRepr::F64 => {
            ctx.emit_get(value);
            ctx.instructions.push(Instruction::Call(0));
        }
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_terminator(ctx: &mut LirLowerCtx, term: &LirTerminator) {
    match term {
        LirTerminator::Return { values } => {
            if let Some(&val) = values.first() {
                ctx.emit_get(val);
            }
            ctx.instructions.push(Instruction::Return);
        }
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        _ => ctx.instructions.push(Instruction::Unreachable),
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_terminator_boxed_i64_abi(ctx: &mut LirLowerCtx, term: &LirTerminator) {
    match term {
        LirTerminator::Return { values } => {
            if let Some(&val) = values.first() {
                emit_return_boxed_i64(ctx, val);
            } else {
                emit_box_none(ctx);
            }
            ctx.instructions.push(Instruction::Return);
        }
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        _ => ctx.instructions.push(Instruction::Unreachable),
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_terminator_multiblock(ctx: &mut LirLowerCtx, term: &LirTerminator, num_blocks: usize) {
    match term {
        LirTerminator::Return { values } => {
            if let Some(&val) = values.first() {
                ctx.emit_get(val);
            }
            ctx.instructions.push(Instruction::Return);
        }
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        LirTerminator::Branch { target, args } => {
            store_lir_block_args(ctx, *target, args);
            if let Some(&tgt_idx) = ctx.block_index.get(target) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        LirTerminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            match ctx.repr_of(*cond) {
                LirRepr::Bool1 => ctx.emit_get(*cond),
                LirRepr::I64 => {
                    ctx.emit_get(*cond);
                    ctx.instructions.push(Instruction::I64Const(0));
                    ctx.instructions.push(Instruction::I64Ne);
                }
                LirRepr::F64 => {
                    ctx.emit_get(*cond);
                    ctx.instructions
                        .push(Instruction::F64Const(Ieee64::from(0.0)));
                    ctx.instructions.push(Instruction::F64Ne);
                }
                LirRepr::DynBox => {
                    ctx.emit_get(*cond);
                    ctx.instructions.push(Instruction::Call(0));
                }
            }
            store_lir_block_args(ctx, *then_block, then_args);
            if let Some(&tgt_idx) = ctx.block_index.get(then_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::BrIf(depth as u32));
            }
            store_lir_block_args(ctx, *else_block, else_args);
            if let Some(&tgt_idx) = ctx.block_index.get(else_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        LirTerminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            for (case_val, target, args) in cases {
                ctx.emit_get(*value);
                ctx.instructions.push(Instruction::I64Const(*case_val));
                ctx.instructions.push(Instruction::I64Eq);
                store_lir_block_args(ctx, *target, args);
                if let Some(&tgt_idx) = ctx.block_index.get(target) {
                    let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                    ctx.instructions.push(Instruction::BrIf(depth as u32));
                }
            }
            store_lir_block_args(ctx, *default, default_args);
            if let Some(&tgt_idx) = ctx.block_index.get(default) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
    }
}

#[cfg(feature = "wasm-backend")]
fn emit_lir_terminator_multiblock_boxed_i64_abi(
    ctx: &mut LirLowerCtx,
    term: &LirTerminator,
    num_blocks: usize,
) {
    match term {
        LirTerminator::Return { values } => {
            if let Some(&val) = values.first() {
                emit_return_boxed_i64(ctx, val);
            } else {
                emit_box_none(ctx);
            }
            ctx.instructions.push(Instruction::Return);
        }
        other => emit_lir_terminator_multiblock(ctx, other, num_blocks),
    }
}

#[cfg(feature = "wasm-backend")]
fn store_lir_block_args(ctx: &mut LirLowerCtx, target: BlockId, args: &[ValueId]) {
    if let Some(block) = ctx.func.blocks.get(&target) {
        for (arg_val, &src_val) in block.args.iter().zip(args.iter()) {
            ctx.emit_get(src_val);
            let dst_local = ctx.get_local(arg_val.id);
            ctx.instructions.push(Instruction::LocalSet(dst_local));
        }
    }
}

// ---------------------------------------------------------------------------
// Peephole: local.set X; local.get X → local.tee X
// ---------------------------------------------------------------------------
//
// The SSA→stack-machine lowering emits an explicit local.set after every op
// result and a local.get before every operand read. This creates abundant
// `local.set X; local.get X` pairs where the value is stored AND immediately
// reloaded. WASM's `local.tee` instruction does both in one shot: it stores
// the value in the local AND leaves a copy on the stack, eliminating the
// redundant get.
//
// This is a single linear pass over the instruction vector: O(N) time, O(N)
// space (new vec). No control-flow analysis needed because the pattern is
// purely sequential and the semantics are identical.
//
// Additionally, when the tee'd value is never read again after the
// immediately following instruction, the entire set can sometimes be
// eliminated — but that requires liveness analysis beyond this peephole's
// scope. wasm-opt handles that downstream.

#[cfg(feature = "wasm-backend")]
fn peephole_set_get_to_tee(instructions: Vec<Instruction<'static>>) -> Vec<Instruction<'static>> {
    if instructions.len() < 2 {
        return instructions;
    }
    let mut out = Vec::with_capacity(instructions.len());
    let mut i = 0;
    while i < instructions.len() {
        // Pattern 1: local.set X; local.get X -> local.tee X
        if i + 1 < instructions.len() {
            if let (Instruction::LocalSet(set_idx), Instruction::LocalGet(get_idx)) =
                (&instructions[i], &instructions[i + 1])
            {
                if set_idx == get_idx {
                    out.push(Instruction::LocalTee(*set_idx));
                    i += 2;
                    continue;
                }
            }
        }
        // Pattern 2: i64.const 0; i64.eq -> i64.eqz (test for zero)
        if i + 1 < instructions.len() {
            if let (Instruction::I64Const(0), Instruction::I64Eq) =
                (&instructions[i], &instructions[i + 1])
            {
                out.push(Instruction::I64Eqz);
                i += 2;
                continue;
            }
        }
        // Pattern 3: i32.const 0; i32.eq -> i32.eqz
        if i + 1 < instructions.len() {
            if let (Instruction::I32Const(0), Instruction::I32Eq) =
                (&instructions[i], &instructions[i + 1])
            {
                out.push(Instruction::I32Eqz);
                i += 2;
                continue;
            }
        }
        // Pattern 4: i64.const 1; i64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len() {
            if let (Instruction::I64Const(1), Instruction::I64Mul) =
                (&instructions[i], &instructions[i + 1])
            {
                // Value already on stack; skip the const+mul.
                i += 2;
                continue;
            }
        }
        // Pattern 5: i64.const 0; i64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len() {
            if let (Instruction::I64Const(0), Instruction::I64Add) =
                (&instructions[i], &instructions[i + 1])
            {
                i += 2;
                continue;
            }
        }
        // Pattern 6: i64.const 0; i64.sub -> (eliminated, sub 0 is identity)
        if i + 1 < instructions.len() {
            if let (Instruction::I64Const(0), Instruction::I64Sub) =
                (&instructions[i], &instructions[i + 1])
            {
                i += 2;
                continue;
            }
        }
        // Pattern 7: i64.const -1; i64.xor -> (equivalent to bit_not, but keep xor)
        // Not folded: -1 xor is the canonical bit_not, no simpler form exists.

        // Pattern 8: f64.const 0.0; f64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len() {
            if let (Instruction::F64Const(z), Instruction::F64Add) =
                (&instructions[i], &instructions[i + 1])
            {
                if f64::from(*z) == 0.0 {
                    i += 2;
                    continue;
                }
            }
        }
        // Pattern 9: f64.const 1.0; f64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len() {
            if let (Instruction::F64Const(one), Instruction::F64Mul) =
                (&instructions[i], &instructions[i + 1])
            {
                if f64::from(*one) == 1.0 {
                    i += 2;
                    continue;
                }
            }
        }
        out.push(instructions[i].clone());
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "wasm-backend")]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

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
        let has_const = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Const(42)));
        assert!(has_const, "expected i64.const 42 in output");

        // Should end with `end`.
        assert!(matches!(output.instructions.last(), Some(Instruction::End)));
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
        let has_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Add));
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
        let has_f64_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::F64Add));
        assert!(has_f64_add, "expected f64.add instruction");
    }

    #[test]
    fn conditional_branch() {
        let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

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
        let has_br_if = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::BrIf(_)));
        assert!(
            has_br_if,
            "expected br_if instruction for conditional branch"
        );
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

        let has_lt = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64LtS));
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
        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for DynBox add");

        let has_i64_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Add));
        assert!(!has_i64_add, "should NOT emit i64.add for DynBox operands");
    }

    #[test]
    fn alloc_task_falls_back_to_runtime_call() {
        let mut func =
            TirFunction::new("alloc_task".into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::AllocTask,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("s_value".into(), AttrValue::Str("task_poll".into()));
                m.insert("task_kind".into(), AttrValue::Str("future".into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for alloc_task");
    }

    #[test]
    fn state_switch_falls_back_to_runtime_call() {
        let mut func = TirFunction::new(
            "state_switch".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StateSwitch,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for state_switch");
    }

    // -----------------------------------------------------------------------
    // Peephole pass tests
    // -----------------------------------------------------------------------

    #[test]
    fn peephole_collapses_set_get_to_tee() {
        let input = vec![
            Instruction::I64Const(42),
            Instruction::LocalSet(3),
            Instruction::LocalGet(3),
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 3);
        assert!(
            matches!(output[0], Instruction::I64Const(42)),
            "const preserved"
        );
        assert!(
            matches!(output[1], Instruction::LocalTee(3)),
            "set+get collapsed to tee"
        );
        assert!(matches!(output[2], Instruction::End), "end preserved");
    }

    #[test]
    fn peephole_preserves_mismatched_set_get() {
        let input = vec![
            Instruction::LocalSet(1),
            Instruction::LocalGet(2), // different local
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 3);
        assert!(
            matches!(output[0], Instruction::LocalSet(1)),
            "set preserved"
        );
        assert!(
            matches!(output[1], Instruction::LocalGet(2)),
            "get preserved"
        );
    }

    #[test]
    fn peephole_handles_consecutive_tee_chains() {
        // Pattern: set(1) get(1) set(2) get(2) → tee(1) tee(2)
        let input = vec![
            Instruction::I64Const(10),
            Instruction::LocalSet(1),
            Instruction::LocalGet(1),
            Instruction::LocalSet(2),
            Instruction::LocalGet(2),
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 4);
        assert!(matches!(output[1], Instruction::LocalTee(1)));
        assert!(matches!(output[2], Instruction::LocalTee(2)));
    }

    #[test]
    fn peephole_empty_and_single() {
        assert!(peephole_set_get_to_tee(vec![]).is_empty());
        let single = vec![Instruction::End];
        assert_eq!(peephole_set_get_to_tee(single).len(), 1);
    }

    #[test]
    fn peephole_applied_in_const_return() {
        // A const-return function should have tee instead of set+get.
        let func = make_const_return_func(99);
        let output = lower_tir_to_wasm(&func);

        // After peephole, the pattern: i64.const 99; local.set X; local.get X; return
        // becomes: i64.const 99; local.tee X; return
        let has_tee = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::LocalTee(_)));
        assert!(has_tee, "expected local.tee from peephole optimization");

        // Should have no set+get pairs for the same local.
        for window in output.instructions.windows(2) {
            if let (Instruction::LocalSet(s), Instruction::LocalGet(g)) = (&window[0], &window[1]) {
                assert_ne!(
                    s, g,
                    "found redundant set+get pair for local {s} that peephole should have eliminated"
                );
            }
        }
    }
}
