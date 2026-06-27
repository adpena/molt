use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::*;
use block_layout::{build_dispatch_block_map, build_dispatch_blocks};
use state_remap::{
    build_dense_state_remap_table, build_sparse_state_remap_entries, build_state_resume_maps,
    emit_sparse_state_remap_lookup,
};

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
