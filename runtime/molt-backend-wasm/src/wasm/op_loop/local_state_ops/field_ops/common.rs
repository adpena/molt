use super::super::LocalStateOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_values::{POINTER_MASK, box_bool};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_qnan_ptr_test(context: &LocalStateOpContext<'_>, func: &mut Function) {
    context.const_cache.emit_qnan_tag_mask(func);
    func.instruction(&Instruction::I64And);
    context.const_cache.emit_qnan_tag_ptr(func);
    func.instruction(&Instruction::I64Eq);
}

pub(super) fn emit_tagged_object_slot_address(func: &mut Function, obj: u32, offset: i64) {
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(offset));
    func.instruction(&Instruction::I64Add);
    func.instruction(&Instruction::I32WrapI64);
}

pub(super) fn emit_resolved_object_slot_address(func: &mut Function, ptr: u32, offset: i64) {
    func.instruction(&Instruction::LocalGet(ptr));
    func.instruction(&Instruction::I32Const(offset as i32));
    func.instruction(&Instruction::I32Add);
}

pub(super) fn emit_i64_load(func: &mut Function) {
    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
}

pub(super) fn emit_i64_store(func: &mut Function) {
    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
}

pub(super) fn emit_runtime_output(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) {
    match op.out.as_deref() {
        Some("none") | None => {
            func.instruction(&Instruction::Drop);
        }
        Some(out) => {
            func.instruction(&Instruction::LocalSet(context.locals[out]));
        }
    }
}

pub(super) fn emit_none_result_for_output(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) {
    if let Some(out) = op.out.as_deref()
        && out != "none"
    {
        context.const_cache.emit_none(func);
        func.instruction(&Instruction::LocalSet(context.locals[out]));
    }
}

pub(super) fn emit_slot_value_to_output(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    tmp_val: u32,
    out: Option<&str>,
) {
    func.instruction(&Instruction::LocalGet(tmp_val));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));

    func.instruction(&Instruction::LocalGet(tmp_val));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
    );
    emit_tmp_value_to_output(context, func, tmp_val, out);

    func.instruction(&Instruction::Else);
    emit_tmp_value_to_output(context, func, tmp_val, out);
    func.instruction(&Instruction::End);
}

pub(super) fn emit_resolve_object_to_tmp(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    obj: u32,
    tmp_ptr: u32,
) {
    func.instruction(&Instruction::LocalGet(obj));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
    );
    func.instruction(&Instruction::LocalSet(tmp_ptr));
}

pub(super) fn emit_resolved_object_bits(func: &mut Function, tmp_ptr: u32) {
    func.instruction(&Instruction::LocalGet(tmp_ptr));
    func.instruction(&Instruction::I64ExtendI32U);
}

pub(super) fn emit_guard_layout_to_tmp(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    tmp_ptr: u32,
    class_bits: u32,
    expected: u32,
    guard_val: u32,
) {
    emit_resolved_object_bits(func, tmp_ptr);
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(expected));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::GuardLayoutPtr],
    );
    func.instruction(&Instruction::LocalSet(guard_val));
}

pub(super) fn emit_guard_success_test(func: &mut Function, guard_val: u32) {
    func.instruction(&Instruction::LocalGet(guard_val));
    func.instruction(&Instruction::I64Const(box_bool(1)));
    func.instruction(&Instruction::I64Eq);
}

fn emit_tmp_value_to_output(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    tmp_val: u32,
    out: Option<&str>,
) {
    func.instruction(&Instruction::LocalGet(tmp_val));
    match out {
        Some(out) => {
            func.instruction(&Instruction::LocalSet(context.locals[out]));
        }
        None => {
            func.instruction(&Instruction::Drop);
        }
    }
}
