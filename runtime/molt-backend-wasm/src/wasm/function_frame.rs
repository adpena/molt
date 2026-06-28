mod emission;
mod planning;

use super::frame_locals::{WasmDispatchFrameLocals, WasmFrameLocals};
use super::multi_return_layout::WasmMultiReturnLayout;
use super::state_dispatch::NonLinearDispatchLocals;
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeSet;
use wasm_encoder::ValType;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameControlMode {
    Plain,
    Jumpful,
    Stateful,
}

impl WasmFrameControlMode {
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
    arena_local: Option<u32>,
    dispatch_locals: Option<WasmDispatchFrameLocals>,
    const_cache: ConstantCache,
    const_seed_locals: Vec<(u32, i64)>,
    seeded_runtime_const_ops: Vec<(usize, OpIR)>,
    seeded_runtime_const_op_indices: BTreeSet<usize>,
    multi_return: WasmMultiReturnLayout,
}

impl WasmFunctionFrame {
    pub(super) fn control_mode(&self) -> WasmFrameControlMode {
        self.control_mode
    }

    pub(super) fn dispatch_locals(&self) -> Option<NonLinearDispatchLocals> {
        self.dispatch_locals.map(|locals| NonLinearDispatchLocals {
            state_local: locals.state_local,
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

    pub(super) fn seeded_runtime_const_op_indices(&self) -> &BTreeSet<usize> {
        &self.seeded_runtime_const_op_indices
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        &self.const_cache
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        &self.scalar_plan
    }

    pub(super) fn multi_return(&self) -> &WasmMultiReturnLayout {
        &self.multi_return
    }

    pub(super) fn tail_call_eligible(&self) -> bool {
        self.tail_call_eligible
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.arena_local
    }
}
