use crate::backend_process::io_limits::write_cached_output;

use super::super::super::protocol::{DaemonJobRequest, DaemonJobResponse};
use super::super::state::DaemonCache;

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
