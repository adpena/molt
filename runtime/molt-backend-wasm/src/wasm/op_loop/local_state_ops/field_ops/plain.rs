use super::super::LocalStateOpContext;
use super::common::{
    FieldObject, emit_field_address, emit_field_needs_runtime, emit_field_write_needs_runtime,
    emit_i64_load, emit_i64_store, emit_inline_field_value_to_output, emit_none_result_for_output,
    emit_qnan_ptr_test, emit_runtime_output,
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
        "store" => emit_store(context, func, op),
        "load" | "guarded_load" => emit_load(context, func, op),
        _ => return false,
    }
    true
}

fn emit_store(context: &mut LocalStateOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let val = context.locals[&args[1]];
    let offset = op.value.unwrap();
    func.instruction(&Instruction::LocalGet(obj));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_field_write_needs_runtime(context, func, FieldObject::Tagged(obj), val);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_runtime_field_write(context, func, op, obj, offset, val);

    func.instruction(&Instruction::Else);
    emit_field_address(func, FieldObject::Tagged(obj), offset);
    func.instruction(&Instruction::LocalGet(val));
    emit_i64_store(func);
    emit_none_result_for_output(context, func, op);
    func.instruction(&Instruction::End);

    func.instruction(&Instruction::Else);
    emit_runtime_field_write(context, func, op, obj, offset, val);
    func.instruction(&Instruction::End);
}

fn emit_load(context: &mut LocalStateOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let offset = op.value.unwrap();
    let tmp_val = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
    let out = op.out.as_ref().unwrap();

    func.instruction(&Instruction::LocalGet(obj));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_field_needs_runtime(func, FieldObject::Tagged(obj));
    func.instruction(&Instruction::If(BlockType::Empty));
    emit_runtime_field_read(context, func, obj, offset, out);
    func.instruction(&Instruction::Else);
    emit_field_address(func, FieldObject::Tagged(obj), offset);
    emit_i64_load(func);
    func.instruction(&Instruction::LocalSet(tmp_val));
    emit_inline_field_value_to_output(
        context,
        func,
        tmp_val,
        Some(out.as_str()),
        |context, func| {
            emit_runtime_field_read(context, func, obj, offset, out);
        },
    );
    func.instruction(&Instruction::End);

    func.instruction(&Instruction::Else);
    emit_runtime_field_read(context, func, obj, offset, out);
    func.instruction(&Instruction::End);
}

fn emit_runtime_field_read(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    obj: u32,
    offset: i64,
    out: &str,
) {
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(offset));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[WasmRuntimeImport::ObjectFieldGet],
    );
    func.instruction(&Instruction::LocalSet(context.locals[out]));
}

fn emit_runtime_field_write(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    obj: u32,
    offset: i64,
    val: u32,
) {
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(offset));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[WasmRuntimeImport::ObjectFieldSet],
    );
    emit_runtime_output(context, func, op);
}
