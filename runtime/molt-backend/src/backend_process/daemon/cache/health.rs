use std::env;
use std::time::Instant;

use super::super::super::config::{
    BACKEND_DAEMON_PROTOCOL_VERSION, MIB, detect_physical_memory_bytes,
};
use super::super::protocol::DaemonHealthResponse;
use super::state::DaemonCache;

#[derive(Default)]
pub(crate) struct DaemonStats {
    pub(crate) requests_total: u64,
    pub(crate) jobs_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
}

pub(crate) fn default_daemon_cache_bytes_from_physical_mem_bytes(bytes: Option<u64>) -> usize {
    let default = bytes
        .and_then(|raw| usize::try_from(raw / 64).ok())
        .unwrap_or(512 * MIB);
    default.clamp(128 * MIB, 2 * 1024 * MIB)
}

pub(crate) fn daemon_cache_limit_bytes() -> usize {
    env::var("MOLT_BACKEND_DAEMON_CACHE_MB")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|mb| mb.saturating_mul(MIB))
        .unwrap_or_else(|| {
            default_daemon_cache_bytes_from_physical_mem_bytes(detect_physical_memory_bytes())
        })
}

pub(crate) fn daemon_health(
    cache: &DaemonCache,
    stats: &DaemonStats,
    spawn_config_digest: Option<&str>,
    active_config_digest: Option<&str>,
    start: Instant,
    request_limit_bytes: usize,
    max_jobs: usize,
) -> DaemonHealthResponse {
    let uptime_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    DaemonHealthResponse {
        protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
        pid: std::process::id(),
        spawn_config_digest: spawn_config_digest.map(str::to_string),
        active_config_digest: active_config_digest.map(str::to_string),
        uptime_ms,
        cache_entries: cache.entries.len(),
        cache_bytes: cache.bytes,
        cache_max_bytes: cache.max_bytes,
        request_limit_bytes: Some(request_limit_bytes),
        max_jobs: Some(max_jobs),
        requests_total: stats.requests_total,
        jobs_total: stats.jobs_total,
        cache_hits: stats.cache_hits,
        cache_misses: stats.cache_misses,
    }
}
