use super::lir_context::{LirLowerCtx, lir_repr_to_val, lir_terminator_successors};
use super::lir_control::{
    emit_lir_terminator, emit_lir_terminator_boxed_i64_abi, emit_lir_terminator_multiblock,
    emit_lir_terminator_multiblock_boxed_i64_abi,
};
use super::lir_ops::emit_lir_block_ops;
use super::peephole::peephole_set_get_to_tee;
use super::prelude::*;

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
        .any(|ty| *ty != crate::tir::types::TirType::I64)
    {
        return None;
    }
    if func.return_types.len() != 1 || func.return_types[0] != crate::tir::types::TirType::I64 {
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
