use super::super::super::result_sink::{store_non_none_result_or_drop, store_result_or_drop};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_super_new(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let type_bits = locals[&args[0]];
    let obj_bits = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(type_bits));
    func.instruction(&Instruction::LocalGet(obj_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::SuperNew],
    );
    store_result_or_drop(func, op, locals);
}

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
    store_result_or_drop(func, op, locals);
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
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_class_layout_version(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassLayoutVersion],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_class_set_layout_version(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    let version_bits = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(version_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassSetLayoutVersion],
    );
    store_non_none_result_or_drop(func, op, locals);
}

pub(super) fn emit_class_merge_layout(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    let offsets_bits = locals[&args[1]];
    let size_bits = locals[&args[2]];
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(offsets_bits));
    func.instruction(&Instruction::LocalGet(size_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassMergeLayout],
    );
    store_non_none_result_or_drop(func, op, locals);
}
