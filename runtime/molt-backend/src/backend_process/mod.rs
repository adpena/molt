#[cfg(any(
    feature = "native-backend",
    feature = "luau-backend",
    feature = "rust-backend",
    feature = "wasm-backend",
    test
))]
mod atomic_publish;
mod cli_args;
mod config;
mod daemon;
mod emit;
mod input;
mod io_limits;
mod memory_guard;
#[cfg(feature = "native-backend")]
mod native_batch;
#[cfg(feature = "native-backend")]
mod shared_stdlib_cache;

#[cfg(feature = "wasm-backend")]
pub(crate) use atomic_publish::AtomicFilePublication;
#[cfg(test)]
pub(crate) use atomic_publish::write_bytes_atomically;
pub(crate) use cli_args::*;
#[cfg(test)]
pub(crate) use config::*;
pub(crate) use daemon::*;
pub(crate) use emit::*;
pub(crate) use input::*;
#[cfg(test)]
pub(crate) use io_limits::*;
pub(crate) use memory_guard::*;
#[cfg(feature = "native-backend")]
pub(crate) use native_batch::*;
#[cfg(all(feature = "native-backend", test))]
pub(crate) use shared_stdlib_cache::*;
