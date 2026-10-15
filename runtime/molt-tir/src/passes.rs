//! SimpleIR/TIR pass facade.
//!
//! The pass families live in child modules so each authority surface has a
//! bounded owner while existing callers continue to route through `crate::passes`.
//! Loop-invariant motion belongs exclusively to the shared TIR pipeline, where
//! CFG dominance, SSA operands, and exception effects govern legal movement.

mod app_callable_manifest;
mod constant_fold;
mod dead_functions;
mod dead_imports;
mod dead_ops;
mod escape;
mod exception_check_elision;
mod guard_elision;
mod megafunction_split;
mod pipeline;
mod profile_order;
mod purity;
mod rc_coalescing;
mod runtime_exit;
mod runtime_roots;
mod stateful_loops;
mod struct_alloc_elision;
mod unbound_checks;

#[cfg(test)]
mod tests;

pub use self::app_callable_manifest::{
    AppCallableRequirements, collect_app_callable_requirements, compute_app_callable_manifest,
    compute_app_callable_manifest_checked,
};
pub use self::constant_fold::{fold_constants, fold_constants_cross_block};
pub use self::dead_functions::{
    eliminate_dead_functions, eliminate_dead_functions_with_roots, reachable_function_names,
};
pub use self::dead_imports::eliminate_dead_imports;
pub use self::dead_ops::eliminate_dead_ops;
pub use self::escape::escape_analysis;
pub use self::exception_check_elision::elide_safe_exception_checks;
pub use self::guard_elision::{
    RuntimeGuardFacts, SsaRuntimeGuardFacts, eliminate_redundant_guard_tags,
};
pub use self::megafunction_split::{
    split_large_function, split_megafunctions, split_megafunctions_with_filter,
};
pub use self::pipeline::{
    SIMPLE_IR_FUNCTION_PASSES, SimpleIrLowering, SimpleIrPass, run_simple_ir_pipeline,
};
pub use self::profile_order::apply_profile_order;
pub use self::purity::{
    SimpleIrScalarPurityFacts, simple_ir_op_is_provably_nonthrowing_with_facts,
};
pub use self::rc_coalescing::{
    build_const_int_map, build_last_use_map, compute_rc_coalesce_skips, rc_coalescing,
};
pub use self::runtime_exit::inject_runtime_exit;
pub use self::stateful_loops::rewrite_stateful_loops;
pub use self::struct_alloc_elision::elide_dead_struct_allocs;
pub use self::unbound_checks::eliminate_unbound_local_checks;

#[cfg(test)]
use self::megafunction_split::{
    is_drop_fact_marker_op, split_chunk_name, verify_split_function_def_use,
    verify_split_generated_ops,
};
#[cfg(test)]
use self::runtime_roots::is_protected_runtime_entrypoint;
