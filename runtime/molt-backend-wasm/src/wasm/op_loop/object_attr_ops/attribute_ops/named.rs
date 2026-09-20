use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm::op_loop::result_sink::store_runtime_result;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_named_attribute_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "get_attr_name" => emit_get_attr_name(func, op, import_ids, locals, reloc_enabled),
        "get_attr_name_default" => {
            emit_get_attr_name_default(func, op, import_ids, locals, reloc_enabled)
        }
        "has_attr_name" => emit_has_attr_name(func, op, import_ids, locals, reloc_enabled),
        "set_attr_name" => emit_set_attr_name(func, op, import_ids, locals, reloc_enabled),
        "del_attr_name" => emit_del_attr_name(func, op, import_ids, locals, reloc_enabled),
        _ => return false,
    }
    true
}

fn emit_get_attr_name(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    emit_named_receiver(func, locals, &args[0], &args[1]);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::GetAttrName],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::GetAttrName,
    );
}

fn emit_get_attr_name_default(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    emit_named_receiver(func, locals, &args[0], &args[1]);
    func.instruction(&Instruction::LocalGet(locals[&args[2]]));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::GetAttrNameDefault],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::GetAttrNameDefault,
    );
}

fn emit_has_attr_name(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    emit_named_receiver(func, locals, &args[0], &args[1]);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::HasAttrName],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::HasAttrName,
    );
}

fn emit_set_attr_name(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    emit_named_receiver(func, locals, &args[0], &args[1]);
    func.instruction(&Instruction::LocalGet(locals[&args[2]]));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::SetAttrName],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::SetAttrName,
    );
}

fn emit_del_attr_name(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    emit_named_receiver(func, locals, &args[0], &args[1]);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::DelAttrName],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        WasmRuntimeImport::DelAttrName,
    );
}

fn emit_named_receiver(
    func: &mut Function,
    locals: &WasmFrameLocals,
    object_value: &str,
    name_value: &str,
) {
    func.instruction(&Instruction::LocalGet(locals[object_value]));
    func.instruction(&Instruction::LocalGet(locals[name_value]));
}
