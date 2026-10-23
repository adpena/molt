use super::super::DaemonJobResponse;

/// A cache miss, or a lease whose unshipped bodies the store no longer holds:
/// the caller ships the bodies at `missing_functions` and asks again.
pub(super) fn daemon_job_needs_functions_response(
    id: String,
    missing_functions: Vec<usize>,
) -> DaemonJobResponse {
    DaemonJobResponse {
        id,
        ok: true,
        cached: false,
        cache_tier: None,
        output_written: false,
        missing_functions: Some(missing_functions),
        message: None,
        warnings: Vec::new(),
    }
}

pub(super) fn daemon_job_success_response(id: String, warnings: Vec<String>) -> DaemonJobResponse {
    DaemonJobResponse {
        id,
        ok: true,
        cached: false,
        cache_tier: None,
        output_written: true,
        missing_functions: None,
        message: None,
        warnings,
    }
}

pub(super) fn daemon_job_error_response(
    id: String,
    message: impl Into<String>,
) -> DaemonJobResponse {
    DaemonJobResponse {
        id,
        ok: false,
        cached: false,
        cache_tier: None,
        output_written: false,
        missing_functions: None,
        message: Some(message.into()),
        warnings: Vec::new(),
    }
}
