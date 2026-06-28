use super::RuntimeServiceOpContext;
use super::call_emit::{RuntimeServiceArg::Local, RuntimeServiceCall, emit_runtime_service_call};
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_file_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = file_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }
    false
}

fn file_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "file_open" => RuntimeServiceCall::result("file_open", &[Local(0), Local(1)]),
        "file_read" => RuntimeServiceCall::result("file_read", &[Local(0), Local(1)]),
        "file_write" => RuntimeServiceCall::result("file_write", &[Local(0), Local(1)]),
        "file_close" => RuntimeServiceCall::result("file_close", &[Local(0)]),
        "file_flush" => RuntimeServiceCall::result("file_flush", &[Local(0)]),
        _ => return None,
    })
}
