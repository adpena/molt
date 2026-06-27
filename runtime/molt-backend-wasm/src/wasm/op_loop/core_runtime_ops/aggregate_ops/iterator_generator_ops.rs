use super::super::super::*;
use super::AggregateRuntimeContext;

#[path = "iterator_generator_ops/generator_protocol_ops.rs"]
mod generator_protocol_ops;
#[path = "iterator_generator_ops/iterator_ops.rs"]
mod iterator_ops;

pub(super) fn emit_iterator_generator_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    if iterator_ops::emit_iterator_op(func, op, ctx) {
        return true;
    }
    generator_protocol_ops::emit_generator_protocol_op(func, op, ctx)
}
