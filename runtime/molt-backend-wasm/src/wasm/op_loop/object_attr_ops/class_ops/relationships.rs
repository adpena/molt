use super::super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_isinstance(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let cls = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::LocalGet(cls));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Isinstance],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_exception_match_builtin(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let exc = locals[&args[0]];
    let tag = op.value.expect("exception_match_builtin missing tag value");
    func.instruction(&Instruction::LocalGet(exc));
    func.instruction(&Instruction::I64Const(tag));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionMatchBuiltin],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_issubclass(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let sub = locals[&args[0]];
    let cls = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(sub));
    func.instruction(&Instruction::LocalGet(cls));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Issubclass],
    );
    store_result_or_drop(func, op, locals);
}
