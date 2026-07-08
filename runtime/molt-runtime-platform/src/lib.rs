//! Platform support shared by the runtime fan-in crate.

pub mod importlib_support;
#[cfg(target_arch = "wasm32")]
pub mod libc_compat;
pub mod randomness;
pub mod socket_constants;
pub mod utils;
#[cfg(windows)]
pub mod windows_abi;
