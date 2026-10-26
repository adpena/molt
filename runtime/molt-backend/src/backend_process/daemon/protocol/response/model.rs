#[derive(Debug)]
pub(crate) struct DaemonJobResponse {
    pub(crate) id: String,
    pub(crate) origin: Option<crate::backend_process::origin::EmissionOrigin>,
    pub(crate) ok: bool,
    pub(crate) cached: bool,
    pub(crate) cache_tier: Option<String>,
    pub(crate) output_written: bool,
    /// Program positions whose bodies the daemon lacks. `Some` asks the
    /// caller to ship them (`needs_ir` on the wire); a probe miss with every
    /// body resident answers `Some(vec![])`.
    pub(crate) missing_functions: Option<Vec<usize>>,
    pub(crate) message: Option<String>,
    /// Function names that were replaced with trap stubs due to Cranelift
    /// compilation failures.  Propagated to the CLI for build warnings.
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct DaemonHealthResponse {
    pub(crate) protocol_version: u32,
    pub(crate) pid: u32,
    pub(crate) spawn_config_digest: Option<String>,
    pub(crate) active_config_digest: Option<String>,
    pub(crate) uptime_ms: u64,
    /// Every resident entry: compiled artifacts and function bodies.
    pub(crate) cache_entries: usize,
    /// Resident function bodies of the content-addressed store (HF-516).
    pub(crate) resident_functions: usize,
    pub(crate) cache_bytes: usize,
    pub(crate) cache_max_bytes: Option<usize>,
    pub(crate) request_limit_bytes: Option<usize>,
    pub(crate) max_jobs: Option<usize>,
    pub(crate) requests_total: u64,
    pub(crate) jobs_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    /// Jobs admitted and not yet answered; zero means idle.
    pub(crate) active_jobs: usize,
    /// The daemon accepts no new connections and exits when drained.
    pub(crate) retiring: bool,
}

#[derive(Debug)]
pub(crate) struct DaemonResponse {
    pub(crate) ok: bool,
    pub(crate) pong: bool,
    pub(crate) jobs: Vec<DaemonJobResponse>,
    pub(crate) error: Option<String>,
    pub(crate) health: Option<DaemonHealthResponse>,
}
