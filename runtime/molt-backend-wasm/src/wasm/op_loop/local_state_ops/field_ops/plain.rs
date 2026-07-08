use super::super::LocalStateOpContext;
use super::common::{
    emit_i64_load, emit_i64_store, emit_none_result_for_output, emit_qnan_ptr_test,
    emit_runtime_output, emit_slot_value_to_output, emit_tagged_object_slot_address,
};
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_plain_field_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "store" => emit_store(context, func, op, WasmRuntimeImport::ObjectFieldSet, false),
        "store_init" => emit_store(context, func, op, WasmRuntimeImport::ObjectFieldInit, true),
        "load" | "guarded_load" => emit_load(context, func, op),
        _ => return false,
    }
    true
}

fn emit_store(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    runtime_import: WasmRuntimeImport,
    init_only: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let val = context.locals[&args[1]];
    let offset = op.value.unwrap();
    let tmp_addr = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
    let tmp_old = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);

    func.instruction(&Instruction::LocalGet(obj));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_tagged_object_slot_address(func, obj, offset);
    func.instruction(&Instruction::LocalSet(tmp_addr));

    if init_only {
        func.instruction(&Instruction::LocalGet(val));
        emit_qnan_ptr_test(context, func);
    } else {
        func.instruction(&Instruction::LocalGet(tmp_addr));
        emit_i64_load(func);
        func.instruction(&Instruction::LocalSet(tmp_old));

        func.instruction(&Instruction::LocalGet(tmp_old));
        emit_qnan_ptr_test(context, func);

        func.instruction(&Instruction::LocalGet(val));
        emit_qnan_ptr_test(context, func);
        func.instruction(&Instruction::I32Or);
    }
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_runtime_field_write(context, func, op, obj, offset, val, runtime_import);

    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(tmp_addr));
    func.instruction(&Instruction::LocalGet(val));
    emit_i64_store(func);
    emit_none_result_for_output(context, func, op);
    func.instruction(&Instruction::End);

    func.instruction(&Instruction::Else);
    emit_runtime_field_write(context, func, op, obj, offset, val, runtime_import);
    func.instruction(&Instruction::End);
}

fn emit_load(context: &mut LocalStateOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let offset = op.value.unwrap();
    let tmp_addr = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
    let tmp_val = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
    let out = op.out.as_ref().unwrap();

    func.instruction(&Instruction::LocalGet(obj));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_tagged_object_slot_address(func, obj, offset);
    func.instruction(&Instruction::LocalSet(tmp_addr));

    func.instruction(&Instruction::LocalGet(tmp_addr));
    emit_i64_load(func);
    func.instruction(&Instruction::LocalSet(tmp_val));

    emit_slot_value_to_output(context, func, tmp_val, Some(out.as_str()));

    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(offset));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[WasmRuntimeImport::ObjectFieldGet],
    );
    func.instruction(&Instruction::LocalSet(context.locals[out]));
    func.instruction(&Instruction::End);
}

fn emit_runtime_field_write(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    obj: u32,
    offset: i64,
    val: u32,
    runtime_import: WasmRuntimeImport,
) {
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(offset));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[runtime_import],
    );
    emit_runtime_output(context, func, op);
}
