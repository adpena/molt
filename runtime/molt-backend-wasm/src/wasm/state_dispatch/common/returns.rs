use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::DispatchMode;
use super::super::plan::NonLinearDispatchLocals;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_arena_free(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
) {
    if let Some(arena_idx) = op_emitter.arena_local() {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(
            func,
            op_emitter.reloc_enabled,
            op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ArenaFree],
        );
    }
}

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
        emit_arena_free(func, op_emitter);
        func.instruction(&Instruction::LocalGet(locals.return_local));
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    } else {
        emit_arena_free(func, op_emitter);
        op_emitter.const_cache().emit_none(func);
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    }
}
