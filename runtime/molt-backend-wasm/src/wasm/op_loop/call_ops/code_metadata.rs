use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_code_metadata_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "code_new" => emit_code_new(call_ctx, func, op),
        "code_slot_set" => emit_value_then_local_drop_call(call_ctx, func, op, "code_slot_set"),
        "fn_ptr_code_set" => {
            emit_table_local_drop_call(call_ctx, func, op, "fn_ptr_code_set", "fn_ptr_code_set")
        }
        "asyncgen_locals_register" => emit_table_two_local_drop_call(
            call_ctx,
            func,
            op,
            "asyncgen_locals_register",
            "asyncgen_locals_register",
        ),
        "gen_locals_register" => emit_table_two_local_drop_call(
            call_ctx,
            func,
            op,
            "gen_locals_register",
            "gen_locals_register",
        ),
        "code_slots_init" => emit_value_drop_call(call_ctx, func, op, "code_slots_init"),
        "trace_enter_slot" => emit_value_drop_call(call_ctx, func, op, "trace_enter_slot"),
        "trace_exit" => emit_no_arg_drop_call(call_ctx, func, "trace_exit"),
        "line" => emit_value_drop_call(call_ctx, func, op, "trace_set_line"),
        "frame_locals_set" => emit_one_local_drop_call(call_ctx, func, op, "frame_locals_set"),
        _ => return CallOpEmission::NotHandled,
    }
    CallOpEmission::Handled
}

fn emit_code_new(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    for arg in args.iter().take(9) {
        func.instruction(&Instruction::LocalGet(call_ctx.locals[arg]));
    }
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids["code_new"],
    );
    if let Some(out) = op.out.as_ref() {
        let res = call_ctx.locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

fn emit_value_then_local_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import_name: &str,
) {
    let args = op.args.as_ref().unwrap();
    let value = op.value.unwrap_or(0);
    func.instruction(&Instruction::I64Const(value));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}

fn emit_table_local_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    table_context: &str,
    import_name: &str,
) {
    let args = op.args.as_ref().unwrap();
    let func_name = op.s_value.as_ref().unwrap();
    let table_idx = call_ctx.call_site_abi.table_index(func_name, table_context);
    emit_table_index_i64(func, call_ctx.reloc_enabled, table_idx);
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}

fn emit_table_two_local_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    table_context: &str,
    import_name: &str,
) {
    let args = op.args.as_ref().unwrap();
    let func_name = op.s_value.as_ref().unwrap();
    let table_idx = call_ctx.call_site_abi.table_index(func_name, table_context);
    emit_table_index_i64(func, call_ctx.reloc_enabled, table_idx);
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[1]]));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}

fn emit_value_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import_name: &str,
) {
    func.instruction(&Instruction::I64Const(op.value.unwrap_or(0)));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}

fn emit_no_arg_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    import_name: &str,
) {
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}

fn emit_one_local_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import_name: &str,
) {
    let args = op
        .args
        .as_ref()
        .expect("one-local metadata op args missing");
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    func.instruction(&Instruction::Drop);
}
