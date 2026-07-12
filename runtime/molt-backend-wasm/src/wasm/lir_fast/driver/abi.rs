use super::super::lir_context::LirLowerCtx;
#[cfg(any(test, feature = "test-util"))]
use super::super::lir_context::lir_repr_to_val;
use super::super::lir_control::LirReturnAbi;
use molt_codegen_abi::INT_SHIFT as INT_SHIFT_BITS;
use molt_tir::tir::lir::{LirFunction, LirRepr};
use wasm_encoder::{Instruction, ValType};

#[derive(Clone, Copy)]
pub(super) enum LirWasmAbi {
    #[cfg(any(test, feature = "test-util"))]
    Native,
    BoxedI64,
}

pub(super) struct LirWasmAbiPlan {
    pub(super) param_types: Vec<ValType>,
    pub(super) result_types: Vec<ValType>,
    pub(super) ctx_local_base: u32,
    pub(super) local_decl_start: u32,
    pub(super) return_abi: LirReturnAbi,
}

impl LirWasmAbi {
    pub(super) fn plan(self, func: &LirFunction) -> Option<LirWasmAbiPlan> {
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
                if func.return_types.len() != 1 {
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

    pub(super) fn emit_entry_prologue(self, ctx: &mut LirLowerCtx) {
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
