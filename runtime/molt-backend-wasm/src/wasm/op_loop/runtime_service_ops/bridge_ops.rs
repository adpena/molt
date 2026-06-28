use super::RuntimeServiceOpContext;
use super::call_emit::{RuntimeServiceArg::Local, RuntimeServiceCall, emit_runtime_service_call};
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_bridge_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = bridge_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }
    false
}

fn bridge_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "bridge_unavailable" => RuntimeServiceCall::result("bridge_unavailable", &[Local(0)]),
        _ => return None,
    })
}
