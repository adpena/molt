#[cfg(any(unix, test))]
mod cache;
#[cfg(any(unix, test))]
mod compile;
#[cfg(any(unix, test))]
mod protocol;
mod server;

#[cfg(any(unix, test))]
pub(crate) use cache::{
    DaemonCache, DaemonStats, daemon_cache_limit_bytes, daemon_health,
    default_daemon_cache_bytes_from_physical_mem_bytes,
};
#[cfg(all(
    any(unix, test),
    any(feature = "native-backend", feature = "wasm-backend")
))]
pub(crate) use cache::{
    daemon_memory_cache_allowed_for_job, insert_daemon_cache_entries, maybe_cache_output_file,
    try_write_cached_daemon_job_output,
};
#[cfg(any(unix, test))]
pub(crate) use compile::{backend_ir_document_from_json_path, compile_single_job};
#[cfg(any(unix, test))]
pub(crate) use protocol::{
    DaemonHealthResponse, DaemonJobRequest, DaemonJobResponse, DaemonRequest, DaemonResponse,
    is_false,
};
pub(crate) use server::run_daemon;
#[cfg(unix)]
pub(crate) use server::{daemon_response_payload, read_daemon_request_bytes};
