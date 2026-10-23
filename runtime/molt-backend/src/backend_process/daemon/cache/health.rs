use super::super::super::config::MIB;
use molt_host_process::detect_physical_memory_bytes;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

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

/// The daemon's cache budget is fixed when it starts: the setting is in the
/// process-scoped `cache` catalog group, so it names the daemon.
pub(crate) fn daemon_cache_limit_bytes() -> usize {
    backend_env::var(BackendEnv::MOLT_BACKEND_DAEMON_CACHE_MB)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|mb| mb.saturating_mul(MIB))
        .unwrap_or_else(|| {
            default_daemon_cache_bytes_from_physical_mem_bytes(detect_physical_memory_bytes())
        })
}
