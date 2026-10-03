use super::super::plan::NonLinearDispatchLocals;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_obj_set_state_arg(
    func: &mut Function,
    locals: NonLinearDispatchLocals,
) {
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
}
