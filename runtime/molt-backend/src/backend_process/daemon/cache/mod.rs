#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod entries;
mod health;
mod job;
mod state;

#[cfg(feature = "wasm-backend")]
pub(crate) use entries::insert_daemon_cache_entries;
#[cfg(test)]
pub(crate) use health::default_daemon_cache_bytes_from_physical_mem_bytes;
#[cfg(unix)]
pub(crate) use health::{DaemonStats, daemon_cache_limit_bytes, daemon_health};
#[cfg(feature = "native-backend")]
pub(crate) use job::maybe_cache_output_file;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
pub(crate) use job::{daemon_memory_cache_allowed_for_job, try_write_cached_daemon_job_output};
pub(crate) use state::DaemonCache;
