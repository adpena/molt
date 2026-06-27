use super::*;

pub(super) fn emit_guarded_field_local_state_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let backend = &mut context.backend;
    let import_ids = context.import_ids;
    let locals = context.locals;
    let const_cache = context.const_cache;
    let func_index = context.func_index;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "guarded_field_get" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_bits = locals[&args[1]];
            let expected = locals[&args[2]];
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
            let tmp_val = locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
            let guard_val = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalSet(tmp_ptr));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
            func.instruction(&Instruction::LocalSet(guard_val));

            func.instruction(&Instruction::LocalGet(guard_val));
            func.instruction(&Instruction::I64Const(box_bool(1)));
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(tmp_val));

            func.instruction(&Instruction::LocalGet(tmp_val));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(tmp_val));
            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            func.instruction(&Instruction::LocalGet(tmp_val));
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_val));
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["guarded_field_get_ptr"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::End);
        }
        "guarded_field_set" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_bits = locals[&args[1]];
            let expected = locals[&args[2]];
            let val = locals[&args[3]];
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
            let tmp_old = locals.synthetic(WasmFrameSyntheticLocal::WasmTmp1);
            let guard_val = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalSet(tmp_ptr));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
            func.instruction(&Instruction::LocalSet(guard_val));

            func.instruction(&Instruction::LocalGet(guard_val));
            func.instruction(&Instruction::I64Const(box_bool(1)));
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(tmp_old));

            func.instruction(&Instruction::LocalGet(tmp_old));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);

            func.instruction(&Instruction::LocalGet(val));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::I32Or);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_set_ptr"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            if let Some(out) = op.out.as_ref()
                && out != "none"
            {
                const_cache.emit_none(func);
                func.instruction(&Instruction::LocalSet(locals[out]));
            }
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(val));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["guarded_field_set_ptr"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::End);
        }
        "guarded_field_init" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_bits = locals[&args[1]];
            let expected = locals[&args[2]];
            let val = locals[&args[3]];
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0);
            let guard_val = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalSet(tmp_ptr));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
            func.instruction(&Instruction::LocalSet(guard_val));

            func.instruction(&Instruction::LocalGet(guard_val));
            func.instruction(&Instruction::I64Const(box_bool(1)));
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(val));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_init_ptr"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            if let Some(out) = op.out.as_ref()
                && out != "none"
            {
                const_cache.emit_none(func);
                func.instruction(&Instruction::LocalSet(locals[out]));
            }
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(val));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["guarded_field_init_ptr"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::End);
        }
        _ => return false,
    }
    true
}
