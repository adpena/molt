#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers

// Immutable IR/data lives in molt-ir; pass/fact orchestration and SimpleIR<->TIR
// round-tripping live in molt-passes behind molt-tir re-exports; backend
// projection and representation planning live in molt-tir. Keep backend-local
// `crate::*` routes explicit so moved facts are not silently borrowed through a
// legacy crate boundary.
pub use molt_ir::{
    MOLT_CLOSURE_PARAM_NAME, debug_artifacts, intrinsic_symbols, ir, ir_schema, json_boundary,
    process_diagnostics, repr, stdlib_module_symbols,
};
pub use molt_tir::trampolines::externalize_function_with_signature;
pub use molt_tir::{passes, representation_plan, tir};

pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
pub use crate::passes::{
    apply_profile_order, build_const_int_map, canonicalize_direct_raise_edges,
    compute_intrinsic_manifest, compute_intrinsic_manifest_checked, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, eliminate_dead_imports,
    eliminate_dead_ops, eliminate_redundant_guard_tags, eliminate_unbound_local_checks,
    escape_analysis, fold_constants, fold_constants_cross_block, fuse_method_dispatch,
    hoist_loop_invariants, inject_runtime_exit, rc_coalescing, rewrite_stateful_loops,
    split_megafunctions,
};
#[cfg(feature = "llvm")]
pub use molt_backend_native::llvm_backend;
#[cfg(feature = "native-backend")]
pub use molt_backend_native::{CompileOutput, NativeBackendModuleContext, SimpleBackend};
pub use molt_ir::intrinsic_symbols::{
    runtime_intrinsic_symbols_from_env, runtime_intrinsic_symbols_required,
};
/// The representation lattice element (the orthogonal carrier axis to
/// `TirType`). Re-exported publicly because it appears in the signature of the
/// `pub` `tir::lower_to_lir::lower_function_to_lir` (Phase 1 of the typed-IR
/// convergence), which the WASM/LIR codegen path drives with the proven
/// `repr_by_value`.
pub use molt_ir::repr::Repr;
pub use molt_ir::stdlib_module_symbols::{
    STDLIB_MODULE_SYMBOLS_ENV, parse_stdlib_module_symbols, stdlib_module_symbols_from_env,
};

#[cfg(feature = "luau-backend")]
pub use molt_backend_luau::luau;
#[cfg(feature = "rust-backend")]
pub use molt_backend_rust::rust;
#[cfg(feature = "wasm-backend")]
pub use molt_backend_wasm::wasm;

#[cfg(feature = "egraphs")]
pub mod egraph_simplify;
