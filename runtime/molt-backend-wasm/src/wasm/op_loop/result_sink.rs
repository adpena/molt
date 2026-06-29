use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use wasm_encoder::{Function, Instruction};

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
