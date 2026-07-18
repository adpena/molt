use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_code_metadata_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "code_new" => emit_code_new(call_ctx, func, op),
        "code_slot_set" => {
            emit_value_then_local_drop_call(call_ctx, func, op, WasmRuntimeImport::CodeSlotSet)
        }
        "fn_ptr_code_set" => emit_table_local_drop_call(
            call_ctx,
            func,
            op,
            "fn_ptr_code_set",
            WasmRuntimeImport::FnPtrCodeSet,
        ),
        "asyncgen_locals_register" => emit_table_two_local_drop_call(
            call_ctx,
            func,
            op,
            "asyncgen_locals_register",
            WasmRuntimeImport::AsyncgenLocalsRegister,
        ),
        "gen_locals_register" => emit_table_two_local_drop_call(
            call_ctx,
            func,
            op,
            "gen_locals_register",
            WasmRuntimeImport::GenLocalsRegister,
        ),
        "code_slots_init" => {
            emit_value_drop_call(call_ctx, func, op, WasmRuntimeImport::CodeSlotsInit)
        }
        "trace_enter_slot" => {
            emit_value_drop_call(call_ctx, func, op, WasmRuntimeImport::TraceEnterSlot)
        }
        "trace_exit" => emit_no_arg_drop_call(call_ctx, func, WasmRuntimeImport::TraceExit),
        "line" => emit_value_drop_call(call_ctx, func, op, WasmRuntimeImport::TraceSetLine),
        "frame_locals_set" => {
            emit_one_local_drop_call(call_ctx, func, op, WasmRuntimeImport::FrameLocalsSet)
        }
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
        call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CodeNew],
    );
    if let Some(out) = op.out.as_ref() {
        let res = call_ctx.locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

fn emit_value_then_local_drop_call(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import: WasmRuntimeImport,
) {
    let args = op.args.as_ref().unwrap();
    let value = op.value.unwrap_or(0);
    func.instruction(&Instruction::I64Const(value));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}

fn emit_table_local_drop_call(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    table_context: &str,
    import: WasmRuntimeImport,
) {
    let args = op.args.as_ref().unwrap();
    let func_name = op.s_value.as_ref().unwrap();
    let target = call_ctx
        .call_site_abi
        .table_target(func_name, table_context);
    call_ctx.table_relocations.emit_i64(
        call_ctx.reloc_enabled,
        call_ctx.func_import_count,
        call_ctx.func_index,
        func,
        &target,
    );
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}

fn emit_table_two_local_drop_call(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    table_context: &str,
    import: WasmRuntimeImport,
) {
    let args = op.args.as_ref().unwrap();
    let func_name = op.s_value.as_ref().unwrap();
    let target = call_ctx
        .call_site_abi
        .table_target(func_name, table_context);
    call_ctx.table_relocations.emit_i64(
        call_ctx.reloc_enabled,
        call_ctx.func_import_count,
        call_ctx.func_index,
        func,
        &target,
    );
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[1]]));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}

fn emit_value_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import: WasmRuntimeImport,
) {
    func.instruction(&Instruction::I64Const(op.value.unwrap_or(0)));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}

fn emit_no_arg_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    import: WasmRuntimeImport,
) {
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}

fn emit_one_local_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import: WasmRuntimeImport,
) {
    let args = op
        .args
        .as_ref()
        .expect("one-local metadata op args missing");
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    func.instruction(&Instruction::Drop);
}
