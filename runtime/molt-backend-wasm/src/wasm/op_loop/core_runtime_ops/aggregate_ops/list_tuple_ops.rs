use super::super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_int;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_list_tuple_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "build_list" | "list_new" => {
            let empty_args_ln: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_ln);
            let out = locals[op.out.as_ref().unwrap()];
            emit_sequence_builder_from_args(
                func,
                args,
                out,
                import_ids,
                locals,
                reloc_enabled,
                BuilderFinish::List,
            );
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
            let callee_value_locals = ctx.multi_return.callee_value_locals();
            if ctx.multi_return.is_callee_tuple_var(out_name)
                && args.len() == callee_value_locals.len()
            {
                for (k, arg_name) in args.iter().enumerate() {
                    let val = locals[arg_name];
                    func.instruction(&Instruction::LocalGet(val));
                    func.instruction(&Instruction::LocalSet(callee_value_locals[k]));
                }
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::LocalSet(out));
            } else {
                emit_sequence_builder_from_args(
                    func,
                    args,
                    out,
                    import_ids,
                    locals,
                    reloc_enabled,
                    BuilderFinish::Tuple,
                );
            }
        }
        "tuple_index" => {
            let args = op.args.as_ref().unwrap();
            let tuple_var = &args[0];
            let res = locals[op.out.as_ref().unwrap()];
            // Multi-value return (Section 3.1): if the tuple was
            // produced by a promoted call_internal, the values
            // are already in dedicated locals.
            if ctx.multi_return.is_promoted_call_tuple(tuple_var) {
                let idx = op.value.unwrap_or(0);
                if let Some(src_local) = ctx.multi_return.promoted_call_value_local(tuple_var, idx)
                {
                    func.instruction(&Instruction::LocalGet(src_local));
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    let tuple = locals[tuple_var];
                    let val = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(tuple));
                    func.instruction(&Instruction::LocalGet(val));
                    emit_call(
                        func,
                        reloc_enabled,
                        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TupleIndex],
                    );
                    func.instruction(&Instruction::LocalSet(res));
                }
            } else {
                let tuple = locals[tuple_var];
                let val = locals[&args[1]];
                func.instruction(&Instruction::LocalGet(tuple));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TupleIndex],
                );
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
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
                );
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        _ => return false,
    }
    true
}
