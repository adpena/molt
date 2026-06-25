// Windows bin-test builds compile Unix daemon protocol code for parser coverage
// without running the daemon loop; production warning policy remains unchanged.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "native-backend")]
use molt_backend::SimpleBackend;
#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::wasm::{WasmBackend, WasmCompileOptions};
use molt_backend::{SimpleIR, rewrite_annotate_stubs};
#[cfg(any(unix, test))]
use serde_json::Value as JsonValue;
#[cfg(feature = "native-backend")]
use sha2::{Digest, Sha256};
#[cfg(any(unix, test))]
use std::cmp::Reverse;
#[cfg(any(unix, test))]
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
#[cfg(unix)]
use std::io::BufRead;
use std::io::Write;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(all(feature = "native-backend", windows))]
use std::os::windows::io::AsRawHandle;
use std::path::Path;
#[cfg(feature = "native-backend")]
use std::path::PathBuf;
#[cfg(any(unix, test))]
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFileEx};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::System::IO::OVERLAPPED;

mod fact_graph_emit;
use fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};

#[cfg(any(unix, test))]
use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};

#[cfg(any(unix, test))]
const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
const DEFAULT_BACKEND_BATCH_SIZE: usize = 64;
const DEFAULT_STDLIB_BATCH_SIZE: usize = 128;
const DEFAULT_BACKEND_BATCH_OP_BUDGET: usize = 8_000;
const MIB: usize = 1024 * 1024;
const DEFAULT_DAEMON_REQUEST_LIMIT_BYTES: usize = 512 * MIB;
const DEFAULT_STDIN_REQUEST_LIMIT_BYTES: usize = DEFAULT_DAEMON_REQUEST_LIMIT_BYTES;
#[cfg(any(unix, test))]
const DEFAULT_DAEMON_MAX_JOBS: usize = 512;
#[cfg(any(unix, test))]
const DAEMON_REQUEST_ENV_KEYS: &[&str] = &[
    "MOLT_DISABLE_DEAD_FUNC_ELIM",
    "MOLT_BACKEND_BATCH_SIZE",
    "MOLT_BACKEND_BATCH_OP_BUDGET",
    "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEM_AVAILABLE_GB",
    "MOLT_MEMORY_AVAILABLE_GB",
    "MOLT_MEM_AVAILABLE_GB",
    "MOLT_BACKEND_MAX_RSS_GB",
    "MOLT_BACKEND_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEM_RESERVE_GB",
    "MOLT_MEMORY_RESERVE_GB",
    "MOLT_MEM_RESERVE_GB",
    "MOLT_MAX_FUNCTION_OPS",
    "MOLT_DISABLE_RC_COALESCING",
    "RAYON_NUM_THREADS",
    "TIR_DUMP",
    "TIR_OPT_STATS",
    "MOLT_TIR_TRACE_FUNC",
    "MOLT_DUMP_CLIF",
    "MOLT_DUMP_CLIF_ON_ERROR",
    "MOLT_DUMP_CLIF_ON_CFG_ERROR",
    "MOLT_DUMP_CLIF_FUNC",
    "MOLT_DUMP_CLIF_FILE",
    "MOLT_DUMP_CLIF_FILE_FILTER",
    "MOLT_DUMP_FINAL_FUNC_IR",
    "MOLT_DUMP_IR",
    // Optimization-pass instruments. Every optimization
    // lands WITH a firing/refusal instrument (the L4/needs_inlining lesson);
    // those instruments are useless if the daemon strips their env keys.
    // Debug-artifact routing: without these the daemon writes artifacts
    // (TIR dumps, llvm/before_opt.ll, pass refusal reports) under its own
    // CWD where nobody finds them.
    "MOLT_DEBUG_ARTIFACT_DIR",
    "MOLT_EXT_ROOT",
    "MOLT_OVERFLOW_PEEL_STATS",
    "MOLT_PROMOTE_DEBUG",
    "MOLT_INLINE_STATS",
    "MOLT_VERIFY_ANALYSIS",
    "MOLT_DEBUG_BIND",
    "MOLT_BACKEND",
    "MOLT_DEBUG_CHECK_EXC",
    "MOLT_DEBUG_CHECK_EXCEPTION",
    "MOLT_LLVM_DUMP_IR",
    "MOLT_BACKEND_TIMING",
    "MOLT_ENTRY_MODULE",
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_CACHE_MANIFEST",
    "MOLT_STDLIB_MODULE_SYMBOLS",
    "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
    "MOLT_DEBUG_DROP",
    "MOLT_DEBUG_LOWER_FUNC",
    "MOLT_TIR_DUMP",
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BackendOutputKind {
    Luau,
    Rust,
    Wasm,
    Native,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct DaemonJobRequest {
    id: String,
    is_wasm: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    target_triple: Option<String>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_link: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    wasm_split_runtime_runtime_table_min: Option<u32>,
    output: String,
    cache_key: String,
    function_cache_key: Option<String>,
    skip_module_output_if_synced: bool,
    skip_function_output_if_synced: bool,
    probe_cache_only: bool,
    ir: Option<SimpleIR>,
    ir_path: Option<String>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
struct DaemonRequest {
    version: Option<u32>,
    ping: Option<bool>,
    include_health: Option<bool>,
    config_digest: Option<String>,
    jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
struct DaemonJobResponse {
    id: String,
    ok: bool,
    cached: bool,
    cache_tier: Option<String>,
    output_written: bool,
    needs_ir: bool,
    message: Option<String>,
    /// Function names that were replaced with trap stubs due to Cranelift
    /// compilation failures.  Propagated to the CLI for build warnings.
    warnings: Vec<String>,
}

#[cfg(any(unix, test))]
fn is_false(value: &bool) -> bool {
    !*value
}

fn ensure_output_parent_dir(output_file: &str) -> io::Result<()> {
    let path = Path::new(output_file);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg_attr(
    not(any(
        feature = "luau-backend",
        feature = "rust-backend",
        feature = "wasm-backend"
    )),
    allow(dead_code)
)]
fn create_backend_output_file(output_file: &str) -> io::Result<File> {
    ensure_output_parent_dir(output_file)?;
    match File::create(output_file) {
        Ok(file) => Ok(file),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // Shared cache/build roots may be pruned between early setup and
            // final artifact emission. Recreate the parent at the point of
            // use and retry once so output emission is authoritative.
            ensure_output_parent_dir(output_file)?;
            File::create(output_file)
        }
        Err(err) => Err(err),
    }
}

fn default_backend_output_path(kind: BackendOutputKind) -> &'static str {
    match kind {
        BackendOutputKind::Luau => "dist/output.luau",
        BackendOutputKind::Rust => "dist/output.rs",
        BackendOutputKind::Wasm => "dist/output.wasm",
        BackendOutputKind::Native => "dist/output.o",
    }
}

fn resolve_backend_output_path(output_path: Option<&str>, kind: BackendOutputKind) -> &str {
    output_path.unwrap_or(default_backend_output_path(kind))
}

#[cfg(feature = "rust-backend")]
fn rust_source_for_ir(ir: &SimpleIR) -> io::Result<String> {
    let mut ir = ir.clone();
    ir.tree_shake_source_emission();
    let mut backend = RustBackend::new();
    backend.compile_checked(&ir).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Rust validation failed: {err}"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LuauTirModulePipelineStats {
    functions: usize,
    module_changed: usize,
}

fn run_luau_tir_module_pipeline(ir: &mut SimpleIR) -> io::Result<LuauTirModulePipelineStats> {
    let target_info = molt_backend::tir::target_info::TargetInfo::luau_release_fast();
    let (mut tir_module, idx_map) =
        molt_backend::tir::lower_from_simple::lower_functions_to_tir_module(&ir.functions);
    tir_module.name = "luau_module".to_string();

    for tir_func in &mut tir_module.functions {
        molt_backend::tir::type_refine::refine_types(tir_func);
        let _stats = molt_backend::tir::passes::run_pipeline(tir_func, &target_info);
        molt_backend::tir::type_refine::refine_types(tir_func);
    }

    let non_inlinable = std::collections::HashSet::new();
    let module_analysis =
        molt_backend::tir::run_module_pipeline(&mut tir_module, &target_info, &non_inlinable);

    for (pos, &orig_idx) in idx_map.iter().enumerate() {
        let tir_func = &tir_module.functions[pos];
        let ops = molt_backend::tir::lower_to_simple::lower_to_simple_ir(tir_func);
        if !molt_backend::tir::lower_to_simple::validate_labels(&ops) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Luau TIR module back-conversion emitted invalid labels for '{}'",
                    tir_func.name
                ),
            ));
        }
        ir.functions[orig_idx].ops = ops;
    }

    Ok(LuauTirModulePipelineStats {
        functions: tir_module.functions.len(),
        module_changed: module_analysis.changed_functions.len(),
    })
}

fn partition_functions_for_batches(
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

fn batch_external_function_names(
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

#[cfg(all(feature = "native-backend", target_os = "macos"))]
fn release_native_backend_batch_memory_to_os() {
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

#[cfg(all(feature = "native-backend", target_os = "linux", target_env = "gnu"))]
fn release_native_backend_batch_memory_to_os() {
    unsafe {
        let _ = libc::malloc_trim(0);
    }
}

#[cfg(all(
    feature = "native-backend",
    not(target_os = "macos"),
    not(all(target_os = "linux", target_env = "gnu"))
))]
fn release_native_backend_batch_memory_to_os() {}

fn resolved_batch_size_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

fn resolved_batch_op_budget_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

#[cfg(feature = "native-backend")]
struct NativeApplicationObjectOptions<'a> {
    target_triple: Option<&'a str>,
    stdlib_split_enabled: bool,
    app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    log_prefix: &'a str,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeApplicationObjectResult {
    function_count: usize,
    batch_count: usize,
}

#[cfg(feature = "native-backend")]
#[derive(serde::Deserialize, serde::Serialize)]
struct NativeBatchModuleMetadata {
    module_context: molt_backend::NativeBackendModuleContext,
}

#[cfg(feature = "native-backend")]
#[derive(serde::Deserialize, serde::Serialize)]
struct NativeBatchObjectJob {
    ir: SimpleIR,
    module_context_path: PathBuf,
    target_triple: Option<String>,
    emit_app_intrinsic_resolver: bool,
    app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    external_function_names: std::collections::BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone)]
struct NativeBatchJobSpec {
    job_path: PathBuf,
    object_path: PathBuf,
}

#[cfg(feature = "native-backend")]
fn deduplicate_functions_by_name(functions: &mut Vec<molt_backend::FunctionIR>) {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    functions.retain(|f| seen.insert(f.name.clone()));
}

fn relocatable_linker_binary(linker_override: Option<&str>) -> String {
    linker_override
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("MOLT_LINKER").ok())
        .or_else(|| std::env::var("LD").ok())
        .or_else(|| std::env::var("CC").ok())
        .unwrap_or_else(|| "ld".to_string())
}

fn merge_relocatable_objects(
    output_path: &Path,
    object_paths: &[std::path::PathBuf],
    linker_override: Option<&str>,
) -> io::Result<()> {
    if object_paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no object files to merge",
        ));
    }

    ensure_output_parent_dir(output_path.to_str().unwrap_or_default())?;

    if object_paths.len() == 1 {
        std::fs::copy(&object_paths[0], output_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to copy batch object '{}' to '{}': {}",
                    object_paths[0].display(),
                    output_path.display(),
                    err
                ),
            )
        })?;
        return Ok(());
    }

    let ld_bin = relocatable_linker_binary(linker_override);
    let mut cmd = std::process::Command::new(&ld_bin);
    if ld_bin.contains("clang") || ld_bin.contains("gcc") {
        cmd.arg("-Wl,-r").arg("-o").arg(output_path);
    } else {
        cmd.arg("-r").arg("-o").arg(output_path);
    }
    for path in object_paths {
        cmd.arg(path);
    }
    let merge_output = cmd.output().map_err(|err| {
        io::Error::other(format!(
            "failed to run relocatable linker '{ld_bin}' for '{}': {err}",
            output_path.display()
        ))
    })?;
    if merge_output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&merge_output.stderr)
        .trim()
        .to_string();
    let detail = if stderr.is_empty() {
        format!("exit {}", merge_output.status)
    } else {
        stderr
    };
    Err(io::Error::other(format!(
        "relocatable link failed via '{ld_bin}' for '{}': {detail}",
        output_path.display()
    )))
}

#[cfg(feature = "native-backend")]
fn write_json_artifact<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let file = File::create(path)?;
    let writer = io::BufWriter::new(file);
    serde_json::to_writer(writer, value).map_err(io::Error::other)
}

#[cfg(feature = "native-backend")]
fn read_json_artifact<T: serde::de::DeserializeOwned>(path: &Path, label: &str) -> io::Result<T> {
    let file = File::open(path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to open {label} '{}': {err}", path.display()),
        )
    })?;
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {label} '{}': {err}", path.display()),
        )
    })
}

#[cfg(feature = "native-backend")]
fn remove_native_batch_temp_dir(path: &Path, label: &str) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("failed to remove {label} '{}': {err}", path.display()),
        )),
    }
}

#[cfg(feature = "native-backend")]
fn finish_native_batch_temp_dir(
    path: &Path,
    label: &str,
    compile_result: io::Result<()>,
) -> io::Result<()> {
    let cleanup_result = remove_native_batch_temp_dir(path, label);
    match (compile_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(compile_err), Ok(())) => Err(compile_err),
        (Err(compile_err), Err(cleanup_err)) => {
            eprintln!(
                "MOLT_BACKEND: failed to clean {label} '{}' after compile error: {cleanup_err}",
                path.display()
            );
            Err(compile_err)
        }
    }
}

#[cfg(feature = "native-backend")]
fn compile_native_batch_object_job(
    job: NativeBatchObjectJob,
    output_path: &Path,
) -> io::Result<()> {
    let metadata: NativeBatchModuleMetadata =
        read_json_artifact(&job.module_context_path, "native batch module metadata")?;
    let mut backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
    backend.skip_ir_passes = true;
    backend.skip_shared_stdlib_partition = true;
    backend.emit_app_intrinsic_resolver = job.emit_app_intrinsic_resolver;
    backend.app_intrinsic_manifest = job.app_intrinsic_manifest;
    backend.external_function_names = job.external_function_names;
    backend.set_module_context(metadata.module_context);
    let output = backend.compile(job.ir);
    write_output_path(output_path, &output.bytes)
}

#[cfg(feature = "native-backend")]
fn compile_native_batch_object_job_file(job_path: &Path, output_path: &Path) -> io::Result<()> {
    let job: NativeBatchObjectJob = read_json_artifact(job_path, "native batch object job")?;
    compile_native_batch_object_job(job, output_path)
}

#[cfg(feature = "native-backend")]
fn sanitize_debug_artifact_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "artifact".to_string()
    } else {
        sanitized.to_string()
    }
}

#[cfg(feature = "native-backend")]
fn preserve_native_batch_worker_failure_artifacts(
    label: &str,
    job_path: &Path,
    object_path: &Path,
) -> io::Result<PathBuf> {
    let mut job: NativeBatchObjectJob =
        read_json_artifact(job_path, "failed native batch object job")?;
    let job_stem = job_path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(sanitize_debug_artifact_component)
        .unwrap_or_else(|| "batch".to_string());
    let label_component = sanitize_debug_artifact_component(label);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let manifest_marker = molt_backend::debug_artifacts::prepare_debug_artifact_path(format!(
        "native-batch-failures/{label_component}/{}-{nonce}-{job_stem}/manifest.json",
        std::process::id()
    ))?;
    let artifact_dir = manifest_marker
        .parent()
        .ok_or_else(|| io::Error::other("debug artifact path has no parent"))?
        .to_path_buf();
    std::fs::create_dir_all(&artifact_dir)?;

    let copied_module_context = artifact_dir.join("module_context.json");
    std::fs::copy(&job.module_context_path, &copied_module_context).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to preserve native batch module context '{}' to '{}': {err}",
                job.module_context_path.display(),
                copied_module_context.display()
            ),
        )
    })?;
    let original_module_context_path = job.module_context_path.clone();
    job.module_context_path = copied_module_context.clone();

    let copied_job = artifact_dir.join(
        job_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("batch.json")),
    );
    write_json_artifact(&copied_job, &job)?;

    let copied_object = if object_path.exists() {
        let copied_object = artifact_dir.join(
            object_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("batch.o")),
        );
        std::fs::copy(object_path, &copied_object).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to preserve partial native batch object '{}' to '{}': {err}",
                    object_path.display(),
                    copied_object.display()
                ),
            )
        })?;
        Some(copied_object)
    } else {
        None
    };
    let replay_object = artifact_dir.join("replay.o");
    let manifest = serde_json::json!({
        "schema_version": 1,
        "label": label,
        "source_job_path": job_path.display().to_string(),
        "source_object_path": object_path.display().to_string(),
        "source_module_context_path": original_module_context_path.display().to_string(),
        "copied_job_path": copied_job.display().to_string(),
        "copied_object_path": copied_object.as_ref().map(|path| path.display().to_string()),
        "copied_module_context_path": copied_module_context.display().to_string(),
        "replay": {
            "argv": [
                "target/debug/molt-backend",
                "--native-batch-job-file",
                copied_job.display().to_string(),
                "--output",
                replay_object.display().to_string()
            ]
        }
    });
    write_json_artifact(&manifest_marker, &manifest)?;
    Ok(artifact_dir)
}

#[cfg(feature = "native-backend")]
fn run_native_batch_worker_with_failure_artifacts(
    label: &str,
    job_path: &Path,
    object_path: &Path,
) -> io::Result<()> {
    match run_native_batch_worker(job_path, object_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let original_error = err.to_string();
            match preserve_native_batch_worker_failure_artifacts(label, job_path, object_path) {
                Ok(artifact_dir) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; preserved replayable {label} artifacts at '{}'",
                        artifact_dir.display()
                    ),
                )),
                Err(preserve_err) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; additionally failed to preserve {label} artifacts: {preserve_err}"
                    ),
                )),
            }
        }
    }
}

#[cfg(all(feature = "native-backend", not(test)))]
fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    let exe = std::env::current_exe().map_err(|err| {
        io::Error::other(format!(
            "failed to resolve current backend executable for batch worker: {err}"
        ))
    })?;
    let status = std::process::Command::new(&exe)
        .arg("--native-batch-job-file")
        .arg(job_path)
        .arg("--output")
        .arg(object_path)
        .status()
        .map_err(|err| {
            io::Error::other(format!(
                "failed to spawn native batch worker '{}' for '{}': {err}",
                exe.display(),
                job_path.display()
            ))
        })?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "native batch worker failed for '{}' with {status}",
        job_path.display()
    )))
}

#[cfg(all(feature = "native-backend", test))]
fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    compile_native_batch_object_job_file(job_path, object_path)
}

#[cfg(feature = "native-backend")]
fn compile_native_application_object_to_path(
    mut ir: SimpleIR,
    output_path: &Path,
    mut options: NativeApplicationObjectOptions<'_>,
) -> io::Result<NativeApplicationObjectResult> {
    if options.stdlib_split_enabled && options.app_intrinsic_manifest.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stdlib-split native application object requires a full-set app intrinsic manifest",
        ));
    }

    // Preserve the one-shot native application-object sequence as the single
    // authority for both direct backend runs and daemon requests.
    molt_backend::inject_runtime_exit(&mut ir);
    if !options.stdlib_split_enabled {
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
    }
    deduplicate_functions_by_name(&mut ir.functions);

    let function_count = ir.functions.len();
    let batch_size = resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE);
    let batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    let total_ops = ir
        .functions
        .iter()
        .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
    if function_count <= batch_size && total_ops <= batch_ops_budget {
        let mut backend = SimpleBackend::new_with_target(options.target_triple);
        if options.stdlib_split_enabled {
            backend.skip_shared_stdlib_partition = true;
        }
        backend.app_intrinsic_manifest = options.app_intrinsic_manifest.take();
        let obj_output = backend.compile(ir);
        write_output_path(output_path, &obj_output.bytes)?;
        eprintln!(
            "Successfully compiled to {} ({} functions)",
            output_path.display(),
            function_count
        );
        return Ok(NativeApplicationObjectResult {
            function_count,
            batch_count: 1,
        });
    }

    let profile = ir.profile;
    let all_functions: Vec<_> = ir.functions.into_iter().collect();
    let all_func_names: std::collections::BTreeSet<String> =
        all_functions.iter().map(|f| f.name.clone()).collect();
    let module_context = SimpleBackend::build_module_context(&all_functions);
    if options.app_intrinsic_manifest.is_none() {
        options.app_intrinsic_manifest = Some(molt_backend::compute_intrinsic_manifest_checked(
            &all_functions,
        ));
    }
    let batches = partition_functions_for_batches(all_functions, batch_size, batch_ops_budget);
    let total_batches = batches.len();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt_batch_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir)?;
    let module_context_path = tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata { module_context },
    )?;
    let mut batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        for (batch_idx, batch_funcs) in batches.into_iter().enumerate() {
            let batch_ops = batch_funcs
                .iter()
                .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
            eprintln!(
                "{}: batch {}/{total_batches} ({} functions, {} ops / budget {})",
                options.log_prefix,
                batch_idx + 1,
                batch_funcs.len(),
                batch_ops,
                batch_ops_budget
            );
            let batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&all_func_names, &batch_ir.functions);
            let job_path = tmp_dir.join(format!("batch_{batch_idx}.json"));
            let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: options.target_triple.map(str::to_owned),
                    emit_app_intrinsic_resolver: batch_idx == 0,
                    app_intrinsic_manifest: if batch_idx == 0 {
                        options.app_intrinsic_manifest.take()
                    } else {
                        None
                    },
                    external_function_names,
                },
            )?;
            batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut batch_paths: Vec<std::path::PathBuf> = Vec::with_capacity(batch_specs.len());
        for (batch_idx, spec) in batch_specs.iter().enumerate() {
            eprintln!(
                "{}: compiling materialized batch {}/{}",
                options.log_prefix,
                batch_idx + 1,
                total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native application batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(output_path, &batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &tmp_dir,
        "native application batch temp dir",
        compile_result,
    )?;

    eprintln!(
        "Successfully compiled to {} ({} functions, {} batches)",
        output_path.display(),
        function_count,
        total_batches
    );
    Ok(NativeApplicationObjectResult {
        function_count,
        batch_count: total_batches,
    })
}

#[cfg(feature = "native-backend")]
fn compile_stdlib_cache_object(
    stdlib_path: &Path,
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
) -> io::Result<()> {
    let stdlib_count = stdlib_funcs.len();
    if stdlib_count == 0 {
        eprintln!("{log_prefix}: stdlib cache is empty (0 reachable functions)");
        let stdlib_ir = SimpleIR {
            functions: Vec::new(),
            profile,
        };
        let mut stdlib_backend = SimpleBackend::new_with_target(target_triple);
        stdlib_backend.skip_ir_passes = true;
        stdlib_backend.skip_shared_stdlib_partition = true;
        stdlib_backend.emit_app_intrinsic_resolver = false;
        let stdlib_output = stdlib_backend.compile(stdlib_ir);
        std::fs::write(stdlib_path, &stdlib_output.bytes)?;
        return Ok(());
    }

    let stdlib_batch_size = resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE);
    let stdlib_batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    let all_stdlib_names: std::collections::BTreeSet<String> =
        stdlib_funcs.iter().map(|f| f.name.clone()).collect();
    let stdlib_module_context = SimpleBackend::build_module_context(&stdlib_funcs);
    let stdlib_batches =
        partition_functions_for_batches(stdlib_funcs, stdlib_batch_size, stdlib_batch_ops_budget);
    let stdlib_total_batches = stdlib_batches.len();

    if stdlib_total_batches == 1 {
        let batch_funcs = stdlib_batches.into_iter().next().unwrap_or_default();
        let batch_ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
        eprintln!(
            "{log_prefix}: stdlib batch 1/1 ({} functions, {} ops / budget {})",
            batch_funcs.len(),
            batch_ops,
            stdlib_batch_ops_budget
        );
        let stdlib_ir = SimpleIR {
            functions: batch_funcs,
            profile,
        };
        let mut stdlib_backend = SimpleBackend::new_with_target(target_triple);
        stdlib_backend.skip_ir_passes = true;
        stdlib_backend.skip_shared_stdlib_partition = true;
        // The stdlib cache object is not the main application object; the per-app
        // resolver is emitted once, into the main object.
        stdlib_backend.emit_app_intrinsic_resolver = false;
        let stdlib_output = stdlib_backend.compile(stdlib_ir);
        std::fs::write(stdlib_path, &stdlib_output.bytes)?;
        return Ok(());
    }

    let stdlib_tmp_dir =
        std::env::temp_dir().join(format!("molt_stdlib_batch_{}", std::process::id()));
    std::fs::create_dir_all(&stdlib_tmp_dir)?;
    let module_context_path = stdlib_tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata {
            module_context: stdlib_module_context,
        },
    )?;
    let mut stdlib_batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        for (stdlib_batch_idx, batch_funcs) in stdlib_batches.into_iter().enumerate() {
            let batch_ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
            eprintln!(
                "{log_prefix}: stdlib batch {}/{} ({} functions, {} ops / budget {})",
                stdlib_batch_idx + 1,
                stdlib_total_batches,
                batch_funcs.len(),
                batch_ops,
                stdlib_batch_ops_budget
            );
            let batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&all_stdlib_names, &batch_ir.functions);
            let job_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.json"));
            let batch_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: target_triple.map(str::to_owned),
                    emit_app_intrinsic_resolver: false,
                    app_intrinsic_manifest: None,
                    external_function_names,
                },
            )?;
            stdlib_batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut stdlib_batch_paths: Vec<std::path::PathBuf> =
            Vec::with_capacity(stdlib_batch_specs.len());
        for (stdlib_batch_idx, spec) in stdlib_batch_specs.iter().enumerate() {
            eprintln!(
                "{log_prefix}: compiling materialized stdlib batch {}/{}",
                stdlib_batch_idx + 1,
                stdlib_total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native stdlib batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            stdlib_batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(stdlib_path, &stdlib_batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &stdlib_tmp_dir,
        "native stdlib batch temp dir",
        compile_result,
    )
}

#[cfg(feature = "native-backend")]
fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let entry_init = format!("molt_init_{entry_module}");
    if name == "molt_main"
        || name == "molt_host_init"
        || name.starts_with(&format!("{entry_module}__"))
        || name == entry_init
        || name == "molt_init___main__"
        || name == "molt_isolate_import"
        || name == "molt_isolate_bootstrap"
    {
        return true;
    }
    if let Some(stdlib_module_symbols) = stdlib_module_symbols {
        if let Some(module_symbol) = emitted_module_symbol(name) {
            return !stdlib_module_symbols.contains(module_symbol);
        }
        return !stdlib_module_symbols
            .iter()
            .any(|module_symbol| emitted_name_matches_module_symbol(name, module_symbol));
    }
    false
}

#[cfg(feature = "native-backend")]
fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
) -> (Vec<molt_backend::FunctionIR>, Vec<molt_backend::FunctionIR>) {
    molt_backend::inject_runtime_exit(ir);
    molt_backend::eliminate_dead_functions(ir);
    molt_backend::eliminate_dead_imports(ir);
    molt_backend::eliminate_dead_ops(ir);
    let user_func_set: std::collections::BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| is_user_owned_symbol(&f.name, entry_module, stdlib_module_symbols))
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs)
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("count")
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("key")
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("manifest.json")
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_partition_manifest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("partition.json")
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_object_digest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("sha256")
}

#[cfg(feature = "native-backend")]
fn sha256_file_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = hasher.finalize();
    let bytes: &[u8] = digest.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

#[cfg(feature = "native-backend")]
const STDLIB_PARTITION_MANIFEST_SCHEMA: &str = "stdlib-partition-v1";

#[cfg(feature = "native-backend")]
fn update_fnv1a64(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(feature = "native-backend")]
fn shared_stdlib_partition_manifest(
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> io::Result<String> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut funcs: Vec<&molt_backend::FunctionIR> = stdlib_funcs.iter().collect();
    funcs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut body_hash = FNV_OFFSET;
    for func in funcs {
        names.push(func.name.clone());
        body_hash = update_fnv1a64(body_hash, func.name.as_bytes());
        body_hash = update_fnv1a64(body_hash, &[0]);
        let body = serde_json::to_vec(&serde_json::json!({
            "name": &func.name,
            "params": &func.params,
            "ops": &func.ops,
            "param_types": &func.param_types,
            "source_file": &func.source_file,
            "is_extern": func.is_extern,
        }))
        .map_err(io::Error::other)?;
        body_hash = update_fnv1a64(body_hash, &body);
        body_hash = update_fnv1a64(body_hash, &[0xff]);
    }

    serde_json::to_string(&serde_json::json!({
        "schema": STDLIB_PARTITION_MANIFEST_SCHEMA,
        "function_count": names.len(),
        "functions": names,
        "body_hash": format!("{body_hash:016x}"),
    }))
    .map_err(io::Error::other)
}

#[cfg(feature = "native-backend")]
fn stdlib_partition_reference_kind(kind: &str) -> bool {
    matches!(
        kind,
        "call"
            | "call_internal"
            | "func_new"
            | "func_new_closure"
            | "func_new_builtin"
            | "code_new"
            | "call_guarded"
            | "call_indirect"
            | "alloc_task"
            | "generator_create"
            | "coro_create"
            | "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "task_new"
            | "generator_send"
            | "spawn"
            | "call_func"
            | "call_method"
            | "import_from"
            | "import_name"
            | "class_def"
            | "decorator"
            | "super_call"
            | "yield_from"
            | "await"
    )
}

#[cfg(feature = "native-backend")]
fn shared_stdlib_partition_closure_issue(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let partition_names: std::collections::BTreeSet<&str> =
        stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
    let mut missing: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for func in stdlib_funcs {
        for op in &func.ops {
            if !stdlib_partition_reference_kind(op.kind.as_str()) {
                continue;
            }
            let Some(target) = op.s_value.as_deref() else {
                continue;
            };
            if !all_function_names.contains(target) {
                continue;
            }
            if !partition_names.contains(target) {
                missing.insert(format!("{} -> {}", func.name, target));
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    let preview: Vec<_> = missing.iter().take(8).cloned().collect();
    let suffix = if missing.len() > preview.len() {
        ", ..."
    } else {
        ""
    };
    Some(format!(
        "shared stdlib partition has unresolved SimpleIR function references: {}{}",
        preview.join(", "),
        suffix
    ))
}

#[cfg(feature = "native-backend")]
fn validate_shared_stdlib_partition(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> io::Result<()> {
    if let Some(issue) = shared_stdlib_partition_closure_issue(stdlib_funcs, all_function_names) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, issue));
    }
    Ok(())
}

#[cfg(feature = "native-backend")]
fn shared_stdlib_split_function_names(
    user_funcs: &[molt_backend::FunctionIR],
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> std::collections::BTreeSet<String> {
    user_funcs
        .iter()
        .chain(stdlib_funcs.iter())
        .map(|func| func.name.clone())
        .collect()
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_publish_lock_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_file_name(format!(
        "{}.publish.lock",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared")
    ))
}

#[cfg(feature = "native-backend")]
fn stdlib_cache_temp_publish_path(stdlib_path: &Path, label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    stdlib_path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared"),
        std::process::id(),
        stamp,
        label,
    ))
}

#[cfg(feature = "native-backend")]
fn atomic_replace_file(temp_path: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if final_path.exists() {
        let _ = std::fs::remove_file(final_path);
    }
    std::fs::rename(temp_path, final_path)
}

#[cfg(feature = "native-backend")]
fn sync_published_file(path: &Path) -> io::Result<()> {
    File::options()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

#[cfg(feature = "native-backend")]
fn write_atomic_text_file(path: &Path, contents: &str) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let temp_path = stdlib_cache_temp_publish_path(path, "text");
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    sync_published_file(path)?;
    Ok(())
}

#[cfg(all(feature = "native-backend", unix))]
fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let lock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if lock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if unlock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", windows))]
fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let mut overlapped = OVERLAPPED::default();
    let lock_rc = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if lock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    if unlock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", not(any(unix, windows))))]
fn with_shared_stdlib_cache_publish_lock<T>(
    _stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    body()
}

#[cfg(feature = "native-backend")]
fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_key_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) {
    let _ = std::fs::remove_file(stdlib_path);
    let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_object_digest_sidecar_path(stdlib_path));
}

#[cfg(feature = "native-backend")]
fn shared_stdlib_cache_matches(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    let Some(expected_key) = expected_key.filter(|key| !key.is_empty()) else {
        return false;
    };
    let Some(expected_manifest) = expected_manifest.filter(|manifest| !manifest.is_empty()) else {
        return false;
    };
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return false;
    }
    let Ok(actual_object_digest) = sha256_file_hex(stdlib_path) else {
        return false;
    };
    let Ok(cached_object_digest) =
        std::fs::read_to_string(stdlib_cache_object_digest_sidecar_path(stdlib_path))
    else {
        return false;
    };
    if cached_object_digest.trim() != actual_object_digest {
        return false;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
    {
        return cached_partition_manifest.as_deref() == Some(expected_partition_manifest);
    }
    cached_partition_manifest.is_some()
}

#[cfg(all(
    any(unix, test),
    any(feature = "native-backend", feature = "wasm-backend")
))]
fn daemon_memory_cache_allowed_for_job(job: &DaemonJobRequest) -> bool {
    if job.is_wasm {
        return true;
    }
    #[cfg(feature = "native-backend")]
    {
        let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
            return true;
        };
        shared_stdlib_cache_matches(
            Path::new(&stdlib_obj_path),
            std::env::var("MOLT_STDLIB_CACHE_KEY").ok().as_deref(),
            std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok().as_deref(),
            None,
        )
    }
    #[cfg(not(feature = "native-backend"))]
    {
        false
    }
}

#[cfg(feature = "native-backend")]
fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
    write_atomic_text_file(&count_path, &stdlib_count.to_string())?;

    let key_path = stdlib_cache_key_sidecar_path(stdlib_path);
    if let Some(cache_key) = cache_key.filter(|key| !key.is_empty()) {
        write_atomic_text_file(&key_path, cache_key)?;
    } else {
        match std::fs::remove_file(&key_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    let manifest_path = stdlib_cache_manifest_sidecar_path(stdlib_path);
    if let Some(cache_manifest) = cache_manifest.filter(|manifest| !manifest.is_empty()) {
        write_atomic_text_file(&manifest_path, cache_manifest)?;
    } else {
        match std::fs::remove_file(&manifest_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    write_atomic_text_file(
        &stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        partition_manifest,
    )?;
    let object_digest = sha256_file_hex(stdlib_path)?;
    write_atomic_text_file(
        &stdlib_cache_object_digest_sidecar_path(stdlib_path),
        &object_digest,
    )?;
    Ok(())
}

#[cfg(feature = "native-backend")]
fn publish_shared_stdlib_cache_object(
    stdlib_path: &Path,
    temp_object_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let result = with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        if let Err(err) = atomic_replace_file(temp_object_path, stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = sync_published_file(stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = write_shared_stdlib_cache_sidecars(
            stdlib_path,
            stdlib_count,
            cache_key,
            cache_manifest,
            partition_manifest,
        ) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temp_object_path);
    }
    result
}

#[derive(Debug)]
#[cfg(any(unix, test))]
struct DaemonHealthResponse {
    protocol_version: u32,
    pid: u32,
    spawn_config_digest: Option<String>,
    active_config_digest: Option<String>,
    uptime_ms: u64,
    cache_entries: usize,
    cache_bytes: usize,
    cache_max_bytes: Option<usize>,
    request_limit_bytes: Option<usize>,
    max_jobs: Option<usize>,
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
struct DaemonResponse {
    ok: bool,
    pong: bool,
    jobs: Vec<DaemonJobResponse>,
    error: Option<String>,
    health: Option<DaemonHealthResponse>,
}

#[cfg(any(unix, test))]
impl DaemonJobRequest {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
        let ir_path = optional_string(obj, "ir_path", ctx)?;
        if obj.get("ir").is_some_and(|value| !value.is_null()) && ir_path.is_some() {
            return Err(format!(
                "{ctx} must use exactly one IR custody field: ir or ir_path"
            ));
        }
        let ir = match obj.get("ir") {
            None | Some(JsonValue::Null) => None,
            Some(ir_value) => Some(SimpleIR::from_json_value(ir_value)?),
        };
        Ok(Self {
            id: required_string(obj, "id", ctx)?,
            is_wasm,
            target_triple: optional_string(obj, "target_triple", ctx)?,
            wasm_link: optional_bool(obj, "wasm_link", ctx)?.unwrap_or(false),
            wasm_data_base: optional_u32(obj, "wasm_data_base", ctx)?,
            wasm_table_base: optional_u32(obj, "wasm_table_base", ctx)?,
            wasm_split_runtime_runtime_table_min: optional_u32(
                obj,
                "wasm_split_runtime_runtime_table_min",
                ctx,
            )?,
            output: required_string(obj, "output", ctx)?,
            cache_key: required_string(obj, "cache_key", ctx)?,
            function_cache_key: optional_string(obj, "function_cache_key", ctx)?,
            skip_module_output_if_synced: optional_bool(obj, "skip_module_output_if_synced", ctx)?
                .unwrap_or(false),
            skip_function_output_if_synced: optional_bool(
                obj,
                "skip_function_output_if_synced",
                ctx,
            )?
            .unwrap_or(false),
            probe_cache_only: optional_bool(obj, "probe_cache_only", ctx)?.unwrap_or(false),
            ir,
            ir_path,
        })
    }
}

#[cfg(any(unix, test))]
fn simple_ir_from_json_path(path: &str) -> Result<SimpleIR, String> {
    let file = File::open(path).map_err(|err| format!("failed to open ir_path {path:?}: {err}"))?;
    serde_json::from_reader(io::BufReader::new(file))
        .map_err(|err| format!("failed to parse ir_path {path:?}: {err}"))
}

#[cfg(any(unix, test))]
impl DaemonRequest {
    fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
        let value: JsonValue =
            serde_json::from_slice(bytes).map_err(|err| format!("invalid request JSON: {err}"))?;
        let obj = expect_object(&value, "request")?;
        let version = match obj.get("version") {
            None | Some(JsonValue::Null) => None,
            Some(value) => {
                let Some(raw) = value.as_u64() else {
                    return Err("request.version must be a non-negative integer".to_string());
                };
                Some(
                    u32::try_from(raw)
                        .map_err(|_| "request.version is out of range for u32".to_string())?,
                )
            }
        };
        let jobs = match obj.get("jobs") {
            None | Some(JsonValue::Null) => None,
            Some(value) => {
                let array = value
                    .as_array()
                    .ok_or_else(|| "request.jobs must be an array".to_string())?;
                let mut out = Vec::with_capacity(array.len());
                for (idx, item) in array.iter().enumerate() {
                    out.push(DaemonJobRequest::from_json_value(
                        item,
                        &format!("request.jobs[{idx}]"),
                    )?);
                }
                Some(out)
            }
        };
        // Apply per-request env var overrides so callers can control
        // backend diagnostics and non-TIR tuning without restarting the
        // daemon. TIR itself is not request-optional.
        for key in DAEMON_REQUEST_ENV_KEYS {
            unsafe {
                std::env::remove_var(key);
            }
        }
        if let Some(JsonValue::Object(env_map)) = obj.get("env") {
            for (key, val) in env_map {
                if let Some(s) = val.as_str() {
                    if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                        molt_backend::parse_stdlib_module_symbols(s)?;
                    }
                    unsafe {
                        std::env::set_var(key, s);
                    }
                } else if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                    return Err(format!(
                        "{} must be a string containing a JSON array of emitted module symbols",
                        molt_backend::STDLIB_MODULE_SYMBOLS_ENV
                    ));
                }
            }
        }
        Ok(Self {
            version,
            ping: optional_bool(obj, "ping", "request")?,
            include_health: optional_bool(obj, "include_health", "request")?,
            config_digest: optional_string(obj, "config_digest", "request")?,
            jobs,
        })
    }
}

#[cfg(any(unix, test))]
impl DaemonJobResponse {
    fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), JsonValue::String(self.id.clone()));
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("cached".to_string(), JsonValue::Bool(self.cached));
        if let Some(cache_tier) = &self.cache_tier {
            obj.insert(
                "cache_tier".to_string(),
                JsonValue::String(cache_tier.clone()),
            );
        }
        obj.insert(
            "output_written".to_string(),
            JsonValue::Bool(self.output_written),
        );
        if !is_false(&self.needs_ir) {
            obj.insert("needs_ir".to_string(), JsonValue::Bool(self.needs_ir));
        }
        if let Some(message) = &self.message {
            obj.insert("message".to_string(), JsonValue::String(message.clone()));
        }
        if !self.warnings.is_empty() {
            obj.insert(
                "warnings".to_string(),
                JsonValue::Array(
                    self.warnings
                        .iter()
                        .map(|w| JsonValue::String(w.clone()))
                        .collect(),
                ),
            );
        }
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonHealthResponse {
    fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "protocol_version".to_string(),
            JsonValue::from(self.protocol_version),
        );
        obj.insert("pid".to_string(), JsonValue::from(self.pid));
        if let Some(spawn_config_digest) = &self.spawn_config_digest {
            obj.insert(
                "spawn_config_digest".to_string(),
                JsonValue::String(spawn_config_digest.clone()),
            );
        }
        if let Some(active_config_digest) = &self.active_config_digest {
            obj.insert(
                "active_config_digest".to_string(),
                JsonValue::String(active_config_digest.clone()),
            );
        }
        obj.insert("uptime_ms".to_string(), JsonValue::from(self.uptime_ms));
        obj.insert(
            "cache_entries".to_string(),
            JsonValue::from(self.cache_entries),
        );
        obj.insert("cache_bytes".to_string(), JsonValue::from(self.cache_bytes));
        if let Some(cache_max_bytes) = self.cache_max_bytes {
            obj.insert(
                "cache_max_bytes".to_string(),
                JsonValue::from(cache_max_bytes),
            );
        }
        if let Some(request_limit_bytes) = self.request_limit_bytes {
            obj.insert(
                "request_limit_bytes".to_string(),
                JsonValue::from(request_limit_bytes),
            );
        }
        if let Some(max_jobs) = self.max_jobs {
            obj.insert("max_jobs".to_string(), JsonValue::from(max_jobs));
        }
        obj.insert(
            "requests_total".to_string(),
            JsonValue::from(self.requests_total),
        );
        obj.insert("jobs_total".to_string(), JsonValue::from(self.jobs_total));
        obj.insert("cache_hits".to_string(), JsonValue::from(self.cache_hits));
        obj.insert(
            "cache_misses".to_string(),
            JsonValue::from(self.cache_misses),
        );
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonResponse {
    fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("pong".to_string(), JsonValue::Bool(self.pong));
        obj.insert(
            "jobs".to_string(),
            JsonValue::Array(
                self.jobs
                    .iter()
                    .map(DaemonJobResponse::to_json_value)
                    .collect(),
            ),
        );
        if let Some(error) = &self.error {
            obj.insert("error".to_string(), JsonValue::String(error.clone()));
        }
        if let Some(health) = &self.health {
            obj.insert("health".to_string(), health.to_json_value());
        }
        JsonValue::Object(obj)
    }
}

#[derive(Default)]
#[cfg(any(unix, test))]
struct DaemonStats {
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct DaemonCache {
    entries: HashMap<Arc<str>, CacheEntry>,
    order: BinaryHeap<Reverse<(u64, Arc<str>)>>,
    clock: u64,
    bytes: usize,
    max_bytes: Option<usize>,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct CacheEntry {
    bytes: Arc<[u8]>,
    stamp: u64,
}

#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
impl DaemonCache {
    fn new(max_bytes: Option<usize>) -> Self {
        Self {
            entries: HashMap::new(),
            order: BinaryHeap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
        }
    }

    fn get_bytes(&mut self, key: &str) -> Option<&[u8]> {
        let key_ref = Arc::clone(self.entries.get_key_value(key)?.0);
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        entry.stamp = stamp;
        self.order.push(Reverse((stamp, key_ref)));
        Some(entry.bytes.as_ref())
    }

    fn insert(&mut self, key: String, value: Arc<[u8]>) {
        if key.is_empty() {
            return;
        }
        if let Some(prev) = self.entries.remove(key.as_str()) {
            self.bytes = self.bytes.saturating_sub(prev.bytes.len());
        }
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        self.bytes = self.bytes.saturating_add(value.len());
        let key = Arc::<str>::from(key);
        self.order.push(Reverse((stamp, Arc::clone(&key))));
        self.entries.insert(
            key,
            CacheEntry {
                bytes: value,
                stamp,
            },
        );
        self.evict();
    }

    fn evict(&mut self) {
        while self
            .max_bytes
            .is_some_and(|max_bytes| self.bytes > max_bytes)
        {
            let Some(Reverse((stamp, old_key))) = self.order.pop() else {
                break;
            };
            let is_live = self
                .entries
                .get(&old_key)
                .is_some_and(|entry| entry.stamp == stamp);
            if !is_live {
                continue;
            }
            if let Some(old_val) = self.entries.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(old_val.bytes.len());
            }
        }
        // Compact stale generations after enough churn.
        if self.order.len() > self.entries.len().saturating_mul(8).saturating_add(32) {
            let mut compacted = BinaryHeap::with_capacity(self.entries.len());
            for (key, entry) in &self.entries {
                compacted.push(Reverse((entry.stamp, Arc::clone(key))));
            }
            self.order = compacted;
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.clock = 0;
        self.bytes = 0;
    }
}

fn env_usize_limit(name: &str, default: usize, min_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value >= min_value)
        .unwrap_or(default)
}

#[cfg(any(unix, test))]
fn daemon_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_DAEMON_REQUEST_LIMIT_BYTES",
        DEFAULT_DAEMON_REQUEST_LIMIT_BYTES,
        1024,
    )
}

fn stdin_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES",
        DEFAULT_STDIN_REQUEST_LIMIT_BYTES,
        1024,
    )
}

#[cfg(any(unix, test))]
fn daemon_max_jobs() -> usize {
    env_usize_limit("MOLT_BACKEND_DAEMON_MAX_JOBS", DEFAULT_DAEMON_MAX_JOBS, 1)
}

#[derive(Debug)]
struct RequestBoundedRead<R> {
    inner: R,
    remaining: usize,
    limit_bytes: usize,
    context: &'static str,
}

impl<R: Read> RequestBoundedRead<R> {
    fn new(inner: R, limit_bytes: usize, context: &'static str) -> Self {
        Self {
            inner,
            remaining: limit_bytes,
            limit_bytes,
            context,
        }
    }
}

impl<R: Read> Read for RequestBoundedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe) {
                Ok(0) => Ok(0),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} exceeded {} byte limit", self.context, self.limit_bytes),
                )),
                Err(err) => Err(err),
            };
        }

        let read_len = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..read_len])?;
        self.remaining = self.remaining.saturating_sub(n);
        Ok(n)
    }
}

fn read_bounded_request_bytes<R: Read>(
    reader: R,
    limit_bytes: usize,
    context: &'static str,
) -> io::Result<Vec<u8>> {
    let mut bounded = RequestBoundedRead::new(reader, limit_bytes, context);
    let mut bytes = Vec::new();
    bounded.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(any(unix, test))]
fn default_daemon_cache_bytes_from_physical_mem_bytes(bytes: Option<u64>) -> usize {
    let default = bytes
        .and_then(|raw| usize::try_from(raw / 64).ok())
        .unwrap_or(512 * MIB);
    default.clamp(128 * MIB, 2 * 1024 * MIB)
}

#[cfg(any(unix, test))]
fn daemon_cache_limit_bytes() -> usize {
    env::var("MOLT_BACKEND_DAEMON_CACHE_MB")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|mb| mb.saturating_mul(MIB))
        .unwrap_or_else(|| {
            default_daemon_cache_bytes_from_physical_mem_bytes(detect_physical_memory_bytes())
        })
}

#[cfg(any(unix, test))]
fn daemon_health(
    cache: &DaemonCache,
    stats: &DaemonStats,
    spawn_config_digest: Option<&str>,
    active_config_digest: Option<&str>,
    start: Instant,
    request_limit_bytes: usize,
    max_jobs: usize,
) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        spawn_config_digest: spawn_config_digest.map(str::to_string),
        active_config_digest: active_config_digest.map(str::to_string),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes: Some(request_limit_bytes),
        max_jobs: Some(max_jobs),
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
enum DaemonCompiledOutput {
    #[cfg(feature = "wasm-backend")]
    Bytes(Arc<[u8]>),
    WrittenToPath,
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
fn insert_daemon_cache_entries(
    cache: &mut DaemonCache,
    cache_key: &str,
    function_cache_key: &str,
    output_bytes: Arc<[u8]>,
) {
    if !cache_key.is_empty() && !function_cache_key.is_empty() && function_cache_key != cache_key {
        cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
        cache.insert(function_cache_key.to_string(), output_bytes);
    } else if !cache_key.is_empty() {
        cache.insert(cache_key.to_string(), output_bytes);
    } else if !function_cache_key.is_empty() {
        cache.insert(function_cache_key.to_string(), output_bytes);
    }
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
fn maybe_cache_output_file(
    cache: &mut DaemonCache,
    output_path: &Path,
    cache_key: &str,
    function_cache_key: &str,
    warnings: &mut Vec<String>,
) {
    if cache_key.is_empty() && function_cache_key.is_empty() {
        return;
    }
    let metadata = match std::fs::metadata(output_path) {
        Ok(metadata) => metadata,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': metadata failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    let output_len = metadata.len();
    if cache
        .max_bytes
        .is_some_and(|max_bytes| output_len > max_bytes as u64)
    {
        let warning = format!(
            "skipped daemon memory cache for '{}' ({} bytes exceeds cache budget)",
            output_path.display(),
            output_len
        );
        eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
        warnings.push(warning);
        return;
    }
    let bytes = match std::fs::read(output_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': read failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    insert_daemon_cache_entries(
        cache,
        cache_key,
        function_cache_key,
        Arc::from(bytes.into_boxed_slice()),
    );
}

#[cfg(any(unix, test))]
fn compile_single_job(job: DaemonJobRequest, _cache: &mut DaemonCache) -> DaemonJobResponse {
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    {
        let unsupported = if job.is_wasm {
            "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend"
        } else {
            "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend"
        };
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            output_written: false,
            needs_ir: false,
            message: Some(unsupported.to_string()),
            warnings: Vec::new(),
        };
    }

    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    {
        let cache_key = job.cache_key.trim();
        let function_cache_key = job
            .function_cache_key
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        let daemon_memory_cache_allowed = daemon_memory_cache_allowed_for_job(&job);
        if daemon_memory_cache_allowed
            && !cache_key.is_empty()
            && let Some(bytes) = _cache.get_bytes(cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_module_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("module".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }
        if daemon_memory_cache_allowed
            && !function_cache_key.is_empty()
            && function_cache_key != cache_key
            && let Some(bytes) = _cache.get_bytes(function_cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_function_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("function".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }

        if job.probe_cache_only {
            return DaemonJobResponse {
                id: job.id,
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: true,
                message: None,
                warnings: Vec::new(),
            };
        }

        let mut ir = if let Some(ir) = job.ir {
            ir
        } else if let Some(ir_path) = job.ir_path.as_deref() {
            match simple_ir_from_json_path(ir_path) {
                Ok(ir) => ir,
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(err),
                        warnings: Vec::new(),
                    };
                }
            }
        } else {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some("missing ir for cache miss".to_string()),
                warnings: Vec::new(),
            };
        };

        let mut warnings = Vec::new();
        let compiled_output = if job.is_wasm {
            #[cfg(feature = "wasm-backend")]
            {
                let mut options = WasmCompileOptions {
                    reloc_enabled: job.wasm_link,
                    ..WasmCompileOptions::default()
                };
                if let Some(data_base) = job.wasm_data_base {
                    options.data_base = data_base;
                }
                if let Some(table_base) = job.wasm_table_base {
                    options.table_base = table_base;
                }
                if let Some(split_runtime_runtime_table_min) =
                    job.wasm_split_runtime_runtime_table_min
                {
                    options.split_runtime_runtime_table_min = Some(split_runtime_runtime_table_min);
                }
                let backend = WasmBackend::with_options(options);
                DaemonCompiledOutput::Bytes(Arc::from(backend.compile(ir)))
            }
            #[cfg(not(feature = "wasm-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        } else {
            #[cfg(feature = "native-backend")]
            {
                let target_triple = job.target_triple.as_deref();

                // ── Stdlib/user partition (mirrors one-shot path in main()) ──
                // When MOLT_STDLIB_OBJ is set, the daemon must exclude stdlib
                // functions from output.o to avoid duplicate symbols when the
                // CLI links output.o + stdlib_shared_*.o together.
                let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
                let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
                let expected_stdlib_cache_manifest =
                    std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
                let entry_module =
                    std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
                let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
                let explicit_stdlib_module_symbols =
                    match molt_backend::stdlib_module_symbols_from_env() {
                        Ok(symbols) => symbols,
                        Err(err) => {
                            return DaemonJobResponse {
                                id: job.id,
                                ok: false,
                                cached: false,
                                cache_tier: None,
                                output_written: false,
                                needs_ir: false,
                                message: Some(err),
                                warnings: Vec::new(),
                            };
                        }
                    };

                // When the program is split into a separate stdlib cache object,
                // compute the per-app intrinsic manifest over the FULL function
                // set now — before partitioning removes/externalizes the stdlib
                // bodies — so the main object's resolver covers intrinsics whose
                // defining stdlib wrappers live in the stdlib cache object. The
                // non-split path leaves this `None` and `compile` derives it from
                // the full `ir.functions` it already holds. This manifest always
                // feeds the main object's resolver; `_checked` filters against the
                // REQUIRED staticlib symbol set (fail-closed) whenever any
                // `molt_`-prefixed const_str exists, and yields the empty manifest
                // (zero-entry resolver, no relocations) otherwise.
                let app_intrinsic_manifest = stdlib_obj_path
                    .as_ref()
                    .map(|_| molt_backend::compute_intrinsic_manifest_checked(&ir.functions));

                if let Some(ref stdlib_path_str) = stdlib_obj_path {
                    let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
                        &mut ir,
                        &entry_module,
                        explicit_stdlib_module_symbols.as_ref(),
                    );
                    let stdlib_path = std::path::Path::new(stdlib_path_str);
                    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(
                        |e| {
                            eprintln!(
                                "MOLT_BACKEND(daemon): warning: failed to create stdlib parent: {e}"
                            );
                        },
                    );
                    let current_partition_manifest =
                        match shared_stdlib_partition_manifest(&stdlib_funcs) {
                            Ok(manifest) => manifest,
                            Err(err) => {
                                return DaemonJobResponse {
                                    id: job.id,
                                    ok: false,
                                    cached: false,
                                    cache_tier: None,
                                    output_written: false,
                                    needs_ir: false,
                                    message: Some(format!(
                                        "failed to compute shared stdlib partition manifest: {err}"
                                    )),
                                    warnings: Vec::new(),
                                };
                            }
                        };
                    let split_function_names =
                        shared_stdlib_split_function_names(&user_remaining, &stdlib_funcs);
                    if let Err(err) =
                        validate_shared_stdlib_partition(&stdlib_funcs, &split_function_names)
                    {
                        remove_shared_stdlib_cache_artifacts(stdlib_path);
                        return DaemonJobResponse {
                            id: job.id,
                            ok: false,
                            cached: false,
                            cache_tier: None,
                            output_written: false,
                            needs_ir: false,
                            message: Some(format!("invalid shared stdlib partition: {err}")),
                            warnings: Vec::new(),
                        };
                    }

                    if have_entry_module && stdlib_path.exists() {
                        if !shared_stdlib_cache_matches(
                            stdlib_path,
                            expected_stdlib_cache_key.as_deref(),
                            expected_stdlib_cache_manifest.as_deref(),
                            Some(current_partition_manifest.as_str()),
                        ) {
                            let cached_key = read_stdlib_cache_key(stdlib_path);
                            let cached_manifest = read_stdlib_cache_manifest(stdlib_path);
                            let cached_partition_manifest =
                                read_stdlib_cache_partition_manifest(stdlib_path);
                            eprintln!(
                                "MOLT_BACKEND(daemon): stdlib cache contract mismatch \
                                 (cached key {}, expected key {}; cached manifest {}, expected manifest present {}; cached partition manifest present {}, expected partition manifest present true) — rebuilding",
                                cached_key.as_deref().unwrap_or("<missing>"),
                                expected_stdlib_cache_key.as_deref().unwrap_or("<missing>"),
                                cached_manifest.as_deref().unwrap_or("<missing>"),
                                expected_stdlib_cache_manifest.is_some(),
                                cached_partition_manifest.is_some(),
                            );
                            remove_shared_stdlib_cache_artifacts(stdlib_path);
                        } else {
                            let mut retained = std::mem::take(&mut user_remaining);
                            let mut extern_count = 0usize;
                            for mut func in std::mem::take(&mut stdlib_funcs) {
                                molt_backend::externalize_function_with_signature(&mut func);
                                extern_count += 1;
                                retained.push(func);
                            }
                            let user_count = retained.len().saturating_sub(extern_count);
                            ir.functions = retained;
                            eprintln!(
                                "MOLT_BACKEND(daemon): incremental — compiling {user_count} user functions \
                             ({extern_count} stdlib extern from {})",
                                stdlib_path.display()
                            );
                        }
                    }

                    if !stdlib_path.exists() {
                        // First build (or stale cache was just deleted) — compile
                        // stdlib separately and cache it, then keep only user
                        // functions for output.o.
                        ensure_output_parent_dir(stdlib_path.to_str().unwrap_or(""))
                            .unwrap_or_else(|e| {
                                eprintln!(
                                    "MOLT_BACKEND(daemon): warning: could not create \
                                 stdlib cache parent dir: {e}"
                                );
                            });

                        let stdlib_count = stdlib_funcs.len();
                        eprintln!(
                            "MOLT_BACKEND(daemon): first build — caching {} stdlib functions to {}",
                            stdlib_count,
                            stdlib_path.display()
                        );
                        let temp_stdlib_path =
                            stdlib_cache_temp_publish_path(stdlib_path, "object");
                        if let Err(err) = compile_stdlib_cache_object(
                            &temp_stdlib_path,
                            std::mem::take(&mut stdlib_funcs),
                            ir.profile.clone(),
                            target_triple,
                            "MOLT_BACKEND(daemon)",
                        ) {
                            let _ = std::fs::remove_file(&temp_stdlib_path);
                            return DaemonJobResponse {
                                id: job.id,
                                ok: false,
                                cached: false,
                                cache_tier: None,
                                output_written: false,
                                needs_ir: false,
                                message: Some(format!(
                                    "failed to materialize shared stdlib cache: {err}"
                                )),
                                warnings: Vec::new(),
                            };
                        }
                        if let Err(err) = publish_shared_stdlib_cache_object(
                            stdlib_path,
                            &temp_stdlib_path,
                            stdlib_count,
                            expected_stdlib_cache_key.as_deref(),
                            expected_stdlib_cache_manifest.as_deref(),
                            current_partition_manifest.as_str(),
                        ) {
                            let _ = std::fs::remove_file(&temp_stdlib_path);
                            return DaemonJobResponse {
                                id: job.id,
                                ok: false,
                                cached: false,
                                cache_tier: None,
                                output_written: false,
                                needs_ir: false,
                                message: Some(format!(
                                    "failed to publish shared stdlib cache: {err}"
                                )),
                                warnings: Vec::new(),
                            };
                        }

                        ir.functions = std::mem::take(&mut user_remaining);
                        eprintln!(
                            "MOLT_BACKEND(daemon): compiling {} user functions",
                            ir.functions.len()
                        );
                    }
                }

                if let Err(err) = compile_native_application_object_to_path(
                    ir,
                    Path::new(&job.output),
                    NativeApplicationObjectOptions {
                        target_triple,
                        stdlib_split_enabled: stdlib_obj_path.is_some(),
                        app_intrinsic_manifest,
                        log_prefix: "MOLT_BACKEND(daemon)",
                    },
                ) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!(
                            "failed to compile native application object: {err}"
                        )),
                        warnings: Vec::new(),
                    };
                }
                DaemonCompiledOutput::WrittenToPath
            }
            #[cfg(not(feature = "native-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        };

        match compiled_output {
            #[cfg(feature = "wasm-backend")]
            DaemonCompiledOutput::Bytes(output_bytes) => {
                if let Err(err) = write_output(&job.output, output_bytes.as_ref()) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write compiled output: {err}")),
                        warnings: Vec::new(),
                    };
                }
                insert_daemon_cache_entries(_cache, cache_key, function_cache_key, output_bytes);
            }
            DaemonCompiledOutput::WrittenToPath => {
                if daemon_memory_cache_allowed {
                    maybe_cache_output_file(
                        _cache,
                        Path::new(&job.output),
                        cache_key,
                        function_cache_key,
                        &mut warnings,
                    );
                }
            }
        }

        DaemonJobResponse {
            id: job.id,
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
            warnings,
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
#[cfg(any(unix, test))]
fn write_cached_output(path: &str, bytes: &[u8], skip_if_synced: bool) -> io::Result<bool> {
    if skip_if_synced {
        return Ok(false);
    }
    write_output(path, bytes)?;
    Ok(true)
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
#[cfg(any(unix, test))]
fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    write_output_path(Path::new(path), bytes)
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
fn write_output_path(output_path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let base_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_name = format!(".{base_name}.{}.{}.tmp", std::process::id(), nonce);
    let tmp_path = output_path.with_file_name(tmp_name);
    let mut file = File::create(&tmp_path)?;
    file.write_all(bytes)?;
    drop(file);

    match std::fs::rename(&tmp_path, output_path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let _ = std::fs::remove_file(output_path);
            match std::fs::rename(&tmp_path, output_path) {
                Ok(()) => Ok(()),
                Err(second_err) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    Err(io::Error::new(
                        second_err.kind(),
                        format!(
                            "failed to atomically replace output (first: {first_err}; second: {second_err})"
                        ),
                    ))
                }
            }
        }
    }
}

#[cfg(unix)]
fn run_daemon(socket_path: &str) -> io::Result<()> {
    use std::os::unix::net::UnixListener;

    let socket = Path::new(socket_path);
    if socket.exists() {
        let _ = std::fs::remove_file(socket);
    }
    if let Some(parent) = socket.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket)?;
    let request_limit_bytes = daemon_request_limit_bytes();
    let max_jobs = daemon_max_jobs();
    let mut cache = DaemonCache::new(Some(daemon_cache_limit_bytes()));
    let mut stats = DaemonStats::default();
    let spawn_config_digest = env::var("MOLT_BACKEND_DAEMON_CONFIG_DIGEST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut active_config_digest: Option<String> = None;
    let started_at = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(
                    &mut conn,
                    DaemonConnectionContext {
                        cache: &mut cache,
                        stats: &mut stats,
                        spawn_config_digest: spawn_config_digest.as_deref(),
                        active_config_digest: &mut active_config_digest,
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    },
                ) {
                    eprintln!("backend daemon connection error: {err}");
                }
            }
            Err(err) => {
                eprintln!("backend daemon accept error: {err}");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
struct DaemonConnectionContext<'a> {
    cache: &'a mut DaemonCache,
    stats: &'a mut DaemonStats,
    spawn_config_digest: Option<&'a str>,
    active_config_digest: &'a mut Option<String>,
    started_at: Instant,
    request_limit_bytes: usize,
    max_jobs: usize,
}

#[cfg(unix)]
fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    ctx: DaemonConnectionContext<'_>,
) -> io::Result<()> {
    let DaemonConnectionContext {
        cache,
        stats,
        spawn_config_digest,
        active_config_digest,
        started_at,
        request_limit_bytes,
        max_jobs,
    } = ctx;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    loop {
        let raw_bytes = read_daemon_request_bytes(&mut reader, request_limit_bytes)?;
        if raw_bytes.is_empty() {
            return Ok(());
        }
        stats.requests_total = stats.requests_total.saturating_add(1);
        if raw_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty request".to_string()),
                health: None,
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let req = match DaemonRequest::from_json_bytes(&raw_bytes) {
            Ok(req) => req,
            Err(err) => {
                let response = DaemonResponse {
                    ok: false,
                    pong: false,
                    jobs: Vec::new(),
                    error: Some(format!("invalid request JSON: {err}")),
                    health: None,
                };
                write_daemon_response(stream, &response)?;
                continue;
            }
        };
        let include_health = req.include_health.unwrap_or(req.ping.unwrap_or(false));
        let version = req.version.unwrap_or(0);
        if version != BACKEND_DAEMON_PROTOCOL_VERSION {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "unsupported protocol version {version}; expected {BACKEND_DAEMON_PROTOCOL_VERSION}"
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if req.ping.unwrap_or(false) {
            let response = DaemonResponse {
                ok: true,
                pong: true,
                jobs: Vec::new(),
                error: None,
                health: Some(daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let request_config_digest = req
            .config_digest
            .as_deref()
            .map(str::trim)
            .filter(|digest| !digest.is_empty())
            .map(|digest| digest.to_string());
        if let Some(ref digest) = request_config_digest
            && active_config_digest.as_deref() != Some(digest.as_str())
        {
            cache.clear();
            *active_config_digest = Some(digest.clone());
        }
        let Some(jobs) = req.jobs else {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("missing jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        };
        if jobs.is_empty() {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if jobs.len() > max_jobs {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "too many jobs in request: {} exceeds daemon max_jobs {}",
                    jobs.len(),
                    max_jobs
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        stats.jobs_total = stats.jobs_total.saturating_add(jobs.len() as u64);
        let mut results = Vec::with_capacity(jobs.len());
        for job in jobs {
            let result = compile_single_job(job, cache);
            if result.ok && result.cached {
                stats.cache_hits = stats.cache_hits.saturating_add(1);
            } else {
                stats.cache_misses = stats.cache_misses.saturating_add(1);
            }
            results.push(result);
        }
        let all_ok = results.iter().all(|job| job.ok);
        let response = DaemonResponse {
            ok: all_ok,
            pong: false,
            jobs: results,
            error: None,
            health: include_health.then(|| {
                daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )
            }),
        };
        write_daemon_response(stream, &response)?;
    }
}

#[cfg(unix)]
fn read_daemon_request_bytes<R: BufRead>(
    reader: &mut R,
    request_limit_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let limit = u64::try_from(request_limit_bytes)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    reader.take(limit).read_until(b'\n', &mut raw_bytes)?;
    if raw_bytes.len() > request_limit_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("daemon request exceeded {request_limit_bytes} byte limit"),
        ));
    }
    Ok(raw_bytes)
}

#[cfg(unix)]
fn write_daemon_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DaemonResponse,
) -> io::Result<()> {
    let mut payload = daemon_response_payload(response)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(unix)]
fn daemon_response_payload(response: &DaemonResponse) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&response.to_json_value()).map_err(io::Error::other)
}

#[cfg(not(unix))]
fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}

const GIB: u64 = 1024 * 1024 * 1024;

fn default_backend_max_rss_gb_from_physical_mem_bytes(bytes: Option<u64>) -> u64 {
    match bytes.map(|raw| raw / GIB).unwrap_or(0) {
        gib if gib >= 64 => 16,
        gib if gib >= 32 => 12,
        gib if gib >= 16 => 8,
        _ => 4,
    }
}

#[cfg(unix)]
fn detect_physical_memory_bytes() -> Option<u64> {
    unsafe {
        let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        if pages <= 0 || page_size <= 0 {
            return None;
        }
        Some((pages as u64).saturating_mul(page_size as u64))
    }
}

#[cfg(windows)]
fn detect_physical_memory_bytes() -> Option<u64> {
    unsafe {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = core::mem::zeroed();
        status.dwLength = core::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return None;
        }
        Some(status.ullTotalPhys)
    }
}

#[cfg(not(any(unix, windows)))]
fn detect_physical_memory_bytes() -> Option<u64> {
    None
}

fn default_backend_max_rss_gb() -> u64 {
    default_backend_max_rss_gb_from_physical_mem_bytes(detect_physical_memory_bytes())
}

fn validate_fact_graph_cli_contract(
    output_path: Option<&str>,
    function_name: Option<&str>,
    is_rust: bool,
) -> io::Result<()> {
    if output_path.is_some() != function_name.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fact-graph-output and --fact-graph-function must be supplied together",
        ));
    }
    if output_path.is_some() && is_rust {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fact graph emission does not support the rust target",
        ));
    }
    Ok(())
}

#[allow(clippy::vec_init_then_push)] // pushes are behind #[cfg] feature gates
fn main() -> io::Result<()> {
    // TIR optimization is mandatory. Invalid roundtrips are fatal compiler
    // bugs and must be debugged through dumps/verifier evidence, not by
    // bypassing typed IR.

    // Hard memory guard: set rlimit on virtual memory to prevent OOM
    // from crashing the entire machine. The default scales with host memory
    // so large TIR-enabled stdlib builds do not trip an artificially tiny cap.
    #[cfg(unix)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            let rlim = libc::rlimit {
                rlim_cur: max_bytes,
                rlim_max: max_bytes,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                // Silently ignore on macOS (Apple Silicon). MOLT_DEBUG_RLIMIT=1 to warn.
                if std::env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1") {
                    eprintln!(
                        "WARNING: failed to set memory limit (RLIMIT_AS={max_gb}GB). OOM guard not active."
                    );
                }
            }
        }
    }

    // Windows memory guard: use job objects to limit working set.
    // Less effective than Unix RLIMIT_AS but prevents unbounded growth.
    #[cfg(windows)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            use windows_sys::Win32::System::Threading::*;
            let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = core::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = max_bytes as usize;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, GetCurrentProcess());
            }
        }
    }

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--features") {
        let features: &[&str] = &[
            #[cfg(feature = "native-backend")]
            "native-backend",
            #[cfg(feature = "luau-backend")]
            "luau-backend",
            #[cfg(feature = "wasm-backend")]
            "wasm-backend",
            #[cfg(feature = "rust-backend")]
            "rust-backend",
            #[cfg(feature = "cbor")]
            "cbor",
        ];
        if features.is_empty() {
            println!("molt-backend: no features enabled");
        } else {
            println!("molt-backend features: {}", features.join(", "));
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--daemon") {
        let socket_path = args
            .iter()
            .position(|arg| arg == "--socket")
            .and_then(|idx| args.get(idx + 1))
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
    let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());
    #[allow(unused_variables)]
    let use_ir_pipeline = args.contains(&"--ir-pipeline".to_string());
    #[cfg_attr(not(feature = "native-backend"), allow(unused_variables))]
    let target_triple = args
        .iter()
        .position(|arg| arg == "--target-triple")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let output_path = args
        .iter()
        .position(|arg| arg == "--output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    let native_batch_job_file = args
        .iter()
        .position(|arg| arg == "--native-batch-job-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    if let Some(job_file) = native_batch_job_file {
        let output_file = output_path.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--output is required with --native-batch-job-file",
            )
        })?;
        compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_file))?;
        return Ok(());
    }

    let ir_file_path = args
        .iter()
        .position(|arg| arg == "--ir-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_output_path = args
        .iter()
        .position(|arg| arg == "--fact-graph-output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_function = args
        .iter()
        .position(|arg| arg == "--fact-graph-function")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    validate_fact_graph_cli_contract(fact_graph_output_path, fact_graph_function, is_rust)?;

    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_link_flag = args.iter().any(|arg| arg == "--wasm-link");
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_data_base = args
        .iter()
        .position(|arg| arg == "--wasm-data-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_table_base = args
        .iter()
        .position(|arg| arg == "--wasm-table-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_split_runtime_runtime_table_min = args
        .iter()
        .position(|arg| arg == "--wasm-split-runtime-runtime-table-min")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());

    let ir_format = args
        .iter()
        .position(|arg| arg == "--ir-format")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str)
        .unwrap_or("json");

    // Read and parse IR.  Drop the raw buffer immediately after
    // deserialization to avoid holding two copies in memory simultaneously.
    let stdin_request_limit_bytes = stdin_request_limit_bytes();
    let mut ir: SimpleIR = {
        if ir_format == "msgpack" {
            // msgpack binary format — deserialize directly via serde
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match rmp_serde::from_read(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                // Streaming msgpack from stdin via BufReader — avoids
                // loading the entire IR into a Vec<u8> first.
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::with_capacity(1 << 20, bounded);
                match rmp_serde::from_read::<_, SimpleIR>(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if ir_format == "cbor" {
            // CBOR binary format — deserialize via ciborium
            #[cfg(not(feature = "cbor"))]
            {
                eprintln!("CBOR support requires the 'cbor' feature");
                std::process::exit(1);
            }
            #[cfg(feature = "cbor")]
            {
                if let Some(ir_path) = ir_file_path {
                    let file = std::fs::File::open(ir_path).map_err(|e| {
                        io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                    })?;
                    let reader = io::BufReader::new(file);
                    match ciborium::de::from_reader(reader) {
                        Ok(ir) => ir,
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    let buf = read_bounded_request_bytes(
                        io::stdin().lock(),
                        stdin_request_limit_bytes,
                        "backend stdin request",
                    )?;
                    match ciborium::de::from_reader::<SimpleIR, _>(&buf[..]) {
                        Ok(ir) => {
                            drop(buf);
                            ir
                        }
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        } else if ir_format == "ndjson" {
            // NDJSON streaming format — one function per line
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match SimpleIR::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::new(bounded);
                match SimpleIR::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if let Some(ir_path) = ir_file_path {
            // Stream JSON directly from file — never holds raw JSON string in memory.
            let file = std::fs::File::open(ir_path).map_err(|e| {
                io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
            })?;
            let reader = io::BufReader::with_capacity(1 << 20, file);
            match serde_json::from_reader::<_, SimpleIR>(reader) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            // Stdin: read into string then deserialize directly (skips DOM intermediate).
            let raw_bytes = read_bounded_request_bytes(
                io::stdin().lock(),
                stdin_request_limit_bytes,
                "backend stdin request",
            )?;
            let buffer = String::from_utf8(raw_bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("backend stdin request is not UTF-8: {err}"),
                )
            })?;
            let result = serde_json::from_str::<SimpleIR>(&buffer);
            drop(buffer);
            match result {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        }
    };

    rewrite_annotate_stubs(&mut ir);

    // Source emitters do not link against native/WASM runtime objects, so they
    // prune unreachable runtime/bootstrap support before textual codegen.
    if is_luau {
        ir.tree_shake_luau();
    }

    if let (Some(path), Some(function_name)) = (fact_graph_output_path, fact_graph_function) {
        let backend_setting = std::env::var("MOLT_BACKEND").ok();
        let target_info = if is_luau {
            molt_backend::tir::target_info::TargetInfo::luau_release_fast()
        } else if is_wasm {
            molt_backend::tir::target_info::TargetInfo::wasm_release_fast()
        } else if backend_setting.as_deref() == Some("llvm") {
            molt_backend::tir::target_info::TargetInfo::llvm_release_fast()
        } else {
            molt_backend::tir::target_info::TargetInfo::native_release_fast()
        };
        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: Path::new(path),
                function_name,
                target_info: &target_info,
            },
        )?;
        eprintln!("Wrote TIR fact graph for '{function_name}' to {path}");
        return Ok(());
    }

    // Luau module phase (Tier-2 E1 parity): source emission is still one
    // compilation unit, so every local body is owned by this module and the
    // inliner has no external-linkage exclusions. Keep Luau on the same
    // structural path as native/WASM: lift once to TIR, run every local
    // function through the per-function pipeline, then run the whole-module
    // pipeline (E1 inliner, generator fusion, module-slot promotion, terminal
    // DropInsertion) before one fail-closed back-conversion.
    if is_luau {
        let tir_start = Instant::now();
        let module_stats = run_luau_tir_module_pipeline(&mut ir)?;
        let tir_elapsed = tir_start.elapsed();
        eprintln!(
            "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
            module_stats.functions, module_stats.module_changed
        );
        molt_backend::eliminate_dead_ops(&mut ir);
    }

    let output_kind = if is_luau {
        BackendOutputKind::Luau
    } else if is_rust {
        BackendOutputKind::Rust
    } else if is_wasm {
        BackendOutputKind::Wasm
    } else {
        BackendOutputKind::Native
    };
    let output_file = resolve_backend_output_path(output_path, output_kind);
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;
    if is_luau {
        #[cfg(feature = "luau-backend")]
        {
            let mut backend = LuauBackend::new();
            let source = if use_ir_pipeline {
                backend.compile_via_ir(&ir)
            } else {
                backend.compile_checked(&ir)
            }
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Luau validation failed for '{}': {}", output_file, err),
                )
            })?;
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            file.write_all(source.as_bytes())?;
            let lines = source.lines().count();
            eprintln!(
                "Successfully transpiled to {output_file} ({lines} lines, {:.1} KB)",
                source.len() as f64 / 1024.0
            );
        }
        #[cfg(not(feature = "luau-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without luau-backend support; rebuild with: cargo build -p molt-backend --features luau-backend",
            ));
        }
    } else if is_rust {
        #[cfg(feature = "rust-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let source = rust_source_for_ir(&ir)?;
            file.write_all(source.as_bytes())?;
            println!("Successfully transpiled to {output_file}");
        }
        #[cfg(not(feature = "rust-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without rust-backend support; rebuild with: cargo build -p molt-backend --features rust-backend",
            ));
        }
    } else if is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let mut options = WasmCompileOptions::default();
            if wasm_link_flag {
                options.reloc_enabled = true;
            }
            if let Some(value) = wasm_data_base {
                options.data_base = value;
            }
            if let Some(value) = wasm_table_base {
                options.table_base = value;
            }
            if let Some(value) = wasm_split_runtime_runtime_table_min {
                options.split_runtime_runtime_table_min = Some(value);
            }
            let backend = WasmBackend::with_options(options);
            let wasm_bytes = backend.compile(ir);
            file.write_all(&wasm_bytes)?;
            println!("Successfully compiled to {output_file}");
        }
        #[cfg(not(feature = "wasm-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend",
            ));
        }
    } else {
        #[cfg(feature = "native-backend")]
        {
            // ── Incremental compilation ──
            // When MOLT_STDLIB_OBJ is set to a path, the backend caches
            // stdlib compilation: stdlib functions compile once to that path,
            // subsequent builds skip them entirely.  User functions always
            // recompile.  This reduces builds from ~5min to ~3sec.
            let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
            let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
            let expected_stdlib_cache_manifest = std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
            let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
            let entry_module =
                std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
            let explicit_stdlib_module_symbols = molt_backend::stdlib_module_symbols_from_env()
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;

            // Per-app intrinsic manifest over the FULL function set, computed
            // before partitioning splits the stdlib bodies into the cache object
            // (see the daemon path for the rationale). `None` in the non-split
            // case lets `compile` derive it from the full IR it holds. Every
            // manifest computed here feeds a resolver-emitting object; `_checked`
            // filters against the REQUIRED staticlib symbol set (fail-closed)
            // whenever any `molt_`-prefixed const_str exists, and yields the empty
            // manifest (zero-entry resolver, no relocations) otherwise.
            let app_intrinsic_manifest = stdlib_obj_path
                .as_ref()
                .map(|_| molt_backend::compute_intrinsic_manifest_checked(&ir.functions));

            if let Some(ref stdlib_path) = stdlib_obj_path {
                let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
                    &mut ir,
                    &entry_module,
                    explicit_stdlib_module_symbols.as_ref(),
                );
                let stdlib_path = std::path::Path::new(stdlib_path);
                // Ensure parent directory exists for stdlib cache path —
                // --rebuild may have cleared the build directory tree.
                ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(|e| {
                    eprintln!("MOLT_BACKEND: warning: failed to create stdlib parent: {e}");
                });
                let current_partition_manifest = shared_stdlib_partition_manifest(&stdlib_funcs)?;
                let split_function_names =
                    shared_stdlib_split_function_names(&user_remaining, &stdlib_funcs);
                validate_shared_stdlib_partition(&stdlib_funcs, &split_function_names)?;
                if have_entry_module && stdlib_path.exists() {
                    // Cached stdlib exists — only reuse it when the CLI and
                    // backend agree on the exact stdlib IR identity.
                    let current_stdlib_count = stdlib_funcs.len();
                    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
                    let cached_count: usize = std::fs::read_to_string(&count_path)
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                    let cached_key = read_stdlib_cache_key(stdlib_path);
                    let cached_manifest = read_stdlib_cache_manifest(stdlib_path);
                    if shared_stdlib_cache_matches(
                        stdlib_path,
                        expected_stdlib_cache_key.as_deref(),
                        expected_stdlib_cache_manifest.as_deref(),
                        Some(current_partition_manifest.as_str()),
                    ) {
                        // Cache exactly matches the requested stdlib IR — mark
                        // stdlib functions as extern stubs so the backend declares
                        // them as Import.  The linker resolves from stdlib_shared.o.
                        let mut retained = std::mem::take(&mut user_remaining);
                        let mut extern_count = 0usize;
                        for mut func in stdlib_funcs.drain(..) {
                            molt_backend::externalize_function_with_signature(&mut func);
                            extern_count += 1;
                            retained.push(func);
                        }
                        let user_count = retained.len().saturating_sub(extern_count);
                        ir.functions = retained;
                        eprintln!(
                            "MOLT_BACKEND: incremental — compiling {user_count} user functions ({extern_count} stdlib extern from {})",
                            stdlib_path.display()
                        );
                    } else {
                        // Cache is stale or from a different stdlib IR topology.
                        let cached_partition_manifest =
                            read_stdlib_cache_partition_manifest(stdlib_path);
                        eprintln!(
                            "MOLT_BACKEND: stdlib cache contract mismatch (cached key {}, expected key {}; cached manifest {}, expected manifest present {}; cached partition manifest present {}, expected partition manifest present true; cached {} functions, need {}) — rebuilding",
                            cached_key.as_deref().unwrap_or("<missing>"),
                            expected_stdlib_cache_key.as_deref().unwrap_or("<missing>"),
                            cached_manifest.as_deref().unwrap_or("<missing>"),
                            expected_stdlib_cache_manifest.is_some(),
                            cached_partition_manifest.is_some(),
                            cached_count,
                            current_stdlib_count,
                        );
                        remove_shared_stdlib_cache_artifacts(stdlib_path);
                    }
                } else {
                    // First build — compile stdlib separately, cache it
                    // Ensure the parent directory exists — it may have been
                    // cleaned by a prior backend rebuild (shutil.rmtree on
                    // the cache root) between cache-setup and compilation.
                    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(
                        |e| {
                            eprintln!("warning: could not create stdlib cache parent dir: {e}");
                        },
                    );
                    // Compile stdlib
                    eprintln!(
                        "MOLT_BACKEND: first build — caching {} stdlib functions to {}",
                        stdlib_funcs.len(),
                        stdlib_path.display()
                    );
                    let stdlib_count = stdlib_funcs.len();
                    let temp_stdlib_path = stdlib_cache_temp_publish_path(stdlib_path, "object");
                    compile_stdlib_cache_object(
                        &temp_stdlib_path,
                        std::mem::take(&mut stdlib_funcs),
                        ir.profile.clone(),
                        target_triple,
                        "MOLT_BACKEND",
                    )?;
                    publish_shared_stdlib_cache_object(
                        stdlib_path,
                        &temp_stdlib_path,
                        stdlib_count,
                        expected_stdlib_cache_key.as_deref(),
                        expected_stdlib_cache_manifest.as_deref(),
                        current_partition_manifest.as_str(),
                    )?;
                    // Now compile user functions only
                    ir.functions = std::mem::take(&mut user_remaining);
                    eprintln!(
                        "MOLT_BACKEND: compiling {} user functions",
                        ir.functions.len()
                    );
                }
            }

            compile_native_application_object_to_path(
                ir,
                Path::new(output_file),
                NativeApplicationObjectOptions {
                    target_triple,
                    stdlib_split_enabled: stdlib_obj_path.is_some(),
                    app_intrinsic_manifest,
                    log_prefix: "MOLT_BACKEND",
                },
            )?;
        }
        #[cfg(not(feature = "native-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "rust-backend")]
    use super::rust_source_for_ir;
    use super::{
        BACKEND_DAEMON_PROTOCOL_VERSION, BackendOutputKind, DEFAULT_BACKEND_BATCH_OP_BUDGET,
        DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE, DaemonCache, DaemonJobRequest,
        DaemonRequest, GIB, MIB, NativeApplicationObjectOptions, RequestBoundedRead,
        compile_native_application_object_to_path, compile_single_job, compile_stdlib_cache_object,
        create_backend_output_file, default_backend_max_rss_gb_from_physical_mem_bytes,
        default_backend_output_path, default_daemon_cache_bytes_from_physical_mem_bytes,
        ensure_output_parent_dir, is_user_owned_symbol, merge_relocatable_objects,
        partition_functions_for_batches, preserve_native_batch_worker_failure_artifacts,
        prune_and_partition_native_stdlib, read_bounded_request_bytes, read_json_artifact,
        read_stdlib_cache_key, read_stdlib_cache_manifest, relocatable_linker_binary,
        remove_native_batch_temp_dir, resolve_backend_output_path, resolved_batch_op_budget_limit,
        resolved_batch_size_limit, run_luau_tir_module_pipeline, shared_stdlib_cache_matches,
        shared_stdlib_partition_closure_issue, shared_stdlib_partition_manifest,
        stdlib_cache_count_sidecar_path, stdlib_cache_partition_manifest_sidecar_path,
        validate_fact_graph_cli_contract, validate_shared_stdlib_partition,
        with_shared_stdlib_cache_publish_lock, write_cached_output, write_json_artifact,
        write_shared_stdlib_cache_sidecars,
    };
    #[cfg(unix)]
    use super::{DaemonResponse, daemon_response_payload, read_daemon_request_bytes};
    use super::{NativeBatchModuleMetadata, NativeBatchObjectJob};
    use molt_backend::{FunctionIR, OpIR, SimpleIR};
    use std::io::{self, Cursor, Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());
    const SHARED_STDLIB_CACHE_ENV_KEYS: &[&str] = &[
        "MOLT_STDLIB_OBJ",
        "MOLT_STDLIB_CACHE_KEY",
        "MOLT_STDLIB_CACHE_MANIFEST",
    ];

    #[test]
    fn fact_graph_cli_contract_requires_output_and_function_pair() {
        let err = validate_fact_graph_cli_contract(Some("graph.json"), None, false)
            .expect_err("unpaired fact graph flags must fail closed");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .contains("--fact-graph-output and --fact-graph-function")
        );
    }

    #[test]
    fn fact_graph_cli_contract_rejects_rust_target() {
        let err = validate_fact_graph_cli_contract(Some("graph.json"), Some("molt_main"), true)
            .expect_err("rust target fact graph request must fail closed");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("rust target"));
    }

    struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        snapshot: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnvGuard {
        fn capture(keys: &'static [&'static str]) -> Self {
            let lock = ENV_TEST_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            Self {
                _lock: lock,
                snapshot,
            }
        }

        fn clear(keys: &'static [&'static str]) -> Self {
            let guard = Self::capture(keys);
            for (key, _) in &guard.snapshot {
                unsafe { std::env::remove_var(key) };
            }
            guard
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.snapshot {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn write_failing_relocatable_linker(tmp_dir: &std::path::Path) -> std::path::PathBuf {
        #[cfg(windows)]
        let linker = tmp_dir.join("fail-linker.cmd");
        #[cfg(not(windows))]
        let linker = tmp_dir.join("fail-linker.sh");

        #[cfg(windows)]
        std::fs::write(
            &linker,
            b"@echo off\r\necho forced relocatable link failure 1>&2\r\nexit /b 1\r\n",
        )
        .expect("write failing linker script");

        #[cfg(not(windows))]
        {
            std::fs::write(
                &linker,
                b"#!/bin/sh\necho forced relocatable link failure >&2\nexit 1\n",
            )
            .expect("write failing linker script");
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&linker)
                .expect("stat failing linker script")
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&linker, permissions)
                .expect("make failing linker script executable");
        }

        linker
    }

    #[test]
    fn daemon_cache_get_bytes_updates_lru_without_cloning() {
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1, 2, 3, 4]));

        let bytes = cache.get_bytes("module").expect("cache hit");
        assert_eq!(bytes, &[1, 2, 3, 4]);

        let entry = cache.entries.get("module").expect("entry retained");
        assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(entry.stamp, cache.clock);
    }

    #[test]
    fn daemon_cache_can_share_bytes_across_keys() {
        let mut cache = DaemonCache::new(None);
        let shared = Arc::<[u8]>::from(vec![9, 8, 7, 6]);
        cache.insert("module".to_string(), Arc::clone(&shared));
        cache.insert("function".to_string(), shared);

        let module = cache.entries.get("module").expect("module entry");
        let function = cache.entries.get("function").expect("function entry");
        assert!(Arc::ptr_eq(&module.bytes, &function.bytes));
    }

    #[test]
    fn luau_tir_module_pipeline_inlines_direct_local_calls() {
        let callee = FunctionIR {
            name: "luau_add1".to_string(),
            params: vec!["x".to_string()],
            param_types: Some(vec!["int".to_string()]),
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("one".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["x".to_string(), "one".to_string()]),
                    out: Some("sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
            ],
            source_file: None,
            is_extern: false,
        };
        let caller = FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            param_types: None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(41),
                    out: Some("arg".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("luau_add1".to_string()),
                    args: Some(vec!["arg".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            source_file: None,
            is_extern: false,
        };
        let mut ir = SimpleIR {
            functions: vec![caller, callee],
            profile: None,
        };

        let stats = run_luau_tir_module_pipeline(&mut ir).expect("luau module pipeline");

        assert_eq!(stats.functions, 2);
        assert!(
            stats.module_changed >= 1,
            "direct call inlining must report at least one changed function"
        );
        let main = ir
            .functions
            .iter()
            .find(|func| func.name == "molt_main")
            .expect("molt_main");
        assert!(
            main.ops
                .iter()
                .all(|op| !(op.kind == "call" && op.s_value.as_deref() == Some("luau_add1"))),
            "Luau module phase must inline direct local calls instead of leaving a call boundary: {:?}",
            main.ops
        );
    }

    #[test]
    fn daemon_native_path_written_output_skips_oversized_memory_cache() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tracked_env = [
            "MOLT_BACKEND_BATCH_SIZE",
            "MOLT_LINKER",
            "MOLT_STDLIB_OBJ",
            "MOLT_STDLIB_CACHE_KEY",
            "MOLT_STDLIB_CACHE_MANIFEST",
            "MOLT_STDLIB_MODULE_SYMBOLS",
            "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
            "MOLT_ENTRY_MODULE",
        ];
        let prior_env: Vec<_> = tracked_env
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect();
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
            std::env::set_var("MOLT_LINKER", "ld");
            std::env::remove_var("MOLT_STDLIB_OBJ");
            std::env::remove_var("MOLT_STDLIB_CACHE_KEY");
            std::env::remove_var("MOLT_STDLIB_CACHE_MANIFEST");
            std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS");
            std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS");
            std::env::remove_var("MOLT_ENTRY_MODULE");
        }

        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-native-cache-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let job = DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
            output: output.to_string_lossy().to_string(),
            cache_key: "module-cache".to_string(),
            function_cache_key: Some("function-cache".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: false,
            ir: Some(SimpleIR {
                functions: vec![
                    FunctionIR {
                        name: "molt_main".to_string(),
                        params: vec![],
                        ops: vec![
                            OpIR {
                                kind: "call".to_string(),
                                s_value: Some("helper".to_string()),
                                value: Some(0),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".to_string(),
                                ..OpIR::default()
                            },
                        ],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                    FunctionIR {
                        name: "helper".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                ],
                profile: None,
            }),
            ir_path: None,
        };
        let mut cache = DaemonCache::new(Some(1));

        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "path-written daemon output missing");
        assert!(
            cache.entries.is_empty(),
            "oversized object must not enter daemon memory cache"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("skipped daemon memory cache")),
            "missing cache-budget warning: {:?}",
            result.warnings
        );

        for (name, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn read_daemon_request_bytes_stops_at_protocol_newline() {
        let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
        let bytes = read_daemon_request_bytes(&mut cursor, 1024).expect("request bytes");
        assert_eq!(bytes, b"{\"version\":1}\n");
    }

    #[cfg(unix)]
    #[test]
    fn read_daemon_request_bytes_rejects_oversized_request() {
        let mut cursor = Cursor::new(b"{\"version\":1}\n".to_vec());
        let err = read_daemon_request_bytes(&mut cursor, 4).expect_err("oversized request");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("daemon request exceeded 4 byte limit")
        );
    }

    #[test]
    fn read_bounded_request_bytes_allows_exact_limit() {
        let cursor = Cursor::new(b"abcd".to_vec());
        let bytes =
            read_bounded_request_bytes(cursor, 4, "backend stdin request").expect("request bytes");
        assert_eq!(bytes, b"abcd");
    }

    #[test]
    fn read_bounded_request_bytes_rejects_oversized_request() {
        let cursor = Cursor::new(b"abcde".to_vec());
        let err = read_bounded_request_bytes(cursor, 4, "backend stdin request")
            .expect_err("oversized request");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("backend stdin request exceeded 4 byte limit")
        );
    }

    #[test]
    fn request_bounded_read_rejects_streaming_read_past_limit() {
        let cursor = Cursor::new(b"abcde".to_vec());
        let mut reader = RequestBoundedRead::new(cursor, 4, "streaming backend stdin request");
        let mut first_chunk = [0_u8; 4];
        assert_eq!(
            reader.read(&mut first_chunk).expect("first bounded read"),
            4
        );
        assert_eq!(&first_chunk, b"abcd");
        let mut probe = [0_u8; 1];
        let err = reader.read(&mut probe).expect_err("stream overflow");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("streaming backend stdin request exceeded 4 byte limit")
        );
    }

    #[test]
    fn daemon_default_cache_limit_scales_with_host_memory() {
        assert_eq!(
            default_daemon_cache_bytes_from_physical_mem_bytes(Some(8 * GIB)),
            128 * MIB
        );
        assert_eq!(
            default_daemon_cache_bytes_from_physical_mem_bytes(Some(128 * GIB)),
            2 * 1024 * MIB
        );
    }

    #[test]
    fn daemon_request_parse_applies_boolean_defaults() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(!job.wasm_link);
        assert_eq!(job.wasm_split_runtime_runtime_table_min, None);
        assert!(!job.skip_module_output_if_synced);
        assert!(!job.skip_function_output_if_synced);
        assert!(!job.probe_cache_only);
        assert!(job.ir.is_none());
        assert!(job.ir_path.is_none());
    }

    #[test]
    fn daemon_request_parse_accepts_path_backed_ir_lease() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module",
                        "ir_path": "/tmp/molt-ir.json"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(job.ir.is_none());
        assert_eq!(job.ir_path.as_deref(), Some("/tmp/molt-ir.json"));
    }

    #[test]
    fn daemon_request_parse_rejects_duplicate_ir_authority() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let err = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module",
                        "ir_path": "/tmp/molt-ir.json",
                        "ir": {"functions": []}
                    }
                ]
            }"#,
        )
        .expect_err("duplicate IR sources");

        assert!(
            err.contains("request.jobs[0] must use exactly one IR custody field: ir or ir_path")
        );
    }

    #[test]
    fn daemon_request_parse_reads_split_runtime_table_min() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": true,
                        "wasm_link": true,
                        "wasm_data_base": 1048576,
                        "wasm_table_base": 4096,
                        "wasm_split_runtime_runtime_table_min": 8192,
                        "output": "/tmp/out.wasm",
                        "cache_key": "module"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(job.wasm_link);
        assert_eq!(job.wasm_data_base, Some(1048576));
        assert_eq!(job.wasm_table_base, Some(4096));
        assert_eq!(job.wasm_split_runtime_runtime_table_min, Some(8192));
    }

    #[cfg(unix)]
    #[test]
    fn daemon_response_payload_omits_false_optional_fields() {
        let payload = daemon_response_payload(&DaemonResponse {
            ok: true,
            pong: false,
            jobs: vec![super::DaemonJobResponse {
                id: "job0".to_string(),
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: true,
                needs_ir: false,
                message: None,
                warnings: Vec::new(),
            }],
            error: None,
            health: None,
        })
        .expect("response payload");

        let text = String::from_utf8(payload).expect("utf8 json");
        assert!(!text.contains("\"needs_ir\""));
        assert!(!text.contains("\"health\""));
        assert!(!text.contains("\"error\""));
    }

    #[test]
    fn write_cached_output_can_skip_disk_write_when_synced() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-test-{nonce}.o"));

        let written = write_cached_output(output.to_str().expect("utf8 path"), b"artifact", true)
            .expect("cache hit succeeds");

        assert!(!written);
        assert!(!output.exists());
    }

    #[test]
    fn ensure_output_parent_dir_creates_nested_directories() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-parent-{nonce}"));
        let output = root.join("nested").join("cache").join("artifact.wasm");

        ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("parent dir creation");

        assert!(output.parent().expect("parent exists").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_backend_output_file_recreates_missing_parent() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-create-{nonce}"));
        let output = root.join("nested").join("cache").join("artifact.o");

        ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("prime parent");
        std::fs::remove_dir_all(root.join("nested")).expect("remove parent tree");

        let mut file =
            create_backend_output_file(output.to_str().expect("utf8 path")).expect("create file");
        file.write_all(b"artifact").expect("write artifact");
        drop(file);

        assert_eq!(std::fs::read(&output).expect("read artifact"), b"artifact");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_batch_temp_cleanup_reports_non_directory_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("molt-backend-cleanup-file-{nonce}"));
        std::fs::write(&path, b"not-a-directory").expect("write cleanup sentinel");

        let err = remove_native_batch_temp_dir(&path, "native batch cleanup test")
            .expect_err("file path must not be silently accepted as cleaned temp dir");

        assert!(
            err.to_string()
                .contains("failed to remove native batch cleanup test"),
            "unexpected cleanup error: {err}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_batch_failure_artifact_rewrites_context_path_for_replay() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_debug_artifact_dir = std::env::var("MOLT_DEBUG_ARTIFACT_DIR").ok();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt-batch-failure-artifact-test-{}-{nonce}",
            std::process::id()
        ));
        let source_dir = root.join("source");
        let debug_dir = root.join("debug");
        std::fs::create_dir_all(&source_dir).expect("create source batch dir");
        unsafe { std::env::set_var("MOLT_DEBUG_ARTIFACT_DIR", &debug_dir) };

        let module_context_path = source_dir.join("module_context.json");
        write_json_artifact(
            &module_context_path,
            &NativeBatchModuleMetadata {
                module_context: molt_backend::NativeBackendModuleContext::default(),
            },
        )
        .expect("write source module context");
        let job_path = source_dir.join("batch_7.json");
        write_json_artifact(
            &job_path,
            &NativeBatchObjectJob {
                ir: SimpleIR {
                    functions: vec![],
                    profile: None,
                },
                module_context_path: module_context_path.clone(),
                target_triple: None,
                emit_app_intrinsic_resolver: false,
                app_intrinsic_manifest: None,
                external_function_names: std::collections::BTreeSet::new(),
            },
        )
        .expect("write source job");
        let object_path = source_dir.join("batch_7.o");

        let artifact_dir = preserve_native_batch_worker_failure_artifacts(
            "native application batch worker",
            &job_path,
            &object_path,
        )
        .expect("preserve failed worker artifacts");
        std::fs::remove_dir_all(&source_dir).expect("source batch temp dir cleanup");

        let copied_job_path = artifact_dir.join("batch_7.json");
        let copied_job: NativeBatchObjectJob =
            read_json_artifact(&copied_job_path, "copied native batch job")
                .expect("read copied native batch job");
        assert_eq!(
            copied_job.module_context_path,
            artifact_dir.join("module_context.json")
        );
        assert!(
            copied_job.module_context_path.exists(),
            "copied module context must survive source cleanup"
        );
        assert!(
            artifact_dir.join("manifest.json").exists(),
            "artifact manifest must describe replay command"
        );

        match prior_debug_artifact_dir {
            Some(value) => unsafe { std::env::set_var("MOLT_DEBUG_ARTIFACT_DIR", value) },
            None => unsafe { std::env::remove_var("MOLT_DEBUG_ARTIFACT_DIR") },
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_backend_output_paths_use_dist_root() {
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Luau),
            "dist/output.luau"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Rust),
            "dist/output.rs"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Wasm),
            "dist/output.wasm"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Native),
            "dist/output.o"
        );
    }

    #[test]
    fn resolve_backend_output_path_prefers_explicit_output() {
        let explicit = "/tmp/custom/output.wasm";
        assert_eq!(
            resolve_backend_output_path(Some(explicit), BackendOutputKind::Wasm),
            explicit
        );
        assert_eq!(
            resolve_backend_output_path(None, BackendOutputKind::Wasm),
            "dist/output.wasm"
        );
    }

    #[cfg(feature = "rust-backend")]
    #[test]
    fn rust_source_for_ir_rejects_stub_markers() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "unsupported_for_rust_target_test".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let err = rust_source_for_ir(&ir).expect_err("Rust target must reject stub markers");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("unimplemented op stubs"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "rust-backend")]
    #[test]
    fn rust_source_for_ir_prunes_unreachable_stub_markers() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "dead_stdlib_helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "unsupported_for_rust_target_test".to_string(),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let source = rust_source_for_ir(&ir).expect("dead stubs must be pruned before Rust emit");
        assert!(source.contains("fn molt_main("));
        assert!(!source.contains("dead_stdlib_helper"));
        assert!(!source.contains("MOLT_STUB"));
    }

    #[test]
    fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
        let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: "/tmp/unused.o".to_string(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(!result.cached);
        assert!(result.needs_ir);
        assert!(!result.output_written);
    }

    #[test]
    fn daemon_probe_cache_only_hits_without_ir() {
        let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.o"));

        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(result.cached);
        assert!(!result.needs_ir);
        assert!(output.exists());
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn daemon_cache_hit_requires_matching_shared_stdlib_artifact() {
        let _env_guard = TestEnvGuard::capture(SHARED_STDLIB_CACHE_ENV_KEYS);

        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-stdlib-cache-{nonce}"));
        let output = root.join("probe-hit.o");
        let missing_stdlib = root.join("missing-stdlib.o");
        std::fs::create_dir_all(&root).expect("create temp dir");
        unsafe {
            std::env::set_var("MOLT_STDLIB_OBJ", &missing_stdlib);
            std::env::set_var("MOLT_STDLIB_CACHE_KEY", "stdlib-key");
            std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "stdlib-manifest");
        }

        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(!result.cached);
        assert!(result.needs_ir);
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn user_owned_symbol_partition_uses_explicit_stdlib_modules() {
        let stdlib_modules =
            std::collections::BTreeSet::from(["sys".to_string(), "json".to_string()]);

        assert!(is_user_owned_symbol(
            "molt_main",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_host_init",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "app__module",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init_app",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init___main__",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_isolate_import",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_isolate_bootstrap",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init_main_molt",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "main_molt__helper",
            "app",
            Some(&stdlib_modules)
        ));

        assert!(!is_user_owned_symbol(
            "molt_init_sys",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(!is_user_owned_symbol(
            "molt_init_json",
            "app",
            Some(&stdlib_modules)
        ));
    }

    #[test]
    fn backend_rss_default_scales_with_host_memory() {
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(8 * GIB)),
            4
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(16 * GIB)),
            8
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(32 * GIB)),
            12
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(64 * GIB)),
            16
        );
    }

    #[test]
    fn shared_stdlib_cache_requires_matching_key() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-cache-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_path = tmp_dir.join("stdlib.o");
        std::fs::write(&stdlib_path, b"placeholder").expect("write stdlib object");

        write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect("write sidecars");
        assert!(shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));
        assert!(shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            None,
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("def456"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"def456\"}"),
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-b"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            None,
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(&stdlib_path, None, None, None));

        std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(&stdlib_path))
            .expect("remove partition manifest");
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            None,
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));

        write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect("rewrite sidecars");
        std::fs::write(&stdlib_path, b"changed-object").expect("mutate object");
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn shared_stdlib_publish_lock_serializes_concurrent_threads() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-publish-lock-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_path = tmp_dir.join("stdlib.o");
        let first_inside = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let violation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel();
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel();

        let first_path = stdlib_path.clone();
        let first_inside_for_first = Arc::clone(&first_inside);
        let first_thread = std::thread::spawn(move || {
            with_shared_stdlib_cache_publish_lock(&first_path, || {
                first_inside_for_first.store(true, std::sync::atomic::Ordering::SeqCst);
                first_entered_tx.send(()).expect("signal first entered");
                std::thread::sleep(std::time::Duration::from_millis(150));
                first_inside_for_first.store(false, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .expect("first lock body");
        });

        first_entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("first thread entered lock");

        let second_path = stdlib_path.clone();
        let first_inside_for_second = Arc::clone(&first_inside);
        let violation_for_second = Arc::clone(&violation);
        let second_thread = std::thread::spawn(move || {
            with_shared_stdlib_cache_publish_lock(&second_path, || {
                if first_inside_for_second.load(std::sync::atomic::Ordering::SeqCst) {
                    violation_for_second.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                second_entered_tx.send(()).expect("signal second entered");
                Ok(())
            })
            .expect("second lock body");
        });

        assert!(
            second_entered_rx
                .recv_timeout(std::time::Duration::from_millis(40))
                .is_err(),
            "second publisher entered while the first publisher held the lock"
        );
        first_thread.join().expect("join first publisher");
        second_entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("second publisher eventually entered");
        second_thread.join().expect("join second publisher");
        assert!(
            !violation.load(std::sync::atomic::Ordering::SeqCst),
            "shared stdlib publish lock allowed overlapping writers"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn shared_stdlib_partition_manifest_tracks_names_and_bodies() {
        let func_a = FunctionIR {
            name: "molt_init_sys".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "return_none".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let func_b = FunctionIR {
            name: "sys__version".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "const_str".to_string(),
                s_value: Some("3.12".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let mut changed = func_b.clone();
        changed.ops[0].s_value = Some("3.13".to_string());

        let ordered = shared_stdlib_partition_manifest(&[func_a.clone(), func_b.clone()])
            .expect("partition manifest");
        let reordered = shared_stdlib_partition_manifest(&[func_b, func_a.clone()])
            .expect("partition manifest");
        let body_changed =
            shared_stdlib_partition_manifest(&[func_a, changed]).expect("partition manifest");

        assert_eq!(ordered, reordered);
        assert_ne!(ordered, body_changed);
        assert!(ordered.contains("\"molt_init_sys\""));
        assert!(ordered.contains("\"sys__version\""));
        assert!(ordered.contains("\"schema\":\"stdlib-partition-v1\""));
    }

    #[test]
    fn shared_stdlib_partition_rejects_unclosed_copy_reference() {
        let userdict_copy = FunctionIR {
            name: "collections__UserDict_copy".to_string(),
            params: vec!["self".to_string()],
            ops: vec![OpIR {
                kind: "call".to_string(),
                s_value: Some("copy__copy".to_string()),
                args: Some(vec!["self".to_string()]),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_init = FunctionIR {
            name: "molt_init_copy".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "call".to_string(),
                s_value: Some("copy__molt_module_chunk_1".to_string()),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_chunk = FunctionIR {
            name: "copy__molt_module_chunk_1".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_copy = FunctionIR {
            name: "copy__copy".to_string(),
            params: vec!["obj".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["obj".to_string()]),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let valid_partition = vec![
            userdict_copy.clone(),
            copy_init,
            copy_chunk,
            copy_copy.clone(),
        ];
        let valid_function_names: std::collections::BTreeSet<String> = valid_partition
            .iter()
            .map(|func| func.name.clone())
            .collect();
        validate_shared_stdlib_partition(&valid_partition, &valid_function_names)
            .expect("closed partition");

        let invalid_partition = vec![userdict_copy];
        let invalid_function_names: std::collections::BTreeSet<String> =
            ["collections__UserDict_copy", "copy__copy"]
                .into_iter()
                .map(str::to_string)
                .collect();
        let issue =
            shared_stdlib_partition_closure_issue(&invalid_partition, &invalid_function_names)
                .expect("missing copy reference");
        assert!(issue.contains("collections__UserDict_copy -> copy__copy"));
        assert!(
            validate_shared_stdlib_partition(&invalid_partition, &invalid_function_names).is_err()
        );
    }

    #[test]
    fn shared_stdlib_cache_sidecar_write_failures_propagate() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-cache-key-error-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let blocking = tmp_dir.join("not-a-dir");
        std::fs::write(&blocking, b"x").expect("write blocking file");
        let stdlib_path = blocking.join("stdlib.o");

        let err = write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect_err("sidecar writes should fail when parent is not a directory");
        assert!(!err.to_string().is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn partition_functions_for_batches_respects_op_budget() {
        let funcs = vec![
            FunctionIR {
                name: "a".to_string(),
                params: vec![],
                ops: vec![Default::default(); 90],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "b".to_string(),
                params: vec![],
                ops: vec![Default::default(); 90],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "c".to_string(),
                params: vec![],
                ops: vec![Default::default(); 10],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let batches = partition_functions_for_batches(funcs, 64, 100);
        let names: Vec<Vec<String>> = batches
            .into_iter()
            .map(|batch| batch.into_iter().map(|f| f.name).collect())
            .collect();

        assert_eq!(
            names,
            vec![
                vec!["a".to_string()],
                vec!["b".to_string(), "c".to_string()],
            ]
        );
    }

    #[test]
    fn partition_functions_for_batches_respects_count_budget() {
        let funcs = (0..5)
            .map(|idx| FunctionIR {
                name: format!("f{idx}"),
                params: vec![],
                ops: vec![Default::default(); 1],
                param_types: None,
                source_file: None,
                is_extern: false,
            })
            .collect();

        let batches = partition_functions_for_batches(funcs, 2, 1000);
        let sizes: Vec<usize> = batches.into_iter().map(|batch| batch.len()).collect();

        assert_eq!(sizes, vec![2, 2, 1]);
    }

    #[test]
    fn relocatable_linker_binary_prefers_override_then_env() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_molt_linker = std::env::var("MOLT_LINKER").ok();
        let prior_ld = std::env::var("LD").ok();
        let prior_cc = std::env::var("CC").ok();

        unsafe {
            std::env::set_var("MOLT_LINKER", "molt-ld");
            std::env::set_var("LD", "system-ld");
            std::env::set_var("CC", "clang");
        }
        assert_eq!(relocatable_linker_binary(Some("explicit")), "explicit");
        assert_eq!(relocatable_linker_binary(None), "molt-ld");

        unsafe {
            std::env::remove_var("MOLT_LINKER");
        }
        assert_eq!(relocatable_linker_binary(None), "system-ld");

        unsafe {
            std::env::remove_var("LD");
        }
        assert_eq!(relocatable_linker_binary(None), "clang");

        match prior_molt_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        match prior_ld {
            Some(value) => unsafe { std::env::set_var("LD", value) },
            None => unsafe { std::env::remove_var("LD") },
        }
        match prior_cc {
            Some(value) => unsafe { std::env::set_var("CC", value) },
            None => unsafe { std::env::remove_var("CC") },
        }
    }

    #[test]
    fn merge_relocatable_objects_copies_single_input() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-merge-reloc-single-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let input = tmp_dir.join("input.o");
        let output = tmp_dir.join("output.o");
        std::fs::write(&input, b"object-bytes").expect("write input object");

        merge_relocatable_objects(
            &output,
            std::slice::from_ref(&input),
            Some("linker-that-must-not-run"),
        )
        .expect("copy single input object");

        assert_eq!(
            std::fs::read(&output).expect("read merged output"),
            b"object-bytes"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn merge_relocatable_objects_reports_linker_failure() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-merge-reloc-fail-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let input_a = tmp_dir.join("a.o");
        let input_b = tmp_dir.join("b.o");
        let output = tmp_dir.join("output.o");
        std::fs::write(&input_a, b"a").expect("write first input object");
        std::fs::write(&input_b, b"b").expect("write second input object");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        let failing_linker_arg = failing_linker.to_string_lossy();

        let err = merge_relocatable_objects(
            &output,
            &[input_a.clone(), input_b.clone()],
            Some(failing_linker_arg.as_ref()),
        )
        .expect_err("merge should fail with failing linker");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn native_application_object_batches_cleanup_temp_dir_after_merge_failure() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_linker = std::env::var("MOLT_LINKER").ok();
        let temp_root = std::env::temp_dir();
        let batch_prefix = format!("molt_batch_{}_", std::process::id());
        let before: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
            .expect("read temp root before")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
            .collect();
        let tmp_dir = temp_root.join(format!(
            "molt-native-app-merge-fail-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
            std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
        }

        let output = tmp_dir.join("output.o");
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("helper".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let err = compile_native_application_object_to_path(
            ir,
            &output,
            NativeApplicationObjectOptions {
                target_triple: None,
                stdlib_split_enabled: false,
                app_intrinsic_manifest: None,
                log_prefix: "MOLT_BACKEND(test)",
            },
        )
        .expect_err("forced linker failure should propagate");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists(), "failed merge must not publish output");

        let after: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
            .expect("read temp root after")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
            .collect();
        assert_eq!(
            after, before,
            "batch temp dirs must be cleaned after failure"
        );

        match prior_batch_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn native_application_object_uses_op_budget_even_when_count_fits() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_op_budget = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();
        let prior_linker = std::env::var("MOLT_LINKER").ok();
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-native-app-op-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "64");
            std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "1");
            std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
        }

        let output = tmp_dir.join("output.o");
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("helper".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let err = compile_native_application_object_to_path(
            ir,
            &output,
            NativeApplicationObjectOptions {
                target_triple: None,
                stdlib_split_enabled: false,
                app_intrinsic_manifest: None,
                log_prefix: "MOLT_BACKEND(test)",
            },
        )
        .expect_err("op budget must force relocatable batching");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists(), "failed merge must not publish output");

        match prior_batch_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_op_budget {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
        }
        match prior_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn resolved_batch_size_and_op_budget_limits_default_and_zero_disable_caps() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_ops = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();

        unsafe {
            std::env::remove_var("MOLT_BACKEND_BATCH_SIZE");
            std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            DEFAULT_BACKEND_BATCH_SIZE
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            DEFAULT_STDLIB_BATCH_SIZE
        );
        assert_eq!(
            resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
            DEFAULT_BACKEND_BATCH_OP_BUDGET
        );

        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "0");
            std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "0");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            usize::MAX
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            usize::MAX
        );
        assert_eq!(
            resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
            usize::MAX
        );

        match prior_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_ops {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
        }
    }

    #[test]
    fn dead_function_elimination_prunes_stdlib_before_partition() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_init_sys".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_app".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "app__module".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(73),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_json".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(843),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        molt_backend::inject_runtime_exit(&mut ir);
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
        let retained: std::collections::BTreeSet<_> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();

        assert!(retained.contains("molt_main"));
        assert!(retained.contains("molt_init_sys"));
        assert!(!retained.contains("molt_init_json"));
    }

    #[test]
    fn prune_and_partition_native_stdlib_keeps_only_reachable_stdlib() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_init_sys".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_app".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "app__module".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(73),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_json".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(843),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut ir, "app", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();

        assert_eq!(user_names, vec!["molt_main"]);
        assert_eq!(stdlib_names, vec!["molt_init_sys"]);
    }

    #[test]
    fn prune_and_partition_native_stdlib_keeps_non_entry_user_module_in_user_partition() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let stdlib_modules = std::collections::BTreeSet::new();
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut ir, "__main__", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();

        assert_eq!(
            user_names,
            vec!["molt_main", "demo__module", "molt_isolate_import"]
        );
        assert!(stdlib_names.is_empty());
    }

    #[test]
    fn compile_stdlib_cache_object_emits_parseable_empty_object() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-empty-stdlib-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib = tmp_dir.join("empty-stdlib.o");

        compile_stdlib_cache_object(&stdlib, Vec::new(), None, None, "MOLT_BACKEND(test)")
            .expect("empty stdlib cache must emit an object");

        let bytes = std::fs::read(&stdlib).expect("read emitted empty stdlib object");
        assert!(
            !bytes.is_empty(),
            "empty stdlib cache path must publish a real object file"
        );
        cranelift_object::object::File::parse(&*bytes)
            .expect("empty stdlib cache must be a parseable object");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_empty_stdlib_partition_emits_cache_artifact_and_sidecars() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-empty-stdlib-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(&runtime_symbols, "molt_main\n").expect("write runtime symbols");

        let env_keys = [
            "MOLT_ENTRY_MODULE",
            "MOLT_STDLIB_OBJ",
            "MOLT_STDLIB_CACHE_KEY",
            "MOLT_STDLIB_CACHE_MANIFEST",
            "MOLT_STDLIB_MODULE_SYMBOLS",
            "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
        ];
        let prior_env: Vec<(&str, Option<String>)> = env_keys
            .iter()
            .copied()
            .map(|key| (key, std::env::var(key).ok()))
            .collect();
        unsafe {
            std::env::set_var("MOLT_ENTRY_MODULE", "demo");
            std::env::set_var("MOLT_STDLIB_OBJ", &stdlib);
            std::env::set_var("MOLT_STDLIB_CACHE_KEY", "daemon-empty-key");
            std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "daemon-empty-manifest");
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
            std::env::set_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS", &runtime_symbols);
        }

        let job = DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
            output: output.to_string_lossy().into_owned(),
            cache_key: "".to_string(),
            function_cache_key: None,
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: false,
            ir: Some(SimpleIR {
                functions: vec![
                    FunctionIR {
                        name: "molt_main".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "call".to_string(),
                            s_value: Some("demo__module".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                    FunctionIR {
                        name: "demo__module".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                    FunctionIR {
                        name: "molt_isolate_bootstrap".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                    FunctionIR {
                        name: "molt_isolate_import".to_string(),
                        params: vec!["p0".to_string()],
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                ],
                profile: None,
            }),
            ir_path: None,
        };

        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        for (key, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "application output object missing");
        let stdlib_bytes = std::fs::read(&stdlib).expect("read daemon empty stdlib object");
        assert!(
            !stdlib_bytes.is_empty(),
            "daemon empty stdlib cache must publish a real object"
        );
        cranelift_object::object::File::parse(&*stdlib_bytes)
            .expect("daemon empty stdlib cache must be a parseable object");
        assert_eq!(
            std::fs::read_to_string(stdlib_cache_count_sidecar_path(&stdlib))
                .expect("read stdlib count sidecar"),
            "0"
        );
        assert_eq!(
            read_stdlib_cache_key(&stdlib).as_deref(),
            Some("daemon-empty-key")
        );
        assert_eq!(
            read_stdlib_cache_manifest(&stdlib).as_deref(),
            Some("daemon-empty-manifest")
        );
        let partition_manifest =
            std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(&stdlib))
                .expect("read stdlib partition manifest");
        assert!(partition_manifest.contains("\"functions\":[]"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_native_without_stdlib_obj_keeps_full_ir() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_bootstrap".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
        let entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
        unsafe {
            std::env::remove_var("MOLT_STDLIB_OBJ");
            std::env::remove_var("MOLT_ENTRY_MODULE");
        }

        // Mirror the daemon native path: without a stdlib cache target,
        // it must compile the full IR, not the drained remainder.
        let maybe_stdlib = std::env::var("MOLT_STDLIB_OBJ").ok();
        if maybe_stdlib.is_none() {
            molt_backend::inject_runtime_exit(&mut ir);
            molt_backend::eliminate_dead_functions(&mut ir);
            molt_backend::eliminate_dead_imports(&mut ir);
            molt_backend::eliminate_dead_ops(&mut ir);
        }

        let names: Vec<_> = ir.functions.iter().map(|func| func.name.as_str()).collect();

        match stdlib_obj_path {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
        }
        match entry_module {
            Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
            None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
        }

        assert_eq!(
            names,
            vec![
                "molt_main",
                "demo__module",
                "molt_isolate_bootstrap",
                "molt_isolate_import"
            ]
        );
    }

    #[test]
    fn daemon_request_with_env_preserves_user_entry_object() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-request-env-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        // The main application object emits the per-app intrinsic resolver, which
        // requires the linked runtime staticlib's `molt_*` intrinsic-symbol set
        // (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Production always extracts and
        // exposes this before native codegen; replicate that precondition through
        // the daemon's env-passthrough so this test exercises the real resolver
        // path instead of hitting the fail-closed guard. These IR functions take
        // no intrinsic addresses (no `const_str` ops), so the resolved manifest is
        // empty regardless; the file just satisfies the required-symbol-set
        // contract with the symbols this object actually references.
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(&runtime_symbols, "molt_init_sys\nmolt_main\n")
            .expect("write runtime intrinsic symbol set");
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
                "MOLT_RUNTIME_INTRINSIC_SYMBOLS": runtime_symbols.to_string_lossy(),
            },
            "jobs": [{
                "id": "job0",
                "is_wasm": false,
                "output": output.to_string_lossy(),
                "cache_key": "",
                "function_cache_key": "",
                "ir": {
                    "functions": [
                        {"name": "molt_main", "params": [], "ops": [{"kind": "call", "s_value": "demo__module", "value": 0}]},
                        {"name": "demo__module", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}, {"kind": "ret_void"}]},
                        {"name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }
            }]
        });

        let request = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");
        assert_eq!(
            std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
            Some("demo")
        );
        assert_eq!(
            std::env::var("MOLT_STDLIB_OBJ").ok().as_deref(),
            Some(stdlib.to_string_lossy().as_ref())
        );
        assert_eq!(
            std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok().as_deref(),
            Some("[\"sys\"]")
        );
        let job = request.jobs.expect("jobs").into_iter().next().expect("job");
        let mut partition_ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("molt_init_sys".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_bootstrap".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };
        let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut partition_ir, "demo", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
        assert_eq!(
            user_names,
            vec![
                "molt_main",
                "demo__module",
                "molt_isolate_bootstrap",
                "molt_isolate_import"
            ]
        );
        assert_eq!(stdlib_names, vec!["molt_init_sys"]);
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "output object missing");
        assert!(
            output.metadata().expect("output metadata").len() > 240,
            "daemon path emitted empty object"
        );

        // The daemon env-passthrough mutated the process environment; clear the
        // resolver symbol-set var so it does not leak into sibling tests that
        // share `ENV_TEST_MUTEX`.
        unsafe { std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS") };
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_request_env_clears_omitted_stdlib_module_symbols() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
            std::env::set_var("MOLT_ENTRY_MODULE", "stale_entry");
        }

        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-clear-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
            },
            "jobs": [],
        });

        let parsed = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");

        assert_eq!(parsed.version, Some(BACKEND_DAEMON_PROTOCOL_VERSION));
        assert_eq!(
            std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
            Some("demo")
        );
        assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
    }

    #[test]
    fn daemon_request_env_clears_omitted_resource_and_trace_keys_between_requests() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let keys = [
            "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
            "MOLT_CLI_MEMORY_AVAILABLE_GB",
            "MOLT_CLI_MEM_AVAILABLE_GB",
            "MOLT_MEMORY_AVAILABLE_GB",
            "MOLT_MEM_AVAILABLE_GB",
            "MOLT_BACKEND_MAX_RSS_GB",
            "MOLT_BACKEND_MEMORY_RESERVE_GB",
            "MOLT_CLI_MEMORY_RESERVE_GB",
            "MOLT_CLI_MEM_RESERVE_GB",
            "MOLT_MEMORY_RESERVE_GB",
            "MOLT_MEM_RESERVE_GB",
            "RAYON_NUM_THREADS",
            "MOLT_TIR_TRACE_FUNC",
        ];
        let prior_env: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        unsafe {
            for key in keys {
                std::env::set_var(key, "stale");
            }
        }

        let first = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-resource-env-set",
            "env": {
                "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "9",
                "MOLT_BACKEND_MEMORY_RESERVE_GB": "1",
                "RAYON_NUM_THREADS": "3",
                "MOLT_TIR_TRACE_FUNC": "target_func",
            },
            "jobs": [],
        });
        DaemonRequest::from_json_bytes(
            serde_json::to_string(&first)
                .expect("serialize first request")
                .as_bytes(),
        )
        .expect("parse first daemon request");
        assert_eq!(
            std::env::var("MOLT_BACKEND_MEMORY_AVAILABLE_GB")
                .ok()
                .as_deref(),
            Some("9")
        );
        assert_eq!(
            std::env::var("MOLT_BACKEND_MEMORY_RESERVE_GB")
                .ok()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            std::env::var("RAYON_NUM_THREADS").ok().as_deref(),
            Some("3")
        );
        assert_eq!(
            std::env::var("MOLT_TIR_TRACE_FUNC").ok().as_deref(),
            Some("target_func")
        );

        let second = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-resource-env-clear",
            "env": {},
            "jobs": [],
        });
        DaemonRequest::from_json_bytes(
            serde_json::to_string(&second)
                .expect("serialize second request")
                .as_bytes(),
        )
        .expect("parse second daemon request");

        for key in keys {
            assert!(
                std::env::var(key).is_err(),
                "{key} leaked across daemon requests"
            );
        }
        for (key, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

    #[test]
    fn daemon_request_env_rejects_malformed_stdlib_module_symbols() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
        }
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-bad-stdlib-symbols-test",
            "env": {
                "MOLT_STDLIB_MODULE_SYMBOLS": "not-json",
            },
            "jobs": [],
        });

        let err = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect_err("malformed stdlib symbol authority must fail closed");
        assert!(
            err.contains("MOLT_STDLIB_MODULE_SYMBOLS must be a JSON array of strings"),
            "unexpected error message: {err}"
        );
        assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
    }

    #[test]
    fn daemon_batch_compile_keeps_user_module_chunk_stub_defined() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-batch-chunk-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        // The main application object emits the per-app intrinsic resolver, which
        // requires the linked runtime staticlib's `molt_*` intrinsic-symbol set
        // (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Production always extracts and
        // exposes this before native codegen; replicate that precondition through
        // the daemon's env-passthrough so this test exercises the real resolver
        // path instead of hitting the fail-closed guard. These IR functions take
        // no intrinsic addresses (no `const_str` ops), so the resolved manifest is
        // empty regardless; the file just satisfies the required-symbol-set
        // contract with the symbols this object actually references.
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(
            &runtime_symbols,
            "molt_init_sys\nmolt_init_demo\nmolt_main\nmolt_host_init\n",
        )
        .expect("write runtime intrinsic symbol set");
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
                "MOLT_RUNTIME_INTRINSIC_SYMBOLS": runtime_symbols.to_string_lossy(),
                "MOLT_BACKEND_BATCH_SIZE": "1",
            },
            "jobs": [{
                "id": "job0",
                "is_wasm": false,
                "output": output.to_string_lossy(),
                "cache_key": "",
                "function_cache_key": "",
                "ir": {
                    "functions": [
                        {"name": "molt_main", "params": [], "ops": [
                            {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_host_init", "params": [], "ops": [
                            {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_init_demo", "params": [], "ops": [
                            {"kind": "call", "s_value": "demo__molt_module_chunk_1", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "demo__molt_module_chunk_1", "params": [], "ops": [
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }
            }]
        });

        let request = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");
        let job = request.jobs.expect("jobs").into_iter().next().expect("job");
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "output object missing");

        let nm_output = std::process::Command::new("nm")
            .args(["-g", output.to_str().expect("utf8 output path")])
            .output()
            .expect("run nm");
        assert!(
            nm_output.status.success(),
            "nm failed: {}",
            String::from_utf8_lossy(&nm_output.stderr)
        );
        let text = String::from_utf8_lossy(&nm_output.stdout);
        let has_defined_chunk = text.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 2 {
                return false;
            }
            let sym = fields
                .last()
                .copied()
                .unwrap_or_default()
                .trim_start_matches('_');
            sym == "demo__molt_module_chunk_1" && fields[fields.len().saturating_sub(2)] == "T"
        });
        let has_undefined_chunk = text.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() != 2 {
                return false;
            }
            let sym = fields
                .last()
                .copied()
                .unwrap_or_default()
                .trim_start_matches('_');
            sym == "demo__molt_module_chunk_1" && fields[0] == "U"
        });

        assert!(has_defined_chunk, "expected defined chunk symbol:\n{text}");
        assert!(
            !has_undefined_chunk,
            "unexpected undefined chunk symbol:\n{text}"
        );

        // The daemon env-passthrough mutated the process environment; clear the
        // resolver symbol-set var so it does not leak into sibling tests that
        // share `ENV_TEST_MUTEX`.
        unsafe { std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS") };
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn batch_external_function_names_excludes_current_batch_symbols() {
        let all_names = std::collections::BTreeSet::from([
            "molt_main".to_string(),
            "demo__module".to_string(),
            "molt_isolate_bootstrap".to_string(),
            "molt_isolate_import".to_string(),
        ]);
        let batch_funcs = vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "demo__module".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let external_names = super::batch_external_function_names(&all_names, &batch_funcs);

        assert_eq!(
            external_names,
            std::collections::BTreeSet::from([
                "molt_isolate_bootstrap".to_string(),
                "molt_isolate_import".to_string(),
            ])
        );
        assert!(!external_names.contains("molt_main"));
        assert!(!external_names.contains("demo__module"));
    }
}
