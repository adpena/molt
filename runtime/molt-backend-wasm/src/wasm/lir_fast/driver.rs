mod abi;
mod entrypoints;
mod function_body;

use super::lir_context::LirLowerCtx;
use super::peephole::peephole_set_get_to_tee;
use crate::wasm::body::WasmBody;
use abi::LirWasmAbi;
#[cfg(any(test, feature = "test-util"))]
pub(crate) use entrypoints::lower_lir_to_wasm;
#[cfg(feature = "wasm-backend")]
pub(crate) use entrypoints::lower_tir_to_wasm_boxed_i64_abi_with_proof;
#[cfg(test)]
pub(crate) use entrypoints::{lower_tir_to_wasm, lower_tir_to_wasm_boxed_i64_abi};
use function_body::emit_lir_function_body;
use molt_tir::tir::lir::LirFunction;
use wasm_encoder::Instruction;

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
