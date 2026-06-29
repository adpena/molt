use super::super::module_abi::WasmCallableCallSiteAbi;
use super::call_emit::{OpLoopRuntimeCallContext, emit_op_loop_runtime_call};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::op_loop_runtime_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use wasm_encoder::Function;

mod async_task_ops;
mod exception_ops;
mod linear_memory_ops;

pub(super) struct RuntimeServiceOpContext<'a> {
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'a>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
}

impl RuntimeServiceOpContext<'_> {
    pub(super) fn op_loop_call_context(&self) -> OpLoopRuntimeCallContext<'_> {
        OpLoopRuntimeCallContext {
            import_ids: self.import_ids,
            locals: self.locals,
            reloc_enabled: self.reloc_enabled,
        }
    }
}

pub(super) fn emit_runtime_service_op(
    context: RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = op_loop_runtime_call(op.kind.as_str()) {
        emit_op_loop_runtime_call(&context.op_loop_call_context(), func, op, call);
        return true;
    }
    if async_task_ops::emit_async_task_runtime_op(&context, func, op) {
        return true;
    }
    if exception_ops::emit_exception_runtime_op(&context, func, op) {
        return true;
    }
    linear_memory_ops::emit_linear_memory_runtime_op(&context, func, op)
}
