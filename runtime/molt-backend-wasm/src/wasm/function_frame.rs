mod anchors;
mod emission;
mod planning;

use super::frame_locals::{WasmDispatchFrameLocals, WasmFrameLocals};
use super::local_analysis::ValueOccupancy;
use super::state_dispatch::NonLinearControlPlan;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_values::ConstantCache;
pub(in crate::wasm) use anchors::{WasmConstAnchorRegion, WasmConstAnchorSlot};
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
}

pub(super) struct WasmFunctionFramePlan {
    local_types: Vec<ValType>,
    frame: WasmFunctionFrame,
}

/// A function's view of its frame's binding homes (`molt_frame_homes`).
#[derive(Clone, Copy)]
pub(super) struct WasmFrameHomes {
    /// The i32 base address the frame lent, once per activation.
    local: u32,
    /// i64 scratch: the bits a home store or clear displaces, released once
    /// the new pair is published.
    displaced: u32,
    /// The code slots this function addresses: one past its largest.
    slots: i64,
    /// A split chunk has no frame entry of its own: it borrows the homes of
    /// the frame it runs in at function entry.
    at_entry: bool,
}

pub(super) struct WasmFunctionFrame {
    locals: WasmFrameLocals,
    /// Exact value occupancy behind shared locals; absent when every name
    /// keeps its own local.
    value_occupancy: Option<ValueOccupancy>,
    runtime_lookup_only_vars: BTreeSet<String>,
    scalar_plan: ScalarRepresentationPlan,
    guard_facts: molt_tir::passes::RuntimeGuardFacts,
    guard_profile_local: Option<u32>,
    control_mode: WasmFrameControlMode,
    control_plan: Option<NonLinearControlPlan>,
    tail_call_eligible: bool,
    // Per-invocation custody, never a persistent generator/task field. A failed
    // entry also creates a runtime attempt marker that this invocation owns.
    owned_frame_attempt: Option<u32>,
    // The binding homes that own a synchronous Python frame's bindings.
    frame_homes: Option<WasmFrameHomes>,
    // Native EH is local control flow. Python exceptions cross function ABIs
    // only through the runtime's pending state, including calls from Rust.
    python_eh_boundary: bool,
    dispatch_locals: Option<WasmDispatchFrameLocals>,
    /// Routing local of a multi-level dispatch scope tree.
    route_local: Option<u32>,
    const_cache: ConstantCache,
    const_seed_locals: Vec<(u32, i64)>,
    const_anchors: Vec<FrameConstAnchor>,
    const_anchor_region: Option<WasmConstAnchorRegion>,
    const_anchor_by_op_index: BTreeMap<usize, WasmConstAnchorSlot>,
}

impl WasmFunctionFrame {
    pub(super) fn guard_profile_local(&self, op: &crate::OpIR) -> Option<u32> {
        self.guard_facts
            .is_profile_only(op)
            .then_some(self.guard_profile_local)
            .flatten()
    }

    pub(super) fn control_mode(&self) -> WasmFrameControlMode {
        self.control_mode
    }

    pub(super) fn dispatch_locals(&self) -> Option<WasmDispatchFrameLocals> {
        self.dispatch_locals
    }

    pub(super) fn route_local(&self) -> Option<u32> {
        self.route_local
    }

    pub(super) fn control_plan(&self) -> Option<&NonLinearControlPlan> {
        self.control_plan.as_ref()
    }

    pub(super) fn locals(&self) -> &WasmFrameLocals {
        &self.locals
    }

    pub(super) fn value_occupancy(&self) -> Option<&ValueOccupancy> {
        self.value_occupancy.as_ref()
    }

    pub(super) fn runtime_lookup_only_vars(&self) -> &BTreeSet<String> {
        &self.runtime_lookup_only_vars
    }

    pub(super) fn const_anchor_for_op(&self, op_idx: usize) -> Option<WasmConstAnchorSlot> {
        self.const_anchor_by_op_index.get(&op_idx).copied()
    }

    pub(in crate::wasm) fn const_anchor_region(&self) -> WasmConstAnchorRegion {
        self.const_anchor_region
            .expect("constant anchor without its invocation region")
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
