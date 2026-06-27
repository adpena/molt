use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::*;
use crate::wasm_dispatch::{
    DispatchControlMaps, build_dispatch_block_map, build_dispatch_blocks,
    build_dispatch_control_maps, dispatch_control_panic,
};
use state_remap::{
    build_dense_state_remap_table, build_sparse_state_remap_entries, build_state_resume_maps,
    emit_sparse_state_remap_lookup,
};

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
