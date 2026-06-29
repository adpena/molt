use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_function_object_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "func_new" => {
            emit_function_constructor(call_ctx, func, op, "func_new", "func_new", None);
            CallOpEmission::Handled
        }
        "func_new_closure" => {
            let closure_name = op
                .args
                .as_ref()
                .and_then(|args| args.first())
                .expect("func_new_closure expects closure arg");
            let closure_bits = call_ctx.locals[closure_name];
            emit_function_constructor(
                call_ctx,
                func,
                op,
                "func_new_closure",
                "func_new_closure",
                Some(closure_bits),
            );
            CallOpEmission::Handled
        }
        "builtin_func" => emit_builtin_func(call_ctx, func, op),
        "missing" => {
            let out = call_ctx.locals[op.out.as_ref().unwrap()];
            emit_call(func, call_ctx.reloc_enabled, call_ctx.import_ids["missing"]);
            func.instruction(&Instruction::LocalSet(out));
            CallOpEmission::Handled
        }
        "function_closure_bits" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = call_ctx.locals[&args[0]];
            let out = call_ctx.locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids["function_closure_bits"],
            );
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::LocalGet(out));
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids["inc_ref_obj"],
            );
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_builtin_func(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    if op.s_value.as_deref() == Some("molt_require_intrinsic_runtime")
        && op
            .out
            .as_ref()
            .is_some_and(|out| call_ctx.runtime_lookup_only_vars.contains(out))
    {
        return CallOpEmission::Handled;
    }
    emit_function_constructor(call_ctx, func, op, "builtin_func", "func_new_builtin", None);
    CallOpEmission::Handled
}

fn emit_function_constructor(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    table_context: &str,
    import_name: &str,
    trailing_local: Option<u32>,
) {
    let func_name = op.s_value.as_ref().unwrap();
    let arity = op.value.unwrap_or(0);
    let table_pair = call_ctx
        .call_site_abi
        .callable_table_pair(func_name, table_context);
    emit_table_index_i64(
        func,
        call_ctx.reloc_enabled,
        table_pair.function_table_index,
    );
    emit_table_index_i64(
        func,
        call_ctx.reloc_enabled,
        table_pair.trampoline_table_index,
    );
    func.instruction(&Instruction::I64Const(arity));
    if let Some(local) = trailing_local {
        func.instruction(&Instruction::LocalGet(local));
    }
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[import_name],
    );
    if let Some(out) = op.out.as_ref() {
        let res = call_ctx.locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}
