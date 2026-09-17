use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

/// Consume the owned boxed value on the machine stack, not just its bits.
pub(super) fn store_owned_result_or_release(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    if let Some(out) = op.out.as_ref() {
        func.instruction(&Instruction::LocalSet(locals[out]));
    } else {
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::DecRefObj],
        );
    }
}

pub(super) fn store_result_or_drop(func: &mut Function, op: &OpIR, locals: &WasmFrameLocals) {
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

pub(super) fn store_non_none_result_or_drop(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
) {
    if let Some(out) = op.out.as_ref()
        && out != "none"
    {
        func.instruction(&Instruction::LocalSet(locals[out]));
    } else {
        func.instruction(&Instruction::Drop);
    }
}
