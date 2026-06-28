mod constant_cache;
mod fast_lane;
mod float;
mod int_box;
mod truthiness;

pub(crate) use constant_cache::ConstantCache;
pub(crate) use fast_lane::{
    IntFastLane, emit_trusted_int_fast_path_guard_close, emit_trusted_int_fast_path_guard_open,
};
pub(crate) use float::{emit_f64_to_i64_canonical, push_f64_to_i64_canonical};
pub(crate) use int_box::{
    emit_box_bool_from_i32, emit_box_int_from_local_opt, emit_inline_int_range_check,
    emit_unbox_int_local_trusted_opt, emit_unbox_int_local_trusted_tee_opt,
};
pub(crate) use molt_codegen_abi::{
    INT_MASK, POINTER_MASK, box_bool_bits as box_bool, box_int_bits as box_int,
    box_none_bits as box_none, box_pending_bits as box_pending, stable_ic_site_id,
};
pub(crate) use truthiness::emit_branch_truthiness_i32;
