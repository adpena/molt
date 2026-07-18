use super::super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_object_new(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjectNew],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_object_new_bound(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op
        .args
        .as_ref()
        .expect("object_new_bound requires class arg");
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::ObjectNewBound],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_object_new_bound_stack(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op
        .args
        .as_ref()
        .expect("object_new_bound_stack requires class arg");
    assert!(
        op.value.is_some_and(|payload_size| payload_size > 0),
        "object_new_bound_stack requires positive payload byte size"
    );
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::ObjectNewBound],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_object_set_class(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let class_obj = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(obj));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
    );
    func.instruction(&Instruction::I64ExtendI32U);
    func.instruction(&Instruction::LocalGet(class_obj));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjectSetClass],
    );
    store_result_or_drop(func, op, locals);
}
