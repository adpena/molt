use super::super::module_abi::WasmCallableCallSiteAbi;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use wasm_encoder::Function;

mod async_task_ops;
mod bridge_ops;
mod channel_ops;
mod exception_ops;
mod file_ops;
mod linear_memory_ops;
mod module_ops;

pub(super) struct RuntimeServiceOpContext<'a> {
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'a>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
}

pub(super) fn emit_runtime_service_op(
    context: RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if channel_ops::emit_channel_runtime_op(&context, func, op) {
        return true;
    }
    if module_ops::emit_module_runtime_op(&context, func, op) {
        return true;
    }
    if async_task_ops::emit_async_task_runtime_op(&context, func, op) {
        return true;
    }
    if exception_ops::emit_exception_runtime_op(&context, func, op) {
        return true;
    }
    if bridge_ops::emit_bridge_runtime_op(&context, func, op) {
        return true;
    }
    if file_ops::emit_file_runtime_op(&context, func, op) {
        return true;
    }
    linear_memory_ops::emit_linear_memory_runtime_op(&context, func, op)
}
