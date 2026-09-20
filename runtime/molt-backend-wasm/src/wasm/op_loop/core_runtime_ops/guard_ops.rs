use super::super::result_sink::store_runtime_result;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_guard_runtime_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "guard_layout" | "guard_dict_shape" => {}
        _ => return false,
    }

    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let class_bits = locals[&args[1]];
    let expected = locals[&args[2]];
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(expected));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::GuardLayout],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::GuardLayout,
    );
    true
}
