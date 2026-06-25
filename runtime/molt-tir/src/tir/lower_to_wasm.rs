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
//! | Ref64       | i64         | Runtime reference word         |
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
use super::lower_to_lir::{lower_function_to_lir, lower_function_to_lir_with_inline_proof};
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
    /// Runtime imports this body calls, in EMISSION ORDER. Each entry pairs
    /// positionally with one `Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)`
    /// in `instructions`; the module assembler walks the stream and replaces
    /// the k-th placeholder with the import index of `runtime_calls[k]`.
    /// Positional (not index-keyed) because the peephole pass rewrites the
    /// stream and shifts instruction indexes. Distinct from the `Call(0)`
    /// BAIL sentinel (reject-this-function) and the `u32::MAX`
    /// skipped-import sentinel.
    pub runtime_calls: Vec<&'static str>,
}

/// Placeholder callee index for a NAMED runtime call recorded in
/// [`WasmFunctionOutput::runtime_calls`]. Resolved to a real import index by
/// the module assembler. `u32::MAX - 1` so it can never collide with the
/// `Call(0)` bail sentinel or the `u32::MAX` skipped-import sentinel.
pub const NAMED_RUNTIME_CALL_PLACEHOLDER: u32 = u32::MAX - 1;

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` → `wasm i64`, `F64` → `wasm f64`, `DynBox` → runtime call.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm(func: &TirFunction) -> WasmFunctionOutput {
    // The generic path derives carriers from the same pure-TIR `repr_by_value`
    // authority as the boxed-i64 ABI path and LLVM. Semantic `I64` alone is not
    // a raw machine carrier; unproven ints lower as DynBox/boxed runtime values,
    // while Bool/F64 and range-proven ints keep their scalar lanes.
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm(&lir)
}

#[cfg(feature = "wasm-backend")]
fn lir_repr_to_val(repr: LirRepr) -> ValType {
    match repr {
        LirRepr::I64 => ValType::I64,
        LirRepr::F64 => ValType::F64,
        LirRepr::Bool1 => ValType::I32,
        LirRepr::DynBox | LirRepr::Ref64 => ValType::I64,
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
    /// Named runtime calls in emission order (see
    /// [`WasmFunctionOutput::runtime_calls`]).
    runtime_calls: Vec<&'static str>,
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
            runtime_calls: Vec::new(),
            rpo,
            block_index,
        }
    }

    /// Emit a NAMED runtime-import call: a placeholder `Call` paired
    /// positionally with `name` in `runtime_calls`, resolved to the real
    /// import index by the module assembler. This is how the LIR fast lane
    /// reaches runtime helpers (e.g. `int_from_i64` for the overflow-safe
    /// box) without bailing the whole function the way `Call(0)` does.
    fn emit_runtime_call(&mut self, name: &'static str) {
        self.instructions
            .push(Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER));
        self.runtime_calls.push(name);
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
        LirTerminator::Switch { cases, default, .. }
        | LirTerminator::StateDispatch { cases, default, .. } => {
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
        runtime_calls: ctx.runtime_calls,
    }
}

#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm_boxed_i64_abi(func: &TirFunction) -> Option<WasmFunctionOutput> {
    let vr = crate::representation_plan::value_range_for(func);
    let repr = crate::representation_plan::repr_by_value_for(func, Some(&vr));
    lower_tir_to_wasm_boxed_i64_abi_with_proof(func, &repr, &vr)
}

/// Boxed-i64 WASM ABI lowering with the value-range proof explicitly paired to
/// the value-keyed Repr map. The production WASM fast lane uses this entry so
/// full-range raw carriers can never take the 47-bit-window checked-i64 triple.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm_boxed_i64_abi_with_proof(
    func: &TirFunction,
    repr: &HashMap<ValueId, crate::repr::Repr>,
    inline_proof: &crate::tir::passes::value_range::ValueRangeResult,
) -> Option<WasmFunctionOutput> {
    let lir = lower_function_to_lir_with_inline_proof(func, repr, inline_proof);
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
        runtime_calls: ctx.runtime_calls,
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
        OpCode::CheckedAdd => {
            // (sum, flag) = signed-i64 add. A TOTAL function with two lanes:
            //
            // RAW lane (both operands LirRepr::I64): EXACT overflow detection
            // at 2^63 (NOT the 47-bit inline-range triple above — that fires
            // 2^16x too early for the overflow_peel fast loop). WASM has no
            // add-with-overflow instruction; the sign-bit identity
            // ((lhs ^ sum) & (rhs ^ sum)) < 0 is exact: overflow occurred
            // iff both operands share a sign and the sum's sign differs.
            //
            // BOXED lane (any operand unproven — the v1 state on WASM, whose
            // value-keyed RawI64Safe is a 47-bit-window contract that cannot
            // carry an unbounded accumulator): dispatch through the runtime
            // add with both operands NaN-boxed — BigInt-exact, so the sum
            // can never silently wrap and the flag is CONSTANT FALSE (the
            // peel's slow path is correctly dead; same semantics, no speedup
            // until the RawI64Full lattice extension lands).
            assert!(
                tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
                "checked_add requires 2 operands and 2 results"
            );
            let lhs = tir_op.operands[0];
            let rhs = tir_op.operands[1];
            let sum = op.result_values[0].id;
            let flag = op.result_values[1].id;
            if matches!(ctx.repr_of(lhs), LirRepr::I64)
                && matches!(ctx.repr_of(rhs), LirRepr::I64)
            {
                ctx.emit_get(lhs);
                ctx.emit_get(rhs);
                ctx.instructions.push(Instruction::I64Add);
                ctx.emit_set(sum);
                ctx.emit_get(lhs);
                ctx.emit_get(sum);
                ctx.instructions.push(Instruction::I64Xor);
                ctx.emit_get(rhs);
                ctx.emit_get(sum);
                ctx.instructions.push(Instruction::I64Xor);
                ctx.instructions.push(Instruction::I64And);
                ctx.instructions.push(Instruction::I64Const(0));
                ctx.instructions.push(Instruction::I64LtS);
                ctx.emit_set(flag);
            } else {
                emit_get_boxed_for_repr(ctx, lhs);
                emit_get_boxed_for_repr(ctx, rhs);
                ctx.instructions.push(Instruction::Call(0));
                ctx.emit_set(sum);
                ctx.instructions.push(Instruction::I32Const(0));
                ctx.emit_set(flag);
            }
        }
        OpCode::CheckedMul => {
            // (product, flag) = signed-i64 multiply. BOXED-LANE-ONLY v1.
            //
            // WASM has no multiply-with-overflow instruction and no raw
            // 64x64->128 widening primitive, so there is NO sound raw fast
            // lane today (unlike CheckedAdd's sign-bit identity). Rather than
            // fabricate a fake helper or a wrong narrow-range check, every
            // CheckedMul bails this function out of the WASM fast lane via the
            // `Call(0)` BAIL sentinel — the guarded slow path then runs the
            // boxed runtime multiply (`molt_mul`), which is BigInt-exact, so
            // the product can never silently wrap. The overflow flag is set
            // CONSTANT FALSE (the peel's slow path is correctly dead on this
            // bailed lane; same semantics, no speedup). This is a DOCUMENTED
            // target limitation per the Performance Constitution backend
            // scoreboard, retired when the RawI64Full lattice + a 64x64->128
            // overflow helper land.
            assert!(
                tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
                "checked_mul requires 2 operands and 2 results"
            );
            let lhs = tir_op.operands[0];
            let rhs = tir_op.operands[1];
            let product = op.result_values[0].id;
            let flag = op.result_values[1].id;
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.instructions.push(Instruction::Call(0));
            ctx.emit_set(product);
            ctx.instructions.push(Instruction::I32Const(0));
            ctx.emit_set(flag);
        }
        OpCode::Sub | OpCode::InplaceSub => emit_lir_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul | OpCode::InplaceMul => emit_lir_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_lir_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_lir_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_lir_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Neg => emit_lir_unary_arith(ctx, op, UnaryOp::Neg),
        OpCode::Pos | OpCode::Copy | OpCode::DeleteVar | OpCode::BoxVal | OpCode::UnboxVal
        | OpCode::TypeGuard => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                if matches!(
                    tir_op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if kind == "binding_alias"
                ) {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.emit_runtime_call("inc_ref_obj");
                }
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
                // `~x` is a bare `x ^ -1` only when `x` is a proven raw i64; an
                // unproven (`DynBox`/`MaybeBigInt`) operand must dispatch through
                // the runtime helper (a raw `I64Xor` on a NaN-boxed word would be
                // a miscompile). On the production fast path the resulting
                // `Call(0)` bails to the guarded slow path.
                if ctx.repr_of(src) == LirRepr::I64 {
                    ctx.emit_get(src);
                    ctx.instructions.push(Instruction::I64Const(-1));
                    ctx.instructions.push(Instruction::I64Xor);
                } else {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.instructions.push(Instruction::Call(0));
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::Shl => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                let result_id = result.id;
                let result_repr = result.repr;
                // Shifts REQUIRE the raw-result proof: a raw `i64.shl` whose
                // count is >= 64 masks mod 64 (wrong value) and a `<<` result can
                // exceed i64. The value-range seed grants `LirRepr::I64` only when
                // the count is range-proven `[0, 63]` and the result fits inline.
                emit_lir_i64_binary_or_boxed(
                    ctx,
                    tir_op.operands[0],
                    tir_op.operands[1],
                    result_id,
                    result_repr,
                    Instruction::I64Shl,
                    true,
                );
            }
        }
        OpCode::Shr => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                let result_id = result.id;
                let result_repr = result.repr;
                emit_lir_i64_binary_or_boxed(
                    ctx,
                    tir_op.operands[0],
                    tir_op.operands[1],
                    result_id,
                    result_repr,
                    Instruction::I64ShrS,
                    true,
                );
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
            if tir_op.operands.len() >= 2 && !op.result_values.is_empty() {
                emit_lir_bool_select(ctx, op, tir_op.opcode == OpCode::And);
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
        | OpCode::OrdAt
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
        | OpCode::ModuleImportFrom
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::ModuleDelGlobalIfPresent
        | OpCode::Pow
        | OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::ExceptionPending
        // Reads a function object's defaults version stamp via a runtime call;
        // the LIR fast lane does not model it, so bail this function to the
        // generic WASM emitter (which has a `function_defaults_version` arm).
        | OpCode::FunctionDefaultsVersion
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
        // ConstBigInt needs a data segment + molt_bigint_from_str, which the
        // LIR fast lane does not model — bail the function to the generic
        // emitter (which handles `const_bigint` natively).
        | OpCode::ConstBigInt => {
            for &operand in &tir_op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0));
            if let Some(result) = op.result_values.first() {
                ctx.emit_set(result.id);
            }
        }
        // RC drop-insertion ops (design 20, §4.3 Phase 4). `molt_dec_ref_obj` /
        // `molt_inc_ref_obj` take the NaN-boxed value by value and fast-path
        // non-pointers, so passing the operand's boxed form is always safe; the
        // repr filter in the drop pass already excludes raw-scalar carriers, so
        // the operand here is a heap-carrying (NaN-boxed-pointer) value. A NAMED
        // runtime call keeps the function in the LIR fast lane rather than
        // bailing it (`Call(0)`) to the generic emitter — preserving the WASM
        // perf contract for drop-inserted functions. Neither op has a result.
        OpCode::DecRef | OpCode::DelBoundary => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call("dec_ref_obj");
            }
        }
        OpCode::IncRef => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call("inc_ref_obj");
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
        // Inline boxing is sound here: the checked-triple gate
        // (`lowers_to_checked_i64_arithmetic`) only fires when BOTH operands
        // are value-range-proven inside the 47-bit inline window.
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
    // LIR-lowering marked this op as requiring the boxed runtime dispatch
    // (raw-i64 operands without the inline-window proof — a bare machine op
    // could wrap at 2^63). Honor it before any repr-keyed arm.
    let boxed_dispatch = matches!(
        tir_op.attrs.get("lir.boxed_dispatch"),
        Some(AttrValue::Bool(true))
    );
    // Phase 1 introduces *mixed* reprs (e.g. a proven `RawI64Safe` operand and an
    // unproven `MaybeBigInt`/`DynBox` operand). The boxed fallthrough dispatches
    // through the BigInt-correct runtime helper, which expects NaN-boxed
    // operands — so operands must be pushed *per-arm*, raw only for the
    // homogeneous unboxed arms and BOXED for the runtime-call arm. Pushing raw
    // before the match (the pre-Phase-1 shape) would feed a raw i64 word to
    // `molt_add` on the mixed case → a hard miscompile.
    let result_repr = op.result_values[0].repr;
    match (lhs_repr, rhs_repr) {
        // Bare machine arithmetic requires the RESULT to be a raw carrier too.
        // Raw carriers may include full-i64 `RawI64FullDeopt` CheckedAdd/
        // CheckedMul results. When the result is unproven (boxed repr), a bare
        // op would silently wrap at 2^63 AND deposit a raw word in a
        // DynBox-typed local; such ops take the boxed runtime dispatch below
        // instead. `boxed_dispatch` (proof-driven, set at LIR-lowering)
        // likewise forces the runtime path.
        (LirRepr::I64, LirRepr::I64) if result_repr == LirRepr::I64 && !boxed_dispatch => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match arith {
                ArithOp::Add => Instruction::I64Add,
                ArithOp::Sub => Instruction::I64Sub,
                ArithOp::Mul => Instruction::I64Mul,
                ArithOp::Div | ArithOp::FloorDiv => Instruction::I64DivS,
                ArithOp::Mod => Instruction::I64RemS,
            });
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            match arith {
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
            }
        }
        _ => {
            // Heterogeneous / boxed operands: dispatch through the runtime
            // helper with both operands NaN-boxed (overflow-safely — a
            // raw-i64 operand may be full-range). A NAMED runtime call keeps
            // the function in the LIR fast lane (Call(0) is the
            // reject-this-function bail sentinel).
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(match arith {
                ArithOp::Add => "add",
                ArithOp::Sub => "sub",
                ArithOp::Mul => "mul",
                ArithOp::Div => "div",
                ArithOp::FloorDiv => "floordiv",
                ArithOp::Mod => "mod",
            });
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

/// Push operand `v` onto the WASM stack in **NaN-boxed** form, ready for a
/// runtime helper call (`molt_add`/`molt_lt`/...). A raw-i64-repr operand is
/// boxed through the overflow-safe path because it may be a full-i64
/// `RawI64FullDeopt` carrier; a `Bool1` is widened to a boxed bool; an `F64` is
/// boxed via the runtime float-box; a `DynBox`/`Ref64` operand is already a
/// NaN-box word and passes through unchanged.
///
/// This is the Phase-1 fix for `emit_lir_binary_arith`'s (and the comparison's)
/// boxed fallthrough: before Phase 1 every int operand was `LirRepr::I64`, so the
/// boxed arm only fired for homogeneous `DynBox`; now a proven `I64` operand can
/// share an op with an unproven `DynBox` operand, and the raw one MUST be boxed
/// before the call.
#[cfg(feature = "wasm-backend")]
fn emit_get_boxed_for_repr(ctx: &mut LirLowerCtx, v: ValueId) {
    match ctx.repr_of(v) {
        // OVERFLOW-SAFE: raw-i64 carriers may include full-i64
        // `RawI64FullDeopt` CheckedAdd/CheckedMul results; the unchecked inline
        // box truncates mod 2^47.
        LirRepr::I64 => emit_box_i64_overflow_safe(ctx, v),
        LirRepr::Bool1 => {
            ctx.emit_get(v);
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions.push(Instruction::I64Const(
                QNAN | 0x0002_0000_0000_0000u64 as i64,
            ));
            ctx.instructions.push(Instruction::I64Or);
        }
        LirRepr::F64 => {
            // Box the unboxed f64 via the runtime float-box helper (placeholder
            // call index, resolved at link time like every other runtime call).
            ctx.emit_get(v);
            ctx.instructions.push(Instruction::Call(0));
        }
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_get(v),
    }
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
fn emit_lir_bool_select(ctx: &mut LirLowerCtx, op: &LirOp, is_and: bool) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];
    let dst = result.id;
    if ctx.repr_of(lhs) == LirRepr::Bool1
        && ctx.repr_of(rhs) == LirRepr::Bool1
        && result.repr == LirRepr::Bool1
    {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(if is_and {
            Instruction::I32And
        } else {
            Instruction::I32Or
        });
        ctx.emit_set(dst);
        return;
    }

    assert!(
        matches!(result.repr, LirRepr::DynBox | LirRepr::Ref64),
        "boxed Python boolean selection must produce a boxed result, got {:?}",
        result.repr
    );
    assert!(
        crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
            tir_op.opcode
        ),
        "boxed Python boolean selection must mint an owned selected operand"
    );

    emit_get_boxed_for_repr(ctx, lhs);
    ctx.emit_runtime_call("is_truthy");
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Ne);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    if is_and {
        emit_get_boxed_for_repr(ctx, rhs);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
    }
    ctx.instructions.push(Instruction::Else);
    if is_and {
        emit_get_boxed_for_repr(ctx, lhs);
    } else {
        emit_get_boxed_for_repr(ctx, rhs);
    }
    ctx.instructions.push(Instruction::End);
    ctx.instructions
        .push(Instruction::LocalTee(ctx.get_local(dst)));
    ctx.emit_runtime_call("inc_ref_obj");
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
    // Same per-arm operand push as `emit_lir_binary_arith` (finding #3): the
    // homogeneous unboxed arms push raw operands; the boxed runtime-dispatch arm
    // must push BOTH operands NaN-boxed, so a proven `RawI64Safe` operand sharing
    // a compare with an unproven `DynBox` operand is boxed before the call.
    match (ctx.repr_of(lhs), ctx.repr_of(rhs)) {
        (LirRepr::I64, LirRepr::I64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match cmp {
                CmpOp::Eq => Instruction::I64Eq,
                CmpOp::Ne => Instruction::I64Ne,
                CmpOp::Lt => Instruction::I64LtS,
                CmpOp::Le => Instruction::I64LeS,
                CmpOp::Gt => Instruction::I64GtS,
                CmpOp::Ge => Instruction::I64GeS,
            });
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
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
            // Boxed dispatch through the NAMED runtime comparison (keeps the
            // function in the LIR fast lane; Call(0) is the bail sentinel).
            // The helper returns a NaN-BOXED bool (i64); a Bool1 destination
            // local is i32, so extract bit 0 and wrap.
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(match cmp {
                CmpOp::Eq => "eq",
                CmpOp::Ne => "ne",
                CmpOp::Lt => "lt",
                CmpOp::Le => "le",
                CmpOp::Gt => "gt",
                CmpOp::Ge => "ge",
            });
            if op.result_values[0].repr == LirRepr::Bool1 {
                ctx.instructions.push(Instruction::I64Const(1));
                ctx.instructions.push(Instruction::I64And);
                ctx.instructions.push(Instruction::I32WrapI64);
            }
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
    let instr = match bw {
        BitwiseOp::And => Instruction::I64And,
        BitwiseOp::Or => Instruction::I64Or,
        BitwiseOp::Xor => Instruction::I64Xor,
    };
    // `&`/`|`/`^` never overflow and the raw machine op is always defined for
    // any i64 operands, so the operand proof alone authorizes the raw lane
    // (require_raw_result = false) — no perf regression on the proven-operand
    // bitwise path.
    emit_lir_i64_binary_or_boxed(
        ctx,
        tir_op.operands[0],
        tir_op.operands[1],
        op.result_values[0].id,
        op.result_values[0].repr,
        instr,
        false,
    );
}

/// Emit a bare two-operand `i64` machine instruction (`I64And`/`I64Shl`/…)
/// **only** when both operands are proven raw-i64 carriers (`LirRepr::I64`).
/// Otherwise — a `MaybeBigInt`/`DynBox` operand — dispatch through the runtime
/// helper with both operands NaN-boxed (finding #3: a bare `I64*` on a NaN-boxed
/// word is a miscompile). On the production fast path the runtime `Call(0)` bails
/// to the IntFastLane-guarded slow path; on the generic path it is the resolved
/// runtime dispatch.
///
/// `require_raw_result` additionally gates the raw lane on the **result** being a
/// raw carrier (`LirRepr::I64`). `I64And`/`I64Or`/`I64Xor` never overflow and the
/// machine op is always defined, so they pass `false` (operand proof suffices).
/// `I64Shl`/`I64ShrS` MUST pass `true`: a `<<` result can exceed i64, and a raw
/// shift whose count is `>= 64` is a silent wrong-value mask-mod-64 on wasm. The
/// shared value-range seed grants a shift result `RawI64Safe` (→ `LirRepr::I64`)
/// ONLY when its count is range-proven in `[0, 63]` and the result fits the inline
/// window, so gating on the result repr here routes every other shift to the
/// boxed `molt_lshift`/`molt_rshift` runtime (BigInt- and exception-correct),
/// exactly as the LLVM `emit_bitwise` gate and the native backend do.
#[cfg(feature = "wasm-backend")]
fn emit_lir_i64_binary_or_boxed(
    ctx: &mut LirLowerCtx,
    lhs: ValueId,
    rhs: ValueId,
    dst: ValueId,
    dst_repr: LirRepr,
    bare_i64_instr: Instruction<'static>,
    require_raw_result: bool,
) {
    let raw_lane_ok = ctx.repr_of(lhs) == LirRepr::I64
        && ctx.repr_of(rhs) == LirRepr::I64
        && (!require_raw_result || dst_repr == LirRepr::I64);
    if raw_lane_ok {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(bare_i64_instr);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.instructions.push(Instruction::Call(0));
    }
    ctx.emit_set(dst);
}

#[cfg(feature = "wasm-backend")]
fn emit_box_inline_i64(ctx: &mut LirLowerCtx, src: ValueId) {
    ctx.emit_get(src);
    ctx.instructions.push(Instruction::I64Const(INT_MASK));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions.push(Instruction::I64Const(QNAN | TAG_INT));
    ctx.instructions.push(Instruction::I64Or);
}

/// Box a raw-i64 carrier OVERFLOW-SAFELY: fits-inline-47 fast path (the
/// band/bor NaN box) with a cold `int_from_i64` runtime call (heap BigInt)
/// for values outside `[-2^46, 2^46)`.
///
/// This is the wasm twin of native `ensure_boxed_overflow_safe` /
/// `box_raw_i64_value_overflow_safe` and the LLVM
/// `box_i64_overflow_safe_with_builder`. It exists because raw-i64 carriers may
/// be full-i64 `RawI64FullDeopt` checked results; the unchecked
/// [`emit_box_inline_i64`] silently truncates mod 2^47 -- the
/// silent-integer-miscompile class -- and is only sound when the value-range
/// analysis proves the inline window.
#[cfg(feature = "wasm-backend")]
fn emit_box_i64_overflow_safe(ctx: &mut LirLowerCtx, src: ValueId) {
    // fits = (src + 2^46) <u 2^47
    ctx.emit_get(src);
    ctx.instructions.push(Instruction::I64Const(1 << 46));
    ctx.instructions.push(Instruction::I64Add);
    ctx.instructions.push(Instruction::I64Const(1 << 47));
    ctx.instructions.push(Instruction::I64LtU);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    ctx.emit_get(src);
    ctx.instructions.push(Instruction::I64Const(INT_MASK));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions.push(Instruction::I64Const(QNAN | TAG_INT));
    ctx.instructions.push(Instruction::I64Or);
    ctx.instructions.push(Instruction::Else);
    ctx.emit_get(src);
    ctx.emit_runtime_call("int_from_i64");
    ctx.instructions.push(Instruction::End);
}

#[cfg(feature = "wasm-backend")]
fn emit_box_none(ctx: &mut LirLowerCtx) {
    ctx.instructions
        .push(Instruction::I64Const(QNAN | TAG_NONE));
}

#[cfg(feature = "wasm-backend")]
fn emit_return_boxed_i64(ctx: &mut LirLowerCtx, value: ValueId) {
    match ctx.repr_of(value) {
        // OVERFLOW-SAFE: return-value boxing of a full-range raw carrier
        // (see emit_get_boxed_for_repr).
        LirRepr::I64 => emit_box_i64_overflow_safe(ctx, value),
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_get(value),
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
                LirRepr::DynBox | LirRepr::Ref64 => {
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
        LirTerminator::StateDispatch { .. } => {
            // `StateDispatch` only appears in generator/coroutine `_poll`
            // bodies, which on WASM are lowered by the SimpleIR relooper path
            // (`wasm.rs`), NOT this LIR fast path: `prepare_lir_wasm_fast_output`
            // is gated to `____molt_globals_builtin__` functions only
            // (`is_production_lir_wasm_fast_path_name`).  Reaching here means a
            // state-machine body was incorrectly routed through the LIR fast
            // lane — fail loud rather than emit a dispatch that silently ignores
            // the saved frame state.
            panic!(
                "StateDispatch terminator reached the LIR→WASM fast lane in '{}'; \
                 generator/coroutine _poll bodies must lower via the SimpleIR relooper",
                ctx.func.name
            );
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
        if i + 1 < instructions.len()
            && let (Instruction::LocalSet(set_idx), Instruction::LocalGet(get_idx)) =
                (&instructions[i], &instructions[i + 1])
            && set_idx == get_idx
        {
            out.push(Instruction::LocalTee(*set_idx));
            i += 2;
            continue;
        }
        // Pattern 2: i64.const 0; i64.eq -> i64.eqz (test for zero)
        if i + 1 < instructions.len()
            && let (Instruction::I64Const(0), Instruction::I64Eq) =
                (&instructions[i], &instructions[i + 1])
        {
            out.push(Instruction::I64Eqz);
            i += 2;
            continue;
        }
        // Pattern 3: i32.const 0; i32.eq -> i32.eqz
        if i + 1 < instructions.len()
            && let (Instruction::I32Const(0), Instruction::I32Eq) =
                (&instructions[i], &instructions[i + 1])
        {
            out.push(Instruction::I32Eqz);
            i += 2;
            continue;
        }
        // Pattern 4: i64.const 1; i64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len()
            && let (Instruction::I64Const(1), Instruction::I64Mul) =
                (&instructions[i], &instructions[i + 1])
        {
            // Value already on stack; skip the const+mul.
            i += 2;
            continue;
        }
        // Pattern 5: i64.const 0; i64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len()
            && let (Instruction::I64Const(0), Instruction::I64Add) =
                (&instructions[i], &instructions[i + 1])
        {
            i += 2;
            continue;
        }
        // Pattern 6: i64.const 0; i64.sub -> (eliminated, sub 0 is identity)
        if i + 1 < instructions.len()
            && let (Instruction::I64Const(0), Instruction::I64Sub) =
                (&instructions[i], &instructions[i + 1])
        {
            i += 2;
            continue;
        }
        // Pattern 7: i64.const -1; i64.xor -> (equivalent to bit_not, but keep xor)
        // Not folded: -1 xor is the canonical bit_not, no simpler form exists.

        // Pattern 8: f64.const 0.0; f64.add -> (eliminated, add 0 is identity)
        if i + 1 < instructions.len()
            && let (Instruction::F64Const(z), Instruction::F64Add) =
                (&instructions[i], &instructions[i + 1])
            && f64::from(*z) == 0.0
        {
            i += 2;
            continue;
        }
        // Pattern 9: f64.const 1.0; f64.mul -> (eliminated, multiply by 1 is identity)
        if i + 1 < instructions.len()
            && let (Instruction::F64Const(one), Instruction::F64Mul) =
                (&instructions[i], &instructions[i + 1])
            && f64::from(*one) == 1.0
        {
            i += 2;
            continue;
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
    use crate::repr::Repr;
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
    fn binding_alias_copy_retains_before_forwarding_bits() {
        let mut func = TirFunction::new(
            "binding_alias_copy".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let alias = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![ValueId(0)],
            results: vec![alias],
            attrs: {
                let mut m = AttrDict::new();
                m.insert(
                    "_original_kind".into(),
                    AttrValue::Str("binding_alias".into()),
                );
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![alias],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"inc_ref_obj"),
            "binding_alias Copy must retain its forwarded source: {:?}",
            output.runtime_calls
        );
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
    fn lir_fast_lane_dec_ref_emits_named_runtime_call() {
        let mut func = TirFunction::new("drop_ref".into(), vec![], TirType::None);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![owned],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"dec_ref_obj"),
            "WASM LIR fast lane must consume shared DecRef through dec_ref_obj; got {:?}",
            output.runtime_calls
        );
        let placeholders = output
            .instructions
            .iter()
            .filter(|i| matches!(i, Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)))
            .count();
        assert_eq!(
            placeholders,
            output.runtime_calls.len(),
            "named-call placeholders must pair 1:1 with runtime_calls entries"
        );
    }

    #[test]
    fn lir_fast_lane_del_boundary_emits_named_dec_ref_runtime_call() {
        let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::None);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![owned],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DelBoundary,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"dec_ref_obj"),
            "WASM LIR fast lane must consume DelBoundary through dec_ref_obj; got {:?}",
            output.runtime_calls
        );
        let placeholders = output
            .instructions
            .iter()
            .filter(|i| matches!(i, Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)))
            .count();
        assert_eq!(
            placeholders,
            output.runtime_calls.len(),
            "named-call placeholders must pair 1:1 with runtime_calls entries"
        );
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
    fn bool1_and_stays_raw_without_selected_ref_retain() {
        let mut func = TirFunction::new(
            "and_bool1".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::And,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::I32And)),
            "raw Bool1 and must stay a native i32.and: {:?}",
            output.instructions
        );
        assert!(
            !output.runtime_calls.contains(&"inc_ref_obj"),
            "raw Bool1 and must not retain a selected boxed operand: {:?}",
            output.runtime_calls
        );
    }

    #[test]
    fn dynbox_or_retains_selected_operand_result() {
        let mut func = TirFunction::new(
            "or_dynbox".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Or,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"is_truthy"),
            "boxed or must test Python truthiness: {:?}",
            output.runtime_calls
        );
        assert!(
            output.runtime_calls.contains(&"inc_ref_obj"),
            "boxed or must retain the selected borrowed operand result: {:?}",
            output.runtime_calls
        );
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::LocalTee(_))),
            "boxed or must tee the selected result before retaining it: {:?}",
            output.instructions
        );
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

    // -----------------------------------------------------------------------
    // Phase 1: mixed-repr integer arithmetic (the delicate correctness core)
    // -----------------------------------------------------------------------

    /// Build `f(a: int, b: int) -> int = a + b` with two i64-typed params and a
    /// single Add. The caller supplies the `Repr` override.
    fn make_add_two_params_func() -> TirFunction {
        let mut func = TirFunction::new(
            "add_two_params".into(),
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
        func
    }

    fn make_add_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
        let mut func = TirFunction::new("add_two_consts".into(), vec![], TirType::I64);
        let lhs_id = func.fresh_value();
        let rhs_id = func.fresh_value();
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(value));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![id],
                attrs,
                source_span: None,
            });
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![lhs_id, rhs_id],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };
        func
    }

    #[test]
    fn generic_tir_to_wasm_uses_value_repr_not_type_floor_for_int_params() {
        let func = make_add_two_params_func();
        let output = lower_tir_to_wasm(&func);

        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call(_))),
            "unproven int params must lower through boxed runtime dispatch, not a type-floor raw i64 op"
        );
        for (idx, inst) in output.instructions.iter().enumerate() {
            if matches!(inst, Instruction::I64Add) {
                assert!(
                    matches!(
                        output.instructions.get(idx + 1),
                        Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                    ),
                    "generic lower_tir_to_wasm emitted a bare operand i64.add at {idx}"
                );
            }
        }
    }

    /// Full-range raw carriers must box through the OVERFLOW-SAFE path: a
    /// full-range raw value without an inline-window range proof (the CheckedAdd
    /// sum / overflow_peel accumulator case) boxed at a runtime-call or
    /// return site must emit the fits-check + named `int_from_i64` cold
    /// call, never the bare 47-bit mask (which truncates mod 2^47).
    #[test]
    fn full_range_raw_carrier_boxes_overflow_safe_with_named_call() {
        let func = make_add_two_params_func();
        // The values are raw full-deopt carriers, and the value-range proof for
        // opaque params does not prove the 47-bit inline window. The checked
        // triple is therefore refused and the add takes the boxed runtime path,
        // boxing both raw operands through the overflow-safe cold call.
        let repr: HashMap<ValueId, Repr> = HashMap::from([
            (ValueId(0), Repr::RawI64FullDeopt),
            (ValueId(1), Repr::RawI64FullDeopt),
            (ValueId(2), Repr::RawI64FullDeopt),
        ]);
        let vr = crate::representation_plan::value_range_for(&func);
        let lir =
            super::super::lower_to_lir::lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
        // Triple refused without an inline-window proof: no op carries
        // lir.checked_overflow.
        let has_triple = lir.blocks.values().flat_map(|b| b.ops.iter()).any(|op| {
            matches!(
                op.tir_op.attrs.get("lir.checked_overflow"),
                Some(crate::tir::ops::AttrValue::Bool(true))
            )
        });
        assert!(
            !has_triple,
            "checked-i64 triple must be refused without a value-range proof"
        );

        let output = lower_lir_to_wasm(&lir);
        // The raw operands are boxed overflow-safely: the cold arm is a
        // NAMED int_from_i64 runtime call recorded in runtime_calls.
        assert!(
            output
                .runtime_calls
                .iter()
                .filter(|name| **name == "int_from_i64")
                .count()
                >= 2,
            "both full-range raw operands must box through the int_from_i64 cold path; got {:?}",
            output.runtime_calls
        );
        // And the placeholder pairing invariant holds: one placeholder per
        // recorded name.
        let placeholders = output
            .instructions
            .iter()
            .filter(|i| matches!(i, Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)))
            .count();
        assert_eq!(
            placeholders,
            output.runtime_calls.len(),
            "named-call placeholders must pair 1:1 with runtime_calls entries"
        );
    }

    /// Count occurrences of the inline-int NaN-box packing
    /// (`emit_box_inline_i64`): `i64.const INT_MASK; i64.and; i64.const
    /// (QNAN|TAG_INT); i64.or`. This is how a proven raw-i64 operand is boxed
    /// before a runtime helper call in the mixed-repr boxed arm.
    fn count_inline_int_boxes(instrs: &[Instruction<'static>]) -> usize {
        instrs
            .windows(4)
            .filter(|w| {
                matches!(w[0], Instruction::I64Const(m) if m == INT_MASK)
                    && matches!(w[1], Instruction::I64And)
                    && matches!(w[2], Instruction::I64Const(t) if t == (QNAN | TAG_INT))
                    && matches!(w[3], Instruction::I64Or)
            })
            .count()
    }

    /// THE regression guard for finding #3: an integer `add` with one proven
    /// `RawI64Safe` operand and one `MaybeBigInt` operand must NOT emit a bare
    /// `i64.add` (the unsound op on a NaN-boxed word). Both operands must be
    /// NaN-boxed before the runtime `Call` (`molt_add`): the proven operand via
    /// the inline-int box, the unproven operand passed through already-boxed.
    #[test]
    fn mixed_repr_int_add_boxes_both_operands_no_bare_i64_add() {
        let func = make_add_two_params_func();
        // a (ValueId 0) is proven RawI64Safe; b (ValueId 1) is an unproven
        // MaybeBigInt; the result (ValueId 2) is therefore MaybeBigInt too (it
        // cannot be proven from an unproven operand). This forces the generic
        // boxed path (NOT the checked-overflow triple, which requires all three
        // to be RawI64Safe).
        let repr: HashMap<ValueId, Repr> = HashMap::from([
            (ValueId(0), Repr::RawI64Safe),
            (ValueId(1), Repr::MaybeBigInt),
            (ValueId(2), Repr::MaybeBigInt),
        ]);
        let lir = lower_function_to_lir_with_inline_proof(
            &func,
            &repr,
            &crate::representation_plan::value_range_for(&func),
        );
        let output = lower_lir_to_wasm(&lir);

        // No bare OPERAND i64.add: a raw machine add on a possibly-heap-BigInt
        // operand is exactly the truncation bug-class this phase makes
        // un-emittable. The overflow-safe box legitimately contains an
        // `i64.add` (the `src + 2^46` fits-inline bias), so the precise
        // invariant is: every I64Add in the stream is a fits-check add —
        // immediately followed by the `2^47` window-limit const — never an
        // operand-pair add.
        for (idx, inst) in output.instructions.iter().enumerate() {
            if matches!(inst, Instruction::I64Add) {
                assert!(
                    matches!(
                        output.instructions.get(idx + 1),
                        Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                    ),
                    "mixed-repr add emitted a bare operand i64.add at {idx} (operand may be a heap BigInt)"
                );
            }
        }
        // Runtime dispatch through the boxed helper (placeholder Call(0)).
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call(_))),
            "mixed-repr add must dispatch through the boxed runtime helper"
        );
        // The proven RawI64Safe operand `a` is NaN-boxed (inline-int box) before
        // the call. (`b` is already a DynBox word and passes through, so exactly
        // one inline-int box is emitted for the operands of this add.)
        assert!(
            count_inline_int_boxes(&output.instructions) >= 1,
            "the proven raw-i64 operand must be NaN-boxed before the runtime call"
        );
    }

    /// The perf-preservation direction: when BOTH operands are proven
    /// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
    /// triple), and no boxed runtime `Call` is needed for the add itself.
    #[test]
    fn proven_raw_i64_add_still_emits_native_i64_add() {
        let func = make_add_two_consts_func(20, 22);
        let output = lower_tir_to_wasm(&func);

        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::I64Add)),
            "proven raw-i64 add must emit native i64.add"
        );
        assert!(
            !output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call(0))),
            "range-proven const add must not dispatch through the boxed helper"
        );
    }

    /// On the production boxed-i64 ABI path, a function whose integer params are
    /// proven `RawI64Safe` keeps the fast path (entry args lower to `I64`); a
    /// `MaybeBigInt` param forces the entry arg to `DynBox`, so the boxed-i64 ABI
    /// (which requires all-`I64` entry args) bails to `None` — falling back to
    /// the IntFastLane-guarded slow path. This is the structural gate that keeps
    /// the unsound bare op un-emittable for unproven ints.
    #[test]
    fn boxed_i64_abi_bails_when_param_is_maybe_bigint() {
        let proven = make_add_two_consts_func(20, 22);
        assert!(
            lower_tir_to_wasm_boxed_i64_abi(&proven).is_some(),
            "range-proven raw-i64 values keep the boxed-i64 ABI fast path"
        );

        let unproven = make_add_two_params_func();
        assert!(
            lower_tir_to_wasm_boxed_i64_abi(&unproven).is_none(),
            "a MaybeBigInt param must bail the boxed-i64 ABI (entry arg is DynBox)"
        );
    }
}
