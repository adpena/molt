mod constants;
#[cfg(molt_backend_daemon)]
mod daemon_env;
mod memory;

pub(crate) use constants::DEFAULT_STDIN_REQUEST_LIMIT_BYTES;
#[cfg(test)]
pub(crate) use constants::GIB;
#[cfg(feature = "native-backend")]
pub(crate) use constants::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE,
};
#[cfg(molt_backend_daemon)]
pub(crate) use constants::{DEFAULT_DAEMON_MAX_JOBS, DEFAULT_DAEMON_REQUEST_LIMIT_BYTES, MIB};
#[cfg(molt_backend_daemon)]
pub(crate) use daemon_env::BACKEND_DAEMON_PROTOCOL_VERSION;
pub(crate) use memory::default_backend_max_rss_gb;
#[cfg(test)]
pub(crate) use memory::default_backend_max_rss_gb_from_physical_mem_bytes;
#[cfg(molt_backend_daemon)]
pub(crate) use memory::detect_physical_memory_bytes;
