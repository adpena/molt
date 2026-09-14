use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::DispatchMode;
use super::super::plan::NonLinearDispatchLocals;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_dispatch_trailing_return(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
) {
    func.instruction(&Instruction::Br(0));
    func.instruction(&Instruction::End);
    if mode == DispatchMode::Stateful {
        op_emitter.const_cache().emit_none(func);
        func.instruction(&Instruction::LocalSet(locals.return_local));
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::LocalGet(locals.return_local));
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    } else {
        op_emitter.const_cache().emit_none(func);
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    }
}
