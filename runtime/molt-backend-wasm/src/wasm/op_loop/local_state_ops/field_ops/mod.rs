use super::LocalStateOpContext;
use crate::OpIR;
use wasm_encoder::Function;

mod common;
mod guarded;
mod plain;

pub(super) fn emit_field_local_state_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if plain::emit_plain_field_op(context, func, op) {
        return true;
    }
    guarded::emit_guarded_field_op(context, func, op)
}
