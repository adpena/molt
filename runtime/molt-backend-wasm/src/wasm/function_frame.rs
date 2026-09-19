mod emission;
mod planning;

use super::frame_locals::{WasmDispatchFrameLocals, WasmFrameLocals};
use super::state_dispatch::NonLinearDispatchLocals;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_values::ConstantCache;
use planning::FrameConstAnchor;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameControlMode {
    Plain,
    Jumpful,
    Stateful,
}

impl WasmFrameControlMode {
    pub(in crate::wasm) fn native_eh_enabled(self, requested: bool, reloc_enabled: bool) -> bool {
        requested && !reloc_enabled && matches!(self, Self::Plain)
    }

    pub(in crate::wasm) fn is_stateful(self) -> bool {
        matches!(self, Self::Stateful)
    }

    fn needs_dispatch(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

pub(super) struct WasmFunctionFramePlan {
    local_types: Vec<ValType>,
    frame: WasmFunctionFrame,
}

pub(super) struct WasmFunctionFrame {
    locals: WasmFrameLocals,
    runtime_lookup_only_vars: BTreeSet<String>,
    scalar_plan: ScalarRepresentationPlan,
    control_mode: WasmFrameControlMode,
    tail_call_eligible: bool,
    dispatch_locals: Option<WasmDispatchFrameLocals>,
    const_cache: ConstantCache,
    const_seed_locals: Vec<(u32, i64)>,
    const_anchors: Vec<FrameConstAnchor>,
    const_anchor_by_op_index: BTreeMap<usize, u32>,
}

impl WasmFunctionFrame {
    pub(super) fn control_mode(&self) -> WasmFrameControlMode {
        self.control_mode
    }

    pub(super) fn dispatch_locals(&self) -> Option<NonLinearDispatchLocals> {
        self.dispatch_locals.map(|locals| NonLinearDispatchLocals {
            state_local: locals.state_local,
            resume_state_local: locals.resume_state_local,
            block_map_base_local: locals.block_map_base_local,
            return_local: locals.return_local,
            self_ptr_local: locals.self_ptr_local,
            state_remap_base_local: locals.state_remap_base_local,
            state_remap_value_local: locals.state_remap_value_local,
        })
    }

    pub(super) fn locals(&self) -> &WasmFrameLocals {
        &self.locals
    }

    pub(super) fn runtime_lookup_only_vars(&self) -> &BTreeSet<String> {
        &self.runtime_lookup_only_vars
    }

    pub(super) fn const_anchor_for_op(&self, op_idx: usize) -> Option<u32> {
        self.const_anchor_by_op_index.get(&op_idx).copied()
    }

    pub(super) fn const_anchor_locals(
        &self,
    ) -> impl DoubleEndedIterator<Item = u32> + ExactSizeIterator + '_ {
        self.const_anchors.iter().map(|anchor| anchor.local)
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        &self.const_cache
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        &self.scalar_plan
    }

    pub(super) fn tail_call_eligible(&self) -> bool {
        self.tail_call_eligible
    }
}
