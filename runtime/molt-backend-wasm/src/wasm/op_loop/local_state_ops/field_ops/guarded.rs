use super::super::LocalStateOpContext;
use super::common::{
    emit_guard_layout_to_tmp, emit_guard_success_test, emit_i64_load, emit_i64_store,
    emit_none_result_for_output, emit_qnan_ptr_test, emit_resolve_object_to_tmp,
    emit_resolved_object_bits, emit_resolved_object_slot_address, emit_runtime_output,
    emit_slot_value_to_output,
};
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_guarded_field_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "guarded_field_get" => emit_guarded_get(context, func, op),
        "guarded_field_set" => emit_guarded_write(
            context,
            func,
            op,
            WasmRuntimeImport::ObjectFieldSetPtr,
            WasmRuntimeImport::GuardedFieldSetPtr,
            false,
        ),
        "guarded_field_init" => emit_guarded_write(
            context,
            func,
            op,
            WasmRuntimeImport::ObjectFieldInitPtr,
            WasmRuntimeImport::GuardedFieldInitPtr,
            true,
        ),
        _ => return false,
    }
    true
}

fn emit_guarded_get(context: &mut LocalStateOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let class_bits = context.locals[&args[1]];
    let expected = context.locals[&args[2]];
    let offset = op.value.unwrap();
    let tmp_ptr = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
    let tmp_val = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
    let guard_val = context.locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
    let attr = op.s_value.as_ref().unwrap();
    let bytes = attr.as_bytes();
    let data = context
        .backend
        .add_data_segment(context.reloc_enabled, bytes);

    emit_resolve_and_guard(context, func, obj, class_bits, expected, tmp_ptr, guard_val);

    emit_guard_success_test(func, guard_val);
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_resolved_object_slot_address(func, tmp_ptr, offset);
    emit_i64_load(func);
    func.instruction(&Instruction::LocalSet(tmp_val));

    emit_slot_value_to_output(context, func, tmp_val, op.out.as_deref());

    func.instruction(&Instruction::Else);
    emit_guarded_runtime_get(
        context,
        func,
        op,
        tmp_ptr,
        class_bits,
        expected,
        offset,
        data,
        bytes.len(),
    );
    func.instruction(&Instruction::End);
}

fn emit_guarded_write(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    ptr_import: WasmRuntimeImport,
    guarded_import: WasmRuntimeImport,
    init_only: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = context.locals[&args[0]];
    let class_bits = context.locals[&args[1]];
    let expected = context.locals[&args[2]];
    let val = context.locals[&args[3]];
    let offset = op.value.unwrap();
    let tmp_ptr = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
    let tmp_old = context.locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
    let guard_val = context.locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
    let attr = op.s_value.as_ref().unwrap();
    let bytes = attr.as_bytes();
    let data = context
        .backend
        .add_data_segment(context.reloc_enabled, bytes);

    emit_resolve_and_guard(context, func, obj, class_bits, expected, tmp_ptr, guard_val);

    emit_guard_success_test(func, guard_val);
    func.instruction(&Instruction::If(BlockType::Empty));

    if init_only {
        func.instruction(&Instruction::LocalGet(val));
        emit_qnan_ptr_test(context, func);
    } else {
        emit_resolved_object_slot_address(func, tmp_ptr, offset);
        emit_i64_load(func);
        func.instruction(&Instruction::LocalSet(tmp_old));

        func.instruction(&Instruction::LocalGet(tmp_old));
        emit_qnan_ptr_test(context, func);

        func.instruction(&Instruction::LocalGet(val));
        emit_qnan_ptr_test(context, func);
        func.instruction(&Instruction::I32Or);
    }
    func.instruction(&Instruction::If(BlockType::Empty));

    emit_runtime_ptr_field_write(context, func, op, tmp_ptr, offset, val, ptr_import);

    func.instruction(&Instruction::Else);
    emit_resolved_object_slot_address(func, tmp_ptr, offset);
    func.instruction(&Instruction::LocalGet(val));
    emit_i64_store(func);
    emit_none_result_for_output(context, func, op);
    func.instruction(&Instruction::End);

    func.instruction(&Instruction::Else);
    emit_guarded_runtime_write(
        context,
        func,
        op,
        tmp_ptr,
        class_bits,
        expected,
        offset,
        val,
        data,
        bytes.len(),
        guarded_import,
    );
    func.instruction(&Instruction::End);
}

fn emit_resolve_and_guard(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    obj: u32,
    class_bits: u32,
    expected: u32,
    tmp_ptr: u32,
    guard_val: u32,
) {
    emit_resolve_object_to_tmp(context, func, obj, tmp_ptr);
    emit_guard_layout_to_tmp(context, func, tmp_ptr, class_bits, expected, guard_val);
}

fn emit_runtime_ptr_field_write(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    tmp_ptr: u32,
    offset: i64,
    val: u32,
    runtime_import: WasmRuntimeImport,
) {
    emit_resolved_object_bits(func, tmp_ptr);
    func.instruction(&Instruction::I64Const(offset));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[runtime_import],
    );
    emit_runtime_output(context, func, op);
}

fn emit_guarded_runtime_get(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    tmp_ptr: u32,
    class_bits: u32,
    expected: u32,
    offset: i64,
    data: DataSegmentRef,
    len: usize,
) {
    emit_resolved_object_bits(func, tmp_ptr);
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(expected));
    func.instruction(&Instruction::I64Const(offset));
    context
        .backend
        .emit_data_ptr(context.reloc_enabled, context.func_index, func, data);
    func.instruction(&Instruction::I64Const(len as i64));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[WasmRuntimeImport::GuardedFieldGetPtr],
    );
    emit_runtime_output(context, func, op);
}

#[allow(clippy::too_many_arguments)]
fn emit_guarded_runtime_write(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    tmp_ptr: u32,
    class_bits: u32,
    expected: u32,
    offset: i64,
    val: u32,
    data: DataSegmentRef,
    len: usize,
    runtime_import: WasmRuntimeImport,
) {
    emit_resolved_object_bits(func, tmp_ptr);
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(expected));
    func.instruction(&Instruction::I64Const(offset));
    func.instruction(&Instruction::LocalGet(val));
    context
        .backend
        .emit_data_ptr(context.reloc_enabled, context.func_index, func, data);
    func.instruction(&Instruction::I64Const(len as i64));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[runtime_import],
    );
    emit_runtime_output(context, func, op);
}
