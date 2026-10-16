mod entries;
mod health;
mod job;
mod shared;
mod state;

#[cfg(feature = "wasm-backend")]
pub(crate) use entries::insert_daemon_cache_entries;
#[cfg(test)]
pub(crate) use health::default_daemon_cache_bytes_from_physical_mem_bytes;
pub(crate) use health::{DaemonStats, daemon_cache_limit_bytes};
#[cfg(feature = "native-backend")]
pub(crate) use job::maybe_cache_output_file;
pub(crate) use job::try_write_cached_daemon_job_output;
pub(crate) use shared::{DaemonCache, Flight};
#[cfg(test)]
pub(crate) use state::DaemonLru;
