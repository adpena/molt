use super::*;

pub(super) fn emit_state_machine_local_state_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "state_switch" => {}
        "state_transition" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let slot_bits = args.get(1).map(|name| locals[name]);
            let out = locals[op.out.as_ref().unwrap()];
            let self_ptr = locals["__molt_tmp0"];
            func.instruction(&Instruction::LocalGet(0));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(self_ptr));
            func.instruction(&Instruction::LocalGet(self_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_poll"]);
            func.instruction(&Instruction::LocalSet(out));
            if let Some(slot) = slot_bits {
                func.instruction(&Instruction::LocalGet(self_ptr));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(slot));
                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                func.instruction(&Instruction::I64And);
                func.instruction(&Instruction::LocalGet(out));
                emit_call(func, reloc_enabled, import_ids["closure_store"]);
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::I64Const(box_pending()));
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(self_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            emit_call(func, reloc_enabled, import_ids["sleep_register"]);
            func.instruction(&Instruction::Drop);
            func.instruction(&Instruction::I64Const(box_pending()));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        }
        _ => return false,
    }
    true
}
