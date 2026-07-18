#[cfg(any(unix, test))]
mod cache;
#[cfg(any(unix, test))]
mod compile;
#[cfg(any(unix, test))]
mod protocol;
mod server;

#[cfg(any(unix, test))]
pub(crate) use cache::{DaemonCache, default_daemon_cache_bytes_from_physical_mem_bytes};
#[cfg(any(unix, test))]
pub(crate) use compile::compile_single_job;
#[cfg(unix)]
pub(crate) use protocol::DaemonHealthResponse;
#[cfg(any(unix, test))]
pub(crate) use protocol::{
    DaemonJobRequest, DaemonJobResponse, DaemonRequest, DaemonResponse, daemon_response_payload,
    read_daemon_request_bytes,
};
pub(crate) use server::run_daemon;
