mod document;
mod ir;
mod output;
mod response;
mod target;

use super::cache::{DaemonCache, try_write_cached_daemon_job_output};
use super::protocol::{DaemonJobRequest, DaemonJobResponse};
use target::ResolvedJobTarget;

pub(crate) fn compile_single_job(
    mut job: DaemonJobRequest,
    cache: &mut DaemonCache,
) -> DaemonJobResponse {
    let cache_key = job.artifact_cache_key(&job.cache_key);
    let function_cache_key =
        job.artifact_cache_key(job.function_cache_key.as_deref().unwrap_or(""));
    let target = match ResolvedJobTarget::resolve(&job) {
        Ok(target) => target,
        Err(error) => return response::daemon_job_error_response(job.id, error),
    };
    let memory_cache_allowed = target.memory_cache_allowed();
    if memory_cache_allowed
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

    let compiled_output = match target.compile(document) {
        Ok(output) => output,
        Err(err) => return response::daemon_job_error_response(job.id, err),
    };

    match output::write_daemon_compiled_output(
        cache,
        &job.output,
        &cache_key,
        &function_cache_key,
        memory_cache_allowed,
        compiled_output,
    ) {
        Ok(warnings) => response::daemon_job_success_response(job.id, warnings),
        Err(err) => response::daemon_job_error_response(job.id, err),
    }
}
