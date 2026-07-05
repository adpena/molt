use std::path::Path;
use std::sync::Arc;

use super::super::super::io_limits::write_cached_output;
#[cfg(feature = "native-backend")]
use super::super::super::shared_stdlib_cache::shared_stdlib_cache_matches;
use super::super::protocol::{DaemonJobRequest, DaemonJobResponse};
use super::state::DaemonCache;

pub(crate) fn daemon_memory_cache_allowed_for_job(job: &DaemonJobRequest) -> bool {
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

pub(crate) fn insert_daemon_cache_entries(
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

pub(crate) fn try_write_cached_daemon_job_output(
    cache: &mut DaemonCache,
    job: &DaemonJobRequest,
    cache_key: &str,
    function_cache_key: &str,
) -> Option<DaemonJobResponse> {
    if !cache_key.is_empty()
        && let Some(bytes) = cache.get_bytes(cache_key)
    {
        return Some(cached_daemon_job_response(
            job,
            "module",
            write_cached_output(&job.output, bytes, job.skip_module_output_if_synced),
        ));
    }
    if !function_cache_key.is_empty()
        && function_cache_key != cache_key
        && let Some(bytes) = cache.get_bytes(function_cache_key)
    {
        return Some(cached_daemon_job_response(
            job,
            "function",
            write_cached_output(&job.output, bytes, job.skip_function_output_if_synced),
        ));
    }
    None
}

fn cached_daemon_job_response(
    job: &DaemonJobRequest,
    cache_tier: &str,
    write_result: std::io::Result<bool>,
) -> DaemonJobResponse {
    match write_result {
        Ok(output_written) => DaemonJobResponse {
            id: job.id.clone(),
            ok: true,
            cached: true,
            cache_tier: Some(cache_tier.to_string()),
            output_written,
            needs_ir: false,
            message: None,
            warnings: Vec::new(),
        },
        Err(err) => DaemonJobResponse {
            id: job.id.clone(),
            ok: false,
            cached: false,
            cache_tier: None,
            output_written: false,
            needs_ir: false,
            message: Some(format!("failed to write cached output: {err}")),
            warnings: Vec::new(),
        },
    }
}

pub(crate) fn maybe_cache_output_file(
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
