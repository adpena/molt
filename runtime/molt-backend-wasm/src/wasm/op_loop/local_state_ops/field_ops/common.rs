use super::super::LocalStateOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_values::{POINTER_MASK, box_bool};
use molt_codegen_abi::{HEADER_FLAG_HAS_PTRS, HEADER_FLAGS_OFFSET};
use wasm_encoder::{BlockType, Function, Instruction};

#[derive(Clone, Copy)]
pub(super) enum FieldObject {
    Tagged(u32),
    Resolved(u32),
}

pub(super) fn emit_qnan_ptr_test(context: &LocalStateOpContext<'_>, func: &mut Function) {
    context.const_cache.emit_qnan_tag_mask(func);
    func.instruction(&Instruction::I64And);
    context.const_cache.emit_qnan_tag_ptr(func);
    func.instruction(&Instruction::I64Eq);
}

pub(super) fn emit_field_address(func: &mut Function, object: FieldObject, offset: i64) {
    match object {
        FieldObject::Tagged(obj) => {
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I32WrapI64);
        }
        FieldObject::Resolved(ptr) => {
            func.instruction(&Instruction::LocalGet(ptr));
        }
    }
    func.instruction(&Instruction::I32Const(offset as i32));
    func.instruction(&Instruction::I32Add);
}

/// Push a nonzero predicate when physical fields or dictionary backing need
/// runtime resolution. Call only after receiver admission: reading a header is
/// already a dereference. Materialization sets HAS_PTRS permanently.
pub(super) fn emit_field_needs_runtime(func: &mut Function, object: FieldObject) {
    emit_field_address(func, object, i64::from(HEADER_FLAGS_OFFSET));
    func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
        align: 2,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::I32Const(HEADER_FLAG_HAS_PTRS as i32));
    func.instruction(&Instruction::I32And);
}

/// All semantic field writes share the same backing-state guard.
/// A pointer-valued incoming operand additionally needs runtime ownership.
pub(super) fn emit_field_write_needs_runtime(
    context: &LocalStateOpContext<'_>,
    func: &mut Function,
    object: FieldObject,
    value: u32,
) {
    emit_field_needs_runtime(func, object);
    func.instruction(&Instruction::LocalGet(value));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::I32Or);
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
    obj: u32,
    class_bits: u32,
    expected: u32,
    guard_val: u32,
) {
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(expected));
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::GuardLayout],
    );
    func.instruction(&Instruction::LocalSet(guard_val));
}

pub(super) fn emit_guard_success_test(func: &mut Function, guard_val: u32) {
    func.instruction(&Instruction::LocalGet(guard_val));
    func.instruction(&Instruction::I64Const(box_bool(1)));
    func.instruction(&Instruction::I64Eq);
}

/// HAS_PTRS-clear storage can still contain the immortal missing sentinel.
/// Admit only immediate values; pointer-tagged candidates require runtime
/// missing/class-fallback resolution even when no ordinary heap owner exists.
pub(super) fn emit_inline_field_value_to_output(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    tmp_val: u32,
    out: Option<&str>,
    runtime: impl FnOnce(&mut LocalStateOpContext<'_>, &mut Function),
) {
    func.instruction(&Instruction::LocalGet(tmp_val));
    emit_qnan_ptr_test(context, func);
    func.instruction(&Instruction::If(BlockType::Empty));
    runtime(context, func);
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(tmp_val));
    match out {
        Some(out) if out != "none" => {
            func.instruction(&Instruction::LocalSet(context.locals[out]));
        }
        _ => {
            func.instruction(&Instruction::Drop);
        }
    }
    func.instruction(&Instruction::End);
}
