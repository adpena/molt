use std::io;

#[cfg(unix)]
mod connection;
#[cfg(unix)]
mod context;
#[cfg(unix)]
mod dispatch;
#[cfg(unix)]
mod responses;
#[cfg(unix)]
mod wire;

#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use super::super::io_limits::{daemon_max_jobs, daemon_request_limit_bytes};
#[cfg(unix)]
use super::{DaemonCache, DaemonStats, daemon_cache_limit_bytes};
#[cfg(unix)]
use connection::handle_daemon_connection;
#[cfg(unix)]
use context::DaemonConnectionContext;

#[cfg(unix)]
pub(crate) use wire::{daemon_response_payload, read_daemon_request_bytes};

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

#[cfg(not(unix))]
pub(crate) fn run_daemon(_socket_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon mode requires unix domain sockets",
    ))
}
