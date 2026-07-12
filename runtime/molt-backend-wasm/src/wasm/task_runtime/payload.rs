use wasm_encoder::{Function, Instruction, MemArg};

use crate::wasm_abi::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;

pub(in crate::wasm) fn emit_register_cancel_token(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    task_local: u32,
) {
    func.instruction(&Instruction::LocalGet(task_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::CancelTokenGetCurrent],
    );
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::TaskRegisterTokenOwned],
    );
    func.instruction(&Instruction::Drop);
}

pub(in crate::wasm) fn emit_task_payload_base(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    task_local: u32,
    base_local: u32,
) {
    func.instruction(&Instruction::LocalGet(task_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::HandleResolve],
    );
    func.instruction(&Instruction::LocalSet(base_local));
}

pub(in crate::wasm) fn emit_store_task_payload_local(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    base_local: u32,
    offset: i32,
    value_local: u32,
) {
    func.instruction(&Instruction::LocalGet(base_local));
    func.instruction(&Instruction::I32Const(offset));
    func.instruction(&Instruction::I32Add);
    func.instruction(&Instruction::LocalGet(value_local));
    func.instruction(&Instruction::I64Store(MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::LocalGet(value_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::IncRefObj],
    );
}
