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
    match op.kind {
        molt_ir::OpKind::BuildList | molt_ir::OpKind::ListNew => {
            list_ops::emit_list_op(func, op, ctx)
        }
        molt_ir::OpKind::TupleNew | molt_ir::OpKind::TupleIndex => {
            tuple_ops::emit_tuple_op(func, op, ctx)
        }
        molt_ir::OpKind::UnpackSequence => unpack_ops::emit_unpack_sequence(func, op, ctx),
        _ => return false,
    }
    true
}
