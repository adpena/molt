use super::super::common::emit_obj_set_state_arg;
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use crate::OpIR;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_state_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
) {
    let args = op.args.as_ref().unwrap();
    let pair = op_emitter.locals()[&args[0]];
    let resume_state_id = op.value.unwrap();
    let resume_encoded = plan
        .state_resume
        .as_ref()
        .and_then(|resume| resume.state_map.get(&resume_state_id).copied())
        .map(|target_idx| !(target_idx as i64));
    func.instruction(&Instruction::I64Const((idx + 1) as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    if let Some(encoded) = resume_encoded {
        func.instruction(&Instruction::I64Const(encoded));
    } else {
        func.instruction(&Instruction::I64Const(resume_state_id));
    }
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
    );
    func.instruction(&Instruction::LocalGet(pair));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
    );
    func.instruction(&Instruction::LocalGet(pair));
    func.instruction(&Instruction::Return);
}
