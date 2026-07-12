use super::super::multi_return_layout::WasmMultiReturnLayout;
use super::call_emit::{OpLoopRuntimeCallContext, emit_op_loop_runtime_call};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::op_loop_runtime_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};
use wasm_encoder::Function;

#[path = "core_runtime_ops/aggregate_ops.rs"]
mod aggregate_ops;
#[path = "core_runtime_ops/allocation_ops.rs"]
mod allocation_ops;
#[path = "core_runtime_ops/data_runtime_ops.rs"]
mod data_runtime_ops;
#[path = "core_runtime_ops/guard_ops.rs"]
mod guard_ops;
#[path = "core_runtime_ops/runtime_effect_ops.rs"]
mod runtime_effect_ops;
#[path = "core_runtime_ops/sequence_ops.rs"]
mod sequence_ops;
#[path = "core_runtime_ops/truth_ops.rs"]
mod truth_ops;

#[allow(unused_variables)]
pub(super) fn emit_core_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    multi_return: &WasmMultiReturnLayout,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    let call_context = OpLoopRuntimeCallContext {
        import_ids,
        locals,
        reloc_enabled,
    };
    if let Some(call) = op_loop_runtime_call(op.kind.as_str()) {
        emit_op_loop_runtime_call(&call_context, func, op, call);
        return true;
    }

    if aggregate_ops::emit_aggregate_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        multi_return,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }
    if sequence_ops::emit_sequence_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }
    if data_runtime_ops::emit_data_runtime_op(
        func,
        op,
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        reloc_enabled,
        arena_local,
        ops,
        op_idx,
    ) {
        return true;
    }
    if runtime_effect_ops::emit_runtime_effect_op(func, op, import_ids, locals, reloc_enabled) {
        return true;
    }
    if truth_ops::emit_truth_runtime_op(func, op, import_ids, locals, scalar_plan, reloc_enabled) {
        return true;
    }
    if guard_ops::emit_guard_runtime_op(func, op, import_ids, locals, reloc_enabled) {
        return true;
    }
    if allocation_ops::emit_allocation_runtime_op(
        func,
        op,
        import_ids,
        locals,
        reloc_enabled,
        arena_local,
    ) {
        return true;
    }
    false
}
