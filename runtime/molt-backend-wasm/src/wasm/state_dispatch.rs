use super::control_flow::ControlKind;
use super::op_loop::WasmFunctionEmitContext;
use super::{WasmBackend, WasmFrameLocals};
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::{INT_MASK, POINTER_MASK, box_pending, emit_branch_truthiness_i32};
use crate::{FunctionIR, OpIR};
use block_layout::{build_dispatch_block_map, build_dispatch_blocks};
use state_remap::{
    build_dense_state_remap_table, build_sparse_state_remap_entries, build_state_resume_maps,
    emit_sparse_state_remap_lookup,
};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, Function, Instruction};

mod block_layout;
mod common;
mod emit;
mod plan;
mod state_remap;
mod stateful_ops;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchMode {
    Stateful,
    Jumpful,
}

pub(super) use common::exception_handler_region_indices;
pub(super) use emit::{emit_jumpful_dispatch, emit_stateful_dispatch};
pub(super) use plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
