use super::common::{emit_obj_set_state_arg, emit_pending_state_value};
use super::*;

pub(super) fn emit_state_transition(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let future = op_emitter.locals[&args[0]];
    let (slot_bits, pending_state) = if args.len() == 2 {
        (None, op_emitter.locals[&args[1]])
    } else {
        (
            Some(op_emitter.locals[&args[1]]),
            op_emitter.locals[&args[2]],
        )
    };
    let pending_state_name = if args.len() == 2 { &args[1] } else { &args[2] };
    let pending_target_idx = pending_encoded_target(plan, pending_state_name);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
    let next_block = idx + 1;
    let return_depth = depth + 2;

    func.instruction(&Instruction::I64Const(next_block as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    emit_pending_state_value(func, pending_state, pending_target_idx);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::LocalGet(future));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["future_poll"],
    );
    func.instruction(&Instruction::LocalSet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::LocalSet(locals.return_local));
    func.instruction(&Instruction::LocalGet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(future));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["handle_resolve"],
    );
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["sleep_register"],
    );
    func.instruction(&Instruction::Drop);
    func.instruction(&Instruction::Br(return_depth));
    func.instruction(&Instruction::End);
    if let Some(slot) = slot_bits {
        emit_obj_set_state_arg(func, locals);
        func.instruction(&Instruction::LocalGet(slot));
        func.instruction(&Instruction::I64Const(INT_MASK as i64));
        func.instruction(&Instruction::I64And);
        func.instruction(&Instruction::LocalGet(out));
        emit_call(
            func,
            op_emitter.reloc_enabled,
            op_emitter.import_ids["closure_store"],
        );
        func.instruction(&Instruction::Drop);
    }
    emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::Br(depth));
}

pub(super) fn emit_state_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
) {
    let args = op.args.as_ref().unwrap();
    let pair = op_emitter.locals[&args[0]];
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
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::LocalGet(pair));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["inc_ref_obj"],
    );
    func.instruction(&Instruction::LocalGet(pair));
    func.instruction(&Instruction::Return);
}

pub(super) fn emit_chan_send_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals[&args[0]];
    let val = op_emitter.locals[&args[1]];
    let pending_state = op_emitter.locals[&args[2]];
    let pending_target_idx = pending_encoded_target(plan, &args[2]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
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
        op_emitter.import_ids["chan_send"],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

pub(super) fn emit_chan_recv_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals[&args[0]];
    let pending_state = op_emitter.locals[&args[1]];
    let pending_target_idx = pending_encoded_target(plan, &args[1]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
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
        op_emitter.import_ids["chan_recv"],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

fn emit_prepare_pending_yield(
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
        op_emitter.import_ids["obj_set_state"],
    );
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
    emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::Br(depth));
}

fn pending_encoded_target(plan: &NonLinearDispatchPlan, pending_state_name: &str) -> Option<i64> {
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
