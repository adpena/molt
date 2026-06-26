use super::*;

#[allow(unused_variables)]
pub(super) fn emit_core_runtime_op(
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
        "exception_pending" => {
            // Read the runtime exception-pending flag as a NaN-boxed
            // bool: `box_bool(molt_exception_pending() != 0)`.
            // Produced by the TIR `ExceptionPending` op (round-tripped
            // to SimpleIR by lower_to_simple when an iterator-consumer
            // loop carries a `loop_break_if_exception`); consumed as
            // the condition of the `br_if`/`if` that breaks the loop on
            // a mid-iteration raise.  Boxing to a proper bool (rather
            // than leaving the raw i64 0/1) is required because the
            // downstream `br_if`/`if` truthiness path calls
            // `is_truthy`, which interprets its operand as a NaN-boxed
            // value.  Non-foldable: it observes mutable runtime state.
            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            emit_box_bool_from_i32(func);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "function_defaults_version" => {
            // Read a function object's __defaults__/__kwdefaults__
            // mutation version stamp as a NaN-boxed inline int
            // (`molt_function_defaults_version(func)`).  Produced by
            // the compile-time defaults-devirt deopt guard; consumed
            // by its `== 0` compare (baked literal vs live read).
            // Non-foldable: it observes mutable runtime state.
            let args = op.args.as_ref().unwrap();
            let func_local = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_local));
            emit_call(func, reloc_enabled, import_ids["function_defaults_version"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "is" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            emit_call(func, reloc_enabled, import_ids["is"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "not" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["not"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bool" | "cast_bool" | "builtin_bool" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(&scalar_plan, &args[0])
            {
                "is_truthy_int"
            } else {
                "is_truthy"
            };
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids[truthy_import]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            emit_box_bool_from_i32(func);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "abs" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["abs_builtin"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "and" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            func.instruction(&Instruction::LocalGet(rhs));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::End);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                debug_assert!(
                    crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(
                        "and"
                    )
                );
                func.instruction(&Instruction::LocalTee(res));
                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "or" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(rhs));
            func.instruction(&Instruction::End);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                debug_assert!(
                    crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(
                        "or"
                    )
                );
                func.instruction(&Instruction::LocalTee(res));
                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "contains" => {
            let args = op.args.as_ref().unwrap();
            let container = locals[&args[0]];
            let item = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(container));
            func.instruction(&Instruction::LocalGet(item));
            let import_key =
                wasm_specialized_container_import(&scalar_plan, op_idx, "contains", op)
                    .unwrap_or("contains");
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
        "guard_type" | "guard_tag" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let expected = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["guard_type"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "guard_layout" | "guard_dict_shape" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_bits = locals[&args[1]];
            let expected = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "print" => {
            let args = op.args.as_ref().unwrap();
            if let Some(&idx) = locals.get(&args[0]) {
                func.instruction(&Instruction::LocalGet(idx));
                emit_call(func, reloc_enabled, import_ids["print_obj"]);
            }
        }
        "print_newline" => {
            emit_call(func, reloc_enabled, import_ids["print_newline"]);
        }
        "alloc" | "stack_alloc" => {
            // Arena fast path: NoEscape allocations marked
            // `arena_eligible` go through `molt_arena_alloc_object`
            // (same NaN-boxed contract as `molt_alloc` but bumps
            // out of the per-function ScopeArena). The arena is
            // freed once at every return in O(1).
            if op.arena_eligible == Some(true)
                && let Some(arena_idx) = arena_local
            {
                func.instruction(&Instruction::LocalGet(arena_idx));
                func.instruction(&Instruction::I64Const(op.value.unwrap()));
                emit_call(func, reloc_enabled, import_ids["arena_alloc_object"]);
            } else {
                func.instruction(&Instruction::I64Const(op.value.unwrap()));
                emit_call(func, reloc_enabled, import_ids["alloc"]);
            }
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "alloc_class" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["alloc_class"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "alloc_class_trusted" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["alloc_class_trusted"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "alloc_class_static" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["alloc_class_static"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "json_parse" => {
            let args = op.args.as_ref().unwrap();
            let arg_name = &args[0];
            if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                let ptr = locals
                    .get(&format!("{arg_name}_ptr"))
                    .copied()
                    .unwrap_or(locals[arg_name]);
                let tmp_rc = locals["__molt_tmp0"];

                func.instruction(&Instruction::I64Const(8));
                emit_call(func, reloc_enabled, import_ids["alloc"]);
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalSet(out_ptr));

                func.instruction(&Instruction::LocalGet(ptr));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(len));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                emit_call(func, reloc_enabled, import_ids["json_parse_scalar"]);
                func.instruction(&Instruction::I64ExtendI32U);
                func.instruction(&Instruction::LocalSet(tmp_rc));

                func.instruction(&Instruction::LocalGet(tmp_rc));
                func.instruction(&Instruction::I64Eqz);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::End);
            } else {
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
            }
        }
        "msgpack_parse" => {
            let args = op.args.as_ref().unwrap();
            let arg_name = &args[0];
            if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                let ptr = locals
                    .get(&format!("{arg_name}_ptr"))
                    .copied()
                    .unwrap_or(locals[arg_name]);
                let tmp_rc = locals["__molt_tmp0"];

                func.instruction(&Instruction::I64Const(8));
                emit_call(func, reloc_enabled, import_ids["alloc"]);
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalSet(out_ptr));

                func.instruction(&Instruction::LocalGet(ptr));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(len));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar"]);
                func.instruction(&Instruction::I64ExtendI32U);
                func.instruction(&Instruction::LocalSet(tmp_rc));

                func.instruction(&Instruction::LocalGet(tmp_rc));
                func.instruction(&Instruction::I64Eqz);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::End);
            } else {
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
            }
        }
        "cbor_parse" => {
            let args = op.args.as_ref().unwrap();
            let arg_name = &args[0];
            if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                let ptr = locals
                    .get(&format!("{arg_name}_ptr"))
                    .copied()
                    .unwrap_or(locals[arg_name]);
                let tmp_rc = locals["__molt_tmp0"];

                func.instruction(&Instruction::I64Const(8));
                emit_call(func, reloc_enabled, import_ids["alloc"]);
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalSet(out_ptr));

                func.instruction(&Instruction::LocalGet(ptr));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(len));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar"]);
                func.instruction(&Instruction::I64ExtendI32U);
                func.instruction(&Instruction::LocalSet(tmp_rc));

                func.instruction(&Instruction::LocalGet(tmp_rc));
                func.instruction(&Instruction::I64Eqz);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(out_ptr));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
                func.instruction(&Instruction::End);
            } else {
                let out_ptr = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalGet(locals[arg_name]));
                emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                func.instruction(&Instruction::LocalSet(out_ptr));
            }
        }
        "len" => {
            let args = op.args.as_ref().unwrap();
            let arg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(arg));
            // Dispatch to specialized fast-path len when container
            // type is known, skipping the 18-type dispatch.
            let import_key =
                wasm_specialized_container_import(&scalar_plan, op_idx, "len", op).unwrap_or("len");
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
        "id" => {
            let args = op.args.as_ref().unwrap();
            let arg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(arg));
            emit_call(func, reloc_enabled, import_ids["id"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "ord" => {
            let args = op.args.as_ref().unwrap();
            let arg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(arg));
            emit_call(func, reloc_enabled, import_ids["ord"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "ord_at" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let index = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(index));
            emit_call(func, reloc_enabled, import_ids["ord_at"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "chr" => {
            let args = op.args.as_ref().unwrap();
            let arg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(arg));
            emit_call(func, reloc_enabled, import_ids["chr"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "callargs_new" => {
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "build_list" | "list_new" => {
            let empty_args_ln: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_ln);
            let out = locals[op.out.as_ref().unwrap()];
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
            emit_call(func, reloc_enabled, import_ids["list_builder_finish"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "list_int_new" => {
            // Specialized flat i64 list: args = [count, fill_value]
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let count = locals[&args[0]];
            let fill = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(count));
            func.instruction(&Instruction::LocalGet(fill));
            emit_call(func, reloc_enabled, import_ids["list_int_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "list_fill_new" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let count = locals[&args[0]];
            let fill = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(count));
            func.instruction(&Instruction::LocalGet(fill));
            emit_call(func, reloc_enabled, import_ids["list_fill_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "range_new" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let start = locals[&args[0]];
            let stop = locals[&args[1]];
            let step = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(step));
            emit_call(func, reloc_enabled, import_ids["range_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
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
        "env_get" => {
            let args = op.args.as_ref().unwrap();
            let key = locals[&args[0]];
            let default = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            emit_call(func, reloc_enabled, import_ids["env_get"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "errno_constants" => {
            emit_call(func, reloc_enabled, import_ids["errno_constants"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_join" => {
            let args = op.args.as_ref().unwrap();
            let sep = locals[&args[0]];
            let items = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(sep));
            func.instruction(&Instruction::LocalGet(items));
            emit_call(func, reloc_enabled, import_ids["string_join"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_split"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_validate" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["string_split_validate"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let index = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(index));
            emit_call(func, reloc_enabled, import_ids["string_split_field"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field_len" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let index = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(index));
            emit_call(func, reloc_enabled, import_ids["string_split_field_len"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field_eq" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let index = locals[&args[2]];
            let expected = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(index));
            func.instruction(&Instruction::LocalGet(expected));
            emit_call(func, reloc_enabled, import_ids["string_split_field_eq"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field_start"
        | "string_split_field_end"
        | "string_split_field_is_ascii"
        | "string_split_field_to_int" => {
            // Split-field deforestation property/parse ops: 3 i64 args
            // (hay, sep, idx) -> i64. The runtime symbol is the op kind
            // prefixed with `molt_`.
            let args = op.args.as_ref().unwrap();
            for a in args.iter().take(3) {
                func.instruction(&Instruction::LocalGet(locals[a]));
            }
            let symbol: &str = match op.kind.as_str() {
                "string_split_field_start" => "string_split_field_start",
                "string_split_field_end" => "string_split_field_end",
                "string_split_field_is_ascii" => "string_split_field_is_ascii",
                _ => "string_split_field_to_int",
            };
            emit_call(func, reloc_enabled, import_ids[symbol]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field_len_from_bounds" => {
            // (hay, start, end, is_ascii) -> i64.
            let args = op.args.as_ref().unwrap();
            for a in args.iter().take(4) {
                func.instruction(&Instruction::LocalGet(locals[a]));
            }
            emit_call(
                func,
                reloc_enabled,
                import_ids["string_split_field_len_from_bounds"],
            );
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_field_ord_at_bounds" => {
            // (hay, start, end, is_ascii, idx) -> i64.
            let args = op.args.as_ref().unwrap();
            for a in args.iter().take(5) {
                func.instruction(&Instruction::LocalGet(locals[a]));
            }
            emit_call(
                func,
                reloc_enabled,
                import_ids["string_split_field_ord_at_bounds"],
            );
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_split_max" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let maxsplit = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(maxsplit));
            emit_call(func, reloc_enabled, import_ids["string_split_max"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "statistics_mean_slice" => {
            let args = op.args.as_ref().unwrap();
            let seq = locals[&args[0]];
            let start = locals[&args[1]];
            let end = locals[&args[2]];
            let has_start = locals[&args[3]];
            let has_end = locals[&args[4]];
            func.instruction(&Instruction::LocalGet(seq));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["statistics_mean_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "statistics_stdev_slice" => {
            let args = op.args.as_ref().unwrap();
            let seq = locals[&args[0]];
            let start = locals[&args[1]];
            let end = locals[&args[2]];
            let has_start = locals[&args[3]];
            let has_end = locals[&args[4]];
            func.instruction(&Instruction::LocalGet(seq));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(end));
            func.instruction(&Instruction::LocalGet(has_start));
            func.instruction(&Instruction::LocalGet(has_end));
            emit_call(func, reloc_enabled, import_ids["statistics_stdev_slice"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_lower" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(hay));
            emit_call(func, reloc_enabled, import_ids["string_lower"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_upper" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(hay));
            emit_call(func, reloc_enabled, import_ids["string_upper"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_capitalize" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(hay));
            emit_call(func, reloc_enabled, import_ids["string_capitalize"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_strip" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let chars = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(chars));
            emit_call(func, reloc_enabled, import_ids["string_strip"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_lstrip" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let chars = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(chars));
            emit_call(func, reloc_enabled, import_ids["string_lstrip"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_rstrip" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let chars = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(chars));
            emit_call(func, reloc_enabled, import_ids["string_rstrip"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_split" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytes_split"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_split_max" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let maxsplit = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(maxsplit));
            emit_call(func, reloc_enabled, import_ids["bytes_split_max"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_split" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            emit_call(func, reloc_enabled, import_ids["bytearray_split"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_split_max" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let maxsplit = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(maxsplit));
            emit_call(func, reloc_enabled, import_ids["bytearray_split_max"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_replace" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let replacement = locals[&args[2]];
            let count = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(replacement));
            func.instruction(&Instruction::LocalGet(count));
            emit_call(func, reloc_enabled, import_ids["bytes_replace"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_replace" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let replacement = locals[&args[2]];
            let count = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(replacement));
            func.instruction(&Instruction::LocalGet(count));
            emit_call(func, reloc_enabled, import_ids["string_replace"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_replace" => {
            let args = op.args.as_ref().unwrap();
            let hay = locals[&args[0]];
            let needle = locals[&args[1]];
            let replacement = locals[&args[2]];
            let count = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(hay));
            func.instruction(&Instruction::LocalGet(needle));
            func.instruction(&Instruction::LocalGet(replacement));
            func.instruction(&Instruction::LocalGet(count));
            emit_call(func, reloc_enabled, import_ids["bytearray_replace"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_fill_range" => {
            let args = op.args.as_ref().unwrap();
            let bytearray = locals[&args[0]];
            let start = locals[&args[1]];
            let stop = locals[&args[2]];
            let value = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(bytearray));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(value));
            emit_call(func, reloc_enabled, import_ids["bytearray_fill_range"]);
            if let Some(out) = op.out.as_ref()
                && out != "none"
            {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["bytes_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytes_from_str" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            let encoding = locals[&args[1]];
            let errors = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::LocalGet(encoding));
            func.instruction(&Instruction::LocalGet(errors));
            emit_call(func, reloc_enabled, import_ids["bytes_from_str"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["bytearray_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bytearray_from_str" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            let encoding = locals[&args[1]];
            let errors = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::LocalGet(encoding));
            func.instruction(&Instruction::LocalGet(errors));
            emit_call(func, reloc_enabled, import_ids["bytearray_from_str"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "float_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["float_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "int_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let base = locals[&args[1]];
            let has_base = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(base));
            func.instruction(&Instruction::LocalGet(has_base));
            emit_call(func, reloc_enabled, import_ids["int_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "int_from_str_of_obj" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let base = locals[&args[1]];
            let has_base = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(base));
            func.instruction(&Instruction::LocalGet(has_base));
            emit_call(func, reloc_enabled, import_ids["int_from_str_of_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "complex_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let imag = locals[&args[1]];
            let has_imag = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(imag));
            func.instruction(&Instruction::LocalGet(has_imag));
            emit_call(func, reloc_enabled, import_ids["complex_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "intarray_from_seq" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["intarray_from_seq"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "memoryview_new" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["memoryview_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "memoryview_tobytes" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["memoryview_tobytes"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "memoryview_cast" => {
            let args = op.args.as_ref().unwrap();
            let view = locals[&args[0]];
            let format = locals[&args[1]];
            let shape = locals[&args[2]];
            let has_shape = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(view));
            func.instruction(&Instruction::LocalGet(format));
            func.instruction(&Instruction::LocalGet(shape));
            func.instruction(&Instruction::LocalGet(has_shape));
            emit_call(func, reloc_enabled, import_ids["memoryview_cast"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "buffer2d_new" => {
            let args = op.args.as_ref().unwrap();
            let rows = locals[&args[0]];
            let cols = locals[&args[1]];
            let init = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(rows));
            func.instruction(&Instruction::LocalGet(cols));
            func.instruction(&Instruction::LocalGet(init));
            emit_call(func, reloc_enabled, import_ids["buffer2d_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "buffer2d_get" => {
            let args = op.args.as_ref().unwrap();
            let buf = locals[&args[0]];
            let row = locals[&args[1]];
            let col = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(buf));
            func.instruction(&Instruction::LocalGet(row));
            func.instruction(&Instruction::LocalGet(col));
            emit_call(func, reloc_enabled, import_ids["buffer2d_get"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "buffer2d_set" => {
            let args = op.args.as_ref().unwrap();
            let buf = locals[&args[0]];
            let row = locals[&args[1]];
            let col = locals[&args[2]];
            let val = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(buf));
            func.instruction(&Instruction::LocalGet(row));
            func.instruction(&Instruction::LocalGet(col));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["buffer2d_set"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "buffer2d_matmul" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            emit_call(func, reloc_enabled, import_ids["buffer2d_matmul"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "str_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["str_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "repr_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["repr_from_obj"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "ascii_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(src));
            emit_call(func, reloc_enabled, import_ids["ascii_from_obj"]);
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
