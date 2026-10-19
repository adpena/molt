mod constants;
mod memory;

pub(crate) use constants::DEFAULT_STDIN_REQUEST_LIMIT_BYTES;
#[cfg(test)]
pub(crate) use constants::GIB;
#[cfg(feature = "native-backend")]
pub(crate) use constants::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE,
};
#[cfg(molt_backend_daemon)]
pub(crate) use constants::{
    DEFAULT_DAEMON_IDLE_TIMEOUT_SECS, DEFAULT_DAEMON_MAX_JOBS, DEFAULT_DAEMON_REQUEST_LIMIT_BYTES,
    MIB,
};
#[cfg(molt_backend_daemon)]
pub(crate) const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../src/molt/backend_daemon_protocol_version.txt"
));
pub(crate) use memory::default_backend_max_rss_gb;
#[cfg(test)]
pub(crate) use memory::default_backend_max_rss_gb_from_physical_mem_bytes;
#[cfg(molt_backend_daemon)]
pub(crate) use memory::detect_physical_memory_bytes;
