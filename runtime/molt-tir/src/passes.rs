//! SimpleIR/TIR pass facade.
//!
//! The pass families live in child modules so each authority surface has a
//! bounded owner while existing callers continue to route through `crate::passes`.
//! Loop-invariant motion belongs exclusively to the shared TIR pipeline, where
//! CFG dominance, SSA operands, and exception effects govern legal movement.

mod app_callable_manifest;
mod dead_functions;
mod guard_elision;
mod megafunction_split;
mod profile_order;
mod rc_coalescing;
mod runtime_exit;
mod runtime_roots;

#[cfg(test)]
mod tests;

pub use self::app_callable_manifest::{
    AppCallableKind, AppCallableManifest, AppCallableRequirements,
    collect_app_callable_requirements, compute_app_callable_manifest,
    compute_app_callable_manifest_checked,
};
pub use self::dead_functions::{
    eliminate_dead_functions, eliminate_dead_functions_with_roots, reachable_functions,
    visit_function_reference_targets,
};
pub use self::guard_elision::{RuntimeGuardFacts, SsaRuntimeGuardFacts};
pub use self::megafunction_split::{
    split_large_function, split_megafunctions, split_megafunctions_with_filter,
};
pub use self::profile_order::apply_profile_order;
pub use self::rc_coalescing::{build_const_int_map, compute_rc_coalesce_skips};
pub use self::runtime_exit::inject_runtime_exit;

#[cfg(test)]
use self::megafunction_split::{
    is_drop_fact_marker_op, split_chunk_name, verify_split_function_def_use,
};
#[cfg(test)]
use self::runtime_roots::is_protected_runtime_entrypoint;
