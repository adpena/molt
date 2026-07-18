mod constants;
#[cfg(any(unix, test))]
mod daemon_env;
mod memory;

#[cfg(any(unix, test))]
pub(crate) use constants::DEFAULT_DAEMON_MAX_JOBS;
#[cfg(any(unix, test))]
pub(crate) use constants::DEFAULT_DAEMON_REQUEST_LIMIT_BYTES;
pub(crate) use constants::DEFAULT_STDIN_REQUEST_LIMIT_BYTES;
#[cfg(test)]
pub(crate) use constants::GIB;
#[cfg(any(unix, test))]
pub(crate) use constants::MIB;
#[cfg(feature = "native-backend")]
pub(crate) use constants::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE,
};
#[cfg(any(unix, test))]
pub(crate) use daemon_env::{BACKEND_DAEMON_PROTOCOL_VERSION, DAEMON_REQUEST_ENV_KEYS};
pub(crate) use memory::default_backend_max_rss_gb;
#[cfg(test)]
pub(crate) use memory::default_backend_max_rss_gb_from_physical_mem_bytes;
#[cfg(unix)]
pub(crate) use memory::detect_physical_memory_bytes;
