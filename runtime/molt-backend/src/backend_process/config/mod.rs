mod constants;
#[cfg(any(unix, test))]
mod daemon_env;
mod memory;

#[cfg(any(unix, test))]
pub(crate) use constants::DEFAULT_DAEMON_MAX_JOBS;
pub(crate) use constants::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE,
    DEFAULT_DAEMON_REQUEST_LIMIT_BYTES, DEFAULT_STDIN_REQUEST_LIMIT_BYTES,
    DEFAULT_STDLIB_BATCH_SIZE, GIB, MIB,
};
#[cfg(any(unix, test))]
pub(crate) use daemon_env::{BACKEND_DAEMON_PROTOCOL_VERSION, DAEMON_REQUEST_ENV_KEYS};
pub(crate) use memory::{
    default_backend_max_rss_gb, default_backend_max_rss_gb_from_physical_mem_bytes,
    detect_physical_memory_bytes,
};
