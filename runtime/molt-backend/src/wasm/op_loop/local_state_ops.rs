use super::*;

#[allow(unused_variables)]
pub(super) fn emit_local_state_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    const_cache: &ConstantCache,
    func_index: u32,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "store" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            let obj = locals[&args[0]];
            let val = locals[&args[1]];
            let offset = op.value.unwrap();
            let tmp_addr = locals["__wasm_tmp0"];
            let tmp_old = locals["__wasm_tmp1"];

            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::I64Add);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalSet(tmp_addr));

            func.instruction(&Instruction::LocalGet(tmp_addr));
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

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_set"]);
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
            func.instruction(&Instruction::LocalGet(tmp_addr));
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
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_set"]);
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
        "store_init" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            let obj = locals[&args[0]];
            let val = locals[&args[1]];
            let offset = op.value.unwrap();
            let tmp_addr = locals["__wasm_tmp0"];

            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::I64Add);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalSet(tmp_addr));

            func.instruction(&Instruction::LocalGet(val));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_init"]);
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
            func.instruction(&Instruction::LocalGet(tmp_addr));
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
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["object_field_init"]);
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
        "load" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let offset = op.value.unwrap();
            let tmp_addr = locals["__wasm_tmp0"];
            let tmp_val = locals["__wasm_tmp1"];
            let out = locals[op.out.as_ref().unwrap()];

            func.instruction(&Instruction::LocalGet(obj));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::I64Add);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalSet(tmp_addr));

            func.instruction(&Instruction::LocalGet(tmp_addr));
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
            func.instruction(&Instruction::LocalSet(out));

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_val));
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            emit_call(func, reloc_enabled, import_ids["object_field_get"]);
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);
        }
        "closure_load" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let tmp_ptr = locals["__molt_tmp0"];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(func, reloc_enabled, import_ids["closure_load"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "closure_store" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let tmp_ptr = locals["__molt_tmp0"];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(locals[&args[1]]));
            emit_call(func, reloc_enabled, import_ids["closure_store"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "guarded_load" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let offset = op.value.unwrap();
            let tmp_addr = locals["__wasm_tmp0"];
            let tmp_val = locals["__wasm_tmp1"];
            let out = locals[op.out.as_ref().unwrap()];

            func.instruction(&Instruction::LocalGet(obj));
            const_cache.emit_qnan_tag_mask(func);
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(offset));
            func.instruction(&Instruction::I64Add);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalSet(tmp_addr));

            func.instruction(&Instruction::LocalGet(tmp_addr));
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
            func.instruction(&Instruction::LocalSet(out));

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(tmp_val));
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::I64Const(offset));
            emit_call(func, reloc_enabled, import_ids["object_field_get"]);
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);
        }
        "guarded_field_get" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_bits = locals[&args[1]];
            let expected = locals[&args[2]];
            let tmp_ptr = locals["__wasm_tmp0"];
            let tmp_val = locals["__wasm_tmp1"];
            let guard_val = locals["__molt_tmp0"];
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
            let tmp_ptr = locals["__wasm_tmp0"];
            let tmp_old = locals["__wasm_tmp1"];
            let guard_val = locals["__molt_tmp0"];
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
            let tmp_ptr = locals["__wasm_tmp0"];
            let guard_val = locals["__molt_tmp0"];
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
        "state_switch" => {}
        "state_transition" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let slot_bits = args.get(1).map(|name| locals[name]);
            let out = locals[op.out.as_ref().unwrap()];
            let self_ptr = locals["__molt_tmp0"];
            func.instruction(&Instruction::LocalGet(0));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(self_ptr));
            func.instruction(&Instruction::LocalGet(self_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_poll"]);
            func.instruction(&Instruction::LocalSet(out));
            if let Some(slot) = slot_bits {
                func.instruction(&Instruction::LocalGet(self_ptr));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(slot));
                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                func.instruction(&Instruction::I64And);
                func.instruction(&Instruction::LocalGet(out));
                emit_call(func, reloc_enabled, import_ids["closure_store"]);
                func.instruction(&Instruction::Drop);
            }
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::I64Const(box_pending()));
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(self_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            emit_call(func, reloc_enabled, import_ids["sleep_register"]);
            func.instruction(&Instruction::Drop);
            func.instruction(&Instruction::I64Const(box_pending()));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        }

        _ => return false,
    }
    true
}
