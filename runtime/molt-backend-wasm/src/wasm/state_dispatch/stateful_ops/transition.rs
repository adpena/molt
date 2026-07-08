use super::super::common::{emit_obj_set_state_arg, emit_pending_state_value};
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use super::pending::pending_encoded_target;
use crate::OpIR;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm_binary::emit_call;
use crate::wasm_values::{INT_MASK, box_pending};
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_state_transition(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let future = op_emitter.locals()[&args[0]];
    let (slot_bits, pending_state) = if args.len() == 2 {
        (None, op_emitter.locals()[&args[1]])
    } else {
        (
            Some(op_emitter.locals()[&args[1]]),
            op_emitter.locals()[&args[2]],
        )
    };
    let pending_state_name = if args.len() == 2 { &args[1] } else { &args[2] };
    let pending_target_idx = pending_encoded_target(plan, pending_state_name);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals()[op.out.as_ref().unwrap()];
    let next_block = idx + 1;
    let return_depth = depth + 2;

    func.instruction(&Instruction::I64Const(next_block as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    emit_pending_state_value(func, pending_state, pending_target_idx);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
    );
    func.instruction(&Instruction::LocalGet(future));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FuturePoll],
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
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
    );
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::SleepRegister],
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
            op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClosureStore],
        );
        func.instruction(&Instruction::Drop);
    }
    emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
    );
    func.instruction(&Instruction::Br(depth));
}
