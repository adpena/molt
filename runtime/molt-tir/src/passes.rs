//! SimpleIR/TIR pass facade.
//!
//! The pass families live in child modules so each authority surface has a
//! bounded owner while existing callers continue to route through `crate::passes`.

mod alias_returns;
mod constant_fold;
mod dead_functions;
mod dead_imports;
mod dead_ops;
mod def_use;
mod escape;
mod exception_check_elision;
mod exception_edges;
mod guard_elision;
mod intrinsics_manifest;
mod loop_hoist;
mod megafunction_split;
mod method_fusion;
mod profile_order;
mod purity;
mod rc_coalescing;
mod runtime_exit;
mod runtime_roots;
mod split_field_deforestation;
mod stateful_loops;
mod struct_alloc_elision;
mod unbound_checks;

#[cfg(test)]
mod tests;

pub use self::alias_returns::{ReturnAliasSummary, compute_return_alias_summaries};
pub use self::constant_fold::{fold_constants, fold_constants_cross_block};
pub use self::dead_functions::eliminate_dead_functions;
pub use self::dead_imports::eliminate_dead_imports;
pub use self::dead_ops::eliminate_dead_ops;
pub use self::escape::escape_analysis;
pub use self::exception_check_elision::elide_safe_exception_checks;
pub use self::exception_edges::canonicalize_direct_raise_edges;
pub use self::guard_elision::eliminate_redundant_guard_tags;
pub use self::intrinsics_manifest::{
    compute_intrinsic_manifest, compute_intrinsic_manifest_checked,
};
pub use self::loop_hoist::hoist_loop_invariants;
pub use self::megafunction_split::{
    split_large_function, split_megafunctions, split_megafunctions_with_filter,
};
pub use self::method_fusion::fuse_method_dispatch;
pub use self::profile_order::apply_profile_order;
pub use self::purity::{
    SimpleIrScalarPurityFacts, simple_ir_op_is_provably_nonthrowing_with_facts,
};
pub use self::rc_coalescing::{build_const_int_map, compute_rc_coalesce_skips, rc_coalescing};
pub use self::runtime_exit::inject_runtime_exit;
pub use self::split_field_deforestation::deforest_split_field_reads;
pub use self::stateful_loops::rewrite_stateful_loops;
pub use self::struct_alloc_elision::elide_dead_struct_allocs;
pub use self::unbound_checks::eliminate_unbound_local_checks;

#[cfg(test)]
use self::megafunction_split::{
    is_drop_fact_marker_op, verify_split_function_def_use, verify_split_generated_ops,
};
#[cfg(test)]
use self::method_fusion::fuse_method_dispatch_inner;
#[cfg(test)]
use self::runtime_roots::is_protected_runtime_entrypoint;
