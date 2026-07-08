mod branches;
mod regions;
mod resume;
mod returns;
mod state_values;

pub(super) use branches::{
    emit_conditional_state_branch, emit_dispatch_check_exception, emit_dispatch_if,
    emit_dispatch_loop_break_cond, emit_set_state_and_br, label_target, loop_break_target,
    require_stateful,
};
pub(in crate::wasm) use regions::exception_handler_region_indices;
pub(super) use regions::exception_handler_region_indices_from_label_map;
pub(super) use resume::emit_stateful_resume_prelude;
pub(super) use returns::{emit_arena_free, emit_dispatch_trailing_return};
pub(super) use state_values::{emit_obj_set_state_arg, emit_pending_state_value};
