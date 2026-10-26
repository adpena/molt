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

pub(super) use emit::emit_non_linear_control;
pub(super) use plan::NonLinearControlPlan;
