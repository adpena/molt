use super::super::control_flow::{DispatchControlMaps, build_dispatch_control_maps};
use super::super::function_frame::WasmFrameControlMode;
use super::block_layout::{build_dispatch_block_map, build_dispatch_blocks};
use super::state_remap::{
    build_dense_state_remap_table, build_state_resume_maps, stateful_entry_prologue_end,
};
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm_data::DataSegmentRef;
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm) struct NonLinearDispatchPlan {
    pub(super) block_starts: Vec<usize>,
    block_for_op: Vec<usize>,
    pub(super) block_map_segment: DataSegmentRef,
    pub(super) control_maps: DispatchControlMaps,
    pub(super) state_resume: Option<StateResumePlan>,
}

/// A static successor uses the forward labels already surrounding dispatch
/// blocks. Only a backward edge needs another dynamic dispatch iteration.
/// The operation-to-block projection is the same one used by resume lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StaticDispatchEdge {
    Forward { label_depth: u32 },
    Redispatch { operation: usize },
    ReturnNone,
}

pub(super) struct StateResumePlan {
    pub(super) state_map: BTreeMap<i64, usize>,
    pub(super) remap_table: Option<(i64, DataSegmentRef)>,
}

#[derive(Clone, Copy)]
pub(in crate::wasm) struct NonLinearDispatchLocals {
    pub(in crate::wasm) state_local: u32,
    pub(in crate::wasm) resume_state_local: Option<u32>,
    pub(in crate::wasm) block_map_base_local: u32,
    pub(in crate::wasm) self_ptr_local: Option<u32>,
    pub(in crate::wasm) state_remap_base_local: Option<u32>,
    pub(in crate::wasm) state_remap_value_local: Option<u32>,
}

impl NonLinearDispatchPlan {
    pub(in crate::wasm) fn build(
        backend: &mut WasmBackend,
        func_ir: &FunctionIR,
        reloc_enabled: bool,
        control_mode: WasmFrameControlMode,
    ) -> Option<Self> {
        if matches!(control_mode, WasmFrameControlMode::Plain) {
            return None;
        }
        let stateful = control_mode.is_stateful();

        let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
        let block_map_bytes = build_dispatch_block_map(&block_for_op);
        let block_map_segment = backend.add_data_segment(reloc_enabled, &block_map_bytes);
        let control_maps = build_dispatch_control_maps(&func_ir.ops, stateful, &func_ir.name);
        let state_resume = stateful.then(|| {
            let entry_prologue_end = stateful_entry_prologue_end(&func_ir.ops);
            let state_map = build_state_resume_maps(&func_ir.ops, entry_prologue_end + 1);
            let remap_table = build_dense_state_remap_table(&state_map).map(|remap_bytes| {
                let remap_entries = (remap_bytes.len() / std::mem::size_of::<i64>()) as i64;
                let remap_segment = backend.add_data_segment(reloc_enabled, &remap_bytes);
                (remap_entries, remap_segment)
            });
            StateResumePlan {
                state_map,
                remap_table,
            }
        });

        Some(Self {
            block_starts,
            block_for_op,
            block_map_segment,
            control_maps,
            state_resume,
        })
    }

    pub(in crate::wasm) fn emit_table_bases(
        &self,
        backend: &mut WasmBackend,
        func_index: u32,
        func: &mut Function,
        reloc_enabled: bool,
        locals: NonLinearDispatchLocals,
    ) {
        backend.emit_data_ptr(reloc_enabled, func_index, func, self.block_map_segment);
        func.instruction(&Instruction::LocalSet(locals.block_map_base_local));
        if let Some((_, remap_segment)) = self
            .state_resume
            .as_ref()
            .and_then(|resume| resume.remap_table.as_ref())
        {
            let remap_base_local = locals
                .state_remap_base_local
                .expect("state remap base local missing for stateful wasm");
            backend.emit_data_ptr(reloc_enabled, func_index, func, *remap_segment);
            func.instruction(&Instruction::LocalSet(remap_base_local));
        }
    }

    pub(super) fn static_edge(&self, source: usize, target: usize) -> StaticDispatchEdge {
        let source_block = self.block_for_op[source];
        if target == self.block_for_op.len() {
            return StaticDispatchEdge::ReturnNone;
        }
        let target_block = *self
            .block_for_op
            .get(target)
            .expect("static WASM successor lies outside the function");
        assert_eq!(
            self.block_starts[target_block], target,
            "static WASM successor must enter a block at its leader"
        );
        if target_block > source_block {
            StaticDispatchEdge::Forward {
                label_depth: (target_block - source_block - 1) as u32,
            }
        } else {
            StaticDispatchEdge::Redispatch { operation: target }
        }
    }
}
