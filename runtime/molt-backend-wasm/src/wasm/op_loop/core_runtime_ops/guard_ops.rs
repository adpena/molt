use super::super::result_sink::store_runtime_result;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

/// `guard_layout` operands are `[obj, expected_version]`; the class statement
/// it trusts is the op's `class_definition`, a compile-time slot (HF-220).
pub(super) fn emit_guard_runtime_op(
    func: &mut Function,
    op: &OpIR,
    code_slots: &molt_ir::code_slots::CodeSlotBindings,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    if op.kind != "guard_layout" {
        return false;
    }

    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let expected = locals[&args[1]];
    let definition_slot = code_slots
        .resolve_class_definition(op)
        .expect("admitted class statement binding");
    func.instruction(&Instruction::LocalGet(obj));
    func.instruction(&Instruction::I64Const(definition_slot as i64));
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
