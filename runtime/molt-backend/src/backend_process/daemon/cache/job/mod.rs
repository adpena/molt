mod hit;
#[cfg(feature = "native-backend")]
mod ingest;

pub(crate) use hit::try_write_cached_daemon_job_output;
#[cfg(feature = "native-backend")]
pub(crate) use ingest::maybe_cache_output_file;
