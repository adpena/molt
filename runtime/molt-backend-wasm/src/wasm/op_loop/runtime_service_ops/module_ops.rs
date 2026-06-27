use super::super::result_sink::{store_non_none_result_or_drop, store_result_or_drop};
use super::*;

pub(super) fn emit_module_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "module_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_new"]);
            store_result_or_drop(func, op, locals);
        }
        "module_cache_get" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
            store_result_or_drop(func, op, locals);
        }
        "module_import" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_import"]);
            store_result_or_drop(func, op, locals);
        }
        "module_cache_set" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let module = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(module));
            emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "module_cache_del" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_cache_del"]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "module_get_attr" | "module_import_from" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            // `from M import name` uses CPython IMPORT_FROM semantics
            // (ImportError on miss + sys.modules submodule fallback);
            // plain `M.name` raises AttributeError.
            let import_symbol = if op.kind == "module_import_from" {
                "module_import_from"
            } else {
                "module_get_attr"
            };
            emit_call(func, reloc_enabled, import_ids[import_symbol]);
            store_result_or_drop(func, op, locals);
        }
        "module_get_global" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_get_global"]);
            store_result_or_drop(func, op, locals);
        }
        "module_del_global" | "module_del_global_if_present" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids[op.kind.as_str()]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "module_get_name" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_get_name"]);
            store_result_or_drop(func, op, locals);
        }
        "module_set_attr" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["module_set_attr"]);
            store_non_none_result_or_drop(func, op, locals);
        }
        "module_import_star" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            let dst = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::LocalGet(dst));
            emit_call(func, reloc_enabled, import_ids["module_import_star"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
