use super::super::result_sink::store_result_or_drop;
use super::super::*;

pub(super) fn emit_attribute_op(
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
        "get_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["get_attr_ptr"]);
            store_result_or_drop(func, op, locals);
        }
        "get_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let source_op_idx = required_source_op_idx(op, op_idx, "get_attr_generic_obj");
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                source_op_idx,
                "get_attr_generic_obj",
            ));
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::I64Const(site_bits));
            emit_call(func, reloc_enabled, import_ids["get_attr_object_ic"]);
            store_result_or_drop(func, op, locals);
        }
        "get_attr_special_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["get_attr_special"]);
            store_result_or_drop(func, op, locals);
        }
        "set_attr_generic_ptr" => {
            // The `_generic_ptr` SETATTR form can target a tagged
            // non-pointer receiver (e.g. `typing.final(42)`). Resolving
            // it to a pointer first (`handle_resolve`) then calling
            // `set_attr_ptr` (which dereferences the object header)
            // would fault on a tagged value. Route through the
            // bits-validating `set_attr_object` instead - identical to
            // the `set_attr_generic_obj` arm - so a tagged receiver
            // raises a clean AttributeError/TypeError. This keeps the
            // native and WASM backends at parity (see the native
            // `fc::attrs` fix).
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let val = locals[&args[1]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
            store_result_or_drop(func, op, locals);
        }
        "set_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = *locals.get(&args[0]).unwrap_or_else(|| {
                panic!(
                    "missing local {} in {} for {}",
                    args[0], func_ir.name, op.kind
                )
            });
            let val = *locals.get(&args[1]).unwrap_or_else(|| {
                panic!(
                    "missing local {} in {} for {}",
                    args[1], func_ir.name, op.kind
                )
            });
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
            store_result_or_drop(func, op, locals);
        }
        "del_attr_generic_ptr" => {
            // Mirror the `set_attr_generic_ptr` fix: a tagged
            // non-pointer receiver must not be `handle_resolve`'d and
            // dereferenced by `del_attr_ptr`. Route through the
            // bits-validating `del_attr_object` (same as
            // `del_attr_generic_obj`) for native/WASM parity.
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
            store_result_or_drop(func, op, locals);
        }
        "del_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
            store_result_or_drop(func, op, locals);
        }
        "get_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["get_attr_name"]);
            store_result_or_drop(func, op, locals);
        }
        "get_attr_name_default" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            let default_val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(default_val));
            emit_call(func, reloc_enabled, import_ids["get_attr_name_default"]);
            store_result_or_drop(func, op, locals);
        }
        "has_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["has_attr_name"]);
            store_result_or_drop(func, op, locals);
        }
        "set_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_name"]);
            store_result_or_drop(func, op, locals);
        }
        "del_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["del_attr_name"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}

fn required_source_op_idx(op: &OpIR, op_idx: usize, kind: &str) -> usize {
    match op.source_op_idx {
        Some(value) => usize::try_from(value)
            .unwrap_or_else(|_| panic!("{kind} has invalid negative source_op_idx {value}")),
        None => panic!("{kind} at stream op {op_idx} requires transported source_op_idx"),
    }
}
