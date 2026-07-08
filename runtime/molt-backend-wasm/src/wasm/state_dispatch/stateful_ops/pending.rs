use super::super::common::{emit_obj_set_state_arg, emit_pending_state_value};
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_prepare_pending_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    idx: usize,
    pending_state: u32,
    pending_target_idx: Option<i64>,
) {
    func.instruction(&Instruction::I64Const((idx + 1) as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    emit_pending_state_value(func, pending_state, pending_target_idx);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
    );
}

pub(super) fn pending_encoded_target(
    plan: &NonLinearDispatchPlan,
    pending_state_name: &str,
) -> Option<i64> {
    let resume = plan
        .state_resume
        .as_ref()
        .expect("state resume maps missing for stateful wasm");
    resume
        .const_ints
        .get(pending_state_name)
        .and_then(|state_id| resume.state_map.get(state_id).copied())
        .map(|idx| !(idx as i64))
}
