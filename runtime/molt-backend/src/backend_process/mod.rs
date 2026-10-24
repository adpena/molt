mod cli_args;
mod config;
#[cfg(molt_backend_daemon)]
mod daemon;
mod emit;
mod input;
mod io_limits;
mod memory_guard;
#[cfg(feature = "native-backend")]
mod native_artifact;
#[cfg(feature = "native-backend")]
mod native_batch;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
pub(crate) mod origin;
#[cfg(feature = "native-backend")]
mod shared_stdlib_cache;

pub(crate) use cli_args::*;
#[cfg(all(test, molt_backend_daemon))]
pub(crate) use config::{BACKEND_DAEMON_PROTOCOL_VERSION, MIB};
#[cfg(all(test, feature = "native-backend"))]
pub(crate) use config::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE,
};
#[cfg(molt_backend_daemon)]
pub(crate) use daemon::*;
pub(crate) use emit::*;
pub(crate) use input::*;
#[cfg(test)]
pub(crate) use io_limits::*;
pub(crate) use io_limits::{read_bounded_request_bytes, stdin_request_limit_bytes};
pub(crate) use memory_guard::*;
#[cfg(feature = "native-backend")]
pub(crate) use native_artifact::{NativeArtifactKind, shared_stdlib_archive_path_from_env};
#[cfg(feature = "native-backend")]
pub(crate) use native_batch::*;
#[cfg(all(feature = "native-backend", test))]
pub(crate) use shared_stdlib_cache::*;
