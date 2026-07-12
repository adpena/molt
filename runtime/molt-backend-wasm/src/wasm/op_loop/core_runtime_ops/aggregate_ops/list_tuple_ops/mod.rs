mod list_ops;
mod tuple_ops;
mod unpack_ops;

use super::AggregateRuntimeContext;
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_list_tuple_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    match op.kind.as_str() {
        "build_list" | "list_new" => list_ops::emit_list_op(func, op, ctx),
        "tuple_new" | "tuple_index" => tuple_ops::emit_tuple_op(func, op, ctx),
        "unpack_sequence" => unpack_ops::emit_unpack_sequence(func, op, ctx),
        _ => return false,
    }
    true
}
