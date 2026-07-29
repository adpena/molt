use super::super::control_flow::{ControlKind, dispatch_control_panic};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::is_shared_drop_fact_marker;
use crate::wasm_values::ConstantCache;
use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::Function;

mod branches;
mod exceptions;
mod loops;
mod returns;

pub(super) struct ControlOpContext<'a> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) scalar_plan: &'a ScalarRepresentationPlan,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) control_stack: &'a mut Vec<ControlKind>,
    pub(super) try_stack: &'a mut Vec<usize>,
    pub(super) label_stack: &'a mut Vec<i64>,
    pub(super) label_depths: &'a mut BTreeMap<i64, usize>,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) op_idx: usize,
}

pub(super) fn emit_control_op(
    mut control_ctx: ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) {
    if returns::emit_return_control_op(&control_ctx, func, op) {
        return;
    }
    if branches::emit_branch_control_op(&mut control_ctx, func, op) {
        return;
    }
    if loops::emit_loop_control_op(&mut control_ctx, func, op) {
        return;
    }
    if exceptions::emit_exception_control_op(&mut control_ctx, func, op) {
        return;
    }
    if is_shared_drop_fact_marker(op.kind.as_str()) {
        return;
    }

    dispatch_control_panic(
        &control_ctx.func_ir.name,
        control_ctx.op_idx,
        format_args!("unsupported op kind `{}`", op.kind),
    );
}
