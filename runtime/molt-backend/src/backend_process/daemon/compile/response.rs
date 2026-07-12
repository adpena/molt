#[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
use super::super::DaemonJobRequest;
use super::super::DaemonJobResponse;

#[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
pub(super) fn unsupported_backend_response(job: DaemonJobRequest) -> DaemonJobResponse {
    let unsupported = if job.is_wasm {
        "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend"
    } else {
        "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend"
    };
    daemon_job_error_response(job.id, unsupported)
}

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
