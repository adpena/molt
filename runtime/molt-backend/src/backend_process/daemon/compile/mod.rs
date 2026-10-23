mod document;
mod output;
mod response;
mod target;

use super::cache::{DaemonCache, Flight, try_write_cached_daemon_job_output};
use super::protocol::{
    DaemonArtifactCacheKeys, DaemonIrSource, DaemonJobRequest, DaemonJobResponse,
};
use target::ResolvedJobTarget;

/// Compiles one job for a connection. A panicking compile is a compiler bug:
/// it fails this job with the panic message instead of dropping the
/// connection, and concurrent jobs continue. `compile_slot` is taken only when
/// the job must compile, so cache hits and followers never wait for one.
pub(crate) fn compile_contained_job<Slot>(
    job: DaemonJobRequest,
    cache: &DaemonCache,
    compile_slot: impl FnOnce() -> Slot,
) -> DaemonJobResponse {
    let id = job.id.clone();
    contain_panic(id, || compile_job(job, cache, compile_slot))
}

/// Runs `compile`, turning a panic into this job's error response. The cache
/// recovers from poisoned locks and single flight releases its followers on
/// unwind, so shared daemon state stays usable.
fn contain_panic(id: String, compile: impl FnOnce() -> DaemonJobResponse) -> DaemonJobResponse {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(compile)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        response::daemon_job_error_response(id, format!("backend compile panicked: {message}"))
    })
}

/// Compiles one job without a compile-slot limit.
#[cfg(test)]
pub(crate) fn compile_single_job(job: DaemonJobRequest, cache: &DaemonCache) -> DaemonJobResponse {
    compile_job(job, cache, || ())
}

fn compile_job<Slot>(
    mut job: DaemonJobRequest,
    cache: &DaemonCache,
    compile_slot: impl FnOnce() -> Slot,
) -> DaemonJobResponse {
    let target = match ResolvedJobTarget::resolve(&job) {
        Ok(target) => target,
        Err(error) => return response::daemon_job_error_response(job.id, error),
    };
    let keys = if target.memory_cache_allowed() {
        match job.take_artifact_cache_keys() {
            Ok(keys) => keys,
            Err(error) => {
                return response::daemon_job_error_response(
                    job.id,
                    format!("invalid backend semantic environment: {error}"),
                );
            }
        }
    } else {
        DaemonArtifactCacheKeys::default()
    };
    if let Some(response) = try_write_cached_daemon_job_output(
        cache,
        &job,
        keys.module.as_ref(),
        keys.function.as_ref(),
    ) {
        return response;
    }

    if let DaemonIrSource::Probe(digests) = &job.ir_source {
        let missing = cache.missing_functions(digests);
        return response::daemon_job_needs_functions_response(job.id, missing);
    }

    // Concurrent requests for one artifact compile it once: the leader holds
    // the flight until its output is cached, and the rest then reuse it.
    let flight_key = keys.module.as_ref().or(keys.function.as_ref());
    let _flight = if let Some(flight_key) = flight_key {
        let flight = cache.begin(flight_key);
        // Probe again: a follower finds the leader's output, and a new leader
        // finds output that a flight finished after this caller's first probe.
        if let Some(response) = try_write_cached_daemon_job_output(
            cache,
            &job,
            keys.module.as_ref(),
            keys.function.as_ref(),
        ) {
            return response;
        }
        match flight {
            Flight::Leader(guard) => Some(guard),
            // The leader cached nothing (it failed, or its output exceeds the
            // memory budget), so this caller compiles for itself.
            Flight::Waited => None,
        }
    } else {
        None
    };
    let _slot = compile_slot();

    let document = match document::read_daemon_job_document(&job, cache) {
        Ok(document::DaemonJobDocument::Ready(document)) => *document,
        Ok(document::DaemonJobDocument::MissingFunctions(positions)) => {
            return response::daemon_job_needs_functions_response(job.id, positions);
        }
        Err(err) => return response::daemon_job_error_response(job.id, err),
    };

    let compiled_output = match target.compile(document) {
        Ok(output) => output,
        Err(err) => return response::daemon_job_error_response(job.id, err),
    };

    match output::write_daemon_compiled_output(
        cache,
        &job.output,
        keys.module.as_ref(),
        keys.function.as_ref(),
        compiled_output,
    ) {
        Ok(warnings) => response::daemon_job_success_response(job.id, warnings),
        Err(err) => response::daemon_job_error_response(job.id, err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_compile_fails_only_its_job() {
        let failed = contain_panic("job0".to_string(), || panic!("formatted {}", 7));
        assert!(!failed.ok);
        assert_eq!(failed.id, "job0");
        assert_eq!(
            failed.message.as_deref(),
            Some("backend compile panicked: formatted 7")
        );
        let failed = contain_panic("job1".to_string(), || {
            std::panic::panic_any("static message")
        });
        assert_eq!(
            failed.message.as_deref(),
            Some("backend compile panicked: static message")
        );
        let passed = contain_panic("job2".to_string(), || {
            response::daemon_job_success_response("job2".to_string(), Vec::new())
        });
        assert!(passed.ok);
    }
}
