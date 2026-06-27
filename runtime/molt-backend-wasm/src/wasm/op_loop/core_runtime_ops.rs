use super::super::multi_return_layout::WasmMultiReturnLayout;
use super::*;

#[path = "core_runtime_ops/aggregate_ops.rs"]
mod aggregate_ops;
#[path = "core_runtime_ops/data_runtime_ops.rs"]
mod data_runtime_ops;
#[path = "core_runtime_ops/sequence_ops.rs"]
mod sequence_ops;

#[allow(unused_variables)]
pub(super) fn emit_core_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    multi_return: &WasmMultiReturnLayout,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    if aggregate_ops::emit_aggregate_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        multi_return,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }
    if sequence_ops::emit_sequence_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }
    if data_runtime_ops::emit_data_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }

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
        _ => return false,
    }
    true
}
