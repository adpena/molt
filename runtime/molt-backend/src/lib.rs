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
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) use molt_tir::simpleir_debug::{dump_ir_matches, dump_ir_ops, should_dump_ir};
pub use molt_tir::trampolines::externalize_function_with_signature;
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) use molt_tir::trampolines::{
    TrampolineKind, TrampolineSpec, function_requires_value_return,
};
pub use molt_tir::{passes, representation_plan, tir};

pub use molt_ir::intrinsic_symbols::{
    runtime_intrinsic_symbols_from_env, runtime_intrinsic_symbols_required,
};
#[cfg(feature = "llvm")]
pub mod llvm_backend;
pub mod luau_ir;
pub mod luau_lower;
#[cfg(feature = "native-backend")]
mod native_backend;
#[cfg(feature = "native-backend")]
pub use crate::native_backend::{CompileOutput, NativeBackendModuleContext, SimpleBackend};
#[cfg(feature = "native-backend")]
pub(crate) use crate::native_backend::{
    DeferredDefine, NanBoxConsts, VarValue, block_has_terminator, extend_unique_tracked,
    switch_to_block_tracking, unbox_int,
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
mod native_backend_consts;
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
/// The representation lattice element (the orthogonal carrier axis to
/// `TirType`). Re-exported publicly because it appears in the signature of the
/// `pub` `tir::lower_to_lir::lower_function_to_lir` (Phase 1 of the typed-IR
/// convergence), which the WASM/LIR codegen path drives with the proven
/// `repr_by_value`.
pub use molt_ir::repr::Repr;
pub use molt_ir::stdlib_module_symbols::{
    STDLIB_MODULE_SYMBOLS_ENV, parse_stdlib_module_symbols, stdlib_module_symbols_from_env,
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
use native_backend_consts::*;

#[cfg(feature = "luau-backend")]
pub mod luau;
#[cfg(feature = "rust-backend")]
pub mod rust;
#[cfg(feature = "wasm-backend")]
pub use molt_backend_wasm::wasm;

#[cfg(feature = "egraphs")]
pub mod egraph_simplify;

#[cfg(any(feature = "native-backend", feature = "llvm"))]
fn pending_bits() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for b in func_name
        .as_bytes()
        .iter()
        .chain(lane.as_bytes().iter())
        .copied()
    {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= op_idx as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    // Keep the id within inline-int payload range and avoid zero.
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}
