use super::RuntimeServiceOpContext;
use super::call_emit::{RuntimeServiceArg::Local, RuntimeServiceCall, emit_runtime_service_call};
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_channel_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = channel_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }
    false
}

fn channel_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "chan_new" => RuntimeServiceCall::result("chan_new", &[Local(0)]),
        "chan_drop" => RuntimeServiceCall::drop("chan_drop", &[Local(0)]),
        _ => return None,
    })
}
