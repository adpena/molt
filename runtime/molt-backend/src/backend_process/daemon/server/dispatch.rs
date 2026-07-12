use super::super::super::config::BACKEND_DAEMON_PROTOCOL_VERSION;
use super::super::{DaemonRequest, DaemonResponse, compile_single_job};
use super::context::DaemonConnectionContext;
use super::responses::{daemon_error_response, daemon_jobs_response, daemon_ping_response};

pub(crate) fn handle_daemon_request_bytes(
    raw_bytes: &[u8],
    ctx: &mut DaemonConnectionContext<'_>,
) -> DaemonResponse {
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
            include_health.then(|| ctx.health()),
        );
    }

    if req.ping.unwrap_or(false) {
        return daemon_ping_response(ctx.health());
    }

    ctx.activate_config_digest(request_config_digest(&req));
    let Some(jobs) = req.jobs else {
        return daemon_error_response(
            "missing jobs in request",
            include_health.then(|| ctx.health()),
        );
    };
    if jobs.is_empty() {
        return daemon_error_response(
            "empty jobs in request",
            include_health.then(|| ctx.health()),
        );
    }
    if jobs.len() > ctx.max_jobs {
        return daemon_error_response(
            format!(
                "too many jobs in request: {} exceeds daemon max_jobs {}",
                jobs.len(),
                ctx.max_jobs
            ),
            include_health.then(|| ctx.health()),
        );
    }

    ctx.stats.jobs_total = ctx.stats.jobs_total.saturating_add(jobs.len() as u64);
    let mut results = Vec::with_capacity(jobs.len());
    for job in jobs {
        let result = compile_single_job(job, ctx.cache);
        if result.ok && result.cached {
            ctx.stats.cache_hits = ctx.stats.cache_hits.saturating_add(1);
        } else {
            ctx.stats.cache_misses = ctx.stats.cache_misses.saturating_add(1);
        }
        results.push(result);
    }

    daemon_jobs_response(results, include_health.then(|| ctx.health()))
}

fn request_config_digest(req: &DaemonRequest) -> Option<String> {
    req.config_digest
        .as_deref()
        .map(str::trim)
        .filter(|digest| !digest.is_empty())
        .map(str::to_string)
}
