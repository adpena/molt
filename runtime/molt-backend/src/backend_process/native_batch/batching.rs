use super::*;

pub(crate) fn partition_functions_for_batches(
    functions: Vec<molt_backend::FunctionIR>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
) -> Vec<Vec<molt_backend::FunctionIR>> {
    let max_functions_per_batch = max_functions_per_batch.max(1);
    let max_ops_per_batch = max_ops_per_batch.max(1);

    let mut batches: Vec<Vec<molt_backend::FunctionIR>> = Vec::new();
    let mut current: Vec<molt_backend::FunctionIR> = Vec::new();
    let mut current_ops = 0usize;

    for func in functions {
        let func_ops = func.ops.len();
        let would_overflow_count = current.len() >= max_functions_per_batch;
        let would_overflow_ops =
            !current.is_empty() && current_ops.saturating_add(func_ops) > max_ops_per_batch;

        if would_overflow_count || would_overflow_ops {
            batches.push(std::mem::take(&mut current));
            current_ops = 0;
        }

        current_ops = current_ops.saturating_add(func_ops);
        current.push(func);
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

pub(crate) fn batch_external_function_names(
    all_function_names: &std::collections::BTreeSet<String>,
    batch_funcs: &[molt_backend::FunctionIR],
) -> std::collections::BTreeSet<String> {
    let batch_names: std::collections::BTreeSet<&str> =
        batch_funcs.iter().map(|func| func.name.as_str()).collect();
    all_function_names
        .iter()
        .filter(|name| !batch_names.contains(name.as_str()))
        .cloned()
        .collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe extern "C" {
        fn malloc_default_zone() -> *mut libc::c_void;
        fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
    }

    unsafe {
        let zone = malloc_default_zone();
        if !zone.is_null() {
            let _ = malloc_zone_pressure_relief(zone, usize::MAX);
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe {
        let _ = libc::malloc_trim(0);
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
pub(crate) fn release_native_backend_batch_memory_to_os() {}

pub(crate) fn resolved_batch_size_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

pub(crate) fn resolved_batch_op_budget_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

pub(crate) struct NativeApplicationObjectOptions<'a> {
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_split_enabled: bool,
    pub(crate) app_callable_manifest: Option<std::collections::BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock, design doc 69): its init
    /// symbols are dead-function-elimination roots and the main application
    /// object emits its blob (`molt_module_registry_blob`).
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeApplicationObjectResult {
    pub(crate) function_count: usize,
    pub(crate) batch_count: usize,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchModuleMetadata {
    pub(crate) module_context: molt_backend::NativeBackendModuleContext,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchObjectJob {
    pub(crate) ir: SimpleIR,
    pub(crate) module_context_path: PathBuf,
    pub(crate) target_triple: Option<String>,
    pub(crate) emit_app_callable_resolver: bool,
    pub(crate) app_callable_manifest: Option<std::collections::BTreeSet<String>>,
    pub(crate) external_function_names: std::collections::BTreeSet<String>,
    /// Carried by the batch that emits the app callable resolver (the main
    /// application object): that batch also emits the module registry blob.
    #[serde(default)]
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeBatchJobSpec {
    pub(crate) job_path: PathBuf,
    pub(crate) object_path: PathBuf,
}

pub(crate) fn deduplicate_functions_by_name(functions: &mut Vec<molt_backend::FunctionIR>) {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    functions.retain(|f| seen.insert(f.name.clone()));
}
