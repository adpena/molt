#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod document;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod ir;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod output;
mod response;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod target;

use super::cache::DaemonCache;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
use super::cache::{daemon_memory_cache_allowed_for_job, try_write_cached_daemon_job_output};
use super::protocol::{DaemonJobRequest, DaemonJobResponse};

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
use target::compile_daemon_job_output;

#[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
pub(crate) fn compile_single_job(
    job: DaemonJobRequest,
    _cache: &mut DaemonCache,
) -> DaemonJobResponse {
    response::unsupported_backend_response(job)
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
pub(crate) fn compile_single_job(
    mut job: DaemonJobRequest,
    cache: &mut DaemonCache,
) -> DaemonJobResponse {
    let cache_key = job.cache_key.trim().to_string();
    let function_cache_key = job
        .function_cache_key
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let daemon_memory_cache_allowed = daemon_memory_cache_allowed_for_job(&job);
    if daemon_memory_cache_allowed
        && let Some(response) =
            try_write_cached_daemon_job_output(cache, &job, &cache_key, &function_cache_key)
    {
        return response;
    }

    if job.probe_cache_only {
        return response::daemon_job_probe_cache_miss_response(job.id);
    }

    let document = match document::take_daemon_job_document(&mut job) {
        Ok(document) => document,
        Err(err) => return response::daemon_job_error_response(job.id, err),
    };

    let mut warnings = Vec::new();
    let compiled_output = match compile_daemon_job_output(&job, document) {
        Ok(output) => output,
        Err(err) => {
            return response::daemon_job_error_response(job.id, err);
        }
    };

    if let Err(err) = output::write_daemon_compiled_output(
        cache,
        &job,
        &cache_key,
        &function_cache_key,
        daemon_memory_cache_allowed,
        compiled_output,
        &mut warnings,
    ) {
        return response::daemon_job_error_response(job.id, err);
    }

    response::daemon_job_success_response(job.id, warnings)
}
