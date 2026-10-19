//! The compile daemon: native and WASM jobs over a Unix domain socket. The
//! parent module compiles it only where `molt_backend_daemon` holds.

mod cache;
mod compile;
mod protocol;
mod server;

#[cfg(test)]
pub(crate) use cache::{
    DaemonCache, DaemonLru, default_daemon_cache_bytes_from_physical_mem_bytes,
};
#[cfg(test)]
pub(crate) use compile::compile_single_job;
pub(crate) use protocol::DaemonHealthResponse;
pub(crate) use protocol::{
    DaemonIrSource, DaemonJobRequest, DaemonJobResponse, DaemonJobTarget, DaemonRequest,
    DaemonResponse, daemon_response_payload, read_daemon_request_bytes,
};
pub(crate) use server::run_daemon;
