use std::sync::Arc;

use molt_backend::backend_environment;

use super::super::super::config::BACKEND_DAEMON_PROTOCOL_VERSION;
use super::super::compile::compile_contained_job;
use super::super::{DaemonRequest, DaemonResponse};
use super::context::DaemonState;
use super::responses::{
    daemon_error_response, daemon_jobs_response, daemon_ping_response, daemon_retire_response,
};

pub(crate) fn handle_daemon_request_bytes(raw_bytes: &[u8], state: &DaemonState) -> DaemonResponse {
    if raw_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
        return daemon_error_response("empty request", None);
    }

    let req = match DaemonRequest::from_json_bytes(raw_bytes) {
        Ok(req) => req,
        Err(err) => {
            return daemon_error_response(format!("invalid request JSON: {err}"), None);
        }
    };

    let include_health = req.include_health.unwrap_or(req.ping.unwrap_or(false));
    let version = req.version.unwrap_or(0);
    if version != BACKEND_DAEMON_PROTOCOL_VERSION {
        return daemon_error_response(
            format!(
                "unsupported protocol version {version}; expected {BACKEND_DAEMON_PROTOCOL_VERSION}"
            ),
            include_health.then(|| state.health()),
        );
    }

    if req.ping.unwrap_or(false) {
        return daemon_ping_response(state.health());
    }

    if req.retire.unwrap_or(false) {
        state.retire();
        return daemon_retire_response(state.health());
    }

    let config_digest = request_config_digest(&req);
    state.record_config_digest(config_digest.as_deref());
    let Some(jobs) = req.jobs else {
        return daemon_error_response(
            "missing jobs in request",
            include_health.then(|| state.health()),
        );
    };
    if jobs.is_empty() {
        return daemon_error_response(
            "empty jobs in request",
            include_health.then(|| state.health()),
        );
    }
    if jobs.len() > state.max_jobs {
        return daemon_error_response(
            format!(
                "too many jobs in request: {} exceeds daemon max_jobs {}",
                jobs.len(),
                state.max_jobs
            ),
            include_health.then(|| state.health()),
        );
    }

    state.stats().jobs_total += jobs.len() as u64;
    let _active = state.admit_jobs(jobs.len());
    let environment = req.environment;
    let mut results = Vec::with_capacity(jobs.len());
    for mut job in jobs {
        job.config_namespace = config_digest.clone();
        let result = {
            let _scope = backend_environment::enter(Arc::clone(&environment));
            compile_contained_job(job, &state.cache, || state.compile_slot())
        };
        let mut stats = state.stats();
        if result.ok && result.cached {
            stats.cache_hits += 1;
        } else {
            stats.cache_misses += 1;
        }
        drop(stats);
        results.push(result);
    }

    daemon_jobs_response(results, include_health.then(|| state.health()))
}

fn request_config_digest(req: &DaemonRequest) -> Option<String> {
    req.config_digest
        .as_deref()
        .map(str::trim)
        .filter(|digest| !digest.is_empty())
        .map(str::to_string)
}
