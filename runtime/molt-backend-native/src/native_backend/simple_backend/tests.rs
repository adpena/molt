use super::{
    DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT, DEFERRED_CODEGEN_FLUSH_OP_BUDGET,
    NativeBackendModuleContext, NativeRcAuthority, SimpleBackend, TrampolineKey,
    analyze_native_backend_ir, assert_requested_llvm_backend_available, compute_function_has_ret,
    drain_cleanup_entry_tracked, drain_cleanup_entry_tracked_with_authority,
    drain_cleanup_tracked_dedup_with_authority, merge_closure_functions, merge_function_arities,
    merge_function_has_ret, merge_leaf_functions, merge_task_kinds, preprocess_backend_tir_input,
    should_flush_deferred_codegen,
};
use crate::TrampolineKind;
use crate::ir::{FunctionIR, OpIR, SimpleIR};
use crate::passes::ReturnAliasSummary;
use cranelift_codegen::ir::Value;
use cranelift_codegen::ir::types;
use cranelift_module::Module;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

fn backend_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Acquire the process-env serialization lock, tolerating a poisoned mutex.
///
/// The guarded value is `()`  the lock exists only to *serialize* tests
/// that mutate process-global env vars (`MOLT_BACKEND`, `MOLT_STDLIB_OBJ`,
/// ) so they do not race. Each such test snapshots and restores the env
/// vars it touches itself; the mutex protects no shared in-memory invariant.
/// When one test panics while holding the guard, the mutex is *poisoned*,
/// but there is no corrupted state to guard against  the only thing the
/// poison flag would do is convert that single panic into a cascade of
/// `PoisonError` panics in every later test that takes the lock, hiding the
/// real failure behind noise. Recovering the guard via `into_inner()` keeps
/// the mutual-exclusion guarantee intact while letting the genuine failure
/// stand alone. This is the textbook-sound use of poison recovery: the
/// protected data carries no invariant the poison could have broken.
fn acquire_backend_env_lock() -> std::sync::MutexGuard<'static, ()> {
    backend_env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn compile_trace_probe_object(emit_traces_env: Option<&str>) -> Vec<u8> {
    let _guard = acquire_backend_env_lock();
    match emit_traces_env {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_EMIT_TRACES", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") },
    }
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "trace_enter_slot".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "trace_exit".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let output = SimpleBackend::new().compile(ir);
    unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") };
    output.bytes
}

fn compile_function_to_clif_text(functions: Vec<FunctionIR>, target_name: &str) -> String {
    let ir = SimpleIR {
        functions,
        profile: None,
    };
    let analysis = analyze_native_backend_ir(&ir, true);
    let function_has_ret = compute_function_has_ret(&ir.functions);
    let function_arities = ir
        .functions
        .iter()
        .map(|func| (func.name.clone(), func.params.len()))
        .collect();
    let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);
    let target_func = ir
        .functions
        .into_iter()
        .find(|func| func.name == target_name)
        .unwrap_or_else(|| panic!("missing target function `{target_name}`"));
    let mut backend = SimpleBackend::new();
    backend.compile_func(
        target_func,
        &analysis.task_kinds,
        &analysis.task_closure_sizes,
        &analysis.defined_functions,
        &analysis.defined_functions,
        &analysis.closure_functions,
        &return_alias_summaries,
        false,
        &analysis.leaf_functions,
        &function_arities,
        &function_has_ret,
    );
    backend
        .deferred_defines
        .iter()
        .find(|deferred| deferred.name == target_name)
        .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
        .func
        .display()
        .to_string()
}

/// Regression  native codegen must compile the CANONICAL bare `get_attr`.
///
/// `tir::lower_to_simple` emits the canonical `get_attr` for any `LoadAttr`
/// that carries no specialized `_original_kind` (its documented default,
/// exactly like `set_attr`/`del_attr`/`index`/`call`). A TIR pass that
/// yields a generic by-name attribute load reaches native codegen with that
/// bare spelling  observed for `__future__._Feature.__repr__` under
/// `--build-profile release`, where the guard-splitting passes leave a
/// generic `get_attr` cold-fallback after specializing the
/// `self.optional`/`.mandatory`/`.compiler_flag` `guarded_field_get`s. The
/// attribute handler (`fc::attrs`) claimed every specialized `get_attr_*`
/// alias but NOT the canonical `get_attr`, so the op hit the dispatch's loud
/// no-codegen catch-all and panicked ("no codegen for result-producing op
/// kind `get_attr`"). This compiles a function whose body is a bare
/// `get_attr`; without the fix it panics in `compile_func`, with it the op
/// routes to the generic-by-name attribute load.

fn roundtrip_function_through_tir(func: &FunctionIR) -> FunctionIR {
    let mut functions = vec![func.clone()];
    let cache_dir = test_tir_pipeline_cache_dir();
    let run = crate::tir::pipeline_cache::run_cached_tir_pipeline(
        &mut functions,
        crate::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: crate::tir::target_info::TargetInfo::native_release_fast(),
            cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Native,
            cache_dir: Some(cache_dir.clone()),
            process_externs: false,
            verify_lir: true,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan_from_limits(
                1, None,
            ),
        },
        preprocess_backend_tir_input,
    );
    assert!(
        run.cached_tir.contains_function(&func.name),
        "shared TIR runner must return optimized TIR custody for '{}'",
        func.name
    );
    let _ = std::fs::remove_dir_all(cache_dir);
    let roundtripped = functions.pop().expect("roundtrip function missing");
    assert!(
        crate::tir::lower_to_simple::validate_labels(&roundtripped.ops),
        "TIR runner roundtrip must preserve all referenced labels: {:#?}",
        roundtripped.ops
    );
    roundtripped
}

fn test_tir_pipeline_cache_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "molt-tir-pipeline-cache-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

mod cleanup;
mod codegen_regressions;
mod compile_pipeline;
mod deferred_codegen;
mod fail_closed_codegen;
mod llvm_backend;
mod module_metadata;
mod tir_analysis;
mod trampolines;
