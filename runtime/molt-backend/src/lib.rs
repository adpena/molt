#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)]
// complex return types in TIR CFG helpers

// Tests save, set and restore process settings around fixtures; production
// code reads them through `molt_ir::backend_environment` (clippy.toml).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

// Immutable IR/data lives in molt-ir; pass/fact orchestration and SimpleIR<->TIR
// round-tripping live in molt-passes behind molt-tir re-exports; backend
// projection and representation planning live in molt-tir. Keep backend-local
// `crate::*` routes explicit so moved facts are not silently borrowed through a
// legacy crate boundary.
pub use molt_ir::OpKind;
pub use molt_ir::{
    MOLT_CLOSURE_PARAM_NAME, backend_environment, debug_artifacts, ir, ir_schema, json_boundary,
    process_diagnostics, repr, runtime_callable_symbols, stdlib_modules,
};
pub use molt_tir::trampolines::externalize_function_with_signature;
pub use molt_tir::{passes, representation_plan, tir};

pub use crate::ir::{
    BackendIrDocument, FunctionIR, ModuleRegistryIR, OpIR, PgoProfileIR, SimpleIR,
    validate_simple_ir,
};
pub use crate::passes::{
    compute_app_callable_manifest_checked, eliminate_dead_functions,
    eliminate_dead_functions_with_roots,
};
#[cfg(feature = "llvm")]
pub use molt_backend_native::llvm_backend;
#[cfg(feature = "native-backend")]
pub use molt_backend_native::{
    CodegenTarget, CompileOutput, NativeBackendModuleContext, NativeTerminalArchive,
    NativeTerminalObject, PreparedJitApplication, SimpleBackend, TerminalObjectRow,
    prepare_native_application_ir,
};
pub use molt_ir::runtime_callable_symbols::RuntimeCallableSymbols;
pub use molt_ir::stdlib_modules::{parse_stdlib_modules, stdlib_modules_from_env};
