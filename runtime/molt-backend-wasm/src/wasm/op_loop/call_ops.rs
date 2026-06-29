use super::super::module_abi::WasmCallableCallSiteAbi;
use super::super::multi_return_layout::WasmMultiReturnLayout;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use crate::{FunctionIR, OpIR};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use wasm_encoder::Function;

mod code_metadata;
mod direct;
mod dynamic;
mod function_object;
mod gpu_ops;
mod local_value_ops;
mod refcount_ops;
mod site;

pub(super) enum CallOpEmission {
    NotHandled,
    Handled,
    HandledAndSkipNext,
}

pub(super) struct CallOpContext<'a, 'ctx, 'm> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) runtime_lookup_only_vars: &'a BTreeSet<String>,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    pub(super) multi_return: &'a WasmMultiReturnLayout,
    pub(super) reloc_enabled: bool,
    pub(super) tail_call_eligible: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) tail_call_count: &'a Cell<usize>,
    pub(super) ops: &'a [OpIR],
    pub(super) last_use_local: &'m BTreeMap<String, usize>,
    pub(super) rc_skip_inc: &'m HashSet<usize>,
    pub(super) rc_skip_dec: &'m HashSet<String>,
    pub(super) rel_idx: usize,
    pub(super) op_idx: usize,
    pub(super) try_stack_is_empty: bool,
}

pub(super) fn emit_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    if let Some(emission) = handled(direct::emit_direct_call_op(call_ctx, func, op)) {
        return emission;
    }
    if let Some(emission) = handled(gpu_ops::emit_gpu_call_op(call_ctx, func, op)) {
        return emission;
    }
    if let Some(emission) = handled(refcount_ops::emit_refcount_call_op(call_ctx, func, op)) {
        return emission;
    }
    if let Some(emission) = handled(local_value_ops::emit_local_value_call_op(
        call_ctx, func, op,
    )) {
        return emission;
    }
    if let Some(emission) = handled(dynamic::emit_dynamic_call_op(call_ctx, func, op)) {
        return emission;
    }
    if let Some(emission) = handled(function_object::emit_function_object_call_op(
        call_ctx, func, op,
    )) {
        return emission;
    }
    if let Some(emission) = handled(code_metadata::emit_code_metadata_call_op(
        call_ctx, func, op,
    )) {
        return emission;
    }

    CallOpEmission::NotHandled
}

fn handled(emission: CallOpEmission) -> Option<CallOpEmission> {
    match emission {
        CallOpEmission::NotHandled => None,
        CallOpEmission::Handled | CallOpEmission::HandledAndSkipNext => Some(emission),
    }
}
