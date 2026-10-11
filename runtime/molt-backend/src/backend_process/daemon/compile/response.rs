use super::super::DaemonJobResponse;

pub(super) fn daemon_job_probe_cache_miss_response(id: String) -> DaemonJobResponse {
    DaemonJobResponse {
        id,
        ok: true,
        cached: false,
        cache_tier: None,
        output_written: false,
        needs_ir: true,
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
        needs_ir: false,
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
        needs_ir: false,
        message: Some(message.into()),
        warnings: Vec::new(),
    }
}
