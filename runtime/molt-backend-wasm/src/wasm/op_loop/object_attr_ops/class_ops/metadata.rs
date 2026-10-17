use super::super::super::result_sink::store_runtime_result;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_builtin_type(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let tag = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(tag));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::BuiltinType],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        crate::wasm_abi_generated::WasmRuntimeImport::BuiltinType,
    );
}

pub(super) fn emit_type_of(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(obj));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TypeOf],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        crate::wasm_abi_generated::WasmRuntimeImport::TypeOf,
    );
}

/// Stamp the class a statement created with the statement's compile-time
/// program slot (HF-220): `molt_class_set_definition(class, slot)`.
pub(super) fn emit_class_set_definition(
    func: &mut Function,
    op: &OpIR,
    code_slots: &molt_ir::code_slots::CodeSlotBindings,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    let definition_slot = code_slots
        .resolve_class_definition(op)
        .expect("admitted class statement binding");
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::I64Const(definition_slot as i64));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassSetDefinition],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        crate::wasm_abi_generated::WasmRuntimeImport::ClassSetDefinition,
    );
}
