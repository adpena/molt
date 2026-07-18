use std::time::Instant;

use super::super::cache::{DaemonCache, DaemonStats, daemon_health};
use super::super::protocol::DaemonHealthResponse;

pub(crate) struct DaemonConnectionContext<'a> {
    pub(crate) cache: &'a mut DaemonCache,
    pub(crate) stats: &'a mut DaemonStats,
    pub(crate) spawn_config_digest: Option<&'a str>,
    pub(crate) active_config_digest: &'a mut Option<String>,
    pub(crate) started_at: Instant,
    pub(crate) request_limit_bytes: usize,
    pub(crate) max_jobs: usize,
}

impl DaemonConnectionContext<'_> {
    pub(crate) fn health(&self) -> DaemonHealthResponse {
        daemon_health(
            self.cache,
            self.stats,
            self.spawn_config_digest,
            self.active_config_digest.as_deref(),
            self.started_at,
            self.request_limit_bytes,
            self.max_jobs,
        )
    }

    pub(crate) fn activate_config_digest(&mut self, request_config_digest: Option<String>) {
        if let Some(digest) = request_config_digest
            && self.active_config_digest.as_deref() != Some(digest.as_str())
        {
            self.cache.clear();
            *self.active_config_digest = Some(digest);
        }
    }
}
