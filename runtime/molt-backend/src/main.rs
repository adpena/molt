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
use serde_json::Value as JsonValue;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
use std::io::BufRead;
use std::io::Write;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

mod json_boundary;

use crate::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};

const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
const DEFAULT_BACKEND_BATCH_SIZE: usize = 64;
const DEFAULT_STDLIB_BATCH_SIZE: usize = 128;
const DEFAULT_STDLIB_BATCH_OP_BUDGET: usize = 12_000;
const DAEMON_REQUEST_ENV_KEYS: &[&str] = &[
    "MOLT_TIR_OPT",
    "MOLT_DISABLE_DEAD_FUNC_ELIM",
    "MOLT_BACKEND_BATCH_SIZE",
    "MOLT_BACKEND_BATCH_OP_BUDGET",
    "MOLT_MAX_FUNCTION_OPS",
    "MOLT_DISABLE_RC_COALESCING",
    "TIR_DUMP",
    "TIR_OPT_STATS",
    "MOLT_DUMP_CLIF",
    "MOLT_DUMP_CLIF_ON_ERROR",
    "MOLT_DUMP_FINAL_FUNC_IR",
    "MOLT_DUMP_IR",
    "MOLT_DEBUG_BIND",
    "MOLT_BACKEND",
    "MOLT_DEBUG_CHECK_EXC",
    "MOLT_DEBUG_CHECK_EXCEPTION",
    "MOLT_LLVM_DUMP_IR",
    "MOLT_BACKEND_TIMING",
    "MOLT_TIR_NO_TYPES",
    "MOLT_ENTRY_MODULE",
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_MODULE_SYMBOLS",
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BackendOutputKind {
    Luau,
    Rust,
    Wasm,
    Native,
}

#[derive(Debug)]
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
}

#[derive(Debug)]
struct DaemonRequest {
    version: Option<u32>,
    ping: Option<bool>,
    include_health: Option<bool>,
    config_digest: Option<String>,
    jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug)]
struct DaemonJobResponse {
    id: String,
    ok: bool,
    cached: bool,
    cache_tier: Option<String>,
    output_written: bool,
    needs_ir: bool,
    message: Option<String>,
}

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

fn resolved_batch_size_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
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
fn compile_stdlib_cache_object(
    stdlib_path: &Path,
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
) -> io::Result<()> {
    let stdlib_count = stdlib_funcs.len();
    if stdlib_count == 0 {
        return Ok(());
    }

    let stdlib_batch_size = resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE);
    let stdlib_batch_ops_budget: usize = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_STDLIB_BATCH_OP_BUDGET);
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
        let stdlib_bytes = stdlib_backend.compile(stdlib_ir);
        std::fs::write(stdlib_path, &stdlib_bytes)?;
        return Ok(());
    }

    let stdlib_tmp_dir =
        std::env::temp_dir().join(format!("molt_stdlib_batch_{}", std::process::id()));
    std::fs::create_dir_all(&stdlib_tmp_dir)?;
    let mut stdlib_batch_paths: Vec<std::path::PathBuf> = Vec::new();
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
            let mut batch_backend = SimpleBackend::new_with_target(target_triple);
            batch_backend.skip_ir_passes = true;
            batch_backend.skip_shared_stdlib_partition = true;
            batch_backend.external_function_names =
                batch_external_function_names(&all_stdlib_names, &batch_ir.functions);
            batch_backend.set_module_context(stdlib_module_context.clone());
            let batch_bytes = batch_backend.compile(batch_ir);
            let batch_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.o"));
            std::fs::write(&batch_path, &batch_bytes)?;
            stdlib_batch_paths.push(batch_path);
        }

        merge_relocatable_objects(stdlib_path, &stdlib_batch_paths, None)
    })();

    for batch_path in &stdlib_batch_paths {
        let _ = std::fs::remove_file(batch_path);
    }
    let _ = std::fs::remove_dir(&stdlib_tmp_dir);

    compile_result
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
fn explicit_stdlib_module_symbols_from_env() -> Option<std::collections::BTreeSet<String>> {
    let raw = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok()?;
    let parsed: Vec<String> = serde_json::from_str(&raw).ok()?;
    Some(parsed.into_iter().collect())
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
    molt_backend::eliminate_dead_functions(ir);
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
fn write_atomic_text_file(path: &Path, contents: &str) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let temp_path = stdlib_cache_temp_publish_path(path, "text");
    std::fs::write(&temp_path, contents)?;
    if let Err(err) = atomic_replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
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

#[cfg(any(not(feature = "native-backend"), not(unix)))]
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
fn shared_stdlib_cache_matches(stdlib_path: &Path, expected_key: Option<&str>) -> bool {
    let Some(expected_key) = expected_key.filter(|key| !key.is_empty()) else {
        return false;
    };
    read_stdlib_cache_key(stdlib_path).as_deref() == Some(expected_key)
}

#[cfg(feature = "native-backend")]
fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
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
    Ok(())
}

#[cfg(feature = "native-backend")]
fn publish_shared_stdlib_cache_object(
    stdlib_path: &Path,
    temp_object_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
) -> io::Result<()> {
    let result = with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        write_shared_stdlib_cache_sidecars(stdlib_path, stdlib_count, cache_key)?;
        if let Err(err) = atomic_replace_file(temp_object_path, stdlib_path) {
            let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
            let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
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
struct DaemonHealthResponse {
    protocol_version: u32,
    pid: u32,
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
struct DaemonResponse {
    ok: bool,
    pong: bool,
    jobs: Vec<DaemonJobResponse>,
    error: Option<String>,
    health: Option<DaemonHealthResponse>,
}

impl DaemonJobRequest {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
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
        })
    }
}

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
        // MOLT_TIR_OPT, MOLT_DISABLE_DEAD_FUNC_ELIM, etc. without
        // restarting the daemon.
        for key in DAEMON_REQUEST_ENV_KEYS {
            unsafe {
                std::env::remove_var(key);
            }
        }
        if let Some(JsonValue::Object(env_map)) = obj.get("env") {
            for (key, val) in env_map {
                if let Some(s) = val.as_str() {
                    unsafe {
                        std::env::set_var(key, s);
                    }
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
        JsonValue::Object(obj)
    }
}

impl DaemonHealthResponse {
    fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "protocol_version".to_string(),
            JsonValue::from(self.protocol_version),
        );
        obj.insert("pid".to_string(), JsonValue::from(self.pid));
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
struct DaemonStats {
    requests_total: u64,
    jobs_total: u64,
    cache_hits: u64,
    cache_misses: u64,
}

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

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
struct CacheEntry {
    bytes: Arc<[u8]>,
    stamp: u64,
}

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

fn daemon_health(cache: &DaemonCache, stats: &DaemonStats, start: Instant) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes: None,
        max_jobs: None,
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}

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
        eprintln!("DEBUG_CACHE: cache_key={:?} function_cache_key={:?} output={:?}", cache_key, function_cache_key, &job.output);
        if !cache_key.is_empty()
            && let Some(bytes) = _cache.get_bytes(cache_key)
        {
            eprintln!("DEBUG_CACHE: HIT module cache for key={:?}", cache_key);
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
                    };
                }
            }
        }
        if !function_cache_key.is_empty()
            && function_cache_key != cache_key
            && let Some(bytes) = _cache.get_bytes(function_cache_key)
        {
            eprintln!("DEBUG_CACHE: HIT function cache for key={:?}", function_cache_key);
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
            };
        }

        let Some(mut ir) = job.ir else {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some("missing ir for cache miss".to_string()),
            };
        };

        let output_bytes: Arc<[u8]> = if job.is_wasm {
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
                Arc::from(backend.compile(ir))
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
                let entry_module =
                    std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
                let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
                let explicit_stdlib_module_symbols = explicit_stdlib_module_symbols_from_env();

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

                    if have_entry_module && stdlib_path.exists() {
                        if !shared_stdlib_cache_matches(
                            stdlib_path,
                            expected_stdlib_cache_key.as_deref(),
                        ) {
                            let cached_key = read_stdlib_cache_key(stdlib_path);
                            eprintln!(
                                "MOLT_BACKEND(daemon): stdlib cache key mismatch \
                                 (cached key {}, expected key {}) — honoring explicit stdlib object {}",
                                cached_key.as_deref().unwrap_or("<missing>"),
                                expected_stdlib_cache_key.as_deref().unwrap_or("<missing>"),
                                stdlib_path.display()
                            );
                        }
                        let mut retained = std::mem::take(&mut user_remaining);
                        let mut extern_count = 0usize;
                        for mut func in std::mem::take(&mut stdlib_funcs) {
                            func.is_extern = true;
                            func.ops.clear();
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

                    if !stdlib_path.exists()
                        && (!user_remaining.is_empty() || !stdlib_funcs.is_empty())
                    {
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
                        if stdlib_count > 0 {
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
                                };
                            }
                            if let Err(err) = publish_shared_stdlib_cache_object(
                                stdlib_path,
                                &temp_stdlib_path,
                                stdlib_count,
                                expected_stdlib_cache_key.as_deref(),
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
                                };
                            }
                        }

                        ir.functions = std::mem::take(&mut user_remaining);
                        eprintln!(
                            "MOLT_BACKEND(daemon): compiling {} user functions",
                            ir.functions.len()
                        );
                    }
                }

                let mut backend = SimpleBackend::new_with_target(target_triple);
                if stdlib_obj_path.is_some() {
                    backend.skip_shared_stdlib_partition = true;
                }
                Arc::from(backend.compile(ir))
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
                };
            }
        };

        if let Err(err) = write_output(&job.output, output_bytes.as_ref()) {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some(format!("failed to write compiled output: {err}")),
            };
        }

        if !cache_key.is_empty()
            && !function_cache_key.is_empty()
            && function_cache_key != cache_key
        {
            eprintln!("DEBUG_CACHE: INSERT cache_key={:?} function_cache_key={:?}", cache_key, function_cache_key);
            _cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
            _cache.insert(function_cache_key.to_string(), output_bytes);
        } else if !cache_key.is_empty() {
            _cache.insert(cache_key.to_string(), output_bytes);
        } else if !function_cache_key.is_empty() {
            _cache.insert(function_cache_key.to_string(), output_bytes);
        }

        DaemonJobResponse {
            id: job.id,
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
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
fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    let output_path = Path::new(path);
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
    let mut cache = DaemonCache::new(None);
    let mut stats = DaemonStats::default();
    let mut active_config_digest: Option<String> = None;
    let started_at = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(
                    &mut conn,
                    &mut cache,
                    &mut stats,
                    &mut active_config_digest,
                    started_at,
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
fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    cache: &mut DaemonCache,
    stats: &mut DaemonStats,
    active_config_digest: &mut Option<String>,
    started_at: Instant,
) -> io::Result<()> {
    loop {
        let raw_bytes = read_daemon_request_bytes(stream)?;
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
                health: Some(daemon_health(cache, stats, started_at)),
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
                health: include_health.then(|| daemon_health(cache, stats, started_at)),
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
            health: include_health.then(|| daemon_health(cache, stats, started_at)),
        };
        write_daemon_response(stream, &response)?;
    }
}

#[cfg(unix)]
fn read_daemon_request_bytes<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let mut buffered = io::BufReader::new(reader);
    buffered.read_until(b'\n', &mut raw_bytes)?;
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

#[allow(clippy::vec_init_then_push)] // pushes are behind #[cfg] feature gates
fn main() -> io::Result<()> {
    // TIR optimization is ON by default. Invalid roundtrips are fatal
    // compiler bugs; disable with MOLT_TIR_OPT=0 only for targeted bisects.

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
            if job != 0 {
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
        let mut features = Vec::new();
        #[cfg(feature = "native-backend")]
        features.push("native-backend");
        #[cfg(feature = "luau-backend")]
        features.push("luau-backend");
        #[cfg(feature = "wasm-backend")]
        features.push("wasm-backend");
        #[cfg(feature = "rust-backend")]
        features.push("rust-backend");
        #[cfg(feature = "cbor")]
        features.push("cbor");
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

    let ir_file_path = args
        .iter()
        .position(|arg| arg == "--ir-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);

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
                let reader = io::BufReader::with_capacity(1 << 20, io::stdin().lock());
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
                    let mut buf = Vec::new();
                    io::stdin().read_to_end(&mut buf)?;
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
                let reader = io::BufReader::new(io::stdin());
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
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer)?;
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

    // Tree-shake for Luau: remove unreachable stdlib functions.
    if is_luau {
        ir.tree_shake_luau();
    }

    // Run the full TIR optimization pipeline for Luau — same 14 passes that
    // native and WASM backends get (refine_types → run_pipeline → refine_types).
    // Without this, the Luau backend was operating on unoptimized SimpleIR,
    // missing unboxing, escape analysis, SCCP, strength reduction, BCE, DCE etc.
    if is_luau {
        let tir_start = Instant::now();
        let mut tir_count = 0usize;
        for func in &mut ir.functions {
            // Skip tiny functions and annotation stubs.
            if func.ops.len() < 4 || func.name.contains("__annotate__") {
                continue;
            }
            if func.ops.iter().any(|op| op.kind == "phi") {
                molt_backend::rewrite_phi_to_store_load(&mut func.ops);
            }
            let mut tir_func =
                molt_backend::tir::lower_from_simple::lower_to_tir(func);
            molt_backend::tir::type_refine::refine_types(&mut tir_func);
            let _stats = molt_backend::tir::passes::run_pipeline(&mut tir_func);
            molt_backend::tir::type_refine::refine_types(&mut tir_func);
            let type_map = molt_backend::tir::type_refine::extract_type_map(&tir_func);
            let ops = molt_backend::tir::lower_to_simple::lower_to_simple_ir(
                &tir_func, &type_map,
            );
            if molt_backend::tir::lower_to_simple::validate_labels(&ops) {
                func.ops = ops;
                tir_count += 1;
            }
        }
        let tir_elapsed = tir_start.elapsed();
        eprintln!(
            "[molt-luau] TIR optimization: {tir_count} functions in {tir_elapsed:.2?}"
        );
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
            let mut backend = RustBackend::new();
            let source = backend.compile(&ir);
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
            let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
            let entry_module =
                std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
            let explicit_stdlib_module_symbols = explicit_stdlib_module_symbols_from_env();

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
                    if shared_stdlib_cache_matches(
                        stdlib_path,
                        expected_stdlib_cache_key.as_deref(),
                    ) {
                        // Cache exactly matches the requested stdlib IR — mark
                        // stdlib functions as extern stubs so the backend declares
                        // them as Import.  The linker resolves from stdlib_shared.o.
                        let mut retained = std::mem::take(&mut user_remaining);
                        let mut extern_count = 0usize;
                        for mut func in stdlib_funcs.drain(..) {
                            func.is_extern = true;
                            func.ops.clear();
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
                        eprintln!(
                            "MOLT_BACKEND: stdlib cache key mismatch (cached key {}, expected key {}; cached {} functions, need {}) — rebuilding",
                            cached_key.as_deref().unwrap_or("<missing>"),
                            expected_stdlib_cache_key.as_deref().unwrap_or("<missing>"),
                            cached_count,
                            current_stdlib_count,
                        );
                        let _ = std::fs::remove_file(stdlib_path);
                        let _ = std::fs::remove_file(&count_path);
                        let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
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
                    )?;
                    // Now compile user functions only
                    ir.functions = std::mem::take(&mut user_remaining);
                    eprintln!(
                        "MOLT_BACKEND: compiling {} user functions",
                        ir.functions.len()
                    );
                }
            }

            if stdlib_obj_path.is_none() {
                // Run dead function elimination on the full IR before batching
                // when the stdlib path did not already do the prune/partition split.
                molt_backend::eliminate_dead_functions(&mut ir);
            }

            // Deduplicate functions by name — the compiler can emit the same
            // function name multiple times (e.g. stdlib re-imports).  Keep the
            // first (largest) definition; duplicates cause "duplicate symbol"
            // errors during ld -r batched compilation.
            {
                let mut seen: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                ir.functions.retain(|f| seen.insert(f.name.clone()));
            }

            let func_count = ir.functions.len();
            let batch_size = resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE);

            if func_count <= batch_size {
                // Small IR (or user-only mode): compile in one shot
                let mut backend = SimpleBackend::new_with_target(target_triple);
                if stdlib_obj_path.is_some() {
                    backend.skip_shared_stdlib_partition = true;
                }
                let obj_bytes = backend.compile(ir);
                let mut file = create_backend_output_file(output_file).map_err(|err| {
                    io::Error::new(
                        err.kind(),
                        format!("failed to create backend output '{}': {}", output_file, err),
                    )
                })?;
                file.write_all(&obj_bytes)?;
                eprintln!("Successfully compiled to {output_file} ({func_count} functions)");
            } else {
                // Large IR: split into batches, compile each independently,
                // then merge with ld -r (partial link).  This prevents OOM
                // when compiling 1000+ stdlib functions into one ObjectModule.
                let mut all_functions: Vec<_> = ir.functions.into_iter().collect();
                let profile = ir.profile;
                let total_batches = all_functions.len().div_ceil(batch_size);
                let mut batch_paths: Vec<std::path::PathBuf> = Vec::new();
                let tmp_dir =
                    std::env::temp_dir().join(format!("molt_batch_{}", std::process::id()));
                let _ = std::fs::create_dir_all(&tmp_dir);

                let mut batch_idx = 0usize;
                // Pre-collect all function names for cross-batch import resolution.
                let all_func_names: std::collections::BTreeSet<String> =
                    all_functions.iter().map(|f| f.name.clone()).collect();
                let module_context = SimpleBackend::build_module_context(&all_functions);

                while !all_functions.is_empty() {
                    let remaining = all_functions.len();
                    let take = remaining.min(batch_size);
                    // drain from the front to keep order
                    let batch_funcs: Vec<_> = all_functions.drain(..take).collect();
                    eprintln!(
                        "MOLT_BACKEND: batch {}/{total_batches} ({} functions)",
                        batch_idx + 1,
                        batch_funcs.len()
                    );
                    let batch_ir = SimpleIR {
                        functions: batch_funcs,
                        profile: profile.clone(),
                    };
                    let mut backend = SimpleBackend::new_with_target(target_triple);
                    // CRITICAL: skip IR-level passes (inline, dead func elim)
                    // for batched compilation — those were already run on the
                    // full IR above. Each batch only does Cranelift codegen.
                    backend.skip_ir_passes = true;
                    backend.skip_shared_stdlib_partition = true;
                    backend.external_function_names =
                        batch_external_function_names(&all_func_names, &batch_ir.functions);
                    backend.set_module_context(module_context.clone());
                    let obj_bytes = backend.compile(batch_ir);

                    let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
                    std::fs::write(&batch_path, &obj_bytes)?;
                    batch_paths.push(batch_path);
                    batch_idx += 1;
                }

                // Merge batch objects with ld -r (relocatable partial link)
                merge_relocatable_objects(Path::new(output_file), &batch_paths, None)?;

                // Cleanup
                for p in &batch_paths {
                    let _ = std::fs::remove_file(p);
                }
                let _ = std::fs::remove_dir(&tmp_dir);
                eprintln!(
                    "Successfully compiled to {output_file} ({func_count} functions, {total_batches} batches)"
                );
            }
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
    use super::{
        BACKEND_DAEMON_PROTOCOL_VERSION, BackendOutputKind, DEFAULT_BACKEND_BATCH_SIZE,
        DEFAULT_STDLIB_BATCH_SIZE, DaemonCache, DaemonJobRequest, DaemonRequest, DaemonResponse,
        GIB, compile_single_job, create_backend_output_file, daemon_response_payload,
        default_backend_max_rss_gb_from_physical_mem_bytes, default_backend_output_path,
        ensure_output_parent_dir, is_user_owned_symbol, merge_relocatable_objects,
        partition_functions_for_batches, prune_and_partition_native_stdlib,
        read_daemon_request_bytes, relocatable_linker_binary, resolve_backend_output_path,
        resolved_batch_size_limit, shared_stdlib_cache_matches, write_cached_output,
        write_shared_stdlib_cache_sidecars,
    };
    use molt_backend::{FunctionIR, OpIR, SimpleIR};
    use std::io::Cursor;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

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
    fn read_daemon_request_bytes_stops_at_protocol_newline() {
        let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
        let bytes = read_daemon_request_bytes(&mut cursor).expect("request bytes");
        assert_eq!(bytes, b"{\"version\":1}\n");
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

    #[test]
    fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
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

        write_shared_stdlib_cache_sidecars(&stdlib_path, 7, Some("abc123"))
            .expect("write sidecars");
        assert!(shared_stdlib_cache_matches(&stdlib_path, Some("abc123")));
        assert!(!shared_stdlib_cache_matches(&stdlib_path, Some("def456")));
        assert!(!shared_stdlib_cache_matches(&stdlib_path, None));

        let _ = std::fs::remove_dir_all(&tmp_dir);
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

        let err = write_shared_stdlib_cache_sidecars(&stdlib_path, 7, Some("abc123"))
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

        merge_relocatable_objects(&output, std::slice::from_ref(&input), Some("false"))
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

        let err =
            merge_relocatable_objects(&output, &[input_a.clone(), input_b.clone()], Some("false"))
                .expect_err("merge should fail with false linker");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn resolved_batch_size_limit_defaults_and_zero_disable_count_cap() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();

        unsafe {
            std::env::remove_var("MOLT_BACKEND_BATCH_SIZE");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            DEFAULT_BACKEND_BATCH_SIZE
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            DEFAULT_STDLIB_BATCH_SIZE
        );

        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "0");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            usize::MAX
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            usize::MAX
        );

        match prior {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
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

        molt_backend::eliminate_dead_functions(&mut ir);
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
            molt_backend::eliminate_dead_functions(&mut ir);
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
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
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
                        {"name": "demo__module", "params": [], "ops": [{"kind": "ret_void"}]},
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
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
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
