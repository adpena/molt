use std::env;

#[cfg(any(unix, test))]
use super::super::config::DEFAULT_DAEMON_MAX_JOBS;
use super::super::config::{DEFAULT_DAEMON_REQUEST_LIMIT_BYTES, DEFAULT_STDIN_REQUEST_LIMIT_BYTES};

fn env_usize_limit(name: &str, default: usize, min_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value >= min_value)
        .unwrap_or(default)
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_DAEMON_REQUEST_LIMIT_BYTES",
        DEFAULT_DAEMON_REQUEST_LIMIT_BYTES,
        1024,
    )
}

pub(crate) fn stdin_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES",
        DEFAULT_STDIN_REQUEST_LIMIT_BYTES,
        1024,
    )
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_max_jobs() -> usize {
    env_usize_limit("MOLT_BACKEND_DAEMON_MAX_JOBS", DEFAULT_DAEMON_MAX_JOBS, 1)
}
