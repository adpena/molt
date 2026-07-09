use crate::wasm::op_loop::result_sink::store_result_or_drop;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{box_int, stable_ic_site_id};
use crate::{FunctionIR, OpIR};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_generic_attribute_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "get_attr_generic_ptr" => emit_get_attr_generic_ptr(
            backend,
            func,
            op,
            import_ids,
            locals,
            func_index,
            reloc_enabled,
        ),
        "get_attr_generic_obj" => emit_get_attr_generic_obj(
            backend,
            func,
            op,
            func_ir,
            import_ids,
            locals,
            func_index,
            reloc_enabled,
            op_idx,
        ),
        "get_attr_special_obj" => emit_get_attr_special_obj(
            backend,
            func,
            op,
            import_ids,
            locals,
            func_index,
            reloc_enabled,
        ),
        "set_attr_generic_ptr" | "set_attr_generic_obj" => emit_set_attr_generic_object(
            backend,
            func,
            op,
            func_ir,
            import_ids,
            locals,
            func_index,
            reloc_enabled,
        ),
        "del_attr_generic_ptr" | "del_attr_generic_obj" => emit_del_attr_generic_object(
            backend,
            func,
            op,
            import_ids,
            locals,
            func_index,
            reloc_enabled,
        ),
        _ => return false,
    }
    true
}

fn emit_get_attr_generic_ptr(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let attr = staged_attr_name(backend, op, reloc_enabled);
    func.instruction(&Instruction::LocalGet(obj));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::HandleResolve],
    );
    emit_staged_attr_name(backend, func, func_index, reloc_enabled, attr);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::GetAttrPtr],
    );
    store_result_or_drop(func, op, locals);
}

fn emit_get_attr_generic_obj(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let attr = staged_attr_name(backend, op, reloc_enabled);
    let source_op_idx = required_source_op_idx(op, op_idx, "get_attr_generic_obj");
    let site_bits = box_int(stable_ic_site_id(
        func_ir.name.as_str(),
        source_op_idx,
        "get_attr_generic_obj",
    ));
    func.instruction(&Instruction::LocalGet(obj));
    emit_staged_attr_name(backend, func, func_index, reloc_enabled, attr);
    func.instruction(&Instruction::I64Const(site_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::GetAttrObjectIc],
    );
    store_result_or_drop(func, op, locals);
}

fn emit_get_attr_special_obj(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let attr = staged_attr_name(backend, op, reloc_enabled);
    func.instruction(&Instruction::LocalGet(obj));
    emit_staged_attr_name(backend, func, func_index, reloc_enabled, attr);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::GetAttrSpecial],
    );
    store_result_or_drop(func, op, locals);
}

fn emit_set_attr_generic_object(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) {
    // The `_generic_ptr` SETATTR form can target a tagged non-pointer receiver
    // (e.g. `typing.final(42)`). Route both generic forms through the
    // bits-validating object runtime import for native/WASM parity.
    let args = op.args.as_ref().unwrap();
    let obj = local_or_panic(locals, &args[0], func_ir, op);
    let val = local_or_panic(locals, &args[1], func_ir, op);
    let attr = staged_attr_name(backend, op, reloc_enabled);
    func.instruction(&Instruction::LocalGet(obj));
    emit_staged_attr_name(backend, func, func_index, reloc_enabled, attr);
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::SetAttrObject],
    );
    store_result_or_drop(func, op, locals);
}

fn emit_del_attr_generic_object(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) {
    // Mirror SETATTR: tagged non-pointer receivers must not be resolved and
    // dereferenced by the pointer delete path.
    let args = op.args.as_ref().unwrap();
    let obj = locals[&args[0]];
    let attr = staged_attr_name(backend, op, reloc_enabled);
    func.instruction(&Instruction::LocalGet(obj));
    emit_staged_attr_name(backend, func, func_index, reloc_enabled, attr);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::DelAttrObject],
    );
    store_result_or_drop(func, op, locals);
}

fn staged_attr_name(
    backend: &mut WasmBackend,
    op: &OpIR,
    reloc_enabled: bool,
) -> (DataSegmentRef, usize) {
    let attr = op.s_value.as_ref().unwrap();
    let bytes = attr.as_bytes();
    (backend.add_data_segment(reloc_enabled, bytes), bytes.len())
}

fn emit_staged_attr_name(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    (data, len): (DataSegmentRef, usize),
) {
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I64Const(len as i64));
}

fn local_or_panic(locals: &WasmFrameLocals, value: &str, func_ir: &FunctionIR, op: &OpIR) -> u32 {
    *locals.get(value).unwrap_or_else(|| {
        panic!(
            "missing local {} in {} for {}",
            value, func_ir.name, op.kind
        )
    })
}

fn required_source_op_idx(op: &OpIR, op_idx: usize, kind: &str) -> usize {
    match op.source_op_idx {
        Some(value) => usize::try_from(value)
            .unwrap_or_else(|_| panic!("{kind} has invalid negative source_op_idx {value}")),
        None => panic!("{kind} at stream op {op_idx} requires transported source_op_idx"),
    }
}
