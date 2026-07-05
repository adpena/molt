use super::super::{DaemonHealthResponse, DaemonJobResponse, DaemonResponse};

pub(crate) fn daemon_error_response(
    error: impl Into<String>,
    health: Option<DaemonHealthResponse>,
) -> DaemonResponse {
    DaemonResponse {
        ok: false,
        pong: false,
        jobs: Vec::new(),
        error: Some(error.into()),
        health,
    }
}

pub(crate) fn daemon_ping_response(health: DaemonHealthResponse) -> DaemonResponse {
    DaemonResponse {
        ok: true,
        pong: true,
        jobs: Vec::new(),
        error: None,
        health: Some(health),
    }
}

pub(crate) fn daemon_jobs_response(
    jobs: Vec<DaemonJobResponse>,
    health: Option<DaemonHealthResponse>,
) -> DaemonResponse {
    let all_ok = jobs.iter().all(|job| job.ok);
    DaemonResponse {
        ok: all_ok,
        pong: false,
        jobs,
        error: None,
        health,
    }
}
