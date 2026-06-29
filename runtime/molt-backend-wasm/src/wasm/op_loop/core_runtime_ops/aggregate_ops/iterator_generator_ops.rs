use super::AggregateRuntimeContext;
use crate::OpIR;
use wasm_encoder::Function;

#[path = "iterator_generator_ops/iterator_ops.rs"]
mod iterator_ops;

pub(super) fn emit_iterator_generator_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    iterator_ops::emit_iterator_op(func, op, ctx)
}
