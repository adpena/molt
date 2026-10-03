use super::super::result_sink::{discard_runtime_result, store_runtime_result};
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
            emit_value_then_two_locals_drop_call(call_ctx, func, op, WasmRuntimeImport::CodeSlotSet)
        }
        "stateful_locals_register" => emit_table_two_local_drop_call(
            call_ctx,
            func,
            op,
            "stateful_locals_register",
            WasmRuntimeImport::StatefulLocalsRegister,
        ),
        "code_slots_init" => emit_value_drop_call(
            call_ctx,
            func,
            op.value.expect("admitted code_slots_init count"),
            WasmRuntimeImport::CodeSlotsInit,
        ),
        "trace_enter_slot" => call_ctx.frame.emit_owned_frame_entry(
            func,
            op.value.expect("admitted trace_enter_slot ID"),
            call_ctx.import_ids,
            call_ctx.reloc_enabled,
        ),
        // Authored lifecycle marker; activation cleanup owns the actual exit,
        // including return edges introduced by this backend (yield/failure).
        "trace_exit" => {}
        // A home store hands operand 0's reference to the frame's home; its
        // result is a view of the operand that owns nothing.
        "frame_home_store" | "frame_home_cell" | "frame_home_private_cell" => {
            let args = op.args.as_ref().expect("admitted frame home operand");
            let value = call_ctx.locals[&args[0]];
            let kind = match op.kind.as_str() {
                "frame_home_cell" => molt_codegen_abi::FRAME_HOME_CELL,
                "frame_home_private_cell" => molt_codegen_abi::FRAME_HOME_PRIVATE_CELL,
                _ => molt_codegen_abi::FRAME_HOME_PLAIN,
            };
            call_ctx.frame.emit_frame_home_store(
                func,
                op.value.expect("admitted frame home slot"),
                kind,
                value,
                call_ctx.import_ids,
                call_ctx.reloc_enabled,
            );
            if let Some(out) = call_ctx.locals.bound_op_result_slot(op) {
                func.instruction(&Instruction::LocalGet(value));
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        // A borrowed view of the slot's binding, valid until its next write.
        "frame_home_load" => {
            call_ctx.frame.emit_frame_home_load(
                func,
                op.value.expect("admitted frame home slot"),
                call_ctx.import_ids,
                call_ctx.reloc_enabled,
            );
            match call_ctx.locals.bound_op_result_slot(op) {
                Some(out) => func.instruction(&Instruction::LocalSet(out)),
                None => func.instruction(&Instruction::Drop),
            };
        }
        // PEP 709's save of an enclosing binding: moved out, owned.
        "frame_home_take" => {
            call_ctx
                .frame
                .emit_frame_home_address(func, op.value.expect("admitted frame home slot"));
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids[WasmRuntimeImport::FrameHomeTake],
            );
            store_runtime_result(
                func,
                op,
                call_ctx.locals,
                call_ctx.import_ids,
                call_ctx.reloc_enabled,
                WasmRuntimeImport::FrameHomeTake,
            );
        }
        "frame_home_clear" => {
            call_ctx.frame.emit_frame_home_clear(
                func,
                op.value.expect("admitted frame home slot"),
                call_ctx.import_ids,
                call_ctx.reloc_enabled,
            );
        }
        // `locals()`: the runtime's one authority over the executing frame.
        "frame_locals" => {
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids[WasmRuntimeImport::LocalsBuiltin],
            );
            store_runtime_result(
                func,
                op,
                call_ctx.locals,
                call_ctx.import_ids,
                call_ctx.reloc_enabled,
                WasmRuntimeImport::LocalsBuiltin,
            );
        }
        "line" => emit_value_drop_call(
            call_ctx,
            func,
            op.value.unwrap_or(0),
            WasmRuntimeImport::TraceSetLine,
        ),
        "frame_locals_set" => {
            emit_one_local_drop_call(call_ctx, func, op, WasmRuntimeImport::FrameLocalsSet)
        }
        _ => return CallOpEmission::NotHandled,
    }
    CallOpEmission::Handled
}

fn emit_code_new(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().expect("admitted code_new operands");
    for arg in args {
        func.instruction(&Instruction::LocalGet(call_ctx.locals[arg]));
    }
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CodeNew],
    );
    store_runtime_result(
        func,
        op,
        call_ctx.locals,
        call_ctx.import_ids,
        call_ctx.reloc_enabled,
        WasmRuntimeImport::CodeNew,
    );
}

fn emit_value_then_two_locals_drop_call(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    import: WasmRuntimeImport,
) {
    let args = op.args.as_ref().expect("admitted code_slot_set operands");
    let value = op.value.expect("admitted code_slot_set ID");
    func.instruction(&Instruction::I64Const(value));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[0]]));
    func.instruction(&Instruction::LocalGet(call_ctx.locals[&args[1]]));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    discard_runtime_result(func, call_ctx.import_ids, call_ctx.reloc_enabled, import);
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
    discard_runtime_result(func, call_ctx.import_ids, call_ctx.reloc_enabled, import);
}

fn emit_value_drop_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    value: i64,
    import: WasmRuntimeImport,
) {
    func.instruction(&Instruction::I64Const(value));
    emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids[import]);
    discard_runtime_result(func, call_ctx.import_ids, call_ctx.reloc_enabled, import);
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
    discard_runtime_result(func, call_ctx.import_ids, call_ctx.reloc_enabled, import);
}
