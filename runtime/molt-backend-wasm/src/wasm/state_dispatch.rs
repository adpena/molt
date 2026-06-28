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
