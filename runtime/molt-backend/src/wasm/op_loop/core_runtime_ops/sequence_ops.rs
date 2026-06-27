use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_sequence_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    scalar_plan: &ScalarRepresentationPlan,
    is_multi_return_callee: Option<usize>,
    multi_ret_locals: &[u32],
    multi_ret_tuple_vars: &BTreeSet<String>,
    multi_ret_call_locals: &BTreeMap<(String, i64), u32>,
    multi_ret_call_vars: &BTreeSet<String>,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "index" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            // Dispatch: list_int / dict / tuple → generic
            let import_key = wasm_specialized_container_import(&scalar_plan, op_idx, "index", op)
                .unwrap_or("index");
            let import_id =
                selected_import_id(import_ids, import_key, &func_ir.name, op.kind.as_str());
            emit_call(func, reloc_enabled, import_id);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "store_index" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            // Dispatch: list_int / dict → generic
            let import_key =
                wasm_specialized_container_import(&scalar_plan, op_idx, "store_index", op)
                    .unwrap_or("store_index");
            let import_id =
                selected_import_id(import_ids, import_key, &func_ir.name, op.kind.as_str());
            emit_call(func, reloc_enabled, import_id);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "del_index" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(func, reloc_enabled, import_ids["del_index"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "slice" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let start = locals[&args[1]];
            let end = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            emit_call(func, reloc_enabled, import_ids["slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "slice_new" => {
            let args = op.args.as_ref().unwrap();
            let start = locals[&args[0]];
            let stop = locals[&args[1]];
            let step = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(step));
            emit_call(func, reloc_enabled, import_ids["slice_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_find" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytes_find"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_find_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytes_find_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_find" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytearray_find"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_find_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytearray_find_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_find" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_find"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_find_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["string_find_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_format" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let spec = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(spec));
            emit_call(func, reloc_enabled, import_ids["format_builtin"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_startswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_startswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_startswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["string_startswith_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_startswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytes_startswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_startswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytes_startswith_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_startswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytearray_startswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_startswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(
                func,
                reloc_enabled,
                import_ids["bytearray_startswith_slice"],
            );
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_endswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_endswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_endswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["string_endswith_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_endswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytes_endswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_endswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytes_endswith_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_endswith" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytearray_endswith"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_endswith_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytearray_endswith_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_count" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_count"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_count" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytes_count"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_count" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytearray_count"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_count_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["string_count_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_count_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytes_count_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_count_slice" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let start = locals[&args[2]];
            let end = locals[&args[3]];
            let has_start = locals[&args[4]];
            let has_end = locals[&args[5]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["bytearray_count_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        _ => return false,
    }
    true
}
