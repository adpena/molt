#[cfg(feature = "native-backend")]
mod json;
mod limits;
mod output;
mod request;

#[cfg(feature = "native-backend")]
pub(crate) use json::{read_json_artifact, write_json_artifact};
pub(crate) use limits::stdin_request_limit_bytes;
#[cfg(molt_backend_daemon)]
pub(crate) use limits::{daemon_idle_timeout, daemon_max_jobs, daemon_request_limit_bytes};
pub(crate) use output::ensure_output_parent_dir;
#[cfg(molt_backend_daemon)]
pub(crate) use output::write_cached_output;
#[cfg(all(molt_backend_daemon, feature = "wasm-backend"))]
pub(crate) use output::write_output;
#[cfg(feature = "native-backend")]
pub(crate) use output::write_output_path;
pub(crate) use request::{RequestBoundedRead, read_bounded_request_bytes};
