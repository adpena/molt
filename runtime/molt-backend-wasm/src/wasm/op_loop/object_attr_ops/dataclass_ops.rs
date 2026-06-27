use super::super::*;
use super::store_or_drop_result;

pub(super) fn emit_dataclass_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
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
            store_or_drop_result(func, op, locals);
        }
        "dataclass_new_values" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let flags = locals[&args[2]];
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(box_int(args[3..].len() as i64)));
            emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for value_name in &args[3..] {
                let value = locals[value_name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(value));
                emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
            }
            func.instruction(&Instruction::LocalGet(out));
            emit_call(func, reloc_enabled, import_ids["tuple_builder_finish"]);
            func.instruction(&Instruction::LocalSet(out));
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
            store_or_drop_result(func, op, locals);
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
            store_or_drop_result(func, op, locals);
        }
        "dataclass_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
            store_or_drop_result(func, op, locals);
        }
        _ => return false,
    }
    true
}
