use super::context::CompileFuncContext;
use super::function_frame::WasmFunctionFrame;
use super::module_abi::WasmCallableCallSiteAbi;
use super::{WasmBackend, WasmFrameLocals};
use crate::FunctionIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

mod builder_ops;
mod call_emit;
mod call_ops;
mod control_ops;
mod core_runtime_ops;
mod emit_sequence;
mod local_slot_ops;
mod local_state_ops;
mod numeric_ops;
mod object_attr_ops;
mod result_sink;
mod runtime_service_ops;

pub(super) struct WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) func_ir: &'a FunctionIR,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) frame: &'a WasmFunctionFrame,
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) tail_call_count: &'a Cell<usize>,
}

impl<'a, 'ctx> WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) fn locals(&self) -> &WasmFrameLocals {
        self.frame.locals()
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        self.frame.const_cache()
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        self.frame.scalar_plan()
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.frame.arena_local()
    }
}
