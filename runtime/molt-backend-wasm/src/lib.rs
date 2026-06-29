#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers

pub use molt_ir::intrinsic_symbols::{
    runtime_intrinsic_symbols_from_env, runtime_intrinsic_symbols_required,
};
pub use molt_ir::repr::Repr;
pub use molt_ir::stdlib_module_symbols::{
    STDLIB_MODULE_SYMBOLS_ENV, parse_stdlib_module_symbols, stdlib_module_symbols_from_env,
};
pub use molt_ir::{
    FunctionIR, MOLT_CLOSURE_PARAM_NAME, OpIR, PgoProfileIR, SimpleIR, debug_artifacts,
    intrinsic_symbols, ir, ir_schema, json_boundary, native_callable_abi, process_diagnostics,
    repr, stdlib_module_symbols, validate_simple_ir,
};
pub use molt_tir::passes::{
    apply_profile_order, build_const_int_map, canonicalize_direct_raise_edges,
    compute_intrinsic_manifest, compute_intrinsic_manifest_checked, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, eliminate_dead_imports,
    eliminate_dead_ops, eliminate_redundant_guard_tags, eliminate_unbound_local_checks,
    escape_analysis, fold_constants, fold_constants_cross_block, fuse_method_dispatch,
    hoist_loop_invariants, inject_runtime_exit, rc_coalescing, rewrite_stateful_loops,
    split_megafunctions,
};
pub use molt_tir::simpleir_debug::{DumpIrConfig, dump_ir_matches, dump_ir_ops, should_dump_ir};
pub use molt_tir::trampolines::{
    TrampolineKind, TrampolineSpec, externalize_function_with_signature,
    function_requires_value_return,
};
pub use molt_tir::{passes, representation_plan, tir};

#[cfg(all(feature = "wasm-backend", feature = "test-util"))]
pub mod test_util;
#[cfg(feature = "wasm-backend")]
mod wasm;
#[cfg(feature = "wasm-backend")]
pub use wasm::{WasmBackend, WasmCompileOptions, WasmProfile};
#[cfg(feature = "wasm-backend")]
mod wasm_abi;
#[cfg(feature = "wasm-backend")]
mod wasm_abi_generated;
#[cfg(feature = "wasm-backend")]
mod wasm_binary;
#[cfg(feature = "wasm-backend")]
mod wasm_data;
#[cfg(feature = "wasm-backend")]
mod wasm_import_tracking;
#[cfg(feature = "wasm-backend")]
mod wasm_options;
#[cfg(feature = "wasm-backend")]
mod wasm_plan;
#[cfg(feature = "wasm-backend")]
mod wasm_values;

#[cfg(feature = "wasm-backend")]
pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}
