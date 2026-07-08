use super::super::plan::NonLinearDispatchLocals;
use crate::wasm_values::INT_MASK;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_obj_set_state_arg(
    func: &mut Function,
    locals: NonLinearDispatchLocals,
) {
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
}

pub(in crate::wasm::state_dispatch) fn emit_pending_state_value(
    func: &mut Function,
    pending_state: u32,
    pending_target_idx: Option<i64>,
) {
    if let Some(pending_encoded) = pending_target_idx {
        func.instruction(&Instruction::I64Const(pending_encoded));
    } else {
        func.instruction(&Instruction::LocalGet(pending_state));
        func.instruction(&Instruction::I64Const(INT_MASK as i64));
        func.instruction(&Instruction::I64And);
    }
}
