use super::*;

pub(super) fn emit_field_local_state_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let const_cache = context.const_cache;
    let reloc_enabled = context.reloc_enabled;

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
        _ => return false,
    }
    true
}
