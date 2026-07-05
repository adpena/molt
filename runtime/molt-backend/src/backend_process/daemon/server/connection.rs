use std::io;
use std::time::Instant;

use super::super::super::config::BACKEND_DAEMON_PROTOCOL_VERSION;
use super::super::{
    DaemonCache, DaemonRequest, DaemonResponse, DaemonStats, compile_single_job, daemon_health,
};
use super::wire::{read_daemon_request_bytes, write_daemon_response};

pub(crate) struct DaemonConnectionContext<'a> {
    pub(crate) cache: &'a mut DaemonCache,
    pub(crate) stats: &'a mut DaemonStats,
    pub(crate) spawn_config_digest: Option<&'a str>,
    pub(crate) active_config_digest: &'a mut Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) request_limit_bytes: usize,
    pub(crate) max_jobs: usize,
}

pub(crate) fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    ctx: DaemonConnectionContext<'_>,
) -> io::Result<()> {
    let DaemonConnectionContext {
        cache,
        stats,
        spawn_config_digest,
        active_config_digest,
        started_at,
        request_limit_bytes,
        max_jobs,
    } = ctx;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    loop {
        let raw_bytes = read_daemon_request_bytes(&mut reader, request_limit_bytes)?;
        if raw_bytes.is_empty() {
            return Ok(());
        }
        stats.requests_total = stats.requests_total.saturating_add(1);
        if raw_bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty request".to_string()),
                health: None,
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let req = match DaemonRequest::from_json_bytes(&raw_bytes) {
            Ok(req) => req,
            Err(err) => {
                let response = DaemonResponse {
                    ok: false,
                    pong: false,
                    jobs: Vec::new(),
                    error: Some(format!("invalid request JSON: {err}")),
                    health: None,
                };
                write_daemon_response(stream, &response)?;
                continue;
            }
        };
        let include_health = req.include_health.unwrap_or(req.ping.unwrap_or(false));
        let version = req.version.unwrap_or(0);
        if version != BACKEND_DAEMON_PROTOCOL_VERSION {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "unsupported protocol version {version}; expected {BACKEND_DAEMON_PROTOCOL_VERSION}"
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if req.ping.unwrap_or(false) {
            let response = DaemonResponse {
                ok: true,
                pong: true,
                jobs: Vec::new(),
                error: None,
                health: Some(daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        let request_config_digest = req
            .config_digest
            .as_deref()
            .map(str::trim)
            .filter(|digest| !digest.is_empty())
            .map(|digest| digest.to_string());
        if let Some(ref digest) = request_config_digest
            && active_config_digest.as_deref() != Some(digest.as_str())
        {
            cache.clear();
            *active_config_digest = Some(digest.clone());
        }
        let Some(jobs) = req.jobs else {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("missing jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        };
        if jobs.is_empty() {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some("empty jobs in request".to_string()),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        if jobs.len() > max_jobs {
            let response = DaemonResponse {
                ok: false,
                pong: false,
                jobs: Vec::new(),
                error: Some(format!(
                    "too many jobs in request: {} exceeds daemon max_jobs {}",
                    jobs.len(),
                    max_jobs
                )),
                health: include_health.then(|| {
                    daemon_health(
                        cache,
                        stats,
                        spawn_config_digest,
                        active_config_digest.as_deref(),
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    )
                }),
            };
            write_daemon_response(stream, &response)?;
            continue;
        }
        stats.jobs_total = stats.jobs_total.saturating_add(jobs.len() as u64);
        let mut results = Vec::with_capacity(jobs.len());
        for job in jobs {
            let result = compile_single_job(job, cache);
            if result.ok && result.cached {
                stats.cache_hits = stats.cache_hits.saturating_add(1);
            } else {
                stats.cache_misses = stats.cache_misses.saturating_add(1);
            }
            results.push(result);
        }
        let all_ok = results.iter().all(|job| job.ok);
        let response = DaemonResponse {
            ok: all_ok,
            pong: false,
            jobs: results,
            error: None,
            health: include_health.then(|| {
                daemon_health(
                    cache,
                    stats,
                    spawn_config_digest,
                    active_config_digest.as_deref(),
                    started_at,
                    request_limit_bytes,
                    max_jobs,
                )
            }),
        };
        write_daemon_response(stream, &response)?;
    }
}
