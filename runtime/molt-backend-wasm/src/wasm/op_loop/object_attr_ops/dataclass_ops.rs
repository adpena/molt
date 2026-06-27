use super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::super::result_sink::store_result_or_drop;
use super::super::*;

pub(super) fn emit_dataclass_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "dataclass_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let values = locals[&args[2]];
            let flags = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(values));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
            store_result_or_drop(func, op, locals);
        }
        "dataclass_new_values" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let flags = locals[&args[2]];
            let out = locals[op.out.as_ref().unwrap()];
            emit_sequence_builder_from_args(
                func,
                &args[3..],
                out,
                import_ids,
                locals,
                reloc_enabled,
                BuilderFinish::Tuple,
            );
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "dataclass_get" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(func, reloc_enabled, import_ids["dataclass_get"]);
            store_result_or_drop(func, op, locals);
        }
        "dataclass_set" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["dataclass_set"]);
            store_result_or_drop(func, op, locals);
        }
        "dataclass_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
