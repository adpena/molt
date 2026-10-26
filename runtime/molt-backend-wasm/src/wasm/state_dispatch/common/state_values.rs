use crate::wasm::frame_locals::WasmDispatchFrameLocals;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_obj_set_state_arg(
    func: &mut Function,
    locals: WasmDispatchFrameLocals,
) {
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
}
