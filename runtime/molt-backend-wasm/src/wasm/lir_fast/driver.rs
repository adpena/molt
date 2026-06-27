#[cfg(any(test, feature = "test-util"))]
use super::lir_context::lir_repr_to_val;
use super::lir_context::{LirLowerCtx, lir_terminator_successors};
use super::lir_control::{LirReturnAbi, emit_lir_terminator, emit_lir_terminator_multiblock};
use super::lir_ops::emit_lir_block_ops;
use super::peephole::peephole_set_get_to_tee;
use crate::wasm::body::WasmBody;
use molt_codegen_abi::INT_SHIFT as INT_SHIFT_BITS;
use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::function::TirFunction;
use molt_tir::tir::lir::{LirFunction, LirRepr};
#[cfg(test)]
use molt_tir::tir::lower_to_lir::lower_function_to_lir;
use molt_tir::tir::lower_to_lir::lower_function_to_lir_with_inline_proof;
use molt_tir::tir::values::ValueId;
use std::collections::HashMap;
use wasm_encoder::{BlockType, Instruction, ValType};

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` -> `wasm i64`, `F64` -> `wasm f64`, `DynBox` -> runtime call.
#[cfg(test)]
pub(crate) fn lower_tir_to_wasm(func: &TirFunction) -> WasmBody {
    // The generic path derives carriers from the same pure-TIR `repr_by_value`
    // authority as the boxed-i64 ABI path and LLVM. Semantic `I64` alone is not
    // a raw machine carrier; unproven ints lower as DynBox/boxed runtime values,
    // while Bool/F64 and range-proven ints keep their scalar lanes.
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm(&lir)
}

#[cfg(any(test, feature = "test-util"))]
pub(crate) fn lower_lir_to_wasm(func: &LirFunction) -> WasmBody {
    lower_lir_to_wasm_with_abi(func, LirWasmAbi::Native)
        .expect("native LIR-to-WASM lowering is total for well-formed LIR")
}

#[cfg(test)]
pub(crate) fn lower_tir_to_wasm_boxed_i64_abi(func: &TirFunction) -> Option<WasmBody> {
    let vr = crate::representation_plan::value_range_for(func);
    let repr = crate::representation_plan::repr_by_value_for(func, Some(&vr));
    lower_tir_to_wasm_boxed_i64_abi_with_proof(func, &repr, &vr)
}

/// Boxed-i64 WASM ABI lowering with the value-range proof explicitly paired to
/// the value-keyed Repr map. The production WASM fast lane uses this entry so
/// full-range raw carriers can never take the 47-bit-window checked-i64 triple.
#[cfg(feature = "wasm-backend")]
pub(crate) fn lower_tir_to_wasm_boxed_i64_abi_with_proof(
    func: &TirFunction,
    repr: &HashMap<ValueId, crate::repr::Repr>,
    inline_proof: &crate::tir::passes::value_range::ValueRangeResult,
) -> Option<WasmBody> {
    let lir = lower_function_to_lir_with_inline_proof(func, repr, inline_proof);
    lower_lir_to_wasm_boxed_i64_abi(&lir)
}

#[cfg(feature = "wasm-backend")]
pub(crate) fn lower_lir_to_wasm_boxed_i64_abi(func: &LirFunction) -> Option<WasmBody> {
    lower_lir_to_wasm_with_abi(func, LirWasmAbi::BoxedI64)
}

#[derive(Clone, Copy)]
enum LirWasmAbi {
    #[cfg(any(test, feature = "test-util"))]
    Native,
    BoxedI64,
}

struct LirWasmAbiPlan {
    param_types: Vec<ValType>,
    result_types: Vec<ValType>,
    ctx_local_base: u32,
    local_decl_start: u32,
    return_abi: LirReturnAbi,
}

impl LirWasmAbi {
    fn plan(self, func: &LirFunction) -> Option<LirWasmAbiPlan> {
        match self {
            #[cfg(any(test, feature = "test-util"))]
            LirWasmAbi::Native => {
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
                let local_decl_start = param_types.len() as u32;
                Some(LirWasmAbiPlan {
                    param_types,
                    result_types,
                    ctx_local_base: 0,
                    local_decl_start,
                    return_abi: LirReturnAbi::Native,
                })
            }
            LirWasmAbi::BoxedI64 => {
                if func
                    .param_types
                    .iter()
                    .any(|ty| *ty != crate::tir::types::TirType::I64)
                {
                    return None;
                }
                if func.return_types.len() != 1
                    || func.return_types[0] != crate::tir::types::TirType::I64
                {
                    return None;
                }
                let entry = func.blocks.get(&func.entry_block)?;
                if entry.args.iter().any(|arg| arg.repr != LirRepr::I64) {
                    return None;
                }

                let param_count = entry.args.len() as u32;
                Some(LirWasmAbiPlan {
                    param_types: vec![ValType::I64; param_count as usize],
                    result_types: vec![ValType::I64],
                    ctx_local_base: param_count,
                    local_decl_start: param_count,
                    return_abi: LirReturnAbi::BoxedI64,
                })
            }
        }
    }

    fn emit_entry_prologue(self, ctx: &mut LirLowerCtx) {
        match self {
            #[cfg(any(test, feature = "test-util"))]
            LirWasmAbi::Native => {}
            LirWasmAbi::BoxedI64 => {
                if let Some(entry) = ctx.func.blocks.get(&ctx.func.entry_block) {
                    for (idx, arg) in entry.args.iter().enumerate() {
                        ctx.instructions.push(Instruction::LocalGet(idx as u32));
                        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
                        ctx.instructions.push(Instruction::I64Shl);
                        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
                        ctx.instructions.push(Instruction::I64ShrS);
                        ctx.emit_set(arg.id);
                    }
                }
            }
        }
    }
}

fn lower_lir_to_wasm_with_abi(func: &LirFunction, abi: LirWasmAbi) -> Option<WasmBody> {
    let plan = abi.plan(func)?;
    let mut ctx = LirLowerCtx::new_with_local_base(func, plan.ctx_local_base);
    ctx.allocate_function_locals();
    abi.emit_entry_prologue(&mut ctx);
    emit_lir_function_body(&mut ctx, plan.return_abi);

    ctx.instructions.push(Instruction::End);
    let locals = ctx.local_declarations_after(plan.local_decl_start);
    let instructions = peephole_set_get_to_tee(ctx.instructions);
    Some(WasmBody {
        param_types: plan.param_types,
        result_types: plan.result_types,
        locals,
        ops: instructions.into_vec(),
    })
}

fn emit_lir_function_body(ctx: &mut LirLowerCtx, return_abi: LirReturnAbi) {
    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = ctx.func.blocks.get(&ctx.func.entry_block) {
            emit_lir_block_ops(ctx, block);
            emit_lir_terminator(ctx, &block.terminator, return_abi);
        }
        return;
    }

    let back_edge_targets = compute_back_edge_targets(ctx, &rpo);
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
        if let Some(block) = ctx.func.blocks.get(&bid) {
            emit_lir_block_ops(ctx, block);
            emit_lir_terminator_multiblock(ctx, &block.terminator, num_blocks, return_abi);
        }
        if i < num_blocks - 1 {
            ctx.instructions.push(Instruction::End);
        }
    }
}

fn compute_back_edge_targets(ctx: &LirLowerCtx, rpo: &[BlockId]) -> HashMap<BlockId, bool> {
    let mut targets = HashMap::new();
    for (src_idx, &bid) in rpo.iter().enumerate() {
        if let Some(block) = ctx.func.blocks.get(&bid) {
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
}
