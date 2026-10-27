//! Platform support shared by the runtime fan-in crate.

pub mod env_support;
pub mod fnmatch;
pub mod importlib_support;
#[cfg(any(target_arch = "wasm32", test))]
pub mod libc_compat;
#[cfg(unix)]
pub mod msghdr;
pub mod path_text;
pub mod randomness;
pub mod socket_constants;
#[cfg(not(target_arch = "wasm32"))]
pub mod native_error_codes {
    include!(concat!(env!("OUT_DIR"), "/native_error_codes.rs"));
}
pub mod stat_support;
pub mod synthetic_errno;
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub mod userdb;
pub mod utils;
pub mod uuid_support;
pub mod wasm_socket_abi;
#[cfg(windows)]
pub mod windows_abi;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod errno_build_tests;
