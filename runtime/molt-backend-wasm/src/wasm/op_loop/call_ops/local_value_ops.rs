use super::super::result_sink::store_borrowed_value;
use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use wasm_encoder::{Function, Instruction};

/// Lower representation aliases that are not ordinary local-slot moves.
/// `local_slot_ops` is the sole store/load/copy ownership authority.
pub(super) fn emit_conversion_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "box" | "unbox" | "cast" | "widen" => {
            emit_conversion_alias(call_ctx, func, op);
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_conversion_alias(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("conversion args missing");
    let src_name = args_names
        .first()
        .expect("conversion op requires one source arg");
    let src = call_ctx.locals[src_name];
    if let Some(out) = call_ctx.locals.bound_op_result_slot(op) {
        func.instruction(&Instruction::LocalGet(src));
        store_borrowed_value(func, Some(out), call_ctx.import_ids, call_ctx.reloc_enabled);
    }
}
