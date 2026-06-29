#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers

// Native and LLVM codegen authority lives here so Cranelift/LLVM edits do not
// rebuild the backend composition crate.
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
#[cfg(feature = "native-backend")]
mod native_backend;
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod runtime_import_abi;
pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
#[cfg(feature = "native-backend")]
pub use crate::native_backend::{CompileOutput, NativeBackendModuleContext, SimpleBackend};
#[cfg(feature = "native-backend")]
pub(crate) use crate::native_backend::{
    DeferredDefine, VarValue, block_has_terminator, extend_unique_tracked,
    switch_to_block_tracking, unbox_int,
};
pub use crate::passes::{
    apply_profile_order, build_const_int_map, canonicalize_direct_raise_edges,
    compute_intrinsic_manifest, compute_intrinsic_manifest_checked, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, eliminate_dead_imports,
    eliminate_dead_ops, eliminate_redundant_guard_tags, eliminate_unbound_local_checks,
    escape_analysis, fold_constants, fold_constants_cross_block, fuse_method_dispatch,
    hoist_loop_invariants, inject_runtime_exit, rc_coalescing, rewrite_stateful_loops,
    split_megafunctions,
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
#[allow(unused_imports)]
pub(crate) use molt_codegen_abi::{
    CANONICAL_NAN_BITS, FUNC_DEFAULT_DICT_POP, FUNC_DEFAULT_DICT_UPDATE, FUNC_DEFAULT_NONE,
    GENERATOR_CONTROL_BYTES, HEADER_COLD_IDX_OFFSET, HEADER_FLAG_CONTAINS_REFS,
    HEADER_FLAG_HAS_PTRS, HEADER_FLAG_IMMORTAL, HEADER_FLAG_SKIP_CLASS_DECREF, HEADER_FLAGS_OFFSET,
    HEADER_REFCOUNT_OFFSET, HEADER_SIZE_BYTES, HEADER_TYPE_ID_OFFSET, INLINE_INT_BIAS,
    INLINE_INT_LIMIT, INT_MASK, INT_MAX_INLINE, INT_MIN_INLINE, INT_SHIFT, INT_SIGN_BIT, INT_WIDTH,
    JIT_TYPE_ID_LIST_BOOL, LIST_INT_STORAGE_DATA_OFFSET, LIST_INT_STORAGE_LEN_OFFSET, NanBoxConsts,
    POINTER_MASK, QNAN, QNAN_TAG_BOOL_I64, QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64, QNAN_TAG_NONE_I64,
    QNAN_TAG_PENDING_I64, QNAN_TAG_PTR_I64, TAG_BOOL, TAG_INT, TAG_MASK, TAG_NONE, TAG_PENDING,
    TAG_PTR, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, TYPE_ID_FUNCTION,
    TYPE_ID_OBJECT, pending_bits, stable_ic_site_id,
};
/// The representation lattice element (the orthogonal carrier axis to
/// `TirType`). Re-exported publicly because it appears in the signature of the
/// `pub` `tir::lower_to_lir::lower_function_to_lir`, which backend codegen paths
/// drive with the proven `repr_by_value`.
pub use molt_ir::repr::Repr;
pub use molt_ir::stdlib_module_symbols::{
    STDLIB_MODULE_SYMBOLS_ENV, parse_stdlib_module_symbols, stdlib_module_symbols_from_env,
};

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}
