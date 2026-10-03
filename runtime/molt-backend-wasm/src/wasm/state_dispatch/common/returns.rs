use super::super::super::op_loop::WasmFunctionEmitContext;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_dispatch_trailing_return(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
) {
    // Every emitted block now has an explicit successor or activation return.
    // Do not manufacture a loop backedge for unreachable linear fallthrough.
    func.instruction(&Instruction::Unreachable);
    func.instruction(&Instruction::End);
    op_emitter.const_cache().emit_none(func);
}
