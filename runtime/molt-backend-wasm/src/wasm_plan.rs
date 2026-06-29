mod multi_return;
mod op_classifiers;
mod stage_audit;

pub(crate) use multi_return::detect_multi_return_candidates;
pub(crate) use op_classifiers::{
    is_shared_drop_fact_marker, wasm_scalar_integer_fast_path_for_op,
    wasm_scalar_truthiness_fast_path_for_name, wasm_specialized_container_import,
};
pub(crate) use stage_audit::{
    emit_wasm_stage_audit, simple_ir_stage_shape, tir_module_stage_shape,
};
