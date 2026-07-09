use super::super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm::object_new_bound_select::{
    required_object_new_bound_stack_runtime, selected_object_new_bound_runtime,
};
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
    let selected = selected_object_new_bound_runtime(op);
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    if let Some(payload_size) = selected.payload_size() {
        func.instruction(&Instruction::I64Const(payload_size));
    }
    emit_call(func, reloc_enabled, import_ids[selected.import]);
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
    let selected = required_object_new_bound_stack_runtime(op);
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::I64Const(
        selected.required_payload_size("object_new_bound_stack"),
    ));
    emit_call(func, reloc_enabled, import_ids[selected.import]);
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
