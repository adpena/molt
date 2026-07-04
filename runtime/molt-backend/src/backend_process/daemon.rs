use super::*;

#[cfg(unix)]
pub(crate) fn run_daemon(socket_path: &str) -> io::Result<()> {
    use std::os::unix::net::UnixListener;

    let socket = Path::new(socket_path);
    if socket.exists() {
        let _ = std::fs::remove_file(socket);
    }
    if let Some(parent) = socket.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket)?;
    let request_limit_bytes = daemon_request_limit_bytes();
    let max_jobs = daemon_max_jobs();
    let mut cache = DaemonCache::new(Some(daemon_cache_limit_bytes()));
    let mut stats = DaemonStats::default();
    let spawn_config_digest = env::var("MOLT_BACKEND_DAEMON_CONFIG_DIGEST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut active_config_digest: Option<String> = None;
    let started_at = Instant::now();
    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(err) = handle_daemon_connection(
                    &mut conn,
                    DaemonConnectionContext {
                        cache: &mut cache,
                        stats: &mut stats,
                        spawn_config_digest: spawn_config_digest.as_deref(),
                        active_config_digest: &mut active_config_digest,
                        started_at,
                        request_limit_bytes,
                        max_jobs,
                    },
                ) {
                    eprintln!("backend daemon connection error: {err}");
                }
            }
            Err(err) => {
                eprintln!("backend daemon accept error: {err}");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) struct DaemonConnectionContext<'a> {
    pub(crate) cache: &'a mut DaemonCache,
    pub(crate) stats: &'a mut DaemonStats,
    pub(crate) spawn_config_digest: Option<&'a str>,
    pub(crate) active_config_digest: &'a mut Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) request_limit_bytes: usize,
    pub(crate) max_jobs: usize,
}

#[cfg(unix)]
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

#[cfg(unix)]
pub(crate) fn read_daemon_request_bytes<R: BufRead>(
    reader: &mut R,
    request_limit_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let limit = u64::try_from(request_limit_bytes)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    reader.take(limit).read_until(b'\n', &mut raw_bytes)?;
    if raw_bytes.len() > request_limit_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("daemon request exceeded {request_limit_bytes} byte limit"),
        ));
    }
    Ok(raw_bytes)
}

#[cfg(unix)]
pub(crate) fn write_daemon_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &DaemonResponse,
) -> io::Result<()> {
    let mut payload = daemon_response_payload(response)?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn daemon_response_payload(response: &DaemonResponse) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&response.to_json_value()).map_err(io::Error::other)
}

#[cfg(not(unix))]
pub(crate) fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}
