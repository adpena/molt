#[cfg(feature = "native-backend")]
mod json;
mod limits;
mod output;
mod request;

#[cfg(feature = "native-backend")]
pub(crate) use json::{read_json_artifact, write_json_artifact};
pub(crate) use limits::stdin_request_limit_bytes;
#[cfg(unix)]
pub(crate) use limits::{daemon_max_jobs, daemon_request_limit_bytes};
#[cfg(test)]
pub(crate) use output::default_backend_output_path;
#[cfg(any(
    all(unix, any(feature = "native-backend", feature = "wasm-backend")),
    test
))]
pub(crate) use output::write_cached_output;
#[cfg(all(any(unix, test), feature = "wasm-backend"))]
pub(crate) use output::write_output;
#[cfg(feature = "native-backend")]
pub(crate) use output::write_output_path;
pub(crate) use output::{BackendOutputKind, ensure_output_parent_dir, resolve_backend_output_path};
pub(crate) use request::{RequestBoundedRead, read_bounded_request_bytes};
