#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)]
// complex return types in TIR CFG helpers

// Tests save, set and restore process settings around fixtures; production
// code reads them through `molt_ir::backend_environment` (clippy.toml).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

#[cfg(all(
    feature = "native-backend",
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "s390x"
    ))
))]
compile_error!("Molt's Cranelift native backend supports x86_64, aarch64, riscv64, and s390x");

// Retain the dependency's native link metadata for binaries and local tests.
#[cfg(feature = "llvm")]
extern crate molt_llvm_link as _;

// Native and LLVM codegen authority lives here so Cranelift/LLVM edits do not
// rebuild the backend composition crate.
pub use molt_ir::{
    MOLT_CLOSURE_PARAM_NAME, backend_environment, debug_artifacts, ir, ir_schema, json_boundary,
    process_diagnostics, repr, runtime_callable_symbols, stdlib_modules,
};
pub use molt_tir::trampolines::externalize_function_with_signature;

/// The task layout of the IR constructor kind
/// (`molt_codegen_abi::task_site`).
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) const fn task_layout(
    kind: molt_ir::task_kind::TaskKind,
) -> molt_codegen_abi::task_site::TaskConstructorLayout {
    use molt_codegen_abi::task_site::TaskConstructorLayout;
    use molt_ir::task_kind::TaskKind;
    match kind {
        TaskKind::Generator => TaskConstructorLayout::generator(),
        TaskKind::Coroutine => TaskConstructorLayout::coroutine(),
        TaskKind::AsyncGenerator => TaskConstructorLayout::async_generator(),
        TaskKind::Future => TaskConstructorLayout::future(),
    }
}
pub use molt_tir::{passes, representation_plan, tir};

pub use molt_ir::runtime_callable_symbols::RuntimeCallableSymbols;
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) const GENERATED_OBJECT_ABI_ANCHOR_SYMBOL: &str = "__molt_generated_object_abi_anchor";

// Compiled `builtin_func` embeds a runtime callable's key from its molt-ir
// `RuntimeFn` row; the runtime compares it with `molt-codegen-abi`'s catalog
// key. Both come from one generator run; this proves they agree (HF-881).
const _: () = {
    let rows = molt_ir::runtime_fn_generated::RUNTIME_FN_ABIS;
    let mut index = 0;
    while index < rows.len() {
        if let Some(callable) = rows[index].callable {
            assert!(
                callable.key == molt_codegen_abi::runtime_callable_key(rows[index].symbol),
                "a RuntimeFn callable key disagrees with the runtime callable catalog",
            );
        }
        index += 1;
    }
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod app_resolver_abi;
#[cfg(feature = "native-backend")]
mod application_ir;
#[cfg(feature = "native-backend")]
pub use application_ir::prepare_native_application_ir;
// Both native instruction selectors consume terminal TIR through this walker.
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod codegen_ssa;
mod codegen_target;
pub use codegen_target::CodegenTarget;
#[cfg(feature = "llvm")]
pub mod llvm_backend;
#[cfg(feature = "native-backend")]
mod native_backend;
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) mod runtime_import_abi;
pub use crate::ir::{
    BackendIrDocument, FunctionIR, ModuleRegistryIR, OpIR, PgoProfileIR, SimpleIR,
    validate_simple_ir,
};
#[cfg(feature = "native-backend")]
pub use crate::native_backend::{
    CompileOutput, NativeBackendModuleContext, NativeProgramScope, NativeTerminalArchive,
    NativeTerminalObject, PreparedJitApplication, SimpleBackend, TerminalObjectRow,
};
#[cfg(feature = "jit")]
pub use crate::native_backend::{
    JIT_FAILURE_STATUS, JitCompileError, JitEntryPoints, JitProgram, JitSymbolLookup,
    probe_jit_execution,
};
pub use crate::passes::{
    apply_profile_order, build_const_int_map, compute_app_callable_manifest,
    compute_app_callable_manifest_checked, eliminate_dead_functions,
    eliminate_dead_functions_with_roots, split_megafunctions,
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
#[allow(unused_imports)]
pub(crate) use molt_codegen_abi::{
    CLASS_POLICY_INSTANCE_KIND_SHIFT, CLASS_POLICY_WORD_OFFSET, FUNC_DEFAULT_DICT_POP,
    FUNC_DEFAULT_DICT_UPDATE, FUNC_DEFAULT_NONE, GENERATOR_CONTROL_BYTES,
    HEADER_FLAG_CONTAINS_REFS, HEADER_FLAG_HAS_PTRS, HEADER_FLAGS_OFFSET, HEADER_SIZE_BYTES,
    HEADER_TYPE_ID_OFFSET, INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_MAX_INLINE,
    INT_MIN_INLINE, INT_SHIFT, INT_SIGN_BIT, INT_WIDTH, JIT_TYPE_ID_LIST_BOOL,
    LIST_STORAGE_DATA_OFFSET, LIST_STORAGE_LEN_OFFSET, NanBoxConsts, QNAN, QNAN_TAG_BOOL_I64,
    QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64, QNAN_TAG_NONE_I64, QNAN_TAG_PENDING_I64, QNAN_TAG_PTR_I64,
    TAG_BOOL, TAG_INT, TAG_MASK, TAG_NONE, TAG_PENDING, TAG_PTR, TASK_KIND_COROUTINE,
    TASK_KIND_FUTURE, TASK_KIND_GENERATOR, TYPE_ID_FUNCTION, TYPE_ID_OBJECT, TYPE_ID_TYPE,
    pending_bits, stable_ic_site_id,
};

pub use molt_ir::stdlib_modules::{parse_stdlib_modules, stdlib_modules_from_env};

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn env_setting(key: backend_environment::BackendEnv) -> Option<String> {
    backend_environment::var(key)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

/// Standalone artifacts use the target baseline unless host specialization is
/// explicitly requested. Both native backends consume this policy.
#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn native_codegen_portable() -> bool {
    let setting = env_setting(backend_environment::BackendEnv::MOLT_PORTABLE)
        .map(|value| value.to_ascii_lowercase());
    match setting.as_deref() {
        None | Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        Some(value) => panic!("invalid MOLT_PORTABLE={value:?}; expected 0/1 or false/true"),
    }
}

#[cfg(test)]
mod test_support;
