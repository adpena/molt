use super::super::super::class_def_layout::ClassDefLayout;
use super::super::super::context::CompileFuncContext;
use super::super::result_sink::{store_non_none_result_or_drop, store_result_or_drop};
use super::super::*;

pub(super) fn emit_class_object_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &CompileFuncContext<'_>,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "class_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["class_new"]);
            store_result_or_drop(func, op, locals);
        }
        "class_def" => {
            let args = op.args.as_ref().unwrap();
            let meta = op.s_value.as_deref().expect("class_def needs s_value");
            let layout = ClassDefLayout::parse(meta);

            let spill_base = ctx.class_def_spill_offset;
            let attrs_base = layout.attrs_base_offset(spill_base);
            let attrs_start = layout.attrs_start_arg_index();

            // `class_def` spills boxed handles through shared linear memory
            // before the runtime helper snapshots them. Pin every handle
            // across that helper call so RC cleanup cannot reclaim or reuse
            // any object between the spill stores and `guarded_class_def`.
            for arg_name in args {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            }

            for (i, base_name) in args[1..1 + layout.nbases()].iter().enumerate() {
                let base = locals[base_name];
                func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
                func.instruction(&Instruction::LocalGet(base));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
            }

            for i in 0..layout.nattrs() {
                let key = locals[&args[attrs_start + i * 2]];
                let val = locals[&args[attrs_start + i * 2 + 1]];
                func.instruction(&Instruction::I32Const(
                    (attrs_base + (i as u32) * 16) as i32,
                ));
                func.instruction(&Instruction::LocalGet(key));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::I32Const(
                    (attrs_base + (i as u32) * 16 + 8) as i32,
                ));
                func.instruction(&Instruction::LocalGet(val));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
            }

            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::I32Const(spill_base as i32));
            func.instruction(&Instruction::I64Const(layout.nbases() as i64));
            func.instruction(&Instruction::I32Const(attrs_base as i32));
            func.instruction(&Instruction::I64Const(layout.nattrs() as i64));
            func.instruction(&Instruction::I64Const(layout.layout_size()));
            func.instruction(&Instruction::I64Const(layout.layout_version()));
            func.instruction(&Instruction::I64Const(layout.flags()));
            emit_call(func, reloc_enabled, import_ids["guarded_class_def"]);
            store_result_or_drop(func, op, locals);
            for arg_name in args.iter().rev() {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
            }
        }
        "class_set_base" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let base_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(base_bits));
            emit_call(func, reloc_enabled, import_ids["class_set_base"]);
            store_result_or_drop(func, op, locals);
        }
        "class_apply_set_name" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["class_apply_set_name"]);
            store_result_or_drop(func, op, locals);
        }
        "super_new" => {
            let args = op.args.as_ref().unwrap();
            let type_bits = locals[&args[0]];
            let obj_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(type_bits));
            func.instruction(&Instruction::LocalGet(obj_bits));
            emit_call(func, reloc_enabled, import_ids["super_new"]);
            store_result_or_drop(func, op, locals);
        }
        "builtin_type" => {
            let args = op.args.as_ref().unwrap();
            let tag = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(tag));
            emit_call(func, reloc_enabled, import_ids["builtin_type"]);
            store_result_or_drop(func, op, locals);
        }
        "type_of" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["type_of"]);
            store_result_or_drop(func, op, locals);
        }
        "class_layout_version" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["class_layout_version"]);
            store_result_or_drop(func, op, locals);
        }
        "class_set_layout_version" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let version_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(version_bits));
            emit_call(func, reloc_enabled, import_ids["class_set_layout_version"]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "class_merge_layout" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let offsets_bits = locals[&args[1]];
            let size_bits = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(offsets_bits));
            func.instruction(&Instruction::LocalGet(size_bits));
            emit_call(func, reloc_enabled, import_ids["class_merge_layout"]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "isinstance" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let cls = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(cls));
            emit_call(func, reloc_enabled, import_ids["isinstance"]);
            store_result_or_drop(func, op, locals);
        }
        "exception_match_builtin" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            let tag = op.value.expect("exception_match_builtin missing tag value");
            func.instruction(&Instruction::LocalGet(exc));
            func.instruction(&Instruction::I64Const(tag));
            emit_call(func, reloc_enabled, import_ids["exception_match_builtin"]);
            store_result_or_drop(func, op, locals);
        }
        "issubclass" => {
            let args = op.args.as_ref().unwrap();
            let sub = locals[&args[0]];
            let cls = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(sub));
            func.instruction(&Instruction::LocalGet(cls));
            emit_call(func, reloc_enabled, import_ids["issubclass"]);
            store_result_or_drop(func, op, locals);
        }
        "object_new" => {
            emit_call(func, reloc_enabled, import_ids["object_new"]);
            store_result_or_drop(func, op, locals);
        }
        "object_new_bound" => {
            let args = op
                .args
                .as_ref()
                .expect("object_new_bound requires class arg");
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            if let Some(payload_size) = op.value.filter(|size| *size > 0) {
                func.instruction(&Instruction::I64Const(payload_size));
                emit_call(func, reloc_enabled, import_ids["object_new_bound_sized"]);
            } else {
                emit_call(func, reloc_enabled, import_ids["object_new_bound"]);
            }
            store_result_or_drop(func, op, locals);
        }
        "object_new_bound_stack" => {
            let args = op
                .args
                .as_ref()
                .expect("object_new_bound_stack requires class arg");
            let payload_size = op
                .value
                .filter(|size| *size > 0)
                .expect("object_new_bound_stack requires positive payload byte size");
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::I64Const(payload_size));
            emit_call(func, reloc_enabled, import_ids["object_new_bound_sized"]);
            store_result_or_drop(func, op, locals);
        }
        "object_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(func, reloc_enabled, import_ids["object_set_class"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
