use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_aggregate_runtime_op(
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
        "list_from_range" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let start = locals[&args[0]];
            let stop = locals[&args[1]];
            let step = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(step));
            emit_call(func, reloc_enabled, import_ids["list_from_range"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "tuple_new" => {
            let empty_args: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args);
            let out_name = op.out.as_ref().unwrap();
            let out = locals[out_name];
            // Multi-value return (Section 3.1): store elements
            // into __multi_ret_N locals instead of heap-allocating
            // when this tuple flows directly to a return in a
            // candidate function.
            if is_multi_return_callee.is_some()
                && multi_ret_tuple_vars.contains(out_name)
                && args.len() == multi_ret_locals.len()
            {
                for (k, arg_name) in args.iter().enumerate() {
                    let val = locals[arg_name];
                    func.instruction(&Instruction::LocalGet(val));
                    func.instruction(&Instruction::LocalSet(multi_ret_locals[k]));
                }
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::LocalSet(out));
            } else {
                func.instruction(&Instruction::I64Const(box_int(args.len() as i64)));
                emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
                func.instruction(&Instruction::LocalSet(out));
                for name in args {
                    let val = locals[name];
                    func.instruction(&Instruction::LocalGet(out));
                    func.instruction(&Instruction::LocalGet(val));
                    emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
                }
                func.instruction(&Instruction::LocalGet(out));
                emit_call(func, reloc_enabled, import_ids["tuple_builder_finish"]);
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        "callargs_push_pos" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
            if let Some(out_name) = op.out.as_ref() {
                let res = locals[out_name];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                // No output variable; the runtime call returns an i64
                // that must be consumed to keep the WASM stack balanced.
                func.instruction(&Instruction::Drop);
            }
        }
        "callargs_push_kw" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["callargs_push_kw"]);
            if let Some(out_name) = op.out.as_ref() {
                let res = locals[out_name];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "callargs_expand_star" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let iterable = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(iterable));
            emit_call(func, reloc_enabled, import_ids["callargs_expand_star"]);
            if let Some(out_name) = op.out.as_ref() {
                let res = locals[out_name];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "callargs_expand_kwstar" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let mapping = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(mapping));
            emit_call(func, reloc_enabled, import_ids["callargs_expand_kwstar"]);
            if let Some(out_name) = op.out.as_ref() {
                let res = locals[out_name];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_append" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_append"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_pop" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(func, reloc_enabled, import_ids["list_pop"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_extend" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["list_extend"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_insert" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_insert"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_remove" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_remove"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_clear" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_clear"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_copy" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_copy"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_reverse" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_reverse"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_count" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_count"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_index" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_index"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "list_index_range" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            let start = locals[&args[2]];
            let stop = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            emit_call(func, reloc_enabled, import_ids["list_index_range"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "tuple_from_list" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["tuple_from_list"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_new" => {
            let empty_args_dn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_dn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
            emit_call(func, reloc_enabled, import_ids["dict_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for pair in args.chunks(2) {
                let key = locals[&pair[0]];
                let val = locals[&pair[1]];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(key));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["dict_set"]);
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        "dict_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["dict_from_obj"]);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(out));
        }
        "set_new" => {
            let empty_args_sn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_sn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(args.len() as i64));
            emit_call(func, reloc_enabled, import_ids["set_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for name in args {
                let val = locals[name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["set_add"]);
                func.instruction(&Instruction::Drop);
            }
        }
        "frozenset_new" => {
            let empty_args_fn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_fn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(args.len() as i64));
            emit_call(func, reloc_enabled, import_ids["frozenset_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for name in args {
                let val = locals[name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_get" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            emit_call(func, reloc_enabled, import_ids["dict_get"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_inc" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let delta = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["dict_inc"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_str_int_inc" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let delta = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["dict_str_int_inc"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_ws_dict_inc" => {
            let args = op.args.as_ref().unwrap();
            let line = locals[&args[0]];
            let dict = locals[&args[1]];
            let delta = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(line));
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["string_split_ws_dict_inc"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "taq_ingest_line" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let line = locals[&args[1]];
            let bucket_size = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(line));
            func.instruction(&Instruction::LocalGet(bucket_size));
            emit_call(func, reloc_enabled, import_ids["taq_ingest_line"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_sep_dict_inc" => {
            let args = op.args.as_ref().unwrap();
            let line = locals[&args[0]];
            let sep = locals[&args[1]];
            let dict = locals[&args[2]];
            let delta = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(line));
            func.instruction(&Instruction::LocalGet(sep));
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["string_split_sep_dict_inc"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_pop" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            let has_default = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            func.instruction(&Instruction::LocalGet(has_default));
            emit_call(func, reloc_enabled, import_ids["dict_pop"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_setdefault" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            emit_call(func, reloc_enabled, import_ids["dict_setdefault"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_setdefault_empty_list" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(
                func,
                reloc_enabled,
                import_ids["dict_setdefault_empty_list"],
            );
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_update" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["dict_update"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_clear" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_clear"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_copy" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_copy"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_popitem" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_popitem"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_update_kwstar" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["dict_update_kwstar"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_add" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(func, reloc_enabled, import_ids["set_add"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_add_probe" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(func, reloc_enabled, import_ids["set_add_probe"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "frozenset_add" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_discard" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(func, reloc_enabled, import_ids["set_discard"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_remove" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(func, reloc_enabled, import_ids["set_remove"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_pop" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(set_bits));
            emit_call(func, reloc_enabled, import_ids["set_pop"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_update" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["set_update"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_intersection_update" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["set_intersection_update"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_difference_update" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["set_difference_update"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_symdiff_update" => {
            let args = op.args.as_ref().unwrap();
            let set_bits = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(set_bits));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["set_symdiff_update"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_keys" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_keys"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_values" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_values"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dict_items" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_items"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "tuple_count" => {
            let args = op.args.as_ref().unwrap();
            let tuple = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(tuple));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["tuple_count"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "tuple_index" => {
            let args = op.args.as_ref().unwrap();
            let tuple_var = &args[0];
            let res = locals[op.out.as_ref().unwrap()];
            // Multi-value return (Section 3.1): if the tuple was
            // produced by a promoted call_internal, the values
            // are already in dedicated locals.
            if multi_ret_call_vars.contains(tuple_var) {
                let idx = op.value.unwrap_or(0);
                if let Some(&src_local) = multi_ret_call_locals.get(&(tuple_var.clone(), idx)) {
                    func.instruction(&Instruction::LocalGet(src_local));
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    let tuple = locals[tuple_var];
                    let val = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(tuple));
                    func.instruction(&Instruction::LocalGet(val));
                    emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                    func.instruction(&Instruction::LocalSet(res));
                }
            } else {
                let tuple = locals[tuple_var];
                let val = locals[&args[1]];
                func.instruction(&Instruction::LocalGet(tuple));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                func.instruction(&Instruction::LocalSet(res));
            }
        }
        "unpack_sequence" => {
            // args[0] is the sequence, args[1..] are output variable names.
            // op.value holds the expected element count.
            // The sequence may be a list (from _emit_list_from_iter) or
            // a tuple, so use the general-purpose `index` import which
            // handles both via __getitem__.
            let args = op.args.as_ref().unwrap();
            let seq = locals[&args[0]];
            let expected_count = op.value.unwrap() as usize;
            for i in 0..expected_count {
                let out = locals[&args[1 + i]];
                func.instruction(&Instruction::LocalGet(seq));
                func.instruction(&Instruction::I64Const(box_int(i as i64)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        "iter" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["iter"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "enumerate" => {
            let args = op.args.as_ref().unwrap();
            let iterable = locals[&args[0]];
            let start = locals[&args[1]];
            let has_start = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(iterable));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(has_start));
            emit_call(func, reloc_enabled, import_ids["enumerate"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "aiter" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["aiter"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "iter_next_unboxed" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            let pair = locals["__molt_tmp0"];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["iter_next"]);
            func.instruction(&Instruction::LocalSet(pair));
            if let Some(done_name) = op.out.as_ref()
                && done_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(1)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(locals[done_name]));
            }
            if let Some(val_name) = op.var.as_ref()
                && val_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(0)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(locals[val_name]));
            }
            func.instruction(&Instruction::LocalGet(pair));
            emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
        }
        "iter_next" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["iter_next"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "anext" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["anext"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "asyncgen_new" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(gen_local));
            emit_call(func, reloc_enabled, import_ids["asyncgen_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "asyncgen_shutdown" => {
            emit_call(func, reloc_enabled, import_ids["asyncgen_shutdown"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "gen_send" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(gen_local));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["generator_send"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "gen_throw" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(gen_local));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["generator_throw"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "gen_close" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(gen_local));
            emit_call(func, reloc_enabled, import_ids["generator_close"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "is_generator" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_generator"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "is_bound_method" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_bound_method"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "is_callable" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_callable"]);
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
