use std::env;

use super::super::super::config::MIB;
use super::super::super::config::detect_physical_memory_bytes;

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

#[expect(
    clippy::disallowed_methods,
    reason = "the daemon's cache budget is fixed when it starts"
)]
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
