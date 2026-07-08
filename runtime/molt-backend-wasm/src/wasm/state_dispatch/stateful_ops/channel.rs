use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use super::pending::{emit_prepare_pending_yield, pending_encoded_target};
use crate::OpIR;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_pending;
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_chan_send_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals()[&args[0]];
    let val = op_emitter.locals()[&args[1]];
    let pending_state = op_emitter.locals()[&args[2]];
    let pending_target_idx = pending_encoded_target(plan, &args[2]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals()[op.out.as_ref().unwrap()];
    emit_prepare_pending_yield(
        func,
        op_emitter,
        locals,
        idx,
        pending_state,
        pending_target_idx,
    );
    func.instruction(&Instruction::LocalGet(chan));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ChanSend],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

pub(in crate::wasm::state_dispatch) fn emit_chan_recv_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals()[&args[0]];
    let pending_state = op_emitter.locals()[&args[1]];
    let pending_target_idx = pending_encoded_target(plan, &args[1]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals()[op.out.as_ref().unwrap()];
    emit_prepare_pending_yield(
        func,
        op_emitter,
        locals,
        idx,
        pending_state,
        pending_target_idx,
    );
    func.instruction(&Instruction::LocalGet(chan));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ChanRecv],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

fn emit_finish_channel_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    out: u32,
    next_state_id: i64,
    depth: u32,
) {
    func.instruction(&Instruction::LocalSet(out));
    func.instruction(&Instruction::LocalGet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::Return);
    func.instruction(&Instruction::End);
    super::super::common::emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
    );
    func.instruction(&Instruction::Br(depth));
}
